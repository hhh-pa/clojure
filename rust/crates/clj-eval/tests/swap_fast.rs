// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `swap!` int-arithmetic pins: known natives over int state fuse
//! to one locked read-compute-publish, with identical values —
//! while validators, watches, non-int state, and overflow fall
//! back to the generic CAS loop.

use clj_eval::{Evaluator, boot};
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

#[test]
fn swap_fast_values() {
    let mut eval = boot();
    for (src, want) in [
        ("(let [a (atom 0)] (swap! a + 5) @a)", "5"),
        ("(let [a (atom 10)] (swap! a - 3) @a)", "7"),
        ("(let [a (atom 10)] (swap! a *) @a)", "10"),
        ("(let [a (atom 6)] (swap! a * 7) @a)", "42"),
        ("(let [a (atom 6)] [(swap! a inc) @a])", "[7 7]"),
        ("(let [a (atom 6)] [(swap! a dec) @a])", "[5 5]"),
        ("(let [a (atom 6)] [(swap! a +) @a])", "[6 6]"),
        ("(let [a (atom 6)] [(swap! a -) @a])", "[-6 -6]"),
        ("(let [a (atom 0)] (swap-vals! a + 41))", "[0 41]"),
        // A hot loop over the fast path.
        (
            "(let [a (atom 0)] (doseq [i (range 100000)] (swap! a + i)) @a)",
            "4999950000",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn swap_fast_fallbacks() {
    let mut eval = boot();
    // Overflow throws (generic path), like `+` itself — state
    // untouched.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 9223372036854775807)] (try (swap! a + 1) (catch ArithmeticException e [:overflow @a])))"
        ),
        "[:overflow 9223372036854775807]"
    );
    // Non-int state runs the generic application.
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1.5)] (swap! a + 1) @a)"),
        "2.5"
    );
    // Non-int extras run the generic application.
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 0)] (swap! a + 1.5) @a)"),
        "1.5"
    );
    // Validators still veto.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1 :validator pos?)] (try (swap! a - 5) (catch Throwable e [:vetoed @a])))"
        ),
        "[:vetoed 1]"
    );
    // Watches still fire with old/new.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0) log (atom [])] (add-watch a :w (fn [_ _ o n] (swap! log conj [o n]))) (swap! a + 3) @log)"
        ),
        "[[0 3]]"
    );
    // Closures (even core-named ones in other namespaces) apply
    // generically — user fns observe their bodies.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (swap! a (fn [o x] (+ o x 100)) 1) @a)"
        ),
        "101"
    );
}
