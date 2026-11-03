// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! In-process JIT execution: `run_str` over the `build_exe`
//! corpus shapes, pinning exit codes (stdout flows to the test
//! log — byte-identity vs the AOT binary is asserted by the
//! `jit` legs of the S8D corpus replay). Serialized: MCJIT-era
//! engines share LLVM global state, so parallel execution
//! tests could interfere.

use clj_aot::jit::run_str;

/// MCJIT-era engines share LLVM global state — serialize
/// execution so parallel tests cannot interfere.
static JIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// JIT-run `source` in-process, returning its exit code.
fn run(source: &str) -> i32 {
    let _serial = JIT_LOCK.lock().expect("jit serial");
    run_str(source, "f.clj", &[]).expect("jit runs")
}

/// JIT-run `source` with `args`, returning its exit code.
fn run_with(source: &str, args: &[String]) -> i32 {
    let _serial = JIT_LOCK.lock().expect("jit serial");
    run_str(source, "f.clj", args).expect("jit runs")
}

#[test]
fn jit_prints() {
    assert_eq!(run("(defn f [x] (+ x 1)) (print (f 41) \"done\")"), 0);
}

#[test]
fn jit_runs_control_flow() {
    assert_eq!(
        run(
            "(let [x 7] (if (> x 5) (print \"big\" x) (print \"small\"))) \
             (print (nth [1 2] 1))"
        ),
        0
    );
}

#[test]
fn jit_runs_loop_recur() {
    // Countdown, accumulation (the sum pins the value: anything
    // but 5050 aborts), rebinding order (the swap), and nesting.
    assert_eq!(
        run("(print (loop [i 0] (if (= i 3) i (recur (+ i 1)))))"),
        0
    );
    assert_eq!(
        run(
            "(let [s (loop [i 1 acc 0] (if (> i 100) acc (recur (+ i 1) (+ acc i))))] \
             (if (= s 5050) (print s) (quot 1 0))) \
             (print (loop [a 1 b 2] (if (= a 2) [a b] (recur b a)))) \
             (print (loop [i 0] (if (= i 2) (loop [j 10] (if (= j 12) (+ i j) (recur (+ j 1)))) (recur (+ i 1)))))"
        ),
        0
    );
}

#[test]
fn jit_loop_half_million_iterations_keeps_a_flat_stack() {
    // argv arrays hoist to the entry block: per-iteration
    // allocas would overflow the 8MB stack around ~130K
    // iterations (three boxed prims per step here), so this
    // count fails loud on the old layout and runs in ~1s.
    assert_eq!(
        run(
            "(let [s (loop [i 0 acc 0] (if (< i 500000) (recur (+ i 1) (+ acc i)) acc))] \
             (if (= s 124999750000) (print s) (quot 1 0)))"
        ),
        0
    );
}

#[test]
fn jit_survives_untaken_let_branches() {
    assert_eq!(
        run("(defn f [c] (if c (let [x 1] x) (let [y 2] y))) \
             (print (f false) (f true) (if false (let [z 3] z) 4))"),
        0
    );
}

#[test]
fn jit_flows_keywords() {
    assert_eq!(
        run("(print [(get [1 2] 5 :miss) (= :a :a) (= :a :b) :x :u/id])"),
        0
    );
}

#[test]
fn jit_runs_slice_a_forms() {
    assert_eq!(
        run("(print (cond false 1 (= 1 2) 2 :else 3)) \
             (print (and 1 2) (and 1 nil 3) (or false nil 7) (or)) \
             (print (when (> 2 1) \"y\") (when-not (= 1 1) \"n\")) \
             (print (long 5.9) (int 7) (clojure.core/when true :q)) \
             (and false (print \"X\")) (or true (print \"Y\")) (when false (print \"W\"))"),
        0
    );
}

#[test]
fn jit_reports_top_level_errors() {
    // A prim failure at top level returns 1 from `main` (the
    // diagnostic went to stderr before the null propagated).
    assert_eq!(run("(print (quot 1 0))"), 1);
}

#[test]
fn jit_reports_errors_inside_defns() {
    // A prim failure inside a `defn` returns null up the call
    // chain to `main`, which returns 1.
    assert_eq!(run("(defn f [x] (quot x 0)) (print (f 1))"), 1);
}

#[test]
fn jit_reports_errors_inside_dash_main() {
    // A failing `-main` exits 1 instead of segfaulting: its
    // null return is checked before drop (lower_main JIT path).
    assert_eq!(run("(defn -main [args] (quot 1 0))"), 1);
}

#[test]
fn jit_reports_arity_errors() {
    // A wrong-arg-count sibling call reports, then returns
    // through the diverged block instead of exiting.
    assert_eq!(run("(defn f [x] x) (print (f 1 2))"), 1);
}

#[test]
fn jit_passes_argv_to_dash_main() {
    let args = ["a".to_owned(), "b".to_owned()];
    assert_eq!(
        run_with("(print :top) (defn -main [args] (print args))", &args),
        0
    );
}

#[test]
fn jit_runs_transient_pipelines() {
    // Vector transients round-trip through the boxed ABI
    // (map transients need map literals, outside the subset),
    // and `nth` reads a live transient buffer.
    assert_eq!(
        run("(print (persistent! (conj! (transient [1]) 2 3))) \
             (print (persistent! (assoc! (transient [1 2]) 0 9))) \
             (print (nth (conj! (transient [7 8]) 9) 2))"),
        0
    );
}

#[test]
fn jit_reports_transient_errors() {
    // A `persistent!` of a persistent value fails the shared
    // native, and use-after-`persistent!` fails the edit check:
    // both return 1 from `main`.
    assert_eq!(run("(print (persistent! [1]))"), 1);
    assert_eq!(
        run("(let [t (transient [1])] (do (persistent! t) (print (conj! t 2))))"),
        1
    );
}
