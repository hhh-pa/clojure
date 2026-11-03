// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Borrow-path equivalence for the owned looping natives (#221):
//! every `*_owned` native also answers through `apply` (the
//! borrow `func` shim), with identical values.

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

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

#[test]
fn owned_natives_answer_through_apply() {
    let mut eval = boot();
    // Each row routes one owned native through the borrow-path
    // `apply` shim (same values as the direct owned call).
    let rows: &[(&str, &str)] = &[
        ("(apply reduce + 0 [[1 2 3]])", "6"),
        ("(apply into [] [[1 2]])", "[1 2]"),
        ("(apply transduce (map inc) + 0 [[1 2]])", "5"),
        ("(apply butlast [[1 2 3]])", "(1 2)"),
        ("(apply reverse [[1 2]])", "(2 1)"),
        ("(apply sort [[3 1 2]])", "(1 2 3)"),
        ("(apply sort-by [first [[2 :a] [1 :b]]])", "([1 :b] [2 :a])"),
        ("(apply mapv [inc [1 2]])", "[2 3]"),
        ("(apply filterv [odd? [1 2 3]])", "[1 3]"),
        ("(apply dorun [[1 2]])", "nil"),
        ("(apply doall [[1 2]])", "[1 2]"),
        ("(apply group-by [odd? [1 3]])", "{true [1 3]}"),
        ("(apply frequencies [[1 1]])", "{1 2}"),
        ("(apply every? [odd? [1 3]])", "true"),
        ("(apply some [odd? [2 3]])", "true"),
        ("(apply not-any? [odd? [2 4]])", "true"),
        ("(apply not-every? [odd? [1 2]])", "true"),
        ("(apply set [[1 1]])", "#{1}"),
        ("(apply vec [[1 2]])", "[1 2]"),
        ("(apply count [[1 2 3]])", "3"),
        ("(apply empty? [[]])", "true"),
        ("(apply last [[1 2 3]])", "3"),
        ("(apply nth [[1 2 3] 1])", "2"),
        ("(apply str [1 2])", "\"12\""),
        ("(apply take-last [2 [1 2 3]])", "(2 3)"),
        ("(apply drop-last [[1 2 3]])", "(1 2)"),
    ];
    for (src, want) in rows {
        assert_eq!(run_last(&mut eval, src), *want, "apply row: {src}");
    }
}
