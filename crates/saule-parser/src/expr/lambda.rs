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
        let end = self.expect(&Token::End, "`end` to close `fn(...)` lambda")?;
        let span = fn_tok.span.start..end.span.end;
        Ok(Spanned::new(
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Block(body.into()),
            },
            span,
        ))
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
        let params = self.parse_param_list_inner(ParamTypes::Optional)?;
        self.expect(&Token::RParen, "`)` after lambda parameters")?;
        let return_ty = self.parse_return_type_opt()?;
        self.expect(&Token::FatArrow, "`=>` in lambda")?;
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
        self.expect(&Token::LParen, "`(` to begin parameter list")?;
        let params = self.parse_param_list_inner(types)?;
        self.expect(&Token::RParen, "`)` to close parameter list")?;
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
            self.expect_ident("parameter name")?.0
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
