use std::borrow::Cow;
use std::ops::Range;

use miette::Diagnostic;
use saule_ast::Spanned;
use saule_lexer::Token;
use thiserror::Error;

impl ParseError {
    /// The byte range this error points at.
    ///
    /// Every variant carries exactly one span, so this is total. Callers that
    /// render many errors at once — the language server publishing a whole
    /// file's worth from [`crate::parse_recover`] — want the range directly
    /// rather than through `miette`'s label iterator.
    pub fn span(&self) -> &Range<usize> {
        match self {
            ParseError::Unexpected { span }
            | ParseError::Expected { span, .. }
            | ParseError::EmptyTypeArgs { span }
            | ParseError::EmptyTypeParams { span }
            | ParseError::LtGtNotEqual { span }
            | ParseError::Eof { span }
            | ParseError::TooDeep { span, .. } => span,
        }
    }

    /// The parser's workhorse error, built from the token that stopped it.
    ///
    /// Taking the token rather than a bare span is what makes the `found …`
    /// half of the message possible, and taking it *by reference to the
    /// stream* rather than by span means no rule can report a position and a
    /// description that disagree.
    pub(crate) fn expected(what: impl Into<Cow<'static, str>>, found: &Spanned<Token>) -> Self {
        ParseError::Expected {
            expected: what.into(),
            found: found.value.describe(),
            help: None,
            span: found.span.clone(),
        }
    }

    /// Attach the line that says what to do about it.
    ///
    /// Only [`ParseError::Expected`] carries a caller-supplied help: the
    /// other variants each name one specific mistake, so their advice is
    /// written once on the variant itself.
    pub(crate) fn with_help(mut self, text: impl Into<String>) -> Self {
        if let ParseError::Expected { help, .. } = &mut self {
            *help = Some(text.into());
        }
        self
    }
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token")]
    Unexpected {
        #[label("here")]
        span: Range<usize>,
    },
    /// A rule wanted something specific and got something else.
    ///
    /// Both halves matter and for different reasons. `expected` is the
    /// grammar's side, written by the rule that failed — the more it names
    /// the *construct* ("`)` to close arguments") rather than the token
    /// ("`)`"), the less the reader has to reconstruct. `found` is the
    /// reader's side: the caret already shows which token it is, so what this
    /// adds is its category — a reserved word, the end of the file — which is
    /// usually the actual explanation.
    #[error("expected {expected}, found {found}")]
    Expected {
        expected: Cow<'static, str>,
        found: Cow<'static, str>,
        #[help]
        help: Option<String>,
        #[label("here")]
        span: Range<usize>,
    },
    /// `f<>(…)` — a call site wrote the angle brackets and no types.
    ///
    /// Its own variant rather than "expected a type, found `>`" because that
    /// message describes the parser's position, not the mistake: the brackets
    /// are optional in the first place, so "you left them empty" comes with
    /// the fix already attached.
    #[error("empty type argument list")]
    #[diagnostic(help(
        "name the type between the brackets — `f<integer>(...)`, `table<string>` — or drop `<>` \
         and let it be inferred"
    ))]
    EmptyTypeArgs {
        #[label("nothing between `<` and `>`")]
        span: Range<usize>,
    },
    /// `fn f<>(…)` — a declaration wrote the angle brackets and no names.
    #[error("empty type parameter list")]
    #[diagnostic(help(
        "name a type parameter — `fn map<T>(...)`, `class Box<T>` — or drop `<>` if this is not \
         generic"
    ))]
    EmptyTypeParams {
        #[label("nothing between `<` and `>`")]
        span: Range<usize>,
    },
    /// `a <> b`, the not-equal spelling from SQL, Pascal and BASIC.
    ///
    /// Recognised for the same reason the lexer refuses to read `~=` as
    /// xor-assignment: it is a habit from another language that would
    /// otherwise be reported as two unrelated operators and an expression
    /// that isn't there.
    #[error("`<>` is not an operator")]
    #[diagnostic(help("Saule writes not-equal as `!=`"))]
    LtGtNotEqual {
        #[label("here")]
        span: Range<usize>,
    },
    #[error("unexpected end of input")]
    Eof {
        #[label("end of file")]
        span: Range<usize>,
    },
    /// The grammar nested deeper than [`crate::MAX_NESTING_DEPTH`].
    ///
    /// This is a recursive-descent parser, so nesting in the source becomes
    /// recursion on the native stack. Without a bound, input like 50k stacked
    /// `(` aborts the process instead of reporting an error — and the language
    /// server parses half-typed input constantly, where an abort takes the
    /// editor session with it.
    #[error("expression nests more than {limit} levels deep")]
    #[diagnostic(help(
        "simplify the expression, or split it across intermediate `local` bindings"
    ))]
    TooDeep {
        limit: u32,
        #[label("nesting starts here")]
        span: Range<usize>,
    },
}
