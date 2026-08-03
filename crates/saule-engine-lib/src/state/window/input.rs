//! Script-facing input queries: pointer position and buttons, key
//! state, and the per-frame event queue.

use super::*;
use crate::event::Event;
use crate::keyboard::{self, KeyState};
use minifb::{CursorStyle, Key, MouseMode};

impl Engine {
    /// Cursor position in window pixels, clamped to the window bounds.
    pub(crate) fn mouse_pos(&self) -> (f64, f64) {
        self.window
            .get_mouse_pos(MouseMode::Clamp)
            .map(|(x, y)| (x as f64, y as f64))
            .unwrap_or((0.0, 0.0))
    }

    /// This frame's mouse state: held buttons, edges, and wheel movement.
    ///
    /// Everything comes from the same `pollEvents` snapshot, so `isDown` and
    /// the button events built from it describe one consistent instant.
    pub(crate) fn mouse(&self) -> &MouseState {
        &self.mouse
    }

    /// Swap the cursor image. Unknown names are rejected rather than silently
    /// ignored, so a typo surfaces at the call site.
    pub(crate) fn set_cursor(&mut self, style: &str) -> Result<(), String> {
        let cursor = match style {
            "arrow" => CursorStyle::Arrow,
            "ibeam" | "text" => CursorStyle::Ibeam,
            "crosshair" => CursorStyle::Crosshair,
            "hand" | "openhand" => CursorStyle::OpenHand,
            "grab" | "closedhand" => CursorStyle::ClosedHand,
            "resizeleftright" | "ew" => CursorStyle::ResizeLeftRight,
            "resizeupdown" | "ns" => CursorStyle::ResizeUpDown,
            "resizeall" | "move" => CursorStyle::ResizeAll,
            other => {
                return Err(format!(
                    "Mouse.setCursor: unknown cursor {other:?} — expected one of \
                     arrow, ibeam, crosshair, hand, grab, resizeleftright, \
                     resizeupdown, resizeall"
                ));
            }
        };
        self.window.set_cursor_style(cursor);
        Ok(())
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.window.set_cursor_visibility(visible);
    }

    pub(crate) fn is_key_down(&self, key: Key) -> bool {
        self.window.is_key_down(key)
    }

    /// The canonical names of every key held right now, in the keyboard
    /// module's table order. Unnameable keys are skipped.
    pub(crate) fn keys_held(&self) -> Vec<&'static str> {
        self.window
            .get_keys()
            .into_iter()
            .filter_map(keyboard::key_name)
            .collect()
    }

    /// This frame's latched keyboard state — held keys and repeat timing.
    pub(crate) fn keys(&self) -> &KeyState {
        &self.keys
    }

    pub(crate) fn keys_mut(&mut self) -> &mut KeyState {
        &mut self.keys
    }

    /// Pump the OS event queue without presenting a frame. Keeps
    /// `is_open` / input fresh at the top of the loop, and is the frame
    /// boundary the keyboard's press/release edges are measured against.
    pub(crate) fn poll_events(&mut self) {
        self.window.update();
        self.keys.sync(&self.window);
        self.mouse.sync(&self.window);

        let resized = self.sync_surface();
        self.collect_events(resized);
    }

    /// This frame's events, in the order described on [`crate::event`].
    pub(crate) fn events(&self) -> &[Event] {
        &self.events
    }

    /// Rebuild [`Engine::events`] from the state just latched.
    ///
    /// Window changes come first, then pointer motion, then buttons and wheel,
    /// then the keyboard log in true arrival order. Motion before buttons is
    /// the ordering that matters: a click must be delivered against an
    /// up-to-date pointer position.
    pub(crate) fn collect_events(&mut self, resized: Option<(usize, usize)>) {
        self.events.clear();

        if let Some((w, h)) = resized {
            self.events.push(Event::Resized {
                width: w as i64,
                height: h as i64,
            });
        }

        let focused = self.window.is_active();
        if focused != self.last_focused {
            self.last_focused = focused;
            self.events.push(Event::FocusChanged(focused));
        }

        if !self.is_open() {
            self.events.push(Event::Closed);
        }

        let (x, y) = self.mouse_pos();
        match self.last_mouse {
            Some((px, py)) if px != x || py != y => {
                self.events.push(Event::MouseMoved {
                    x,
                    y,
                    dx: x - px,
                    dy: y - py,
                });
            }
            // The first sighting is a position, not a movement.
            None => self.events.push(Event::MouseMoved {
                x,
                y,
                dx: 0.0,
                dy: 0.0,
            }),
            _ => {}
        }
        self.last_mouse = Some((x, y));

        for button in 1..=3 {
            if self.mouse.was_pressed(button) {
                self.events.push(Event::MousePressed { x, y, button });
            }
            if self.mouse.was_released(button) {
                self.events.push(Event::MouseReleased { x, y, button });
            }
        }

        let (wheel_x, wheel_y) = self.mouse.wheel();
        if wheel_x != 0.0 || wheel_y != 0.0 {
            self.events.push(Event::WheelMoved {
                dx: wheel_x,
                dy: wheel_y,
            });
        }

        for message in keyboard::drain_messages() {
            let event = match message {
                keyboard::KeyMessage::Down { key, mods } => {
                    keyboard::key_name(key).map(|key| Event::KeyPressed {
                        key,
                        shift: mods.shift,
                        ctrl: mods.ctrl,
                        alt: mods.alt,
                    })
                }
                keyboard::KeyMessage::Up { key, mods } => {
                    keyboard::key_name(key).map(|key| Event::KeyReleased {
                        key,
                        shift: mods.shift,
                        ctrl: mods.ctrl,
                        alt: mods.alt,
                    })
                }
                keyboard::KeyMessage::Text(text) => Some(Event::TextInput(text)),
            };

            if let Some(event) = event {
                self.events.push(event);
            }
        }

        // Key repeat is the engine's own invention rather than an OS message,
        // so it is synthesised after the real ones. Held modifiers are read at
        // level, which is exactly right for a key that is still down.
        let mods = self.held_modifiers();
        for key in self.keys.repeated_names() {
            self.events.push(Event::KeyPressed {
                key,
                shift: mods.shift,
                ctrl: mods.ctrl,
                alt: mods.alt,
            });
        }
    }

    pub(crate) fn held_modifiers(&self) -> keyboard::Modifiers {
        keyboard::Modifiers {
            shift: self.is_key_down(Key::LeftShift) || self.is_key_down(Key::RightShift),
            ctrl: self.is_key_down(Key::LeftCtrl) || self.is_key_down(Key::RightCtrl),
            alt: self.is_key_down(Key::LeftAlt) || self.is_key_down(Key::RightAlt),
        }
    }
}
