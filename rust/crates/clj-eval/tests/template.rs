// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W1 `clojure.template` smoke tests: require + the docstring-oracle
//! entry points (`apply-template`, `do-template`).

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

fn require_template(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.template :as t])")
        .unwrap();
}

#[test]
fn template_apply_template_docstring() {
    let mut eval = boot();
    require_template(&mut eval);
    // Docstring oracle: (apply-template '[x] '(+ x x) '[2]) ;=> (+ 2 2)
    assert_eq!(
        run(&mut eval, "(t/apply-template '[x] '(+ x x) '[2])"),
        "(+ 2 2)"
    );
    assert_eq!(
        run(&mut eval, "(t/apply-template '[a b] '(+ a b) '[1 2])"),
        "(+ 1 2)"
    );
    assert_eq!(
        run(&mut eval, "(t/apply-template '[x y] '([x] [y x]) '[1 2])"),
        "([1] [2 1])"
    );
}

#[test]
fn template_do_template_expand_and_eval() {
    let mut eval = boot();
    require_template(&mut eval);
    // Docstring oracle: (macroexpand '(do-template [x y] (+ y x) 2 4 3 5))
    // ;=> (do (+ 4 2) (+ 5 3))
    assert_eq!(
        run(
            &mut eval,
            "(macroexpand '(t/do-template [x y] (+ y x) 2 4 3 5))"
        ),
        "(do (+ 4 2) (+ 5 3))"
    );
    assert_eq!(run(&mut eval, "(t/do-template [x] (+ x 1) 1 2)"), "3");
    assert_eq!(run(&mut eval, "(t/do-template [x y] (+ x y) 1 2 3 4)"), "7");
}
