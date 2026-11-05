//! Lua-style patterns, over characters rather than bytes.
//!
//! This is a port of the matcher in Lua's `lstrlib.c` — the same recursive
//! backtracking design, the same operators, the same greedy/lazy expansion.
//! It backs `String.match`, `String.gmatch` and `String.gsub`.
//!
//! **Why Lua patterns and not regular expressions.** They are what the
//! audience this language borrows from already knows, they need no dependency,
//! and the whole engine is a few hundred lines. The cost is no alternation
//! (`a|b`) and no counted repetition (`a{2,5}`); a full regex module can land
//! beside this later without either having to change.
//!
//! ## Two deliberate departures from Lua
//!
//! **Characters, not bytes.** A Saule `string` is always valid UTF-8 and is
//! measured and indexed in codepoints ([`crate::stdlib::string`]), so this
//! matcher runs over `&[char]` and every index it reports is a character
//! index. In Lua, `("ä"):find(".")` matches half of a two-byte character; here
//! `.` is one character, whatever it encodes to. That difference is the reason
//! this is a port rather than a binding: the algorithm is Lua's, the indexing
//! is Saule's.
//!
//! **Character classes are Unicode-aware where that is what "letter" means.**
//! `%a` is `char::is_alphabetic`, so it matches `é` and `ą`. `%d` is
//! deliberately *not* Unicode: it is ASCII `0-9` only, because a `%d+` is
//! almost always about to be read as a number, and `String.toInteger` would
//! reject the Devanagari digits a Unicode `%d` would have matched. The full
//! table is in `DOCS.md`.
//!
//! ## What is not supported
//!
//! Position captures — `()`, which in Lua yields an integer index — are
//! rejected with an error. Saule's captures are a `table<string>`, and there
//! is no honest way to put an integer in one: returning the decimal digits as
//! a string would be a silent trap for the caller who then indexes with it.
//! `String.find` already answers the question position captures are for.

/// Lua's limit, and for the same reason: the capture array is fixed-size, and
/// a pattern wanting more than 32 captures has outgrown patterns.
const MAX_CAPTURES: usize = 32;

/// Guard against patterns that recurse without consuming, such as `("")` on
/// an empty subject, or deeply nested `(((...)))`. Lua uses 200.
const MAX_DEPTH: u32 = 200;

/// A closed capture's length is its character count; these two markers stand
/// in for the states a length cannot represent.
const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;

/// One successful match: the span it covered and the captures inside it.
///
/// Both indices are 0-based character offsets, `end` exclusive, so
/// `&src[m.start..m.end]` is the matched text.
#[derive(Debug, Clone)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    /// Each capture as `(start, end)` character offsets. Empty when the
    /// pattern declared no captures — callers substitute the whole match.
    pub captures: Vec<(usize, usize)>,
}

impl Match {
    /// The captures as strings, or the whole match when the pattern had none.
    ///
    /// This is the shape `String.match` and `String.gmatch` hand back, and the
    /// fallback is Lua's rule: a pattern with no parentheses captures itself.
    pub fn capture_texts(&self, src: &[char]) -> Vec<String> {
        if self.captures.is_empty() {
            vec![src[self.start..self.end].iter().collect()]
        } else {
            self.captures
                .iter()
                .map(|&(s, e)| src[s..e].iter().collect())
                .collect()
        }
    }
}

struct MatchState<'a> {
    src: &'a [char],
    pat: &'a [char],
    level: usize,
    capture: [(usize, isize); MAX_CAPTURES],
    depth: u32,
}

/// Does `c` belong to the class named by `cl`?
///
/// An uppercase class letter is the complement of its lowercase form, which is
/// why the result is flipped at the end rather than written out twice.
fn match_class(c: char, cl: char) -> bool {
    let res = match cl.to_ascii_lowercase() {
        'a' => c.is_alphabetic(),
        // ASCII only, on purpose — see the module docs.
        'd' => c.is_ascii_digit(),
        'l' => c.is_lowercase(),
        's' => c.is_whitespace(),
        'u' => c.is_uppercase(),
        'w' => c.is_alphanumeric(),
        'x' => c.is_ascii_hexdigit(),
        'p' => is_punctuation(c),
        'c' => c.is_control(),
        // Printable except space, matching Lua's `%g`.
        'g' => !c.is_whitespace() && !c.is_control(),
        // Not a class letter: `%.` and friends stand for the literal.
        _ => return c == cl,
    };
    if cl.is_uppercase() { !res } else { res }
}

