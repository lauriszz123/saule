#!/usr/bin/env python3
"""Time Saule against Lua on the programs in `sau/` and `lua/`.

    ./benchmarks/bench.py                          # release build vs lua
    ./benchmarks/bench.py new=./target/release/saule old=/tmp/saule-before

Every `sau/NAME.sau` has a line-for-line `lua/NAME.lua` twin that computes the
same answer, so a difference in timing is a difference in the runtimes rather
than in the programs. `check` verifies that: it runs both and compares stdout.

Reading the output: **compare ratios, not seconds.** Wall-clock times drift by
20-40% between runs on a laptop as clocks and core assignment change, but
every engine in one run sees the same conditions, which is why each rep runs
all of them back to back and the reported figure is the minimum across reps.
"""
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
BENCHES = (
    # Microbenchmarks: one language mechanism each.
    "loop_arith fib array map oop mandel strings closure sort "
    # Whole programs, in the shape everyday code actually takes.
    "bintree matrix entity wordfreq json interp "
    # Control.
    "startup"
).split()
REPS = int(os.environ.get("REPS", "7"))


def engines(argv):
    """Parse `name=path` pairs; default to the workspace release build."""
    out = [(n, [p, "run"], "sau", ".sau") for n, p in (a.split("=", 1) for a in argv if "=" in a)]
    if not out:
        out = [("saule", [os.path.join(ROOT, "target/release/saule"), "run"], "sau", ".sau")]
    for lua in ("lua", "luajit"):
        if subprocess.run(["which", lua], capture_output=True).returncode == 0:
            out.append((lua, [lua], "lua", ".lua"))
    return out


def path_for(bench, directory, ext):
    return os.path.join(HERE, directory, bench + ext)


def timed(argv):
    start = time.perf_counter()
    done = subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter() - start if done.returncode == 0 else float("nan")


def check(engs):
    """Confirm Saule and Lua 5.4+ agree on every benchmark's output.

    LuaJIT is timed but deliberately not checked: it implements Lua 5.1, which
    has no integer type, so it prints `3000000` where the others print
    `3000000.0` and wraps the `sort` benchmark's 64-bit LCG through a double.
    Those are language-version differences, not benchmark bugs.
    """
    checked = [e for e in engs if e[0] != "luajit"]
    ok = True
    for bench in BENCHES:
        outputs = {}
        for name, prefix, d, ext in checked:
            done = subprocess.run(prefix + [path_for(bench, d, ext)], capture_output=True, text=True)
            outputs[name] = done.stdout.strip()
        agree = len(set(outputs.values())) == 1
        ok = ok and agree
        print(f"{bench:<12} {'ok' if agree else 'MISMATCH':<10} {outputs}")
    return ok


def main():
    argv = [a for a in sys.argv[1:] if a != "check"]
    engs = engines(argv)
    if "check" in sys.argv[1:]:
        sys.exit(0 if check(engs) else 1)

    best = {b: {n: float("inf") for n, *_ in engs} for b in BENCHES}
    for bench in BENCHES:
        for _ in range(REPS):
            for name, prefix, d, ext in engs:
                t = timed(prefix + [path_for(bench, d, ext)])
                if t == t:  # not nan
                    best[bench][name] = min(best[bench][name], t)

    names = [n for n, *_ in engs]
    ref = names[0]
    header = f"{'bench':<12}" + "".join(f"{n:>10}" for n in names)
    if "lua" in names:
        header += f"{ref + '/lua':>12}"
    print(header)
    print("-" * len(header))
    for bench in BENCHES:
        row = f"{bench:<12}" + "".join(f"{best[bench][n]:>10.3f}" for n in names)
        if "lua" in names and best[bench]["lua"]:
            row += f"{best[bench][ref] / best[bench]['lua']:>11.1f}x"
        print(row)


if __name__ == "__main__":
    main()
