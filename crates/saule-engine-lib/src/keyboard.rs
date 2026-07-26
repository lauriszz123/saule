//! Keyboard module — a polling take on Love2D's `love.keyboard`.
//!
//! Love2D splits keyboard input in two: level queries (`love.keyboard.isDown`)
//! and callbacks (`love.keypressed`, `love.keyreleased`, `love.textinput`).
//! Saule owns the game loop here and the engine has no callback registry, so
//! the callbacks become per-frame *queries*: `wasPressed` / `wasReleased` and
//! `getTextInput`. All three are relative to the last `Window.pollEvents()`,
//! which is the frame boundary the loop already has.
//!
//! Key names follow Love2D's `KeyConstant` strings — `"a"`, `"space"`,
//! `"lshift"`, `"return"`, `"kp0"`, `"/"` — so code reads the same as its
//! Love2D equivalent. Unrecognised names never panic: they simply report as
//! "not down".

use std::cell::RefCell;
use std::time::Instant;

use minifb::{InputCallback, Key, Window};
use saule_sdk::prelude::*;
use saule_sdk::saule_export;

use crate::state;

// ---------------------------------------------------------------------------
// Key names
// ---------------------------------------------------------------------------

/// Every key the backend can report, paired with its canonical Love2D name.
///
/// This is the source of truth for the `getKeys*` accessors, and [`parse_key`]
/// mirrors it in the other direction (a unit test asserts the two agree).
const KEY_TABLE: &[(Key, &str)] = &[
    // ── Digits ───────────────────────────────────────────────────────────
    (Key::Key0, "0"),
    (Key::Key1, "1"),
    (Key::Key2, "2"),
    (Key::Key3, "3"),
    (Key::Key4, "4"),
    (Key::Key5, "5"),
    (Key::Key6, "6"),
    (Key::Key7, "7"),
    (Key::Key8, "8"),
    (Key::Key9, "9"),
    // ── Letters ──────────────────────────────────────────────────────────
    (Key::A, "a"),
    (Key::B, "b"),
    (Key::C, "c"),
    (Key::D, "d"),
    (Key::E, "e"),
    (Key::F, "f"),
    (Key::G, "g"),
    (Key::H, "h"),
    (Key::I, "i"),
    (Key::J, "j"),
    (Key::K, "k"),
    (Key::L, "l"),
    (Key::M, "m"),
    (Key::N, "n"),
    (Key::O, "o"),
    (Key::P, "p"),
    (Key::Q, "q"),
    (Key::R, "r"),
    (Key::S, "s"),
    (Key::T, "t"),
    (Key::U, "u"),
    (Key::V, "v"),
    (Key::W, "w"),
    (Key::X, "x"),
    (Key::Y, "y"),
    (Key::Z, "z"),
    // ── Function keys ────────────────────────────────────────────────────
    (Key::F1, "f1"),
    (Key::F2, "f2"),
    (Key::F3, "f3"),
    (Key::F4, "f4"),
    (Key::F5, "f5"),
    (Key::F6, "f6"),
    (Key::F7, "f7"),
    (Key::F8, "f8"),
    (Key::F9, "f9"),
    (Key::F10, "f10"),
    (Key::F11, "f11"),
    (Key::F12, "f12"),
    (Key::F13, "f13"),
    (Key::F14, "f14"),
    (Key::F15, "f15"),
    // ── Arrows ───────────────────────────────────────────────────────────
    (Key::Up, "up"),
    (Key::Down, "down"),
    (Key::Left, "left"),
    (Key::Right, "right"),
    // ── Punctuation (Love2D names these by the character) ────────────────
    (Key::Apostrophe, "'"),
    (Key::Backquote, "`"),
    (Key::Backslash, "\\"),
    (Key::Comma, ","),
    (Key::Equal, "="),
    (Key::LeftBracket, "["),
    (Key::Minus, "-"),
    (Key::Period, "."),
    (Key::RightBracket, "]"),
    (Key::Semicolon, ";"),
    (Key::Slash, "/"),
    // ── Editing and navigation ───────────────────────────────────────────
    (Key::Space, "space"),
    (Key::Enter, "return"),
    (Key::Tab, "tab"),
    (Key::Backspace, "backspace"),
    (Key::Delete, "delete"),
    (Key::Insert, "insert"),
    (Key::Home, "home"),
    (Key::End, "end"),
    (Key::PageUp, "pageup"),
    (Key::PageDown, "pagedown"),
    (Key::Escape, "escape"),
    (Key::Pause, "pause"),
    (Key::Menu, "menu"),
    // ── Locks ────────────────────────────────────────────────────────────
    (Key::CapsLock, "capslock"),
    (Key::NumLock, "numlock"),
    (Key::ScrollLock, "scrolllock"),
    // ── Modifiers ────────────────────────────────────────────────────────
    (Key::LeftShift, "lshift"),
    (Key::RightShift, "rshift"),
    (Key::LeftCtrl, "lctrl"),
    (Key::RightCtrl, "rctrl"),
    (Key::LeftAlt, "lalt"),
    (Key::RightAlt, "ralt"),
    (Key::LeftSuper, "lgui"),
    (Key::RightSuper, "rgui"),
    // ── Keypad ───────────────────────────────────────────────────────────
    (Key::NumPad0, "kp0"),
    (Key::NumPad1, "kp1"),
    (Key::NumPad2, "kp2"),
    (Key::NumPad3, "kp3"),
    (Key::NumPad4, "kp4"),
    (Key::NumPad5, "kp5"),
    (Key::NumPad6, "kp6"),
    (Key::NumPad7, "kp7"),
    (Key::NumPad8, "kp8"),
    (Key::NumPad9, "kp9"),
    (Key::NumPadDot, "kp."),
    (Key::NumPadSlash, "kp/"),
    (Key::NumPadAsterisk, "kp*"),
    (Key::NumPadMinus, "kp-"),
    (Key::NumPadPlus, "kp+"),
    (Key::NumPadEnter, "kpenter"),
];

