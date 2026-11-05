#!/usr/bin/env python3
"""Head-to-head gate: reference leg vs challenger leg on cpu_s + maxrss_kb.

Usage:
  h2h_compare.py --jvm JVM.json [JVM.json ...] --rs RS.json [RS.json ...]
                 --label SUITE [--summary-file FILE] [--report-only]
                 [--merged-jvm FILE] [--merged-rs FILE]
                 [--base-name NAME] [--challenger-name NAME]

--jvm takes the reference leg, --rs the challenger leg (a "win" is
challenger-faster). The JVM-vs-native gate uses the defaults; the
native-vs-native comparison passes two clojure-rs legs with
--base-name/--challenger-name (e.g. rs-from/rs-to).

Compares per-case medians (primary) and cross-checks the README
convention (cpu min-of-repeats, rss max-of-repeats). Multiple JSONs
per leg are unioned (one per measure shard); duplicate case keys
across shards are a hard error, except identical run-less
sweep-skip markers, which every shard records by design.

Strict gate: exits non-zero on any CPU/RSS loss, failure, missing
case, not-ok case, behavioral drift, or leg-coverage mismatch,
unless --report-only (triage mode: always exits 0).

Stdlib only (python3 on the GitHub runner).
"""
import argparse
import json
import sys

# Namespaces whose :pass count varies run to run by design (they
# assert per edge of a random graph); :test/:fail/:error still
# compare exactly. Mirrors the report-compare job of the
# clojure-algorithms perf-callgrind workflow.
VARY_PASS_LABELS = frozenset([
    "clojure-algorithms.graphs.karger-test",
    "clojure-algorithms.graphs.random-graph-generator-test",
])


def load(path):
    with open(path) as f:
        return json.load(f)


def key(r):
    return (r["label"], r.get("stem", ""), r.get("case", ""))


def _is_skip_marker(r):
    """True for a run-less sweep-skip marker (no timed runs at all)."""
    if r.get("runs"):
        return False
    summary = r.get("summary") or {}
    return not summary.get("cpu_s")


def union(paths, leg):
    """Union shard reports into (results-by-key, failures, missing, files)."""
    merged = {}
    failures = set()
    missing = set()
    for path in paths:
        doc = load(path)
        for r in doc["results"]:
            k = key(r)
            if k in merged:
                # Sweep-skipped namespaces (e.g. algos sweep_skips.json)
                # are recorded run-less in EVERY shard by design; dedupe
                # identical markers, keep the hard error for real dupes.
                prev = merged[k]
                if _is_skip_marker(prev) and _is_skip_marker(r) and prev == r:
                    continue
                raise ValueError(
                    f"duplicate case {k[0]}/{k[1]}/{k[2]} across {leg} shards "
                    f"(shards must partition cases; check --shard flags)")
            merged[k] = r
        failures.update(doc.get("failures", []))
        missing.update(doc.get("missing", []))
    return merged, sorted(failures), sorted(missing)


def norm_counts(label, counts):
    if counts is None:
        return None
    counts = dict(counts)
    if label in VARY_PASS_LABELS:
        counts["pass"] = 0
    return counts


def parse_args(argv):
    p = argparse.ArgumentParser(description="Reference-vs-challenger CPU/RSS gate")
    p.add_argument("--jvm", nargs="+", required=True, metavar="BASE.json",
                   help="reference leg: one merged JSON, or one JSON per shard")
    p.add_argument("--rs", nargs="+", required=True, metavar="CHALLENGER.json",
                   help="challenger leg: one merged JSON, or one JSON per shard")
    p.add_argument("--base-name", default="jvm",
                   help="display name of the reference leg (default: jvm)")
    p.add_argument("--challenger-name", default="rs",
                   help="display name of the challenger leg (default: rs)")
    p.add_argument("--label", required=True, help="suite name (recipes, algos)")
    p.add_argument("--summary-file", default=None,
                   help="append a Markdown verdict table (GITHUB_STEP_SUMMARY)")
    p.add_argument("--report-only", action="store_true",
                   help="triage mode: print the verdict but always exit 0")
    p.add_argument("--merged-jvm", default=None,
                   help="write the unioned JVM leg JSON here")
    p.add_argument("--merged-rs", default=None,
                   help="write the unioned clojure-rs leg JSON here")
    return p.parse_args(argv)


def short(k):
    return f"{k[0]}/{k[2]}" if k[2] != "run" else k[0]


def write_merged(path, first_doc, results, failures, missing):
    doc = dict(first_doc)
    doc["results"] = [results[k] for k in sorted(results)]
    doc["failures"] = failures
    doc["missing"] = missing
    config = dict(doc.get("config", {}))
    config["shard"] = "merged"
    doc["config"] = config
    with open(path, "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")