/// `%p`: punctuation and symbols, but not letters, digits, spaces or controls.
///
/// `char` has no single predicate for this. ASCII punctuation is listed by
/// `is_ascii_punctuation`; beyond ASCII, "not alphanumeric, not whitespace,
/// not control" is the same set Lua's `ispunct` selects for its locale.
fn is_punctuation(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control()
    }
}

impl<'a> MatchState<'a> {
    fn new(src: &'a [char], pat: &'a [char]) -> Self {
        MatchState {
            src,
            pat,
            level: 0,
            capture: [(0, 0); MAX_CAPTURES],
            depth: 0,
        }
    }

    /// Index just past the single pattern item starting at `p`.
    ///
    /// An item is one character, an escape (`%a`), or a whole bracket set —
    /// the unit that a following `*`, `+`, `-` or `?` applies to.
    fn class_end(&self, p: usize) -> Result<usize, String> {
        let Some(&c) = self.pat.get(p) else {
            return Err("malformed pattern: ends after `%`".to_string());
        };
        match c {
            '%' => {
                if p + 1 >= self.pat.len() {
                    return Err("malformed pattern: ends after `%`".to_string());
                }
                Ok(p + 2)
            }
            '[' => {
                let mut q = p + 1;
                // A `^` here negates the set and is not a member of it.
                if self.pat.get(q) == Some(&'^') {
                    q += 1;
                }
                // Step before testing, so a `]` in the first position is a
                // literal member rather than an empty set: `[]]` is the set
                // containing `]`.
                loop {
                    let Some(&cc) = self.pat.get(q) else {
                        return Err("malformed pattern: missing `]`".to_string());
                    };
                    q += 1;
                    // `%]` is a literal `]`, so it must not close the set.
                    if cc == '%' {
                        if q >= self.pat.len() {
                            return Err("malformed pattern: ends after `%`".to_string());
                        }
                        q += 1;
                    }
                    if self.pat.get(q) == Some(&']') {
                        return Ok(q + 1);
                    }
                }
            }
            _ => Ok(p + 1),
        }
    }

    /// Is `c` a member of the bracket set spanning `p..ec`? `p` is the `[`.
    fn match_bracket_class(&self, c: char, p: usize, ec: usize) -> bool {
        let mut sig = true;
        let mut i = p + 1;
        if self.pat.get(i) == Some(&'^') {
            sig = false;
            i += 1;
        }
        while i < ec {
            let ch = self.pat[i];
            if ch == '%' && i + 1 < ec {
                i += 1;
                if match_class(c, self.pat[i]) {
                    return sig;
                }
                i += 1;
            } else if i + 2 < ec && self.pat[i + 1] == '-' {
                // A range, `a-z`. Compared by scalar value, as Lua compares
                // by byte — for ASCII endpoints these agree exactly.
                if self.pat[i] <= c && c <= self.pat[i + 2] {
                    return sig;
                }
                i += 3;
            } else {
                if ch == c {
                    return sig;
                }
                i += 1;
            }
        }
        !sig
    }

    /// Does the single pattern item at `p..ep` match the character at `s`?
    fn single_match(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        match self.pat[p] {
            '.' => true,
            '%' => match_class(c, self.pat[p + 1]),
            '[' => self.match_bracket_class(c, p, ep - 1),
            pc => pc == c,
        }
    }

    /// `%bxy` — a balanced run from `x` to its matching `y`, nesting counted.
    fn match_balance(&mut self, s: usize, p: usize) -> Result<Option<usize>, String> {
        if p + 1 >= self.pat.len() {
            return Err("malformed pattern: `%b` needs two characters".to_string());
        }
        if self.src.get(s) != Some(&self.pat[p]) {
            return Ok(None);
        }
        let (open, close) = (self.pat[p], self.pat[p + 1]);
        let mut cont = 1;
        let mut i = s + 1;
        while i < self.src.len() {
            let c = self.src[i];
            if c == close {
                cont -= 1;
                if cont == 0 {
                    return Ok(Some(i + 1));
                }
            } else if c == open {
                cont += 1;
            }
            i += 1;
        }
        Ok(None)
    }