/// The canonical Love2D name for a key, or `None` for keys the backend cannot
/// name (`Key::Unknown`).
pub(crate) fn key_name(key: Key) -> Option<&'static str> {
    KEY_TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, name)| *name)
}

/// Map a Love2D-style key name to a minifb [`Key`]. Returns `None` for
/// unrecognised names so callers can degrade gracefully.
///
/// Accepts the canonical names from [`KEY_TABLE`] plus a few spelled-out
/// aliases for the punctuation keys, where typing the character itself reads
/// poorly.
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
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "pause" => Key::Pause,
        "menu" => Key::Menu,
        // ── Locks ────────────────────────────────────────────────────────
        "capslock" => Key::CapsLock,
        "numlock" => Key::NumLock,
        "scrolllock" => Key::ScrollLock,
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
        "lgui" | "lsuper" | "lwin" => Key::LeftSuper,
        "rgui" | "rsuper" | "rwin" => Key::RightSuper,
        // ── Punctuation, by character and by name ────────────────────────
        "'" | "apostrophe" | "quote" => Key::Apostrophe,
        "`" | "backquote" | "grave" => Key::Backquote,
        "\\" | "backslash" => Key::Backslash,
        "," | "comma" => Key::Comma,
        "=" | "equal" | "equals" => Key::Equal,
        "[" | "leftbracket" => Key::LeftBracket,
        "-" | "minus" => Key::Minus,
        "." | "period" => Key::Period,
        "]" | "rightbracket" => Key::RightBracket,
        ";" | "semicolon" => Key::Semicolon,
        "/" | "slash" => Key::Slash,
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
        "f13" => Key::F13,
        "f14" => Key::F14,
        "f15" => Key::F15,
        // ── Keypad ───────────────────────────────────────────────────────
        "kp0" => Key::NumPad0,
        "kp1" => Key::NumPad1,
        "kp2" => Key::NumPad2,
        "kp3" => Key::NumPad3,
        "kp4" => Key::NumPad4,
        "kp5" => Key::NumPad5,
        "kp6" => Key::NumPad6,
        "kp7" => Key::NumPad7,
        "kp8" => Key::NumPad8,
        "kp9" => Key::NumPad9,
        "kp." => Key::NumPadDot,
        "kp/" => Key::NumPadSlash,
        "kp*" => Key::NumPadAsterisk,
        "kp-" => Key::NumPadMinus,
        "kp+" => Key::NumPadPlus,
        "kpenter" => Key::NumPadEnter,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Per-frame edge tracking
// ---------------------------------------------------------------------------

/// Slots in the backend's key table; `Key::Count` is its sentinel last variant.
const KEY_SLOTS: usize = Key::Count as usize;

/// Seconds a key must be held before key repeat starts, and the interval
/// between repeats after that. These match the backend's (and most desktops')
/// defaults.
const REPEAT_DELAY: f32 = 0.25;
const REPEAT_RATE: f32 = 0.05;

