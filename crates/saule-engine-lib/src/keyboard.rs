//! Keyboard module — Love2D's `love.keyboard`, split between a queue and a
//! set of level queries.
//!
//! Love2D splits keyboard input in two: level queries (`love.keyboard.isDown`)
//! and callbacks (`love.keypressed`, `love.keyreleased`, `love.textinput`).
//! Saule owns the game loop here and the engine has no callback registry, so
//! the callbacks become *events*, drained by `Window.pollEvents()` — see
//! [`crate::event`]. What stays in this module is the level half, plus the
//! machinery the queue is built from: the ordered message log ([`LOG`]) and the
//! edge bitsets ([`EDGES`]) that drive held-key timing and repeat.
//!
//! Key names follow Love2D's `KeyConstant` strings — `"a"`, `"space"`,
//! `"lshift"`, `"return"`, `"kp0"`, `"/"` — so code reads the same as its
//! Love2D equivalent. Unrecognised names never panic: they simply report as
//! "not down".

use std::cell::{Cell, RefCell};
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
/// The edges are *not* derived by comparing level snapshots. The backend pumps
/// the OS queue at several points in a frame — `Window.pollEvents`,
/// `Graphics.present` — and a key tapped between two of them is already back up
/// by the time the next snapshot is taken, so diffing levels drops the press
/// entirely. That is invisible for letters (they arrive as text) but loses real
/// keystrokes for backspace and the arrows, which have no other channel.
///
/// So the edges come from [`EDGES`], latched inside the backend's key callback
/// as each key message is handled, and are drained here once per frame. Level
/// state is still read straight from the window, where it cannot go stale.
pub struct KeyState {
    down: [bool; KEY_SLOTS],
    /// Edges drained from [`EDGES`] for this frame.
    pressed: [bool; KEY_SLOTS],
    released: [bool; KEY_SLOTS],
    /// Seconds each key has been held, or `-1` while it is up.
    held: [f32; KEY_SLOTS],
    /// Keys whose repeat timer fired this frame. These are the engine's own
    /// invention rather than OS messages, so the event queue synthesises a
    /// `KeyPressed` for each after the frame's real ones.
    repeated: [bool; KEY_SLOTS],
    repeat: bool,
    last_sync: Instant,
}

impl Default for KeyState {
    fn default() -> Self {
        KeyState {
            down: [false; KEY_SLOTS],
            pressed: [false; KEY_SLOTS],
            released: [false; KEY_SLOTS],
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

        self.down = [false; KEY_SLOTS];
        for key in window.get_keys() {
            let i = key as usize;
            if i < KEY_SLOTS {
                self.down[i] = true;
            }
        }

        let (pressed, released) = drain_edges();
        self.pressed = pressed;
        self.released = released;

        for i in 0..KEY_SLOTS {
            self.repeated[i] = false;
            if !self.down[i] {
                self.held[i] = -1.0;
            } else if self.pressed[i] || self.held[i] < 0.0 {
                // A fresh press restarts the timer; so does finding a key
                // already down that we never saw go down (the window regaining
                // focus with a key held).
                self.held[i] = 0.0;
            } else {
                let before = self.held[i];
                let after = before + dt;
                self.held[i] = after;
                self.repeated[i] = self.repeat && repeat_fired(before, after);
            }
        }
    }

    /// The names of every key matching `pick`, in [`KEY_TABLE`] order.
    fn names_where(&self, pick: impl Fn(&Self, Key) -> bool) -> Vec<&'static str> {
        KEY_TABLE
            .iter()
            .filter(|(k, _)| pick(self, *k))
            .map(|(_, name)| *name)
            .collect()
    }

