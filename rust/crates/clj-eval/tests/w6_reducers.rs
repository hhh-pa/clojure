// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W6: `clojure.core.reducers` smoke tests — reduce/fold/map/filter
//! parity with the sequential core ops.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

fn pr(v: &clj_value::Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

fn boot_red() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.reducers :as r])");
    eval
}

#[test]
fn reducers_ns_loads() {
    let mut eval = boot_red();
    assert_eq!(
        run(
            &mut eval,
            "[(ifn? r/reduce) (ifn? r/fold) (ifn? r/reducer) (ifn? r/folder) (ifn? r/map) (ifn? r/mapcat) (ifn? r/filter) (ifn? r/remove) (ifn? r/flatten) (ifn? r/take-while) (ifn? r/take) (ifn? r/drop) (ifn? r/cat) (ifn? r/append!) (ifn? r/foldcat) (ifn? r/monoid) (ifn? r/fjtask) (ifn? r/coll-fold)]"
        ),
        "[true true true true true true true true true true true true true true true true true true]"
    );
}

#[test]
fn reduce_basics() {
    let mut eval = boot_red();
    assert_eq!(run(&mut eval, "(r/reduce + 0 [1 2 3 4])"), "10");
    // No init: (f) seeds.
    assert_eq!(run(&mut eval, "(r/reduce + [1 2 3])"), "6");
    assert_eq!(run(&mut eval, "(r/reduce conj [] nil)"), "[]");
    // Maps reduce kv.
    assert_eq!(
        run(
            &mut eval,
            "(r/reduce (fn [m k v] (assoc m k (inc v))) {} {:a 1 :b 2})"
        ),
        "{:a 2, :b 3}"
    );
    // `reduced` short-circuits.
    assert_eq!(
        run(
            &mut eval,
            "(r/reduce (fn [a x] (if (> x 2) (reduced a) (+ a x))) 0 [1 2 3 4])"
        ),
        "3"
    );
}

#[test]
fn xforms_match_sequential() {
    let mut eval = boot_red();
    // Each reducer agrees with its sequential twin.
    assert_eq!(
        run(
            &mut eval,
            "[(= (r/reduce + 0 (r/map #(* % %) [1 2 3])) (reduce + 0 (map #(* % %) [1 2 3])))
              (= (r/reduce conj [] (r/filter even? [1 2 3 4])) (vec (filter even? [1 2 3 4])))
              (= (r/reduce conj [] (r/remove even? [1 2 3 4])) (vec (remove even? [1 2 3 4])))
              (= (r/reduce conj [] (r/mapcat #(repeat 2 %) [1 2])) (vec (mapcat #(repeat 2 %) [1 2])))
              (= (r/reduce conj [] (r/take 2 [1 2 3])) [1 2])
              (= (r/reduce conj [] (r/drop 2 [1 2 3 4])) [3 4])
              (= (r/reduce conj [] (r/take-while #(< % 3) [1 2 3 1])) [1 2])
              (= (r/reduce conj [] (r/flatten [1 [2 [3]]])) [1 2 3])]"
        ),
        "[true true true true true true true true]",
    );
    // Infinite sources terminate under take.
    assert_eq!(
        run(&mut eval, "(r/reduce conj [] (r/take 3 (range)))"),
        "[0 1 2]"
    );
    // Curried one-arg calls return fns awaiting the collection.
    assert_eq!(
        run(&mut eval, "(r/reduce conj [] ((r/map inc) [1 2]))"),
        "[2 3]"
    );
    // Maps drive the kv arity.
    assert_eq!(
        run(
            &mut eval,
            "(into {} (r/reduce conj [] (r/map (fn [k v] [k (inc v)]) {:a 1})))"
        ),
        "{:a 2}"
    );
}

#[test]
fn fold_matches_sequential() {
    let mut eval = boot_red();
    assert_eq!(run(&mut eval, "(r/fold + [1 2 3 4])"), "10");
    assert_eq!(run(&mut eval, "(r/fold + + [1 2 3 4])"), "10");
    // Small n forces chunk splits (parallel path).
    assert_eq!(
        run(&mut eval, "(r/fold 8 + + (vec (range 1000)))"),
        "499500"
    );
    // Default n over a large vector agrees too.
    assert_eq!(run(&mut eval, "(r/fold + + (vec (range 2000)))"), "1999000");
    // Seqs, maps, and nil fold through the sequential paths.
    assert_eq!(run(&mut eval, "(r/fold + + (range 10))"), "45");
    assert_eq!(
        run(
            &mut eval,
            "(r/fold (fn ([] {}) ([a b] (merge a b))) (fn ([] {}) ([m k v] (assoc m k v))) {:a 1 :b 2})"
        ),
        "{:a 1, :b 2}"
    );
    assert_eq!(run(&mut eval, "(r/fold + + nil)"), "0");
    // Folders parallelize, reducers reduce.
    assert_eq!(run(&mut eval, "(r/fold + (r/map inc [1 2 3]))"), "9");
    assert_eq!(run(&mut eval, "(r/fold + (r/take 2 [1 2 3]))"), "3");
    assert_eq!(
        run(
            &mut eval,
            "(r/fold 2 + + (r/filter even? (vec (range 100))))"
        ),
        "2450"
    );
}

#[test]
fn cat_foldcat_monoid_append() {
    let mut eval = boot_red();
    assert_eq!(run(&mut eval, "(vec (r/cat [1 2] [3]))"), "[1 2 3]");
    assert_eq!(run(&mut eval, "(r/cat [] [1])"), "[1]");
    assert_eq!(run(&mut eval, "(count (r/cat [1] [2]))"), "2");
    // Cats are lazy seqs (sequential `=` holds against vectors and
    // lists alike on the port, unlike the JVM).
    assert_eq!(run(&mut eval, "(seq? (r/cat [1] [2]))"), "true");
    assert_eq!(run(&mut eval, "(= (r/cat [1] [2]) [1 2])"), "true");
    assert_eq!(run(&mut eval, "(= (r/cat [1] [2]) (list 1 2))"), "true");
    assert_eq!(
        run(&mut eval, "(vec (sort (r/foldcat (r/map inc [3 1 2]))))"),
        "[2 3 4]"
    );
    assert_eq!(run(&mut eval, "((r/monoid + (fn [] 0)) 1 2)"), "3");
    assert_eq!(run(&mut eval, "((r/monoid + (fn [] 0)))"), "0");
    assert_eq!(run(&mut eval, "(r/append! [1] 2)"), "[1 2]");
}

#[test]
fn fj_tasks_round_trip() {
    let mut eval = boot_red();
    // fjfork/fjjoin are private (as upstream) — resolve at runtime.
    assert_eq!(
        run(
            &mut eval,
            "(let [fork @(ns-resolve 'clojure.core.reducers 'fjfork) join @(ns-resolve 'clojure.core.reducers 'fjjoin)] (join (fork (r/fjtask (fn [] 42)))))"
        ),
        "42"
    );
    assert_eq!(run(&mut eval, "(:w6-pool @r/pool)"), ":reducers");
}
