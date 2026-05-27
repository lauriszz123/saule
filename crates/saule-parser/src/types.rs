//! Type parsing: nullable suffix, table<T>/table<K,V>, generic param/arg
//! lists, function types.

use saule_ast::Type;
use saule_lexer::Token;

use crate::Parser;
use crate::error::ParseError;

impl Parser {
    // ─────────────────────────────────────────────────────────────────────────
    // Types
    // ─────────────────────────────────────────────────────────────────────────

    /// Optional return type after a `)` in a function/method/lambda signature.
    /// Accepts either `-> T` (BNF/spec) or `: T` (legacy form still used in
    /// many tests and older READMEs).
    pub(crate) fn parse_return_type_opt(&mut self) -> Result<Option<Type>, ParseError> {
        if self.eat(&Token::Arrow) || self.eat(&Token::Colon) {
            Ok(Some(self.parse_type()?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn parse_type(&mut self) -> Result<Type, ParseError> {
        let mut ty = self.parse_base_type()?;
        // Trailing `?` makes the type nullable. Allow chaining (`T??` though rare).
        while self.check(&Token::Question) {
            self.advance();
            ty = Type::Nullable(Box::new(ty));
        }
        Ok(ty)
    }

    pub(crate) fn parse_base_type(&mut self) -> Result<Type, ParseError> {
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
                // `table<T>` (array) and `table<K, V>` (hashmap) are first-class
                // forms in the type system. Other identifiers may carry generic
                // arguments which we still accept-and-discard for forward-compat.
                if name == "table" && self.check(&Token::Lt) {
                    self.expect(&Token::Lt, "`<`")?;
                    let first = self.parse_type()?;
                    let (key, value) = if self.eat(&Token::Comma) {
                        let v = self.parse_type()?;
                        (Some(Box::new(first)), Box::new(v))
                    } else {
                        (None, Box::new(first))
                    };
                    self.expect(&Token::Gt, "`>` to close `table<...>`")?;
                    return Ok(Type::Table { key, value });
                }
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
    pub(crate) fn skip_generic_args(&mut self) -> Result<(), ParseError> {
        self.expect(&Token::Lt, "`<`")?;
        let _ = self.parse_type()?;
        while self.eat(&Token::Comma) {
            let _ = self.parse_type()?;
        }
        self.expect(&Token::Gt, "`>` to close generic arguments")?;
        Ok(())
    }

    /// Consumes a `<T, U, ...>` *parameter* list on a function or method
    /// declaration and returns the type-parameter names. Unlike
    /// [`skip_generic_args`], every entry must be a bare identifier.
    pub(crate) fn parse_generic_params(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(&Token::Lt, "`<`")?;
        let mut params = Vec::new();
        let (first, _) = self.expect_ident("generic parameter name")?;
        params.push(first);
        while self.eat(&Token::Comma) {
            let (n, _) = self.expect_ident("generic parameter name")?;
            params.push(n);
        }
        self.expect(&Token::Gt, "`>` to close generic parameters")?;
        Ok(params)
    }

    /// Try to consume a generic-call instantiation `<T, U, ...>` immediately
    /// followed by `(`. Returns `true` if the consumption succeeded; restores
    /// the cursor and returns `false` otherwise (so the `<` can be parsed as
    /// a less-than operator instead). Used at call sites like
    /// `filter<integer>(nums, ...)`.
    pub(crate) fn try_eat_generic_call_args(&mut self) -> bool {
        let saved = self.pos;
        if !self.check(&Token::Lt) {
            return false;
        }
        // Try to consume a `<` ... `>` window where every entry parses as a
        // type. If the window doesn't end in `>(`, restore.
        self.advance(); // `<`
        if self.parse_type().is_err() {
            self.pos = saved;
            return false;
        }
        while self.eat(&Token::Comma) {
            if self.parse_type().is_err() {
                self.pos = saved;
                return false;
            }
        }
        if !self.eat(&Token::Gt) {
            self.pos = saved;
            return false;
        }
        if !self.check(&Token::LParen) {
            self.pos = saved;
            return false;
        }
        true
    }
}
