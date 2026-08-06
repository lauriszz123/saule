//! Tests moved out of src/lib.rs.
use saule_ast::*;
use saule_lexer::Lexer;
use saule_parser::parse;

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
        elseif b then
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
    let m = parse_src("for v: Player in queue do v.greet() end");
    assert!(matches!(m.stmts[0].value, Stmt::ForIn { .. }));
}

#[test]
fn parses_class_with_init_and_method() {
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
            fn greet(self)
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

#[test]
fn parses_glob_import_with_from() {
    // The explicit glob spelling stays supported.
    let m = parse_src(r#"import * from "engine""#);
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Import {
                names,
                path,
                quoted,
            } => {
                assert_eq!(names, &ImportNames::All);
                assert_eq!(path, "engine");
                assert!(quoted);
            }
            _ => panic!("expected import"),
        },
        _ => panic!("expected import"),
    }
}

#[test]
fn parses_unquoted_dotted_module_path() {
    // `from some.folder.module` — no quotes; `.` is already a path separator.
    let m = parse_src("import * from some.folder.module");
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Import {
                names,
                path,
                quoted,
            } => {
                assert_eq!(names, &ImportNames::All);
                assert_eq!(path, "some.folder.module");
                assert!(!quoted);
            }
            _ => panic!("expected import"),
        },
        _ => panic!("expected import"),
    }
}

#[test]
fn unquoted_import_does_not_swallow_next_statement() {
    // No statement can begin with `.`, so the bare path stops at `engine`
    // and the following call parses as its own statement.
    let m = parse_src("import * from engine\nGraphics.present()");
    assert_eq!(m.stmts.len(), 2);
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Import { path, quoted, .. } => {
                assert_eq!(path, "engine");
                assert!(!quoted);
            }
            _ => panic!("expected import"),
        },
        _ => panic!("expected import"),
    }
}

#[test]
fn parses_unquoted_named_import_with_alias() {
    let m = parse_src("import View as V, Button from some.folder.module");
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Import { names, path, .. } => {
                assert_eq!(
                    names,
                    &ImportNames::List(vec![
                        ("View".to_string(), Some("V".to_string())),
                        ("Button".to_string(), None),
                    ])
                );
                assert_eq!(path, "some.folder.module");
            }
            _ => panic!("expected import"),
        },
        _ => panic!("expected import"),
    }
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

/// Unwraps `stmts[0]` as an expression statement holding a call, returning its
/// argument list.
fn call_args_of(m: &Module) -> &[CallArg] {
    match &m.stmts[0].value {
        Stmt::Expr(e) => match &e.value {
            Expr::Call { args, .. } => args,
            other => panic!("expected call, got {other:?}"),
        },
        other => panic!("expected expression statement, got {other:?}"),
    }
}

#[test]
fn trailing_block_becomes_final_lambda_argument() {
    let m = parse_src("View(spacing: 10)\ndo\n    Text(\"Hello\")\nend");
    let args = call_args_of(&m);
    assert_eq!(args.len(), 2);
    assert!(matches!(args[0], CallArg::Named { .. }));
    match &args[1] {
        CallArg::Positional(e) => match &e.value {
            Expr::Lambda {
                params,
                return_ty,
                body: LambdaBody::Block(stmts),
            } => {
                assert!(params.is_empty());
                assert!(return_ty.is_none());
                assert_eq!(stmts.len(), 1);
            }
            other => panic!("expected block lambda, got {other:?}"),
        },
        other => panic!("expected positional arg, got {other:?}"),
    }
}

#[test]
fn trailing_block_is_identical_to_explicit_lambda_argument() {
    let sugar = parse_src("each(items)\ndo (item)\n    print(item)\nend");
    let desugared = parse_src("each(items, fn(item)\n    print(item)\nend)");
    // Spans differ; the shapes must not.
    let (a, b) = (call_args_of(&sugar), call_args_of(&desugared));
    assert_eq!(a.len(), b.len());
    match (&a[1], &b[1]) {
        (CallArg::Positional(x), CallArg::Positional(y)) => match (&x.value, &y.value) {
            (
                Expr::Lambda {
                    params: pa,
                    body: LambdaBody::Block(ba),
                    ..
                },
                Expr::Lambda {
                    params: pb,
                    body: LambdaBody::Block(bb),
                    ..
                },
            ) => {
                assert_eq!(pa.len(), pb.len());
                assert_eq!(pa[0].name, pb[0].name);
                assert_eq!(ba.len(), bb.len());
            }
            other => panic!("expected two block lambdas, got {other:?}"),
        },
        other => panic!("expected positional args, got {other:?}"),
    }
}

