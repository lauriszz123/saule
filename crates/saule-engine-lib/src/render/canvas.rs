//! Render targets: the screen, offscreen canvases, and loaded images —
//! allocating them, selecting the current one, and blitting them.

use super::*;
use crate::geom::Transform;
use crate::raster::{self, Gradient, Paint, Rect, Surface};

/// Pick the destination surface out of a destructured [`Renderer`].
///
/// Taking the fields rather than `&mut self` is what lets a draw call hold the
/// scratch buffers and the target at the same time.
pub(crate) fn surface_of<'a>(
    screen: &'a mut Surface,
    canvases: &'a mut [Slot<Surface>],
    target: Option<usize>,
) -> &'a mut Surface {
    match target {
        None => screen,
        Some(i) => canvases[i]
            .value
            .as_mut()
            .expect("the bound canvas is never on loan during a draw"),
    }
}

impl Renderer {
    pub(crate) fn target_size(&self) -> (usize, usize) {
        match self.target {
            None => (self.screen.w, self.screen.h),
            Some(i) => self.canvases[i]
                .value
                .as_ref()
                .map(|s| (s.w, s.h))
                .unwrap_or((0, 0)),
        }
    }

    pub(crate) fn target_mut(&mut self) -> &mut Surface {
        let Renderer {
            screen,
            canvases,
            target,
            ..
        } = self;
        surface_of(screen, canvases, *target)
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
            gradient: self.st.gradient,
        }
    }

    /// The paint a *blit* uses: the same state, minus the gradient.
    ///
    /// A gradient is a source of colour, and an image already has one — the
    /// paint colour tints the sampled pixels instead. Silently ignoring the
    /// gradient here beats letting it overwrite the image.
    pub(crate) fn blit_paint(&self) -> Paint {
        Paint {
            gradient: None,
            ..self.paint()
        }
    }

    /// Set the fill source to a gradient in *local* coordinates, which are
    /// baked to device space here so the gradient stays put under a later
    /// transform — the same rule scissors follow.
    pub(crate) fn set_gradient(&mut self, gradient: Gradient) {
        self.st.gradient = Some(gradient.transformed(&self.st.transform));
    }

    pub(crate) fn clear_gradient(&mut self) {
        self.st.gradient = None;
    }

    pub(crate) fn has_gradient(&self) -> bool {
        self.st.gradient.is_some()
    }

    /// Allocate a canvas and return its handle.
    pub(crate) fn new_canvas(&mut self, w: i64, h: i64) -> Result<i64, String> {
        if w <= 0 || h <= 0 {
            return Err("Graphics.newCanvas: width and height must be positive".into());
        }
        // Guard against a typo turning into a multi-gigabyte allocation.
        if w > 16384 || h > 16384 {
            return Err("Graphics.newCanvas: dimensions may not exceed 16384".into());
        }
        Ok(insert(
            &mut self.canvases,
            &mut self.free_canvases,
            Surface::new(w as usize, h as usize),
        ))
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
        match self.target {
            None => 0,
            Some(i) => pack_handle(self.canvases[i].tag, i),
        }
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
        let (w, h) = self.canvas_size(idx, "Graphics.draw")?;
        self.blit(idx, Rect::new(0.0, 0.0, w as f64, h as f64), x, y, angle, sx, sy, ox, oy)
    }

    /// Composite one cell of an image onto the current target — the
    /// spritesheet draw. `frame` selects the source rectangle; the rest
    /// positions it exactly like [`Renderer::draw_canvas`].
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
        self.blit(idx, frame, x, y, angle, sx, sy, ox, oy)
    }

    /// The shared body of `draw` and `drawFrame`.
    #[allow(clippy::too_many_arguments)]
    fn blit(
        &mut self,
        idx: usize,
        frame: Rect,
        x: f64,
        y: f64,
        angle: f64,
        sx: f64,
        sy: f64,
        ox: f64,
        oy: f64,
    ) -> Result<(), String> {
        if self.target == Some(idx) {
            return Err("Graphics.draw: an image cannot be drawn onto itself".into());
        }

        let local = Transform::translation(x, y)
            .then(&Transform::rotation(angle))
            .then(&Transform::scaling(sx, sy))
            .then(&Transform::translation(-ox, -oy));
        let xform = self.st.transform.then(&local);
        let paint = self.blit_paint();

        // Lift the source out of the registry so the source and destination
        // borrows are provably disjoint, then put it straight back.
        let src = self.canvases[idx]
            .value
            .take()
            .ok_or("Graphics.draw: the image is on loan to a draw call")?;
        raster::blit_surface_sub(self.target_mut(), &src, frame, &xform, &paint);
        self.canvases[idx].value = Some(src);
        Ok(())
    }

    /// Decode a PNG into the surface registry and return its handle.
    ///
    /// Images and canvases share one registry, so the handle works with
    /// `draw`, `drawFrame`, `imageSize`, `release`, and even `setCanvas` — a
    /// loaded image is simply a canvas that started life with pixels in it.
    pub(crate) fn new_image(&mut self, path: &str) -> Result<i64, String> {
        let surface = crate::image::load(path)?;
        Ok(insert(
            &mut self.canvases,
            &mut self.free_canvases,
            surface,
        ))
    }

    /// [`Renderer::new_image`], reporting failure as `None` rather than an
    /// error.
    ///
    /// A native error ends the Saule program, so a caller that wants to skip a
    /// missing asset and carry on otherwise has to `Io.open` the file first
    /// just to see whether it exists — which opens it twice and still races
    /// anything that deletes it in between.
    pub(crate) fn try_new_image(&mut self, path: &str) -> Option<i64> {
        self.new_image(path).ok()
    }

    /// Decode a base64-encoded PNG — an asset embedded in the source, with no
    /// file on disk.
    pub(crate) fn new_image_from_base64(&mut self, data: &str) -> Result<i64, String> {
        let surface = crate::image::decode_base64(data)?;
        Ok(insert(&mut self.canvases, &mut self.free_canvases, surface))
    }

    fn canvas_size(&self, idx: usize, func: &str) -> Result<(usize, usize), String> {
        self.canvases[idx]
            .value
            .as_ref()
            .map(|s| (s.w, s.h))
            .ok_or_else(|| format!("{func}: the image is on loan to a draw call"))
    }

    /// Pixel dimensions of an image or canvas.
    pub(crate) fn image_size(&self, handle: i64) -> Result<(i64, i64), String> {
        let idx = self.canvas_index(handle, "Graphics.imageSize")?;
        let (w, h) = self.canvas_size(idx, "Graphics.imageSize")?;
        Ok((w as i64, h as i64))
    }

    /// Encode a canvas, image, or the screen (handle `0`) as a PNG on disk.
    pub(crate) fn save_image(&self, handle: i64, path: &str) -> Result<(), String> {
        let surface = if handle == 0 {
            &self.screen
        } else {
            let idx = self.canvas_index(handle, "Graphics.saveImage")?;
            self.canvases[idx]
                .value
                .as_ref()
                .ok_or("Graphics.saveImage: the image is on loan to a draw call")?
        };
        crate::image::save(surface, path)
    }
}
