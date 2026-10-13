//! A Saule diagnostic as data, rather than as rendered text.
//!
//! `miette`'s rendered form is the right thing for a person reading a
//! terminal, and the wrong thing for anything that has to *act* on a
//! diagnostic: the box-drawing characters, the source excerpt and the
//! underline carets all have to be parsed back off again, and a parser for
//! them fails quietly the moment the renderer's layout changes.
//!
//! Three callers need the data form, and before this crate existed only one
//! of them had it:
//!
//!   * the **web playground**, which has always had it — [`Diagnostic`] is
//!     the shape `saule-wasm` already returned, lifted out of that crate so
//!     the two cannot drift apart;
//!   * **`saule check --json`**, so a build script, an editor that is not
//!     using the LSP, or a CI job can read the error list without screen
//!     scraping;
//!   * **training-data harnesses**, which run the compiler over very large
//!     numbers of generated programs and record what it said. This is the
//!     caller that makes a silent parse failure expensive: a regex that
//!     stops matching produces a corpus of unlabelled broken programs, and
//!     nothing downstream notices.
//!
//! # Why this is a leaf crate
//!
//! Every error type in the toolchain derives [`miette::Diagnostic`], and the
//! derive puts the span on a `#[label]`, the hint on `help(...)` and the
//! identifier on `code(...)`. [`to_diagnostic`] reads all three **off the
//! trait**, so it is generic over the error rather than a match over each
//! enum. The consequences are worth stating because they are the whole design:
//!
//!   * this crate depends on `miette` and `serde` and nothing else, so the
//!     lexer, parser, semantic pass, typechecker and VM can all be mapped
//!     through it without any of them depending on one another;
//!   * a new error variant anywhere in the toolchain needs no change here.
//!
//! # Stability
//!
//! The JSON is a published contract — the playground reads it in the
//! browser — so the field names and the [`Phase`] spellings are not free to
//! change. Adding a field is safe; `Option` fields are omitted entirely when
//! absent, so a diagnostic that carries no help text produces no `help` key.

use serde::Serialize;

/// Byte offsets into the source text, as a half-open range.
///
/// Byte offsets rather than line and column, because that is what the
/// compiler already has and what an editor needs to highlight a range. A
/// consumer that wants line and column computes them from the source it
/// already holds; one that wants to splice a fix needs exactly these.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// The length of the span in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span covers no bytes. A zero-width span is normal: it is
    /// what a "something is missing here" diagnostic points at.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// How bad a diagnostic is.
///
/// Only [`Severity::Error`] is produced today. The variant exists because the
/// shape allows warnings and the playground already renders them
/// differently, so a future warning needs no contract change.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Which pass produced a diagnostic.
///
/// This is the coarse classification a consumer gets for free, without the
/// toolchain having to name all 112 of its error variants. It is enough to
/// tell a syntax error from a type error from a runtime fault, which is the
/// distinction most callers actually branch on — and for a training harness,
/// enough to balance a generated dataset across error families.
///
/// The spellings are lowercase in JSON (`"lex"`, `"parse"`, …) and are part
/// of the published contract.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Reading the source at all: a missing file, or one that could not be
    /// read. Not a fault in the code, which is why it is not
    /// [`Phase::Import`] — a consumer that retries or reports differently for
    /// an environment problem than for a mistake in a program needs the two
    /// kept apart.
    Io,
    /// Tokenizing. A lexical error changes what the tokens are, so parse
    /// errors below it are downstream of it.
    Lex,
    /// Parsing, including recovered syntax errors.
    Parse,
    /// Resolving an `import`. Distinct from [`Phase::Semantic`] because the
    /// fix is a missing file or a missing dependency rather than a mistake in
    /// the code — and because the browser, which has no filesystem, can never
    /// emit it while the CLI routinely does.
    Import,
    /// Name resolution and the other semantic checks.
    Semantic,
    /// Type checking.
    Type,
    /// The bytecode compiler: a program the language rejects that the checker
    /// let through, an `import` where there is no filesystem behind the
    /// source, or a compiler limit.
    Compile,
    /// A fault while the program was running.
    Runtime,
}

