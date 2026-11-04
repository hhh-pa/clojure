// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Guarded integer-loop accelerator (`Evaluator::try_int_loop`):
//! counted `loop`s over `if`/comparison/`recur` run tagless in
//! `i64`s; overflow, non-`Int` state, exotic shapes, and any
//! redefinition bail to the interpreter with exactly its behavior
//! (including the JVM's `long overflow` throws, probed on JVM
//! master — and unlike the JVM's `:inline` arithmetic, every
//! inlined call observes redefinition, like the evaluator's own
//! direct calls).

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
fn int_loop_sum() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 100) (recur (inc i) (+ s i)) s))"
        ),
        "4950"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i (long 0) s (long 0)] (if (< i 1000000) (recur (inc i) (+ s i)) s))"
        ),
        "499999500000"
    );
}

#[test]
fn int_loop_counts_down() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 10 s 0] (if (> i 0) (recur (dec i) (+ s i)) s))"
        ),
        "55"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 5 a 1] (if (> i 1) (recur (dec i) (* a i)) a))"
        ),
        "120"
    );
}

#[test]
fn int_loop_zero_iterations() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 10 s 7] (if (< i 3) (recur (inc i) (+ s i)) s))"
        ),
        "7"
    );
}

#[test]
fn int_loop_nested_and_nary() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 1 s 0] (if (< i 6) (recur (inc i) (+ s (* i i))) s))"
        ),
        "55"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 10) (recur (+ i 1 1) (+ s i 100)) s))"
        ),
        "520"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 100] (if (< i 3) (recur (inc i) (- s i 1)) s))"
        ),
        "94"
    );
    assert_eq!(
        run_last(&mut eval, "(loop [i 0] (if (< i 5 10) (recur (inc i)) i))"),
        "5"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 5] (if (< i 1) (recur (inc i) (+)) s))"
        ),
        "0"
    );
    // Nested steps run the general machine (leaf-built shapes never
    // reach it): nested `*` over a constant, nested `inc`, nested
    // `*` under `-`, unary `-` over a compound, nested `dec`, and a
    // zero-arg op nested as an operand (declines the leaf fold).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 1 s 0] (if (< i 4) (recur (inc i) (+ s (* i 2))) s))"
        ),
        "12"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 3) (recur (inc i) (+ (inc i) s)) s))"
        ),
        "6"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 1 s 100] (if (< i 4) (recur (inc i) (- s (* i i))) s))"
        ),
        "86"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 5] (if (< i 2) (recur (inc i) (- (+ s i))) s))"
        ),
        "4"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 3 s 0] (if (> i 0) (recur (dec i) (+ s (dec i))) s))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 3) (recur (inc i) (+ (+) s i)) s))"
        ),
        "3"
    );
}

#[test]
fn int_loop_exit_polarity() {
    let mut eval = boot();
    // `recur` in the `else` branch, `=` test, keyword exit value.
    assert_eq!(
        run_last(&mut eval, "(loop [i 0] (if (= i 5) :done (recur (inc i))))"),
        ":done"
    );
    // Single-arg tests hold (like the interpreter); the exit sits in
    // the `then` branch.
    assert_eq!(
        run_last(&mut eval, "(loop [i 10] (if (< i) :t (recur (dec i))))"),
        ":t"
    );
    // The exit evaluates with the final bindings (unary `-`).
    assert_eq!(
        run_last(&mut eval, "(loop [i 0] (if (< i 2) (recur (inc i)) (- i)))"),
        "-2"
    );
}

#[test]
fn int_loop_outer_const() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n 50] (loop [i 0 s 0] (if (< i n) (recur (inc i) (+ s i)) s)))"
        ),
        "1225"
    );
}

#[test]
fn int_loop_exit_recur_continues() {
    let mut eval = boot();
    // The exit branch recurs back in (the surfaced `recur` resumes
    // the fast loop with its values).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (< i 5) (recur (inc i)) (if (< i 10) (recur (inc i)) i)))"
        ),
        "10"
    );
}

