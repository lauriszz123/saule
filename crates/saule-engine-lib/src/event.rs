//! The per-frame event queue — what `Window.pollEvents()` hands back.
//!
//! The engine used to answer input only as *state*: "is this key down", "was it
//! pressed since last frame". That cannot express order, and it collapses two
//! taps of the same key inside one frame into one. An event queue can, so this
//! is the primary input API and the level queries (`Keyboard.isDown`,
//! `Mouse.getPos`) are what remain of the old one — they answer a question
//! events genuinely cannot.
//!
//! ## Crossing the ABI
//!
//! [`crate::state`] builds these as Rust values, and [`Event::to_table`] turns
//! each into an array-style table `[kind, payload…]`. The native ABI has no
//! enum-variant type, so the tagged union is reassembled on the Saule side —
//! see `UIKit/Events.sau`, which decodes these into a real `Event` enum with
//! tuple payloads you can `match` on.
//!
//! ## Ordering
//!
//! Keyboard messages keep true arrival order, because they are recorded in the
//! backend's input callback as each OS message lands (see
//! [`crate::keyboard::drain_messages`]). Mouse and window events are derived
//! per frame — the backend offers no callback for them — so within one frame
//! the order is: window changes, mouse motion, mouse buttons, wheel, then the
//! keyboard log. Motion before buttons is the one that matters, so a click is
//! always dispatched against an up-to-date pointer position.

use saule_sdk::types::{STable, SValue};

/// One thing that happened since the last poll.
pub enum Event {
    KeyPressed {
        key: &'static str,
        shift: bool,
        ctrl: bool,
        alt: bool,
    },
    KeyReleased {
        key: &'static str,
        shift: bool,
        ctrl: bool,
        alt: bool,
    },
    /// Typed text, already layout- and modifier-aware — `shift`+`a` arrives
    /// here as `"A"`. Never assemble text out of `KeyPressed`.
    TextInput(String),
    MouseMoved {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    MousePressed {
        x: f64,
        y: f64,
        button: i64,
    },
    MouseReleased {
        x: f64,
        y: f64,
        button: i64,
    },
    /// A second press of the same button, close enough in time and place to
    /// the first. Always follows the `MousePressed` it completes, so a handler
    /// that only cares about single clicks needs no change.
    MouseDoubleClicked {
        x: f64,
        y: f64,
        button: i64,
    },
    /// Wheel movement in notches, positive away from the user.
    WheelMoved {
        dx: f64,
        dy: f64,
    },
    /// The pointer came over the window. A hover highlight that only ever sees
    /// motion events has no other way to know it should light up.
    MouseEntered {
        x: f64,
        y: f64,
    },
    /// The pointer left the window, so nothing is hovered any more.
    MouseLeft,
    Resized {
        width: i64,
        height: i64,
    },
    FocusChanged(bool),
    /// The user asked to close the window. `Window.isOpen()` is already false
    /// by the time this is seen.
    Closed,
}

impl Event {
    /// The tag the Saule side matches on to pick a variant.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::KeyPressed { .. } => "keyPressed",
            Event::KeyReleased { .. } => "keyReleased",
            Event::TextInput(_) => "textInput",
            Event::MouseMoved { .. } => "mouseMoved",
            Event::MousePressed { .. } => "mousePressed",
            Event::MouseReleased { .. } => "mouseReleased",
            Event::MouseDoubleClicked { .. } => "mouseDoubleClicked",
            Event::MouseEntered { .. } => "mouseEntered",
            Event::MouseLeft => "mouseLeft",
            Event::WheelMoved { .. } => "wheelMoved",
            Event::Resized { .. } => "resized",
            Event::FocusChanged(_) => "focusChanged",
            Event::Closed => "closed",
        }
    }

    /// Serialise to `[kind, payload…]`.
    ///
    /// Positional rather than keyed: the reader is a decoder that already knows
    /// each kind's shape, and positional costs one host call per field instead
    /// of one per field *and* key.
    pub fn to_table(&self) -> Result<STable, String> {
        let out = STable::new();
        out.push(SValue::from(self.kind()))?;

        match self {
            Event::KeyPressed {
                key,
                shift,
                ctrl,
                alt,
            }
            | Event::KeyReleased {
                key,
                shift,
                ctrl,
                alt,
            } => {
                out.push(SValue::from(*key))?;
                out.push(SValue::from(*shift))?;
                out.push(SValue::from(*ctrl))?;
                out.push(SValue::from(*alt))?;
            }
            Event::TextInput(text) => {
                out.push(SValue::from(text.as_str()))?;
            }
            Event::MouseMoved { x, y, dx, dy } => {
                out.push(SValue::from(*x))?;
                out.push(SValue::from(*y))?;
                out.push(SValue::from(*dx))?;
                out.push(SValue::from(*dy))?;
            }
            Event::MousePressed { x, y, button }
            | Event::MouseReleased { x, y, button }
            | Event::MouseDoubleClicked { x, y, button } => {
                out.push(SValue::from(*x))?;
                out.push(SValue::from(*y))?;
                out.push(SValue::from(*button))?;
            }
            Event::MouseEntered { x, y } => {
                out.push(SValue::from(*x))?;
                out.push(SValue::from(*y))?;
            }
            Event::MouseLeft => {}
            Event::WheelMoved { dx, dy } => {
                out.push(SValue::from(*dx))?;
                out.push(SValue::from(*dy))?;
            }
            Event::Resized { width, height } => {
                out.push(SValue::from(*width))?;
                out.push(SValue::from(*height))?;
            }
            Event::FocusChanged(focused) => {
                out.push(SValue::from(*focused))?;
            }
            Event::Closed => {}
        }

        Ok(out)
    }
}

/// Pack a frame's events into the array table `Window.pollEvents()` returns.
pub fn to_table(events: &[Event]) -> Result<STable, String> {
    let out = STable::new();
    for event in events {
        out.push(SValue::from(event.to_table()?))?;
    }
    Ok(out)
}
