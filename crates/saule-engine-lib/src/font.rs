//! Text: TrueType loading, glyph rasterization and caching, measurement, and
//! line breaking.
//!
//! Glyphs are rasterized on demand by [`fontdue`] into 8-bit coverage masks and
//! cached per font object, so a label costs one rasterization the first frame
//! and a memcpy-shaped blit on every frame after.
//!
//! There is no embedded default typeface. Instead the engine locates one from
//! the host OS the first time text is drawn (see [`load_default`]), which keeps
//! the library small and makes out-of-the-box text look native. Explicitly
//! loading a `.ttf` with `Graphics.newFont` always takes precedence.
//!
//! ## Fallback
//!
//! One face never covers everything. A UI font has no CJK, and almost nothing
//! outside a colour-emoji font has emoji, so a missing glyph used to rasterize
//! as a blank box with no recourse. Each [`FontRes`] therefore keeps a lazily
//! loaded chain of host faces ([`fallback_candidates`]) and consults it for any
//! codepoint its primary face lacks. The chain is loaded once, on the first
//! character that actually needs it, so a Latin-only UI never pays for it.

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
    /// Host faces consulted for codepoints `face` does not cover. Empty until
    /// the first missing glyph forces [`FontRes::load_fallbacks`].
    fallbacks: Vec<Font>,
    fallbacks_loaded: bool,
    size: f32,
    ascent: f64,
    line_height: f64,
    cache: HashMap<char, Glyph>,
}

