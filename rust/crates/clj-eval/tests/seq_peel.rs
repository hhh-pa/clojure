// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Seq-peel pins: uniquely-owned `map`/`filter`/`remove` chains over
//! a range walk fused, with identical values, errors, check timing,
//! and side-effect order to the generic stepping walk — while held
//! (shared) chains keep generic memoization.

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
fn peel_values() {
    let mut eval = boot();
    for (src, want) in [
        // Fresh chains (peeled) compute the same values.
        ("(reduce + 0 (filter odd? (map inc (range 100))))", "2500"),
        ("(reduce + 0 (map inc (filter odd? (range 100))))", "2550"),
        ("(reduce + 0 (remove even? (range 20)))", "100"),
        ("(reduce + (map inc (range 5)))", "15"),
        ("(reduce + 0 (map inc (range 0)))", "0"),
        (
            "(reduce + 0 (map #(* % 2) (filter odd? (map inc (range 50)))))",
            "1250",
        ),
        // Short-circuit (`reduced`) stops the fused walk.
        (
            "(reduce (fn [a x] (if (> x 50) (reduced a) (+ a x))) 0 (filter odd? (map inc (range 100))))",
            "625",
        ),
        // Other fused-walk callers.
        ("(dorun (map inc (range 10)))", "nil"),
        ("(doall (map inc (range 3)))", "(1 2 3)"),
        ("(every? pos? (map inc (range 10)))", "true"),
        ("(some even? (map inc (range 10)))", "true"),
        (
            "(frequencies (map #(mod % 3) (range 9)))",
            "{0 3, 1 3, 2 3}",
        ),
        // Non-peelable shapes stay correct (generic path).
        ("(reduce + 0 (map + (range 5) (range 5)))", "20"),
        ("(reduce + 0 (take 5 (map inc (range 100))))", "15"),
        ("(reduce + 0 (map inc [1 2 3]))", "9"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn peel_held_parity() {
    // Held chains walk the generic path; fresh chains peel — same
    // results either way (each pair pins one against the other).
    let mut eval = boot();
    for (fresh, held) in [
        (
            "(reduce + 0 (filter odd? (map inc (range 100))))",
            "(let [s (filter odd? (map inc (range 100)))] (reduce + 0 s))",
        ),
        (
            "(reduce (fn [a x] (if (> x 50) (reduced a) (+ a x))) 0 (filter odd? (map inc (range 100))))",
            "(let [s (filter odd? (map inc (range 100)))] (reduce (fn [a x] (if (> x 50) (reduced a) (+ a x))) 0 s))",
        ),
        (
            "(doall (map inc (filter odd? (range 20))))",
            "(let [s (map inc (filter odd? (range 20)))] (doall s))",
        ),
        (
            "(frequencies (map #(mod % 3) (range 9)))",
            "(let [s (map #(mod % 3) (range 9))] (frequencies s))",
        ),
    ] {
        assert_eq!(
            run_last(&mut eval, fresh),
            run_last(&mut eval, held),
            "{fresh}"
        );
    }
}

#[test]
fn peel_check_timing() {
    // `IFn` checks fire per chunk reached (like the step fns):
    // empty inputs never check, and an op behind an emptying
    // filter never checks — fresh and held alike.
    let mut eval = boot();
    for (src, want) in [
        ("(reduce + 0 (map 42 (range 0)))", "0"),
        (
            "(reduce + 0 (map 42 (filter (fn [_] false) (range 10))))",
            "0",
        ),
        (
            "(let [s (map 42 (filter (fn [_] false) (range 10)))] (reduce + 0 s))",
            "0",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // Reached bad callees fail identically on both paths.
    for src in [
        "(reduce + 0 (map 42 (range 10)))",
        "(reduce + 0 (filter odd? (map 42 (range 10))))",
        "(let [s (map 42 (range 10))] (reduce + 0 s))",
    ] {
        let err = run_last(
            &mut eval,
            &format!("(try {src} (catch Throwable e [(str (class e)) (ex-message e)]))"),
        );
        let want = run_last(
            &mut eval,
            "(try (reduce + 0 (map 42 (range 10))) (catch Throwable e [(str (class e)) (ex-message e)]))",
        );
        assert_eq!(err, want, "{src}");
        assert!(err.contains("ClassCastException"), "{src} -> {err}");
    }
}

#[test]
fn peel_effect_order() {
    // Side-effect order matches the generic chunked walk across
    // the 32-item chunk boundary (40 heads: 32 + 8).
    let tmpl = "(let [log (atom []) f (fn [x] (swap! log conj [:m x]) (inc x)) p (fn [x] (swap! log conj [:p x]) (odd? x))] CHAIN @log)";
    let fresh = tmpl.replace("CHAIN", "(reduce + 0 (filter p (map f (range 40))))");
    let held = tmpl.replace(
        "CHAIN",
        "(let [s (filter p (map f (range 40)))] (reduce + 0 s))",
    );
    let mut eval = boot();
    let a = run_last(&mut eval, &fresh);
    let b = run_last(&mut eval, &held);
    assert_eq!(a, b);
    // Sanity: map runs chunk-at-a-time ahead of filter.
    assert!(a.starts_with("[[:m 0] [:m 1]"), "{a}");
}

#[test]
fn peel_memoization() {
    // Held chains memoize through the generic walk; mid-chain
    // sharing rebuilds the peeled shells and still memoizes the
    // shared cell.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (map inc (range 10))] (reduce + 0 s) (realized? s))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [inner (map inc (range 10))] [(reduce + 0 (map #(* % 2) inner)) (realized? inner)])"
        ),
        "[110 true]"
    );
}

#[test]
fn reduce_plus_range() {
    // Fused `(reduce + init? range)` accumulates tagless.
    let mut eval = boot();
    for (src, want) in [
        ("(reduce + (range 100))", "4950"),
        ("(reduce + 5 (range 100))", "4955"),
        ("(reduce + (range 1))", "0"),
        ("(reduce + 7 (range 0))", "7"),
        ("(reduce + [])", "0"),
        ("(reduce + (range 0))", "0"),
        ("(reduce + (range 1000000))", "499999500000"),
        // Redefined `+` takes the generic path (observes the root).
        ("(with-redefs [+ -] (reduce + (range 5)))", "-10"),
        ("(with-redefs [+ -] (reduce + 100 (range 5)))", "90"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // Overflow bails and throws `long overflow`, like `+` itself
    // (3-arg and 2-arg alike).
    assert!(
        uncaught(&mut eval, "(reduce + 9223372036854775807 (range 10))").contains("long overflow")
    );
    assert!(
        uncaught(
            &mut eval,
            "(reduce + (range 9223372036854775802 9223372036854775807))"
        )
        .contains("long overflow")
    );
}

#[test]
fn range_walk_tail_pins() {
    let mut eval = boot();
    // Early stop short-circuits a range walk.
    assert_eq!(
        run_last(&mut eval, "(reduce (fn [_ _] (reduced :x)) 0 (range 10))"),
        ":x"
    );
    // A completing walk visits the final head (exact tail).
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce (fn [a b] (+ a b)) 0 (range 9223372036854775807 1e300 1))"
        ),
        "9223372036854775807"
    );
    // ... and raises at an inexact tail after yielding its heads.
    assert_eq!(
        uncaught(&mut eval, "(reduce (fn [a b] (+ a b)) 0 (range 0M 1M 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // Peeled chains exhaust and raise the same way.
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (map inc (filter odd? (range 10)))))"
        ),
        "\"(2 4 6 8 10)\""
    );
    assert_eq!(
        uncaught(&mut eval, "(doall (map inc (range 0M 1M 1/3)))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // A realized base stops the peel (generic walk from there).
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (filter odd? (doall (map inc (range 10))))))"
        ),
        "\"(1 3 5 7 9)\""
    );
}

