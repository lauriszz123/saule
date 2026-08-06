//! Statement parsing: local, control flow (if/while/repeat/for/try),
//! return, throw, expression-statement-or-assignment, plus block helpers.

use saule_ast::{BinOp, Spanned, Stmt};
use saule_lexer::Token;

use crate::error::ParseError;
use crate::{Parser, stmt_decl};

/// The binary operator behind a compound-assignment token, or `None` if the
/// token isn't one.
///
/// The comparison and logical operators have no compound form: `a <= b` is
/// already a valid expression, so `a <== b` would be ambiguous, and `and=` /
/// `or=` would have to answer whether the RHS is evaluated when the operator
/// short-circuits.
fn compound_assign_op(tok: &Token) -> Option<BinOp> {
    Some(match tok {
        Token::PlusEq => BinOp::Add,
        Token::MinusEq => BinOp::Sub,
        Token::StarEq => BinOp::Mul,
        Token::SlashEq => BinOp::Div,
        Token::PercentEq => BinOp::Mod,
        Token::CaretEq => BinOp::Pow,
        Token::DotDotEq => BinOp::Concat,
        _ => return None,
    })
}

impl Parser {
    // ─────────────────────────────────────────────────────────────────────────
    // Statements
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_statement(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        self.nested(Self::parse_statement_inner)
    }

    /// Bounded by [`Parser::parse_statement`] — every nested block re-enters
    /// through it, so block nesting is counted alongside expression nesting.
    fn parse_statement_inner(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        // Any leading `;` has already been consumed by the caller — see
        // `parse` and `parse_block_until`, which skip separators before
        // testing for the end of the block. This function is only ever
        // entered when a statement really does follow.
        let tok = self.peek().clone();
        match tok.value {
            Token::Local => self.parse_local(),
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::Repeat => self.parse_repeat(),
            Token::For => self.parse_for(),
            Token::Try => self.parse_try(),
            Token::Return => self.parse_return(),
            Token::Throw => self.parse_throw(),
            Token::Break => {
                let t = self.advance();
                Ok(Spanned::new(Stmt::Break, t.span))
            }
            Token::Continue => {
                let t = self.advance();
                Ok(Spanned::new(Stmt::Continue, t.span))
            }
            Token::Fn => self.parse_fn_decl(false, false).map(stmt_decl),
            Token::Class => self.parse_class_decl(false).map(stmt_decl),
            Token::Interface => self.parse_interface_decl(false).map(stmt_decl),
            Token::Enum => self.parse_enum_decl(false).map(stmt_decl),
            Token::Import => self.parse_import().map(stmt_decl),
            Token::Export => self.parse_export(),
            _ => self.parse_expr_or_assign(),
        }
    }

