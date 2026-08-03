//! `when(source):stage():stage()` pipeline expressions.

use crate::Parser;
use crate::error::ParseError;
use saule_ast::{Expr, PipeStage, Spanned};
use saule_lexer::Token;

impl Parser {
    pub(crate) fn parse_when_pipe(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let kw = self.advance(); // `when`
        self.expect(&Token::LParen, "`(` after `when` to wrap the source value")?;
        let source = self.parse_expression()?;
        let close = self.expect(&Token::RParen, "`)` to close `when(...)`")?;

        let mut stages: Vec<PipeStage> = Vec::new();
        while self.check(&Token::Colon) {
            stages.push(self.parse_pipe_stage()?);
        }
        if stages.is_empty() {
            return Err(ParseError::Expected {
                expected: "`:name(args)` after `when(...)` — a pipeline needs at least one stage",
                span: self.peek().span.clone(),
            });
        }

        let span_end = stages.last().map(|s| s.span.end).unwrap_or(close.span.end);
        let span = kw.span.start..span_end;
        Ok(Spanned::new(
            Expr::Pipe {
                source: Box::new(source),
                stages,
            },
            span,
        ))
    }

    /// Parses one `:name(args)` stage of a `when` pipeline. The leading
    /// colon must be the current token; callers (only [`parse_when_pipe`])
    /// peek for it.
    pub(crate) fn parse_pipe_stage(&mut self) -> Result<PipeStage, ParseError> {
        let colon = self.expect(&Token::Colon, "`:` to begin a pipeline stage")?;
        let (name, _) = self.expect_ident("function name after `:` in pipeline")?;
        let (args, close_span) = self.parse_call_args()?;
        Ok(PipeStage {
            name,
            args,
            span: colon.span.start..close_span.end,
        })
    }
}
