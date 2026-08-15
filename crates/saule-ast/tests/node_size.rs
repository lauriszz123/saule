//! `Spanned` is the most replicated struct in the compiler: every expression,
//! statement and declaration is one, and the tree-walker walks them in
//! sequence on every evaluation, so its size is a cache-footprint knob.
//!
//! Adding `NodeId` grew `Spanned<Expr>` from 88 bytes to 96 — four bytes of
//! id plus four of padding, because `Range<usize>` forces 8-byte alignment.
//! That is the one hot-path structural cost Phase 0 imposed, and this test
//! exists so a future change cannot add another without someone noticing.
use saule_ast::{Expr, Spanned, Stmt};

#[test]
fn spanned_stays_small() {
    println!("Expr          = {}", std::mem::size_of::<Expr>());
    println!("Spanned<Expr> = {}", std::mem::size_of::<Spanned<Expr>>());
    println!("Stmt          = {}", std::mem::size_of::<Stmt>());
    println!("Spanned<Stmt> = {}", std::mem::size_of::<Spanned<Stmt>>());

    assert!(
        std::mem::size_of::<Spanned<Expr>>() <= 96,
        "Spanned<Expr> grew past 96 bytes — every expression node in every \
         program pays for this"
    );
}
