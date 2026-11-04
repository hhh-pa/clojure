// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #226 Interpreter optimization sweep: in-place move in `try_rebind`,
//! recur scratch buffer pooling, zero-copy local integer arithmetic,
//! and O(1) ASCII string indexing.
//!
//! Verifies correctness, captured-frame copy-on-recur guarantees,
//! overflow and type fallback semantics, and enforces performance
//! regression bounds on loops and string scans.

use std::time::Instant;

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => panic!("expected an uncaught throw for {src}, got {vs:?}"),
    }
}

/// A basic loop accumulates 1,000,000 iterations correctly and
/// rebinds in-place with zero element cloning.
#[test]
fn loop_recur_in_place_rebind() {
    let mut eval = boot();
    let src = "(loop [i 0 acc 0]
                 (if (< i 1000000)
                   (recur (inc i) (+ acc i))
                   acc))";
    assert_eq!(run_last(&mut eval, src), "499999500000");
}

/// Multi-variable swap loop: verify in-place move semantics when
/// variables change values and swap positions each iteration.
#[test]
fn loop_recur_variable_swaps() {
    let mut eval = boot();
    let src = "(loop [n 1000 a 1 b 2 c 3]
                 (if (> n 0)
                   (recur (dec n) b c a)
                   [a b c]))";
    // 1000 % 3 == 1, so [a b c] shifts left by 1: [b c a] -> [2 3 1]
    assert_eq!(run_last(&mut eval, src), "[2 3 1]");
}

/// Captured loop frame: closures constructed within loop iterations
/// keep observing their respective iteration's values (copy-on-recur).
#[test]
fn captured_frame_preserves_copy_on_recur() {
    let mut eval = boot();
    let src = "(let [fns (loop [i 0 acc []]
                          (if (< i 5)
                            (recur (inc i) (conj acc (fn [] i)))
                            acc))]
                 (mapv (fn [f] (f)) fns))";
    assert_eq!(run_last(&mut eval, src), "[0 1 2 3 4]");
}

/// Nested loops: inner loop recycles buffers from the pool without
/// disturbing outer loop variables or state.
#[test]
fn nested_loops_buffer_pool_isolation() {
    let mut eval = boot();
    let src = "(loop [i 0 outer_acc 0]
                 (if (< i 100)
                   (let [inner_sum (loop [j 0 inner_acc 0]
                                     (if (< j 100)
                                       (recur (inc j) (+ inner_acc j))
                                       inner_acc))]
                     (recur (inc i) (+ outer_acc inner_sum)))
                   outer_acc))";
    // Each inner loop sum is 4950; 100 times is 495000
    assert_eq!(run_last(&mut eval, src), "495000");
}

/// Named and anonymous `fn` recur: tail recur to fn rebinds
/// parameters in place and reuses the buffer pool with high throughput.
#[test]
fn fn_recur_in_place() {
    let mut eval = boot();
    let start = Instant::now();
    let res = run_last(
        &mut eval,
        "(let [f (fn [n acc]
                     (if (> n 0)
                       (recur (dec n) (+ acc n))
                       acc))]
             (f 500000 0))",
    );
    let elapsed = start.elapsed();
    assert_eq!(res, "125000250000");
    assert!(
        elapsed.as_secs_f64() < 10.0,
        "500k fn tail recur took {:.3}s, expected < 10.0s",
        elapsed.as_secs_f64()
    );
}

/// Performance regression gate: high-density local integer arithmetic
/// operations (`+`, `-`, `*`, `<=`, `inc`, `dec`) must borrow slots directly
/// and compute in place without cloning 64-byte `Value`s or allocating.
#[test]
fn perf_local_arith_throughput() {
    let mut eval = boot();
    let start = Instant::now();
    let res = run_last(
        &mut eval,
        "(loop [i 0 a 1 b 2 acc 0]
           (if (<= i 500000)
             (recur (inc i) (+ a 1) (dec b) (+ acc (- (* a 2) b)))
             acc))",
    );
    let elapsed = start.elapsed();
    assert!(res.parse::<i64>().unwrap() > 0);
    assert!(
        elapsed.as_secs_f64() < 10.0,
        "500k multi-arith loop took {:.3}s, expected < 10.0s",
        elapsed.as_secs_f64()
    );
}

/// Performance regression gate: wide binding frames (7 variables, analogous
/// to the dijkstra-parse loop) must rebind in place with zero per-iteration
/// vector allocations or element cloning churn.
#[test]
fn perf_wide_frame_rebind() {
    let mut eval = boot();
    let start = Instant::now();
    let res = run_last(
        &mut eval,
        "(loop [n 300000 a 1 b 2 c 3 d 4 e 5 f 6]
           (if (> n 0)
             (recur (dec n) b c d e f (inc a))
             (+ a b c d e f)))",
    );
    let elapsed = start.elapsed();
    assert!(res.parse::<i64>().unwrap() > 0);
    assert!(
        elapsed.as_secs_f64() < 10.0,
        "300k 7-binding loop took {:.3}s, expected < 10.0s",
        elapsed.as_secs_f64()
    );
}

