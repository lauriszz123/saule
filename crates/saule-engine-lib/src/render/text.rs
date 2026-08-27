//! Font loading and text rendering.

use crate::font::{self, Align, FontRes};
use crate::geom::Transform;
use crate::raster::{self, Paint, Surface};

use super::*;

impl Renderer {
    /// Load a typeface. `path` of `None` uses the system default face.
    /// Returns the new font's handle.
    pub fn new_font(&mut self, size: f64, path: Option<&str>) -> Result<i64, String> {
        let res = match path {
            Some(p) => FontRes::from_file(p, size)?,
            None => font::load_default(size).ok_or_else(no_system_font)?,
        };
        Ok(insert(&mut self.fonts, &mut self.free_fonts, res))
    }

    /// [`Renderer::new_font`], reporting failure as `None` rather than an
    /// error.
    ///
    /// A native error ends the Saule program, so a caller that wants to degrade
    /// gracefully — fall back to the default face, log and carry on — otherwise
    /// has to stat the file first and hope nothing changes in between. This is
    /// the same load with the outcome as a value.
    pub fn try_new_font(&mut self, size: f64, path: Option<&str>) -> Option<i64> {
        self.new_font(size, path).ok()
    }

    pub fn set_font(&mut self, handle: i64) -> Result<(), String> {
        self.st.font = self.font_index(handle, "Graphics.setFont")?;
        Ok(())
    }

    /// The selected font's handle, or `0` for the default face.
    pub fn get_font(&self) -> i64 {
        match self.st.font {
            0 => 0,
            i => pack_handle(self.fonts[i].tag, i),
        }
    }

    /// Make sure the selected font slot is populated, loading the system
    /// default on first use.
    pub(crate) fn ensure_font(&mut self) -> Result<(), String> {
        let i = self.st.font;
        if self.fonts.get(i).is_some_and(|f| f.value.is_some()) {
            return Ok(());
        }
        if i != 0 {
            return Err(format!("no font with handle {i}"));
        }
        self.fonts[0].value = Some(font::load_default(font::DEFAULT_SIZE).ok_or_else(no_system_font)?);
        Ok(())
    }

    pub fn font_height(&mut self) -> Result<f64, String> {
        self.ensure_font()?;
        Ok(self.fonts[self.st.font]
            .value
            .as_ref()
            .expect("ensure_font ran before this call")
            .height())
    }

    pub fn text_width(&mut self, text: &str) -> Result<f64, String> {
        self.ensure_font()?;
        let i = self.st.font;
        Ok(self.fonts[i]
            .value
            .as_mut()
            .expect("ensure_font ran before this call")
            .measure(text))
    }

    /// Draw a single line of text with its top-left corner at `(x, y)`.
    pub fn print(&mut self, text: &str, x: f64, y: f64) -> Result<(), String> {
        self.ensure_font()?;
        let paint = self.blit_paint();
        let xform = self.st.transform;

        let Renderer {
            screen,
            canvases,
            target,
            fonts,
            st,
            scratch,
            ..
        } = self;
        let surf = surface_of(screen, canvases, *target);
        let font = fonts[st.font]
            .value
            .as_mut()
            .expect("ensure_font ran before this call");

        let mut cursor_y = y;
        for line in text.split('\n') {
            draw_line(
                surf,
                font,
                &mut scratch.glyphs,
                line,
                x,
                cursor_y,
                0.0,
                &xform,
                &paint,
            );
            cursor_y += font.height();
        }
        Ok(())
    }

    /// Draw wrapped, aligned text inside a `limit`-wide box anchored at
    /// `(x, y)`.
    pub fn printf(
        &mut self,
        text: &str,
        x: f64,
        y: f64,
        limit: f64,
        align: &str,
    ) -> Result<(), String> {
        let align = Align::parse(align)?;
        self.ensure_font()?;
        let paint = self.blit_paint();
        let xform = self.st.transform;

        let Renderer {
            screen,
            canvases,
            target,
            fonts,
            st,
            scratch,
            ..
        } = self;
        let surf = surface_of(screen, canvases, *target);
        let font = fonts[st.font]
            .value
            .as_mut()
            .expect("ensure_font ran before this call");

        font.wrap_into(text, limit, &mut scratch.wrap);

        let mut cursor_y = y;
        for line in scratch.wrap.lines() {
            let width = font.measure_line(&line.text);
            draw_line(
                surf,
                font,
                &mut scratch.glyphs,
                &line.text,
                x + align.offset(width, limit),
                cursor_y,
                align.word_spacing(line, width, limit),
                &xform,
                &paint,
            );
            cursor_y += font.height();
        }
        Ok(())
    }
}

/// Blit one laid-out line. `y` is the line's *top*, matching how Love2D's
/// `print` anchors text.
///
/// `word_spacing` is extra advance added at each space, which is what turns a
/// left-aligned line into a justified one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_line(
    surf: &mut Surface,
    font: &mut FontRes,
    glyphs: &mut Vec<(char, f64)>,
    text: &str,
    x: f64,
    y: f64,
    word_spacing: f64,
    xform: &Transform,
    paint: &Paint,
) {
    if text.is_empty() {
        return;
    }
    let baseline = y + font.ascent();
    font.layout_into(text, glyphs);

    // Spaces seen so far, so each following glyph carries the justification
    // slack of every gap to its left.
    let mut spread = 0.0;
    for &(ch, pen) in glyphs.iter() {
        if ch == ' ' {
            spread += word_spacing;
            continue; // a space has advance but no pixels
        }
        let Some(glyph) = font.glyph(ch) else {
            continue;
        };
        if glyph.mask.w == 0 || glyph.mask.h == 0 {
            continue; // whitespace carries advance but no pixels
        }
        let placement =
            Transform::translation(x + pen + spread + glyph.left, baseline + glyph.top);
        raster::blit_mask(surf, &glyph.mask, &xform.then(&placement), paint);
    }
}

pub(crate) fn no_system_font() -> String {
    "no font available — the engine found no system typeface to fall back on; \
     load one explicitly with Graphics.newFont(size, path)"
        .to_string()
}
