// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Threaded-vector loop pins (`Evaluator::try_int_loop` over packed
//! int vectors): conditional-swap loops thread one vector through
//! `Vector::assoc_int_threaded` with slow-path-identical values,
//! entry preservation, and identity — while generic vectors,
//! exotic shapes, bounds failures, and redefined roots decline to
//! the interpreter.

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

#[test]
fn vec_loop_basics() {
    let mut eval = boot();
    for (src, want) in [
        // Plain assoc loop.
        (
            "(loop [i 0 v (vec (range 10))] (if (< i 10) (recur (inc i) (assoc v i (* i 2))) v))",
            "[0 2 4 6 8 10 12 14 16 18]",
        ),
        // Multi-pair assoc.
        (
            "(loop [i 0 v [0 0 0 0]] (if (< i 2) (recur (inc i) (assoc v i 1 (+ i 2) 2)) v))",
            "[1 1 2 2]",
        ),
        // Conditional swap over `nth` reads (the bubble inner shape).
        (
            "(loop [j 0 arr [3 1 2]] (if (>= j 2) arr (recur (inc j) (if (> (nth arr j) (nth arr (inc j))) (assoc arr j (nth arr (inc j)) (inc j) (nth arr j)) arr))))",
            "[1 2 3]",
        ),
        // Index-at-len appends (like the interpreter).
        (
            "(loop [i 0 v []] (if (< i 5) (recur (inc i) (assoc v i i)) v))",
            "[0 1 2 3 4]",
        ),
        // Passthrough steps keep the pre-round vector.
        (
            "(loop [i 0 v [7 8]] (if (< i 3) (recur (inc i) (if (> i 1) (assoc v 0 9) v)) v))",
            "[9 8]",
        ),
        // Two vectors thread disjoint bases.
        (
            "(loop [i 0 a [1 2] b [3 4]] (if (< i 2) (recur (inc i) (assoc a i 9) (assoc b i 8)) [a b]))",
            "[[9 9] [8 8]]",
        ),
        // Two steps, one base: each sees the pre-round vector.
        (
            "(loop [i 0 x [0 0] y [0 0]] (if (< i 1) (recur (inc i) (assoc x 0 1) (assoc x 1 2)) [x y]))",
            "[[1 0] [0 2]]",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn vec_loop_bubble() {
    // The h2h bubble shape (nested loops, hashed init) sorts.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (vec (map (fn [x] (mod (* x 7919) 200)) (range 200))) n (count a)] [(nth (loop [i 0 arr a] (if (>= i n) arr (recur (inc i) (loop [j 0 arr arr] (if (>= j (- n i 1)) arr (recur (inc j) (if (> (nth arr j) (nth arr (inc j))) (assoc arr j (nth arr (inc j)) (inc j) (nth arr j)) arr))))))) 0) (nth (loop [i 0 arr a] (if (>= i n) arr (recur (inc i) (loop [j 0 arr arr] (if (>= j (- n i 1)) arr (recur (inc j) (if (> (nth arr j) (nth arr (inc j))) (assoc arr j (nth arr (inc j)) (inc j) (nth arr j)) arr))))))) 199)])"
        ),
        "[0 199]"
    );
    // Threaded and explicitly-slow shapes agree element-wise
    // (3-arg `nth` declines to the interpreter but never misses —
    // every read is in bounds).
    let mut eval = boot();
    let init = "(vec (map (fn [x] (mod (* x 7919) 50)) (range 50)))";
    let fast = "(fn [a] (let [n (count a)] (loop [i 0 arr a] (if (>= i n) arr (recur (inc i) (loop [j 0 arr arr] (if (>= j (- n i 1)) arr (recur (inc j) (if (> (nth arr j) (nth arr (inc j))) (assoc arr j (nth arr (inc j)) (inc j) (nth arr j)) arr)))))))))";
    let slow = "(fn [a] (let [n (count a)] (loop [i 0 arr a] (if (>= i n) arr (recur (inc i) (loop [j 0 arr arr] (if (>= j (- n i 1)) arr (recur (inc j) (if (> (nth arr j :nf) (nth arr (inc j) :nf)) (assoc arr j (nth arr (inc j) :nf) (inc j) (nth arr j :nf)) arr)))))))))";
    let a = run_last(&mut eval, &format!("({fast} {init})"));
    let b = run_last(&mut eval, &format!("({slow} {init})"));
    assert_eq!(a, b);
    let v: Vec<i64> = a
        .trim_matches(|c| c == '[' || c == ']')
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    assert_eq!(v.len(), 50);
    assert!(v.windows(2).all(|w| w[0] <= w[1]), "{a}");
}

#[test]
fn vec_loop_preserves_entry() {
    let mut eval = boot();
    // The entry vector is never mutated...
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v0 [3 1 2]] [(loop [j 0 arr v0] (if (>= j 2) arr (recur (inc j) (if (> (nth arr j) (nth arr (inc j))) (assoc arr j (nth arr (inc j)) (inc j) (nth arr j)) arr)))) v0])"
        ),
        "[[1 2 3] [3 1 2]]"
    );
    // ...and the threaded result never aliases its identity.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v0 [3 1 2]] (identical? v0 (loop [j 0 arr v0] (if (>= j 1) arr (recur (inc j) (assoc arr j 9))))))"
        ),
        "false"
    );
}