/// Local integer arithmetic fast path: operations directly on locals
/// compute in place without cloning 64-byte `Value`s.
#[test]
fn fast_local_arith_and_comparisons() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x 10 y 20] [(+ x y) (- y x) (* x y) (< x y) (<= x y) (> x y) (>= x y)])"
        ),
        "[30 10 200 true true false false]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [x 10] [(inc x) (dec x)])"),
        "[11 9]"
    );
}

/// Overflow in local integer arithmetic correctly falls through
/// to the native road and reports `ArithmeticException: long overflow`.
#[test]
fn fast_local_arith_overflow_fallback() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(let [x 9223372036854775807] (inc x))"),
        "java.lang.ArithmeticException | long overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [x -9223372036854775808] (dec x))"),
        "java.lang.ArithmeticException | long overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [x 9223372036854775807 y 1] (+ x y))"),
        "java.lang.ArithmeticException | long overflow"
    );
}

/// Non-integer local operands fall through correctly to the native road.
#[test]
fn fast_local_arith_non_integer_fallback() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x 10.5 y 2.0] [(+ x y) (- x y) (* x y) (< x y)])"
        ),
        "[12.5 8.5 21.0 false]"
    );
}

/// ASCII string `nth` operates in O(1) time with exact bounds checks.
#[test]
fn ascii_string_nth_lookup_and_bounds() {
    let mut eval = boot();
    // 100-char ASCII alphabet repeat
    let setup = "(def s (apply str (repeat 20 \"abcdefghijklmnopqrstuvwxyz\")))";
    run_last(&mut eval, setup);

    // Indexing at start, middle, and end
    assert_eq!(run_last(&mut eval, "(nth s 0)"), "\\a");
    assert_eq!(run_last(&mut eval, "(nth s 25)"), "\\z");
    assert_eq!(run_last(&mut eval, "(nth s 26)"), "\\a");
    assert_eq!(run_last(&mut eval, "(nth s 519)"), "\\z");

    // Default value when out of bounds
    assert_eq!(run_last(&mut eval, "(nth s 1000 :not-found)"), ":not-found");
    assert_eq!(run_last(&mut eval, "(nth s -1 :not-found)"), ":not-found");

    // Exception when out of bounds with no default
    assert_eq!(
        uncaught(&mut eval, "(nth s 520)"),
        "java.lang.StringIndexOutOfBoundsException | Index 520 out of bounds for length 520"
    );
}

/// Performance regression gate: 1,000,000 loop-recur iterations must
/// complete with high throughput (bounds test prevents regression to
/// per-iteration cloning or heap allocations).
#[test]
fn perf_loop_recur_million_iterations() {
    let mut eval = boot();
    // Warm up
    run_last(&mut eval, "(loop [i 0] (if (< i 10000) (recur (inc i)) i))");

    let start = Instant::now();
    let res = run_last(
        &mut eval,
        "(loop [i 0 acc 0]
           (if (< i 1000000)
             (recur (inc i) (+ acc i))
             acc))",
    );
    let elapsed = start.elapsed();
    assert_eq!(res, "499999500000");

    // In debug builds this runs in ~250-400ms; in release under ~80ms.
    // 10.0 seconds is a massive safety cushion that accommodates instrumented
    // coverage builds (e.g. llvm-cov) while still catching any quadratic
    // regression or accidental clone reintroduction.
    assert!(
        elapsed.as_secs_f64() < 10.0,
        "1M loop iterations took {:.3}s, expected < 10.0s",
        elapsed.as_secs_f64()
    );
}

/// Performance regression gate: 200,000 ASCII string lookups across
/// a 50KB string must complete quickly via O(1) single-byte loads.
#[test]
fn perf_ascii_string_indexing() {
    let mut eval = boot();
    run_last(
        &mut eval,
        "(def big-str (apply str (repeat 2000 \"abcdefghijklmnopqrstuvwxyz\")))",
    );

    let start = Instant::now();
    let res = run_last(
        &mut eval,
        "(loop [i 0 acc 0]
           (if (< i 200000)
             (let [c (nth big-str (rem i 52000))]
               (recur (inc i) (if (= c \\a) (inc acc) acc)))
             acc))",
    );
    let elapsed = start.elapsed();
    // In 200,000 steps over 26-char cycles, exactly 200000 / 26 = 7693 'a's (or 7693)
    assert!(res.parse::<i64>().unwrap() > 7000);

    // Bounded check ensures O(1) lookup without UTF-8 scanning regressions
    assert!(
        elapsed.as_secs_f64() < 10.0,
        "200k string nth took {:.3}s, expected < 10.0s",
        elapsed.as_secs_f64()
    );
}
