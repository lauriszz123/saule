//! Lambdas in both spellings (`fn(x) ... end` and `x => ...`) and
//! the parameter lists shared with function declarations.

use crate::Parser;
use crate::error::ParseError;
use saule_ast::{Expr, LambdaBody, Param, Spanned, Type};
use saule_lexer::Token;

use super::*;

impl Parser {
    /// `fn(params) -> T ... end` as an expression (Lua-style anonymous fn).
    pub(crate) fn parse_fn_lambda(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let fn_tok = self.advance(); // consume `fn`

        let params = self.parse_param_list_typed(ParamTypes::Optional)?;
        let return_ty = self.parse_return_type_opt()?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect_close(&Token::End, "`end` to close `fn(...)` lambda")?;
        let span = fn_tok.span.start..end.max(fn_tok.span.end);
        Ok(Spanned::new(
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Block(body.into()),
            },
            span,
        ))
    }

    /// `do (params) -> T … end` — the block half of a trailing-block call.
    ///
    /// Produces an ordinary [`Expr::Lambda`], so everything downstream (the
    /// typechecker's lambda-argument inference, the interpreter's closure
    /// construction, every LSP walker) handles it without knowing this sugar
    /// exists. The caller has already parsed the argument list and appends the
    /// result as the final positional argument.
    ///
    /// Both the parameter list and the return type are optional; the common
    /// case is a bare `do … end` taking nothing. A `(` immediately after `do`
    /// is always read as a parameter list, never as a parenthesised expression
    /// statement opening the block.
    pub(crate) fn parse_trailing_block(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let do_tok = self.expect(&Token::Do, "`do` to open a trailing block")?;
        let (params, return_ty) = if self.check(&Token::LParen) {
            let params = self.parse_param_list_typed(ParamTypes::Optional)?;
            (params, self.parse_return_type_opt()?)
        } else {
            (Vec::new(), None)
        };
        // A trailing block's body is a fresh statement context: a `do` inside it
        // belongs to whatever call it follows, not to an enclosing loop header.
        let body = self.with_trailing_block(|p| p.parse_block_until(&[Token::End]))?;
        let end = self.expect_close(&Token::End, "`end` to close trailing block")?;
        Ok(Spanned::new(
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Block(body.into()),
            },
            do_tok.span.start..end.max(do_tok.span.end),
        ))
    }

    /// Decide whether `(` starts an arrow lambda or a parenthesised
    /// expression, by speculatively parsing the lambda's header and rewinding.
    ///
    /// The header is `(params) [-> T] =>`, and the optional return type is why
    /// this is a real parse rather than a token peek: `-> T` is the full type
    /// grammar (`table<K, V>`, `fn(A) -> B`, trailing `?`), so scanning for the
    /// `=>` by hand would mean a second copy of that grammar to keep in sync.
    /// Parsing it with [`Self::parse_type`] cannot drift.
    ///
    /// Nothing is committed: the attempt runs inside [`Self::speculate`], so
    /// the cursor is rewound, no diagnostic is recorded, and — crucially —
    /// error recovery is switched off for the duration. A probe that
    /// recovered its way to success would read `(1 + 2)` as a lambda header
    /// with a patched-over parameter name.
    pub(crate) fn looks_like_arrow_lambda(&mut self) -> bool {
        let start = self.pos;
        let ok = self
            .speculate(|p| p.try_arrow_lambda_header().then_some(()))
            .is_some();
        // This only answers a question — the caller re-parses from `start`
        // either way — so the cursor is restored on both paths, not just the
        // failing one `speculate` handles.
        self.pos = start;
        ok
    }

    /// The speculative half of [`Self::looks_like_arrow_lambda`]. Leaves the
    /// cursor wherever it got to; the caller rewinds.
    fn try_arrow_lambda_header(&mut self) -> bool {
        if self.expect(&Token::LParen, "`(`").is_err() {
            return false;
        }
        if self.parse_param_list_inner(ParamTypes::Optional).is_err() {
            return false;
        }
        if self.expect(&Token::RParen, "`)`").is_err() {
            return false;
        }
        if self.parse_return_type_opt().is_err() {
            return false;
        }
        self.check(&Token::FatArrow)
    }

    pub(crate) fn parse_arrow_lambda(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let open = self.expect(&Token::LParen, "`(`")?;
        let params = self.parse_param_list_inner(ParamTypes::Optional)?;
        self.expect_close(&Token::RParen, "`)` after lambda parameters")?;
        let return_ty = self.parse_return_type_opt()?;
        self.expect_recover(&Token::FatArrow, "`=>` in lambda")?;
        let body_expr = self.parse_expression()?;
        let span = open.span.start..body_expr.span.end;
        Ok(Spanned::new(
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Expr(std::sync::Arc::new(body_expr)),
            },
            span,
        ))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Parameter lists
    // ─────────────────────────────────────────────────────────────────────────

    /// Parses `( ... )` and returns the parameter list. Declarations require
    /// a type on every parameter; see [`Self::parse_param`] for why lambdas
    /// don't.
    pub(crate) fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        self.parse_param_list_typed(ParamTypes::Required)
    }

    /// [`Self::parse_param_list`] with control over whether parameter types
    /// may be omitted.
    pub(crate) fn parse_param_list_typed(
        &mut self,
        types: ParamTypes,
    ) -> Result<Vec<Param>, ParseError> {
        if self
            .expect_recover(&Token::LParen, "`(` to begin parameter list")?
            .is_none()
        {
            // A signature still being typed — `fn draw` with the `(` not yet
            // written. Assuming an empty list keeps the declaration *and its
            // body*; parsing on would read the next line as parameters and
            // lose both.
            return Ok(Vec::new());
        }
        let params = self.parse_param_list_inner(types)?;
        self.expect_close(&Token::RParen, "`)` to close parameter list")?;
        Ok(params)
    }

    /// Parses the contents of a parameter list; stops at `)`.
    pub(crate) fn parse_param_list_inner(
        &mut self,
        types: ParamTypes,
    ) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.check(&Token::RParen) {
            return Ok(params);
        }
        params.push(self.parse_param(types)?);
        while self.eat(&Token::Comma) {
            params.push(self.parse_param(types)?);
        }
        Ok(params)
    }

    /// Parse one parameter.
    ///
    /// With [`ParamTypes::Optional`] a missing `: T` yields `any`, which the
    /// typechecker then refines from the lambda's target type — so
    /// `local f: fn(integer) -> integer = fn(x) ... end` types `x` as
    /// `integer`. Declarations pass [`ParamTypes::Required`]: a top-level
    /// `fn` has no target to infer from, and its signature is the contract
    /// callers rely on.
    pub(crate) fn parse_param(&mut self, types: ParamTypes) -> Result<Param, ParseError> {
        let start = self.peek().span.start;
        let variadic = self.eat(&Token::Ellipsis);
        // `self` is a keyword token but is also a legal parameter name in
        // method signatures (`fn greet(self): nil`).
        let name = if self.check(&Token::Self_) {
            self.advance();
            "self".to_string()
        } else {
            self.expect_ident_recover("parameter name")?.0
        };
        // Implicit `self` parameter in method signatures: typed as the
        // enclosing class. We don't know that here, so use a placeholder.
        let ty = if self.eat(&Token::Colon) {
            self.parse_type()?
        } else if name == "self" {
            Type::Named("self".to_string())
        } else if types == ParamTypes::Optional {
            Type::Named("any".to_string())
        } else {
            self.error_type("`:` and parameter type")?
        };
        let default = if self.eat(&Token::Assign) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        Ok(Param {
            name,
            ty,
            default,
            variadic,
            span: self.span_to_here(start),
        })
    }
}
