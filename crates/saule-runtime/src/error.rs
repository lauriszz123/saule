//! Runtime errors: what can go wrong while a program runs.
//!
//! Kept disjoint from the compile-time error families:
//!
//! * `saule_lexer::LexerError` — lexing
//! * `saule_parser::ParseError` — parsing
//! * `saule_semantic::SemanticError` — structural / control-flow / definite
//!   assignment / name resolution
//! * `saule_typeck::TypeCheckError` — types, nullability, match exhaustiveness,
//!   unknown members, function arity, …
//! * `RuntimeError` (this file) — only genuinely-dynamic failures: division
//!   by zero, force-unwrap of `nil`, uncaught `throw`, file-I/O, etc.
//!
//! The variants below are the residual set after semantic + typeck have run.
//! Anything caught by an earlier pass has been removed; if you find yourself
//! about to add a new variant here, consider whether semantic or typeck is
//! the more appropriate owner first.

use miette::Diagnostic;
use thiserror::Error;

/// A built-in's signature as DOCS.md writes it — parameter names included,
/// which the checker's own signature table doesn't have.
fn documented_signature(qname: &str) -> Option<String> {
    let entry = saule_docs::stdlib().entry(qname)?;
    (entry.kind == saule_docs::EntryKind::Function).then(|| entry.signature.clone())
}