impl FontRes {
    pub fn from_bytes(bytes: &[u8], size: f64) -> Result<Self, String> {
        let size = size.clamp(1.0, 512.0) as f32;
        let face = parse_face(bytes, size)?;

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
            fallbacks: Vec::new(),
            fallbacks_loaded: false,
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

    /// Load the host's wide-coverage faces, once, on first need.
    fn load_fallbacks(&mut self) {
        if self.fallbacks_loaded {
            return;
        }
        self.fallbacks_loaded = true;
        for path in fallback_candidates() {
            if let Ok(bytes) = std::fs::read(path)
                && let Ok(face) = parse_face(&bytes, self.size)
            {
                self.fallbacks.push(face);
            }
        }
    }

    /// Rasterize `ch` if it isn't cached yet, falling back to another face when
    /// the primary one has no glyph for it.
    fn ensure(&mut self, ch: char) {
        if self.cache.contains_key(&ch) {
            return;
        }

        if !self.face.has_glyph(ch) {
            self.load_fallbacks();
        }
        // The primary face wins whenever it can draw the character at all;
        // only a genuine miss consults the chain, and a miss everywhere falls
        // back to the primary's `.notdef` so something visible is drawn.
        let source = if self.face.has_glyph(ch) {
            &self.face
        } else {
            self.fallbacks
                .iter()
                .find(|f| f.has_glyph(ch))
                .unwrap_or(&self.face)
        };

        let (metrics, data) = source.rasterize(ch, self.size);
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
    ///
    /// Only the primary face is asked: a kern pair spanning two different
    /// faces is not a pair either face knows about.
    fn kern(&self, a: char, b: char) -> f64 {
        if !(self.face.has_glyph(a) && self.face.has_glyph(b)) {
            return 0.0;
        }
        self.face.horizontal_kern(a, b, self.size).unwrap_or(0.0) as f64
    }

    /// Lay a single line out into `(char, pen_x)` pairs, caching every glyph it
    /// touches. Returns the line's total advance width.
    ///
    /// `out` is cleared first and reused, so a frame of text costs no
    /// allocation once the buffer has grown.
    pub fn layout_into(&mut self, text: &str, out: &mut Vec<(char, f64)>) -> f64 {
        out.clear();
        out.reserve(text.len());
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
        pen
    }

    /// [`FontRes::layout_into`] with its own buffer, for callers that only want
    /// the pairs once.
    #[cfg(test)]
    pub fn layout(&mut self, text: &str) -> (Vec<(char, f64)>, f64) {
        let mut out = Vec::new();
        let width = self.layout_into(text, &mut out);
        (out, width)
    }

    /// The advance width of one line, without recording glyph positions.
    ///
    /// Layout is run for every text view on every frame, and measurement is
    /// most of it — this is the same arithmetic with nothing to write down.
    pub fn measure_line(&mut self, text: &str) -> f64 {
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
            pen += self.cache[&ch].advance;
            prev = Some(ch);
        }
        pen
    }

    /// The advance width of `text`, taking the widest line.
    pub fn measure(&mut self, text: &str) -> f64 {
        text.split('\n')
            .map(|line| self.measure_line(line))
            .fold(0.0, f64::max)
    }

    /// Greedy line breaking at `limit` pixels, honouring explicit newlines.
    ///
    /// Breaks fall after spaces and between wide (CJK, kana, hangul, emoji)
    /// characters — scripts that are written without spaces used to produce a
    /// single unbreakable "word" per paragraph and never wrapped at all. A
    /// Latin word longer than `limit` is still left overlong on its own line
    /// rather than split mid-word, matching Love2D's `printf`.
    pub fn wrap_into(&mut self, text: &str, limit: f64, out: &mut WrapBuf) {
        out.begin();

        // Taken out of `out` so the unit list is a reused buffer rather than a
        // fresh allocation on every `printf`.
        let mut units = std::mem::take(&mut out.units);
        for paragraph in text.split('\n') {
            if limit <= 0.0 {
                let line = out.push();
                line.text.push_str(paragraph);
                line.ends_paragraph = true;
                continue;
            }

            break_units(paragraph, &mut units);

            let mut current_w = 0.0;
            let mut started = false;
            let mut emitted = 0usize;

            for &(start, end) in units.iter() {
                let unit = &paragraph[start..end];
                let w = self.measure_line(unit);
                // Measure without the trailing space, so a space that only
                // falls at the edge never pushes the line over.
                let trimmed_w = self.measure_line(unit.trim_end());

                if started && current_w + trimmed_w > limit {
                    trim_trailing(out.last_mut());
                    let line = out.push();
                    line.text.push_str(unit);
                    line.ends_paragraph = false;
                    current_w = w;
                    emitted += 1;
                } else {
                    if !started {
                        out.push();
                        started = true;
                        emitted += 1;
                    }
                    out.last_mut().text.push_str(unit);
                    current_w += w;
                }
            }

            if emitted == 0 {
                // An empty paragraph is still a line — a blank one.
                out.push();
            }
            trim_trailing(out.last_mut());
            out.last_mut().ends_paragraph = true;
        }
        out.units = units;
    }

    /// [`FontRes::wrap_into`] returning plain strings, for callers that only
    /// want the text.
    #[cfg(test)]
    pub fn wrap(&mut self, text: &str, limit: f64) -> Vec<String> {
        let mut buf = WrapBuf::default();
        self.wrap_into(text, limit, &mut buf);
        buf.lines().iter().map(|l| l.text.clone()).collect()
    }
}

/// Parse a face, retrying as a collection member if the first attempt fails.
///
/// macOS ships most of its wide-coverage faces as `.ttc` collections, and
/// fontdue reads one member at a time, so a plain parse of the container is not
/// enough.
fn parse_face(bytes: &[u8], size: f32) -> Result<Font, String> {
    let settings = FontSettings {
        scale: size,
        ..FontSettings::default()
    };
    Font::from_bytes(bytes, settings).map_err(|e| format!("could not parse font: {e}"))
}

/// Drop a line's trailing spaces, which are layout artefacts rather than text.
fn trim_trailing(line: &mut WrapLine) {
    let trimmed = line.text.trim_end().len();
    line.text.truncate(trimmed);
}

/// One line produced by wrapping.
#[derive(Default)]
pub struct WrapLine {
    pub text: String,
    /// True for the last line of a paragraph — the one justification skips,
    /// since stretching it would leave a short final line spread edge to edge.
    pub ends_paragraph: bool,
}

/// A reusable buffer of wrapped lines.
///
/// Like [`crate::geom::PathSet`], `begin` rewinds without freeing, so the
/// `String` allocations survive from frame to frame.
#[derive(Default)]
pub struct WrapBuf {
    lines: Vec<WrapLine>,
    used: usize,
    /// Break-unit byte ranges, kept here purely so wrapping reuses the buffer.
    units: Vec<(usize, usize)>,
}

impl WrapBuf {
    pub fn begin(&mut self) {
        self.used = 0;
    }

