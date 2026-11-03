//! Type parsing: nullable suffix, table<T>/table<K,V>, generic param/arg
//! lists, function types.

use saule_ast::{Spanned, Type, TypeArgs, TypeBound, TypeRef};
use saule_lexer::Token;
use std::ops::Range;

use crate::Parser;
use crate::error::ParseError;

/// A parsed `<T, U: Bound>` parameter list.
///
/// Two vectors rather than one list of pairs because the parameter *names*
/// are what the rest of the compiler reads, in ~250 places, while the bounds
/// matter in three — see [`saule_ast::TypeBound`].
#[derive(Debug, Default, Clone)]
pub(crate) struct GenericParams {
    pub(crate) names: Vec<String>,
    pub(crate) bounds: Vec<TypeBound>,
}

impl Parser {
    // ── `<>` ────────────────────────────────────────────────────────────────

    /// Whether the cursor is on a `<` closed by the very next token.
    ///
    /// `<>` has no reading in Saule: empty type argument and parameter lists
    /// are not a thing, and the pair is not an operator either. Recognising
    /// it as a *shape* — before any rule tries to parse a type out of it — is
    /// what lets each caller say which of those three mistakes it is looking
    /// at, instead of leaving the reader with "expected an expression"
    /// pointing at the `>`.
    pub(crate) fn at_empty_angles(&self) -> bool {
        self.check(&Token::Lt) && matches!(self.peek_at(1).value, Token::Gt)
    }

    /// Consume the `<>` under the cursor and report it as `err`.
    ///
    /// Recovering: the brackets are dropped and parsing continues as though
    /// they had never been written, which is what makes the rest of the line
    /// — the argument list, the function body — still parse. Returns `Err`
    /// while speculating, where a probe needs the failure.
    pub(crate) fn report_empty_angles(
        &mut self,
        err: impl FnOnce(Range<usize>) -> ParseError,
    ) -> Result<(), ParseError> {
        let open = self.advance();
        let close = self.advance();
        let err = err(open.span.start..close.span.end);
        if !self.recovering() {
            return Err(err);
        }
        self.record(err);
        Ok(())
    }

