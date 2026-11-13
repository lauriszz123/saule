//! Declaration parsing: export, fn, class (+ members), interface, enum, import.

use saule_ast::{ClassMember, Decl, EnumVariant, ImportNames, Method, MethodSig, Spanned, Stmt};
use saule_lexer::Token;

use crate::error::ParseError;
use crate::types::GenericParams;
use crate::{Parser, stmt_decl};

/// What every import error points at: the module comes first.
const IMPORT_FORMS: &str = "the module comes first: `from \"uikit\" import *`, \
                            `from shine import Window`, or `import shine`";

impl Parser {
    // ─────────────────────────────────────────────────────────────────────────
    // Declarations
    // ─────────────────────────────────────────────────────────────────────────

    pub(crate) fn parse_export(&mut self) -> Result<Spanned<Stmt>, ParseError> {
        let kw = self.advance(); // consume `export`
        let decl = match self.peek().value {
            Token::Fn => self.parse_fn_decl(true, false)?,
            Token::Class => self.parse_class_decl(true)?,
            Token::Interface => self.parse_interface_decl(true)?,
            Token::Enum => self.parse_enum_decl(true)?,
            Token::From => self.parse_from_import(true)?,
            Token::Import => {
                let tok = self.peek().clone();
                return Err(ParseError::Expected {
                    expected: "a declaration after `export`".into(),
                    found: "`import`".into(),
                    help: Some(
                        "a namespace cannot be re-exported; name what to pass on with \
                         `export from path import Name` or `export from path import *`"
                            .to_string(),
                    ),
                    span: tok.span,
                    label: "here".into(),
                });
            }
            // `export name: T = expr` — a module-level variable. No keyword
            // introduces it, so an identifier here is the whole signal.
            Token::Identifier(_) => self.parse_variable_decl(true, kw.span.start)?,
            _ => {
                return Err(self.expected_here("a declaration or `name: T = value` after `export`"));
            }
        };
        Ok(stmt_decl(decl))
    }

    /// `name: T = expr`, the body of an `export`ed module variable. The
    /// annotation and the initializer are each optional in the grammar;
    /// leaving the value off is what the "never initialized" check in
    /// typeck reports against a non-nullable `T`.
    pub(crate) fn parse_variable_decl(
        &mut self,
        exported: bool,
        start: usize,
    ) -> Result<Spanned<Decl>, ParseError> {
        let (name, name_span) = self.expect_ident_recover("variable name")?;
        let (ty, ty_span) = if self.eat(&Token::Colon) {
            let ty_start = self.peek().span.start;
            let t = self.parse_type()?;
            (Some(t), Some(self.span_to_here(ty_start)))
        } else {
            (None, None)
        };
        let (value, end) = if self.eat(&Token::Assign) {
            let e = self.parse_expression()?;
            let end = e.span.end;
            (Some(e), end)
        } else {
            (None, self.last_consumed_end())
        };
        Ok(Spanned::new(
            Decl::Variable {
                exported,
                name,
                name_span,
                ty,
                ty_span,
                value,
            },
            start..end,
        ))
    }

