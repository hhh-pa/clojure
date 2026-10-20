// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S8 coverage round 3: close the `lazy.rs` line gaps left by
//! `lazy_gaps2.rs` (17 `DA:0` lines). Every test below executes at
//! least one currently-open line; see `/tmp/lazy-r3-draft.md` for the
//! line-to-test map and for the provably-dead arms (restructure
//! proposals, no test can reach them). All expectations are
//! JVM-oracle-verified.

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
fn lazy3_rest_cons_list_tail() {
    let mut eval = boot();
    // A cons over a non-empty list rests to that list (covers the
    // non-empty-List-tail path in `rest_tail_value`).
    assert_eq!(run_last(&mut eval, "(rest (cons 1 (quote (2))))"), "(2)");
}

#[test]
fn lazy3_seq_iseq_reduced() {
    let mut eval = boot();
    // `seq` over a non-seqable extension value (covers the trailing
    // `iseq_err` in `seq_value_extension`).
    assert_eq!(
        uncaught(&mut eval, "(seq (reduced 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Reduced"
    );
}

#[test]
fn lazy3_seq_subvector() {
    let mut eval = boot();
    // `seq` over a raw subvector view (covers the `as_subvector` arm
    // in `seq_value_extension`).
    assert_eq!(run_last(&mut eval, "(seq (subvec [1 2 3] 1 2))"), "(2)");
}

#[test]
fn lazy3_keys_repeat_rest() {
    let mut eval = boot();
    // Entry-view rest over a repeat checks the repeat tail (covers
    // the repeat arm in `keys_empty_extension`).
    assert_eq!(
        run_last(&mut eval, "(first (rest (keys (repeat (first {:a 1})))))"),
        ":a"
    );
}

#[test]
fn lazy3_keys_cons_range_rest() {
    let mut eval = boot();
    // Entry-view rest over a cons-over-range checks the range tail
    // (covers the range arm in `keys_empty_extension`); the head
    // read then fails like the JVM's `KeySeq.first` cast.
    assert_eq!(
        uncaught(&mut eval, "(first (rest (keys (cons [:a 1] (range 3)))))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn lazy3_step_iseq_reduced() {
    let mut eval = boot();
    // Stepping a non-seqable extension value (covers the fallthrough
    // past the subvector check in `step_unit_extension`).
    assert_eq!(
        uncaught(&mut eval, "(seq (map inc (reduced 1)))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Reduced"
    );
}
