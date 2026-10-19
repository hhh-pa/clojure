// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D seq spot pins: `mapcat` splices (found via the `not`-fix
//! ripple — the old shape never concatenated), plus the
//! take/drop-while backlog rows.

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
fn reduced_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(reduced? (reduced 1))"), "true");
    assert_eq!(run_last(&mut eval, "(reduced? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(reduced? nil)"), "false");
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce (fn [a b] (if (> a 1) (reduced :stop) (+ a b))) [1 2 3])"
        ),
        ":stop"
    );
}

#[test]
fn mapcat_rows() {
    let mut eval = boot();
    // Each outer item is seq'd and spliced (≡ `(apply concat
    // (map f xs))`); nil/empty items vanish.
    assert_eq!(run_last(&mut eval, "(mapcat list [1 2 3])"), "(1 2 3)");
    assert_eq!(
        run_last(
            &mut eval,
            "(mapcat (fn [x] (if (odd? x) (list x))) [1 2 3])"
        ),
        "(1 3)"
    );
    assert_eq!(run_last(&mut eval, "(mapcat list nil)"), "()");
    assert_eq!(run_last(&mut eval, "(mapcat (fn [x] nil) [1 2])"), "()");
    assert_eq!(
        run_last(&mut eval, "(mapcat vector [1 2] \"ab\")"),
        "(1 \\a 2 \\b)"
    );
    // Non-seqable items fail like `concat` would.
    assert_eq!(
        uncaught(&mut eval, "(doall (mapcat (fn [x] x) [1]))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // One arg is the (out-of-scope) transducer arity.
    assert_eq!(
        uncaught(&mut eval, "(mapcat list)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/mapcat"
    );
}

#[test]
fn while_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(take-while odd? [1 3 4 5])"), "(1 3)");
    assert_eq!(run_last(&mut eval, "(drop-while odd? [1 3 4 5])"), "(4 5)");
    assert_eq!(
        run_last(&mut eval, "(split-with odd? [1 3 4 5])"),
        "[(1 3) (4 5)]"
    );
}

#[test]
fn xform_divergence_rows() {
    let mut eval = boot();
    // The 1-arg transducer arities are out of scope: arity errors
    // naming the `clojure.core/F` display form.
    for f in ["map", "filter", "remove", "keep", "keep-indexed"] {
        let src = format!("({f} list)");
        assert_eq!(
            uncaught(&mut eval, &src),
            format!(
                "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/{f}"
            ),
            "{src}"
        );
    }
    for f in ["distinct", "dedupe"] {
        let src = format!("({f})");
        assert_eq!(
            uncaught(&mut eval, &src),
            format!(
                "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/{f}"
            ),
            "{src}"
        );
    }
}