#[test]
fn trailing_block_takes_typed_params_and_return_type() {
    let m = parse_src("map(nums)\ndo (n: integer) -> integer\n    return n * 2\nend");
    match &call_args_of(&m)[1] {
        CallArg::Positional(e) => match &e.value {
            Expr::Lambda {
                params, return_ty, ..
            } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].ty, Type::Named("integer".to_string()));
                assert_eq!(
                    return_ty.as_ref(),
                    Some(&Type::Named("integer".to_string()))
                );
            }
            other => panic!("expected lambda, got {other:?}"),
        },
        other => panic!("expected positional arg, got {other:?}"),
    }
}

#[test]
fn trailing_block_attaches_to_method_calls() {
    let m = parse_src("ui.panel(title: \"x\")\ndo\n    ui.text(\"y\")\nend");
    assert_eq!(call_args_of(&m).len(), 2);
}

#[test]
fn trailing_block_chains_with_further_postfix() {
    // The result of the call is still an ordinary expression.
    let m = parse_src("build()\ndo\n    return 1\nend.value");
    match &m.stmts[0].value {
        Stmt::Expr(e) => assert!(matches!(e.value, Expr::Member { .. })),
        other => panic!("expected expression statement, got {other:?}"),
    }
}

#[test]
fn while_header_do_belongs_to_the_loop() {
    // Without the header suppression this parses as `while (queue.pop() do … end)`
    // and then fails looking for the loop's own `do`.
    let m = parse_src("while queue.pop() do\n    x = 1\nend");
    assert!(matches!(m.stmts[0].value, Stmt::While { .. }));
}

#[test]
fn for_in_header_do_belongs_to_the_loop() {
    let m = parse_src("for v in items() do\n    x = v\nend");
    assert!(matches!(m.stmts[0].value, Stmt::ForIn { .. }));
}

#[test]
fn numeric_for_header_do_belongs_to_the_loop() {
    let m = parse_src("for i = 1, count() do\n    x = i\nend");
    assert!(matches!(m.stmts[0].value, Stmt::ForNumeric { .. }));
}

#[test]
fn trailing_block_allowed_inside_a_loop_body() {
    // Suppression is scoped to the header, not the whole statement.
    let m = parse_src("while ok do\n    View()\n    do\n        Text(\"hi\")\n    end\nend");
    match &m.stmts[0].value {
        Stmt::While { body, .. } => match &body[0].value {
            Stmt::Expr(e) => match &e.value {
                Expr::Call { args, .. } => assert_eq!(args.len(), 1),
                other => panic!("expected call, got {other:?}"),
            },
            other => panic!("expected expression statement, got {other:?}"),
        },
        other => panic!("expected while, got {other:?}"),
    }
}

#[test]
fn trailing_block_allowed_inside_a_loop_header_when_parenthesised() {
    let m = parse_src("while (frame() do\n    return true\nend) do\n    x = 1\nend");
    assert!(matches!(m.stmts[0].value, Stmt::While { .. }));
}

#[test]
fn trailing_block_allowed_inside_an_argument_list_in_a_header() {
    let m = parse_src("while any(check() do\n    return true\nend) do\n    x = 1\nend");
    assert!(matches!(m.stmts[0].value, Stmt::While { .. }));
}

#[test]
fn bare_identifier_takes_no_trailing_block() {
    // Only a call can carry one, so `View do … end` is an error rather than a
    // silently different parse.
    let tokens = Lexer::new("View do\n    Text(\"hi\")\nend")
        .tokenize()
        .expect("lex ok");
    assert!(parse(tokens).is_err());
}

// ── Arrow lambdas with a declared return type ───────────────────────────────

/// Unwraps `local x = <lambda>` and returns the lambda's parts.
fn local_lambda(m: &Module) -> (&Vec<Param>, &Option<Type>) {
    match &m.stmts[0].value {
        Stmt::Local { value: Some(e), .. } => match &e.value {
            Expr::Lambda {
                params, return_ty, ..
            } => (params, return_ty),
            other => panic!("expected lambda, got {other:?}"),
        },
        other => panic!("expected local with initialiser, got {other:?}"),
    }
}

#[test]
fn parses_arrow_lambda_with_a_return_type() {
    let m = parse_src("local f = (n: integer) -> integer => n * 2");
    let (params, return_ty) = local_lambda(&m);
    assert_eq!(params.len(), 1);
    assert_eq!(
        return_ty.as_ref(),
        Some(&Type::Named("integer".to_string()))
    );
}

