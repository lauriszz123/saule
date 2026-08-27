//! Script-facing input queries: pointer position and buttons, key
//! state, and the per-frame event queue.

use super::*;
use crate::event::Event;
use crate::keyboard::{self, KeyState};
use minifb::{CursorStyle, Key, MouseMode};

/// How long after a click a second one still counts as a double-click, and how
/// far the pointer may drift in between.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_SLOP: f64 = 4.0;

impl Engine {
    /// Cursor position in framebuffer pixels, clamped to the window bounds.
    ///
    /// minifb reports this in the same units as `get_size`, which on macOS is
    /// points — so it is scaled to match the framebuffer, or every hit test
    /// would be off by the backing scale on a Retina display.
    pub(crate) fn mouse_pos(&self) -> (f64, f64) {
        let backing = self.backing_scale();
        self.window
            .get_mouse_pos(MouseMode::Clamp)
            .map(|(x, y)| (x as f64 * backing, y as f64 * backing))
            .unwrap_or((0.0, 0.0))
    }

    /// Whether the pointer is over the window right now.
    ///
    /// `Discard` is the mode that answers this: it reports `None` rather than
    /// clamping when the pointer is outside, which is what separates "at the
    /// edge" from "gone".
    pub(crate) fn pointer_over_window(&self) -> bool {
        self.window.get_mouse_pos(MouseMode::Discard).is_some()
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

        // Delivered once, on the transition. Re-reporting it every poll made
        // any handler that was not idempotent fire for as long as the loop
        // took to notice.
        if !self.is_open() && !self.close_reported {
            self.close_reported = true;
            self.events.push(Event::Closed);
        }

        let (x, y) = self.mouse_pos();

        let inside = self.pointer_over_window();
        if inside != self.pointer_inside {
            self.pointer_inside = inside;
            self.events.push(if inside {
                Event::MouseEntered { x, y }
            } else {
                Event::MouseLeft
            });
        }

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

                // A second press close in time *and* place is a double-click.
                // Deriving it here rather than in every app is the point: the
                // timing threshold should be one number, not one per widget.
                let slot = (button - 1) as usize;
                let now = Instant::now();
                let double = self.last_click[slot].is_some_and(|(at, px, py)| {
                    now.duration_since(at) <= DOUBLE_CLICK_WINDOW
                        && (x - px).abs() <= DOUBLE_CLICK_SLOP
                        && (y - py).abs() <= DOUBLE_CLICK_SLOP
                });

                if double {
                    self.events.push(Event::MouseDoubleClicked { x, y, button });
                    // Consumed, so a third click starts a fresh pair rather
                    // than reporting a second double.
                    self.last_click[slot] = None;
                } else {
                    self.last_click[slot] = Some((now, x, y));
                }
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
