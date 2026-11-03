// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W1 `clojure.data` smoke tests: require + `diff` against the
//! docstring-oracle comparison rules (equal, maps subdiffed, sets
//! never subdiffed, sequentials by index as vectors, everything
//! else — including strings — an atom).

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

fn require_data(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.data :as d])").unwrap();
}

#[test]
fn data_equal_and_atoms() {
    let mut eval = boot();
    require_data(&mut eval);
    // For equal a and b, return [nil nil a].
    assert_eq!(run(&mut eval, "(d/diff 1 1)"), "[nil nil 1]");
    assert_eq!(
        run(&mut eval, "(d/diff {:a [1]} {:a [1]})"),
        "[nil nil {:a [1]}]"
    );
    // Unequal atoms.
    assert_eq!(run(&mut eval, "(d/diff 1 2)"), "[1 2 nil]");
    assert_eq!(run(&mut eval, "(d/diff nil nil)"), "[nil nil nil]");
    // Everything else (including strings!) is an atom.
    assert_eq!(run(&mut eval, "(d/diff \"a\" \"b\")"), "[\"a\" \"b\" nil]");
    assert_eq!(run(&mut eval, "(d/diff :a :b)"), "[:a :b nil]");
    // Different partitions fall back to the atom diff.
    assert_eq!(run(&mut eval, "(d/diff {:a 1} [1])"), "[{:a 1} [1] nil]");
    assert_eq!(run(&mut eval, "(d/diff nil {})"), "[nil {} nil]");
    assert_eq!(run(&mut eval, "(d/diff #{1} [1])"), "[#{1} [1] nil]");
}

#[test]
fn data_maps_subdiff() {
    let mut eval = boot();
    require_data(&mut eval);
    assert_eq!(
        run(&mut eval, "(d/diff {:a 1 :b 2} {:a 1 :c 3})"),
        "({:b 2} {:c 3} {:a 1})"
    );
    assert_eq!(
        run(&mut eval, "(d/diff {:a {:b 1}} {:a {:b 2}})"),
        "({:a {:b 1}} {:a {:b 2}} nil)"
    );
    assert_eq!(
        run(&mut eval, "(d/diff {:a 1} {:a 1 :b 2})"),
        "(nil {:b 2} {:a 1})"
    );
    // Upstream quirk, preserved: map diffs reduce through
    // (doall (map merge ...)), so the answer is a seq, not a vector.
    assert_eq!(run(&mut eval, "(vector? (d/diff {:a 1} {:b 2}))"), "false");
    assert_eq!(run(&mut eval, "(seq? (d/diff {:a 1} {:b 2}))"), "true");
}

#[test]
fn data_sequentials_by_index() {
    let mut eval = boot();
    require_data(&mut eval);
    // Sequential things compare by index; results come back vectors.
    assert_eq!(
        run(&mut eval, "(d/diff [1 2 3] [1 9])"),
        "[[nil 2 3] [nil 9] [1]]"
    );
    assert_eq!(
        run(&mut eval, "(d/diff '(1 2) '(1 3))"),
        "[[nil 2] [nil 3] [1]]"
    );
    assert_eq!(
        run(&mut eval, "(d/diff [1 2] [1 2 3])"),
        "[nil [nil nil 3] [1 2]]"
    );
}

#[test]
fn data_sets_never_subdiff() {
    let mut eval = boot();
    require_data(&mut eval);
    assert_eq!(run(&mut eval, "(d/diff #{1 2} #{2 3})"), "[#{1} #{3} #{2}]");
    assert_eq!(run(&mut eval, "(d/diff #{1} #{1 2})"), "[nil #{2} #{1}]");
    assert_eq!(
        run(&mut eval, "(d/diff #{{:a 1}} #{{:a 2}})"),
        "[#{{:a 1}} #{{:a 2}} nil]"
    );
}

#[test]
fn data_records_diff_as_maps() {
    let mut eval = boot();
    require_data(&mut eval);
    eval.eval_source("(defrecord DiffR [x])").unwrap();
    // Records are maps upstream (IPersistentMap); the port agrees.
    assert_eq!(
        run(&mut eval, "(d/diff (->DiffR 1) (->DiffR 2))"),
        "({:x 1} {:x 2} nil)"
    );
    assert_eq!(
        run(&mut eval, "(d/diff (->DiffR 1) (->DiffR 1))"),
        "[nil nil #user.DiffR{:x 1}]"
    );
}

#[test]
fn data_protocol_vars_exist() {
    let mut eval = boot();
    require_data(&mut eval);
    // Implementation-detail protocols, kept for API shape.
    assert_eq!(
        run(&mut eval, "(some? (resolve 'clojure.data/Diff))"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(some? (resolve 'clojure.data/EqualityPartition))"
        ),
        "true"
    );
}