/// A type's name with the article English puts in front of it, for a
/// diagnostic that says "this is …": "a `string`", "an `integer`", "an
/// `Item`", and `nil` bare, since there is only one.
pub fn a_type(name: &str) -> String {
    match name {
        "nil" => "`nil`".to_string(),
        _ if name.starts_with(['a', 'e', 'i', 'o', 'u', 'A', 'E', 'I', 'O', 'U']) => {
            format!("an `{name}`")
        }
        _ => format!("a `{name}`"),
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum RuntimeError {
    /// A failure the program can cause, which the checker could not rule out:
    /// an operand of the wrong kind arriving through `any`, a member that
    /// isn't there, a built-in function rejecting its argument.
    ///
    /// Written the way a Rust diagnostic is, in three parts that each do one
    /// job: `message` says what went wrong and nothing else; `label`, under
    /// the caret, says what *this* expression has to do with it ("this is a
    /// `string`"); `help` says what to change. Advice never goes in the
    /// message, where it turns a title into a paragraph.
    ///
    /// Build one with [`RuntimeError::fault`], or [`RuntimeError::native`]
    /// for a built-in function's `Err(String)`.
    #[error("{message}")]
    Fault {
        message: String,
        #[label("{label}")]
        span: std::ops::Range<usize>,
        label: String,
        #[help]
        help: Option<String>,
    },

    /// A broken invariant inside the engine: a chunk disagreeing with the
    /// layout it was compiled against, an opcode that cannot occur. Never the
    /// program's fault, and the message says so rather than leaving the
    /// reader to wonder what they did wrong.
    #[error("internal error: {}", .message.trim_start_matches("internal: "))]
    #[diagnostic(help(
        "this is a bug in Saule, not in your program — please report it, with the program that triggered it, at https://github.com/lauriszz123/saule/issues"
    ))]
    Internal {
        message: String,
        #[label("while running this")]
        span: std::ops::Range<usize>,
    },

    /// `error(msg, level)` — a failure the program raised on purpose.
    ///
    /// Its own variant because nothing went wrong with the *language* here:
    /// the message is the program's, so it is shown as written, with no
    /// category in front of it. `span` is the call at `level`: the `error`
    /// call itself at level 1, the call into the function that raised it at
    /// level 2.
    #[error("{message}")]
    Raised {
        message: String,
        #[label("{label}")]
        span: std::ops::Range<usize>,
        /// What the caret says, which depends on whose line it is.
        label: String,
        /// Where `error` itself was called — the same as `span` at level 1.
        /// The stack trace's innermost frame is reported at this line, not
        /// at the line the level blames.
        origin: usize,
    },

    #[error("cannot mix an `integer` and a `float` in arithmetic")]
    #[diagnostic(help(
        "convert one side so both match: `(n as float)` keeps the fraction, `(x as integer)` drops it"
    ))]
    NumericMix {
        #[label("one side is an `integer`, the other a `float`")]
        span: std::ops::Range<usize>,
    },

    /// Integer `/` or `%` by zero. Float division by zero is not an error —
    /// it is `inf` or NaN, as IEEE 754 says.
    #[error("integer division by zero")]
    #[diagnostic(help(
        "check the divisor first: `if d != 0 then … end` (float division by zero gives `inf` instead)"
    ))]
    DivisionByZero {
        #[label("the divisor here is 0")]
        span: std::ops::Range<usize>,
    },

    #[error("`for` loop step cannot be 0")]
    #[diagnostic(help(
        "a step of 0 never reaches the limit; use a positive step to count up or a negative one to count down, e.g. `for i = 10, 1, -1 do`"
    ))]
    ZeroStep {
        #[label("the step here is 0")]
        span: std::ops::Range<usize>,
    },

    #[error("force-unwrapped a `nil` value")]
    #[diagnostic(help(
        "use `??` to provide a fallback, or check with `if x != nil` before unwrapping"
    ))]
    ForceUnwrapNil {
        #[label("this expression was `nil` when `!` was applied")]
        span: std::ops::Range<usize>,
    },

    #[error("`{thing}` is not yet implemented")]
    Unsupported {
        thing: &'static str,
        #[label("this feature will be supported in a future version")]
        span: std::ops::Range<usize>,
    },

    /// A coroutine yielding: not an error, but the way a `yield` leaves the
    /// dispatch loop. The loop's own error path is the one exit that keeps
    /// every frame where it is, which is exactly what suspending needs.
    ///
    /// It never escapes the VM. The coroutine's `resume` is the only thing
    /// that runs a dispatch loop a `yield` can be raised in, and it turns
    /// this back into the yielded values. If one is ever rendered, that is a
    /// VM bug, and the message says so.
    #[error("internal: a coroutine `yield` escaped the VM")]
    Yield,

    #[error("import error: {message}")]
    ImportError {
        message: String,
        #[label("could not resolve this import")]
        span: std::ops::Range<usize>,
    },

    /// An error that bubbled up from an imported module. Carries its own
    /// `NamedSource` so miette renders the snippet from the *imported* file
    /// rather than the importer's source. The original error is preserved
    /// as a nested `#[diagnostic_source]` so the user sees both the import
    /// site (in the parent file) and the actual offending line.
    #[error("error in imported module `{module_label}`")]
    ImportFailed {
        module_label: String,
        #[label("imported here")]
        import_span: std::ops::Range<usize>,
        #[diagnostic_source]
        inner: Box<dyn miette::Diagnostic + Send + Sync + 'static>,
    },

    /// A runtime error fired while executing code defined in an imported
    /// module *after* it had finished loading (e.g. a method called from
    /// the entry file later on). Carries the imported module's source so
    /// miette renders the snippet from the right file even though no
    /// `import` statement is on the call stack at the moment of failure.
    #[error("runtime error in `{module_label}`")]
    InModule {
        module_label: String,
        #[diagnostic_source]
        inner: Box<dyn miette::Diagnostic + Send + Sync + 'static>,
    },

    /// A `throw` no `try ... catch` caught, reported at the top of the
    /// program. The VM unwinds a throw through its own handler tables and
    /// only builds this once the value has escaped every frame, so it
    /// carries the value's display form rather than the value — a
    /// `RuntimeError` must be `Send + Sync` for miette.
    #[error("uncaught exception: {value}")]
    #[diagnostic(help("wrap the throwing code in `try ... catch e: <Type> ... end` to handle it"))]
    Thrown {
        /// Display form of the thrown value.
        value: String,
        #[label("uncaught `throw`")]
        span: std::ops::Range<usize>,
    },

    /// Calls nested deeper than the limit — the VM's frame cap, or
    /// [`crate::call::MAX_CALL_DEPTH`] for calls that re-enter the VM from
    /// a native.
    ///
    /// Without a limit a deeply recursive program exhausts memory, or — for
    /// re-entrant calls, which each take a native stack frame — the native
    /// stack, and the process dies with no span, no message, no chance for a
    /// `catch` to run; in the language server it takes the whole editor
    /// session down. The limit turns that into an ordinary error.
    #[error("stack overflow: evaluation nested more than {limit} levels deep")]
    #[diagnostic(help(
        "this is usually unbounded recursion — check that the recursive call has a base case"
    ))]
    StackOverflow {
        limit: u32,
        #[label("while evaluating this")]
        span: std::ops::Range<usize>,
    },

    /// Nesting ran out of *stack* before it ran out of levels.
    ///
    /// A call that re-enters the engine — a native invoking a callback that
    /// calls back into it — takes native stack per level, and how much
    /// depends on the build: a debug build's frames are several times a
    /// release build's. So the level count ([`crate::call::MAX_CALL_DEPTH`])
    /// cannot be the only limit, and a thread that reported its stack
    /// (`crate::call::set_stack_budget`) gets this instead of a `SIGSEGV`
    /// no `catch` can see.
    #[error("stack overflow: nested calls ran out of stack")]
    #[diagnostic(help(
        "this is usually unbounded recursion through a callback — check that it has a base case"
    ))]
    StackExhausted {
        #[label("while evaluating this")]
        span: std::ops::Range<usize>,
    },
}

