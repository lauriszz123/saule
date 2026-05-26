//! Recursive-descent parser for Saule.
//!
//! The parser consumes a stream of `Spanned<Token>` produced by `saule-lexer`
//! and produces a [`Module`] (a list of [`Spanned<Stmt>`]).
//!
//! Grammar overview (Lua-flavoured, block keywords are `then`/`do`/`end`):
//!
//!   * Expressions follow a standard precedence ladder, lowest at the top:
//!     `or` → `and` → `==`/`!=` → `<`/`<=`/`>`/`>=` → `??` → `..` → `+`/`-`
//!     → `*`/`/`/`%` → unary (`-`, `not`, `#`) → postfix (`.`, `?.`, `[]`,
//!     `(...)`, `:method(...)`, `!`) → primary.
//!   * Statements use keyword-led forms (`if … then … end`, `while … do … end`,
//!     `for v = a, b [, s] do … end`, `for v in iter do … end`,
//!     `repeat … until cond`, `try … catch e: T … end`).
//!   * Declarations: `fn`, `class`, `interface`, `enum`, `import`, `export`.
//!
//! Spans on combined nodes use the start of the leftmost child to the end of
//! the rightmost child so diagnostics highlight the whole construct.

mod parseerror;

pub use parseerror::ParseError;

use saule_ast::{
    BinOp, CallArg, ClassMember, Decl, EnumVariant, Expr, ImportNames, LambdaBody, Method,
    MethodSig, Module, Param, Spanned, Stmt, Type, UnaryOp,
};
use saule_lexer::Token;
use std::ops::Range;

// ─── Entry point ─────────────────────────────────────────────────────────────

pub fn parse(tokens: Vec<Spanned<Token>>) -> Result<Module, ParseError> {
    let mut p = Parser::new(tokens);
    let mut stmts = Vec::new();
    while !p.is_eof() {
        stmts.push(p.parse_statement()?);
    }
    Ok(Module { stmts })
}

// ─── Parser state ────────────────────────────────────────────────────────────

pub struct Parser {
    tokens: Vec<Spanned<Token>>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned<Token>>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ── Cursor helpers ──────────────────────────────────────────────────────

    fn peek(&self) -> &Spanned<Token> {
        &self.tokens[self.pos]
    }