    pub(crate) fn parse_local(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        // `local fn name(...)` is a non-exported function declaration.
        if matches!(self.peek_at(1).value, Token::Fn) {
            self.advance(); // consume `local`
            let decl = self.parse_fn_decl(false, true)?;
            return Ok(stmt_decl(decl));
        }

        let kw = self.advance(); // `local`
        let (first_name, first_span) = self.expect_ident("variable name after `local`")?;
        let (first_ty, first_ty_span) = if self.eat(&Token::Colon) {
            let start = self.peek().span.start;
            let t = self.parse_type()?;
            let end = self.last_consumed_end();
            (Some(t), Some(start..end))
        } else {
            (None, None)
        };

        // `local a: T, b: U = e1, e2`
        if self.check(&Token::Comma) {
            let mut names = vec![(first_name, first_span.clone(), first_ty)];
            while self.eat(&Token::Comma) {
                let (n, n_span) = self.expect_ident("variable name in `local` list")?;
                let t = if self.eat(&Token::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                names.push((n, n_span, t));
            }
            let mut values = Vec::new();
            let mut end = self.last_consumed_end();
            if self.eat(&Token::Assign) {
                values.push(self.parse_expression()?);
                while self.eat(&Token::Comma) {
                    values.push(self.parse_expression()?);
                }
                end = values.last().map(|e| e.span.end).unwrap_or(end);
            }
            return Ok(Spanned::new(
                Stmt::LocalMulti { names, values },
                kw.span.start..end,
            ));
        }

        let (value, end) = if self.eat(&Token::Assign) {
            let e = self.parse_expression()?;
            let end = e.span.end;
            (Some(e), end)
        } else {
            (None, first_span.end)
        };
        Ok(Spanned::new(
            Stmt::Local {
                name: first_name,
                name_span: first_span.clone(),
                ty: first_ty,
                ty_span: first_ty_span,
                value,
            },
            kw.span.start..end,
        ))
    }

    pub(crate) fn parse_if(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `if`
        let cond = self.parse_expression()?;
        self.expect(&Token::Then, "`then` after `if` condition")?;
        let then_block = self.parse_block_until(&[Token::Else, Token::Elseif, Token::End])?;

        let mut elseifs = Vec::new();
        let mut else_block: Option<Vec<Spanned<Stmt>>> = None;

        loop {
            if self.eat(&Token::Elseif) {
                let ec = self.parse_expression()?;
                self.expect(&Token::Then, "`then` after `elseif` condition")?;
                let eb = self.parse_block_until(&[Token::Else, Token::Elseif, Token::End])?;
                elseifs.push((ec, eb));
                continue;
            }
            if self.eat(&Token::Else) {
                else_block = Some(self.parse_block_until(&[Token::End])?);
            }
            break;
        }

        let end = self.expect(&Token::End, "`end` to close `if`")?;
        Ok(Spanned::new(
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            },
            kw.span.start..end.span.end,
        ))
    }

    pub(crate) fn parse_while(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `while`
        let cond = self.without_trailing_block(|p| p.parse_expression())?;
        self.expect(&Token::Do, "`do` after `while` condition")?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close `while`")?;
        Ok(Spanned::new(
            Stmt::While { cond, body },
            kw.span.start..end.span.end,
        ))
    }

    pub(crate) fn parse_repeat(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `repeat`
        let body = self.parse_block_until(&[Token::Until])?;
        self.expect(&Token::Until, "`until` after `repeat` body")?;
        let cond = self.parse_expression()?;
        let end_pos = cond.span.end;
        Ok(Spanned::new(
            Stmt::Repeat { body, cond },
            kw.span.start..end_pos,
        ))
    }