impl RuntimeError {
    /// A [`RuntimeError::Fault`] with no help yet; add one with
    /// [`with_help`](Self::with_help).
    pub fn fault(
        message: impl Into<String>,
        label: impl Into<String>,
        span: std::ops::Range<usize>,
    ) -> RuntimeError {
        RuntimeError::Fault {
            message: message.into(),
            span,
            label: label.into(),
            help: None,
        }
    }

    /// Attach the line that says what to do about it. A no-op on any variant
    /// but [`RuntimeError::Fault`], whose others carry fixed advice.
    pub fn with_help(mut self, text: impl Into<String>) -> RuntimeError {
        if let RuntimeError::Fault { help, .. } = &mut self {
            *help = Some(text.into());
        }
        self
    }

    /// Replace what the caret says. A no-op on any variant but
    /// [`RuntimeError::Fault`].
    pub fn relabel(mut self, text: impl Into<String>) -> RuntimeError {
        if let RuntimeError::Fault { label, .. } = &mut self {
            *label = text.into();
        }
        self
    }

    /// What a built-in function's `Err(message)` becomes.
    ///
    /// Built-ins can only fail with a `String`, and there are hundreds of
    /// them, each written as "what went wrong — what to do". So the one rule
    /// lives here: the part before the first ` — ` is the message, the part
    /// after it the help, and the caret names the function that refused —
    /// which, from the call site alone, is often not obvious in a chain.
    pub fn native(
        message: String,
        function: Option<&str>,
        span: std::ops::Range<usize>,
    ) -> RuntimeError {
        let (title, help) = match message.split_once(" — ") {
            Some((title, help)) => (title.to_string(), Some(help.to_string())),
            None => (message, None),
        };
        // "Table.insert: position 5 out of range" — the label below already
        // names the function, so the title needn't.
        let title = match function.and_then(|f| title.strip_prefix(f)) {
            Some(rest) if rest.starts_with(": ") => rest[2..].to_string(),
            _ => title,
        };
        let label = match function {
            Some("assert") => "this assertion failed".to_string(),
            Some(name) if !name.is_empty() => format!("`{name}` failed here"),
            _ => "failed here".to_string(),
        };
        // The signature answers the question most of these leave open — what
        // goes where — so it closes the help. `assert`'s is noise next to the
        // program's own message.
        let signature = function
            .filter(|f| *f != "assert")
            .and_then(documented_signature);
        let help = match (help, signature) {
            (Some(h), Some(s)) => Some(format!("{h}\nsignature: `{s}`")),
            (None, Some(s)) => Some(format!("signature: `{s}`")),
            (h, None) => h,
        };
        RuntimeError::Fault {
            message: title,
            span,
            label,
            help,
        }
    }

