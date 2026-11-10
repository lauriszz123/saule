#!/usr/bin/env python3
"""Time Saule against Lua on the programs in `sau/` and `lua/`.

    ./benchmarks/bench.py                          # release build vs lua
    ./benchmarks/bench.py check                    # outputs still agree?
    ./benchmarks/bench.py new=./target/release/saule old=/tmp/saule-before
    ONLY=coroutine,fib ./benchmarks/bench.py       # just these two

Every `sau/NAME.sau` has a line-for-line `lua/NAME.lua` twin that computes the
same answer, so a difference in timing is a difference in the runtimes rather
than in the programs. `check` verifies that: it runs both and compares stdout.

Reading the output: **compare ratios, not seconds.** Wall-clock times drift by
20-40% between runs on a laptop as clocks and core assignment change, but
every engine in one run sees the same conditions, which is why each rep runs
all of them back to back and the reported figure is the minimum across reps.
Each section ends in the geometric mean of its ratios, which is the average a
column of ratios has — a 4x and a 0.25x cancel to 1x.
"""
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
# Grouped only for readability: 31 rows is too many to scan flat. Nothing in
# the timing depends on the grouping — every engine still runs every
# benchmark back to back within a rep.
GROUPS = (
    ("core mechanisms", "loop_arith fib array map oop mandel strings closure sort"),
    ("language features", "coroutine coro_pipe coro_tree vecops iface generic errors nullsafe match"),
    ("standard library", "stringlib pattern tablelib iterlib mathlib bytes"),
    ("whole programs", "bintree matrix entity wordfreq json interp"),
    ("control", "startup"),
)
BENCHES = [b for _, names in GROUPS for b in names.split()]
REPS = int(os.environ.get("REPS", "7"))
# `ONLY=fib,sort` narrows the run while iterating on one hot path. Ratios
# stay comparable because every engine still runs back to back within a rep.
if os.environ.get("ONLY"):
    _want = set(os.environ["ONLY"].split(","))
    BENCHES = [b for b in BENCHES if b in _want]
    GROUPS = tuple(
        (title, " ".join(b for b in names.split() if b in _want))
        for title, names in GROUPS
    )


def engines(argv):
    """Parse `name=path` pairs; default to the workspace release build."""
    out = [(n, [p, "run"], "sau", ".sau") for n, p in (a.split("=", 1) for a in argv if "=" in a)]
    if not out:
        out = [("saule", [os.path.join(ROOT, "target/release/saule"), "run"], "sau", ".sau")]
    if subprocess.run(["which", "lua"], capture_output=True).returncode == 0:
        out.append(("lua", ["lua"], "lua", ".lua"))
    return out


def path_for(bench, directory, ext):
    return os.path.join(HERE, directory, bench + ext)


def timed(argv):
    start = time.perf_counter()
    done = subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter() - start if done.returncode == 0 else float("nan")


def check(engs):
    """Confirm Saule and Lua 5.4+ agree on every benchmark's output."""
    checked = list(engs)
    ok = True
    for title, group in GROUPS:
        members = group.split()
        if not members:
            continue
        print(f"-- {title}")
        for bench in members:
            outputs = {}
            for name, prefix, d, ext in checked:
                done = subprocess.run(prefix + [path_for(bench, d, ext)], capture_output=True, text=True)
                outputs[name] = done.stdout.strip()
            agree = len(set(outputs.values())) == 1
            ok = ok and agree
            print(f"{bench:<12} {'ok' if agree else 'MISMATCH':<10} {outputs}")
    return ok


def geomean(values):
    """The right average for ratios: a 4x and a 0.25x cancel to 1x."""
    kept = [v for v in values if v == v and v > 0]
    if not kept:
        return float("nan")
    return math.exp(sum(math.log(v) for v in kept) / len(kept))


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
    versus = "lua" in names and ref != "lua"
    header = f"{'bench':<12}" + "".join(f"{n:>10}" for n in names)
    if versus:
        header += f"{ref + '/lua':>12}"
    print(header)

    def ratio(bench):
        lua = best[bench]["lua"]
        return best[bench][ref] / lua if lua else float("nan")

    for title, group in GROUPS:
        members = group.split()
        if not members:
            continue
        print(f"-- {title} " + "-" * max(0, len(header) - len(title) - 4))
        for bench in members:
            row = f"{bench:<12}" + "".join(f"{best[bench][n]:>10.3f}" for n in names)
            if versus:
                row += f"{ratio(bench):>11.1f}x"
            print(row)
        if versus and len(members) > 1:
            # Geometric mean of the ratios, which is what summarises a column
            # of ratios; the seconds columns are deliberately left blank.
            pad = " " * (10 * len(names))
            print(f"{'  geomean':<12}{pad}{geomean([ratio(b) for b in members]):>11.1f}x")

    if versus and len(BENCHES) > 1:
        print("-" * len(header))
        pad = " " * (10 * len(names))
        print(f"{'ALL geomean':<12}{pad}{geomean([ratio(b) for b in BENCHES]):>11.1f}x")


if __name__ == "__main__":
    main()