impl Phase {
    /// The JSON spelling, for a caller assembling output by hand.
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Io => "io",
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Import => "import",
            Phase::Semantic => "semantic",
            Phase::Type => "type",
            Phase::Compile => "compile",
            Phase::Runtime => "runtime",
        }
    }
}

/// One diagnostic, as data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub phase: Phase,
    /// The one-line message, as `Display` renders the error.
    pub message: String,
    /// The first labelled range, where the error carries one.
    ///
    /// The *first*: an error may label several ranges, and the one the derive
    /// puts first is the one the renderer points its primary caret at. A
    /// consumer that needs the others needs a richer shape than this, and
    /// nothing has needed it yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// A stable identifier for the error kind, from `#[diagnostic(code(…))]`.
    ///
    /// Absent on every error variant today — none of them declare one yet.
    /// The field is wired now so that adding codes to the error enums is a
    /// change in those crates alone, and so a consumer can start keying off
    /// `code` the moment one appears rather than waiting for a contract bump.
    /// Until then, [`Phase`] is the available classification.
    ///
    /// One constraint on whoever writes that taxonomy: `code(...)` is parsed
    /// as a Rust path, so **no segment may be a Rust keyword**.
    /// `code(saule::type::bad_cast)` does not compile — it fails with
    /// `expected identifier, found keyword `type``. Spell it `types`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl Diagnostic {
    /// Build a diagnostic by hand, for a condition that has no error type
    /// behind it — a file that could not be read, say.
    pub fn new(phase: Phase, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            phase,
            message: message.into(),
            span: None,
            help: None,
            code: None,
        }
    }

    /// Attach a span.
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Attach help text.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Map any toolchain error onto a [`Diagnostic`].
///
/// Generic over the error rather than a match over each enum, because every
/// one of them derives [`miette::Diagnostic`] and therefore already carries
/// the span, the help text and the code. A new variant needs no work here.
pub fn to_diagnostic<E>(err: &E, phase: Phase) -> Diagnostic
where
    E: std::error::Error + miette::Diagnostic,
{
    Diagnostic {
        severity: Severity::Error,
        phase,
        message: err.to_string(),
        span: first_label(err),
        help: err.help().map(|h| h.to_string()),
        code: err.code().map(|c| c.to_string()),
    }
}

/// Map a `dyn` error, for a caller holding a trait object.
///
/// Separate from [`to_diagnostic`] because `&dyn miette::Diagnostic` does not
/// satisfy the generic bound: `miette::Diagnostic` requires `std::error::Error`
/// as a supertrait, but a `&dyn miette::Diagnostic` is not itself an
/// `std::error::Error` implementor.
pub fn to_diagnostic_dyn(err: &dyn miette::Diagnostic, phase: Phase) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        phase,
        message: err.to_string(),
        span: first_label_dyn(err),
        help: err.help().map(|h| h.to_string()),
        code: err.code().map(|c| c.to_string()),
    }
}

fn first_label<E>(err: &E) -> Option<Span>
where
    E: miette::Diagnostic,
{
    err.labels()
        .and_then(|mut labels| labels.next())
        .map(|label| Span {
            start: label.offset(),
            end: label.offset() + label.len(),
        })
}