    /// [`native`](Self::native), unless the failure was `error()` raising on
    /// purpose, which becomes [`RuntimeError::Raised`] at its own call.
    ///
    /// For the callers that have no stack to blame a level against — a
    /// built-in calling another one back. Taking the level here, rather than
    /// leaving it, is what stops it from attaching to some later failure.
    pub fn from_native_call(
        message: String,
        function: &str,
        span: std::ops::Range<usize>,
    ) -> RuntimeError {
        if crate::stdlib::core::take_raised_level().is_some() {
            let origin = span.start;
            return RuntimeError::Raised {
                message,
                span,
                label: "raised here".to_string(),
                origin,
            };
        }
        RuntimeError::native(message, Some(function), span)
    }

    /// "this value cannot be called".
    ///
    /// Written once because it is reached from two unrelated places — the
    /// runtime's [`crate::call::call_value`], and the VM's `CALL` on a
    /// register holding something that is not a function — and a program
    /// failing here has to fail the same way whichever reached it.
    pub fn not_callable(type_name: &str, span: std::ops::Range<usize>) -> RuntimeError {
        RuntimeError::fault(
            format!("cannot call {}", a_type(type_name)),
            format!("this is {}, not a function", a_type(type_name)),
            span,
        )
        .with_help(
            "only functions and classes can be called; check that this variable holds the function you expect",
        )
    }