/// Which keys went down or came up between the last two frames.
///
/// minifb has its own `is_key_pressed` / `is_key_released`, but those edges are
/// relative to *its* update count, and the engine pumps the OS queue twice per
/// frame (`Window.pollEvents` and `Graphics.present`). Roughly half the edges
/// would therefore land outside the window a program can observe. Snapshotting
/// the level state once per `pollEvents` instead makes an edge mean exactly
/// "since the previous frame", however often the backend updates.
pub struct KeyState {
    down: [bool; KEY_SLOTS],
    prev: [bool; KEY_SLOTS],
    /// Seconds each key has been held, or `-1` while it is up.
    held: [f32; KEY_SLOTS],
    /// Keys whose repeat timer fired this frame; `wasPressed` reports these
    /// alongside genuine presses.
    repeated: [bool; KEY_SLOTS],
    repeat: bool,
    last_sync: Instant,
}

impl Default for KeyState {
    fn default() -> Self {
        KeyState {
            down: [false; KEY_SLOTS],
            prev: [false; KEY_SLOTS],
            held: [-1.0; KEY_SLOTS],
            repeated: [false; KEY_SLOTS],
            // Love2D's `love.keyboard.setKeyRepeat` defaults to off.
            repeat: false,
            last_sync: Instant::now(),
        }
    }
}

impl KeyState {
    /// Latch this frame's key states. Called once per `Window.pollEvents`.
    pub fn sync(&mut self, window: &Window) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_sync).as_secs_f32();
        self.last_sync = now;

        self.prev = self.down;
        self.down = [false; KEY_SLOTS];
        for key in window.get_keys() {
            let i = key as usize;
            if i < KEY_SLOTS {
                self.down[i] = true;
            }
        }

        for i in 0..KEY_SLOTS {
            self.repeated[i] = false;
            if !self.down[i] {
                self.held[i] = -1.0;
            } else if !self.prev[i] {
                self.held[i] = 0.0;
            } else {
                let before = self.held[i];
                let after = before + dt;
                self.held[i] = after;
                self.repeated[i] = self.repeat && repeat_fired(before, after);
            }
        }
    }

    /// `true` if the key went down this frame, or its repeat timer fired.
    pub fn is_pressed(&self, key: Key) -> bool {
        let i = key as usize;
        (self.down[i] && !self.prev[i]) || self.repeated[i]
    }

    /// `true` if the key came up this frame.
    pub fn is_released(&self, key: Key) -> bool {
        let i = key as usize;
        !self.down[i] && self.prev[i]
    }

    /// The names of every key matching `pick`, in [`KEY_TABLE`] order.
    fn names_where(&self, pick: impl Fn(&Self, Key) -> bool) -> Vec<&'static str> {
        KEY_TABLE
            .iter()
            .filter(|(k, _)| pick(self, *k))
            .map(|(_, name)| *name)
            .collect()
    }

    pub fn set_repeat(&mut self, enabled: bool) {
        self.repeat = enabled;
    }

    pub fn repeat(&self) -> bool {
        self.repeat
    }
}

/// Whether the repeat timer for a key held from `before` to `after` seconds
/// crossed a repeat boundary this frame.
fn repeat_fired(before: f32, after: f32) -> bool {
    if after < REPEAT_DELAY {
        return false;
    }
    // The first repeat fires as the key crosses the initial delay.
    if before < REPEAT_DELAY {
        return true;
    }
    ((after - REPEAT_DELAY) / REPEAT_RATE).floor() > ((before - REPEAT_DELAY) / REPEAT_RATE).floor()
}

// ---------------------------------------------------------------------------
// Text input
// ---------------------------------------------------------------------------

/// Text typed since the last `Keyboard.getTextInput()`, plus the `setTextInput`
/// gate.
///
/// This lives outside [`crate::state::Engine`] because minifb delivers
/// characters from inside `window.update()` — i.e. while the engine is already
/// mutably borrowed. The interpreter is single-threaded, so a `thread_local` is
/// both sufficient and the only thing the callback can reach.
struct TextState {
    enabled: bool,
    buf: String,
}

thread_local! {
    static TEXT: RefCell<TextState> = const {
        RefCell::new(TextState {
            // Love2D starts with text input enabled on desktop.
            enabled: true,
            buf: String::new(),
        })
    };
}

/// Cap on unread typed text, so a program that never drains the buffer cannot
/// grow it without bound.
const TEXT_LIMIT: usize = 4096;

/// Receives characters from the windowing backend. Installed by
/// [`crate::state::create`].
pub(crate) struct TextCollector;