    /// Keys whose repeat timer fired this frame, in [`KEY_TABLE`] order.
    ///
    /// These are not OS messages, so they are absent from the ordered log and
    /// the event queue synthesises a `KeyPressed` for each. Holding backspace
    /// in a text field depends on it.
    pub fn repeated_names(&self) -> Vec<&'static str> {
        self.names_where(|s, k| s.repeated[k as usize])
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

// The `setTextInput` gate.
//
// This lives outside `crate::state::Engine` because minifb delivers characters
// from inside `window.update()` — i.e. while the engine is already mutably
// borrowed. The interpreter is single-threaded, so a `thread_local` is both
// sufficient and the only thing the callback can reach.
//
// There is no buffer any more: typed text leaves as a `TextInput` event, in
// order with the key messages around it, rather than as a pile drained whenever
// the app got round to asking.
thread_local! {
    // Love2D starts with text input enabled on desktop.
    static TEXT_ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Cap on the characters held in one coalesced `Text` message, so a program
/// that stops polling cannot grow it without bound.
const TEXT_LIMIT: usize = 4096;

/// Key edges seen since the last [`KeyState::sync`], recorded as the backend
/// handles each key message rather than sampled afterwards.
///
/// This is what makes a keystroke that begins and ends inside one frame
/// survive: by the time anything polls `get_keys()` the key is up again, and
/// the press would otherwise leave no trace. It lives beside [`TEXT`], and for
/// the same reason — the callback runs inside `window.update()`, with the
/// engine already mutably borrowed, so it cannot reach [`crate::state`].
struct EdgeState {
    pressed: [bool; KEY_SLOTS],
    released: [bool; KEY_SLOTS],
    /// The callback's own view of what is held, so the repeated `down`
    /// messages the OS sends for a held key are not mistaken for new presses.
    /// Key repeat is the engine's own business (see [`repeat_fired`]).
    live: [bool; KEY_SLOTS],
}

thread_local! {
    static EDGES: RefCell<EdgeState> = const {
        RefCell::new(EdgeState {
            pressed: [false; KEY_SLOTS],
            released: [false; KEY_SLOTS],
            live: [false; KEY_SLOTS],
        })
    };
}

/// One keyboard message, in the order the backend handed it over.
///
/// The [`EDGES`] bitsets answer "did this key go down at some point this
/// frame", which is all the level-state API ever needed. An event queue has to
/// say *in what order*, and has to keep two taps of the same key apart, so the
/// callback also appends here. Modifiers are captured at message time rather
/// than read back later, because by the end of the frame the shift key may well
/// be up again.
pub(crate) enum KeyMessage {
    Down {
        key: Key,
        mods: Modifiers,
    },
    Up {
        key: Key,
        mods: Modifiers,
    },
    /// A run of typed characters. Consecutive characters coalesce into one
    /// message, so a fast typist costs one event rather than one per letter.
    Text(String),
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Cap on unread messages, matching [`TEXT_LIMIT`]'s reasoning: a program that
/// stops polling must not grow this without bound.
const LOG_LIMIT: usize = 1024;

thread_local! {
    static LOG: RefCell<Vec<KeyMessage>> = const { RefCell::new(Vec::new()) };
}

/// Take this frame's keyboard messages in arrival order.
pub(crate) fn drain_messages() -> Vec<KeyMessage> {
    LOG.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Whether the given slot is held, per the callback's own live view.
fn live_any(edges: &EdgeState, keys: [Key; 2]) -> bool {
    keys.iter().any(|k| {
        let i = *k as usize;
        i < KEY_SLOTS && edges.live[i]
    })
}

fn current_mods(edges: &EdgeState) -> Modifiers {
    Modifiers {
        shift: live_any(edges, [Key::LeftShift, Key::RightShift]),
        ctrl: live_any(edges, [Key::LeftCtrl, Key::RightCtrl]),
        alt: live_any(edges, [Key::LeftAlt, Key::RightAlt]),
    }
}

fn push_message(msg: KeyMessage) {
    LOG.with(|cell| {
        let mut log = cell.borrow_mut();
        // Coalesce a run of characters into the message already at the back.
        if let (KeyMessage::Text(incoming), Some(KeyMessage::Text(tail))) = (&msg, log.last_mut()) {
            if tail.len() + incoming.len() <= TEXT_LIMIT {
                tail.push_str(incoming);
            }
            return;
        }
        if log.len() < LOG_LIMIT {
            log.push(msg);
        }
    });
}

/// Take this frame's edges, leaving the buffer empty for the next one.
fn drain_edges() -> ([bool; KEY_SLOTS], [bool; KEY_SLOTS]) {
    EDGES.with(|cell| {
        let mut edges = cell.borrow_mut();
        let pressed = std::mem::replace(&mut edges.pressed, [false; KEY_SLOTS]);
        let released = std::mem::replace(&mut edges.released, [false; KEY_SLOTS]);
        (pressed, released)
    })
}

/// Receives characters and key edges from the windowing backend. Installed by
/// [`crate::state::create`].
pub(crate) struct TextCollector;

impl InputCallback for TextCollector {
    fn add_char(&mut self, uni_char: u32) {
        let Some(ch) = char::from_u32(uni_char) else {
            return;
        };
        // Backspace, Return and friends arrive here on some backends; they are
        // key presses, not text, and already have their own `KeyPressed`.
        if ch.is_control() {
            return;
        }
        if TEXT_ENABLED.with(Cell::get) {
            push_message(KeyMessage::Text(ch.to_string()));
        }
    }

    fn set_key_state(&mut self, key: Key, state: bool) {
        let i = key as usize;
        if i >= KEY_SLOTS {
            return; // `Key::Unknown`, and anything a future backend adds
        }
        let message = EDGES.with(|cell| {
            let mut edges = cell.borrow_mut();
            if edges.live[i] == state {
                return None; // OS auto-repeat, or a duplicate message
            }
            edges.live[i] = state;
            if state {
                edges.pressed[i] = true;
            } else {
                edges.released[i] = true;
            }
            // Read the modifiers now: `live` is already updated, so a shift
            // press reports itself as shifted, matching how every other
            // toolkit reports it.
            let mods = current_mods(&edges);
            Some(if state {
                KeyMessage::Down { key, mods }
            } else {
                KeyMessage::Up { key, mods }
            })
        });

        if let Some(message) = message {
            push_message(message);
        }
    }
}

/// Drop the text and key edges buffered from a previous window session.
pub(crate) fn reset_input() {
    LOG.with(|cell| cell.borrow_mut().clear());
    EDGES.with(|cell| {
        *cell.borrow_mut() = EdgeState {
            pressed: [false; KEY_SLOTS],
            released: [false; KEY_SLOTS],
            live: [false; KEY_SLOTS],
        };
    });
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

/// `Keyboard.getKeysDown()` — the names of every key currently held.
#[saule_export(class = "Keyboard", name = "getKeysDown")]
fn keyboard_get_keys_down() -> Result<STable<SString>, String> {
    let names = state::with(|e| e.keys_held()).unwrap_or_default();
    names_table(&names)
}

/// `Keyboard.setKeyRepeat(enable)` — when enabled, a held key keeps emitting
/// `KeyPressed` events after a short delay, the way a text field wants. Off by
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
    TEXT_ENABLED.with(|enabled| enabled.set(enable));

    if !enable {
        // Drop text already queued this frame, so disabling mid-frame does not
        // deliver a keystroke the app has just said it does not want.
        LOG.with(|cell| {
            cell.borrow_mut()
                .retain(|m| !matches!(m, KeyMessage::Text(_)))
        });
    }
}

/// `Keyboard.hasTextInput()` — whether typed text is being collected.
#[saule_export(class = "Keyboard", name = "hasTextInput")]
fn keyboard_has_text_input() -> bool {
    TEXT_ENABLED.with(Cell::get)
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

    /// Stand-in for a `sync` without a live window: take the callback's edges
    /// and pair them with a level state supplied by the test.
    fn sync_with(keys: &mut KeyState, down: &[Key]) {
        keys.down = [false; KEY_SLOTS];
        for key in down {
            keys.down[*key as usize] = true;
        }
        let (pressed, released) = drain_edges();
        keys.pressed = pressed;
        keys.released = released;
    }

    /// Render the drained log compactly, so a test can assert on the whole
    /// frame's worth of messages in order.
    fn log_summary() -> Vec<String> {
        drain_messages()
            .into_iter()
            .map(|m| match m {
                KeyMessage::Down { key, mods } => {
                    format!("down:{}{}", key_name(key).unwrap_or("?"), mod_suffix(mods))
                }
                KeyMessage::Up { key, mods } => {
                    format!("up:{}{}", key_name(key).unwrap_or("?"), mod_suffix(mods))
                }
                KeyMessage::Text(t) => format!("text:{t}"),
            })
            .collect()
    }

    fn mod_suffix(mods: Modifiers) -> String {
        let mut out = String::new();
        if mods.shift {
            out.push_str("+shift");
        }
        if mods.ctrl {
            out.push_str("+ctrl");
        }
        if mods.alt {
            out.push_str("+alt");
        }
        out
    }

    #[test]
    fn key_messages_arrive_in_order_and_only_on_edges() {
        reset_input();
        let mut keys = KeyState::default();

        // Down, then the OS repeating the down message for a held key, then up.
        // Only the two real edges are messages.
        TextCollector.set_key_state(Key::Space, true);
        TextCollector.set_key_state(Key::Space, true);
        TextCollector.set_key_state(Key::Space, false);
        sync_with(&mut keys, &[]);

        assert_eq!(log_summary(), vec!["down:space", "up:space"]);
    }

    /// The ordering the state API could not express: two taps of the same key
    /// inside one frame, and text interleaved with the keys around it.
    #[test]
    fn a_double_tap_and_interleaved_text_keep_their_order() {
        reset_input();
        keyboard_set_text_input(true);

        TextCollector.set_key_state(Key::A, true);
        TextCollector.add_char('a' as u32);
        TextCollector.set_key_state(Key::A, false);
        TextCollector.set_key_state(Key::A, true);
        TextCollector.add_char('a' as u32);
        TextCollector.set_key_state(Key::A, false);

        assert_eq!(
            log_summary(),
            vec!["down:a", "text:a", "up:a", "down:a", "text:a", "up:a"]
        );
    }

    #[test]
    fn a_run_of_characters_coalesces_into_one_message() {
        reset_input();
        keyboard_set_text_input(true);

        for ch in "hello".chars() {
            TextCollector.add_char(ch as u32);
        }

        assert_eq!(log_summary(), vec!["text:hello"]);
    }

    #[test]
    fn modifiers_are_captured_when_the_message_arrives() {
        reset_input();

        // Shift goes down, then a letter, then shift comes up and another
        // letter follows. Reading modifiers at the end of the frame would call
        // both unshifted.
        TextCollector.set_key_state(Key::LeftShift, true);
        TextCollector.set_key_state(Key::A, true);
        TextCollector.set_key_state(Key::A, false);
        TextCollector.set_key_state(Key::LeftShift, false);
        TextCollector.set_key_state(Key::B, true);

        assert_eq!(
            log_summary(),
            vec![
                "down:lshift+shift",
                "down:a+shift",
                "up:a+shift",
                "up:lshift",
                "down:b",
            ]
        );
    }

    #[test]
    fn a_tap_inside_one_frame_still_reports_a_press() {
        // The regression this whole mechanism exists for: press and release
        // both land between two frames, so every level snapshot says "up".
        // Diffing levels loses the keystroke; recording messages keeps it.
        reset_input();

        TextCollector.set_key_state(Key::Backspace, true);
        TextCollector.set_key_state(Key::Backspace, false);

        assert_eq!(log_summary(), vec!["down:backspace", "up:backspace"]);
        // And draining is what clears it — the next frame starts empty.
        assert!(log_summary().is_empty());
    }

    #[test]
    fn messages_do_not_leak_between_window_sessions() {
        reset_input();
        TextCollector.set_key_state(Key::Escape, true);
        reset_input();

        assert!(log_summary().is_empty());
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
        reset_input();
        keyboard_set_text_input(false);
        TextCollector.add_char('x' as u32);
        assert!(log_summary().is_empty());

        keyboard_set_text_input(true);
        TextCollector.add_char('h' as u32);
        TextCollector.add_char('i' as u32);
        // Control characters are key presses, not text.
        TextCollector.add_char('\n' as u32);
        assert_eq!(log_summary(), vec!["text:hi"]);
    }

    #[test]
    fn disabling_text_input_discards_what_was_already_typed_this_frame() {
        reset_input();
        keyboard_set_text_input(true);
        TextCollector.set_key_state(Key::A, true);
        TextCollector.add_char('a' as u32);

        keyboard_set_text_input(false);

        // The keystroke stays — it is a key press either way — but the text
        // the app just said it did not want is gone.
        assert_eq!(log_summary(), vec!["down:a"]);
        keyboard_set_text_input(true);
    }

    #[test]
    fn a_repeating_key_reports_its_name_for_the_event_queue() {
        let mut keys = KeyState::default();
        keys.set_repeat(true);
        keys.repeated[Key::Backspace as usize] = true;

        assert_eq!(keys.repeated_names(), vec!["backspace"]);
    }
}