#[test]
fn plus_range_edge_pins() {
    let mut eval = boot();
    // Empty 2-arg: the generic walk calls `(f)`.
    assert_eq!(run_last(&mut eval, "(reduce + (range 0))"), "0");
    // Non-`Int` heads bail (first head, later head).
    assert_eq!(run_last(&mut eval, "(reduce + (range 0.5 2))"), "2.0");
    assert_eq!(run_last(&mut eval, "(reduce + (range 0 2 0.5))"), "3.0");
    // Single-head exact tail (overflow-stepped, non-long).
    assert_eq!(
        run_last(&mut eval, "(reduce + (range 9223372036854775807 1e300 1))"),
        "9223372036854775807"
    );
    // Multi-head exact tail with an offset init.
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce + -9223372036854775806 (range 9223372036854775806 1e300 1))"
        ),
        "9223372036854775807"
    );
    // Non-`Int` init bails (generic walk adds).
    assert_eq!(run_last(&mut eval, "(reduce + 0.5 (range 10))"), "45.5");
    // Inexact steps bail on their non-`Int` heads (generic raises).
    assert_eq!(
        uncaught(&mut eval, "(reduce + (range 0M 1M 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
}

#[test]
fn peel_reused_frame_pins() {
    let mut eval = boot();
    // Captures observe their own apply (the shared frame falls back
    // to a fresh bind per apply).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [fs (doall (map (fn [x] (fn [] x)) (range 3)))] (mapv (fn [f] (f)) fs))"
        ),
        "[0 1 2]"
    );
    // `recur` to the mapped fn loops like `apply_arity`.
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (map (fn [x] (if (> x 0) (recur (dec x)) :done)) (range 3)))"
        ),
        "(:done :done :done)"
    );
    // Capture plus `recur` (the shared frame rebinds fresh, and
    // each capture keeps its own round).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom [])] (doall (map (fn [x] (do (swap! a conj (fn [] x)) (if (> x 0) (recur (dec x)) :done))) (range 2))) [(count @a) (mapv (fn [f] (f)) @a)])"
        ),
        "[3 [0 1 0]]"
    );
    // A throwing map fn propagates (like the generic apply).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (doall (map (fn [x] (if (= x 1) (throw (ex-info \"b\" {})) x)) (range 3))) (catch Exception e :caught))"
        ),
        ":caught"
    );
    // The specialized filter arm (closure predicate).
    assert_eq!(
        run_last(&mut eval, "(doall (filter #(> % 1) (range 4)))"),
        "(2 3)"
    );
    // Ineligible fns apply generically (identical values).
    assert_eq!(
        run_last(&mut eval, "(doall (map (fn ([x] x) ([x y] y)) (range 3)))"),
        "(0 1 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (map (bound-fn [x] (* x 2)) (range 3)))"),
        "(0 2 4)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (map (fn [& xs] (count xs)) (range 3)))"),
        "(1 1 1)"
    );
}