    fn peek_at(&self, offset: usize) -> &Spanned<Token> {
        let i = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[i]
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek().value, Token::Eof)
    }

    fn advance(&mut self) -> Spanned<Token> {
        let tok = self.tokens[self.pos].clone();
        if !matches!(tok.value, Token::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, t: &Token) -> bool {
        std::mem::discriminant(&self.peek().value) == std::mem::discriminant(t)
    }

    /// End offset of the most recently consumed token (or 0 if none).
    fn last_consumed_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }

    fn eat(&mut self, t: &Token) -> bool {
        if self.check(t) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Token, what: &'static str) -> Result<Spanned<Token>, ParseError> {
        if self.check(t) {
            Ok(self.advance())
        } else {
            Err(ParseError::Expected {
                expected: what,
                span: self.peek().span.clone(),
            })
        }
    }

    fn expect_ident(&mut self, what: &'static str) -> Result<(String, Range<usize>), ParseError> {
        let tok = self.peek().clone();
        if let Token::Identifier(name) = tok.value {
            self.advance();
            Ok((name, tok.span))
        } else {
            Err(ParseError::Expected {
                expected: what,
                span: tok.span,
            })
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Types
    // ─────────────────────────────────────────────────────────────────────────

    /// Optional return type after a `)` in a function/method/lambda signature.
    /// Accepts either `-> T` (BNF/spec) or `: T` (legacy form still used in
    /// many tests and older READMEs).
    fn parse_return_type_opt(&mut self) -> Result<Option<Type>, ParseError> {
        if self.eat(&Token::Arrow) || self.eat(&Token::Colon) {
            Ok(Some(self.parse_type()?))
        } else {
            Ok(None)
        }
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        let mut ty = self.parse_base_type()?;
        // Trailing `?` makes the type nullable. Allow chaining (`T??` though rare).
        while self.check(&Token::Question) {
            self.advance();
            ty = Type::Nullable(Box::new(ty));
        }
        Ok(ty)
    }

    fn parse_base_type(&mut self) -> Result<Type, ParseError> {
        let tok = self.peek().clone();
        match tok.value {
            Token::LParen => {
                self.advance();
                let mut items = Vec::new();
                if !self.check(&Token::RParen) {
                    items.push(self.parse_type()?);
                    while self.eat(&Token::Comma) {
                        items.push(self.parse_type()?);
                    }
                }
                self.expect(&Token::RParen, "`)` to close tuple type")?;
                if items.len() == 1 {
                    Ok(items.into_iter().next().expect("one tuple item"))
                } else {
                    Ok(Type::Tuple(items))
                }
            }
            Token::Identifier(name) => {
                self.advance();
                // Drop any generic argument list `<T, U>` — generics aren't
                // implemented yet but appear in the README; we accept and
                // ignore them so real programs parse.
                if self.check(&Token::Lt) {
                    self.skip_generic_args()?;
                }
                Ok(Type::Named(name))
            }
            // `nil` is a keyword token but also a legal return type ("void").
            Token::Nil => {
                self.advance();
                Ok(Type::Named("nil".to_string()))
            }
            Token::Fn => {
                self.advance();
                self.expect(&Token::LParen, "`(` in function type")?;
                let mut params = Vec::new();
                if !self.check(&Token::RParen) {
                    params.push(self.parse_type()?);
                    while self.eat(&Token::Comma) {
                        params.push(self.parse_type()?);
                    }
                }
                self.expect(&Token::RParen, "`)` in function type")?;
                if !(self.eat(&Token::Arrow) || self.eat(&Token::Colon)) {
                    return Err(ParseError::Expected {
                        expected: "`->` or `:` before return type",
                        span: self.peek().span.clone(),
                    });
                }
                let ret = self.parse_type()?;
                Ok(Type::Function {
                    params,
                    ret: Box::new(ret),
                })
            }
            _ => Err(ParseError::Expected {
                expected: "a type",
                span: tok.span,
            }),
        }
    }

    /// Consumes a `<T, U, ...>` generic argument list, discarding the types.
    /// We assume `<` is currently the next token.
    fn skip_generic_args(&mut self) -> Result<(), ParseError> {
        self.expect(&Token::Lt, "`<`")?;
        let _ = self.parse_type()?;
        while self.eat(&Token::Comma) {
            let _ = self.parse_type()?;
        }
        self.expect(&Token::Gt, "`>` to close generic arguments")?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Expressions  (lowest precedence first)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_expression(&mut self) -> Result<Spanned<Expr>, ParseError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.and_expr()?;
        while self.check(&Token::Or) {
            self.advance();
            let right = self.and_expr()?;
            left = mk_binary(BinOp::Or, left, right);
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.equality_expr()?;
        while self.check(&Token::And) {
            self.advance();
            let right = self.equality_expr()?;
            left = mk_binary(BinOp::And, left, right);
        }
        Ok(left)
    }

    fn equality_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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

    fn comparison_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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

    fn coalesce_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.concat_expr()?;
        while self.check(&Token::QuestionQuestion) {
            self.advance();
            // `??` is right-associative; recurse for the right operand.
            let right = self.coalesce_expr()?;
            left = mk_binary(BinOp::Coalesce, left, right);
        }
        Ok(left)
    }

    fn concat_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.additive_expr()?;
        while self.check(&Token::DotDot) {
            self.advance();
            // `..` is right-associative in Lua-likes.
            let right = self.concat_expr()?;
            left = mk_binary(BinOp::Concat, left, right);
        }
        Ok(left)
    }

    fn additive_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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

    fn mul_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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

    fn unary_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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

    fn postfix_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut expr = self.primary_expr()?;
        loop {
            match self.peek().value {
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
                Token::Colon => {
                    // `obj:method(args)` — method call.
                    self.advance();
                    let (method, _) = self.expect_ident("method name after `:`")?;
                    let (args, close_span) = self.parse_call_args()?;
                    let span = expr.span.start..close_span.end;
                    expr = Spanned::new(
                        Expr::MethodCall {
                            obj: Box::new(expr),
                            method,
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
    fn parse_call_args(&mut self) -> Result<(Vec<CallArg>, Range<usize>), ParseError> {
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

    fn parse_call_arg(&mut self) -> Result<CallArg, ParseError> {
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

    fn primary_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
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
            Token::Super => {
                self.advance();
                Ok(Spanned::new(Expr::Super, tok.span))
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
            Token::New => self.parse_new(),
            Token::LBrace => self.parse_table_literal(),
            Token::Fn => self.parse_fn_lambda(),
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

    fn parse_new(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let new_tok = self.advance(); // consume `new`
        let (class, _) = self.expect_ident("class name after `new`")?;
        // Optional generic arguments on construction, e.g. `new List<Player>(...)`.
        if self.check(&Token::Lt) {
            self.skip_generic_args()?;
        }
        let (raw_args, close_span) = self.parse_call_args()?;
        let mut args = Vec::with_capacity(raw_args.len());
        for arg in raw_args {
            match arg {
                CallArg::Positional(expr) => args.push(expr),
                CallArg::Named { value, .. } => {
                    return Err(ParseError::Expected {
                        expected: "positional argument in constructor call",
                        span: value.span,
                    });
                }
            }
        }
        let span = new_tok.span.start..close_span.end;
        Ok(Spanned::new(Expr::New { class, args }, span))
    }

    fn parse_table_literal(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let open = self.advance(); // consume `{`
        let mut items = Vec::new();
        if !self.check(&Token::RBrace) {
            items.push(self.parse_expression()?);
            while self.eat(&Token::Comma) {
                if self.check(&Token::RBrace) {
                    break; // allow trailing comma
                }
                items.push(self.parse_expression()?);
            }
        }
        let close = self.expect(&Token::RBrace, "`}` to close table literal")?;
        let span = open.span.start..close.span.end;
        Ok(Spanned::new(Expr::Table(items), span))
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
    fn looks_like_arrow_lambda(&self) -> bool {
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

    fn parse_arrow_lambda(&mut self) -> Result<Spanned<Expr>, ParseError> {
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
    fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(&Token::LParen, "`(` to begin parameter list")?;
        let params = self.parse_param_list_inner()?;
        self.expect(&Token::RParen, "`)` to close parameter list")?;
        Ok(params)
    }

    /// Parses the contents of a parameter list; stops at `)`.
    fn parse_param_list_inner(&mut self) -> Result<Vec<Param>, ParseError> {
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

    fn parse_param(&mut self) -> Result<Param, ParseError> {
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

    // ─────────────────────────────────────────────────────────────────────────
    // Statements
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_statement(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        // Optional `;` separators are skipped.
        while self.eat(&Token::Semi) {}

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
            Token::Fn => self.parse_fn_decl(false).map(stmt_decl),
            Token::Class => self.parse_class_decl(false).map(stmt_decl),
            Token::Interface => self.parse_interface_decl(false).map(stmt_decl),
            Token::Enum => self.parse_enum_decl(false).map(stmt_decl),
            Token::Import => self.parse_import().map(stmt_decl),
            Token::Export => self.parse_export(),
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_local(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        // `local fn name(...)` is a non-exported function declaration.
        if matches!(self.peek_at(1).value, Token::Fn) {
            self.advance(); // consume `local`
            let decl = self.parse_fn_decl(false)?;
            return Ok(stmt_decl(decl));
        }

        let kw = self.advance(); // `local`
        let (first_name, first_span) = self.expect_ident("variable name after `local`")?;
        let first_ty = if self.eat(&Token::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        // `local a: T, b: U = e1, e2`
        if self.check(&Token::Comma) {
            let mut names = vec![(first_name, first_ty)];
            while self.eat(&Token::Comma) {
                let (n, _) = self.expect_ident("variable name in `local` list")?;
                let t = if self.eat(&Token::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                names.push((n, t));
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
                ty: first_ty,
                value,
            },
            kw.span.start..end,
        ))
    }

    fn parse_if(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `if`
        let cond = self.parse_expression()?;
        self.expect(&Token::Then, "`then` after `if` condition")?;
        let then_block = self.parse_block_until(&[Token::Else, Token::End])?;

        let mut elseifs = Vec::new();
        let mut else_block: Option<Vec<Spanned<Stmt>>> = None;

        while self.check(&Token::Else) {
            self.advance();
            // `else if ...` chains as an `elseif` arm.
            if self.check(&Token::If) {
                self.advance();
                let ec = self.parse_expression()?;
                self.expect(&Token::Then, "`then` after `else if` condition")?;
                let eb = self.parse_block_until(&[Token::Else, Token::End])?;
                elseifs.push((ec, eb));
            } else {
                else_block = Some(self.parse_block_until(&[Token::End])?);
                break;
            }
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

    fn parse_while(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `while`
        let cond = self.parse_expression()?;
        self.expect(&Token::Do, "`do` after `while` condition")?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close `while`")?;
        Ok(Spanned::new(
            Stmt::While { cond, body },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_repeat(&mut self) -> Result<Spanned<Stmt>, ParseError> {
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

    fn parse_for(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `for`
        let (first_name, _) = self.expect_ident("loop variable name")?;
        let first_ty = if self.eat(&Token::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        // Lua-style numeric for loop: `for i = from, to [, step] do ... end`
        if self.eat(&Token::Assign) {
            let from = self.parse_expression()?;
            self.expect(&Token::Comma, "`,` after start value in numeric `for`")?;
            let to = self.parse_expression()?;
            let step = if self.eat(&Token::Comma) {
                Some(self.parse_expression()?)
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
        let iter = self.parse_expression()?;
        self.expect(&Token::Do, "`do` in for-in")?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close `for`")?;
        Ok(Spanned::new(
            Stmt::ForIn { vars, iter, body },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_try(&mut self) -> Result<Spanned<Stmt>, ParseError> {
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

    fn parse_return(&mut self) -> Result<Spanned<Stmt>, ParseError> {
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

    fn parse_throw(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // `throw`
        let e = self.parse_expression()?;
        let end = e.span.end;
        Ok(Spanned::new(Stmt::Throw(e), kw.span.start..end))
    }

    /// Parses an expression and, if followed by `=`, an assignment.
    /// Also handles multi-target assignment `a, b = x, y`.
    fn parse_expr_or_assign(&mut self) -> Result<Spanned<Stmt>, ParseError> {
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
    fn parse_block_until(
        &mut self,
        terminators: &[Token],
    ) -> Result<Vec<Spanned<Stmt>>, ParseError> {
        let mut stmts = Vec::new();
        while !self.is_eof() && !terminators.iter().any(|t| self.check(t)) {
            stmts.push(self.parse_statement()?);
        }
        Ok(stmts)
    }

    fn at_block_terminator(&self) -> bool {
        matches!(
            self.peek().value,
            Token::End | Token::Else | Token::Until | Token::Catch
        )
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Declarations
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_export(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        self.advance(); // consume `export`
        let decl = match self.peek().value {
            Token::Fn => self.parse_fn_decl(true)?,
            Token::Class => self.parse_class_decl(true)?,
            Token::Interface => self.parse_interface_decl(true)?,
            Token::Enum => self.parse_enum_decl(true)?,
            _ => {
                return Err(ParseError::Expected {
                    expected: "a declaration after `export`",
                    span: self.peek().span.clone(),
                });
            }
        };
        Ok(stmt_decl(decl))
    }

    fn parse_fn_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `fn`
        let (name, _) = self.expect_ident("function name")?;
        // Optional generic parameter list — accepted and discarded.
        if self.check(&Token::Lt) {
            self.skip_generic_args()?;
        }
        let params = self.parse_param_list()?;
        let return_ty = self.parse_return_type_opt()?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close function")?;
        Ok(Spanned::new(
            Decl::Function {
                exported,
                name,
                params,
                return_ty,
                body,
            },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_class_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `class`
        let (name, _) = self.expect_ident("class name")?;
        if self.check(&Token::Lt) {
            self.skip_generic_args()?;
        }

        let extends = if self.eat(&Token::Extends) {
            let (p, _) = self.expect_ident("parent class name")?;
            if self.check(&Token::Lt) {
                self.skip_generic_args()?;
            }
            Some(p)
        } else {
            None
        };

        let mut implements = Vec::new();
        if self.eat(&Token::Implements) {
            let (n, _) = self.expect_ident("interface name")?;
            if self.check(&Token::Lt) {
                self.skip_generic_args()?;
            }
            implements.push(n);
            while self.eat(&Token::Comma) {
                let (n, _) = self.expect_ident("interface name")?;
                if self.check(&Token::Lt) {
                    self.skip_generic_args()?;
                }
                implements.push(n);
            }
        }

        let mut members = Vec::new();
        while !self.check(&Token::End) && !self.is_eof() {
            members.push(self.parse_class_member()?);
        }
        let end = self.expect(&Token::End, "`end` to close class")?;

        Ok(Spanned::new(
            Decl::Class {
                exported,
                name,
                extends,
                implements,
                members,
            },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_class_member(&mut self) -> Result<Spanned<ClassMember>, ParseError> {
        let start = self.peek().span.start;
        // Accept both `static local ...` and `local static ...`.
        let mut is_static = false;
        let mut has_local = false;
        loop {
            if !is_static && self.eat(&Token::Static) {
                is_static = true;
                continue;
            }
            if !has_local && self.eat(&Token::Local) {
                has_local = true;
                continue;
            }
            break;
        }

        let member = match self.peek().value {
            Token::Fn => {
                let m_start = self.peek().span.start;
                self.advance();
                let (name, _) = self.expect_ident("method name")?;
                if self.check(&Token::Lt) {
                    self.skip_generic_args()?;
                }
                let params = self.parse_param_list()?;
                let return_ty = self.parse_return_type_opt()?;
                let body = self.parse_block_until(&[Token::End])?;
                let end_tok = self.expect(&Token::End, "`end` to close method")?;
                ClassMember::Method(Method {
                    is_static,
                    name,
                    params,
                    return_ty,
                    body,
                    span: m_start..end_tok.span.end,
                })
            }
            Token::Identifier(_) if !has_local => {
                // Public field: `name: T [= default]` (also works after `static`).
                let (name, _) = self.expect_ident("field name")?;
                self.expect(&Token::Colon, "`:` and type on field")?;
                let ty = self.parse_type()?;
                let default = if self.eat(&Token::Assign) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                ClassMember::Field {
                    is_static,
                    name,
                    ty,
                    default,
                }
            }
            Token::Identifier(_) if has_local => {
                // Private field: `local name: T [= default]`.
                let (name, _) = self.expect_ident("field name")?;
                self.expect(&Token::Colon, "`:` and type on field")?;
                let ty = self.parse_type()?;
                let default = if self.eat(&Token::Assign) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                ClassMember::Field {
                    is_static,
                    name,
                    ty,
                    default,
                }
            }
            _ => {
                return Err(ParseError::Expected {
                    expected: "a class member (`[local] name: type`, `fn`, or `static`)",
                    span: self.peek().span.clone(),
                });
            }
        };
        let end = self.last_consumed_end();
        Ok(Spanned::new(member, start..end))
    }

    fn parse_interface_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `interface`
        let (name, _) = self.expect_ident("interface name")?;
        if self.check(&Token::Lt) {
            self.skip_generic_args()?;
        }

        let mut extends = Vec::new();
        if self.eat(&Token::Extends) {
            let (n, _) = self.expect_ident("parent interface name")?;
            if self.check(&Token::Lt) {
                self.skip_generic_args()?;
            }
            extends.push(n);
            while self.eat(&Token::Comma) {
                let (n, _) = self.expect_ident("parent interface name")?;
                if self.check(&Token::Lt) {
                    self.skip_generic_args()?;
                }
                extends.push(n);
            }
        }

        let mut methods = Vec::new();
        while !self.check(&Token::End) && !self.is_eof() {
            let m_start = self.peek().span.start;
            self.expect(&Token::Fn, "`fn` in interface body")?;
            let (mname, _) = self.expect_ident("method name")?;
            if self.check(&Token::Lt) {
                self.skip_generic_args()?;
            }
            let params = self.parse_param_list()?;
            let return_ty = self.parse_return_type_opt()?;
            let m_end = self.last_consumed_end();
            methods.push(MethodSig {
                name: mname,
                params,
                return_ty,
                span: m_start..m_end,
            });
        }
        let end = self.expect(&Token::End, "`end` to close interface")?;

        Ok(Spanned::new(
            Decl::Interface {
                exported,
                name,
                extends,
                methods,
            },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_enum_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `enum`
        let (name, _) = self.expect_ident("enum name")?;

        let mut variants = Vec::new();
        let mut methods = Vec::new();

        // Variants first — newline-separated; we treat them as a run of
        // `Ident [= expr]` tokens, optionally separated by commas.
        loop {
            self.eat(&Token::Comma); // tolerate stray commas
            let tok = self.peek().clone();
            match tok.value {
                Token::Identifier(vname) => {
                    let v_start = tok.span.start;
                    self.advance();
                    let variant = if self.eat(&Token::Assign) {
                        let value = self.parse_expression()?;
                        EnumVariant::Valued(vname, value)
                    } else {
                        EnumVariant::Bare(vname)
                    };
                    let v_end = self.last_consumed_end();
                    variants.push(Spanned::new(variant, v_start..v_end));
                }
                _ => break,
            }
        }

        // Then optional methods.
        while self.check(&Token::Fn) {
            let m_start = self.peek().span.start;
            self.advance();
            let (mname, _) = self.expect_ident("method name")?;
            let params = self.parse_param_list()?;
            let return_ty = self.parse_return_type_opt()?;
            let body = self.parse_block_until(&[Token::End])?;
            let end_tok = self.expect(&Token::End, "`end` to close enum method")?;
            methods.push(Method {
                is_static: false,
                name: mname,
                params,
                return_ty,
                body,
                span: m_start..end_tok.span.end,
            });
        }

        let end = self.expect(&Token::End, "`end` to close enum")?;
        Ok(Spanned::new(
            Decl::Enum {
                exported,
                name,
                variants,
                methods,
            },
            kw.span.start..end.span.end,
        ))
    }

    fn parse_import(&mut self) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `import`

        // `import * from "path"`
        let names = if self.eat(&Token::Star) {
            ImportNames::All
        } else {
            let mut list = Vec::new();
            let (n, _) = self.expect_ident("imported name")?;
            let alias = if self.eat(&Token::As) {
                let (a, _) = self.expect_ident("alias name after `as`")?;
                Some(a)
            } else {
                None
            };
            list.push((n, alias));
            while self.eat(&Token::Comma) {
                let (n, _) = self.expect_ident("imported name")?;
                let alias = if self.eat(&Token::As) {
                    let (a, _) = self.expect_ident("alias name after `as`")?;
                    Some(a)
                } else {
                    None
                };
                list.push((n, alias));
            }
            ImportNames::List(list)
        };

        self.expect(&Token::From, "`from` in import")?;
        let path_tok = self.peek().clone();
        let path = match path_tok.value {
            Token::String(s) => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::Expected {
                    expected: "a quoted module path",
                    span: path_tok.span,
                });
            }
        };
        Ok(Spanned::new(
            Decl::Import { names, path },
            kw.span.start..path_tok.span.end,
        ))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn mk_binary(op: BinOp, lhs: Spanned<Expr>, rhs: Spanned<Expr>) -> Spanned<Expr> {
    let span = lhs.span.start..rhs.span.end;
    Spanned::new(
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    )
}

/// Wraps a `Spanned<Decl>` as a `Spanned<Stmt::Decl(...)>` preserving the span.
fn stmt_decl(d: Spanned<Decl>) -> Spanned<Stmt> {
    let span = d.span.clone();
    Spanned::new(Stmt::Decl(d), span)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use saule_lexer::Lexer;

    fn parse_src(src: &str) -> Module {
        let tokens = Lexer::new(src).tokenize().expect("lex ok");
        parse(tokens).expect("parse ok")
    }

    #[test]
    fn parses_local_with_arithmetic() {
        let m = parse_src("local x: integer = 1 + 2 * 3");
        assert_eq!(m.stmts.len(), 1);
        match &m.stmts[0].value {
            Stmt::Local {
                name,
                value: Some(_),
                ..
            } => assert_eq!(name, "x"),
            _ => panic!("expected local"),
        }
    }

    #[test]
    fn parses_if_else_chain() {
        let src = r#"
            if a then
                x = 1
            else if b then
                x = 2
            else
                x = 3
            end
        "#;
        let m = parse_src(src);
        match &m.stmts[0].value {
            Stmt::If {
                elseifs,
                else_block,
                ..
            } => {
                assert_eq!(elseifs.len(), 1);
                assert!(else_block.is_some());
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn parses_numeric_for() {
        let m = parse_src("for i: integer = 1, 10, 2 do x = i end");
        assert!(matches!(m.stmts[0].value, Stmt::ForNumeric { .. }));
    }

    #[test]
    fn parses_for_in() {
        let m = parse_src("for v: Player in queue do v:greet() end");
        assert!(matches!(m.stmts[0].value, Stmt::ForIn { .. }));
    }

    #[test]
    fn parses_class_with_constructor_and_method() {
        let src = r#"
            class Player extends Entity implements Damageable
                local health: integer

                fn init(name: string, health: integer)
                    self.health = health
                end

                fn isAlive() -> boolean
                    return self.health > 0
                end
            end
        "#;
        let m = parse_src(src);
        match &m.stmts[0].value {
            Stmt::Decl(d) => match &d.value {
                Decl::Class {
                    name,
                    extends,
                    implements,
                    members,
                    ..
                } => {
                    assert_eq!(name, "Player");
                    assert_eq!(extends.as_deref(), Some("Entity"));
                    assert_eq!(implements, &vec!["Damageable".to_string()]);
                    assert_eq!(members.len(), 3);
                }
                _ => panic!("expected class"),
            },
            _ => panic!("expected decl"),
        }
    }

    #[test]
    fn parses_interface_and_enum() {
        let src = r#"
            interface Greetable
                fn greet(self): nil
            end

            enum Direction
                North
                South
                East
                West
            end
        "#;
        let m = parse_src(src);
        assert_eq!(m.stmts.len(), 2);
        match &m.stmts[1].value {
            Stmt::Decl(d) => match &d.value {
                Decl::Enum { variants, .. } => assert_eq!(variants.len(), 4),
                _ => panic!("expected enum"),
            },
            _ => panic!("expected decl"),
        }
    }

    #[test]
    fn parses_lambda_and_call() {
        let m = parse_src("local f: any = (x: integer) => x * 2");
        match &m.stmts[0].value {
            Stmt::Local { value: Some(e), .. } => match &e.value {
                Expr::Lambda { params, .. } => assert_eq!(params.len(), 1),
                _ => panic!("expected lambda"),
            },
            _ => panic!("expected local"),
        }
    }

    #[test]
    fn parses_tuple_return_type() {
        let m = parse_src("fn pair() -> (integer, integer) return 1, 2 end");
        match &m.stmts[0].value {
            Stmt::Decl(d) => match &d.value {
                Decl::Function {
                    return_ty: Some(Type::Tuple(items)),
                    ..
                } => assert_eq!(items.len(), 2),
                other => panic!("expected function with tuple return, got {other:?}"),
            },
            _ => panic!("expected decl"),
        }
    }

    #[test]
    fn parses_null_safety_chain() {
        let m = parse_src("local v: any = a?.b ?? c!");
        assert!(matches!(
            m.stmts[0].value,
            Stmt::Local { value: Some(_), .. }
        ));
    }

    #[test]
    fn parses_try_catch() {
        let src = r#"
            try
                doStuff()
            catch err: Error
                print(err)
            end
        "#;
        let m = parse_src(src);
        assert!(matches!(m.stmts[0].value, Stmt::Try { .. }));
    }

    #[test]
    fn parses_import() {
        let m = parse_src(r#"import Player, Entity as E from "game.entities""#);
        match &m.stmts[0].value {
            Stmt::Decl(d) => assert!(matches!(d.value, Decl::Import { .. })),
            _ => panic!("expected import"),
        }
    }
}