    /// The message alone, for a caller that has nowhere to put a label or a
    /// help — a native package receiving the failure of a callback it made.
    pub fn message_text(&self) -> String {
        match self {
            RuntimeError::Fault { message, .. }
            | RuntimeError::Raised { message, .. }
            | RuntimeError::Internal { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }
}

/// Convenience constructor — most non-`Local`/`Expr` statements still funnel
/// through this until the matching phase lands.
pub fn unsupported(thing: &'static str, span: std::ops::Range<usize>) -> RuntimeError {
    RuntimeError::Unsupported { thing, span }
}

/// Diagnostic wrapper that carries an imported module's `NamedSource` so
/// miette renders the offending snippet from *that* file. Used as the
/// `#[diagnostic_source]` of [`RuntimeError::ImportFailed`].
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
pub struct ImportedDiagnostic {
    /// The inner error's `Display` text (e.g. the parser/typecheck/runtime
    /// message). Shown as the primary error line.
    pub message: String,
    /// Source file the error happened in. Drives the snippet miette
    /// underlines.
    #[source_code]
    pub src: miette::NamedSource<String>,
    /// Primary offending span inside `src`.
    #[label("{label}")]
    pub span: miette::SourceSpan,
    /// Label text rendered under the span (often "here").
    pub label: String,
}

impl ImportedDiagnostic {
    /// Build an `ImportedDiagnostic` from any `miette::Diagnostic` plus the
    /// source it came from. Pulls the first label off the inner diagnostic
    /// if any, otherwise falls back to a zero-length span at the start.
    pub fn from_inner(
        inner: &dyn miette::Diagnostic,
        file_label: String,
        source_text: String,
    ) -> Self {
        let (span, label) = inner
            .labels()
            .and_then(|mut it| it.next())
            .map(|l| {
                let s: miette::SourceSpan = (l.offset(), l.len()).into();
                (s, l.label().unwrap_or("here").to_string())
            })
            .unwrap_or_else(|| ((0usize, 0usize).into(), "here".to_string()));

        // Compose the message: prefer the inner's `Display`; append the
        // help text if any (so e.g. typecheck hints survive the relay).
        let mut message = inner.to_string();
        if let Some(help) = inner.help() {
            let help_str = help.to_string();
            if !help_str.is_empty() {
                message.push_str("\n  help: ");
                message.push_str(&help_str);
            }
        }

        Self {
            message,
            src: miette::NamedSource::new(file_label, source_text),
            span,
            label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic;

    fn parts(e: &RuntimeError) -> (String, String, Option<String>) {
        let label = e
            .labels()
            .and_then(|mut l| l.next())
            .and_then(|l| l.label().map(str::to_string))
            .unwrap_or_default();
        (e.to_string(), label, e.help().map(|h| h.to_string()))
    }

    #[test]
    fn a_type_picks_the_article() {
        assert_eq!(a_type("string"), "a `string`");
        assert_eq!(a_type("integer"), "an `integer`");
        assert_eq!(a_type("Item"), "an `Item`");
        assert_eq!(a_type("Box"), "a `Box`");
        assert_eq!(a_type("nil"), "`nil`");
    }

    #[test]
    fn a_natives_advice_moves_from_the_message_to_the_help() {
        let e = RuntimeError::native("bad thing — do this instead".into(), Some("Foo.bar"), 0..1);
        assert_eq!(
            parts(&e),
            (
                "bad thing".into(),
                "`Foo.bar` failed here".into(),
                Some("do this instead".into())
            )
        );
    }

    #[test]
    fn only_the_first_dash_splits() {
        let e = RuntimeError::native("a — b — c".into(), None, 0..1);
        assert_eq!(
            parts(&e),
            ("a".into(), "failed here".into(), Some("b — c".into()))
        );
    }

    #[test]
    fn a_message_without_advice_has_no_help() {
        let e = RuntimeError::native("just this".into(), Some("f"), 0..1);
        assert_eq!(parts(&e).2, None);
    }

    #[test]
    fn the_functions_own_name_is_stripped_only_as_a_prefix_with_a_colon() {
        let e = RuntimeError::native(
            "Table.insert: out of range".into(),
            Some("Table.insert"),
            0..1,
        );
        assert_eq!(parts(&e).0, "out of range");
        // No colon: it is part of the sentence, so it stays.
        let e = RuntimeError::native("String.gsub limit is bad".into(), Some("String.gsub"), 0..1);
        assert_eq!(parts(&e).0, "String.gsub limit is bad");
        // Another function's name is not this one's.
        let e = RuntimeError::native("Table.insert: x".into(), Some("Table.remove"), 0..1);
        assert_eq!(parts(&e).0, "Table.insert: x");
    }

    #[test]
    fn a_documented_built_in_gets_its_signature_after_the_advice() {
        let e = RuntimeError::native("bad — fix it".into(), Some("Math.sqrt"), 0..1);
        assert_eq!(
            parts(&e).2.as_deref(),
            Some("fix it\nsignature: `Math.sqrt<N>(n: N) -> float`")
        );
        // With no advice of its own, the signature is the whole help.
        let e = RuntimeError::native("bad".into(), Some("Math.sqrt"), 0..1);
        assert_eq!(
            parts(&e).2.as_deref(),
            Some("signature: `Math.sqrt<N>(n: N) -> float`")
        );
    }

    #[test]
    fn an_undocumented_name_and_assert_get_no_signature() {
        let e = RuntimeError::native("bad".into(), Some("Nope.nothing"), 0..1);
        assert_eq!(parts(&e).2, None);
        let e = RuntimeError::native("assertion failed: x".into(), Some("assert"), 0..1);
        assert_eq!(parts(&e).2, None);
        // A constant is documented too, but has no signature to call by.
        let e = RuntimeError::native("bad".into(), Some("Math.pi"), 0..1);
        assert_eq!(parts(&e).2, None);
    }

    #[test]
    fn assert_gets_its_own_label() {
        let e = RuntimeError::native("assertion failed: x".into(), Some("assert"), 0..1);
        assert_eq!(parts(&e).1, "this assertion failed");
    }

    #[test]
    fn an_internal_error_says_it_is_a_bug_and_drops_the_prefix() {
        let e = RuntimeError::Internal {
            message: "internal: no enum 3".into(),
            span: 0..1,
        };
        let (message, _, help) = parts(&e);
        assert_eq!(message, "internal error: no enum 3");
        assert!(help.is_some_and(|h| h.contains("bug in Saule")));
    }

    #[test]
    fn help_and_relabel_touch_only_a_fault() {
        let e = RuntimeError::DivisionByZero { span: 0..1 }
            .with_help("ignored")
            .relabel("ignored");
        assert_eq!(parts(&e).1, "the divisor here is 0");
        assert_ne!(parts(&e).2.as_deref(), Some("ignored"));
    }

    #[test]
    fn message_text_is_the_bare_message() {
        let e = RuntimeError::fault("the message", "a label", 0..1).with_help("a help");
        assert_eq!(e.message_text(), "the message");
    }
}
