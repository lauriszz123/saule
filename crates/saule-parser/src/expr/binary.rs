//! The binary-operator precedence ladder, loosest to tightest:
//! `or` -> `and` -> equality -> comparison -> `??` -> `..` ->
//! additive -> multiplicative -> unary -> `^` -> `as`.

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
    /// The loop tolerates `x as A as B`; it parses, and the typechecker
    /// rejects it because the second operand is no longer `any`.
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
                },
                span,
            );
        }
        Ok(expr)
    }

    // ── Postfix layer: chains of  `.x`, `?.x`, `[i]`, `(args)`, `:m(args)`, `!`
}
