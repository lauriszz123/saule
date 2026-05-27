//! Declaration parsing: export, fn, class (+ members), interface, enum, import.

use saule_ast::{ClassMember, Decl, EnumVariant, ImportNames, Method, MethodSig, Spanned, Stmt};
use saule_lexer::Token;

use crate::{Parser, stmt_decl};
use crate::error::ParseError;

impl Parser {
    // ─────────────────────────────────────────────────────────────────────────
    // Declarations
    // ─────────────────────────────────────────────────────────────────────────

    pub(crate) fn parse_export(&mut self) -> Result<Spanned<Stmt>, ParseError> {
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

    pub(crate) fn parse_fn_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `fn`
        let (name, _) = self.expect_ident("function name")?;
        // Optional generic parameter list.
        let type_params = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };
        let params = self.parse_param_list()?;
        let return_ty = self.parse_return_type_opt()?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect(&Token::End, "`end` to close function")?;
        Ok(Spanned::new(
            Decl::Function {
                exported,
                name,
                type_params,
                params,
                return_ty,
                body,
            },
            kw.span.start..end.span.end,
        ))
    }

    pub(crate) fn parse_class_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
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

    pub(crate) fn parse_class_member(&mut self) -> Result<Spanned<ClassMember>, ParseError> {
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
                let type_params = if self.check(&Token::Lt) {
                    self.parse_generic_params()?
                } else {
                    Vec::new()
                };
                let params = self.parse_param_list()?;
                let return_ty = self.parse_return_type_opt()?;
                let body = self.parse_block_until(&[Token::End])?;
                let end_tok = self.expect(&Token::End, "`end` to close method")?;
                ClassMember::Method(Method {
                    is_static,
                    is_private: has_local,
                    name,
                    type_params,
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
                    is_private: false,
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
                    is_private: true,
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

    pub(crate) fn parse_interface_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
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

    pub(crate) fn parse_enum_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
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
                    } else if self.check(&Token::LParen) {
                        // `Click(x: integer, y: integer)` — tuple-style payload.
                        let fields = self.parse_param_list()?;
                        EnumVariant::Tuple {
                            name: vname,
                            fields,
                        }
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
                is_private: false,
                name: mname,
                type_params: Vec::new(),
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

    pub(crate) fn parse_import(&mut self) -> Result<Spanned<Decl>, ParseError> {
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
