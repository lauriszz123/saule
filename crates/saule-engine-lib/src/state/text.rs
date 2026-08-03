//! Font loading and text rendering.

use crate::font::{self, Align, FontRes};
use crate::raster::Surface;

use super::*;

impl Engine {
    /// Load a typeface. `path` of `None` uses the system default face.
    /// Returns the new font's handle.
    pub fn new_font(&mut self, size: f64, path: Option<&str>) -> Result<i64, String> {
        let res = match path {
            Some(p) => FontRes::from_file(p, size)?,
            None => font::load_default(size).ok_or_else(no_system_font)?,
        };
        self.fonts.push(Some(res));
        Ok(self.fonts.len() as i64 - 1)
    }

    pub fn set_font(&mut self, handle: i64) -> Result<(), String> {
        if handle < 0 || handle as usize >= self.fonts.len() {
            return Err(format!("Graphics.setFont: no font with handle {handle}"));
        }
        self.st.font = handle as usize;
        Ok(())
    }

    pub fn get_font(&self) -> i64 {
        self.st.font as i64
    }

    /// Make sure the selected font slot is populated, loading the system
    /// default on first use.
    pub(crate) fn ensure_font(&mut self) -> Result<(), String> {
        let i = self.st.font;
        if self.fonts.get(i).is_some_and(|f| f.is_some()) {
            return Ok(());
        }
        if i != 0 {
            return Err(format!("no font with handle {i}"));
        }
        self.fonts[0] = Some(font::load_default(font::DEFAULT_SIZE).ok_or_else(no_system_font)?);
        Ok(())
    }

    /// Borrow the render target and the active font at once. They live in
    /// disjoint fields, so destructuring is what makes the two `&mut`s legal.
    pub(crate) fn target_and_font(&mut self) -> (&mut Surface, &mut FontRes) {
        let Engine {
            screen,
            canvases,
            target,
            fonts,
            st,
            ..
        } = self;
        let surf = match target {
            None => screen,
            Some(i) => canvases[*i]
                .as_mut()
                .expect("the bound canvas is never on loan during a draw"),
        };
        let font = fonts[st.font]
            .as_mut()
            .expect("ensure_font ran before this call");
        (surf, font)
    }

    pub fn font_height(&mut self) -> Result<f64, String> {
        self.ensure_font()?;
        Ok(self.fonts[self.st.font].as_ref().unwrap().height())
    }

    pub fn text_width(&mut self, text: &str) -> Result<f64, String> {
        self.ensure_font()?;
        let i = self.st.font;
        Ok(self.fonts[i].as_mut().unwrap().measure(text))
    }

    /// Draw a single line of text with its top-left corner at `(x, y)`.
    pub fn print(&mut self, text: &str, x: f64, y: f64) -> Result<(), String> {
        self.ensure_font()?;
        let paint = self.paint();
        let xform = self.st.transform;
        let (surf, font) = self.target_and_font();

        let mut cursor_y = y;
        for line in text.split('\n') {
            draw_line(surf, font, line, x, cursor_y, &xform, &paint);
            cursor_y += font.height();
        }
        Ok(())
    }

    /// Draw word-wrapped, aligned text inside a `limit`-wide box anchored at
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
        let paint = self.paint();
        let xform = self.st.transform;
        let (surf, font) = self.target_and_font();

        let lines = font.wrap(text, limit);
        let mut cursor_y = y;
        for line in &lines {
            let width = font.layout(line).1;
            draw_line(
                surf,
                font,
                line,
                x + align.offset(width, limit),
                cursor_y,
                &xform,
                &paint,
            );
            cursor_y += font.height();
        }
        Ok(())
    }
}

/// Narrow a state colour to the rasterizer's working precision.
pub(crate) fn narrow(c: [f64; 4]) -> [f32; 4] {
    [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]
}

pub(crate) fn no_system_font() -> String {
    "no font available — the engine found no system typeface to fall back on; \
     load one explicitly with Graphics.newFont(size, path)"
        .to_string()
}
