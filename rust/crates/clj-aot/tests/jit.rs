// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

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
