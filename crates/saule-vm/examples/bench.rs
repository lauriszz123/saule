//! In-process benchmark timer: compile once, execute N times, report the best.
//!
//! ```text
//! cargo run --release -p saule-vm --example bench -- benchmarks/sau/fib.sau [reps]
//! ```
//!
//! **Why this exists.** Timing `saule run prog.sau` from a shell measures
//! process start-up, parse and typecheck as well as the program — about 50ms
//! of it on a Windows laptop. Half the benchmarks in `benchmarks/sau` carry
//! only 10-40ms of actual work, so the figure that matters is a small
//! difference between two large numbers, and the start-up baseline itself
//! drifts a few milliseconds between runs. That turns a 3% change in the VM
//! into a 30% swing in the reported ratio: a whole optimisation session can
//! be spent chasing, or nearly keeping, noise.
//!
//! Compiling once and timing only `run_module` + `Main.main` removes the
//! baseline from the measurement rather than subtracting an estimate of it.
//! The minimum over `reps` is reported because the distribution is one-sided
//! — the fastest run is the one least interrupted by the rest of the machine.
//!
//! Programs that print are run with stdout as-is; redirect it if the output
//! is in the way. A program with side effects outside memory (file writes)
//! is not a candidate for this harness.

use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: bench <program.sau> [reps]");
        std::process::exit(2);
    };
    let reps: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(7);

    // Before `compile`, not after: the resolver classifies prelude names
    // against the stdlib this registers, and without it every program fails
    // to compile with "a name the resolver could not classify".
    saule_interpreter::init();
    let program = match saule_vm::program::compile(Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("compile failed: {e}");
            std::process::exit(1);
        }
    };

    let mut best = f64::MAX;
    for _ in 0..reps {
        // A fresh `Vm` per rep, so each one starts from the same state: the
        // module slots and statics a previous rep wrote would otherwise make
        // rep 2 a different program from rep 1.
        let mut vm = saule_vm::Vm::for_chunks(program.modules.clone());
        let t = Instant::now();
        for i in 0..=program.entry {
            if let Err(e) = vm.run_module(i) {
                eprintln!("runtime error: {e}");
                std::process::exit(1);
            }
        }
        if let Some(Err(e)) = vm.call_static("Main", "main") {
            eprintln!("runtime error: {e}");
            std::process::exit(1);
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    eprintln!("{path}\t{best:.6}");
}
