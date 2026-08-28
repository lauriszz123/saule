//! The binary-operator precedence ladder, loosest to tightest:
//! `or` -> `and` -> equality -> comparison -> `|` -> `~` -> `&` ->
//! shift -> `??` -> `..` -> additive -> multiplicative -> unary -> `^` ->
//! `as`.
//!
//! The five bitwise rungs sit in Lua 5.3's order and in Lua 5.3's place —
//! just above comparison, with `..` still binding tighter than a shift.

use crate::error::ParseError;
use crate::{Parser, mk_binary};
use saule_ast::{BinOp, Expr, Spanned, UnaryOp};
use saule_lexer::Token;

impl Parser {
    /// Entry to the precedence ladder, and the one place every nested
    /// sub-expression re-enters it — parenthesised groups, call arguments,
    /// index brackets, table entries. Counting depth here therefore bounds
    /// expression recursion without touching each individual rung.
    pub(crate) fn parse_expression(&mut self) -> Result<Spanned<Expr>, ParseError> {
        self.nested(|p| p.or_expr())
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
        let mut left = self.bor_expr()?;
        loop {
            // `a <> b` — not-equal in SQL, Pascal and BASIC, and nothing at
            // all here. Left alone it reads as `a < (> b)` and reports a
            // missing operand, which describes the parser's predicament
            // rather than the reader's mistake. Recovered as the `!=` it was
            // meant to be, so the expression around it still typechecks.
            //
            // A generic call — `f<>(…)`, the other empty `<>` — never reaches
            // this rung: [`Parser::postfix_expr`] has already claimed it.
            if self.at_empty_angles() {
                self.report_empty_angles(|span| ParseError::LtGtNotEqual { span })?;
                let right = self.bor_expr()?;
                left = mk_binary(BinOp::NotEq, left, right);
                continue;
            }
            let op = match self.peek().value {
                Token::Lt => BinOp::Lt,
                Token::LtEq => BinOp::LtEq,
                Token::Gt => BinOp::Gt,
                Token::GtEq => BinOp::GtEq,
                _ => break,
            };
            self.advance();
            let right = self.bor_expr()?;
            left = mk_binary(op, left, right);
        }
        Ok(left)
    }

    /// `a | b` — bitwise or, the loosest of the five bitwise rungs.
    pub(crate) fn bor_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.bxor_expr()?;
        while self.check(&Token::Pipe) {
            self.advance();
            let right = self.bxor_expr()?;
            left = mk_binary(BinOp::BOr, left, right);
        }
        Ok(left)
    }

    /// `a ~ b` — bitwise xor. Lua spells it `~` because `^` is taken by
    /// exponentiation, and so does Saule for exactly the same reason.
    pub(crate) fn bxor_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.band_expr()?;
        while self.check(&Token::Tilde) {
            self.advance();
            let right = self.band_expr()?;
            left = mk_binary(BinOp::BXor, left, right);
        }
        Ok(left)
    }

    /// `a & b` — bitwise and, the tightest of the three logical bitwise ops.
    pub(crate) fn band_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.shift_expr()?;
        while self.check(&Token::Amp) {
            self.advance();
            let right = self.shift_expr()?;
            left = mk_binary(BinOp::BAnd, left, right);
        }
        Ok(left)
    }

    /// `a << b`, `a >> b` — shifts, left-associative as everywhere else.
    ///
    /// The lexer hands over `>>` whole; the only place a `>>` means two
    /// closers instead is inside a type argument list, which
    /// [`Parser::parse_type`] has already claimed by the time this rung runs.
    pub(crate) fn shift_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut left = self.coalesce_expr()?;
        loop {
            let op = match self.peek().value {
                Token::Shl => BinOp::Shl,
                Token::Shr => BinOp::Shr,
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
            // `~x` — bitwise complement. Prefix `~` and infix `~` (xor) are
            // told apart by position, exactly as `-` already is: the binary
            // rung only looks for `~` after it has an operand in hand.
            Token::Tilde => Some(UnaryOp::BNot),
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
        self.pow_expr()
    }

    /// `a ^ b` — exponentiation.
    ///
    /// Sits *below* the unary layer and is right-associative, matching Lua:
    /// `-2 ^ 2` is `-(2 ^ 2)`, `2 ^ 3 ^ 2` is `2 ^ (3 ^ 2)`, and the right
    /// operand may itself be unary so `2 ^ -1` parses.
    pub(crate) fn pow_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let left = self.cast_expr()?;
        if self.check(&Token::Caret) {
            self.advance();
            let right = self.unary_expr()?;
            return Ok(mk_binary(BinOp::Pow, left, right));
        }
        Ok(left)
    }

    /// `expr as T` — sits between the unary and postfix layers.
    ///
    /// Binding tighter than every binary operator makes the useful readings
    /// the default: `y as integer ?? 0` is `(y as integer) ?? 0`, and
    /// `y as integer + 1` is `(y as integer) + 1`. Binding looser than the
    /// postfix chain means `obj.field() as string` casts the call's result
    /// rather than the callee.
    ///
    /// The loop accepts `x as A as B`, which is a chain of two casts and
    /// means what it reads as: probe, then convert (`v as float as
    /// integer`). The typechecker rejects the links that have no meaning.
    pub(crate) fn cast_expr(&mut self) -> Result<Spanned<Expr>, ParseError> {
        let mut expr = self.postfix_expr()?;
        while self.check(&Token::As) {
            self.advance();
            let ty = self.parse_type()?;
            let span = expr.span.start..self.last_consumed_end();
            expr = Spanned::new(
                Expr::Cast {
                    value: Box::new(expr),
                    ty,
                    // Undecidable here: which reading this is depends on
                    // the operand's type. `saule_typeck::resolve_casts`
                    // fills it in.
                    kind: saule_ast::CastKind::Unresolved,
                },
                span,
            );
        }
        Ok(expr)
    }

    // ── Postfix layer: chains of  `.x`, `?.x`, `[i]`, `(args)`, `:m(args)`, `!`
}
