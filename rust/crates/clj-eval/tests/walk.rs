// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W1 `clojure.walk` smoke tests: require + the primary entry points
//! (`walk`, `postwalk`, `prewalk`, the replace/key/macroexpand
//! helpers) against upstream-documented behavior.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

/// Eval, returning the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

fn require_walk(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.walk :as w])").unwrap();
}

#[test]
fn walk_postwalk_nested() {
    let mut eval = boot();
    require_walk(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(w/postwalk #(if (number? %) (inc %) %) {:a [1 2] :b '(3)})"
        ),
        "{:a [2 3], :b (4)}"
    );
    // Identity over a map round-trips the map.
    assert_eq!(
        run(&mut eval, "(w/postwalk identity {:a [1 {:b 2}]})"),
        "{:a [1 {:b 2}]}"
    );
}

#[test]
fn walk_entry_shape_preserved() {
    let mut eval = boot();
    require_walk(&mut eval);
    // The MapEntry branch rebuilds a real entry (W1 port), never a
    // bare vector.
    assert_eq!(
        run(
            &mut eval,
            "(map-entry? (w/postwalk identity (find {:a 1} :a)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(w/postwalk #(if (number? %) (inc %) %) (find {:a 1} :a))"
        ),
        "[:a 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(map-entry? (first (w/postwalk identity {:a 1})))"
        ),
        "true"
    );
}

#[test]
fn walk_pre_vs_post_order() {
    let mut eval = boot();
    require_walk(&mut eval);
    // Post-order: leaves before their parents.
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (w/postwalk #(do (swap! seen conj %) %) [1 [2]]) @seen)"
        ),
        "[1 2 [2] [1 [2]]]"
    );
    // Pre-order: parents before their leaves.
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (w/prewalk #(do (swap! seen conj %) %) [1 [2]]) @seen)"
        ),
        "[[1 [2]] 1 [2] 2]"
    );
}

#[test]
fn walk_replace_helpers() {
    let mut eval = boot();
    require_walk(&mut eval);
    assert_eq!(
        run(&mut eval, "(w/postwalk-replace {:a :b} {:a [1 {:a 2}]})"),
        "{:b [1 {:b 2}]}"
    );
    assert_eq!(
        run(&mut eval, "(w/prewalk-replace {:a 1} [:a :b])"),
        "[1 :b]"
    );
    // Root-first vs leaves-first differ when a child replacement
    // enables a parent match.
    assert_eq!(
        run(&mut eval, "(w/prewalk-replace {:a :b [:b] :c} [:a])"),
        "[:b]"
    );
    assert_eq!(
        run(&mut eval, "(w/postwalk-replace {:a :b [:b] :c} [:a])"),
        ":c"
    );
}

#[test]
fn walk_key_helpers() {
    let mut eval = boot();
    require_walk(&mut eval);
    assert_eq!(
        run(&mut eval, "(w/keywordize-keys {\"a\" {\"b\" 1}})"),
        "{:a {:b 1}}"
    );
    assert_eq!(
        run(&mut eval, "(w/stringify-keys {:a {:b 1}})"),
        "{\"a\" {\"b\" 1}}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(w/transform-keys {:a 1 :b {:c 2}} (fn [k] (keyword \"n\" (name k))))"
        ),
        "#:n{:a 1, :b #:n{:c 2}}"
    );
    // Non-map leaves pass through untouched.
    assert_eq!(run(&mut eval, "(w/keywordize-keys [\"a\" 1])"), "[\"a\" 1]");
}

#[test]
fn walk_macroexpand_all() {
    let mut eval = boot();
    require_walk(&mut eval);
    assert_eq!(
        run(&mut eval, "(w/macroexpand-all '(when true 1))"),
        "(if true (do 1))"
    );
    assert_eq!(
        run(&mut eval, "(w/macroexpand-all '(-> 1 (+ 2)))"),
        "(+ 1 2)"
    );
}

#[test]
fn walk_records_keep_type() {
    let mut eval = boot();
    require_walk(&mut eval);
    eval.eval_source("(defrecord WalkR [x])").unwrap();
    // The IRecord branch conjs back onto the record (stays a
    // record, values walked).
    assert_eq!(
        run(
            &mut eval,
            "(w/postwalk #(if (number? %) (* 10 %) %) (->WalkR 2))"
        ),
        "#user.WalkR{:x 20}"
    );
}
