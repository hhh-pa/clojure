// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W1 `clojure.zip` smoke tests: require + navigation, editing,
//! enumeration, and the error messages, against
//! upstream-documented behavior.

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

fn require_zip(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.zip :as z])").unwrap();
    eval.eval_source("(def zz (z/vector-zip [[1 2] [3]]))")
        .unwrap();
}

#[test]
fn zip_navigation() {
    let mut eval = boot();
    require_zip(&mut eval);
    assert_eq!(run(&mut eval, "(z/node zz)"), "[[1 2] [3]]");
    assert_eq!(run(&mut eval, "(z/branch? zz)"), "true");
    assert_eq!(run(&mut eval, "(z/children zz)"), "([1 2] [3])");
    assert_eq!(run(&mut eval, "(-> zz z/down z/node)"), "[1 2]");
    assert_eq!(run(&mut eval, "(-> zz z/down z/right z/node)"), "[3]");
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/right z/left z/node)"),
        "[1 2]"
    );
    assert_eq!(run(&mut eval, "(-> zz z/down z/up z/node)"), "[[1 2] [3]]");
    assert_eq!(run(&mut eval, "(z/up zz)"), "nil");
    assert_eq!(run(&mut eval, "(-> zz z/down z/path)"), "[[[1 2] [3]]]");
    assert_eq!(run(&mut eval, "(-> zz z/down z/right z/lefts)"), "([1 2])");
    assert_eq!(run(&mut eval, "(-> zz z/down z/lefts)"), "nil");
    assert_eq!(run(&mut eval, "(-> zz z/down z/right z/rights)"), "nil");
    assert_eq!(run(&mut eval, "(-> zz z/down z/leftmost z/node)"), "[1 2]");
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/right z/leftmost z/node)"),
        "[1 2]"
    );
    assert_eq!(run(&mut eval, "(-> zz z/down z/rightmost z/node)"), "[3]");
    assert_eq!(run(&mut eval, "(-> zz z/down z/down z/node)"), "1");
    assert_eq!(run(&mut eval, "(z/branch? (-> zz z/down z/down))"), "false");
}

#[test]
fn zip_edits() {
    let mut eval = boot();
    require_zip(&mut eval);
    assert_eq!(
        run(&mut eval, "(-> zz z/down (z/replace 9) z/root)"),
        "[9 [3]]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> zz z/down z/down z/right (z/edit + 10) z/root)"
        ),
        "[[1 12] [3]]"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down (z/insert-right 9) z/root)"),
        "[[1 2] 9 [3]]"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/right (z/insert-left 9) z/root)"),
        "[[1 2] 9 [3]]"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down (z/insert-child 0) z/node)"),
        "[0 1 2]"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down (z/append-child 9) z/node)"),
        "[1 2 9]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> zz z/down z/down z/right (z/replace 9) z/root)"
        ),
        "[[1 9] [3]]"
    );
}

#[test]
fn zip_walk_next_prev() {
    let mut eval = boot();
    require_zip(&mut eval);
    // Depth-first next-walk collects every node, root first.
    assert_eq!(
        run(
            &mut eval,
            "(loop [loc zz out []] (if (z/end? loc) out (recur (z/next loc) (conj out (z/node loc)))))"
        ),
        "[[[1 2] [3]] [1 2] 1 2 [3] 3]"
    );
    assert_eq!(run(&mut eval, "(-> zz z/next z/next z/node)"), "1");
    assert_eq!(
        run(&mut eval, "(-> zz z/next z/next z/next z/prev z/node)"),
        "1"
    );
    assert_eq!(run(&mut eval, "(z/prev zz)"), "nil");
    assert_eq!(run(&mut eval, "(z/end? zz)"), "false");
    // next stays at the end once there.
    assert_eq!(
        run(
            &mut eval,
            "(let [e (-> zz z/next z/next z/next z/next z/next z/next)] [(z/end? e) (z/end? (z/next e))])"
        ),
        "[true true]"
    );
}

#[test]
fn zip_remove() {
    let mut eval = boot();
    require_zip(&mut eval);
    // Remove returns the loc preceding the removal in depth-first
    // order (here 2 precedes [3]).
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/right z/remove z/node)"),
        "2"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/right z/remove z/root)"),
        "[[1 2]]"
    );
    assert_eq!(
        run(&mut eval, "(-> zz z/down z/down z/remove z/root)"),
        "[[2] [3]]"
    );
}

#[test]
fn zip_seq_and_xml() {
    let mut eval = boot();
    require_zip(&mut eval);
    assert_eq!(
        run(&mut eval, "(-> (z/seq-zip '(1 (2))) z/down z/right z/node)"),
        "(2)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> (z/seq-zip '(1 2)) z/down z/right (z/replace 9) z/root)"
        ),
        "(1 9)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> (z/xml-zip {:tag :a :content [{:tag :b :content [\"hi\"]}]}) z/down z/node)"
        ),
        "{:tag :b, :content [\"hi\"]}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> (z/xml-zip {:tag :a :content [\"x\"]}) z/down (z/replace \"y\") z/root)"
        ),
        "{:tag :a, :content [\"y\"]}"
    );
}

#[test]
fn zip_errors() {
    let mut eval = boot();
    require_zip(&mut eval);
    // Upstream messages, verbatim; the W1 port throws them via
    // ex-info (no Exception ctor), still caught by (catch Exception).
    assert_eq!(
        run(
            &mut eval,
            "(try (z/children (-> zz z/down z/down)) (catch Exception e (ex-message e)))"
        ),
        "\"called children on a leaf node\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (z/insert-left zz :x) (catch Exception e (ex-message e)))"
        ),
        "\"Insert at top\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (z/insert-right zz :x) (catch Exception e (ex-message e)))"
        ),
        "\"Insert at top\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (z/remove zz) (catch Exception e (ex-message e)))"
        ),
        "\"Remove at top\""
    );
}