#[test]
fn int_loop_bail_runs_effects_once() {
    let mut eval = boot();
    // An overflow bail after exit rounds with effects resumes the
    // slow loop from the current state (it must not rerun from
    // entry: the two `swap!`s run exactly once, then the overflow
    // still throws like the interpreter).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (try (loop [i 0] (if (< i 2) (do (swap! a inc) (recur (inc i))) (recur (+ i 9223372036854775807)))) (catch ArithmeticException e [:threw @a])))"
        ),
        "[:threw 2]"
    );
    // A root write (epoch bump) after exit rounds likewise resumes
    // instead of rerunning (the `def` in round 0 bumps the epoch;
    // round 1 resumes slow and the loop completes with three
    // increments, not four).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (loop [i 0] (if (< i 3) (do (swap! a inc) (def int-loop-bail-pin i) (recur (inc i))) @a)))"
        ),
        "3"
    );
    // An exotic exit `recur` resumes from its own values (the
    // `:boom` binds `i`, like the slow path — the test then throws
    // exactly as evaluating `(< :boom 2)` directly, and the two
    // `swap!`s run once).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (try (loop [i 0] (if (< i 2) (do (swap! a inc) (recur (if (< i 1) (inc i) :boom))) @a)) (catch Exception e [:caught @a])))"
        ),
        "[:caught 2]"
    );
}

#[test]
fn int_loop_exit_capture_keeps_round() {
    // Exit rounds reuse one env rebound in place — a round that
    // captures its env in a closure and recurs still observes its
    // own round (the shared frame falls back to a fresh bind).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom [])] (loop [i 0] (if (< i 3) (do (swap! a conj (fn [] i)) (recur (inc i))) (mapv (fn [f] (f)) @a))))"
        ),
        "[0 1 2]"
    );
}

#[test]
fn int_loop_redefs_observed() {
    // Unlike the JVM's `:inline` arithmetic (which ignores
    // `with-redefs`), the accelerator observes redefinition like
    // the evaluator's own direct calls: any binding declines the
    // plan and the slow path answers.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [+ -] (loop [i 0 s 0] (if (< i 10) (recur (inc i) (+ s i)) s)))"
        ),
        "-45"
    );
}

#[test]
fn int_loop_overflow_throws() {
    let mut eval = boot();
    for src in [
        "(loop [i 0 s 0] (if (< i 3) (recur (inc i) (+ s 9223372036854775800)) s))",
        "(loop [i -9223372036854775808] (if (= i -9223372036854775808) (recur (- i)) :no))",
        "(loop [i 0 s 2] (if (< i 1) (recur (inc i) (* s 9223372036854775807)) s))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ArithmeticException | long overflow",
            "{src}"
        );
    }
}

#[test]
fn int_loop_slow_shapes() {
    let mut eval = boot();
    // Non-`Int` entries decline the fast path.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s \"x\"] (if (< i 3) (recur (inc i) (str s i)) s))"
        ),
        "\"x012\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 1/2] (if (< i 2) (recur (inc i) (+ s 1)) s))"
        ),
        "5/2"
    );
    assert_eq!(
        run_last(&mut eval, "(loop [i 0.0] (if (< i 3) (recur (+ i 1.0)) i))"),
        "3.0"
    );
    // Destructuring binds decline (multi-slot patterns).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [[a b] [0 0]] (if (< a 3) (recur [(inc a) (+ b a)]) b))"
        ),
        "3"
    );
    // Non-arithmetic test operands decline.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (= (str i) \"3\") :three (recur (inc i))))"
        ),
        ":three"
    );
}

