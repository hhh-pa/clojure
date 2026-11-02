// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The embedded `clojure.set` port: every public var runs through
//! `require` and matches the JVM oracle labels (probes under
//! clojure-1.12.0 — see `host_members.rs` for the probe setup).
//! Set/map results compare with `=`, never pr-strings (print
//! order is implementation-defined).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
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

fn require_set(eval: &mut Evaluator) {
    run(eval, "(require '[clojure.set :as set])");
}

/// `(= expected actual)` as a boolean pr-string (order-free).
fn eq(eval: &mut Evaluator, expected: &str, actual: &str) -> String {
    run(eval, &format!("(= {expected} {actual})"))
}

#[test]
fn set_algebra_rows() {
    let mut eval = boot();
    require_set(&mut eval);
    assert_eq!(eq(&mut eval, "#{1 2}", "(set/union #{1} #{2})"), "true");
    assert_eq!(eq(&mut eval, "#{1}", "(set/union nil #{1})"), "true");
    assert_eq!(eq(&mut eval, "#{}", "(set/union)"), "true");
    assert_eq!(
        eq(&mut eval, "#{1}", "(set/difference #{1 2} #{2 3})"),
        "true"
    );
    assert_eq!(eq(&mut eval, "#{1}", "(set/difference #{1} nil)"), "true");
    assert_eq!(
        eq(&mut eval, "#{2}", "(set/intersection #{1 2} #{2 3})"),
        "true"
    );
    assert_eq!(eq(&mut eval, "#{}", "(set/intersection #{1} #{2})"), "true");
    assert_eq!(run(&mut eval, "(set/subset? #{1} #{1 2})"), "true");
    assert_eq!(run(&mut eval, "(set/subset? #{1 3} #{1 2})"), "false");
    assert_eq!(run(&mut eval, "(set/superset? #{1 2} #{2})"), "true");
}

#[test]
fn set_relational_rows() {
    let mut eval = boot();
    require_set(&mut eval);
    assert_eq!(
        eq(&mut eval, "#{1 3}", "(set/select odd? #{1 2 3})"),
        "true"
    );
    assert_eq!(
        eq(
            &mut eval,
            "#{{:a 1} {:a 3}}",
            "(set/project [{:a 1 :b 2} {:a 3 :b 4}] [:a])"
        ),
        "true"
    );
    assert_eq!(
        eq(
            &mut eval,
            "{:b 2 :A 1}",
            "(set/rename-keys {:a 1 :b 2} {:a :A})"
        ),
        "true"
    );
    assert_eq!(
        eq(&mut eval, "#{{:b 1}}", "(set/rename #{{:a 1}} {:a :b})"),
        "true"
    );
    assert_eq!(
        eq(
            &mut eval,
            "{{:a 1} #{{:a 1 :b 1}} {:a 2} #{{:a 2 :b 2}}}",
            "(set/index [{:a 1 :b 1} {:a 2 :b 2}] [:a])"
        ),
        "true"
    );
    assert_eq!(
        eq(&mut eval, "{1 :a 2 :b}", "(set/map-invert {:a 1 :b 2})"),
        "true"
    );
    assert_eq!(
        eq(
            &mut eval,
            "#{{:a 1 :b 2}}",
            "(set/join #{{:a 1}} #{{:a 1 :b 2}})"
        ),
        "true"
    );
    assert_eq!(
        eq(
            &mut eval,
            "#{{:a 1 :b 1 :c 2}}",
            "(set/join #{{:a 1}} #{{:b 1 :c 2}} {:a :b})"
        ),
        "true"
    );
}
