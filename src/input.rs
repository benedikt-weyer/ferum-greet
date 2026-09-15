//! Raw evdev keyboard and pointer input, read directly from
//! `/dev/input/event*` - no compositor or seat daemon required, only
//! membership in the `input` group (the NixOS module grants this to the
//! greeter's user).

use std::sync::mpsc::Sender;
use std::thread;

use evdev::{EventSummary, EventType, KeyCode, RelativeAxisCode};

use crate::keymap::{self, Key};

pub enum InputEvent {
    Key(Key),
    /// Relative pointer motion, in device units (roughly pixels).
    MouseMove { dx: f32, dy: f32 },
    /// The primary (left) mouse button changed state.
    MouseButton { pressed: bool },
}

/// Spawns one reader thread per detected keyboard device and forwards
/// decoded key presses to `tx`. Devices that appear later (e.g. USB
/// keyboards hotplugged after startup) are not picked up; that's a
/// reasonable simplification for a greeter running for a short, bounded
/// session.
pub fn spawn_keyboard_readers(tx: Sender<InputEvent>) {
    let mut found = 0;
    for (path, mut device) in evdev::enumerate() {
        let is_keyboard = device
            .supported_events()
            .contains(EventType::KEY)
            && device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::KEY_ENTER));
        if !is_keyboard {
            continue;
        }
        // Grab the device exclusively so the kernel VT keyboard driver
        // doesn't also see these events - without this, typed characters
        // (including the password) get echoed in plaintext to the
        // greeter's console/tty.
        if let Err(err) = device.grab() {
            log::warn!("could not grab keyboard device {} ({err}) - input may echo to the console", path.display());
        }
        found += 1;
        let tx = tx.clone();
        log::info!("reading keyboard input from {}", path.display());
        thread::spawn(move || run_keyboard_reader(device, tx));
    }
    if found == 0 {
        log::warn!("no keyboard devices found under /dev/input - is the greeter user in the 'input' group?");
    }
}

fn run_keyboard_reader(mut device: evdev::Device, tx: Sender<InputEvent>) {
    let mut shift_held = false;
    loop {
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(err) => {
                log::warn!("keyboard read error: {err}");
                return;
            }
        };
        for ev in events {
            let EventSummary::Key(_, code, value) = ev.destructure() else {
                continue;
            };
            // value: 0 = release, 1 = press, 2 = autorepeat.
            if keymap::is_shift(code) {
                shift_held = value != 0;
                continue;
            }
            if value == 0 {
                continue;
            }
            if let Some(key) = keymap::map_key(code, shift_held)
                && tx.send(InputEvent::Key(key)).is_err()
            {
                return;
            }
        }
    }
}

/// Spawns one reader thread per detected mouse (a device with relative X/Y
/// motion and a left button - trackpads report the same shape once the
/// kernel's libinput-less pointer emulation is in play, so this picks those
/// up too). A greeter with no pointer attached simply drives none of these
/// threads; the cursor stays hidden until one shows up.
pub fn spawn_pointer_readers(tx: Sender<InputEvent>) {
    let mut found = 0;
    for (path, mut device) in evdev::enumerate() {
        let is_pointer = device
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y))
            && device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
        if !is_pointer {
            continue;
        }
        if let Err(err) = device.grab() {
            log::warn!("could not grab pointer device {} ({err})", path.display());
        }
        found += 1;
        let tx = tx.clone();
        log::info!("reading pointer input from {}", path.display());
        thread::spawn(move || run_pointer_reader(device, tx));
    }
    if found == 0 {
        log::info!("no pointer devices found under /dev/input - mouse input disabled");
    }
}

fn run_pointer_reader(mut device: evdev::Device, tx: Sender<InputEvent>) {
    loop {
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(err) => {
                log::warn!("pointer read error: {err}");
                return;
            }
        };
        for ev in events {
            let sent = match ev.destructure() {
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, v) => {
                    tx.send(InputEvent::MouseMove { dx: v as f32, dy: 0.0 })
                }
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, v) => {
                    tx.send(InputEvent::MouseMove { dx: 0.0, dy: v as f32 })
                }
                EventSummary::Key(_, KeyCode::BTN_LEFT, value) => {
                    tx.send(InputEvent::MouseButton { pressed: value != 0 })
                }
                _ => continue,
            };
            if sent.is_err() {
                return;
            }
        }
    }
}
