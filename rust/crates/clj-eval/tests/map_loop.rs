// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Threaded-map loop pins (`Evaluator::try_int_loop` over map
//! bindings): build loops thread one assoc chain through
//! `Map::insert_threaded` with slow-path-identical values,
//! iteration order, entry preservation, and identity — while
//! sorted maps, exotic keys, and exotic shapes decline to the
//! interpreter.

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
fn map_loop_builds() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (loop [i 0 m {}] (if (< i 100000) (recur (inc i) (assoc m i (* i 2))) m))] [(count m) (get m 99999) (reduce + 0 (vals m))])"
        ),
        "[100000 199998 9999900000]"
    );
}

#[test]
fn map_loop_preserves_entry() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m0 {:a 1}] [(loop [m m0 i 0] (if (< i 3) (recur (assoc m i i) (inc i)) (count m))) m0])"
        ),
        "[4 {:a 1}]"
    );
    // A hash entry survives threading intact (deep-copy path).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m0 (into {} (map (fn [i] [i i]) (range 100)))] (let [r (loop [m m0 i 100] (if (< i 110) (recur (assoc m i (* i 2)) (inc i)) m))] [(count r) (get r 109) (count m0) (get m0 50)]))"
        ),
        "[110 218 100 50]"
    );
}

#[test]
fn map_loop_identity() {
    let mut eval = boot();
    // Mutated loops answer a fresh version ...
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m0 {}] (let [r (loop [m m0 i 0] (if (< i 2) (recur (assoc m i i) (inc i)) m))] [(identical? m0 r) (count r)]))"
        ),
        "[false 2]"
    );
    // ... even when every round rewrites existing keys ...
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m0 {:a 1}] (let [r (loop [m m0 i 0] (if (< i 3) (recur (assoc m :a (inc i)) (inc i)) m))] [r (identical? m0 r)]))"
        ),
        "[{:a 3} false]"
    );
    // ... while a loop that never assocs answers the entry itself.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m0 {:a 1}] (identical? m0 (loop [m m0 i 10] (if (< i 3) (recur (assoc m i i) (inc i)) m))))"
        ),
        "true"
    );
}

#[test]
fn map_loop_nested_and_const() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 3) (recur (inc i) (assoc (assoc m :a i) :b i)) m))"
        ),
        "{:a 2, :b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 3) (recur (inc i) (assoc m \"k\" i)) m))"
        ),
        "{\"k\" 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v 99] (loop [i 0 m {}] (if (< i 3) (recur (inc i) (assoc m i v)) m)))"
        ),
        "{0 99, 1 99, 2 99}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m (with-meta {} {:a 1})] (if (< i 2) (recur (inc i) (assoc m i i)) (meta m)))"
        ),
        "{:a 1}"
    );
}

#[test]
fn map_loop_two_vars() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 a {} b {}] (if (< i 3) (recur (inc i) (assoc a i 1) (assoc b i 2)) [a b]))"
        ),
        "[{0 1, 1 1, 2 1} {0 2, 1 2, 2 2}]"
    );
}

#[test]
fn map_loop_shared_base() {
    // Both steps assoc `m` (each sees the pre-round map, like the
    // slow path — `n` inherits `m`'s round-1 entry).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {} n {}] (if (< i 2) (recur (inc i) (assoc m :a i) (assoc m :b i)) [m n]))"
        ),
        "[{:a 1} {:a 0, :b 1}]"
    );
}

#[test]
fn map_loop_exit_recur_continues() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 2) (recur (inc i) (assoc m i i)) (if (< i 4) (recur (inc i) (assoc m i i)) (count m))))"
        ),
        "4"
    );
}

#[test]
fn map_loop_bail_runs_effects_once() {
    let mut eval = boot();
    // Like `int_loop_bail_runs_effects_once`, over a mixed map/int
    // loop (the overflow bail resumes from the round-start snapshot,
    // effects exactly once).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (try (loop [i 0 m {}] (if (< i 2) (do (swap! a inc) (recur (inc i) (assoc m i i))) (recur (+ i 9223372036854775807) m))) (catch ArithmeticException e [:threw @a])))"
        ),
        "[:threw 2]"
    );
    // A root write after threaded exit rounds resumes with the map
    // intact (three increments and three entries, not four).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (loop [i 0 m {}] (if (< i 3) (do (swap! a inc) (def map-loop-bail-pin i) (recur (inc i) (assoc m i i))) [(count m) @a])))"
        ),
        "[3 3]"
    );
    // An exotic exit `recur` resumes from its own values (like the
    // integer loop above — the test throws exactly as `(< :boom 2)`
    // directly, effects once).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (try (loop [i 0 m {}] (if (< i 2) (do (swap! a inc) (recur (if (< i 1) (inc i) :boom) (assoc m i i))) @a)) (catch Exception e [:caught @a])))"
        ),
        "[:caught 2]"
    );
}

