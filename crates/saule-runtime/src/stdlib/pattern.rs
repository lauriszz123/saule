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
//! ## Characters or bytes
//!
//! The matcher is written once, over a [`Unit`]: a `char`, or a byte. Text
//! that is all ASCII has one byte to a character, so a subject and a pattern
//! that are both ASCII are matched as the bytes they already are, with
//! nothing decoded and every index the same number either way. Anything
//! else is decoded to characters first. Which of the two ran is not
//! something a program can tell: the classes are asked of the same `char`
//! in both.
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
#[derive(Debug, Clone, PartialEq)]
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

/// What a subject and a pattern are made of: characters, or bytes when
/// both are ASCII. See the module docs.
pub trait Unit: Copy + PartialEq + PartialOrd {
    /// The character this is. A byte is only ever an ASCII one here.
    fn ch(self) -> char;

    /// The class `%cl` as a row of [`ASCII_CLASS`] and whether to be in
    /// it, where a class can be read off that table: for bytes. `None`
    /// for characters, whose classes are Unicode's, and for a `cl` that
    /// names no class.
    fn class_row(cl: char) -> Option<(u16, bool)>;

    /// This unit's entry in [`ASCII_CLASS`]. Only asked of a byte.
    fn class_bits(self) -> u16;
}

impl Unit for char {
    #[inline(always)]
    fn ch(self) -> char {
        self
    }

    #[inline(always)]
    fn class_row(_cl: char) -> Option<(u16, bool)> {
        None
    }

    #[inline(always)]
    fn class_bits(self) -> u16 {
        0
    }
}

impl Unit for u8 {
    #[inline(always)]
    fn ch(self) -> char {
        self as char
    }

    #[inline(always)]
    fn class_row(cl: char) -> Option<(u16, bool)> {
        class_row(cl).map(|row| (row, cl.is_ascii_lowercase()))
    }

    #[inline(always)]
    fn class_bits(self) -> u16 {
        ASCII_CLASS[(self & 0x7F) as usize]
    }
}

/// The row of [`ASCII_CLASS`] for the class letter `cl`, of either case.
#[inline]
fn class_row(cl: char) -> Option<u16> {
    Some(match cl.to_ascii_lowercase() {
        'a' => 1 << 0,
        'd' => 1 << 1,
        'l' => 1 << 2,
        's' => 1 << 3,
        'u' => 1 << 4,
        'w' => 1 << 5,
        'x' => 1 << 6,
        'p' => 1 << 7,
        'c' => 1 << 8,
        'g' => 1 << 9,
        _ => return None,
    })
}

/// For each ASCII character, the classes it is in: a bit for each row
/// [`class_row`] names. What [`match_class`] answers for that character,
/// worked out once, so that a run of `%d` or `%a` over ASCII text is a
/// table read for each character and not a choice among ten classes.
static ASCII_CLASS: [u16; 128] = {
    let mut table = [0u16; 128];
    let mut i = 0;
    while i < 128 {
        let c = i as u8;
        let space = matches!(c, 9..=13 | 32);
        let mut bits = 0u16;
        if c.is_ascii_alphabetic() {
            bits |= 1 << 0;
        }
        if c.is_ascii_digit() {
            bits |= 1 << 1;
        }
        if c.is_ascii_lowercase() {
            bits |= 1 << 2;
        }
        if space {
            bits |= 1 << 3;
        }
        if c.is_ascii_uppercase() {
            bits |= 1 << 4;
        }
        if c.is_ascii_alphanumeric() {
            bits |= 1 << 5;
        }
        if c.is_ascii_hexdigit() {
            bits |= 1 << 6;
        }
        if c.is_ascii_punctuation() {
            bits |= 1 << 7;
        }
        if c.is_ascii_control() {
            bits |= 1 << 8;
        }
        if !space && !c.is_ascii_control() {
            bits |= 1 << 9;
        }
        table[i] = bits;
        i += 1;
    }
    table
};

/// One pattern item, read once for a loop that asks it of character after
/// character: the run a `+` or `*` takes, and the places a search steps
/// over.
#[derive(Clone, Copy)]
enum Item<U> {
    /// `.`
    Any,
    /// A character, or an escaped one.
    Is(U),
    /// A class of ASCII, as its row of [`ASCII_CLASS`] and whether to be
    /// in it.
    Class(u16, bool),
    /// A set, or a class of characters: asked as
    /// [`MatchState::single_match`] asks.
    Other,
}

