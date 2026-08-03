//! The output buffer itself: indentation, line breaks, width
//! accounting, and the comment-interleaving that keeps a comment
//! attached to the line it was written on.

use std::{collections::VecDeque, fmt::Write, ops::Range};

use super::*;

impl<'a> Printer<'a> {
    pub(crate) fn new(source: &'a str, comments: &'a [Comment], opts: FmtOptions) -> Self {
        let mut queue: VecDeque<&'a Comment> = comments.iter().collect();
        // Tolerate unsorted inputs.
        queue.make_contiguous().sort_by_key(|c| c.span.start);
        Self {
            out: String::new(),
            indent: 0,
            needs_indent: false,
            source,
            comments: queue,
            last_pos: 0,
            last_comment_end: 0,
            indent_unit: opts.unit(),
            opts,
            force_inline: false,
        }
    }

    /// A sub-printer sharing this printer's configuration and position, used
    /// to render a candidate layout into a string so its width can be
    /// measured before committing to it. Always renders on one line.
    pub(crate) fn sub_printer(&self) -> Printer<'a> {
        Printer {
            out: String::new(),
            indent: self.indent,
            needs_indent: false,
            source: self.source,
            comments: VecDeque::new(),
            last_pos: self.last_pos,
            last_comment_end: self.last_comment_end,
            opts: self.opts,
            indent_unit: self.indent_unit.clone(),
            force_inline: true,
        }
    }

    /// The soft line-width target.
    pub(crate) fn max_width(&self) -> usize {
        self.opts.max_width
    }

    pub(crate) fn finish(mut self) -> String {
        // Guarantee exactly one trailing newline, even for an empty
        // module — every formatted file ends with `\n`.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    /// Emit the pending indentation, if any. Kept separate so `write` and
    /// `writef` cannot drift apart.
    pub(crate) fn flush_indent(&mut self) {
        if self.needs_indent {
            for _ in 0..self.indent {
                self.out.push_str(&self.indent_unit);
            }
            self.needs_indent = false;
        }
    }

    pub(crate) fn write(&mut self, s: &str) {
        self.flush_indent();
        self.out.push_str(s);
    }

    pub(crate) fn writef(&mut self, args: std::fmt::Arguments<'_>) {
        self.flush_indent();
        let _ = self.out.write_fmt(args);
    }

    pub(crate) fn newline(&mut self) {
        self.out.push('\n');
        self.needs_indent = true;
    }

    pub(crate) fn blank_line(&mut self) {
        // Don't emit duplicate blank lines or a blank line at the start.
        if self.out.is_empty() {
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
        self.needs_indent = true;
    }

    // ---- comment interleaving ---------------------------------------------

    /// Emit every queued comment whose start lies strictly before `pos`,
    /// one per line at the current indent. Preserves blank-line gaps
    /// between consecutive source comments (≥ 2 newlines in the original
    /// source → blank line in the output).
    ///
    /// Returns whether anything was emitted, so callers can tell a comment
    /// apart from an empty gap. `last_pos` can't answer that: it is a running
    /// maximum that [`Printer::block`] deliberately advances past the comment
    /// when anchoring to the block's first line.
    pub(crate) fn drain_before(&mut self, pos: usize) -> bool {
        let mut emitted = false;
        while let Some(c) = self.comments.front() {
            if c.span.start >= pos {
                break;
            }
            let c = self.comments.pop_front().unwrap();
            // If the printer's current line already has content, this
            // comment can't be a "leading" one — push it to its own line
            // first. (Happens when called mid-construct; rare but cheap.)
            if !self.out.is_empty() && !self.out.ends_with('\n') {
                self.newline();
            }
            // Blank-line preservation between comments / from start of file.
            if self.newlines_in_source(self.last_pos, c.span.start) >= 2 {
                self.blank_line();
            }
            self.write_comment(c);
            self.newline();
            self.last_pos = self.last_pos.max(c.span.end);
            self.last_comment_end = c.span.end;
            emitted = true;
        }
        emitted
    }

    /// If the next pending comment starts on the same source line as
    /// `after_pos`, emit it as a same-line trailing comment (preceded by
    /// two spaces) and consume it. Returns `true` if a comment was
    /// emitted, so callers can skip their own trailing newline logic if
    /// they want.
    pub(crate) fn try_trailing(&mut self, after_pos: usize) -> bool {
        let Some(c) = self.comments.front() else {
            return false;
        };
        if c.span.start < after_pos {
            // Pathological: drain_before should have handled it. Be safe.
            return false;
        }
        if self.newlines_in_source(after_pos, c.span.start) > 0 {
            return false;
        }
        let c = self.comments.pop_front().unwrap();
        self.out.push_str("  ");
        self.write_comment(c);
        self.last_pos = self.last_pos.max(c.span.end);
        true
    }

    pub(crate) fn write_comment(&mut self, c: &Comment) {
        // `write_str` would apply indentation; we want indent only on the
        // first line. Manage it manually.
        self.flush_indent();
        match c.kind {
            CommentKind::Line => {
                self.out.push_str("--");
                self.out.push_str(&c.text);
            }
            CommentKind::Block => {
                self.out.push_str("--[[");
                self.out.push_str(&c.text);
                self.out.push_str("]]");
            }
        }
    }

    /// Count `\n` bytes in `source[from..to]`. Returns 0 if either bound
    /// is out of range or if `from > to`. Used to distinguish same-line
    /// trailing comments from leading ones, and to preserve blank lines.
    pub(crate) fn newlines_in_source(&self, from: usize, to: usize) -> usize {
        if from > to || to > self.source.len() {
            return 0;
        }
        self.source.as_bytes()[from..to]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    }
    /// True when the author left a blank line between the comment that was
    /// just drained and the construct starting at `next_start`.
    ///
    /// `last_comment_end` sits at the end of that comment, which is *before*
    /// its newline, so a bare line break counts as one newline and a
    /// deliberate blank line counts as two.
    pub(crate) fn gap_after_comment(&self, next_start: usize) -> bool {
        !self.source.is_empty() && self.newlines_in_source(self.last_comment_end, next_start) >= 2
    }

    // ---- top-level ---------------------------------------------------------

    /// Column (0-based) of the *next* character to be emitted.
    ///
    /// Used by [`Expr::Pipe`] to remember where the `w` of `when` lands
    /// so subsequent `:stage()` lines can align under it. Accounts for a
    /// pending indent that hasn't been flushed yet.
    pub(crate) fn current_column(&self) -> usize {
        if self.needs_indent {
            return self.indent * self.opts.display_width();
        }
        match self.out.rfind('\n') {
            Some(p) => self.out.len() - p - 1,
            None => self.out.len(),
        }
    }

    pub(crate) fn source_range_has_newline(&self, range: Range<usize>) -> bool {
        self.source
            .get(range)
            .map(|s| s.contains('\n'))
            .unwrap_or(false)
    }
}