    /// Re-split a `>>` sitting where a type-argument list wants to close.
    ///
    /// `table<table<integer>>` ends in two closers the lexer has no way to
    /// tell from a right shift, so it always produces [`Token::Shr`] and
    /// leaves the ambiguity to the one caller that can resolve it: a parser
    /// that is *already inside* `<…>` and needs a `>`. Every site expecting
    /// that closer calls this first; it rewrites the token stream in place
    /// into the two `Gt`s the grammar wanted, so the outer list finds its own
    /// closer waiting.
    ///
    /// A no-op on every other token, so calling it costs one discriminant
    /// test on the overwhelmingly common single-`>` close.
    fn split_closing_shr(&mut self) {
        if !self.check(&Token::Shr) {
            return;
        }
        let span = self.peek().span.clone();
        let mid = span.start + 1;
        self.replace_current(
            Spanned::new(Token::Gt, span.start..mid),
            Spanned::new(Token::Gt, mid..span.end),
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Types
    // ─────────────────────────────────────────────────────────────────────────

    /// Optional return type after a `)` in a function/method/lambda signature.
    /// Only `-> T` is accepted; the legacy `: T` form has been removed.
    pub(crate) fn parse_return_type_opt(&mut self) -> Result<Option<Type>, ParseError> {
        if self.eat(&Token::Arrow) {
            Ok(Some(self.parse_type()?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn parse_type(&mut self) -> Result<Type, ParseError> {
        let mut ty = self.parse_base_type()?;
        // Suffixes bind left to right, so `T?[]` is an array of nullable `T`
        // and `T[]?` a nullable array. Both read the way they are written.
        loop {
            if self.check(&Token::Question) {
                // Trailing `?` makes the type nullable. Allow chaining
                // (`T??` though rare).
                self.advance();
                ty = Type::Nullable(Box::new(ty));
            } else if self.at_array_suffix() {
                ty = self.parse_array_suffix(ty)?;
            } else {
                break;
            }
        }
        Ok(ty)
    }

    /// Is the cursor on an array suffix — `[`, then only commas, then `]`?
    ///
    /// Checked before consuming anything because `parse_type` is reachable
    /// from `cast_expr`, where a `[` after the type belongs to the
    /// expression rather than to the annotation. `x as Dog[1]` has to leave
    /// its bracket alone; only an *empty* bracket group, or one holding
    /// nothing but commas, is a suffix, and no expression can be either.
    pub(crate) fn at_array_suffix(&self) -> bool {
        if !self.check(&Token::LBracket) {
            return false;
        }
        // `peek_at` clamps at the end of the stream, so an unterminated
        // group runs into `Eof` and answers `false` rather than reading off
        // the end.
        let mut i = 1;
        loop {
            match self.peek_at(i).value {
                Token::RBracket => return true,
                Token::Comma => i += 1,
                _ => return false,
            }
        }
    }

    /// `T[]` → `table<integer, T>`, and one more wrap per comma: `T[,]` is
    /// `table<integer, table<integer, T>>`, the same type `T[][]` builds by
    /// going round the suffix loop twice.
    ///
    /// The key is written out as `integer` rather than left implicit. Both
    /// spellings are the same type — see `table_key` in `saule-typeck` — and
    /// naming it keeps the sugar's meaning visible in the error messages
    /// that print a type back.
    fn parse_array_suffix(&mut self, mut ty: Type) -> Result<Type, ParseError> {
        self.expect(&Token::LBracket, "`[`")?;
        let mut dims = 1;
        while self.eat(&Token::Comma) {
            dims += 1;
        }
        self.expect_close(&Token::RBracket, "`]` to close an array type")?;
        for _ in 0..dims {
            ty = Type::Table {
                key: Some(Box::new(Type::Named("integer".to_string()))),
                value: Box::new(ty),
            };
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
                self.expect_close(&Token::RParen, "`)` to close tuple type")?;
                if items.len() == 1 {
                    Ok(items.into_iter().next().expect("one tuple item"))
                } else {
                    Ok(Type::Tuple(items))
                }
            }
            Token::Identifier(name) => {
                self.advance();
                // `table<T>` (array) and `table<K, V>` (hashmap) have their own
                // node — see `Type::Generic` for why the one built-in
                // container stays special-cased. Any other identifier carrying
                // arguments is a user-declared generic.
                if name == "table" && self.check(&Token::Lt) {
                    // `table<>`: recover as the bare `table` the brackets
                    // were adding nothing to.
                    if self.at_empty_angles() {
                        self.report_empty_angles(|span| ParseError::EmptyTypeArgs { span })?;
                        return Ok(Type::Named(name));
                    }
                    self.expect(&Token::Lt, "`<`")?;
                    let first = self.parse_type()?;
                    let (key, value) = if self.eat(&Token::Comma) {
                        let v = self.parse_type()?;
                        (Some(Box::new(first)), Box::new(v))
                    } else {
                        (None, Box::new(first))
                    };
                    self.split_closing_shr();
                    self.expect_close(&Token::Gt, "`>` to close `table<...>`")?;
                    return Ok(Type::Table { key, value });
                }
                // `Result<integer>`, `Box<string>` — a user-declared generic
                // applied to its arguments. These used to be parsed and
                // thrown away, which is why `local b: Box<integer>` checked
                // as a bare `Box` and the argument meant nothing.
                if self.check(&Token::Lt) {
                    let args = self.parse_generic_args()?;
                    // An empty list only happens after `<>` was reported;
                    // fall back to the bare name so the rest still parses.
                    if !args.is_empty() {
                        return Ok(Type::generic(name, args));
                    }
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
                self.expect_close(&Token::RParen, "`)` in function type")?;
                self.expect_recover(&Token::Arrow, "`->` before return type")?;
                let ret = self.parse_type()?;
                Ok(Type::Function {
                    params,
                    ret: Box::new(ret),
                })
            }
            _ => self.error_type("a type"),
        }
    }

    /// Consumes a `<T, U, ...>` generic argument list and returns the types.
    /// We assume `<` is currently the next token.
    ///
    /// Returns an empty vector for `<>`, which has already been reported —
    /// the caller then treats the reference as the bare name it otherwise is,
    /// so one bad annotation costs its own arguments and nothing else.
    pub(crate) fn parse_generic_args(&mut self) -> Result<Vec<Type>, ParseError> {
        if self.at_empty_angles() {
            self.report_empty_angles(|span| ParseError::EmptyTypeArgs { span })?;
            return Ok(Vec::new());
        }
        self.expect(&Token::Lt, "`<`")?;
        let mut args = vec![self.parse_type()?];
        while self.eat(&Token::Comma) {
            args.push(self.parse_type()?);
        }
        self.split_closing_shr();
        self.expect_close(&Token::Gt, "`>` to close generic arguments")?;
        Ok(args)
    }

    /// A named type reference in a declaration header — the `Animal` in
    /// `extends Animal`, the `Repository<Player>` in `implements …`.
    pub(crate) fn parse_type_ref(&mut self, what: &'static str) -> Result<TypeRef, ParseError> {
        let (name, _) = self.expect_ident_recover(what)?;
        let args = if self.check(&Token::Lt) {
            self.parse_generic_args()?
        } else {
            Vec::new()
        };
        Ok(TypeRef { name, args })
    }

    /// Consumes a `<T, U, ...>` *parameter* list on a function or method
    /// declaration. Unlike [`Parser::skip_generic_args`], every entry is a
    /// bare identifier, optionally followed by `: Bound`.
    pub(crate) fn parse_generic_params(&mut self) -> Result<GenericParams, ParseError> {
        // `fn map<>(…)` — recovered as the non-generic declaration it
        // otherwise is, so its parameters and body still parse.
        if self.at_empty_angles() {
            self.report_empty_angles(|span| ParseError::EmptyTypeParams { span })?;
            return Ok(GenericParams::default());
        }
        self.expect(&Token::Lt, "`<`")?;
        let mut out = GenericParams::default();
        self.parse_generic_param(&mut out)?;
        while self.eat(&Token::Comma) {
            self.parse_generic_param(&mut out)?;
        }
        self.split_closing_shr();
        self.expect_close(&Token::Gt, "`>` to close generic parameters")?;
        Ok(out)
    }

    /// One entry of a parameter list: `T`, or `T: Bound`.
    ///
    /// The bound is a [`TypeRef`] rather than a full type: a constraint names
    /// an interface or a class, with its own type arguments
    /// (`<T: OpAdd<T, T>>`). `T: integer?` or `T: fn() -> nil` would say
    /// nothing a parameter could satisfy, so the shape that parses is the
    /// shape that means something.
    fn parse_generic_param(&mut self, out: &mut GenericParams) -> Result<(), ParseError> {
        let (name, _) = self.expect_ident_recover("generic parameter name")?;
        if self.eat(&Token::Colon) {
            let start = self.peek().span.start;
            let bound = self.parse_type_ref("the interface or class to bound by")?;
            let end = self.last_consumed_end().max(start);
            out.bounds.push(TypeBound {
                param: name.clone(),
                bound,
                span: start..end,
            });
        }
        out.names.push(name);
        Ok(())
    }

    /// Try to consume a generic-call instantiation `<T, U, ...>` immediately
    /// followed by `(`. Returns `true` if the consumption succeeded; restores
    /// the cursor and returns `false` otherwise (so the `<` can be parsed as
    /// a less-than operator instead). Used at call sites like
    /// `filter<integer>(nums, ...)`.
    /// Runs inside [`Parser::speculate`], which rewinds the cursor and — the
    /// part that matters here — disables error recovery, so `parse_type`
    /// still *reports* a non-type instead of patching an `any` over it and
    /// letting `a < b` masquerade as an instantiation.
    ///
    /// Deliberately does **not** call [`Self::split_closing_shr`] before its
    /// own `>`. Splitting edits the token stream, and `speculate` rewinds only
    /// the cursor — so a probe that split and then failed would leave a real
    /// right shift permanently torn in two, and `a < b >> c` would stop
    /// parsing. The split is not needed here anyway: a nested close like
    /// `filter<table<integer>>(xs)` is already split by the *inner*
    /// `parse_type`, which leaves a plain `>` for this `eat` to take. The
    /// only shape that would need it is `f<T>>(…)`, which is not a call.
    pub(crate) fn try_eat_generic_call_args(&mut self) -> Option<Box<TypeArgs>> {
        if !self.check(&Token::Lt) {
            return None;
        }
        let start = self.peek().span.start;
        self.speculate(|p| {
            // Consume a `<` ... `>` window where every entry parses as a
            // type. If the window doesn't end in `>(`, this isn't one.
            p.advance(); // `<`
            let mut types = vec![p.parse_type().ok()?];
            while p.eat(&Token::Comma) {
                types.push(p.parse_type().ok()?);
            }
            if !p.eat(&Token::Gt) {
                return None;
            }
            let end = p.last_consumed_end();
            p.check(&Token::LParen).then(|| {
                Box::new(TypeArgs {
                    types,
                    span: start..end,
                })
            })
        })
    }
}