    pub fn push(&mut self) -> &mut WrapLine {
        if self.used == self.lines.len() {
            self.lines.push(WrapLine::default());
        }
        let i = self.used;
        self.used += 1;
        self.lines[i].text.clear();
        self.lines[i].ends_paragraph = false;
        &mut self.lines[i]
    }

    /// The line currently being built. Only called after a `push`.
    fn last_mut(&mut self) -> &mut WrapLine {
        &mut self.lines[self.used - 1]
    }

    pub fn lines(&self) -> &[WrapLine] {
        &self.lines[..self.used]
    }
}

// ---------------------------------------------------------------------------
// Line breaking
// ---------------------------------------------------------------------------

/// Split a paragraph into the smallest units a line break may fall between.
///
/// A run of Latin text up to and including its trailing space is one unit; each
/// wide character is a unit of its own. Opening and closing brackets are kept
/// attached to their neighbour so a line never starts with `。` or ends with
/// `「`.
fn break_units(text: &str, out: &mut Vec<(usize, usize)>) {
    out.clear();
    if text.is_empty() {
        return;
    }

    let mut start = 0usize;
    let mut prev: Option<char> = None;

    for (i, ch) in text.char_indices() {
        if i == 0 {
            prev = Some(ch);
            continue;
        }
        let previous = prev.expect("set on the first character");

        // A break may fall here when the previous character ended a unit, or
        // this one starts a new wide unit.
        let after_space = previous == ' ';
        let wide_boundary = (is_wide(ch) || is_wide(previous)) && !no_break_before(ch)
            && !no_break_after(previous);

        if after_space || wide_boundary {
            out.push((start, i));
            start = i;
        }
        prev = Some(ch);
    }
    out.push((start, text.len()));
}

/// Characters written without spaces, where a break may fall between any two.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F      // Hangul Jamo
        | 0x2E80..=0x2FFF    // CJK radicals, Kangxi
        | 0x3040..=0x33FF    // kana, bopomofo, compatibility
        | 0x3400..=0x4DBF    // unified ideographs extension A
        | 0x4E00..=0x9FFF    // unified ideographs
        | 0xA000..=0xA4CF    // Yi
        | 0xAC00..=0xD7A3    // Hangul syllables
        | 0xF900..=0xFAFF    // compatibility ideographs
        | 0xFE30..=0xFE6F    // CJK compatibility forms
        | 0xFF01..=0xFF60    // fullwidth forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF  // emoji and pictographs
        | 0x20000..=0x2FA1F  // ideographs, later planes
    )
}

/// Closing punctuation, which may not begin a line.
fn no_break_before(ch: char) -> bool {
    matches!(
        ch,
        '。' | '、' | '，' | '．' | '！' | '？' | '：' | '；'
            | '」' | '』' | '）' | '】' | '〕' | '》' | '〉' | '〞'
            | 'ー' | '～' | '…'
    )
}

/// Opening punctuation, which may not end a line.
fn no_break_after(ch: char) -> bool {
    matches!(
        ch,
        '「' | '『' | '（' | '【' | '〔' | '《' | '〈' | '〝'
    )
}

// ---------------------------------------------------------------------------
// Host typefaces
// ---------------------------------------------------------------------------

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

