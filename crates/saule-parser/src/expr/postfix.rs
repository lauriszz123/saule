//! Postfix chains: calls, indexing, member access and method
//! calls, plus the argument lists they carry.

use crate::Parser;
use crate::error::ParseError;
use saule_ast::{CallArg, Expr, Spanned};
use saule_lexer::Token;
use std::ops::Range;

impl Parser {
    pub(crate) fn postfix_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut expr = self.primary_expr()?;
        loop {
            let tok = self.peek().value.clone();
            match tok {
                Token::Dot => {
                    self.advance();
                    // `super` is a keyword everywhere else, but after `.` we
                    // treat it as just a member name so `self.super(args)`
                    // can dispatch to the parent constructor.
                    let (name, name_span) = if matches!(self.peek().value, Token::Super) {
                        let t = self.advance();
                        ("super".to_string(), t.span)
                    } else {
                        self.expect_ident_recover("field name after `.`")?
                    };
                    let span = expr.span.start..name_span.end;
                    expr = Spanned::new(
                        Expr::Member {
                            obj: Box::new(expr),
                            name,
                        },
                        span,
                    );
                }
                Token::QuestionDot => {
                    self.advance();
                    let (name, name_span) = self.expect_ident_recover("field name after `?.`")?;
                    let span = expr.span.start..name_span.end;
                    expr = Spanned::new(
                        Expr::SafeMember {
                            obj: Box::new(expr),
                            name,
                        },
                        span,
                    );
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.with_trailing_block(|p| p.parse_expression())?;
                    let close = self.expect_close(&Token::RBracket, "`]` after index")?;
                    let span = expr.span.start..close.max(expr.span.end);
                    expr = Spanned::new(
                        Expr::Index {
                            obj: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                Token::LParen => {
                    let (args, close_span) = self.parse_call_args()?;
                    let span = expr.span.start..close_span.end;
                    expr = Spanned::new(
                        Expr::Call {
                            callee: Box::new(expr),
                            args,
                            type_args: None,
                        },
                        span,
                    );
                }
                // `name<T, U>(args)` — generic instantiation. The type args
                // are kept: the typechecker binds the callee's parameters to
                // them, so `filter<string>(nums)` on a `table<integer>` is a
                // mismatch rather than a silently re-inferred `T`.
                Token::Lt => {
                    // `f<>(…)` — the brackets were written and the types were
                    // not. Claimed here, before the probe below: the probe can
                    // only *fail* on an empty list, which hands the `<` to the
                    // comparison rung and turns one omission into "expected an
                    // expression" pointing at the `>`.
                    //
                    // `f<>(x)` and `f <> (x)` are the same three tokens, and
                    // the second is far more likely to be the SQL not-equal
                    // than a generic call — nobody puts a space between a
                    // callee and its type arguments. So the claim is made
                    // only on the tight spelling, and the spaced one falls
                    // through to [`Parser::comparison_expr`], which reports
                    // it as the `!=` it probably is. Either way the reader
                    // gets an error on this token; this decides which
                    // sentence it is.
                    if self.at_empty_angles()
                        && expr.span.end == self.peek().span.start
                        && matches!(self.peek_at(2).value, Token::LParen)
                    {
                        self.report_empty_angles(|span| ParseError::EmptyTypeArgs { span })?;
                        continue;
                    }
                    // Not an instantiation after all (`a < b`) — leave the `<`
                    // for the binary-operator level to claim.
                    let Some(type_args) = self.try_eat_generic_call_args() else {
                        break;
                    };
                    let type_args = Some(type_args);
                    let (args, close_span) = self.parse_call_args()?;
                    let span = expr.span.start..close_span.end;
                    expr = Spanned::new(
                        Expr::Call {
                            callee: Box::new(expr),
                            args,
                            type_args,
                        },
                        span,
                    );
                }
                // `f(args) do (p) … end` — trailing block. Sugar for passing a
                // lambda as the final positional argument, so `View(spacing: 10)
                // do … end` is exactly `View(spacing: 10, fn() … end)`.
                //
                // Requiring the receiver to already be a `Call` keeps the form
                // anchored to an argument list: a bare `while x do` can never be
                // mistaken for one, and `View do … end` (no parens) is a clear
                // error rather than a silently different parse.
                Token::Do if !self.no_trailing_block && matches!(expr.value, Expr::Call { .. }) => {
                    let block = self.parse_trailing_block()?;
                    let span = expr.span.start..block.span.end;
                    let Expr::Call {
                        callee,
                        mut args,
                        type_args,
                    } = expr.value
                    else {
                        unreachable!("guarded by the match arm above")
                    };
                    args.push(CallArg::Positional(block));
                    expr = Spanned::new(
                        Expr::Call {
                            callee,
                            args,
                            type_args,
                        },
                        span,
                    );
                }
                Token::Bang => {
                    let bang = self.advance();
                    let span = expr.span.start..bang.span.end;
                    expr = Spanned::new(Expr::ForceUnwrap(Box::new(expr)), span);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// Parses `(arg1, arg2, ...)`; the opening `(` must be the next token.
    pub(crate) fn parse_call_args(&mut self) -> Result<(Vec<CallArg>, Range<usize>), ParseError> {
        self.expect(&Token::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.check(&Token::RParen) {
            args.push(self.parse_call_arg()?);
            while self.eat(&Token::Comma) {
                args.push(self.parse_call_arg()?);
            }
        }
        let close_start = self.peek().span.start;
        let close_end = self.expect_close(&Token::RParen, "`)` to close arguments")?;
        // The span the caller uses for the whole call. When the `)` was never
        // written it collapses to a point at the cursor, which is exactly
        // where signature help wants to believe the argument list still is.
        Ok((args, close_start.min(close_end)..close_end))
    }

    pub(crate) fn parse_call_arg(&mut self) -> Result<CallArg, ParseError> {
        if let Token::Identifier(name) = self.peek().value.clone()
            && matches!(self.peek_at(1).value, Token::Colon)
        {
            self.advance();
            self.advance();
            let value = self.with_trailing_block(|p| p.parse_expression())?;
            return Ok(CallArg::Named { name, value });
        }
        Ok(CallArg::Positional(
            self.with_trailing_block(|p| p.parse_expression())?,
        ))
    }

    // ── Primary expressions ─────────────────────────────────────────────────
}