    /// Greedy: take as many as possible, then give one back per failure.
    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> Result<Option<usize>, String> {
        let mut i = 0usize;
        while self.single_match(s + i, p, ep) {
            i += 1;
        }
        loop {
            if let Some(r) = self.do_match(s + i, ep + 1)? {
                return Ok(Some(r));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    /// Lazy: take as few as possible, adding one per failure.
    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> Result<Option<usize>, String> {
        loop {
            if let Some(r) = self.do_match(s, ep + 1)? {
                return Ok(Some(r));
            }
            if self.single_match(s, p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: isize) -> Result<Option<usize>, String> {
        if self.level >= MAX_CAPTURES {
            return Err(format!("pattern has more than {MAX_CAPTURES} captures"));
        }
        self.capture[self.level] = (s, what);
        self.level += 1;
        let r = self.do_match(s, p)?;
        if r.is_none() {
            self.level -= 1;
        }
        Ok(r)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Result<Option<usize>, String> {
        let l = self.capture_to_close()?;
        self.capture[l].1 = (s - self.capture[l].0) as isize;
        let r = self.do_match(s, p)?;
        if r.is_none() {
            self.capture[l].1 = CAP_UNFINISHED;
        }
        Ok(r)
    }

    fn capture_to_close(&self) -> Result<usize, String> {
        for i in (0..self.level).rev() {
            if self.capture[i].1 == CAP_UNFINISHED {
                return Ok(i);
            }
        }
        Err("malformed pattern: `)` with no `(`".to_string())
    }

    /// `%1`–`%9` — match the same text a previous capture matched.
    fn match_capture(&mut self, s: usize, idx: char) -> Result<Option<usize>, String> {
        let l = (idx as usize)
            .checked_sub('1' as usize)
            .filter(|&l| l < self.level && self.capture[l].1 != CAP_UNFINISHED)
            .ok_or_else(|| format!("malformed pattern: invalid capture reference `%{idx}`"))?;
        let len = self.capture[l].1 as usize;
        let from = self.capture[l].0;
        if self.src.len() - s >= len && self.src[from..from + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }

    /// The matcher proper. Returns the end index on success.
    ///
    /// Structured as a loop rather than a tail call so that the cases Lua
    /// reaches with `goto init` — which advance `s`/`p` and restart — stay
    /// iterative and cannot grow the Rust stack.
    fn do_match(&mut self, mut s: usize, mut p: usize) -> Result<Option<usize>, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err("pattern too complex (recursion limit reached)".to_string());
        }
        let result = loop {
            if p >= self.pat.len() {
                break Some(s);
            }
            match self.pat[p] {
                '(' => {
                    if self.pat.get(p + 1) == Some(&')') {
                        self.depth -= 1;
                        return Err("position capture `()` is not supported: captures are a \
                                    `table<string>`. Use `String.find` for an index."
                            .to_string());
                    }
                    break self.start_capture(s, p + 1, CAP_UNFINISHED)?;
                }
                ')' => break self.end_capture(s, p + 1)?,
                // A `$` anywhere but the end is an ordinary character.
                '$' if p + 1 == self.pat.len() => {
                    break if s == self.src.len() { Some(s) } else { None };
                }
                '%' if p + 1 < self.pat.len() => {
                    match self.pat[p + 1] {
                        'b' => match self.match_balance(s, p + 2)? {
                            Some(ns) => {
                                s = ns;
                                p += 4;
                                continue;
                            }
                            None => break None,
                        },
                        'f' => {
                            p += 2;
                            if self.pat.get(p) != Some(&'[') {
                                self.depth -= 1;
                                return Err(
                                    "malformed pattern: `%f` must be followed by `[`".to_string()
                                );
                            }
                            let ep = self.class_end(p)?;
                            // The frontier sits *between* two characters, so
                            // it tests the one before and the one at `s`.
                            // Outside the subject both read as `\0`, which is
                            // how `%f[%w]` anchors at the start of a word.
                            let prev = if s == 0 { '\0' } else { self.src[s - 1] };
                            let cur = self.src.get(s).copied().unwrap_or('\0');
                            if !self.match_bracket_class(prev, p, ep - 1)
                                && self.match_bracket_class(cur, p, ep - 1)
                            {
                                p = ep;
                                continue;
                            }
                            break None;
                        }
                        c if c.is_ascii_digit() => match self.match_capture(s, c)? {
                            Some(ns) => {
                                s = ns;
                                p += 2;
                                continue;
                            }
                            None => break None,
                        },
                        // `%a`, `%.`, … fall through to the default handling.
                        _ => {}
                    }
                    match self.default_case(&mut s, &mut p)? {
                        Step::Continue => continue,
                        Step::Done(r) => break r,
                    }
                }
                _ => match self.default_case(&mut s, &mut p)? {
                    Step::Continue => continue,
                    Step::Done(r) => break r,
                },
            }
        };
        self.depth -= 1;
        Ok(result)
    }

    /// A single item, plus whatever quantifier follows it.
    fn default_case(&mut self, s: &mut usize, p: &mut usize) -> Result<Step, String> {
        let ep = self.class_end(*p)?;
        let quant = self.pat.get(ep).copied();
        if !self.single_match(*s, *p, ep) {
            // A failed item is fine if the quantifier allows zero of it.
            if matches!(quant, Some('*') | Some('?') | Some('-')) {
                *p = ep + 1;
                Ok(Step::Continue)
            } else {
                Ok(Step::Done(None))
            }
        } else {
            match quant {
                Some('?') => {
                    if let Some(r) = self.do_match(*s + 1, ep + 1)? {
                        Ok(Step::Done(Some(r)))
                    } else {
                        *p = ep + 1;
                        Ok(Step::Continue)
                    }
                }
                Some('+') => Ok(Step::Done(self.max_expand(*s + 1, *p, ep)?)),
                Some('*') => Ok(Step::Done(self.max_expand(*s, *p, ep)?)),
                Some('-') => Ok(Step::Done(self.min_expand(*s, *p, ep)?)),
                _ => {
                    *s += 1;
                    *p = ep;
                    Ok(Step::Continue)
                }
            }
        }
    }

    fn collect(&self, start: usize, end: usize) -> Result<Match, String> {
        let mut captures = Vec::with_capacity(self.level);
        for i in 0..self.level {
            let (cs, len) = self.capture[i];
            if len == CAP_UNFINISHED {
                return Err("malformed pattern: unfinished capture".to_string());
            }
            if len == CAP_POSITION {
                return Err("position capture `()` is not supported".to_string());
            }
            captures.push((cs, cs + len as usize));
        }
        Ok(Match {
            start,
            end,
            captures,
        })
    }
}

enum Step {
    Continue,
    Done(Option<usize>),
}

/// Does this pattern begin with the `^` anchor?
///
/// Callers need this because an anchored pattern can match at most once, and
/// each of the three functions owes that a different answer: `String.match`
/// returns it, `String.gsub` substitutes it and stops, and `String.gmatch`
/// refuses the pattern outright.
pub fn is_anchored(pat: &[char]) -> bool {
    pat.first() == Some(&'^')
}

/// Find the first match at or after character index `init`.
///
/// A leading `^` anchors the pattern: it is tried at `init` only, and a
/// failure there is a failure overall rather than a reason to slide forward.
pub fn find(src: &[char], pat: &[char], init: usize) -> Result<Option<Match>, String> {
    let anchored = is_anchored(pat);
    let pat_body = if anchored { &pat[1..] } else { pat };
    let mut s = init.min(src.len());
    loop {
        let mut ms = MatchState::new(src, pat_body);
        if let Some(e) = ms.do_match(s, 0)? {
            return Ok(Some(ms.collect(s, e)?));
        }
        if anchored || s >= src.len() {
            return Ok(None);
        }
        s += 1;
    }
}

/// Expand a `String.gsub` replacement template against one match.
///
/// `%1`–`%9` name a capture, `%0` the whole match, and `%%` a literal percent.
/// Any other `%x` is an error rather than a silent literal, because it is far
/// more likely to be a typo than an intent.
pub fn expand_replacement(template: &[char], src: &[char], m: &Match) -> Result<String, String> {
    let mut out = String::new();
    let mut i = 0;
    while i < template.len() {
        let c = template[i];
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        let Some(&next) = template.get(i + 1) else {
            return Err("malformed replacement: ends after `%`".to_string());
        };
        match next {
            '%' => out.push('%'),
            '0' => out.extend(&src[m.start..m.end]),
            d if d.is_ascii_digit() => {
                let idx = d as usize - '1' as usize;
                let texts = m.capture_texts(src);
                let text = texts.get(idx).ok_or_else(|| {
                    format!(
                        "replacement refers to capture `%{d}`, but the pattern has {}",
                        texts.len()
                    )
                })?;
                out.push_str(text);
            }
            other => {
                return Err(format!(
                    "malformed replacement: `%{other}` is not a capture reference \
                     (use `%%` for a literal `%`)"
                ));
            }
        }
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `find` on `&str`, returning the matched text — the shape most of these
    /// assertions want.
    fn m(src: &str, pat: &str) -> Option<String> {
        let s: Vec<char> = src.chars().collect();
        let p: Vec<char> = pat.chars().collect();
        find(&s, &p, 0)
            .unwrap()
            .map(|mt| s[mt.start..mt.end].iter().collect())
    }

    /// `find`, returning the captures as `capture_texts` would.
    fn caps(src: &str, pat: &str) -> Option<Vec<String>> {
        let s: Vec<char> = src.chars().collect();
        let p: Vec<char> = pat.chars().collect();
        find(&s, &p, 0).unwrap().map(|mt| mt.capture_texts(&s))
    }

    fn err(src: &str, pat: &str) -> String {
        let s: Vec<char> = src.chars().collect();
        let p: Vec<char> = pat.chars().collect();
        find(&s, &p, 0).unwrap_err()
    }

    #[test]
    fn literals_and_wildcards() {
        assert_eq!(m("hello", "ell").as_deref(), Some("ell"));
        assert_eq!(m("hello", "h.l").as_deref(), Some("hel"));
        assert_eq!(m("hello", "xyz"), None);
    }

    #[test]
    fn quantifiers_are_greedy_except_the_dash() {
        assert_eq!(m("<a><b>", "<.*>").as_deref(), Some("<a><b>"));
        assert_eq!(m("<a><b>", "<.->").as_deref(), Some("<a>"));
        assert_eq!(m("aaa", "a+").as_deref(), Some("aaa"));
        assert_eq!(m("bbb", "a*").as_deref(), Some(""));
        assert_eq!(m("colour", "colou?r").as_deref(), Some("colour"));
        assert_eq!(m("color", "colou?r").as_deref(), Some("color"));
    }

    #[test]
    fn anchors() {
        assert_eq!(m("hello", "^he").as_deref(), Some("he"));
        assert_eq!(m("hello", "^el"), None);
        assert_eq!(m("hello", "lo$").as_deref(), Some("lo"));
        assert_eq!(m("hello", "he$"), None);
        // A `$` that is not last is an ordinary character.
        assert_eq!(m("a$b", "a$b").as_deref(), Some("a$b"));
    }

    #[test]
    fn bracket_sets() {
        assert_eq!(m("abc123", "[0-9]+").as_deref(), Some("123"));
        assert_eq!(m("abc123", "[^0-9]+").as_deref(), Some("abc"));
        // A `]` in first position is a member, not the close.
        assert_eq!(m("a]b", "[]]").as_deref(), Some("]"));
        assert_eq!(m("x_9", "[%a_]+").as_deref(), Some("x_"));
        // An escape inside a set does not close it.
        assert_eq!(m("a]b", "[%]]").as_deref(), Some("]"));
    }

    #[test]
    fn captures_default_to_the_whole_match() {
        assert_eq!(caps("abc", "b").unwrap(), vec!["b"]);
        assert_eq!(caps("k=v", "(%a)=(%a)").unwrap(), vec!["k", "v"]);
        // Nested captures report outer first, as Lua does.
        assert_eq!(caps("abc", "((a)b)").unwrap(), vec!["ab", "a"]);
    }

    #[test]
    fn back_references_match_the_captured_text() {
        assert_eq!(m("abcabc", "(abc)%1").as_deref(), Some("abcabc"));
        assert_eq!(m("abcdef", "(abc)%1"), None);
    }

    #[test]
    fn balanced_and_frontier() {
        assert_eq!(m("f(a(b)c)x", "%b()").as_deref(), Some("(a(b)c)"));
        // The outer `(` never closes, so the scan slides forward and finds
        // the inner pair that does. Lua agrees.
        assert_eq!(m("f(a(b)c", "%b()").as_deref(), Some("(b)"));
        assert_eq!(m("f(abc", "%b()"), None);
        // `%f[%a]` sits at a transition into a letter.
        assert_eq!(m("  word", "%f[%a]%a+").as_deref(), Some("word"));
    }

    #[test]
    fn digit_class_is_ascii_but_letters_are_unicode() {
        // The two deliberate departures from Lua, pinned.
        assert_eq!(m("saulė", "%a+").as_deref(), Some("saulė"));
        assert_eq!(m("٣٤", "%d+"), None);
        assert_eq!(m("42", "%d+").as_deref(), Some("42"));
        // Uppercase class letters are the complement.
        assert_eq!(m("abc123", "%D+").as_deref(), Some("abc"));
    }

    #[test]
    fn indices_are_characters_not_bytes() {
        // "saulė" is 5 characters in 6 bytes. A byte-oriented matcher would
        // report 6 here, and could split the two-byte `ė`.
        let s: Vec<char> = "saulė".chars().collect();
        let p: Vec<char> = ".".chars().collect();
        let mt = find(&s, &p, 4).unwrap().unwrap();
        assert_eq!(mt.start, 4);
        assert_eq!(mt.end, 5);
        assert_eq!(s[mt.start..mt.end].iter().collect::<String>(), "ė");
    }

    #[test]
    fn malformed_patterns_are_errors_not_panics() {
        assert!(err("abc", "[abc").contains("missing `]`"));
        assert!(err("abc", "%").contains("ends after `%`"));
        assert!(err("abc", "(a").contains("unfinished capture"));
        assert!(err("abc", "a)").contains("`)` with no `(`"));
        assert!(err("abc", "()").contains("position capture"));
        assert!(err("abc", "%f%a").contains("`%f` must be followed by `[`"));
    }

    #[test]
    fn replacement_templates_expand_captures() {
        let s: Vec<char> = "john smith".chars().collect();
        let p: Vec<char> = "(%a+) (%a+)".chars().collect();
        let mt = find(&s, &p, 0).unwrap().unwrap();

        let t: Vec<char> = "%2, %1".chars().collect();
        assert_eq!(expand_replacement(&t, &s, &mt).unwrap(), "smith, john");

        let whole: Vec<char> = "[%0]".chars().collect();
        assert_eq!(expand_replacement(&whole, &s, &mt).unwrap(), "[john smith]");

        let pct: Vec<char> = "100%%".chars().collect();
        assert_eq!(expand_replacement(&pct, &s, &mt).unwrap(), "100%");

        let bad: Vec<char> = "%9".chars().collect();
        assert!(
            expand_replacement(&bad, &s, &mt)
                .unwrap_err()
                .contains("but the pattern has 2")
        );

        let typo: Vec<char> = "%q".chars().collect();
        assert!(
            expand_replacement(&typo, &s, &mt)
                .unwrap_err()
                .contains("not a capture reference")
        );
    }

    #[test]
    fn is_anchored_reports_the_caret() {
        assert!(is_anchored(&['^', 'a']));
        assert!(!is_anchored(&['a', '^']));
        assert!(!is_anchored(&[]));
    }

    #[test]
    fn a_pathological_pattern_errors_rather_than_hanging() {
        // Nested unbounded quantifiers are the classic backtracking blowup.
        // The depth guard turns it into an error instead of a wedged process.
        let s: Vec<char> = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab".chars().collect();
        let p: Vec<char> = "(a*)*c".chars().collect();
        let r = find(&s, &p, 0);
        assert!(
            r.is_err() || r.unwrap().is_none(),
            "must terminate, one way or the other"
        );
    }
}
