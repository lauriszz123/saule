//! Text: TrueType loading, glyph rasterization and caching, measurement, and
//! word wrapping.
//!
//! Glyphs are rasterized on demand by [`fontdue`] into 8-bit coverage masks and
//! cached per font object, so a label costs one rasterization the first frame
//! and a memcpy-shaped blit on every frame after.
//!
//! There is no embedded default typeface. Instead the engine locates one from
//! the host OS the first time text is drawn (see [`load_default`]), which keeps
//! the library small and makes out-of-the-box text look native. Explicitly
//! loading a `.ttf` with `Graphics.newFont` always takes precedence.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use crate::raster::Mask;

/// Point size used when a size is not given.
pub const DEFAULT_SIZE: f64 = 13.0;

/// A rasterized glyph plus the metrics needed to position it.
pub struct Glyph {
    pub mask: Mask,
    /// Offset from the pen position to the mask's top-left corner.
    pub left: f64,
    pub top: f64,
    pub advance: f64,
}

/// A loaded typeface at one fixed point size — the equivalent of a Love2D
/// `Font` object.
pub struct FontRes {
    face: Font,
    size: f32,
    ascent: f64,
    line_height: f64,
    cache: HashMap<char, Glyph>,
}

impl FontRes {
    pub fn from_bytes(bytes: &[u8], size: f64) -> Result<Self, String> {
        let size = size.clamp(1.0, 512.0) as f32;
        let face = Font::from_bytes(
            bytes,
            FontSettings {
                scale: size,
                ..FontSettings::default()
            },
        )
        .map_err(|e| format!("could not parse font: {e}"))?;

        // `horizontal_line_metrics` is absent on fonts without an `hhea`
        // table; fall back to the usual 80/20 split of the em box.
        let (ascent, line_height) = match face.horizontal_line_metrics(size) {
            Some(m) => (
                m.ascent as f64,
                (m.ascent - m.descent + m.line_gap).max(1.0) as f64,
            ),
            None => (size as f64 * 0.8, size as f64 * 1.2),
        };

        Ok(FontRes {
            face,
            size,
            ascent,
            line_height,
            cache: HashMap::new(),
        })
    }

    pub fn from_file(path: &str, size: f64) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("could not read font `{path}`: {e}"))?;
        Self::from_bytes(&bytes, size)
    }

    /// Distance from the top of a line to its baseline.
    pub fn ascent(&self) -> f64 {
        self.ascent
    }

    /// Baseline-to-baseline distance — what `Graphics.getFontHeight` reports
    /// and what `printf` advances by per line.
    pub fn height(&self) -> f64 {
        self.line_height
    }

    /// Rasterize `ch` if it isn't cached yet.
    fn ensure(&mut self, ch: char) {
        if self.cache.contains_key(&ch) {
            return;
        }
        let (metrics, data) = self.face.rasterize(ch, self.size);
        let glyph = Glyph {
            mask: Mask {
                data,
                w: metrics.width,
                h: metrics.height,
            },
            left: metrics.xmin as f64,
            // fontdue reports `ymin` from the baseline upward; the mask's top
            // edge sits `height + ymin` above it.
            top: -((metrics.height as i32 + metrics.ymin) as f64),
            advance: metrics.advance_width as f64,
        };
        self.cache.insert(ch, glyph);
    }

    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.cache.get(&ch)
    }

    /// Kerning adjustment to apply between two adjacent glyphs.
    fn kern(&self, a: char, b: char) -> f64 {
        self.face.horizontal_kern(a, b, self.size).unwrap_or(0.0) as f64
    }

    /// Lay a single line out into `(char, pen_x)` pairs, caching every glyph it
    /// touches. Returns the pairs and the line's total advance width.
    pub fn layout(&mut self, text: &str) -> (Vec<(char, f64)>, f64) {
        let mut out = Vec::with_capacity(text.len());
        let mut pen = 0.0;
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            if ch == '\r' {
                continue;
            }
            self.ensure(ch);
            if let Some(p) = prev {
                pen += self.kern(p, ch);
            }
            out.push((ch, pen));
            pen += self.cache[&ch].advance;
            prev = Some(ch);
        }
        (out, pen)
    }

    /// The advance width of `text` as a single unwrapped line.
    pub fn measure(&mut self, text: &str) -> f64 {
        text.split('\n')
            .map(|line| self.layout(line).1)
            .fold(0.0, f64::max)
    }

    /// Greedy word wrap at `limit` pixels, honouring explicit newlines.
    ///
    /// A word longer than `limit` is left overlong on its own line rather than
    /// being split mid-word — the same choice Love2D's `printf` makes.
    pub fn wrap(&mut self, text: &str, limit: f64) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            if limit <= 0.0 {
                lines.push(paragraph.to_string());
                continue;
            }
            let mut current = String::new();
            let mut current_w = 0.0;

            for word in paragraph.split_inclusive(' ') {
                let w = self.layout(word).1;
                // `trim_end` so a trailing space never pushes a line over.
                let trimmed_w = self.layout(word.trim_end()).1;

                if !current.is_empty() && current_w + trimmed_w > limit {
                    lines.push(current.trim_end().to_string());
                    current = word.to_string();
                    current_w = w;
                } else {
                    current.push_str(word);
                    current_w += w;
                }
            }
            lines.push(current.trim_end().to_string());
        }
        lines
    }
}