struct MatchState<'a, U: Unit> {
    src: &'a [U],
    pat: &'a [U],
    level: usize,
    capture: Captures,
    depth: u32,
}

/// Where each open or closed capture starts and how long it is.
///
/// The first few are held in place and the rest, for the rare pattern that
/// has them, in a list. Thirty-two in place was 512 bytes to zero for
/// every search, and a line scanned word by word is a search for every
/// word; few patterns have more than three captures.
struct Captures {
    first: [(usize, isize); Captures::IN_PLACE],
    rest: Vec<(usize, isize)>,
}

impl Captures {
    const IN_PLACE: usize = 4;

    fn new() -> Captures {
        Captures {
            first: [(0, 0); Captures::IN_PLACE],
            rest: Vec::new(),
        }
    }

    /// Open capture number `level` at `s`: the one write that may be to a
    /// capture not yet there.
    fn open(&mut self, level: usize, s: usize, what: isize) {
        match level.checked_sub(Captures::IN_PLACE) {
            None => self.first[level] = (s, what),
            Some(i) if i < self.rest.len() => self.rest[i] = (s, what),
            Some(_) => self.rest.push((s, what)),
        }
    }
}

impl std::ops::Index<usize> for Captures {
    type Output = (usize, isize);
    #[inline]
    fn index(&self, i: usize) -> &(usize, isize) {
        match i.checked_sub(Captures::IN_PLACE) {
            None => &self.first[i],
            Some(i) => &self.rest[i],
        }
    }
}

impl std::ops::IndexMut<usize> for Captures {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut (usize, isize) {
        match i.checked_sub(Captures::IN_PLACE) {
            None => &mut self.first[i],
            Some(i) => &mut self.rest[i],
        }
    }
}

/// Does `c` belong to the class named by `cl`?
///
/// An uppercase class letter is the complement of its lowercase form, which is
/// why the result is flipped at the end rather than written out twice.
#[inline]
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
    // Every class letter is ASCII, so this is the whole of "is it the
    // capital": the Unicode question costs a table walk to say the same.
    if cl.is_ascii_uppercase() { !res } else { res }
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

impl<'a, U: Unit> MatchState<'a, U> {
    fn new(src: &'a [U], pat: &'a [U]) -> Self {
        MatchState {
            src,
            pat,
            level: 0,
            capture: Captures::new(),
            depth: 0,
        }
    }

    /// The pattern's character at `p`, if it has one there.
    #[inline(always)]
    fn at(&self, p: usize) -> Option<char> {
        self.pat.get(p).map(|u| u.ch())
    }

    /// Index just past the single pattern item starting at `p`.
    ///
    /// An item is one character, an escape (`%a`), or a whole bracket set —
    /// the unit that a following `*`, `+`, `-` or `?` applies to.
    fn class_end(&self, p: usize) -> Result<usize, String> {
        let Some(&c) = self.pat.get(p) else {
            return Err("malformed pattern: it ends with a lone `%` — `%` escapes the character after it; write `%%` for a literal percent sign".to_string());
        };
        match c.ch() {
            '%' => {
                if p + 1 >= self.pat.len() {
                    return Err("malformed pattern: it ends with a lone `%` — `%` escapes the character after it; write `%%` for a literal percent sign".to_string());
                }
                Ok(p + 2)
            }
            '[' => {
                let mut q = p + 1;
                // A `^` here negates the set and is not a member of it.
                if self.at(q) == Some('^') {
                    q += 1;
                }
                // Step before testing, so a `]` in the first position is a
                // literal member rather than an empty set: `[]]` is the set
                // containing `]`.
                loop {
                    let Some(&cc) = self.pat.get(q) else {
                        return Err("malformed pattern: a `[` set is never closed — end it with `]`, or write `%[` for a literal bracket".to_string());
                    };
                    q += 1;
                    // `%]` is a literal `]`, so it must not close the set.
                    if cc.ch() == '%' {
                        if q >= self.pat.len() {
                            return Err("malformed pattern: it ends with a lone `%` — `%` escapes the character after it; write `%%` for a literal percent sign".to_string());
                        }
                        q += 1;
                    }
                    if self.at(q) == Some(']') {
                        return Ok(q + 1);
                    }
                }
            }
            _ => Ok(p + 1),
        }
    }