#[test]
fn map_loop_exit_capture_keeps_round() {
    // Like `int_loop_exit_capture_keeps_round`, over a mixed
    // map/int loop (the reused exit env falls back per round once
    // a closure observes it).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom [])] (loop [i 0 m {}] (if (< i 3) (do (swap! a conj (fn [] i)) (recur (inc i) (assoc m i i))) [(count m) (mapv (fn [f] (f)) @a)])))"
        ),
        "[3 [0 1 2]]"
    );
}

#[test]
fn map_loop_slow_shapes() {
    let mut eval = boot();
    // Sorted entries decline (comparator order preserved — the keys
    // insert reversed so insertion order would differ).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m (sorted-map) i 0] (if (< i 3) (recur (assoc m (- 2 i) i) (inc i)) (vec m)))"
        ),
        "[[0 2] [1 1] [2 0]]"
    );
    // Tests over the map decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(count (loop [i 0 m {}] (if (< (count m) 3) (recur (inc i) (assoc m i i)) m)))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {}] (if (= m {}) (recur (inc i) (assoc m i i)) :never))"
        ),
        ":never"
    );
    // Multi-pair assocs decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(count (loop [i 0 m {}] (if (< i 6) (recur (+ i 2) (assoc m i i (inc i) (inc i))) m)))"
        ),
        "6"
    );
    // Non-integer key expressions decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 2) (recur (inc i) (assoc m (str i) i)) m))"
        ),
        "{\"0\" 0, \"1\" 1}"
    );
}

#[test]
fn map_loop_assoc_redefs_observed() {
    // `assoc` is not inlined (JVM observes too): a rebound `assoc`
    // declines threading and the slow path answers.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [assoc (fn [m k v] :mocked)] (loop [i 0 m {}] (if (< i 2) (recur (inc i) (assoc m i i)) m)))"
        ),
        ":mocked"
    );
}

#[test]
fn map_loop_overflow_bails() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 2) (recur (inc i) (assoc m (+ i 9223372036854775807) i)) m))"
        ),
        "java.lang.ArithmeticException | long overflow"
    );
}

#[test]
fn map_loop_bail_pins() {
    let mut eval = boot();
    // Overflow inside the loop TEST bails (the interpreter throws).
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 9223372036854775807 m {}] (if (< i (+ i 1)) (recur i m) :done))"
        ),
        "java.lang.ArithmeticException | long overflow"
    );
    // Throws from a map-loop exit branch propagate.
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 0 m {}] (if (< i 3) (recur (inc i) (assoc m i i)) (throw (ex-info \"boom\" {}))))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
    // A root change under an exit-branch `recur` bails mid-loop
    // (the slow path re-runs from the entry).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m {} i 0] (if (< i 1) (recur (assoc m i i) (inc i)) (if (= i 1) (do (def epoch-bump-9 9) (recur m (inc i))) :done)))"
        ),
        ":done"
    );
    // Non-matching map steps decline (slow path decides).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m {} i 0] (if (< i 2) (recur ((fn [x] x) m) (inc i)) m))"
        ),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m {:a 1} i 0] (if (< i 1) (recur (dissoc m :a) (inc i)) m))"
        ),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [outer {}] (loop [m {} i 0] (if (< i 2) (recur outer (inc i)) m)))"
        ),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m {} i 0] (if (< i 2) (recur i (inc i)) m))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [m {} i 0] (if (< i 2) (recur {:a 1} (inc i)) m))"
        ),
        "{:a 1}"
    );
    // A natively-rooted `assoc` stand-in fails the display check
    // (declines; the slow path answers without calling it).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [assoc conj] (loop [m {} i 9] (if (< i 2) (recur (assoc m i i) (inc i)) :exited)))"
        ),
        ":exited"
    );
}
