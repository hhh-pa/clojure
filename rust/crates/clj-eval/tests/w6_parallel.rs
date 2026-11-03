// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W6: `clojure.parallel` smoke tests — the deprecated ForkJoin
//! wrapper's public API over the record-based port (oracle examples
//! from the file's own comment block included).

use clj_eval::{Evaluator, SourceError, boot};
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

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

fn boot_par() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.parallel :as p])");
    eval
}

#[test]
fn parallel_ns_loads() {
    let mut eval = boot_par();
    assert_eq!(
        run(
            &mut eval,
            "[(ifn? p/par) (ifn? p/pany) (ifn? p/pmax) (ifn? p/pmin) (ifn? p/psummary) (ifn? p/preduce) (ifn? p/pvec) (ifn? p/pdistinct) (ifn? p/psort) (ifn? p/pfilter-nils) (ifn? p/pfilter-dupes)]"
        ),
        "[true true true true true true true true true true true]"
    );
}

#[test]
fn par_attaches_bound_filter_map() {
    let mut eval = boot_par();
    assert_eq!(run(&mut eval, "(p/pvec (p/par [3 1 2]))"), "[3 1 2]");
    assert_eq!(run(&mut eval, "(p/pvec [1 2])"), "[1 2]");
    // An already-parallel array returns as-is.
    assert_eq!(
        run(&mut eval, "(= (p/par [1]) (p/par (p/par [1])))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(p/pvec (p/par [1 2 3 4] :map inc))"),
        "[2 3 4 5]"
    );
    assert_eq!(
        run(&mut eval, "(p/pvec (p/par [1 2 3 4] :filter even?))"),
        "[2 4]"
    );
    assert_eq!(
        run(&mut eval, "(p/pvec (p/par [1 2 3 4] :bound [1 3]))"),
        "[2 3]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(p/pvec (p/par [1 2 3 4] :bound [1 4] :filter even? :map #(* % 10)))"
        ),
        "[20 40]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(p/pvec (p/par [5 6 7] :map-index (fn [x i] (* x i))))"
        ),
        "[0 6 14]"
    );
    // Oracle comment-block examples.
    assert_eq!(
        run(
            &mut eval,
            "(p/pvec (p/par [11 2 3 2] :filter-index (fn [x i] (> i x))))"
        ),
        "[2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(p/pvec (p/par [11 2 3 2] :filter-with [(fn [x y] (> y x)) [110 2 33 2]]))"
        ),
        "[11 3]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(p/preduce + 0 (p/par [11 2 3 2] :filter-with [< [110 2 33 2]]))"
        ),
        "14"
    );
}

#[test]
fn aggregates_match_sequential() {
    let mut eval = boot_par();
    assert_eq!(run(&mut eval, "(p/pany [42])"), "42");
    assert_eq!(
        run(&mut eval, "[(p/pmax [3 1 2]) (p/pmin [3 1 2])]"),
        "[3 1]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(p/pmax ["a" "ccc" "bb"] (fn [a b] (compare (count a) (count b))))"#
        ),
        r#""ccc""#
    );
    assert_eq!(
        run(&mut eval, "(p/psummary [3 1 2 1])"),
        "{:min 1, :max 3, :size 4, :min-index 1, :max-index 0}"
    );
    assert_eq!(run(&mut eval, "(p/preduce + 0 [1 2 3 2 1])"), "9");
    assert_eq!(run(&mut eval, "(p/preduce + 0 (range 1000))"), "499500");
    assert_eq!(run(&mut eval, "(p/preduce + 0 [])"), "0");
    assert_eq!(run(&mut eval, "(p/pmax [])"), "nil");
}

#[test]
fn collection_ops_match_sequential() {
    let mut eval = boot_par();
    assert_eq!(
        run(&mut eval, "(p/pvec (p/par [3 1 2] :map inc))"),
        "[4 2 3]"
    );
    assert_eq!(run(&mut eval, "(p/pdistinct [1 2 3 2 1])"), "[1 2 3]");
    assert_eq!(run(&mut eval, "(p/psort [3 1 2])"), "[1 2 3]");
    assert_eq!(
        run(
            &mut eval,
            r#"(p/psort ["bb" "a" "ccc"] (fn [a b] (compare (count a) (count b))))"#
        ),
        r#"["a" "bb" "ccc"]"#
    );
    assert_eq!(run(&mut eval, "(p/pfilter-nils [1 nil 2 nil])"), "[1 2]");
    assert_eq!(run(&mut eval, "(p/pfilter-dupes [1 1 2 2 1])"), "[1 2 1]");
}

#[test]
fn par_rejects_bad_and_misordered_ops() {
    let mut eval = boot_par();
    let err = uncaught(&mut eval, "(p/par [1] :bogus 1)");
    assert!(err.contains("Unsupported par op: :bogus"), "{err}");
    let err = uncaught(&mut eval, "(p/par [1] :map inc :filter even?)");
    assert!(err.contains("bound/filter/map order"), "{err}");
    let err = uncaught(&mut eval, "(p/pvec (p/par [1 2] :map-with [+ [1]]))");
    assert!(err.contains("must match in size"), "{err}");
}