    /// The item at `p`, whose end [`Self::class_end`] has found.
    #[inline]
    fn item(&self, p: usize) -> Item<U> {
        let pc = self.pat[p];
        match pc.ch() {
            '.' => Item::Any,
            '[' => Item::Other,
            '%' => {
                let cl = self.pat[p + 1];
                match U::class_row(cl.ch()) {
                    Some((row, within)) => Item::Class(row, within),
                    None if class_row(cl.ch()).is_some() => Item::Other,
                    None => Item::Is(cl),
                }
            }
            _ => Item::Is(pc),
        }
    }

    /// Does `item`, the one at `p..ep`, match the character at `s`? What
    /// [`Self::single_match`] answers.
    #[inline(always)]
    fn holds(&self, item: Item<U>, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        match item {
            Item::Any => true,
            Item::Is(pc) => pc == c,
            Item::Class(row, within) => (c.class_bits() & row != 0) == within,
            Item::Other => self.single_match(s, p, ep),
        }
    }

    /// Is `c` a member of the bracket set spanning `p..ec`? `p` is the `[`.
    fn match_bracket_class(&self, c: U, p: usize, ec: usize) -> bool {
        let mut sig = true;
        let mut i = p + 1;
        if self.at(i) == Some('^') {
            sig = false;
            i += 1;
        }
        while i < ec {
            let ch = self.pat[i];
            if ch.ch() == '%' && i + 1 < ec {
                i += 1;
                if match_class(c.ch(), self.pat[i].ch()) {
                    return sig;
                }
                i += 1;
            } else if i + 2 < ec && self.pat[i + 1].ch() == '-' {
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
    #[inline]
    fn single_match(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        let pc = self.pat[p];
        match pc.ch() {
            '.' => true,
            '%' => match_class(c.ch(), self.pat[p + 1].ch()),
            '[' => self.match_bracket_class(c, p, ep - 1),
            _ => pc == c,
        }
    }

    /// `%bxy` — a balanced run from `x` to its matching `y`, nesting counted.
    fn match_balance(&mut self, s: usize, p: usize) -> Result<Option<usize>, String> {
        if p + 1 >= self.pat.len() {
            return Err("malformed pattern: `%b` needs two characters after it — the opening and closing delimiter, e.g. `%b()` matches balanced parentheses".to_string());
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
        let item = self.item(p);
        let mut i = 0usize;
        while self.holds(item, s + i, p, ep) {
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
            return Err(format!(
                "pattern has more than {MAX_CAPTURES} captures — split it into smaller patterns, or use `%(` for a literal parenthesis"
            ));
        }
        self.capture.open(self.level, s, what);
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
        Err("malformed pattern: `)` closes a capture that was never opened — write `%)` for a literal parenthesis".to_string())
    }

    /// `%1`–`%9` — match the same text a previous capture matched.
    fn match_capture(&mut self, s: usize, idx: char) -> Result<Option<usize>, String> {
        let l = (idx as usize)
            .checked_sub('1' as usize)
            .filter(|&l| l < self.level && self.capture[l].1 != CAP_UNFINISHED)
            .ok_or_else(|| {
                format!(
                    "malformed pattern: `%{idx}` refers to capture {idx}, which doesn't exist at this point — `%1`–`%9` repeat a `( … )` capture that comes earlier in the pattern"
                )
            })?;
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
            return Err(
                "pattern too complex: matching it nested too deeply — simplify it, e.g. replace a run of optional items with a single `*`"
                    .to_string(),
            );
        }
        let result = loop {
            if p >= self.pat.len() {
                break Some(s);
            }
            match self.pat[p].ch() {
                '(' => {
                    if self.at(p + 1) == Some(')') {
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
                    match self.pat[p + 1].ch() {
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
                            if self.at(p) != Some('[') {
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
                            let before = if s == 0 { None } else { Some(self.src[s - 1]) };
                            let here = self.src.get(s).copied();
                            if !self.in_set_or_nul(before, p, ep - 1)
                                && self.in_set_or_nul(here, p, ep - 1)
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

    /// [`Self::match_bracket_class`] for a place that may be outside the
    /// subject, which reads as `\0` there: no unit of the subject, so it is
    /// asked of the set as the character.
    fn in_set_or_nul(&self, c: Option<U>, p: usize, ec: usize) -> bool {
        match c {
            Some(c) => self.match_bracket_class(c, p, ec),
            None => self.nul_in_set(p, ec),
        }
    }

    /// Is `\0` a member of the bracket set spanning `p..ec`? As
    /// [`Self::match_bracket_class`] answers for a unit of the subject.
    fn nul_in_set(&self, p: usize, ec: usize) -> bool {
        let c = '\0';
        let mut sig = true;
        let mut i = p + 1;
        if self.at(i) == Some('^') {
            sig = false;
            i += 1;
        }
        while i < ec {
            let ch = self.pat[i].ch();
            if ch == '%' && i + 1 < ec {
                i += 1;
                if match_class(c, self.pat[i].ch()) {
                    return sig;
                }
                i += 1;
            } else if i + 2 < ec && self.pat[i + 1].ch() == '-' {
                if self.pat[i].ch() <= c && c <= self.pat[i + 2].ch() {
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

    /// A single item, plus whatever quantifier follows it.
    fn default_case(&mut self, s: &mut usize, p: &mut usize) -> Result<Step, String> {
        let ep = self.class_end(*p)?;
        let quant = self.at(ep);
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

    /// Where the pattern's first item ends, when that item is one that has
    /// to match the character an attempt starts at: a character, a class or
    /// a set, taken at least once. `None` for anything else — a capture, an
    /// anchor, `%b`, `%f`, a back-reference, or an item that may be taken
    /// no times — where an attempt has to be made to know.
    ///
    /// A failed attempt at such a place changes nothing and reports
    /// nothing, so skipping it is not seen. An item that is malformed is
    /// said here, as the first attempt would say it.
    fn leading_item(&self) -> Result<Option<usize>, String> {
        let Some(first) = self.at(0) else {
            return Ok(None);
        };
        match first {
            '(' | ')' => return Ok(None),
            '$' if self.pat.len() == 1 => return Ok(None),
            '%' => match self.at(1) {
                Some('b') | Some('f') => return Ok(None),
                Some(c) if c.is_ascii_digit() => return Ok(None),
                _ => {}
            },
            _ => {}
        }
        let ep = self.class_end(0)?;
        Ok(match self.at(ep) {
            Some('*') | Some('?') | Some('-') => None,
            _ => Some(ep),
        })
    }

    fn collect(&self, start: usize, end: usize) -> Result<Match, String> {
        let mut captures = Vec::with_capacity(self.level);
        for i in 0..self.level {
            let (cs, len) = self.capture[i];
            if len == CAP_UNFINISHED {
                return Err(
                    "malformed pattern: unfinished capture — a `(` is never closed; end it with `)`, or write `%(` for a literal parenthesis"
                        .to_string(),
                );
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
pub fn is_anchored<U: Unit>(pat: &[U]) -> bool {
    pat.first().is_some_and(|u| u.ch() == '^')
}

/// Find the first match at or after character index `init`.
///
/// A leading `^` anchors the pattern: it is tried at `init` only, and a
/// failure there is a failure overall rather than a reason to slide forward.
pub fn find<U: Unit>(src: &[U], pat: &[U], init: usize) -> Result<Option<Match>, String> {
    let anchored = is_anchored(pat);
    let pat_body = if anchored { &pat[1..] } else { pat };
    let mut s = init.min(src.len());
    // One state for every start position, reset rather than rebuilt.
    // Nothing reads a capture at or above `level`, and every one below it
    // is written when it opens, so `level` and `depth` back at zero is
    // exactly a fresh state, whatever an earlier attempt left behind.
    let mut ms = MatchState::new(src, pat_body);
    // Where the pattern's first item can't match, nothing can: those places
    // are stepped over here, a comparison each, instead of being found out
    // one failed attempt at a time. `code=(%d+)` over a line of fifty
    // characters was thirty-odd attempts before the `c`.
    let lead = match anchored {
        true => None,
        false => ms.leading_item()?.map(|ep| (ms.item(0), ep)),
    };
    loop {
        if let Some((item, ep)) = lead {
            while s < src.len() && !ms.holds(item, s, 0, ep) {
                s += 1;
            }
            if s >= src.len() {
                return Ok(None);
            }
        }
        ms.level = 0;
        ms.depth = 0;
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
    expand_replacement_into(template, src, m, &mut out)?;
    Ok(out)
}

/// [`expand_replacement`], appending to `out` — what `String.gsub` uses, so a
/// replacement is written straight into the result rather than built in a
/// `String` of its own per match and copied across.
///
/// A capture reference reads that capture's span directly. It used to call
/// [`Match::capture_texts`], which builds a `String` for *every* capture and
/// a `Vec` to hold them, to use one: `"<%2:%1>"` over two captures was six
/// allocations per match thrown away. The rule `capture_texts` encodes is
/// kept — a pattern with no captures has its whole match as capture `1` —
/// and so is the message for a reference past the last one.
pub fn expand_replacement_into(
    template: &[char],
    src: &[char],
    m: &Match,
    out: &mut String,
) -> Result<(), String> {
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
                let span = if m.captures.is_empty() {
                    (idx == 0).then_some((m.start, m.end))
                } else {
                    m.captures.get(idx).copied()
                };
                let (from, to) = span.ok_or_else(|| {
                    format!(
                        "replacement refers to capture `%{d}`, but the pattern has {} — \
                         `%1`–`%9` insert what the pattern's `( … )` captures matched, and `%0` the whole match",
                        m.captures.len().max(1)
                    )
                })?;
                out.extend(&src[from..to]);
            }
            other => {
                return Err(format!(
                    "malformed replacement: `%{other}` is not a capture reference — \
                     in a replacement, `%` is followed by a digit `0`–`9`; write `%%` for a literal `%`"
                ));
            }
        }
        i += 2;
    }
    Ok(())
}

/// [`expand_replacement_into`] for a subject that is all ASCII and was
/// matched as bytes: `m`'s offsets are byte offsets into `src`, and the
/// template is read as the text it is. The same rules and the same
/// messages.
pub fn expand_replacement_ascii(
    template: &str,
    src: &str,
    m: &Match,
    out: &mut String,
) -> Result<(), String> {
    let mut rest = template;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let mut after = rest[at + 1..].chars();
        let Some(next) = after.next() else {
            return Err("malformed replacement: ends after `%`".to_string());
        };
        match next {
            '%' => out.push('%'),
            '0' => out.push_str(&src[m.start..m.end]),
            d if d.is_ascii_digit() => {
                let idx = d as usize - '1' as usize;
                let span = if m.captures.is_empty() {
                    (idx == 0).then_some((m.start, m.end))
                } else {
                    m.captures.get(idx).copied()
                };
                let (from, to) = span.ok_or_else(|| {
                    format!(
                        "replacement refers to capture `%{d}`, but the pattern has {} — \
                         `%1`–`%9` insert what the pattern's `( … )` captures matched, and `%0` the whole match",
                        m.captures.len().max(1)
                    )
                })?;
                out.push_str(&src[from..to]);
            }
            other => {
                return Err(format!(
                    "malformed replacement: `%{other}` is not a capture reference — \
                     in a replacement, `%` is followed by a digit `0`–`9`; write `%%` for a literal `%`"
                ));
            }
        }
        rest = after.as_str();
    }
    out.push_str(rest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `find` over characters, as text that is not all ASCII is matched.
    /// Text that is all ASCII is matched as bytes: whenever both are, the
    /// two are run here and have to say the same, so every assertion in
    /// this module is made of both.
    fn found(src: &str, pat: &str) -> (Vec<char>, Result<Option<Match>, String>) {
        let s: Vec<char> = src.chars().collect();
        let p: Vec<char> = pat.chars().collect();
        let by_chars = find(&s, &p, 0);
        if src.is_ascii() && pat.is_ascii() {
            let by_bytes = find(src.as_bytes(), pat.as_bytes(), 0);
            assert_eq!(by_bytes, by_chars, "{pat:?} over {src:?}, as bytes");
        }
        (s, by_chars)
    }

    /// The matched text — the shape most of these assertions want.
    fn m(src: &str, pat: &str) -> Option<String> {
        let (s, found) = found(src, pat);
        found
            .unwrap()
            .map(|mt| s[mt.start..mt.end].iter().collect())
    }

    /// The captures, as `capture_texts` would give them.
    fn caps(src: &str, pat: &str) -> Option<Vec<String>> {
        let (s, found) = found(src, pat);
        found.unwrap().map(|mt| mt.capture_texts(&s))
    }

    fn err(src: &str, pat: &str) -> String {
        found(src, pat).1.unwrap_err()
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
        assert!(err("abc", "[abc").contains("a `[` set is never closed"));
        assert!(err("abc", "%").contains("ends with a lone `%`"));
        assert!(err("abc", "(a").contains("unfinished capture"));
        assert!(err("abc", "a)").contains("`)` closes a capture that was never opened"));
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
        assert!(!is_anchored::<char>(&[]));
        assert!(is_anchored(b"^a"));
        assert!(!is_anchored(b"a^"));
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

    /// `find` keeps one state across the start positions it tries, reset
    /// rather than rebuilt. An attempt that opens captures and then fails must
    /// leave nothing a later attempt can see — not a capture, not the text a
    /// back-reference compares against.
    #[test]
    fn a_failed_attempt_leaves_no_capture_behind() {
        // Two captures open at `ab_` and again at `abd` before `abc` succeeds.
        assert_eq!(caps("ab_abd_abc", "(a)(b)c").unwrap(), vec!["a", "b"]);
        // Every attempt before `zz` captures a different letter for `%1`.
        assert_eq!(m("xa yb zz", "(%a)%1").unwrap(), "zz");
        // A deeper failure first: three levels open, then the last fails.
        assert_eq!(
            caps("((x)y ((x)z", "%((%((%a)%)z)").unwrap(),
            vec!["(x)z", "x"]
        );
        assert_eq!(m("abc abd", "(a)(b)e"), None);
    }

    /// A pattern with no captures has its whole match as capture `1`, and
    /// nothing as `2` — the rule `capture_texts` encodes, which the in-place
    /// expansion now applies without building every capture's text.
    #[test]
    fn a_replacement_reads_captures_by_span() {
        let s: Vec<char> = "ab cd".chars().collect();
        let mt = find(&s, &"%a+".chars().collect::<Vec<_>>(), 0)
            .unwrap()
            .unwrap();
        let t: Vec<char> = "<%1|%0>".chars().collect();
        assert_eq!(expand_replacement(&t, &s, &mt).unwrap(), "<ab|ab>");
        let two: Vec<char> = "%2".chars().collect();
        assert!(
            expand_replacement(&two, &s, &mt)
                .unwrap_err()
                .starts_with("replacement refers to capture `%2`, but the pattern has 1 — ")
        );
        // Appending: what is already in the buffer stays in front.
        let mut out = String::from("pre:");
        expand_replacement_into(&t, &s, &mt, &mut out).unwrap();
        assert_eq!(out, "pre:<ab|ab>");
    }

    /// Places where the pattern's first item can't match are stepped over
    /// without an attempt. What is found, and what is said of a malformed
    /// pattern, has to be what an attempt at every place would have given.
    #[test]
    fn stepping_over_where_the_first_item_cannot_match_changes_nothing() {
        assert_eq!(caps("took=41ms code=7 ", "code=(%d+)").unwrap(), vec!["7"]);
        assert_eq!(m("ab  cd", "%a+").as_deref(), Some("ab"));
        assert_eq!(m("  42 7", "%d+").as_deref(), Some("42"));
        assert_eq!(m("..x.", "[%a_]").as_deref(), Some("x"));
        assert_eq!(m("abc", "%d+"), None);
        assert_eq!(m("", "a"), None);
        // An item that may be taken no times matches where it stands.
        assert_eq!(m("bbb", "a*").as_deref(), Some(""));
        assert_eq!(m("bbb", "a?b").as_deref(), Some("b"));
        assert_eq!(m("bbb", "a-b").as_deref(), Some("b"));
        assert_eq!(m("", "a*").as_deref(), Some(""));
        // What cannot be skipped by its first character.
        assert_eq!(m("x(a)", "%b()").as_deref(), Some("(a)"));
        assert_eq!(m("  word", "%f[%a]").as_deref(), Some(""));
        assert_eq!(m("xab", "(a)b").as_deref(), Some("ab"));
        assert_eq!(m("ab", "$"), Some(String::new()));
        assert_eq!(m("a$", "$").as_deref(), Some(""));
        assert_eq!(m("a$b", "$b").as_deref(), Some("$b"));
        // Malformed first items are said whatever the subject holds.
        assert!(err("", "%").contains("ends with a lone `%`"));
        assert!(err("zzz", "[abc").contains("a `[` set is never closed"));
        assert!(err("zzz", ")").contains("never opened"));
        assert!(err("zzz", "()").contains("position capture"));
        // One that is malformed further on is said when it is reached, and
        // not before, as it always was.
        assert_eq!(m("zzz", "a[bc"), None);
        assert!(err("zza", "a[bc").contains("a `[` set is never closed"));
    }

    /// A replacement written into text matched as bytes is the same text,
    /// with the same refusals.
    #[test]
    fn a_replacement_for_ascii_text_is_the_same_text() {
        let src = "john smith";
        let chars: Vec<char> = src.chars().collect();
        let by_bytes = find(src.as_bytes(), "(%a+) (%a+)".as_bytes(), 0)
            .unwrap()
            .unwrap();
        // A template need not be ASCII for its subject to be.
        for template in ["%2, %1", "[%0]", "100%%", "ė%1ž%%%2—", "", "plain", "%9", "%q", "x%"] {
            let t: Vec<char> = template.chars().collect();
            let mut out = String::from("pre:");
            let got = expand_replacement_ascii(template, src, &by_bytes, &mut out).map(|()| out);
            let mut out = String::from("pre:");
            let want = expand_replacement_into(&t, &chars, &by_bytes, &mut out).map(|()| out);
            assert_eq!(got, want, "{template:?}");
        }
        // No captures: the whole match is capture 1, and there is no 2.
        let whole = find(src.as_bytes(), "%a+".as_bytes(), 0).unwrap().unwrap();
        let mut out = String::new();
        expand_replacement_ascii("<%1|%0>", src, &whole, &mut out).unwrap();
        assert_eq!(out, "<john|john>");
        assert!(
            expand_replacement_ascii("%2", src, &whole, &mut String::new())
                .unwrap_err()
                .starts_with("replacement refers to capture `%2`, but the pattern has 1 — ")
        );
    }

    /// A class of ASCII is read off a table when text is matched as bytes.
    /// For every ASCII character and every letter after a `%`, the table
    /// has to say what the class says.
    #[test]
    fn the_table_of_ascii_classes_agrees_with_the_classes() {
        for cl in 0u8..128 {
            for c in 0u8..128 {
                let by_table = match <u8 as Unit>::class_row(cl as char) {
                    Some((row, within)) => (c.class_bits() & row != 0) == within,
                    None => c == cl,
                };
                assert_eq!(
                    by_table,
                    match_class(c as char, cl as char),
                    "%{} of {:?}",
                    cl as char,
                    c as char
                );
            }
        }
        // And through the matcher, every class over every ASCII character.
        let all: String = (0u8..128).map(|c| c as char).collect();
        for cl in "adlsuwxpcgADLSUWXPCG.%-".chars() {
            for pat in [format!("%{cl}+"), format!("x%{cl}*"), format!("[%{cl}]+")] {
                let _ = found(&all, &pat);
            }
        }
    }

    /// The first four captures are held in place and the rest in a list.
    /// A pattern with more than four behaves as one with fewer does:
    /// through failed attempts, back-references and the limit.
    #[test]
    fn captures_past_the_ones_held_in_place() {
        let six = "(a)(b)(c)(d)(e)(f)";
        assert_eq!(
            caps("abcdef", six).unwrap(),
            vec!["a", "b", "c", "d", "e", "f"]
        );
        // Five open and close before the sixth fails, twice, then all six.
        assert_eq!(
            caps("abcde_abcdeX abcdef", six).unwrap(),
            vec!["a", "b", "c", "d", "e", "f"]
        );
        assert_eq!(m("abcde_abcdeX", six), None);
        // Nested, so the later ones close before the earlier.
        assert_eq!(
            caps("abcdef", "(a(b(c(d(e(f))))))").unwrap(),
            vec!["abcdef", "bcdef", "cdef", "def", "ef", "f"]
        );
        // A back-reference to one in the list, and a greedy run that gives
        // characters back across them.
        assert_eq!(m("abcdee", "(a)(b)(c)(d)(e)%5").as_deref(), Some("abcdee"));
        assert_eq!(m("abcdef", "(a)(b)(c)(d)(e)%5"), None);
        assert_eq!(
            caps("1 2 3 4 5 66", "(%d+) (%d+) (%d+) (%d+) (%d+) (%d+)6").unwrap(),
            vec!["1", "2", "3", "4", "5", "6"]
        );
        // The limit is where it was.
        let many = "(a)".repeat(32);
        assert_eq!(caps(&"a".repeat(32), &many).unwrap().len(), 32);
        assert!(err(&"a".repeat(33), &"(a)".repeat(33)).contains("more than 32 captures"));
        // An unclosed one past the fourth is still said.
        assert!(err("abcde", "(a)(b)(c)(d)(e").contains("unfinished capture"));
    }
}
