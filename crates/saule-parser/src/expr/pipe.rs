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
        let close = self.expect_close(&Token::RParen, "`)` to close `when(...)`")?;

        let mut stages: Vec<PipeStage> = Vec::new();
        while self.check(&Token::Colon) {
            stages.push(self.parse_pipe_stage()?);
        }
        if stages.is_empty() {
            let err = self.expected_here(
                "`:name(args)` after `when(...)` — a pipeline needs at least one stage",
            );
            if !self.recovering() {
                return Err(err);
            }
            // `when(x)` with the first `:stage()` not yet typed. A stageless
            // pipe is still the right node: it carries the source expression,
            // which is what hover and signature help read.
            self.record(err);
        }

        let span_end = stages.last().map(|s| s.span.end).unwrap_or(close);
        let span = kw.span.start..span_end.max(kw.span.end);
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
        let (name, _) = self.expect_ident_recover("function name after `:` in pipeline")?;
        // A stage is an ordinary call, so it takes the same explicit
        // instantiation: `:filter<integer>(x => x % 2 == 0)`.
        let type_args = self.try_eat_generic_call_args();
        let (args, close_span) = self.parse_call_args()?;
        Ok(PipeStage {
            name,
            args,
            type_args,
            span: colon.span.start..close_span.end.max(colon.span.end),
        })
    }
}
