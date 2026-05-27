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
        else if b then
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
    let m = parse_src("for v: Player in queue do v:greet() end");
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
            fn greet(self): nil
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
