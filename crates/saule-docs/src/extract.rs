//! Recovering a [`DocBlock`] from raw source text.
//!
//! Two stages: gather the contiguous run of `---` lines sitting above an
//! anchor offset ([`doc_lines`]), then split those lines into a summary
//! and its `@param` / `@return` tags ([`parse`]).

use crate::{DocBlock, ParamDoc};

/// One `---` line, stripped down to the text a reader actually wrote.
struct DocLine {
    /// Content after the `---` marker and one optional following space.
    text: String,
    /// Byte offset of `text`'s first character in the original source,
    /// so tag spans can be mapped back for diagnostics.
    start: usize,
    /// Byte offset of the line itself, used to span the whole block.
    line_start: usize,
}

/// Find the doc comment attached to the declaration containing `anchor`.
///
/// `anchor` may be any byte offset inside the declaration — the scan
/// begins at the start of `anchor`'s *line*, so leading `export`,
/// `local` or `static` modifiers on the same line are irrelevant.
///
/// Returns `None` when the line above isn't a doc comment. A blank line
/// detaches the block, so a `---` run separated from the declaration by
/// an empty line documents nothing.
pub fn extract(source: &str, anchor: usize) -> Option<DocBlock> {
    let lines = doc_lines(source, anchor);
    if lines.is_empty() {
        return None;
    }
    Some(parse(&lines))
}

/// Walk upward from `anchor`'s line collecting doc lines, nearest last
/// (the returned vector is in source order).
fn doc_lines(source: &str, anchor: usize) -> Vec<DocLine> {
    let anchor = anchor.min(source.len());
    // Start of the line the anchor sits on.
    let mut cursor = source[..anchor].rfind('\n').map_or(0, |i| i + 1);

    let mut out = Vec::new();
    while cursor > 0 {
        // The line immediately above spans [prev_start, cursor - 1),
        // with `cursor - 1` being its terminating newline.
        let newline = cursor - 1;
        let prev_start = source[..newline].rfind('\n').map_or(0, |i| i + 1);
        let raw = &source[prev_start..newline];

        let Some((offset, text)) = doc_content(raw) else {
            break;
        };
        out.push(DocLine {
            text,
            start: prev_start + offset,
            line_start: prev_start,
        });
        cursor = prev_start;
    }

    out.reverse();
    out
}

/// Classify one raw source line.
///
/// Returns `(byte offset of the content within `line`, content)` when
/// the line is a doc comment, `None` otherwise.
///
/// The marker is exactly three dashes **not** followed by a fourth, so
/// `---`, `--- text` are docs while `----`, `--------` and
/// `---- Section ----` are ordinary comments. This mirrors Rust's
/// `///` / `////` split and keeps decorative rules from latching onto
/// the declaration below them.
fn doc_content(line: &str) -> Option<(usize, String)> {
    // Tolerate CRLF sources — the trailing `\r` is not content.
    let line = line.strip_suffix('\r').unwrap_or(line);

    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];

    let after = rest.strip_prefix("---")?;
    if after.starts_with('-') {
        return None;
    }

    // Strip exactly one space after the marker so Markdown indentation
    // inside the comment survives.
    let (extra, body) = match after.strip_prefix(' ') {
        Some(b) => (1, b),
        None => (0, after),
    };
    Some((indent + 3 + extra, body.to_string()))
}

/// Split gathered doc lines into summary text and structured tags.
///
/// Lines are claimed by the most recent `@param` / `@return` tag until
/// the next tag appears. Anything before the first tag — and any `@tag`
/// we don't recognise — falls through to the summary in source order,
/// which is what makes an unsupported `@deprecated` render as written
/// rather than vanish.
fn parse(lines: &[DocLine]) -> DocBlock {
    /// Where subsequent continuation lines should be appended.
    enum Target {
        Summary,
        Param(usize),
        Return,
    }

    let mut summary: Vec<String> = Vec::new();
    let mut params: Vec<ParamDoc> = Vec::new();
    let mut returns: Option<String> = None;
    let mut target = Target::Summary;

    for line in lines {
        let trimmed = line.text.trim_start();
        let lead = line.text.len() - trimmed.len();

        if let Some(rest) = tag(trimmed, "param") {
            // `@param <name> <desc...>` — the name is the first word.
            let word = rest.trim_start();
            // Offset of the name within `trimmed`: past `@param`, then
            // past whatever spacing the author used before the name.
            let name_at = (trimmed.len() - rest.len()) + (rest.len() - word.len());
            let end = word.find(char::is_whitespace).unwrap_or(word.len());
            let name = word[..end].to_string();
            if name.is_empty() {
                summary.push(line.text.clone());
                continue;
            }
            let start = line.start + lead + name_at;
            params.push(ParamDoc {
                name,
                desc: word[end..].trim().to_string(),
                name_span: start..start + end,
            });
            target = Target::Param(params.len() - 1);
            continue;
        }

        if let Some(rest) = tag(trimmed, "return").or_else(|| tag(trimmed, "returns")) {
            returns = Some(rest.trim().to_string());
            target = Target::Return;
            continue;
        }

        // An unrecognised tag ends the current continuation and is kept
        // verbatim as part of the description.
        if trimmed.starts_with('@') {
            target = Target::Summary;
            summary.push(line.text.clone());
            continue;
        }

        match target {
            Target::Summary => summary.push(line.text.clone()),
            Target::Param(i) => append(&mut params[i].desc, trimmed),
            Target::Return => {
                if let Some(r) = returns.as_mut() {
                    append(r, trimmed);
                }
            }
        }
    }

    let span = lines
        .first()
        .zip(lines.last())
        .map(|(f, l)| f.line_start..l.start + l.text.len())
        .unwrap_or_default();

    DocBlock {
        summary: trim_blank_edges(&summary),
        params,
        returns,
        span,
    }
}

/// Match `@<name>` at the head of `line`, returning the remainder.
///
/// Requires the tag to be followed by whitespace or end-of-line so
/// `@paramount` is not mistaken for `@param`.
fn tag<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix('@')?.strip_prefix(name)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

/// Fold a continuation line into an existing description. A blank line
/// becomes a paragraph break; anything else joins with a single space so
/// hard-wrapped prose reflows cleanly in a hover popup.
fn append(dest: &mut String, line: &str) {
    if line.is_empty() {
        dest.push_str("\n\n");
    } else {
        if !dest.is_empty() && !dest.ends_with('\n') {
            dest.push(' ');
        }
        dest.push_str(line);
    }
}

/// Join summary lines, dropping leading and trailing blank ones so a
/// `---` spacer above a `@param` block doesn't leave stray newlines.
fn trim_blank_edges(lines: &[String]) -> String {
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let Some(start) = start else {
        return String::new();
    };
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap();
    lines[start..=end].join("\n")
}