impl InputCallback for TextCollector {
    fn add_char(&mut self, uni_char: u32) {
        let Some(ch) = char::from_u32(uni_char) else {
            return;
        };
        // Backspace, Return and friends arrive here on some backends; they are
        // key presses, not text, and `wasPressed` already reports them.
        if ch.is_control() {
            return;
        }
        TEXT.with(|cell| {
            let mut text = cell.borrow_mut();
            if text.enabled && text.buf.len() + ch.len_utf8() <= TEXT_LIMIT {
                text.buf.push(ch);
            }
        });
    }
}

/// Drop any text buffered from a previous window session.
pub(crate) fn reset_text() {
    TEXT.with(|cell| cell.borrow_mut().buf.clear());
}

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

/// `Keyboard.isDown(key)` — `true` while the key is held. Unknown key names
/// report `false`.
#[saule_export(class = "Keyboard", name = "isDown")]
pub(crate) fn keyboard_is_down(key: String) -> bool {
    key_is_down(&key)
}

/// `Keyboard.isAnyDown({"lshift", "rshift"})` — `true` if any of the named keys
/// is held. This is the engine's stand-in for Love2D's variadic
/// `love.keyboard.isDown(key, ...)`.
#[saule_export(class = "Keyboard", name = "isAnyDown")]
fn keyboard_is_any_down(keys: STable<SString>) -> Result<bool, String> {
    for value in keys.to_vec()? {
        let Some(name) = value.as_str() else {
            return Err("Keyboard.isAnyDown: the table must hold key-name strings".to_string());
        };
        if key_is_down(name) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `Keyboard.wasPressed(key)` — `true` if the key went down since the previous
/// `Window.pollEvents()`. The polling equivalent of Love2D's `love.keypressed`
/// callback; with `setKeyRepeat(true)` it also fires on repeats.
#[saule_export(class = "Keyboard", name = "wasPressed")]
fn keyboard_was_pressed(key: String) -> bool {
    match parse_key(&key) {
        Some(k) => state::with(|e| e.keys().is_pressed(k)).unwrap_or(false),
        None => false,
    }
}

/// `Keyboard.wasReleased(key)` — `true` if the key came up since the previous
/// `Window.pollEvents()`. The polling equivalent of `love.keyreleased`.
#[saule_export(class = "Keyboard", name = "wasReleased")]
fn keyboard_was_released(key: String) -> bool {
    match parse_key(&key) {
        Some(k) => state::with(|e| e.keys().is_released(k)).unwrap_or(false),
        None => false,
    }
}

/// `Keyboard.getKeysDown()` — the names of every key currently held.
#[saule_export(class = "Keyboard", name = "getKeysDown")]
fn keyboard_get_keys_down() -> Result<STable<SString>, String> {
    let names = state::with(|e| e.keys_held()).unwrap_or_default();
    names_table(&names)
}

/// `Keyboard.getKeysPressed()` — the names of every key that went down this
/// frame, in the same order `getKeysDown` uses.
#[saule_export(class = "Keyboard", name = "getKeysPressed")]
fn keyboard_get_keys_pressed() -> Result<STable<SString>, String> {
    let names = state::with(|e| e.keys().names_where(KeyState::is_pressed)).unwrap_or_default();
    names_table(&names)
}

/// `Keyboard.getKeysReleased()` — the names of every key that came up this
/// frame.
#[saule_export(class = "Keyboard", name = "getKeysReleased")]
fn keyboard_get_keys_released() -> Result<STable<SString>, String> {
    let names = state::with(|e| e.keys().names_where(KeyState::is_released)).unwrap_or_default();
    names_table(&names)
}

/// `Keyboard.setKeyRepeat(enable)` — when enabled, a held key keeps reporting
/// `wasPressed` after a short delay, the way a text field wants. Off by
/// default, matching Love2D.
#[saule_export(class = "Keyboard", name = "setKeyRepeat")]
fn keyboard_set_key_repeat(enable: bool) -> Result<(), String> {
    state::with(|e| e.keys_mut().set_repeat(enable))
}

/// `Keyboard.hasKeyRepeat()` — whether key repeat is enabled.
#[saule_export(class = "Keyboard", name = "hasKeyRepeat")]
fn keyboard_has_key_repeat() -> Result<bool, String> {
    state::with(|e| e.keys().repeat())
}

/// `Keyboard.setTextInput(enable)` — start or stop collecting typed text.
/// Disabling it also drops whatever has been buffered.
#[saule_export(class = "Keyboard", name = "setTextInput")]
fn keyboard_set_text_input(enable: bool) {
    TEXT.with(|cell| {
        let mut text = cell.borrow_mut();
        text.enabled = enable;
        if !enable {
            text.buf.clear();
        }
    });
}

/// `Keyboard.hasTextInput()` — whether typed text is being collected.
#[saule_export(class = "Keyboard", name = "hasTextInput")]
fn keyboard_has_text_input() -> bool {
    TEXT.with(|cell| cell.borrow().enabled)
}

/// `Keyboard.getTextInput()` — the text typed since the last call, and clears
/// the buffer. This is the polling equivalent of Love2D's `love.textinput`
/// callback: it is layout- and modifier-aware (so `shift`+`a` gives `"A"`),
/// unlike the raw key names `wasPressed` reports. Returns `""` when nothing was
/// typed or when `setTextInput(false)` is in effect.
#[saule_export(class = "Keyboard", name = "getTextInput")]
fn keyboard_get_text_input() -> String {
    TEXT.with(|cell| std::mem::take(&mut cell.borrow_mut().buf))
}

/// Shared by `isDown` and `isAnyDown`: level state straight from the window,
/// so it is fresh even between frames.
fn key_is_down(name: &str) -> bool {
    match parse_key(name) {
        Some(k) => state::with(|e| e.is_key_down(k)).unwrap_or(false),
        None => false,
    }
}

/// Pack key names into a fresh Saule `table<string>`.
fn names_table(names: &[&'static str]) -> Result<STable<SString>, String> {
    let table = STable::new();
    for name in names {
        table.push(*name)?;
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_canonical_name_parses_back_to_its_key() {
        // Guards the two directions — `KEY_TABLE` and `parse_key` — from
        // drifting apart as keys are added.
        for (key, name) in KEY_TABLE {
            assert_eq!(parse_key(name), Some(*key), "key name `{name}`");
        }
    }

    #[test]
    fn key_names_are_unique() {
        for (i, (_, name)) in KEY_TABLE.iter().enumerate() {
            assert!(
                !KEY_TABLE[..i].iter().any(|(_, other)| other == name),
                "duplicate key name `{name}`"
            );
        }
    }

    #[test]
    fn unknown_key_has_no_name() {
        assert_eq!(key_name(Key::Unknown), None);
        assert_eq!(key_name(Key::Space), Some("space"));
    }

    #[test]
    fn aliases_resolve_to_the_canonical_key() {
        assert_eq!(parse_key("enter"), parse_key("return"));
        assert_eq!(parse_key("esc"), parse_key("escape"));
        assert_eq!(parse_key("comma"), parse_key(","));
        assert_eq!(parse_key("lsuper"), parse_key("lgui"));
        assert_eq!(parse_key("hyperspace"), None);
    }

    #[test]
    fn edges_are_relative_to_the_previous_frame() {
        let mut keys = KeyState::default();
        // Frame 1: the key goes down.
        keys.prev = keys.down;
        keys.down[Key::Space as usize] = true;
        assert!(keys.is_pressed(Key::Space));
        assert!(!keys.is_released(Key::Space));

        // Frame 2: still held — a press is an edge, not a level.
        keys.prev = keys.down;
        assert!(!keys.is_pressed(Key::Space));

        // Frame 3: released.
        keys.prev = keys.down;
        keys.down[Key::Space as usize] = false;
        assert!(keys.is_released(Key::Space));
        assert!(!keys.is_pressed(Key::Space));
    }

    #[test]
    fn repeat_fires_after_the_delay_then_at_the_repeat_rate() {
        // Nothing before the initial delay elapses.
        assert!(!repeat_fired(0.0, REPEAT_DELAY - 0.01));
        // The first repeat lands as the delay is crossed.
        assert!(repeat_fired(REPEAT_DELAY - 0.01, REPEAT_DELAY + 0.01));
        // Then once per rate interval, not once per frame.
        assert!(!repeat_fired(
            REPEAT_DELAY + 0.01,
            REPEAT_DELAY + REPEAT_RATE * 0.5
        ));
        assert!(repeat_fired(
            REPEAT_DELAY + 0.01,
            REPEAT_DELAY + REPEAT_RATE + 0.01
        ));
    }

    #[test]
    fn text_input_gate_drops_characters_while_disabled() {
        keyboard_set_text_input(false);
        TextCollector.add_char('x' as u32);
        assert_eq!(keyboard_get_text_input(), "");

        keyboard_set_text_input(true);
        TextCollector.add_char('h' as u32);
        TextCollector.add_char('i' as u32);
        // Control characters are key presses, not text.
        TextCollector.add_char('\n' as u32);
        assert_eq!(keyboard_get_text_input(), "hi");
        // Reading drains the buffer.
        assert_eq!(keyboard_get_text_input(), "");
    }
}
