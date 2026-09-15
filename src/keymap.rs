//! A minimal US-QWERTY evdev keycode -> character mapping.
//!
//! This is intentionally small: a greeter only needs to accept a username
//! and a password, not full IME/locale-aware text input. Layout could be
//! made configurable in the future by swapping this table out.

use evdev::KeyCode;

pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Shutdown,
    Reboot,
}

pub fn map_key(code: KeyCode, shift: bool) -> Option<Key> {
    let plain_shift = |lo: char, hi: char| Some(Key::Char(if shift { hi } else { lo }));

    match code {
        KeyCode::KEY_A => plain_shift('a', 'A'),
        KeyCode::KEY_B => plain_shift('b', 'B'),
        KeyCode::KEY_C => plain_shift('c', 'C'),
        KeyCode::KEY_D => plain_shift('d', 'D'),
        KeyCode::KEY_E => plain_shift('e', 'E'),
        KeyCode::KEY_F => plain_shift('f', 'F'),
        KeyCode::KEY_G => plain_shift('g', 'G'),
        KeyCode::KEY_H => plain_shift('h', 'H'),
        KeyCode::KEY_I => plain_shift('i', 'I'),
        KeyCode::KEY_J => plain_shift('j', 'J'),
        KeyCode::KEY_K => plain_shift('k', 'K'),
        KeyCode::KEY_L => plain_shift('l', 'L'),
        KeyCode::KEY_M => plain_shift('m', 'M'),
        KeyCode::KEY_N => plain_shift('n', 'N'),
        KeyCode::KEY_O => plain_shift('o', 'O'),
        KeyCode::KEY_P => plain_shift('p', 'P'),
        KeyCode::KEY_Q => plain_shift('q', 'Q'),
        KeyCode::KEY_R => plain_shift('r', 'R'),
        KeyCode::KEY_S => plain_shift('s', 'S'),
        KeyCode::KEY_T => plain_shift('t', 'T'),
        KeyCode::KEY_U => plain_shift('u', 'U'),
        KeyCode::KEY_V => plain_shift('v', 'V'),
        KeyCode::KEY_W => plain_shift('w', 'W'),
        KeyCode::KEY_X => plain_shift('x', 'X'),
        KeyCode::KEY_Y => plain_shift('y', 'Y'),
        KeyCode::KEY_Z => plain_shift('z', 'Z'),

        KeyCode::KEY_1 => plain_shift('1', '!'),
        KeyCode::KEY_2 => plain_shift('2', '@'),
        KeyCode::KEY_3 => plain_shift('3', '#'),
        KeyCode::KEY_4 => plain_shift('4', '$'),
        KeyCode::KEY_5 => plain_shift('5', '%'),
        KeyCode::KEY_6 => plain_shift('6', '^'),
        KeyCode::KEY_7 => plain_shift('7', '&'),
        KeyCode::KEY_8 => plain_shift('8', '*'),
        KeyCode::KEY_9 => plain_shift('9', '('),
        KeyCode::KEY_0 => plain_shift('0', ')'),

        KeyCode::KEY_MINUS => plain_shift('-', '_'),
        KeyCode::KEY_EQUAL => plain_shift('=', '+'),
        KeyCode::KEY_LEFTBRACE => plain_shift('[', '{'),
        KeyCode::KEY_RIGHTBRACE => plain_shift(']', '}'),
        KeyCode::KEY_BACKSLASH => plain_shift('\\', '|'),
        KeyCode::KEY_SEMICOLON => plain_shift(';', ':'),
        KeyCode::KEY_APOSTROPHE => plain_shift('\'', '"'),
        KeyCode::KEY_GRAVE => plain_shift('`', '~'),
        KeyCode::KEY_COMMA => plain_shift(',', '<'),
        KeyCode::KEY_DOT => plain_shift('.', '>'),
        KeyCode::KEY_SLASH => plain_shift('/', '?'),
        KeyCode::KEY_SPACE => Some(Key::Char(' ')),

        KeyCode::KEY_BACKSPACE => Some(Key::Backspace),
        KeyCode::KEY_DELETE => Some(Key::Delete),
        KeyCode::KEY_ENTER | KeyCode::KEY_KPENTER => Some(Key::Enter),
        KeyCode::KEY_TAB => Some(Key::Tab),
        KeyCode::KEY_ESC => Some(Key::Escape),
        KeyCode::KEY_LEFT => Some(Key::Left),
        KeyCode::KEY_RIGHT => Some(Key::Right),
        KeyCode::KEY_UP => Some(Key::Up),
        KeyCode::KEY_DOWN => Some(Key::Down),
        KeyCode::KEY_F1 => Some(Key::Shutdown),
        KeyCode::KEY_F2 => Some(Key::Reboot),

        _ => None,
    }
}

pub fn is_shift(code: KeyCode) -> bool {
    matches!(code, KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT)
}
