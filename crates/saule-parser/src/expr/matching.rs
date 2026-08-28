//! `match` expressions and the pattern grammar their arms use.

use crate::Parser;
use crate::error::ParseError;
use saule_ast::{Expr, MatchArm, MatchBody, Pattern, Spanned, Stmt};
use saule_lexer::Token;

impl Parser {
    pub(crate) fn parse_match_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let kw = self.advance(); // `match`
        let scrutinee = self.parse_expression()?;
        let mut arms = Vec::new();
        loop {
            if self.check(&Token::Case) {
                arms.push(self.parse_match_arm()?);
                continue;
            }
            if self.check(&Token::End) {
                break;
            }
            // Anything else here is almost certainly a forgotten `case`
            // — e.g. `_ then return` or `"foo" then ...` written without
            // the leading keyword. A bare "expected `end`" message in
            // that situation is confusing; point at the arm-start
            // explicitly.
            let err = self.expected_here("`case` to start a match arm, or `end` to close `match`");
            if !self.recovering() {
                return Err(err);
            }
            // Stop rather than skip: the arms parsed so far are the useful
            // part, and the enclosing statement loop resynchronises on what
            // is left.
            self.record(err);
            break;
        }
        let end = self.expect_close(&Token::End, "`end` to close `match`")?;
        let span = kw.span.start..end.max(kw.span.end);
        Ok(Spanned::new(
            Expr::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span,
        ))
    }

    // ── `when(...)` pipeline ────────────────────────────────────────────────
    //
    // Surface (per README → "Pipelines"):
    //
    //     when(source):stage1(args):stage2(args)…
    //
    // `when(x)` wraps a value; every subsequent `:name(args)` invokes
    // `name(prev, args)` and feeds the result into the next stage. The
    // chain *requires at least one* `:stage()` — a bare `when(x)` with
    // nothing piped is a parse error so users don't accidentally call
    // `when` like a regular function (the `when` keyword is otherwise
    // reserved as a match-arm guard).

    pub(crate) fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let case_tok = self.advance(); // `case`
        let pattern = self.parse_pattern()?;
        let guard = if self.eat(&Token::When) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect_recover(&Token::Then, "`then` to start arm body")?;

        // The body runs until the next `case` or the closing `end`.
        // Single-expression arms like `case x then 42` parse as one
        // `Stmt::Expr(42)` and collapse to `MatchBody::Expr` so the
        // expression's value is the arm's value. Multi-statement arms
        // (`case x then dp = dp + 1` followed by more statements, or
        // a `do ... end` block) stay as `MatchBody::Block`.
        let stmts = self.parse_block_until(&[Token::Case, Token::End])?;

        let (body, end_pos) = match (stmts.len(), stmts.first()) {
            (
                1,
                Some(Spanned {
                    value: Stmt::Expr(e),
                    span,
                    ..
                }),
            ) => {
                let end = span.end;
                (MatchBody::Expr(e.clone()), end)
            }
            (0, _) => {
                let err = self.expected_here("an expression or statement after `then`");
                if !self.recovering() {
                    return Err(err);
                }
                // `case Foo.Bar then ` with the body not yet typed. Keeping
                // the arm keeps its *pattern*, which is what completion in
                // the body position needs to know the scrutinee's shape.
                self.record(err);
                let at = self.last_consumed_end();
                (MatchBody::Expr(Spanned::new(Expr::Error, at..at)), at)
            }
            _ => {
                let end = stmts
                    .last()
                    .map(|s| s.span.end)
                    .unwrap_or(case_tok.span.end);
                (MatchBody::Block(stmts), end)
            }
        };

        Ok(MatchArm {
            pattern,
            guard,
            body,
            span: case_tok.span.start..end_pos,
        })
    }

    pub(crate) fn parse_pattern(&mut self) -> Result<Spanned<Pattern>, ParseError> {
        let tok = self.peek().clone();
        match tok.value {
            Token::Nil => {
                self.advance();
                Ok(Spanned::new(Pattern::Nil, tok.span))
            }
            Token::True => {
                self.advance();
                Ok(Spanned::new(Pattern::Bool(true), tok.span))
            }
            Token::False => {
                self.advance();
                Ok(Spanned::new(Pattern::Bool(false), tok.span))
            }
            Token::Int(n) => {
                self.advance();
                Ok(Spanned::new(Pattern::Int(n), tok.span))
            }
            Token::Float(f) => {
                self.advance();
                Ok(Spanned::new(Pattern::Float(f), tok.span))
            }
            Token::String(s) => {
                self.advance();
                Ok(Spanned::new(Pattern::Str(s), tok.span))
            }
            // `-1` / `-1.5` — negated numeric literal (handy for `case -1 then ...`).
            Token::Minus => {
                self.advance();
                let next = self.peek().clone();
                match next.value {
                    Token::Int(n) => {
                        self.advance();
                        let span = tok.span.start..next.span.end;
                        Ok(Spanned::new(Pattern::Int(-n), span))
                    }
                    Token::Float(f) => {
                        self.advance();
                        let span = tok.span.start..next.span.end;
                        Ok(Spanned::new(Pattern::Float(-f), span))
                    }
                    _ => Err(ParseError::expected("a numeric literal after `-`", &next)),
                }
            }
            // Tuple pattern: `(p1, p2, ...)`
            Token::LParen => {
                self.advance();
                let mut elems = Vec::new();
                if !self.check(&Token::RParen) {
                    elems.push(self.parse_pattern()?);
                    while self.eat(&Token::Comma) {
                        elems.push(self.parse_pattern()?);
                    }
                }
                let close = self.expect_close(&Token::RParen, "`)` to close tuple pattern")?;
                let span = tok.span.start..close.max(tok.span.end);
                Ok(Spanned::new(Pattern::Tuple(elems), span))
            }
            // Identifier: wildcard `_`, binding `name`, or enum-variant
            // `Enum.Variant[(fields)]` form.
            Token::Identifier(name) => {
                self.advance();
                if name == "_" {
                    return Ok(Spanned::new(Pattern::Wildcard, tok.span));
                }
                // `Name.Variant[(p1, p2, ...)]` — qualified variant pattern.
                if self.eat(&Token::Dot) {
                    let (variant, vspan) = self.expect_ident_recover("variant name after `.`")?;
                    let mut fields = Vec::new();
                    let mut end = vspan.end;
                    if self.eat(&Token::LParen) {
                        if !self.check(&Token::RParen) {
                            fields.push(self.parse_pattern()?);
                            while self.eat(&Token::Comma) {
                                fields.push(self.parse_pattern()?);
                            }
                        }
                        end = self
                            .expect_close(&Token::RParen, "`)` to close variant pattern payload")?
                            .max(end);
                    }
                    let span = tok.span.start..end;
                    return Ok(Spanned::new(
                        Pattern::Variant {
                            enum_name: name,
                            variant,
                            fields,
                        },
                        span,
                    ));
                }
                Ok(Spanned::new(Pattern::Bind(name), tok.span))
            }
            _ => {
                let err = ParseError::expected(
                    "a pattern (literal, identifier, `_`, `(...)`, or `Enum.Variant`)",
                    &tok,
                );
                if !self.recovering() {
                    return Err(err);
                }
                // A wildcard is the pattern that assumes least about what the
                // author meant. It reads as exhaustive, which would matter if
                // anything downstream ran exhaustiveness checking on this
                // tree — nothing does: a file with parse errors gets its
                // parse errors and no further analysis.
                self.record(err);
                Ok(Spanned::new(
                    Pattern::Wildcard,
                    tok.span.start..tok.span.start,
                ))
            }
        }
    }

    // ── Lambdas ─────────────────────────────────────────────────────────────
}