#[test]
fn parses_arrow_lambda_with_a_return_type_and_untyped_params() {
    let m = parse_src("local f = (n) -> integer => n + 1");
    let (params, return_ty) = local_lambda(&m);
    assert_eq!(params[0].ty, Type::Named("any".to_string()));
    assert_eq!(
        return_ty.as_ref(),
        Some(&Type::Named("integer".to_string()))
    );
}

#[test]
fn parses_arrow_lambda_with_a_generic_return_type() {
    // The return type is the full type grammar, not a single token — the
    // lookahead has to parse it to find the `=>` past the `<...>`.
    let m = parse_src("local f = (n: integer) -> table<integer> => {n}");
    let (_, return_ty) = local_lambda(&m);
    assert_eq!(
        return_ty.as_ref(),
        Some(&Type::Table {
            key: None,
            value: Box::new(Type::Named("integer".to_string())),
        })
    );
}

#[test]
fn parses_arrow_lambda_with_a_nullable_return_type() {
    let m = parse_src("local f = (n: integer) -> integer? => nil");
    let (_, return_ty) = local_lambda(&m);
    assert!(matches!(return_ty, Some(Type::Nullable(_))));
}

#[test]
fn parses_arrow_lambda_returning_a_function_type() {
    // `-> fn(integer) -> integer` contains its own `->`, so a scanner looking
    // for the first `=>` after the parens has several chances to go wrong.
    let m = parse_src("local f = (n: integer) -> fn(integer) -> integer => (m: integer) => n + m");
    let (_, return_ty) = local_lambda(&m);
    assert!(matches!(return_ty, Some(Type::Function { .. })));
}

#[test]
fn parses_zero_param_arrow_lambda_with_a_return_type() {
    let m = parse_src("local f = () -> integer => 1");
    let (params, return_ty) = local_lambda(&m);
    assert!(params.is_empty());
    assert_eq!(
        return_ty.as_ref(),
        Some(&Type::Named("integer".to_string()))
    );
}

/// The lookahead speculatively parses a parameter list; when that doesn't pan
/// out the `(` must still parse as an ordinary parenthesised expression.
#[test]
fn parenthesised_expressions_are_unaffected_by_the_lambda_lookahead() {
    for src in [
        "local x = (1 + 2) * 3",
        "local x = (twice)(4)",
        "local x = (Box(5)).get()",
        "local x = ((7))",
        "local x = (-3) + 1",
        "local x = (a.b).c",
    ] {
        let m = parse_src(src);
        match &m.stmts[0].value {
            Stmt::Local { value: Some(e), .. } => assert!(
                !matches!(e.value, Expr::Lambda { .. }),
                "{src} parsed as a lambda"
            ),
            other => panic!("expected local, got {other:?}"),
        }
    }
}

/// A parameter list that parses but isn't followed by `=>` is not a lambda.
#[test]
fn a_parenthesised_identifier_without_a_fat_arrow_is_not_a_lambda() {
    let m = parse_src("local x = (y)");
    match &m.stmts[0].value {
        Stmt::Local { value: Some(e), .. } => {
            assert!(matches!(e.value, Expr::Ident(_)), "got {:?}", e.value)
        }
        other => panic!("expected local, got {other:?}"),
    }
}

// ── Semicolon separators ────────────────────────────────────────────────────
//
// `;` is a separator, never a statement. It used to be consumed *inside*
// `parse_statement`, which meant a `;` sitting immediately before the end of a
// block committed the parser to a statement that wasn't there — every one of
// these parsed as "expected an expression".

#[test]
fn trailing_semicolon_at_end_of_file() {
    let m = parse_src("local a = 1;");
    assert_eq!(m.stmts.len(), 1);
}

#[test]
fn file_of_only_semicolons_is_empty() {
    let m = parse_src(";;;");
    assert!(m.stmts.is_empty());
}

#[test]
fn leading_and_doubled_semicolons_are_separators() {
    let m = parse_src("; local a = 1;; local b = 2");
    assert_eq!(m.stmts.len(), 2);
}

#[test]
fn semicolon_before_block_terminators() {
    // One case per terminator `parse_block_until` is asked to stop at.
    for src in [
        "if c then\n x = 1;\nend",
        "if c then\n x = 1;\nelse\n y = 2;\nend",
        "while c do\n x = 1;\nend",
        "repeat\n x = 1;\nuntil c",
        "fn f()\n x = 1;\nend",
        "try\n x = 1;\ncatch e: string\n y = 2;\nend",
    ] {
        let m = parse_src(src);
        assert_eq!(m.stmts.len(), 1, "expected one statement from: {src}");
    }
}

