#!/usr/bin/env python3
"""perf-207 benchmark runner: times the 4 named recipes cases.

Usage (from the repo root):
    python3 scripts/bench/perf-207/run.py --binary rust/target/release/clojure-rs \
        --scripts /path/to/recipes-clojure/src --resources /path/to/shared [--cases ...]

Runs each case's script once per shared test case (stdin in, stdout out),
checks stdout against the snip-expected output (trailing whitespace per
line ignored, like `snip test`), and prints per-case wall-clock seconds.
The recipes repo and resources are read-only inputs; nothing is modified.

Exits nonzero on any output mismatch or runner failure.
"""
import argparse
import subprocess
import sys
import time
from pathlib import Path

# (script relative to --scripts, resources relative to --resources, label)
CASES = [
    ("sort/bubble_sort/solution.clj", "sort/bubble_sort/solution", "sort"),
    (
        "string_related/suffix_array/string_function_calculation.clj",
        "string_related/suffix_array/string_function_calculation",
        "suffix-array",
    ),
    (
        "data_structure/disjoint_set/connected_component.clj",
        "data_structure/disjoint_set/connected_component",
        "disjoint-set",
    ),
    (
        "graph/shortest_path/dijkstrashortreach.clj",
        "graph/shortest_path/dijkstrashortreach",
        "dijkstra",
    ),
]


def norm(data: bytes) -> list[str]:
    return [line.rstrip() for line in data.decode("utf-8", "replace").splitlines()]


def run_one(binary: Path, script: Path, case: Path, timeout: float) -> tuple[bool, float, str]:
    inp = (case / "input.txt").read_bytes()
    t0 = time.perf_counter()
    try:
        p = subprocess.run(
            [str(binary), "run", str(script)],
            input=inp,
            capture_output=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return False, timeout, "TIMEOUT"
    dt = time.perf_counter() - t0
    exp = norm((case / "output.txt").read_bytes())
    got = norm(p.stdout)
    if p.returncode != 0:
        return False, dt, f"rc={p.returncode} {p.stderr[:200]!r}"
    if got != exp:
        return False, dt, "OUTPUT-DIFF"
    return True, dt, "OK"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary", required=True, type=Path)
    ap.add_argument("--scripts", required=True, type=Path)
    ap.add_argument("--resources", required=True, type=Path)
    ap.add_argument("--timeout", type=float, default=3600.0)
    ap.add_argument("--cases", nargs="*", default=[c[2] for c in CASES])
    args = ap.parse_args()
    failed = False
    for script_rel, res_rel, label in CASES:
        if label not in args.cases:
            continue
        script = args.scripts / script_rel
        resdir = args.resources / res_rel
        cases = sorted(resdir.glob("test_case_*"))
        print(f"== {label}: {script_rel} ({len(cases)} cases)", flush=True)
        for case in cases:
            ok, dt, msg = run_one(args.binary, script, case, args.timeout)
            print(f"{label}/{case.name} {msg} {dt:.2f}s", flush=True)
            failed = failed or not ok
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
