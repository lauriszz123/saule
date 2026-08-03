//! Mouse state and the wheel / button bookkeeping the script-facing
//! queries read.

use minifb::{MouseButton, Window};

/// Per-frame mouse edges and wheel movement, latched by
/// [`Engine::poll_events`] the same way [`KeyState`] latches key edges.
///
/// minifb reports the wheel as a delta that is only valid for the `update`
/// that produced it, so it has to be captured at the frame boundary or it is
/// lost. Buttons are sampled the same way, and both feed the `MousePressed` /
/// `MouseReleased` / `WheelMoved` events the frame's queue is built from.
#[derive(Default)]
pub struct MouseState {
    /// Left, right, middle — indices 0, 1, 2 (button numbers 1, 2, 3).
    down: [bool; 3],
    pressed: [bool; 3],
    released: [bool; 3],
    wheel: (f64, f64),
    /// Edges and wheel movement seen during a `present`. Same reason as
    /// [`crate::keyboard::KeyState`]: minifb pumps the OS queue twice a frame
    /// and each pump clears its own scroll delta, so half of them would be
    /// thrown away before Saule ever saw them.
    carried_pressed: [bool; 3],
    carried_released: [bool; 3],
    carried_wheel: (f64, f64),
}

/// One wheel notch in minifb's units: it reports `WHEEL_DELTA` (120) scaled by
/// 0.1, so a single click arrives as 12.0. Normalising to 1.0 per notch is what
/// makes "pixels per notch" mean anything on the Saule side.
pub(crate) const WHEEL_NOTCH: f64 = 12.0;

impl MouseState {
    pub(crate) fn sync(&mut self, window: &Window) {
        const BUTTONS: [MouseButton; 3] =
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

        for (i, button) in BUTTONS.iter().enumerate() {
            let now = window.get_mouse_down(*button);
            self.pressed[i] = (now && !self.down[i]) || self.carried_pressed[i];
            self.released[i] = (!now && self.down[i]) || self.carried_released[i];
            self.down[i] = now;
        }

        let (wx, wy) = Self::read_wheel(window);
        self.wheel = (wx + self.carried_wheel.0, wy + self.carried_wheel.1);

        self.carried_pressed = [false; 3];
        self.carried_released = [false; 3];
        self.carried_wheel = (0.0, 0.0);
    }

    /// Sample the mouse after presenting, keeping the edges and wheel movement
    /// for the next [`MouseState::sync`].
    pub(crate) fn observe(&mut self, window: &Window) {
        const BUTTONS: [MouseButton; 3] =
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

        for (i, button) in BUTTONS.iter().enumerate() {
            let now = window.get_mouse_down(*button);

            if now && !self.down[i] {
                self.carried_pressed[i] = true;
            }

            if !now && self.down[i] {
                self.carried_released[i] = true;
            }

            self.down[i] = now;
        }

        let (wx, wy) = Self::read_wheel(window);
        self.carried_wheel = (self.carried_wheel.0 + wx, self.carried_wheel.1 + wy);
    }

    /// The wheel delta in notches: 1.0 per click, positive away from the user.
    pub(crate) fn read_wheel(window: &Window) -> (f64, f64) {
        window.get_scroll_wheel().map_or((0.0, 0.0), |(x, y)| {
            (f64::from(x) / WHEEL_NOTCH, f64::from(y) / WHEEL_NOTCH)
        })
    }

    /// `1` = left, `2` = right, `3` = middle; anything else is not a button.
    pub(crate) fn slot(button: i64) -> Option<usize> {
        match button {
            1 => Some(0),
            2 => Some(1),
            3 => Some(2),
            _ => None,
        }
    }

    pub fn is_down(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.down[i])
    }

    pub fn was_pressed(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.pressed[i])
    }

    pub fn was_released(&self, button: i64) -> bool {
        Self::slot(button).is_some_and(|i| self.released[i])
    }

    /// Wheel movement since the last `pollEvents`, as `(x, y)`. Positive `y` is
    /// a scroll up / away from the user.
    pub fn wheel(&self) -> (f64, f64) {
        self.wheel
    }
}