#[test]
fn semicolon_does_not_produce_an_empty_statement() {
    // The block is a single statement, not one statement plus a stray empty.
    let m = parse_src("fn f()\n x = 1;\nend");
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Function { body, .. } => assert_eq!(body.len(), 1),
            _ => panic!("expected a function"),
        },
        _ => panic!("expected a declaration"),
    }
}

#[test]
fn parses_exported_module_variable() {
    let m = parse_src("export appName: string = \"Saule\"");
    assert_eq!(m.stmts.len(), 1);
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Variable {
                exported,
                name,
                ty: Some(Type::Named(ty)),
                value: Some(_),
                ..
            } => {
                assert!(exported);
                assert_eq!(name, "appName");
                assert_eq!(ty, "string");
            }
            other => panic!("expected a variable decl, got {other:?}"),
        },
        other => panic!("expected a decl, got {other:?}"),
    }
}

/// The annotation and the initializer are independently optional: the
/// bare-name form is what the "never initialized" check reports against.
#[test]
fn parses_module_variable_without_initializer() {
    let m = parse_src("export pending: string?");
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Variable {
                name,
                ty: Some(Type::Nullable(_)),
                value: None,
                ..
            } => assert_eq!(name, "pending"),
            other => panic!("expected a nullable variable decl, got {other:?}"),
        },
        other => panic!("expected a decl, got {other:?}"),
    }
}

#[test]
fn parses_module_variable_with_inferred_type() {
    let m = parse_src("export retries = 3");
    match &m.stmts[0].value {
        Stmt::Decl(d) => match &d.value {
            Decl::Variable {
                name,
                ty: None,
                value: Some(_),
                ..
            } => assert_eq!(name, "retries"),
            other => panic!("expected an un-annotated variable decl, got {other:?}"),
        },
        other => panic!("expected a decl, got {other:?}"),
    }
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn parses_every_compound_assignment_operator() {
    let cases = [
        ("a += 1", BinOp::Add),
        ("a -= 1", BinOp::Sub),
        ("a *= 1", BinOp::Mul),
        ("a /= 1", BinOp::Div),
        ("a %= 1", BinOp::Mod),
        ("a ^= 1", BinOp::Pow),
        ("a ..= 1", BinOp::Concat),
    ];
    for (src, expected) in cases {
        let m = parse_src(src);
        match &m.stmts[0].value {
            Stmt::CompoundAssign { target, op, .. } => {
                assert_eq!(*op, expected, "{src}");
                assert!(matches!(&target.value, Expr::Ident(n) if n == "a"), "{src}");
            }
            other => panic!("expected compound assign for `{src}`, got {other:?}"),
        }
    }
}

#[test]
fn compound_assignment_rhs_is_a_full_expression() {
    // `x *= 3 + 4` multiplies by 7, not by 3 — the RHS runs to the end of
    // the statement rather than binding only the next primary.
    let m = parse_src("x *= 3 + 4");
    match &m.stmts[0].value {
        Stmt::CompoundAssign { op, value, .. } => {
            assert_eq!(*op, BinOp::Mul);
            assert!(matches!(&value.value, Expr::Binary { op: BinOp::Add, .. }));
        }
        other => panic!("expected compound assign, got {other:?}"),
    }
}

#[test]
fn compound_assignment_accepts_member_and_index_targets() {
    match &parse_src("obj.count += 1").stmts[0].value {
        Stmt::CompoundAssign { target, .. } => {
            assert!(matches!(&target.value, Expr::Member { name, .. } if name == "count"));
        }
        other => panic!("expected compound assign, got {other:?}"),
    }
    match &parse_src("t[i] ..= \"x\"").stmts[0].value {
        Stmt::CompoundAssign { target, op, .. } => {
            assert_eq!(*op, BinOp::Concat);
            assert!(matches!(&target.value, Expr::Index { .. }));
        }
        other => panic!("expected compound assign, got {other:?}"),
    }
}

#[test]
fn compound_assignment_span_covers_target_through_value() {
    let m = parse_src("count += 10");
    assert_eq!(m.stmts[0].span, 0..11);
}

#[test]
fn plain_assignment_is_still_plain() {
    assert!(matches!(
        &parse_src("a = 1").stmts[0].value,
        Stmt::Assign { .. }
    ));
}