fn on_worker<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .expect("spawn worker")
        .join()
        .expect("worker panicked")
}

#[test]
fn peel_reused_depth_parity() {
    // Nested peeled maps trip the same `StackOverflowError` as the
    // generic path (the guard matches `apply_closure`
    // apply-for-apply).
    on_worker(|| {
        let mut eval = boot();
        assert_eq!(
            run_last(
                &mut eval,
                "(try (doall (map (fn f [n] (if (zero? n) 0 (first (doall (map f (range (dec n) n)))))) [100000])) (catch StackOverflowError e :so))"
            ),
            ":so"
        );
    });
}

#[test]
fn peel_ineligible_fn_pins() {
    let mut eval = boot();
    // Ineligible fns decline the reused frame inside a peeling
    // walk too (identical values to the generic walk): multi-arity
    // and conveyed (`bound-fn`) closures, and rest-arg closures.
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce + 0 (map (fn ([x] (inc x)) ([x y] (+ x y))) (range 3)))"
        ),
        "6"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce + 0 (map (bound-fn [x] (* x 2)) (range 3)))"
        ),
        "6"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce + 0 (map (fn [& xs] (apply + xs)) (range 3)))"
        ),
        "3"
    );
}

#[test]
fn peel_reused_frame_dorun_pins() {
    let mut eval = boot();
    // A frame observed across applies (a retained capture) falls
    // back to a fresh bind per apply.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom nil)] (dorun (map (fn [x] (reset! a (fn [] x)) x) (range 3))))"
        ),
        "nil"
    );
    // `recur` to the mapped fn loops the reused frame in place.
    assert_eq!(
        run_last(
            &mut eval,
            "(dorun (map (fn [x] (if (> x 0) (recur (dec x)) :done)) (range 3)))"
        ),
        "nil"
    );
    // Capture plus `recur` (the observed frame rebinds fresh).
    assert_eq!(
        run_last(
            &mut eval,
            "(dorun (map (fn [x] (let [c (fn [] x)] (if (> x 0) (recur (dec x)) (c)))) (range 3)))"
        ),
        "nil"
    );
    // A throwing map fn propagates through the reused apply.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (dorun (map (fn [x] (if (= x 1) (throw (ex-info \"b\" {})) x)) (range 3))) (catch Exception e :caught))"
        ),
        ":caught"
    );
}

#[test]
fn peel_walk_tail_and_empty_pins() {
    let mut eval = boot();
    // A peeling walk visits the pulled heads, then raises at an
    // inexact tail (the `walk_range` tail).
    assert_eq!(
        uncaught(&mut eval, "(dorun (map inc (range 0M 1M 1/3)))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // An empty 2-arg `+` reduce over a range bails to the generic
    // walk, which calls `(f)`.
    assert_eq!(run_last(&mut eval, "(reduce + (range 5 5))"), "0");
}

#[test]
fn peel_reused_depth_guard() {
    // Depth grown entirely through the reused apply (direct calls
    // don't count) trips the reused apply's own guard — a nil-
    // message `StackOverflowError`, like the generic apply.
    on_worker(|| {
        let mut eval = boot();
        assert_eq!(
            run_last(
                &mut eval,
                "(try ((fn f [x] (dorun (map f (range x (inc x))))) 0) (catch StackOverflowError e [:caught (nil? (ex-message e))]))"
            ),
            "[:caught true]"
        );
    });
}
