//! Offscreen canvases and images: allocating them, selecting the
//! current render target, and blitting them to it.

use super::*;
use crate::geom::Transform;
use crate::raster::{self, Paint, Rect, Surface};

impl Engine {
    pub(crate) fn target_size(&self) -> (usize, usize) {
        match self.target {
            None => (self.screen.w, self.screen.h),
            Some(i) => self.canvases[i]
                .as_ref()
                .map(|s| (s.w, s.h))
                .unwrap_or((0, 0)),
        }
    }

    pub(crate) fn target_mut(&mut self) -> &mut Surface {
        match self.target {
            None => &mut self.screen,
            Some(i) => self.canvases[i]
                .as_mut()
                .expect("the bound canvas is never on loan during a draw"),
        }
    }

    /// The paint for the current state, with the scissor already reduced to the
    /// target's bounds.
    pub(crate) fn paint(&self) -> Paint {
        let (w, h) = self.target_size();
        let bounds = Rect::surface(w, h);
        Paint {
            color: narrow(self.st.color),
            blend: self.st.blend,
            clip: match self.st.scissor {
                Some(s) => s.intersect(&bounds),
                None => bounds,
            },
            antialias: self.st.smooth,
            linear_filter: self.linear_filter,
        }
    }

    pub(crate) fn canvas_index(&self, handle: i64, func: &str) -> Result<usize, String> {
        if handle < 1 || handle as usize > self.canvases.len() {
            return Err(format!("{func}: no canvas with handle {handle}"));
        }
        Ok(handle as usize - 1)
    }

    /// Allocate a canvas and return its handle (`1`-based).
    pub(crate) fn new_canvas(&mut self, w: i64, h: i64) -> Result<i64, String> {
        if w <= 0 || h <= 0 {
            return Err("Graphics.newCanvas: width and height must be positive".into());
        }
        // Guard against a typo turning into a multi-gigabyte allocation.
        if w > 16384 || h > 16384 {
            return Err("Graphics.newCanvas: dimensions may not exceed 16384".into());
        }
        self.canvases
            .push(Some(Surface::new(w as usize, h as usize)));
        Ok(self.canvases.len() as i64)
    }

    /// Bind a canvas as the render target. `None` (or handle `0`) restores the
    /// screen.
    pub(crate) fn set_canvas(&mut self, handle: Option<i64>) -> Result<(), String> {
        self.target = match handle {
            None | Some(0) => None,
            Some(h) => Some(self.canvas_index(h, "Graphics.setCanvas")?),
        };
        Ok(())
    }

    /// The bound canvas handle, or `0` for the screen.
    pub(crate) fn get_canvas(&self) -> i64 {
        self.target.map(|i| i as i64 + 1).unwrap_or(0)
    }

    /// Composite a canvas onto the current target.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_canvas(
        &mut self,
        handle: i64,
        x: f64,
        y: f64,
        angle: f64,
        sx: f64,
        sy: f64,
        ox: f64,
        oy: f64,
    ) -> Result<(), String> {
        let idx = self.canvas_index(handle, "Graphics.draw")?;
        if self.target == Some(idx) {
            return Err("Graphics.draw: a canvas cannot be drawn onto itself".into());
        }

        let local = Transform::translation(x, y)
            .then(&Transform::rotation(angle))
            .then(&Transform::scaling(sx, sy))
            .then(&Transform::translation(-ox, -oy));
        let xform = self.st.transform.then(&local);
        let paint = self.paint();

        // Lift the source out of the registry so the source and destination
        // borrows are provably disjoint, then put it straight back.
        let src = self.canvases[idx]
            .take()
            .expect("a canvas is only on loan during its own draw");
        raster::blit_surface(self.target_mut(), &src, &xform, &paint);
        self.canvases[idx] = Some(src);
        Ok(())
    }

    /// Decode a PNG into the surface registry and return its handle.
    ///
    /// Images and canvases share one registry, so the handle works with
    /// `draw`, `drawFrame`, `imageSize`, and even `setCanvas` — a loaded image
    /// is simply a canvas that started life with pixels in it.
    pub(crate) fn new_image(&mut self, path: &str) -> Result<i64, String> {
        let surface = crate::image::load(path)?;
        self.canvases.push(Some(surface));
        Ok(self.canvases.len() as i64)
    }

    /// Pixel dimensions of an image or canvas.
    pub(crate) fn image_size(&self, handle: i64) -> Result<(i64, i64), String> {
        let idx = self.canvas_index(handle, "Graphics.imageSize")?;
        let surface = self.canvases[idx]
            .as_ref()
            .ok_or("Graphics.imageSize: the image is on loan to a draw call")?;
        Ok((surface.w as i64, surface.h as i64))
    }

    /// Composite one cell of an image onto the current target — the
    /// spritesheet draw. `frame` selects the source rectangle; the rest
    /// positions it exactly like [`Engine::draw_canvas`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_frame(
        &mut self,
        handle: i64,
        frame: Rect,
        x: f64,
        y: f64,
        angle: f64,
        sx: f64,
        sy: f64,
        ox: f64,
        oy: f64,
    ) -> Result<(), String> {
        let idx = self.canvas_index(handle, "Graphics.drawFrame")?;
        if self.target == Some(idx) {
            return Err("Graphics.drawFrame: an image cannot be drawn onto itself".into());
        }

        let local = Transform::translation(x, y)
            .then(&Transform::rotation(angle))
            .then(&Transform::scaling(sx, sy))
            .then(&Transform::translation(-ox, -oy));
        let xform = self.st.transform.then(&local);
        let paint = self.paint();

        let src = self.canvases[idx]
            .take()
            .expect("an image is only on loan during its own draw");
        raster::blit_surface_sub(self.target_mut(), &src, frame, &xform, &paint);
        self.canvases[idx] = Some(src);
        Ok(())
    }
}