#[test]
fn vec_loop_bail_runs_effects_once() {
    let mut eval = boot();
    // Like `map_loop_bail_runs_effects_once`, over a threaded
    // vector (the root-write bail resumes with the vector intact).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (loop [i 0 v [9 9 9]] (if (< i 3) (do (swap! a inc) (def vec-loop-bail-pin i) (recur (inc i) (assoc v i i))) [(nth v 2) @a])))"
        ),
        "[2 3]"
    );
}

#[test]
fn vec_loop_bails() {
    let mut eval = boot();
    // Out-of-bounds assoc: the fast path bails and the slow path
    // throws exactly (past-the-end, where not even `assoc` appends).
    let oob_fast = "(loop [i 0 v [1 2]] (if (< i 5) (recur (inc i) (assoc v 9 9)) v))";
    let oob_slow = "(loop [i 0 v (assoc (assoc [1 2] 0 :k) 0 1)] (if (< i 5) (recur (inc i) (assoc v 9 9)) v))";
    assert_eq!(uncaught(&mut eval, oob_fast), uncaught(&mut eval, oob_slow));
    // Out-of-bounds `nth` in the condition bails to the exact throw.
    let nth_fast =
        "(loop [i 0 v [1 2]] (if (< i 5) (recur (inc i) (if (> (nth v 9) 0) (assoc v 0 9) v)) v))";
    let nth_slow = "(loop [i 0 v [1 2]] (if (< i 5) (recur (inc i) (if (> (nth v 9 -1) 0) (assoc v 0 9) v)) v))";
    // The slow shape reads the not-found default (no throw); the
    // fast shape bails and the interpreter throws exactly.
    assert_eq!(run_last(&mut eval, nth_slow), "[1 2]");
    assert!(uncaught(&mut eval, nth_fast).contains("IndexOutOfBounds"));
    // Overflowing indexes throw `long overflow` both ways.
    let ovf =
        "(loop [i 0 v [1 2]] (if (< i 2) (recur (inc i) (assoc v (+ 9223372036854775807 1) 9)) v))";
    assert!(uncaught(&mut eval, ovf).contains("long overflow"));
    // Redefined roots decline to the interpreter (which observes
    // the redefinition).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [assoc (fn [c & _] c)] (loop [i 0 v [1 2]] (if (< i 3) (recur (inc i) (assoc v i 9)) v)))"
        ),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [nth (fn [& _] 0)] (loop [i 0 v [5 6]] (if (< i 2) (recur (inc i) (if (> (nth v i) 0) (assoc v i 9) v)) v)))"
        ),
        "[5 6]"
    );
    // Nested `assoc` chains decline (same values, slow path).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 v [0 0]] (if (< i 1) (recur (inc i) (assoc (assoc v 0 1) 1 2)) v))"
        ),
        "[1 2]"
    );
    // Generic vectors decline (same values, slow path).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 v [:a :b]] (if (< i 2) (recur (inc i) (assoc v i i)) v))"
        ),
        "[0 1]"
    );
}