fn first_label_dyn(err: &dyn miette::Diagnostic) -> Option<Span> {
    err.labels()
        .and_then(|mut labels| labels.next())
        .map(|label| Span {
            start: label.offset(),
            end: label.offset() + label.len(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic as _;
    use thiserror::Error;

    #[derive(Debug, Error, miette::Diagnostic)]
    #[error("cannot assign value of type `any` to variable of type `integer`")]
    #[diagnostic(
        code(saule::types::any_needs_cast),
        help("the value's type isn't compatible with the declared type")
    )]
    struct Labelled {
        #[label("incompatible type")]
        at: std::ops::Range<usize>,
    }

    #[derive(Debug, Error, miette::Diagnostic)]
    #[error("unexpected character")]
    struct Bare;

    #[test]
    fn reads_span_help_and_code_off_the_trait() {
        let d = to_diagnostic(&Labelled { at: 21..22 }, Phase::Type);

        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.phase, Phase::Type);
        assert_eq!(
            d.message,
            "cannot assign value of type `any` to variable of type `integer`"
        );
        assert_eq!(d.span, Some(Span { start: 21, end: 22 }));
        assert_eq!(
            d.help.as_deref(),
            Some("the value's type isn't compatible with the declared type")
        );
        assert_eq!(d.code.as_deref(), Some("saule::types::any_needs_cast"));
    }

    #[test]
    fn an_error_without_a_label_has_no_span() {
        let d = to_diagnostic(&Bare, Phase::Lex);
        assert_eq!(d.span, None);
        assert_eq!(d.help, None);
        assert_eq!(d.code, None);
    }

    /// The `dyn` path must agree with the generic one, or the two call sites
    /// would disagree about the same error.
    #[test]
    fn dyn_mapping_matches_the_generic_one() {
        let err = Labelled { at: 3..9 };
        let direct = to_diagnostic(&err, Phase::Type);
        let dynamic = to_diagnostic_dyn(&err as &dyn miette::Diagnostic, Phase::Type);
        assert_eq!(direct, dynamic);
    }

    /// The field names and the absence of empty keys are a published
    /// contract: the playground parses this in the browser.
    #[test]
    fn json_shape_is_the_published_contract() {
        let d = to_diagnostic(&Labelled { at: 21..22 }, Phase::Type);
        let json = serde_json::to_value(&d).unwrap();

        assert_eq!(json["severity"], "error");
        assert_eq!(json["phase"], "type");
        assert_eq!(json["span"]["start"], 21);
        assert_eq!(json["span"]["end"], 22);
        assert!(json["help"].is_string());
        assert_eq!(json["code"], "saule::types::any_needs_cast");
    }

    /// An absent optional is omitted, not serialized as `null`. A consumer
    /// written against the old two-field shape keeps working.
    #[test]
    fn absent_optionals_are_omitted_entirely() {
        let json = serde_json::to_value(to_diagnostic(&Bare, Phase::Lex)).unwrap();
        let obj = json.as_object().unwrap();

        assert!(!obj.contains_key("span"));
        assert!(!obj.contains_key("help"));
        assert!(!obj.contains_key("code"));
        assert_eq!(obj.len(), 3, "only severity, phase and message: {obj:?}");
    }

    #[test]
    fn phase_spellings_are_stable() {
        for (phase, spelling) in [
            (Phase::Io, "io"),
            (Phase::Lex, "lex"),
            (Phase::Parse, "parse"),
            (Phase::Import, "import"),
            (Phase::Semantic, "semantic"),
            (Phase::Type, "type"),
            (Phase::Compile, "compile"),
            (Phase::Runtime, "runtime"),
        ] {
            assert_eq!(phase.as_str(), spelling);
            assert_eq!(serde_json::to_value(phase).unwrap(), spelling);
        }
    }

    #[test]
    fn hand_built_diagnostics_carry_what_they_were_given() {
        let d = Diagnostic::new(Phase::Import, "could not find module `shine`")
            .with_span(Span { start: 0, end: 18 })
            .with_help("install it with `saule install`");

        assert_eq!(d.span.unwrap().len(), 18);
        assert!(!d.span.unwrap().is_empty());
        assert_eq!(d.help.as_deref(), Some("install it with `saule install`"));
    }

    #[test]
    fn a_zero_width_span_is_empty_but_valid() {
        let span = Span { start: 7, end: 7 };
        assert_eq!(span.len(), 0);
        assert!(span.is_empty());
    }

    /// `Labelled` declares a code, so the toolchain's own derive path is
    /// exercised; this guards the assumption that `code()` is readable off
    /// the trait at all.
    #[test]
    fn miette_exposes_the_declared_code() {
        assert_eq!(
            Labelled { at: 0..1 }
                .code()
                .map(|c| c.to_string())
                .as_deref(),
            Some("saule::types::any_needs_cast")
        );
    }
}
