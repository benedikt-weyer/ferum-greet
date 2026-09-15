//! Raw evdev keyboard input, read directly from `/dev/input/event*` - no
//! compositor or seat daemon required, only membership in the `input` group
//! (the NixOS module grants this to the greeter's user).

use std::sync::mpsc::Sender;
use std::thread;

use evdev::{EventSummary, EventType, KeyCode};

use crate::keymap::{self, Key};

pub enum InputEvent {
    Key(Key),
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
        thread::spawn(move || run_reader(device, tx));
    }
    if found == 0 {
        log::warn!("no keyboard devices found under /dev/input - is the greeter user in the 'input' group?");
    }
}

fn run_reader(mut device: evdev::Device, tx: Sender<InputEvent>) {
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
            if let Some(key) = keymap::map_key(code, shift_held) {
                if tx.send(InputEvent::Key(key)).is_err() {
                    return;
                }
            }
        }
    }
}