/// Load a typeface from the host OS, trying the usual UI faces in order.
///
/// Returns `None` when nothing suitable is installed, in which case text calls
/// report an actionable error pointing at `Graphics.newFont`.
pub fn load_default(size: f64) -> Option<FontRes> {
    for path in default_font_candidates() {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(font) = FontRes::from_bytes(&bytes, size)
        {
            return Some(font);
        }
    }
    None
}

fn default_font_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &[
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            r"C:\Windows\Fonts\tahoma.ttf",
            r"C:\Windows\Fonts\verdana.ttf",
            r"C:\Windows\Fonts\calibri.ttf",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/SFNSText.ttf",
            "/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
        ]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            // WSL installs usually have the Windows font directory mounted.
            "/mnt/c/Windows/Fonts/segoeui.ttf",
            "/mnt/c/Windows/Fonts/arial.ttf",
        ]
    }
}

/// Horizontal alignment for `Graphics.printf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "left" => Ok(Align::Left),
            "center" | "centre" => Ok(Align::Center),
            "right" => Ok(Align::Right),
            "justify" => Ok(Align::Justify),
            other => Err(format!(
                "unknown alignment `{other}` (expected \"left\", \"center\", \
                 \"right\", or \"justify\")"
            )),
        }
    }

    /// The x offset of a `line_width`-wide line inside a `limit`-wide box.
    pub fn offset(self, line_width: f64, limit: f64) -> f64 {
        match self {
            // Justify only differs from left in inter-word spacing, which the
            // simple layout here does not adjust.
            Align::Left | Align::Justify => 0.0,
            Align::Center => (limit - line_width) * 0.5,
            Align::Right => limit - line_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text test needs a real typeface; skip rather than fail on a host
    /// with no fonts installed (bare CI containers).
    fn font(size: f64) -> Option<FontRes> {
        load_default(size)
    }

    #[test]
    fn align_offsets_position_the_line() {
        assert_eq!(Align::Left.offset(40.0, 100.0), 0.0);
        assert_eq!(Align::Center.offset(40.0, 100.0), 30.0);
        assert_eq!(Align::Right.offset(40.0, 100.0), 60.0);
    }

    #[test]
    fn align_parse_accepts_both_spellings_of_centre() {
        assert_eq!(Align::parse("center").unwrap(), Align::Center);
        assert_eq!(Align::parse("centre").unwrap(), Align::Center);
        assert!(Align::parse("middle").is_err());
    }

    #[test]
    fn measuring_is_monotonic_in_length() {
        let Some(mut f) = font(16.0) else { return };
        let short = f.measure("ab");
        let long = f.measure("abcdef");
        assert!(long > short, "{long} should exceed {short}");
    }

    #[test]
    fn empty_string_measures_zero() {
        let Some(mut f) = font(16.0) else { return };
        assert_eq!(f.measure(""), 0.0);
    }

    #[test]
    fn multiline_measure_takes_the_widest_line() {
        let Some(mut f) = font(16.0) else { return };
        let wide = f.measure("wwwwwwww");
        assert!((f.measure("i\nwwwwwwww") - wide).abs() < 0.01);
    }

    #[test]
    fn line_height_is_positive_and_exceeds_ascent() {
        let Some(f) = font(20.0) else { return };
        assert!(f.height() > 0.0);
        assert!(f.height() >= f.ascent());
    }

    #[test]
    fn wrap_splits_on_the_limit() {
        let Some(mut f) = font(16.0) else { return };
        let text = "the quick brown fox jumps over the lazy dog";
        let lines = f.wrap(text, 80.0);
        assert!(lines.len() > 1, "expected wrapping, got {lines:?}");
        for line in &lines {
            // Single words may overflow; anything with a space must fit.
            if line.contains(' ') {
                assert!(f.measure(line) <= 80.0 + 0.5, "line too wide: {line:?}");
            }
        }
    }

    #[test]
    fn wrap_preserves_explicit_newlines() {
        let Some(mut f) = font(16.0) else { return };
        let lines = f.wrap("a\nb\nc", 10_000.0);
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn wrap_does_not_split_an_overlong_word() {
        let Some(mut f) = font(16.0) else { return };
        let lines = f.wrap("supercalifragilistic", 10.0);
        assert_eq!(lines, vec!["supercalifragilistic"]);
    }

    #[test]
    fn layout_advances_left_to_right() {
        let Some(mut f) = font(16.0) else { return };
        let (glyphs, width) = f.layout("abc");
        assert_eq!(glyphs.len(), 3);
        assert!(glyphs[0].1 < glyphs[1].1 && glyphs[1].1 < glyphs[2].1);
        assert!(width > glyphs[2].1);
    }

    #[test]
    fn layout_caches_the_glyphs_it_lays_out() {
        let Some(mut f) = font(16.0) else { return };
        f.layout("A");
        assert!(f.glyph('A').is_some());
        assert!(f.glyph('Z').is_none());
    }
}
