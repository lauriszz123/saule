//! Atoms: literals, identifiers, parenthesised expressions, and
//! table literals.

use crate::Parser;
use crate::error::ParseError;
use saule_ast::{Expr, LambdaBody, Param, Spanned, TableEntry, Type};
use saule_lexer::Token;

impl Parser {
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
                            body: LambdaBody::Expr(std::sync::Arc::new(body_expr)),
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
            Token::When => self.parse_when_pipe(),
            Token::LParen => {
                if self.looks_like_arrow_lambda() {
                    self.parse_arrow_lambda()
                } else {
                    // Parenthesised expression.
                    self.advance();
                    let inner = self.with_trailing_block(|p| p.parse_expression())?;
                    self.expect_recover(&Token::RParen, "`)`")?;
                    Ok(inner)
                }
            }
            _ => self.error_expr(),
        }
    }

    pub(crate) fn parse_table_literal(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let open = self.advance(); // consume `{`
        let mut items: Vec<TableEntry> = Vec::new();
        self.with_trailing_block(|p| {
            if !p.check(&Token::RBrace) {
                items.push(p.parse_table_entry()?);
                while p.eat(&Token::Comma) {
                    if p.check(&Token::RBrace) {
                        break; // allow trailing comma
                    }
                    items.push(p.parse_table_entry()?);
                }
            }
            Ok(())
        })?;
        let close = self.expect_close(&Token::RBrace, "`}` to close table literal")?;
        let span = open.span.start..close.max(open.span.end);
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
    pub(crate) fn parse_table_entry(&mut self) -> Result<TableEntry, ParseError> {
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
}