#[test]
fn int_loop_slow_errors() {
    let mut eval = boot();
    // Arity failures inside steps decline (the interpreter raises).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (loop [i 0] (if (< i 3) (recur (do (-) (inc i))) i)) (catch clojure.lang.ArityException e :arity))"
        ),
        ":arity"
    );
    // Zero-arg tests decline (the interpreter raises).
    assert_eq!(
        uncaught(&mut eval, "(loop [i 0] (if (<) :t :f))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/<"
    );
    // Throws from the exit branch propagate.
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 0] (if (< i 3) (recur (inc i)) (throw (ex-info \"boom\" {}))))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
}

#[test]
fn int_loop_two_million_canary() {
    // Exercises the fast path at scale (milliseconds fast, seconds
    // slow — a silent deopt shows up as a slow suite, not a wrong
    // answer).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 2000000) (recur (inc i) (+ s i)) s))"
        ),
        "1999999000000"
    );
}

#[test]
fn int_loop_bail_pins() {
    let mut eval = boot();
    // Overflow inside the loop TEST bails (the interpreter throws).
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 9223372036854775807] (if (< i (+ i 1)) (recur i) :done))"
        ),
        "java.lang.ArithmeticException | long overflow"
    );
    // Non-`Global` step and test heads decline (slow path decides).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (< i 2) (recur ((fn [a] (inc a)) i)) i))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if ((fn [a b] (< a b)) i 2) (recur (inc i)) i))"
        ),
        "2"
    );
    // A non-comparison `Direct` test declines (truthy exit here).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 5] (if (+ i 1) :immediate (recur (inc i))))"
        ),
        ":immediate"
    );
    // Outer non-`Int` operands decline (the interpreter raises).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [n :kw] (loop [i 0] (if (< i n) (recur (inc i)) i)))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    // An exit-branch `recur` with a non-`Int` value bails, then
    // the interpreter raises at the re-entered test.
    assert_eq!(
        uncaught(
            &mut eval,
            "(loop [i 0] (if (< i 5) (recur (inc i)) (recur :oops)))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn int_loop_guard_observes_bindings_and_closures() {
    let mut eval = boot();
    // A thread binding on a loop-called var declines the plan (the
    // slow path observes the binding): a dynamic alias of `+`
    // shares the `+` native (hence its fast tag) but carries its
    // own thread binding.
    assert_eq!(
        run_last(
            &mut eval,
            "(def ^:dynamic myplus clojure.core/+) (binding [myplus (fn [a b] (+ a b 100))] (loop [i 0] (if (< i 3) (recur (myplus i 1)) i)))"
        ),
        "101"
    );
    // A redefinition to a non-native (a closure) declines the plan
    // too (the guard only trusts the analyzed root); the redefined
    // root computes through the slow path.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [orig +] (with-redefs [+ (fn [a b] (orig a b 100))] (loop [i 0] (if (< i 3) (recur (+ i 1)) i))))"
        ),
        "101"
    );
    // A redefinition to plain data declines the plan as well (the
    // guard only inlines `Extension` roots); the slow path calls
    // the data and raises exactly like the interpreter.
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-redefs [+ 5] (loop [i 0] (if (< i 3) (recur (+ i 1)) i)))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

#[test]
fn int_loop_leaf_edge_shapes() {
    let mut eval = boot();
    // `inc`/`dec` over a zero-operand leaf (`(+)` folds to a bare
    // counted op, not a `Var`/`Const`): the leaf read misses, so
    // the general machine computes it.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (< (dec (+)) i (inc (+))) (recur (inc i)) :done))"
        ),
        ":done"
    );
    // A leaf-built step that ends in a non-counted op (nested
    // `inc`) skips the leaf fold for the general machine.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (< i 5) (recur (inc (inc i))) i))"
        ),
        "6"
    );
    // A counted `*` over a non-flat operand (a nested `+`)
    // declines the leaf fold (operand count mismatch) for the
    // general machine.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 j 3] (if (< 0 j) (recur (* (+ i 1) j) (dec j)) i))"
        ),
        "9"
    );
}