#[test]
fn vec_loop_decline_pins() {
    let mut eval = boot();
    // Non-matching vector bases decline (slow path decides).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [o [9]] (loop [v [1] i 0] (if (< i 1) (recur o (inc i)) v)))"
        ),
        "[9]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur i (inc i)) v))"
        ),
        "0"
    );
    // Non-matching `assoc` shapes decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur ((fn [x] x) v) (inc i)) v))"
        ),
        "[1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (conj v 2) (inc i)) v))"
        ),
        "[1 2]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (assoc v) (inc i)) v))"
        ),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/assoc"
    );
    // Non-matching `assoc` operands decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (assoc v 0 \"s\") (inc i)) v))"
        ),
        "[\"s\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (assoc v 0 ((fn [c k] (nth c k)) v 0)) (inc i)) v))"
        ),
        "[1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (assoc v 0 (get v 0)) (inc i)) v))"
        ),
        "[1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [1] i 0] (if (< i 1) (recur (assoc v 0 (nth [9] 0)) (inc i)) v))"
        ),
        "[9]"
    );
    // Cross-base reads decline (each step sees one base).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 a [0] b [7]] (if (< i 1) (recur (inc i) (assoc a 0 (nth b 0)) b) a))"
        ),
        "[7]"
    );
    // Mixed-base `if` branches decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 a [0] b [0]] (if (< i 1) (recur (inc i) (if (> i 0) (assoc a 0 9) b) b) a))"
        ),
        "[0]"
    );
    // Pass-only steps need no root guard.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [5] i 0] (if (< i 2) (recur (if (< i 9) v v) (inc i)) v))"
        ),
        "[5]"
    );
    // A natively-rooted `nth` stand-in fails the display check
    // (declines; the slow path answers without calling it).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [nth first] (loop [v [5] i 9] (if (< i 2) (recur (assoc v 0 (nth v 0)) (inc i)) :exited)))"
        ),
        ":exited"
    );
}

#[test]
fn vec_loop_cond_pins() {
    let mut eval = boot();
    // Indirect and exotic `if` conditions decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [0] i 0] (if (< i 2) (recur (if (odd? i) (assoc v 0 9) v) (inc i)) v))"
        ),
        "[9]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [0] i 0] (if (< i 1) (recur (if ((fn [a] a) i) (assoc v 0 9) v) (inc i)) v))"
        ),
        "[9]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [0] i 0] (if (< i 1) (recur (if 1 (assoc v 0 9) v) (inc i)) v))"
        ),
        "[9]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [0] i 0] (if (< i 1) (recur (if (+ i 1) (assoc v 0 9) v) (inc i)) v))"
        ),
        "[9]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [v [0] i 0] (if (< i 1) (recur (if (< i) (assoc v 0 9) v) (inc i)) v))"
        ),
        "[9]"
    );
    // Every comparison threads through `if` steps.
    for (cond, want) in [
        ("(< (nth v 0) i)", "[9]"),
        ("(<= (nth v 0) i)", "[9]"),
        ("(>= (nth v 0) i)", "[0]"),
        ("(= (nth v 0) i)", "[0]"),
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!(
                    "(loop [v [0] i 1] (if (< i 2) (recur (if {cond} (assoc v 0 9) v) (inc i)) v))"
                )
            ),
            want,
            "{cond}"
        );
    }
}