    pub(crate) fn parse_for(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `for`
        let (first_name, _) = self.expect_ident("loop variable name")?;
        let first_ty = if self.eat(&Token::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        // Lua-style numeric for loop: `for i = from, to [, step] do ... end`
        if self.eat(&Token::Assign) {
            let from = self.without_trailing_block(|p| p.parse_expression())?;
            self.expect(&Token::Comma, "`,` after start value in numeric `for`")?;
            let to = self.without_trailing_block(|p| p.parse_expression())?;
            let step = if self.eat(&Token::Comma) {
                Some(self.without_trailing_block(|p| p.parse_expression())?)
            } else {
                None
            };
            self.expect(&Token::Do, "`do` in numeric `for`")?;
            let body = self.parse_block_until(&[Token::End])?;
            let end = self.expect(&Token::End, "`end` to close `for`")?;
            return Ok(Spanned::new(
                Stmt::ForNumeric {
                    var: first_name,
                    var_ty: first_ty,
                    from,
                    to,
                    step,
                    body,
                },
                kw.span.start..end.span.end,
            ));
        }

        // For-in: `for v[, v]* in iter do ... end`
        let mut vars = vec![(first_name, first_ty)];
        while self.eat(&Token::Comma) {
            let (n, _) = self.expect_ident("loop variable name")?;
            let t = if self.eat(&Token::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            vars.push((n, t));
        }
        self.expect(&Token::In, "`in` or `=` in `for`")?;
        let iter = self.without_trailing_block(|p| p.parse_expression())?;
        self.expect(&Token::Do, "`do` in for-in")?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close `for`")?;
        Ok(Spanned::new(
            Stmt::ForIn { vars, iter, body },
            kw.span.start..end.span.end,
        ))
    }

    pub(crate) fn parse_try(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `try`
        let body = self.parse_block_until(&[Token::Catch])?;
        self.expect(&Token::Catch, "`catch` after `try` body")?;
        let (catch_var, _) = self.expect_ident("error binding name in `catch`")?;
        self.expect(&Token::Colon, "`:` and error type in `catch`")?;
        let catch_ty = self.parse_type()?;
        let catch_body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close `try`")?;
        Ok(Spanned::new(
            Stmt::Try {
                body,
                catch_var,
                catch_ty,
                catch_body,
            },
            kw.span.start..end.span.end,
        ))
    }

    pub(crate) fn parse_return(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `return`
        let mut values = Vec::new();
        if !self.at_block_terminator() && !self.is_eof() && !self.check(&Token::Semi) {
            values.push(self.parse_expression()?);
            while self.eat(&Token::Comma) {
                values.push(self.parse_expression()?);
            }
        }
        let end = values.last().map(|e| e.span.end).unwrap_or(kw.span.end);
        Ok(Spanned::new(Stmt::Return(values), kw.span.start..end))
    }

    pub(crate) fn parse_throw(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `throw`
        let e = self.parse_expression()?;
        let end = e.span.end;
        Ok(Spanned::new(Stmt::Throw(e), kw.span.start..end))
    }

    /// Parses an expression and, if followed by `=`, an assignment.
    /// Also handles multi-target assignment `a, b = x, y`.
    pub(crate) fn parse_expr_or_assign(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let expr = self.parse_expression()?;

        if self.check(&Token::Comma) {
            let mut targets = vec![expr];
            while self.eat(&Token::Comma) {
                targets.push(self.parse_expression()?);
            }
            self.expect(&Token::Assign, "`=` after assignment targets")?;
            let mut values = vec![self.parse_expression()?];
            while self.eat(&Token::Comma) {
                values.push(self.parse_expression()?);
            }
            let start = targets.first().unwrap().span.start;
            let end = values.last().unwrap().span.end;
            return Ok(Spanned::new(
                Stmt::AssignMulti { targets, values },
                start..end,
            ));
        }

        // `a op= b`. Checked before plain `=` simply because the tokens are
        // distinct; the RHS is a full expression, so `x += a + b` updates by
        // the whole sum.
        if let Some(op) = compound_assign_op(&self.peek().value) {
            self.advance();
            let value = self.parse_expression()?;
            let span = expr.span.start..value.span.end;
            return Ok(Spanned::new(
                Stmt::CompoundAssign {
                    target: expr,
                    op,
                    value,
                },
                span,
            ));
        }

        if self.eat(&Token::Assign) {
            let value = self.parse_expression()?;
            let span = expr.span.start..value.span.end;
            Ok(Spanned::new(
                Stmt::Assign {
                    target: expr,
                    value,
                },
                span,
            ))
        } else {
            let span = expr.span.clone();
            Ok(Spanned::new(Stmt::Expr(expr), span))
        }
    }

    // ── Block parsing ───────────────────────────────────────────────────────

    /// Parses statements until one of the given terminator keywords is next.
    /// Does NOT consume the terminator.
    ///
    /// Separators are skipped before each terminator test, so a block may end
    /// with one: `local a = 1;` directly before `end` is a block of a single
    /// statement, not a statement followed by an empty one.
    pub(crate) fn parse_block_until(
        &mut self,
        terminators: &[Token],
    ) -> Result<Vec<Spanned<Stmt>>, ParseError> {
        let mut stmts = Vec::new();
        loop {
            self.skip_semicolons();
            if self.is_eof() || terminators.iter().any(|t| self.check(t)) {
                break;
            }
            stmts.push(self.parse_statement()?);
        }
        Ok(stmts)
    }

    pub(crate) fn at_block_terminator(&self) -> bool {
        matches!(
            self.peek().value,
            Token::End | Token::Else | Token::Until | Token::Catch
        )
    }

    // ─────────────────────────────────────────────────────────────────────────
}