    pub(crate) fn parse_fn_decl(
        &mut self,
        exported: bool,
        is_local: bool,
    ) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `fn`
        // Strict — see `parse_class_member`: a `fn` with no identifiable
        // name would open a body running to the next `end`, taking every
        // declaration after it down with the one bad line.
        let (name, _) = self.expect_ident("function name")?;
        // Optional generic parameter list.
        let generics = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            GenericParams::default()
        };
        let (type_params, type_bounds) = (generics.names, generics.bounds);
        let params = self.parse_param_list()?;
        let return_ty = self.parse_return_type_opt()?;
        let body = self.parse_block_until(&[Token::End])?;
        let end = self.expect_close(&Token::End, "`end` to close function")?;
        Ok(Spanned::new(
            Decl::Function {
                exported,
                is_local,
                name,
                type_params,
                type_bounds,
                params,
                return_ty,
                body,
            },
            kw.span.start..end.max(kw.span.end),
        ))
    }

    pub(crate) fn parse_class_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `class`
        let (name, _) = self.expect_ident("class name")?;
        let generics = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            GenericParams::default()
        };
        let (type_params, type_bounds) = (generics.names, generics.bounds);

        let extends = if self.eat(&Token::Extends) {
            Some(self.parse_type_ref("parent class name")?)
        } else {
            None
        };

        let mut implements = Vec::new();
        if self.eat(&Token::Implements) {
            implements.push(self.parse_type_ref("interface name")?);
            while self.eat(&Token::Comma) {
                implements.push(self.parse_type_ref("interface name")?);
            }
        }

        let mut members = Vec::new();
        let mut body_col = None;
        self.block_depth += 1;
        while !self.check(&Token::End) && !self.is_eof() {
            // A declaration that has left the class body: the `end` is
            // missing and this was never one of its methods. Same rule as in
            // `parse_block_until`.
            if self.block_ends_here(body_col) {
                break;
            }
            // One unparseable member costs that member, not the class: the
            // ones around it still reach hover, completion and the registry.
            let before = self.pos;
            let began_line = self.at_line_start();
            let col = self.line_col();
            match self.parse_class_member() {
                Ok(m) => members.push(m),
                Err(e) if self.recovering() => {
                    self.record(e);
                    self.synchronize_member();
                }
                Err(e) => return Err(e),
            }
            if self.pos == before {
                self.advance();
            }
            if began_line && body_col.is_none() {
                body_col = col;
            }
        }
        self.block_depth -= 1;
        let end = self.expect_close(&Token::End, "`end` to close class")?;

        Ok(Spanned::new(
            Decl::Class {
                exported,
                name,
                type_params,
                type_bounds,
                extends,
                implements,
                members,
            },
            kw.span.start..end.max(kw.span.end),
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
                // Strict, unlike everywhere else a name is read. A method
                // body runs to `end`, so a `fn` the parser cannot identify
                // would open a block that swallows every member after it —
                // the whole class for the price of one bad line. Failing
                // here instead hands the member loop a chance to
                // resynchronise on the next `fn`.
                let (name, _) = self.expect_ident("method name")?;
                let generics = if self.check(&Token::Lt) {
                    self.parse_generic_params()?
                } else {
                    GenericParams::default()
                };
                let (type_params, type_bounds) = (generics.names, generics.bounds);
                let params = self.parse_param_list()?;
                let return_ty = self.parse_return_type_opt()?;
                let body = self.parse_block_until(&[Token::End])?;
                let end = self.expect_close(&Token::End, "`end` to close method")?;
                ClassMember::Method(Method {
                    is_static,
                    is_private: has_local,
                    name,
                    type_params,
                    type_bounds,
                    params,
                    return_ty,
                    body,
                    span: m_start..end.max(m_start),
                })
            }
            Token::Identifier(_) if !has_local => {
                // Public field: `name: T [= default]` (also works after `static`).
                let (name, _) = self.expect_ident_recover("field name")?;
                self.expect_recover(&Token::Colon, "`:` and type on field")?;
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
                let (name, _) = self.expect_ident_recover("field name")?;
                self.expect_recover(&Token::Colon, "`:` and type on field")?;
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
                return Err(
                    self.expected_here("a class member (`[local] name: type`, `fn`, or `static`)")
                );
            }
        };
        Ok(Spanned::new(member, self.span_to_here(start)))
    }

    pub(crate) fn parse_interface_decl(
        &mut self,
        exported: bool,
    ) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `interface`
        let (name, _) = self.expect_ident("interface name")?;
        let generics = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            GenericParams::default()
        };
        let (type_params, type_bounds) = (generics.names, generics.bounds);

        let mut extends = Vec::new();
        if self.eat(&Token::Extends) {
            extends.push(self.parse_type_ref("parent interface name")?);
            while self.eat(&Token::Comma) {
                extends.push(self.parse_type_ref("parent interface name")?);
            }
        }

        let mut methods = Vec::new();
        let mut body_col = None;
        self.block_depth += 1;
        while !self.check(&Token::End) && !self.is_eof() {
            if self.block_ends_here(body_col) {
                break;
            }
            // As in `parse_class_decl`: a bad signature costs one method.
            let before = self.pos;
            let began_line = self.at_line_start();
            let col = self.line_col();
            match self.parse_interface_method() {
                Ok(m) => methods.push(m),
                Err(e) if self.recovering() => {
                    self.record(e);
                    self.synchronize_member();
                }
                Err(e) => return Err(e),
            }
            if self.pos == before {
                self.advance();
            }
            if began_line && body_col.is_none() {
                body_col = col;
            }
        }
        self.block_depth -= 1;
        let end = self.expect_close(&Token::End, "`end` to close interface")?;

        Ok(Spanned::new(
            Decl::Interface {
                exported,
                name,
                type_params,
                type_bounds,
                extends,
                methods,
            },
            kw.span.start..end.max(kw.span.end),
        ))
    }

    /// One `fn name(params) [-> T]` signature line in an interface body.
    fn parse_interface_method(&mut self) -> Result<MethodSig, ParseError> {
        let m_start = self.peek().span.start;
        self.expect(&Token::Fn, "`fn` in interface body")?;
        let (name, _) = self.expect_ident("method name")?;
        let generics = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            GenericParams::default()
        };
        let (type_params, type_bounds) = (generics.names, generics.bounds);
        let params = self.parse_param_list()?;
        let return_ty = self.parse_return_type_opt()?;
        let m_end = self.last_consumed_end();
        Ok(MethodSig {
            name,
            type_params,
            type_bounds,
            params,
            return_ty,
            span: m_start..m_end.max(m_start),
        })
    }

    pub(crate) fn parse_enum_decl(&mut self, exported: bool) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `enum`
        let (name, _) = self.expect_ident("enum name")?;
        let generics = if self.check(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            GenericParams::default()
        };
        let (type_params, type_bounds) = (generics.names, generics.bounds);

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
            // Strict for the same reason as class methods — see
            // `parse_class_member`.
            let (mname, _) = self.expect_ident("method name")?;
            let params = self.parse_param_list()?;
            let return_ty = self.parse_return_type_opt()?;
            let body = self.parse_block_until(&[Token::End])?;
            let m_end = self.expect_close(&Token::End, "`end` to close enum method")?;
            methods.push(Method {
                is_static: false,
                is_private: false,
                name: mname,
                type_params: Vec::new(),
                type_bounds: Vec::new(),
                params,
                return_ty,
                body,
                span: m_start..m_end.max(m_start),
            });
        }

        let end = self.expect_close(&Token::End, "`end` to close enum")?;
        Ok(Spanned::new(
            Decl::Enum {
                exported,
                name,
                type_params,
                type_bounds,
                variants,
                methods,
            },
            kw.span.start..end.max(kw.span.end),
        ))
    }

    /// `import path` or `import path as name`: a namespace. The path is a
    /// quoted literal or a bare dotted chain; the name defaults to its last
    /// segment.
    pub(crate) fn parse_import(&mut self) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `import`
        if matches!(self.peek().value, Token::Star) || self.at_old_import_list() {
            return Err(self
                .expected_here("a module after `import`")
                .with_help(IMPORT_FORMS));
        }
        let (path, quoted, mut end) = self.parse_module_path()?;
        let name = if self.eat(&Token::As) {
            let (alias, span) = self.expect_ident_recover("a name after `as`")?;
            end = span.end;
            alias
        } else {
            let last = path
                .rsplit(['/', '.'])
                .next()
                .unwrap_or_default()
                .to_string();
            let usable = last
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
                && last.chars().all(|c| c.is_alphanumeric() || c == '_');
            if !usable && !path.is_empty() {
                let err = ParseError::Expected {
                    expected: "`as name` after this module".into(),
                    found: format!("`{last}`, which is not a name").into(),
                    help: Some(format!("write `import \"{path}\" as someName`")),
                    span: kw.span.start..end,
                    label: "here".into(),
                };
                if !self.recovering() {
                    return Err(err);
                }
                self.record(err);
            }
            last
        };
        if !name.is_empty() {
            self.namespaces.insert(name.clone());
        }
        Ok(Spanned::new(
            Decl::Import {
                names: ImportNames::Namespace(name),
                path,
                quoted,
                exported: false,
            },
            kw.span.start..end.max(kw.span.end),
        ))
    }

    /// The removed `import A, B from path`, recognised only where nothing
    /// else can be meant: a list of two or more names before `from`.
    ///
    /// A single name is left alone. `import uikit` followed by a `from …`
    /// line is the same run of tokens as the removed `import Window from
    /// shine`, and the parser cannot see the line break between them, so
    /// guessing would flag the import on the line above whatever is being
    /// typed. Read as written, the old single-name form still fails where
    /// its `from` has no `import`, and that error says what to write.
    fn at_old_import_list(&self) -> bool {
        let mut i = 0;
        let mut names = 0;
        loop {
            if !matches!(self.peek_at(i).value, Token::Identifier(_)) {
                return false;
            }
            names += 1;
            let mut next = i + 1;
            if matches!(self.peek_at(next).value, Token::As) {
                if !matches!(self.peek_at(next + 1).value, Token::Identifier(_)) {
                    return false;
                }
                next += 2;
            }
            match self.peek_at(next).value {
                Token::Comma => i = next + 1,
                Token::From => return names > 1,
                _ => return false,
            }
        }
    }

    /// `from path import *` or `from path import A, B as C`, after an
    /// optional `export` the caller has already consumed.
    pub(crate) fn parse_from_import(
        &mut self,
        exported: bool,
    ) -> Result<Spanned<Decl>, ParseError> {
        let kw = self.advance(); // `from`
        let (path, quoted, mut end) = self.parse_module_path()?;
        if !self.eat(&Token::Import) {
            // Anchored on the `from` line rather than on the next token,
            // which is usually on another line, often far below.
            let err = ParseError::Expected {
                expected: "`import` after the module".into(),
                found: self.peek().value.describe(),
                help: Some(IMPORT_FORMS.to_string()),
                span: kw.span.start..end,
                label: "here".into(),
            };
            if !self.recovering() {
                return Err(err);
            }
            // `from shine ` still being typed: keep the declaration so
            // completion knows the module.
            self.record(err);
            return Ok(Spanned::new(
                Decl::Import {
                    names: ImportNames::List(Vec::new()),
                    path,
                    quoted,
                    exported,
                },
                kw.span.start..end,
            ));
        }
        end = self.last_consumed_end();
        let names = if self.eat(&Token::Star) {
            end = self.last_consumed_end();
            ImportNames::All
        } else {
            let mut list = Vec::new();
            loop {
                if !matches!(self.peek().value, Token::Identifier(_)) {
                    // `from shine import ` with the names not typed yet.
                    let err = self.expected_here("a name to import, or `*`");
                    if !self.recovering() {
                        return Err(err);
                    }
                    self.record(err);
                    break;
                }
                let (n, span) = self.expect_ident_recover("imported name")?;
                end = span.end;
                let alias = if self.eat(&Token::As) {
                    let (a, span) = self.expect_ident_recover("a name after `as`")?;
                    end = span.end;
                    Some(a)
                } else {
                    None
                };
                list.push((n, alias));
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            ImportNames::List(list)
        };
        Ok(Spanned::new(
            Decl::Import {
                names,
                path,
                quoted,
                exported,
            },
            kw.span.start..end.max(kw.span.end),
        ))
    }

    /// A quoted literal or a bare dotted chain (`some.folder.module`); both
    /// give the same path string. Returns it, whether it was quoted, and
    /// where it ends.
    fn parse_module_path(&mut self) -> Result<(String, bool, usize), ParseError> {
        let tok = self.peek().clone();
        match tok.value {
            Token::String(s) => {
                self.advance();
                Ok((s, true, tok.span.end))
            }
            Token::Identifier(_) => {
                let (first, first_span) = self.expect_ident_recover("module name")?;
                let mut parts = vec![first];
                let mut end = first_span.end;
                // Only extend on `.`; no statement can start with one, so
                // this can't swallow the following line.
                while self.eat(&Token::Dot) {
                    let (seg, seg_span) = self.expect_ident_recover("path segment after `.`")?;
                    parts.push(seg);
                    end = seg_span.end;
                }
                Ok((parts.join("."), false, end))
            }
            _ => {
                let err = ParseError::expected(
                    "a module path, quoted or bare (e.g. \"a/b\" or a.b)",
                    &tok,
                );
                if !self.recovering() {
                    return Err(err);
                }
                // A module still being typed. Keeping the declaration with an
                // empty path is what lets completion offer the modules that
                // could go there.
                self.record(err);
                Ok((String::new(), false, self.last_consumed_end()))
            }
        }
    }
}
