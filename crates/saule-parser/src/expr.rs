//! Expression and parameter-list parsing: precedence ladder, postfix chains,
//! primary expressions (including match, lambdas, table literals), and
//! function-parameter declarations.

use std::ops::Range;

use saule_ast::{
    BinOp, CallArg, Expr, LambdaBody, MatchArm, MatchBody, Param, Pattern, Spanned, Stmt,
    TableEntry, Type, UnaryOp,
};
use saule_lexer::Token;

use crate::{Parser, mk_binary};
use crate::error::ParseError;

impl Parser {
    // ─────────────────────────────────────────────────────────────────────────
    // Expressions  (lowest precedence first)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_expression(&mut self) -> Result<Spanned<Expr>, ParseError> {
        self.or_expr()
    }

    pub(crate) fn or_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.and_expr()?;
        while self.check(&Token::Or) {
            self.advance();
            let right = self.and_expr()?;
            left = mk_binary(BinOp::Or, left, right);
        }
        Ok(left)
    }

    pub(crate) fn and_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.equality_expr()?;
        while self.check(&Token::And) {
            self.advance();
            let right = self.equality_expr()?;
            left = mk_binary(BinOp::And, left, right);
        }
        Ok(left)
    }

    pub(crate) fn equality_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.comparison_expr()?;
        loop {
            let op = match self.peek().value {
                Token::EqEq => BinOp::Eq,
                Token::NotEq => BinOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.comparison_expr()?;
            left = mk_binary(op, left, right);
        }
        Ok(left)
    }

    pub(crate) fn comparison_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.coalesce_expr()?;
        loop {
            let op = match self.peek().value {
                Token::Lt => BinOp::Lt,
                Token::LtEq => BinOp::LtEq,
                Token::Gt => BinOp::Gt,
                Token::GtEq => BinOp::GtEq,
                _ => break,
            };
            self.advance();
            let right = self.coalesce_expr()?;
            left = mk_binary(op, left, right);
        }
        Ok(left)
    }

    pub(crate) fn coalesce_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.concat_expr()?;
        while self.check(&Token::QuestionQuestion) {
            self.advance();
            // `??` is right-associative; recurse for the right operand.
            let right = self.coalesce_expr()?;
            left = mk_binary(BinOp::Coalesce, left, right);
        }
        Ok(left)
    }

    pub(crate) fn concat_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.additive_expr()?;
        while self.check(&Token::DotDot) {
            self.advance();
            // `..` is right-associative in Lua-likes.
            let right = self.concat_expr()?;
            left = mk_binary(BinOp::Concat, left, right);
        }
        Ok(left)
    }

    pub(crate) fn additive_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.mul_expr()?;
        loop {
            let op = match self.peek().value {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.mul_expr()?;
            left = mk_binary(op, left, right);
        }
        Ok(left)
    }

    pub(crate) fn mul_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.unary_expr()?;
        loop {
            let op = match self.peek().value {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.unary_expr()?;
            left = mk_binary(op, left, right);
        }
        Ok(left)
    }

    pub(crate) fn unary_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let tok = self.peek().clone();
        let op = match tok.value {
            Token::Minus => Some(UnaryOp::Neg),
            Token::Not => Some(UnaryOp::Not),
            Token::Hash => Some(UnaryOp::Len),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let rhs = self.unary_expr()?;
            let span = tok.span.start..rhs.span.end;
            return Ok(Spanned::new(
                Expr::Unary {
                    op,
                    rhs: Box::new(rhs),
                },
                span,
            ));
        }
        self.postfix_expr()
    }

    // ── Postfix layer: chains of  `.x`, `?.x`, `[i]`, `(args)`, `:m(args)`, `!`

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
                        self.expect_ident("field name after `.`")?
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
                    let (name, name_span) = self.expect_ident("field name after `?.`")?;
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
                    let index = self.parse_expression()?;
                    let close = self.expect(&Token::RBracket, "`]` after index")?;
                    let span = expr.span.start..close.span.end;
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
                        },
                        span,
                    );
                }
                Token::Lt if self.try_eat_generic_call_args() => {
                    // `name<T, U>(args)` — generic instantiation. Type args
                    // are erased at parse time; the typechecker is generic-
                    // parameter aware so it doesn't penalize the call.
                    let (args, close_span) = self.parse_call_args()?;
                    let span = expr.span.start..close_span.end;
                    expr = Spanned::new(
                        Expr::Call {
                            callee: Box::new(expr),
                            args,
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
        let close = self.expect(&Token::RParen, "`)` to close arguments")?;
        Ok((args, close.span))
    }

    pub(crate) fn parse_call_arg(&mut self) -> Result<CallArg, ParseError> {
        if let Token::Identifier(name) = self.peek().value.clone()
            && matches!(self.peek_at(1).value, Token::Colon)
        {
            self.advance();
            self.advance();
            let value = self.parse_expression()?;
            return Ok(CallArg::Named { name, value });
        }
        Ok(CallArg::Positional(self.parse_expression()?))
    }

    // ── Primary expressions ─────────────────────────────────────────────────

    pub(crate) fn primary_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let tok = self.peek().clone();
        match tok.value {
            Token::Int(n) => {
                self.advance();
                Ok(Spanned::new(Expr::Int(n), tok.span))
            }
            Token::Float(f) => {
                self.advance();
                Ok(Spanned::new(Expr::Float(f), tok.span))
            }
            Token::True => {
                self.advance();
                Ok(Spanned::new(Expr::Bool(true), tok.span))
            }
            Token::False => {
                self.advance();
                Ok(Spanned::new(Expr::Bool(false), tok.span))
            }
            Token::Nil => {
                self.advance();
                Ok(Spanned::new(Expr::Nil, tok.span))
            }
            Token::String(s) => {
                self.advance();
                Ok(Spanned::new(Expr::Str(s), tok.span))
            }
            Token::Self_ => {
                self.advance();
                Ok(Spanned::new(Expr::Self_, tok.span))
            }
            Token::Identifier(name) => {
                self.advance();
                if self.eat(&Token::FatArrow) {
                    let body_expr = self.parse_expression()?;
                    let span = tok.span.start..body_expr.span.end;
                    let param = Param {
                        name,
                        ty: Type::Named("any".to_string()),
                        default: None,
                        variadic: false,
                        span: tok.span.clone(),
                    };
                    Ok(Spanned::new(
                        Expr::Lambda {
                            params: vec![param],
                            return_ty: None,
                            body: LambdaBody::Expr(Box::new(body_expr)),
                        },
                        span,
                    ))
                } else {
                    Ok(Spanned::new(Expr::Ident(name), tok.span))
                }
            }
            Token::LBrace => self.parse_table_literal(),
            Token::Fn => self.parse_fn_lambda(),
            Token::Match => self.parse_match_expr(),
            Token::LParen => {
                if self.looks_like_arrow_lambda() {
                    self.parse_arrow_lambda()
                } else {
                    // Parenthesised expression.
                    self.advance();
                    let inner = self.parse_expression()?;
                    self.expect(&Token::RParen, "`)`")?;
                    Ok(inner)
                }
            }
            _ => Err(ParseError::Expected {
                expected: "an expression",
                span: tok.span,
            }),
        }
    }

    pub(crate) fn parse_table_literal(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let open = self.advance(); // consume `{`
        let mut items: Vec<TableEntry> = Vec::new();
        if !self.check(&Token::RBrace) {
            items.push(self.parse_table_entry()?);
            while self.eat(&Token::Comma) {
                if self.check(&Token::RBrace) {
                    break; // allow trailing comma
                }
                items.push(self.parse_table_entry()?);
            }
        }
        let close = self.expect(&Token::RBrace, "`}` to close table literal")?;
        let span = open.span.start..close.span.end;
        Ok(Spanned::new(Expr::Table(items), span))
    }

    /// One `{ ... }` entry. Recognises three shapes:
    ///
    /// * `ident: expr`  — sugar for `"ident": expr`
    /// * `"str": expr`  — explicit string key
    /// * `expr`         — positional (appended to the array part)
    ///
    /// Only the literal-key forms are treated as field entries, so a
    /// bare-identifier expression like `foo` (looking up a binding) still
    /// works as a positional entry — the lookahead requires the `:` to
    /// follow immediately.
    fn parse_table_entry(&mut self) -> Result<TableEntry, ParseError> {
        // `ident :` field
        if let Token::Identifier(name) = self.peek().value.clone()
            && matches!(self.peek_at(1).value, Token::Colon)
        {
            let name_tok = self.advance();
            self.advance(); // `:`
            let key_span = name_tok.span.clone();
            let key = Spanned::new(Expr::Str(name), key_span);
            let value = self.parse_expression()?;
            return Ok(TableEntry::Field { key, value });
        }
        // `"str" :` field
        if let Token::String(s) = self.peek().value.clone()
            && matches!(self.peek_at(1).value, Token::Colon)
        {
            let str_tok = self.advance();
            self.advance(); // `:`
            let key = Spanned::new(Expr::Str(s), str_tok.span);
            let value = self.parse_expression()?;
            return Ok(TableEntry::Field { key, value });
        }
        // positional
        Ok(TableEntry::Positional(self.parse_expression()?))
    }

    // ── `match` expression ──────────────────────────────────────────────────
    //
    // Surface (per README):
    //
    //   match <scrutinee>
    //       case <pattern> [when <guard>] then <body>
    //       ...
    //   end
    //
    // Body is parsed as a single expression — multi-statement arms aren't
    // supported in the v1 surface and would clash with the `case`/`end`
    // boundaries. The whole match is itself an expression.

    pub(crate) fn parse_match_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let kw = self.advance(); // `match`
        let scrutinee = self.parse_expression()?;
        let mut arms = Vec::new();
        while self.check(&Token::Case) {
            arms.push(self.parse_match_arm()?);
        }
        let end = self.expect(&Token::End, "`end` to close `match`")?;
        let span = kw.span.start..end.span.end;
        Ok(Spanned::new(
            Expr::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span,
        ))
    }

    pub(crate) fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let case_tok = self.advance(); // `case`
        let pattern = self.parse_pattern()?;
        let guard = if self.eat(&Token::When) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect(&Token::Then, "`then` to start arm body")?;

        // The body runs until the next `case` or the closing `end`.
        // Single-expression arms like `case x then 42` parse as one
        // `Stmt::Expr(42)` and collapse to `MatchBody::Expr` so the
        // expression's value is the arm's value. Multi-statement arms
        // (`case x then dp = dp + 1` followed by more statements, or
        // a `do ... end` block) stay as `MatchBody::Block`.
        let stmts = self.parse_block_until(&[Token::Case, Token::End])?;

        let (body, end_pos) = match (stmts.len(), stmts.first()) {
            (1, Some(Spanned { value: Stmt::Expr(e), span })) => {
                let end = span.end;
                (MatchBody::Expr(e.clone()), end)
            }
            (0, _) => {
                return Err(ParseError::Expected {
                    expected: "an expression or statement after `then`",
                    span: self.peek().span.clone(),
                });
            }
            _ => {
                let end = stmts.last().map(|s| s.span.end).unwrap_or(case_tok.span.end);
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
                    _ => Err(ParseError::Expected {
                        expected: "a numeric literal after `-`",
                        span: next.span,
                    }),
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
                let close = self.expect(&Token::RParen, "`)` to close tuple pattern")?;
                let span = tok.span.start..close.span.end;
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
                    let (variant, vspan) = self.expect_ident("variant name after `.`")?;
                    let mut fields = Vec::new();
                    let mut end = vspan.end;
                    if self.eat(&Token::LParen) {
                        if !self.check(&Token::RParen) {
                            fields.push(self.parse_pattern()?);
                            while self.eat(&Token::Comma) {
                                fields.push(self.parse_pattern()?);
                            }
                        }
                        let close = self
                            .expect(&Token::RParen, "`)` to close variant pattern payload")?;
                        end = close.span.end;
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
            _ => Err(ParseError::Expected {
                expected: "a pattern (literal, identifier, `_`, `(...)`, or `Enum.Variant`)",
                span: tok.span,
            }),
        }
    }

    // ── Lambdas ─────────────────────────────────────────────────────────────

     /// `fn(params): T ... end` or `fn => (params) ... end` as an expression.
     fn parse_fn_lambda(&mut self) -> Result<Spanned<Expr>, ParseError> {
         let fn_tok = self.advance(); // consume `fn`

         // Check for `fn => (params)` syntax
         if self.eat(&Token::FatArrow) {
             let params = self.parse_param_list()?;
             let return_ty = self.parse_return_type_opt()?;
             let body = self.parse_block_until(&[Token::End])?;
             let end = self.expect(&Token::End, "`end` to close `fn =>` lambda")?;
             let span = fn_tok.span.start..end.span.end;
             Ok(Spanned::new(
                 Expr::Lambda {
                     params,
                     return_ty,
                     body: LambdaBody::Block(body),
                 },
                 span,
             ))
         } else {
             // Standard `fn(params)` syntax
             let params = self.parse_param_list()?;
             let return_ty = self.parse_return_type_opt()?;
             let body = self.parse_block_until(&[Token::End])?;
             let end = self.expect(&Token::End, "`end` to close `fn` lambda")?;
             let span = fn_tok.span.start..end.span.end;
             Ok(Spanned::new(
                 Expr::Lambda {
                     params,
                     return_ty,
                     body: LambdaBody::Block(body),
                 },
                 span,
             ))
         }
     }

    /// Heuristic: peeks past a balanced `(...)` and checks for `=>` to decide
    /// whether `(` starts an arrow lambda or a parenthesised expression.
    pub(crate) fn looks_like_arrow_lambda(&self) -> bool {
        // Walk tokens until we balance the opening paren.
        let mut depth = 0i32;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match self.tokens[i].value {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        // Look at the token after the closing `)`.
                        return matches!(
                            self.tokens.get(i + 1).map(|t| &t.value),
                            Some(Token::FatArrow)
                        );
                    }
                }
                Token::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    pub(crate) fn parse_arrow_lambda(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let open = self.expect(&Token::LParen, "`(`")?;
        let params = self.parse_param_list_inner()?;
        self.expect(&Token::RParen, "`)` after lambda parameters")?;
        let return_ty = self.parse_return_type_opt()?;
        self.expect(&Token::FatArrow, "`=>` in lambda")?;
        let body_expr = self.parse_expression()?;
        let span = open.span.start..body_expr.span.end;
        Ok(Spanned::new(
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Expr(Box::new(body_expr)),
            },
            span,
        ))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Parameter lists
    // ─────────────────────────────────────────────────────────────────────────

    /// Parses `( ... )` and returns the parameter list.
    pub(crate) fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(&Token::LParen, "`(` to begin parameter list")?;
        let params = self.parse_param_list_inner()?;
        self.expect(&Token::RParen, "`)` to close parameter list")?;
        Ok(params)
    }

    /// Parses the contents of a parameter list; stops at `)`.
    pub(crate) fn parse_param_list_inner(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.check(&Token::RParen) {
            return Ok(params);
        }
        params.push(self.parse_param()?);
        while self.eat(&Token::Comma) {
            params.push(self.parse_param()?);
        }
        Ok(params)
    }

    pub(crate) fn parse_param(&mut self) -> Result<Param, ParseError> {
        let start = self.peek().span.start;
        let variadic = self.eat(&Token::Ellipsis);
        // `self` is a keyword token but is also a legal parameter name in
        // method signatures (`fn greet(self): nil`).
        let name = if self.check(&Token::Self_) {
            self.advance();
            "self".to_string()
        } else {
            self.expect_ident("parameter name")?.0
        };
        // Implicit `self` parameter in method signatures: typed as the
        // enclosing class. We don't know that here, so use a placeholder.
        let ty = if self.eat(&Token::Colon) {
            self.parse_type()?
        } else if name == "self" {
            Type::Named("self".to_string())
        } else {
            return Err(ParseError::Expected {
                expected: "`:` and parameter type",
                span: self.peek().span.clone(),
            });
        };
        let default = if self.eat(&Token::Assign) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        let end = self.last_consumed_end();
        Ok(Param {
            name,
            ty,
            default,
            variadic,
            span: start..end,
        })
    }

}