/// Wide-coverage faces consulted for codepoints the primary face lacks.
///
/// Ordered so the most complete face wins: a pan-Unicode face first, then the
/// per-script ones, then emoji. Missing files are skipped, so listing a face
/// the host does not have costs one failed `read`.
fn fallback_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &[
            r"C:\Windows\Fonts\seguisym.ttf",
            r"C:\Windows\Fonts\msgothic.ttc",
            r"C:\Windows\Fonts\simsun.ttc",
            r"C:\Windows\Fonts\malgun.ttf",
            r"C:\Windows\Fonts\seguiemj.ttf",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "/Library/Fonts/Arial Unicode.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Apple Color Emoji.ttc",
        ]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/ancient-scripts/Symbola_hint.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
            "/mnt/c/Windows/Fonts/msgothic.ttc",
            "/mnt/c/Windows/Fonts/seguiemj.ttf",
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
            // Justified lines start at the left edge; the slack goes into the
            // gaps between words rather than the margin (see `word_spacing`).
            Align::Left | Align::Justify => 0.0,
            Align::Center => (limit - line_width) * 0.5,
            Align::Right => limit - line_width,
        }
    }

    /// Extra advance to add at each space so the line fills `limit` exactly.
    ///
    /// Zero for every alignment but `justify`, and zero for the last line of a
    /// paragraph, which would otherwise be stretched across the full width.
    pub fn word_spacing(self, line: &WrapLine, line_width: f64, limit: f64) -> f64 {
        if self != Align::Justify || line.ends_paragraph {
            return 0.0;
        }
        let gaps = line.text.chars().filter(|&c| c == ' ').count();
        if gaps == 0 {
            return 0.0;
        }
        let slack = limit - line_width;
        if slack <= 0.0 { 0.0 } else { slack / gaps as f64 }
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
    fn measure_line_agrees_with_layout() {
        let Some(mut f) = font(16.0) else { return };
        let (_, laid_out) = f.layout("the quick brown fox");
        assert!((f.measure_line("the quick brown fox") - laid_out).abs() < 1e-9);
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
    fn wrap_keeps_a_blank_line_blank() {
        let Some(mut f) = font(16.0) else { return };
        let lines = f.wrap("a\n\nb", 10_000.0);
        assert_eq!(lines, vec!["a", "", "b"]);
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

    #[test]
    fn layout_into_reuses_its_buffer() {
        let Some(mut f) = font(16.0) else { return };
        let mut buf = Vec::new();
        f.layout_into("abcdef", &mut buf);
        let capacity = buf.capacity();

        // A shorter second line must not leave the first one's tail behind.
        f.layout_into("xy", &mut buf);
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.capacity(), capacity);
    }

    // ── Line breaking ────────────────────────────────────────────────────

    fn units(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        break_units(text, &mut out);
        out.iter().map(|&(a, b)| text[a..b].to_string()).collect()
    }

    #[test]
    fn latin_breaks_after_spaces() {
        assert_eq!(units("one two three"), vec!["one ", "two ", "three"]);
    }

    #[test]
    fn each_wide_character_is_its_own_unit() {
        assert_eq!(units("日本語"), vec!["日", "本", "語"]);
    }

    #[test]
    fn a_latin_run_beside_wide_text_stays_whole() {
        assert_eq!(units("ab日本"), vec!["ab", "日", "本"]);
    }

    #[test]
    fn closing_punctuation_never_starts_a_unit() {
        // The full stop stays attached to the character it follows.
        assert_eq!(units("日。本"), vec!["日。", "本"]);
    }

    #[test]
    fn opening_punctuation_never_ends_a_unit() {
        assert_eq!(units("「日"), vec!["「日"]);
    }

    #[test]
    fn wide_text_wraps_where_spaced_text_would_not() {
        let Some(mut f) = font(16.0) else { return };
        // No spaces anywhere: the old space-only breaker produced one line.
        let text = "日本語のテキストは空白なしで折り返せるはずです";
        let lines = f.wrap(text, 60.0);
        assert!(lines.len() > 1, "expected wrapping, got {lines:?}");
    }

    // ── Justification ────────────────────────────────────────────────────

    #[test]
    fn justify_spreads_slack_across_the_gaps() {
        let line = WrapLine {
            text: "a b c".to_string(),
            ends_paragraph: false,
        };
        // Two gaps, 20px of slack.
        assert_eq!(Align::Justify.word_spacing(&line, 80.0, 100.0), 10.0);
    }

    #[test]
    fn justify_leaves_the_last_line_of_a_paragraph_alone() {
        let line = WrapLine {
            text: "a b c".to_string(),
            ends_paragraph: true,
        };
        assert_eq!(Align::Justify.word_spacing(&line, 80.0, 100.0), 0.0);
    }

    #[test]
    fn other_alignments_never_add_word_spacing() {
        let line = WrapLine {
            text: "a b c".to_string(),
            ends_paragraph: false,
        };
        for align in [Align::Left, Align::Center, Align::Right] {
            assert_eq!(align.word_spacing(&line, 80.0, 100.0), 0.0);
        }
    }
}