def main(argv):
    args = parse_args(argv)
    try:
        jm, j_fail, j_miss = union(args.jvm, args.base_name)
        rm, r_fail, r_miss = union(args.rs, args.challenger_name)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as e:
        print(f"h2h_compare: error loading leg JSONs: {e}", file=sys.stderr)
        return 2
    if args.merged_jvm:
        write_merged(args.merged_jvm, load(args.jvm[0]), jm, j_fail, j_miss)
    if args.merged_rs:
        write_merged(args.merged_rs, load(args.rs[0]), rm, r_fail, r_miss)

    an, bn = args.base_name, args.challenger_name
    print(f"== {args.label} ==")
    print(f"{an}: shards={len(args.jvm)} results={len(jm)} "
          f"failures={len(j_fail)} missing={len(j_miss)}")
    print(f"{bn}: shards={len(args.rs)} results={len(rm)} "
          f"failures={len(r_fail)} missing={len(r_miss)}")
    only_jvm = sorted(set(jm) - set(rm))
    only_rs = sorted(set(rm) - set(jm))
    print(f"common cases: {len(set(jm) & set(rm))} "
          f"only-{an}: {len(only_jvm)} only-{bn}: {len(only_rs)}")
    for k in only_jvm[:10]:
        print(f"  only-{an}:", "/".join(k))
    for k in only_rs[:10]:
        print(f"  only-{bn}:", "/".join(k))

    n_cpu_win = n_cpu_loss = n_cpu_tie = 0
    n_mem_win = n_mem_loss = n_mem_tie = 0
    n_both_win = 0
    cpu_losses, mem_losses = [], []
    not_ok, drift, skipped = [], [], []
    worst_cpu_margin, worst_mem_margin = None, None
    for k in sorted(set(jm) & set(rm)):
        a, b = jm[k], rm[k]
        name = short(k)
        if not a["ok"] or not b["ok"]:
            not_ok.append((name, a.get("error"), b.get("error")))
            continue
        if _is_skip_marker(a) or _is_skip_marker(b):
            skipped.append((name, len(a.get("runs", [])),
                            len(b.get("runs", []))))
            continue
        # behavioral drift gate (algos carry test_counts)
        if "test_counts" in a or "test_counts" in b:
            if norm_counts(k[0], a.get("test_counts")) != \
               norm_counts(k[0], b.get("test_counts")):
                drift.append((name, a.get("test_counts"), b.get("test_counts")))
        jc, rc = a["summary"]["cpu_s"], b["summary"]["cpu_s"]
        jr, rr = a["summary"]["maxrss_kb"], b["summary"]["maxrss_kb"]
        cpu_ratio = rc["median"] / jc["median"] if jc["median"] else float("inf")
        mem_ratio = rr["median"] / jr["median"] if jr["median"] else float("inf")
        if rc["median"] < jc["median"]:
            n_cpu_win += 1
        elif rc["median"] > jc["median"]:
            n_cpu_loss += 1
            cpu_losses.append((cpu_ratio, name, jc["median"], rc["median"]))
        else:
            n_cpu_tie += 1
        if rr["median"] < jr["median"]:
            n_mem_win += 1
        elif rr["median"] > jr["median"]:
            n_mem_loss += 1
            mem_losses.append((mem_ratio, name, jr["median"], rr["median"]))
        else:
            n_mem_tie += 1
        if rc["median"] < jc["median"] and rr["median"] < jr["median"]:
            n_both_win += 1
        # README-convention cross-check
        rc_readme = rc["min"] < jc["min"]
        rm_readme = rr["max"] < jr["max"]
        if rc_readme != (rc["median"] < jc["median"]) or \
           rm_readme != (rr["median"] < jr["median"]):
            print(f"  NOTE {name}: median verdict differs from README convention "
                  f"(cpu min {rc['min']:.3f} vs {jc['min']:.3f}; "
                  f"rss max {rr['max']} vs {jr['max']})")
        if worst_cpu_margin is None or cpu_ratio > worst_cpu_margin[0]:
            worst_cpu_margin = (cpu_ratio, name)
        if worst_mem_margin is None or mem_ratio > worst_mem_margin[0]:
            worst_mem_margin = (mem_ratio, name)

    n = n_cpu_win + n_cpu_loss + n_cpu_tie
    print(f"compared ok-cases: {n}")
    print(f"CPU (median): {bn} wins {n_cpu_win}, losses {n_cpu_loss}, ties {n_cpu_tie}")
    print(f"MEM (median): {bn} wins {n_mem_win}, losses {n_mem_loss}, ties {n_mem_tie}")
    print(f"both-win cases: {n_both_win}/{n}")
    if worst_cpu_margin is not None:
        print(f"worst {bn} cpu ratio: {worst_cpu_margin[0]:.3f}x ({worst_cpu_margin[1]})")
        print(f"worst {bn} mem ratio: {worst_mem_margin[0]:.3f}x ({worst_mem_margin[1]})")
    else:
        print(f"worst {bn} cpu ratio: n/a (no compared cases)")
        print(f"worst {bn} mem ratio: n/a (no compared cases)")
    if cpu_losses:
        print(f"--- CPU LOSSES ({len(cpu_losses)}) ---")
        for ratio, name, j, r in sorted(cpu_losses, reverse=True)[:50]:
            print(f"  {ratio:.3f}x {name} {an}={j:.3f}s {bn}={r:.3f}s")
    if mem_losses:
        print(f"--- MEM LOSSES ({len(mem_losses)}) ---")
        for ratio, name, j, r in sorted(mem_losses, reverse=True)[:50]:
            print(f"  {ratio:.3f}x {name} {an}={j:.0f}K {bn}={r:.0f}K")
    if skipped:
        print(f"--- SKIPPED-NO-RUNS ({len(skipped)}) ---")
        for name, aj, bj in skipped[:20]:
            print(f"  {name} {an}_runs={aj} {bn}_runs={bj}")
    if not_ok:
        print(f"--- NOT-OK ({len(not_ok)}) ---")
        for name, je, re in not_ok[:50]:
            print(f"  {name} {an}_err={je} {bn}_err={re}")
    if drift:
        print(f"--- TEST-COUNT DRIFT ({len(drift)}) ---")
        for name, j, r in drift[:20]:
            print(f"  {name} jvm={j} rs={r}")
    if j_fail:
        print(f"--- {an.upper()} failures ({len(j_fail)}) ---")
        for f in j_fail[:20]:
            print(" ", f)
    if r_fail:
        print(f"--- {bn.upper()} failures ({len(r_fail)}) ---")
        for f in r_fail[:20]:
            print(" ", f)
    if j_miss:
        print(f"--- {an.upper()} missing ({len(j_miss)}) ---")
        for f in j_miss[:20]:
            print(" ", f)
    if r_miss:
        print(f"--- {bn.upper()} missing ({len(r_miss)}) ---")
        for f in r_miss[:20]:
            print(" ", f)

    problems = []
    if n_cpu_loss:
        problems.append(f"{n_cpu_loss} CPU losses")
    if n_mem_loss:
        problems.append(f"{n_mem_loss} MEM losses")
    if not_ok:
        problems.append(f"{len(not_ok)} not-ok cases")
    if drift:
        problems.append(f"{len(drift)} test-count drifts")
    if only_jvm or only_rs:
        problems.append(f"leg mismatch (only-{an} {len(only_jvm)}, "
                        f"only-{bn} {len(only_rs)})")
    if j_fail or r_fail:
        problems.append(f"sweep failures ({an} {len(j_fail)}, {bn} {len(r_fail)})")
    if j_miss or r_miss:
        problems.append(f"missing cases ({an} {len(j_miss)}, {bn} {len(r_miss)})")
    if n == 0:
        problems.append("no compared cases")

    gate_pass = not problems
    if args.report_only:
        verdict = "PASS (report-only)" if gate_pass else \
            f"FAIL ignored (--report-only): {'; '.join(problems)}"
    else:
        verdict = "PASS" if gate_pass else f"FAIL: {'; '.join(problems)}"
    print(f"GATE {args.label}: {verdict}")

    if args.summary_file:
        worst_cpu = ("n/a" if worst_cpu_margin is None else
                     f"{worst_cpu_margin[0]:.3f}x ({worst_cpu_margin[1]})")
        worst_mem = ("n/a" if worst_mem_margin is None else
                     f"{worst_mem_margin[0]:.3f}x ({worst_mem_margin[1]})")
        lines = [
            f"## h2h {args.label}: {an} vs {bn}",
            "",
            f"common cases: {n} ({an} shards {len(args.jvm)}, "
            f"{bn} shards {len(args.rs)}); both-win {n_both_win}/{n}",
            "",
            f"| metric | {bn} wins | losses | ties |",
            "| --- | ---: | ---: | ---: |",
            f"| CPU (median cpu_s) | {n_cpu_win} | {n_cpu_loss} | {n_cpu_tie} |",
            f"| MEM (median maxrss_kb) | {n_mem_win} | {n_mem_loss} | {n_mem_tie} |",
            "",
            f"worst {bn} cpu ratio: {worst_cpu}; worst {bn} mem ratio: {worst_mem}",
            "",
            f"### GATE: {'PASS' if gate_pass else 'FAIL'}",
        ]
        if problems:
            lines.append("")
            lines.append("; ".join(problems))
        if cpu_losses:
            lines += ["", "<details><summary>CPU losses "
                      f"({len(cpu_losses)}, worst first)</summary>", "",
                      f"| ratio | case | {an} | {bn} |",
                      "| ---: | --- | ---: | ---: |"]
            for ratio, name, j, r in sorted(cpu_losses, reverse=True)[:30]:
                lines.append(f"| {ratio:.3f}x | {name} | {j:.3f}s | {r:.3f}s |")
            lines.append("</details>")
        if mem_losses:
            lines += ["", "<details><summary>MEM losses "
                      f"({len(mem_losses)}, worst first)</summary>", "",
                      f"| ratio | case | {an} | {bn} |",
                      "| ---: | --- | ---: | ---: |"]
            for ratio, name, j, r in sorted(mem_losses, reverse=True)[:30]:
                lines.append(f"| {ratio:.3f}x | {name} | {j:.0f}K | {r:.0f}K |")
            lines.append("</details>")
        with open(args.summary_file, "a") as f:
            f.write("\n".join(lines) + "\n")

    if args.report_only:
        return 0
    return 0 if gate_pass else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
