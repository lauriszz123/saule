//! Node numbering, checked against real source rather than hand-built trees.
//!
//! The invariant these pin down is the one downstream side tables depend on
//! (`saule_typeck::TypeTable`, `saule_semantic::ResolveTable`): every node a
//! parse produces has an id, no two nodes share one, and re-parsing the same
//! text produces the same numbering.

use std::collections::HashSet;

use saule_ast::*;
use saule_lexer::Lexer;
use saule_parser::parse;

fn parse_src(src: &str) -> Module {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    parse(tokens).expect("parse ok")
}

/// Every id reachable in the tree, collected the long way so this test does
/// not simply re-run the walker it is checking.
fn collect(m: &Module) -> Vec<NodeId> {
    let mut ids = Vec::new();
    fn stmts(ss: &[Spanned<Stmt>], out: &mut Vec<NodeId>) {
        for s in ss {
            out.push(s.id);
            match &s.value {
                Stmt::Local { value, .. } => {
                    if let Some(e) = value {
                        expr(e, out)
                    }
                }
                Stmt::LocalMulti { values, .. } | Stmt::Return(values) => {
                    values.iter().for_each(|e| expr(e, out))
                }
                Stmt::Assign { target, value } | Stmt::CompoundAssign { target, value, .. } => {
                    expr(target, out);
                    expr(value, out);
                }
                Stmt::AssignMulti { targets, values } => {
                    targets.iter().for_each(|e| expr(e, out));
                    values.iter().for_each(|e| expr(e, out));
                }
                Stmt::Expr(e) | Stmt::Throw(e) => expr(e, out),
                Stmt::If {
                    cond,
                    then_block,
                    elseifs,
                    else_block,
                } => {
                    expr(cond, out);
                    stmts(then_block, out);
                    for (c, b) in elseifs {
                        expr(c, out);
                        stmts(b, out);
                    }
                    if let Some(b) = else_block {
                        stmts(b, out);
                    }
                }
                Stmt::While { cond, body } => {
                    expr(cond, out);
                    stmts(body, out);
                }
                Stmt::Repeat { body, cond } => {
                    stmts(body, out);
                    expr(cond, out);
                }
                Stmt::ForNumeric {
                    from,
                    to,
                    step,
                    body,
                    ..
                } => {
                    expr(from, out);
                    expr(to, out);
                    if let Some(e) = step {
                        expr(e, out)
                    }
                    stmts(body, out);
                }
                Stmt::ForIn { iter, body, .. } => {
                    expr(iter, out);
                    stmts(body, out);
                }
                Stmt::Try {
                    body, catch_body, ..
                } => {
                    stmts(body, out);
                    stmts(catch_body, out);
                }
                Stmt::Decl(d) => decl(d, out),
                Stmt::Break | Stmt::Continue | Stmt::Error => {}
            }
        }
    }

    fn decl(d: &Spanned<Decl>, out: &mut Vec<NodeId>) {
        out.push(d.id);
        match &d.value {
            Decl::Function { params, body, .. } => {
                params.iter().for_each(|p| {
                    if let Some(e) = &p.default {
                        expr(e, out)
                    }
                });
                stmts(body, out);
            }
            Decl::Class { members, .. } => {
                for m in members {
                    out.push(m.id);
                    match &m.value {
                        ClassMember::Field { default, .. } => {
                            if let Some(e) = default {
                                expr(e, out)
                            }
                        }
                        ClassMember::Method(me) => {
                            me.params.iter().for_each(|p| {
                                if let Some(e) = &p.default {
                                    expr(e, out)
                                }
                            });
                            stmts(&me.body, out);
                        }
                    }
                }
            }
            Decl::Enum {
                variants, methods, ..
            } => {
                for v in variants {
                    out.push(v.id);
                    match &v.value {
                        EnumVariant::Valued(_, e) => expr(e, out),
                        EnumVariant::Tuple { fields, .. } => fields.iter().for_each(|p| {
                            if let Some(e) = &p.default {
                                expr(e, out)
                            }
                        }),
                        EnumVariant::Bare(_) => {}
                    }
                }
                for m in methods {
                    m.params.iter().for_each(|p| {
                        if let Some(e) = &p.default {
                            expr(e, out)
                        }
                    });
                    stmts(&m.body, out);
                }
            }
            Decl::Variable { value, .. } => {
                if let Some(e) = value {
                    expr(e, out)
                }
            }
            Decl::Interface { .. } | Decl::Import { .. } => {}
        }
    }

    fn expr(e: &Spanned<Expr>, out: &mut Vec<NodeId>) {
        out.push(e.id);
        match &e.value {
            Expr::Unary { rhs, .. } => expr(rhs, out),
            Expr::Binary { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => expr(obj, out),
            Expr::Index { obj, index } => {
                expr(obj, out);
                expr(index, out);
            }
            Expr::Call {
                callee,
                args: call_args,
            } => {
                expr(callee, out);
                args(call_args, out);
            }
            Expr::ForceUnwrap(i) => expr(i, out),
            Expr::Cast { value, .. } => expr(value, out),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => expr(v, out),
                        TableEntry::Field { key, value } => {
                            expr(key, out);
                            expr(value, out);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                params.iter().for_each(|p| {
                    if let Some(d) = &p.default {
                        expr(d, out)
                    }
                });
                match body {
                    LambdaBody::Expr(b) => expr(b, out),
                    LambdaBody::Block(b) => stmts(b, out),
                }
            }
            Expr::Match { scrutinee, arms } => {
                expr(scrutinee, out);
                for arm in arms {
                    pattern(&arm.pattern, out);
                    if let Some(g) = &arm.guard {
                        expr(g, out)
                    }
                    match &arm.body {
                        MatchBody::Expr(b) => expr(b, out),
                        MatchBody::Block(b) => stmts(b, out),
                    }
                }
            }
            Expr::Pipe { source, stages } => {
                expr(source, out);
                for st in stages {
                    args(&st.args, out);
                }
            }
            _ => {}
        }
    }

    fn args(a: &[CallArg], out: &mut Vec<NodeId>) {
        for arg in a {
            match arg {
                CallArg::Positional(e) => expr(e, out),
                CallArg::Named { value, .. } => expr(value, out),
            }
        }
    }

    fn pattern(p: &Spanned<Pattern>, out: &mut Vec<NodeId>) {
        out.push(p.id);
        match &p.value {
            Pattern::Variant { fields, .. } | Pattern::Tuple(fields) => {
                fields.iter().for_each(|f| pattern(f, out))
            }
            _ => {}
        }
    }

    stmts(&m.stmts, &mut ids);
    ids
}

/// A program touching most of the node kinds that exist.
const SOURCE: &str = r#"
enum Status
  Ok
  Failed = "failed"
  Code(n: integer)
end

class Player
  health: integer = 100
  static count: integer = 0

  fn damage(amount: integer, critical: boolean = false) -> nil
    self.health = self.health - (critical and amount * 2 or amount)
  end
end

fn describe(s: Status) -> string
  return match s
    case Status.Ok then "ok"
    case Status.Code(n) when n > 0 then "code"
    case _ then "other"
  end
end

fn main() -> nil
  local p = Player()
  local list = {1, 2, 3, name: "xs"}
  for i = 1, #list do
    p.damage(list[i])
  end
  for k, v in list do
    print(k, v)
  end
  local f = fn(x: integer) -> integer
    return x + 1
  end
  local g = (x: integer) => x * 2
  while p.health > 0 do
    p.damage(1)
    if p.health < 10 then break end
  end
  try
    throw "boom"
  catch e: string
    print(e)
  end
  print(describe(Status.Ok), f(1), g(2))
end
"#;

#[test]
fn every_node_is_numbered_and_unique() {
    let m = parse_src(SOURCE);
    let ids = collect(&m);

    assert!(ids.len() > 80, "test source got smaller: only {} nodes", ids.len());
    assert!(
        !ids.iter().any(|i| i.is_none()),
        "{} node(s) were left at NodeId::NONE",
        ids.iter().filter(|i| i.is_none()).count()
    );

    let unique: HashSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "ids collide");
}

#[test]
fn numbering_is_deterministic_across_parses() {
    assert_eq!(collect(&parse_src(SOURCE)), collect(&parse_src(SOURCE)));
}

#[test]
fn ids_are_dense_from_zero() {
    // A dense range is what lets a side table be a `Vec` rather than a
    // `HashMap`, so it is worth pinning rather than leaving incidental.
    let m = parse_src(SOURCE);
    let mut ids: Vec<u32> = collect(&m).into_iter().map(|i| i.0).collect();
    ids.sort_unstable();
    assert_eq!(ids.first(), Some(&0));
    assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<_>>());
}

#[test]
fn a_recovered_parse_is_still_numbered() {
    let tokens = Lexer::new("local x = ").tokenize().expect("lex ok");
    let parsed = saule_parser::parse_recover(tokens, "local x = ");
    assert!(!parsed.module.stmts.is_empty());
    assert!(collect(&parsed.module).iter().all(|i| !i.is_none()));
}
