use minifb::Key;
use saule_sdk::saule_export;

use crate::state;

#[saule_export(class = "Keyboard", name = "isDown")]
pub(crate) fn keyboard_is_down(key: String) -> bool {
    match parse_key(&key) {
        Some(k) => state::with(|e| e.is_key_down(k)).unwrap_or(false),
        None => false,
    }
}

/// Map a Love2D-style key name to a minifb `Key`. Returns `None` for
/// unrecognised names so callers can degrade gracefully.
pub(crate) fn parse_key(s: &str) -> Option<Key> {
    Some(match s {
        // ── Letters ──────────────────────────────────────────────────────
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        // ── Digits ───────────────────────────────────────────────────────
        "0" => Key::Key0,
        "1" => Key::Key1,
        "2" => Key::Key2,
        "3" => Key::Key3,
        "4" => Key::Key4,
        "5" => Key::Key5,
        "6" => Key::Key6,
        "7" => Key::Key7,
        "8" => Key::Key8,
        "9" => Key::Key9,
        // ── Special ──────────────────────────────────────────────────────
        "space" => Key::Space,
        "return" | "enter" => Key::Enter,
        "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        // ── Arrows ───────────────────────────────────────────────────────
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        // ── Modifiers ────────────────────────────────────────────────────
        "lshift" => Key::LeftShift,
        "rshift" => Key::RightShift,
        "lctrl" => Key::LeftCtrl,
        "rctrl" => Key::RightCtrl,
        "lalt" => Key::LeftAlt,
        "ralt" => Key::RightAlt,
        // ── Function keys ────────────────────────────────────────────────
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ => return None,
    })
}
