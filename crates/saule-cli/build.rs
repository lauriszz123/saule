//! Build script for the `saule` binary.
//!
//! Its only job is the macOS main-thread stack. Everywhere else the toolchain
//! runs on a thread we spawn ourselves with a large stack (see
//! `RUN_STACK_SIZE` in `main.rs`); on macOS it cannot, because AppKit only
//! allows window and menu work on the process's first thread, so `saule run`
//! has to execute the interpreter there. The first thread's stack is fixed at
//! exec time and cannot be grown from inside the process — the linker is the
//! only place to ask for a bigger one.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    // 512 MiB — the largest `-stack_size` ld accepts on arm64 ("must be <=
    // 512MB on arm64 platforms"), against a default of 8 MiB. Measured, that
    // carries a debug build — the pessimistic profile, its frames being the
    // largest — to roughly 13k interpreter frames, so the 10k
    // `MAX_EVAL_DEPTH` stays the binding limit and deep recursion still ends
    // in a diagnostic rather than a `SIGSEGV`. The margin is thinner than the
    // 1 GiB thread's, so it was checked against heavy frames too (six
    // arguments, nested arithmetic at every call): those still hit the guard
    // at 10k in both profiles.
    println!("cargo::rustc-link-arg-bins=-Wl,-stack_size,0x20000000");
}
