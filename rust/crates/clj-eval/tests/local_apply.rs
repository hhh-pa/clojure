// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Local-head closure-call pins: calls through a local bound to a
//! closure skip generic dispatch (shared arity selection), with
//! identical values and errors — while locals bound to anything
//! else fall through untouched.

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
fn local_apply_values() {
    let mut eval = boot();
    for (src, want) in [
        ("(let [f (fn [x] (+ x 1))] (f 1) (f 2))", "3"),
        (
            "(let [fib (fn fib [n] (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))] (fib 10))",
            "55",
        ),
        ("(let [f (fn [x & xs] [x xs])] (f 1 2 3))", "[1 (2 3)]"),
        (
            "(let [f (fn ([x] x) ([x y] (+ x y)))] [(f 1) (f 1 2)])",
            "[1 3]",
        ),
        (
            "(let [f (fn [x] {:pre [(pos? x)]} (* x 2))] [(f 3) (try (f -1) (catch Throwable e :pre))])",
            "[6 :pre]",
        ),
        ("(let [f +] (f 1 2))", "3"),
        ("(map (let [f inc] f) [1 2])", "(2 3)"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn local_apply_fallthrough() {
    // Non-closure locals take the generic path (values unchanged).
    let mut eval = boot();
    for (src, want) in [
        ("(let [m {:a 1}] (m :a))", "1"),
        ("(let [v [10 20]] (v 1))", "20"),
        ("(let [k :a] (k {:a 1}))", "1"),
        ("(let [s #{1 2}] (s 2))", "2"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn single_arg_call_and_recur() {
    // One-parameter closures bind through the inline call frame
    // (values identical), and `recur` rebinds that frame in place.
    let mut eval = boot();
    for (src, want) in [
        ("((fn [x] (+ x 1)) 41)", "42"),
        (
            "((fn cd [n] (if (zero? n) :done (recur (dec n)))) 100000)",
            ":done",
        ),
        (
            "(let [s ((fn grab [n] (if (zero? n) [(fn [] n)] (recur (dec n)))) 3)] ((first s)))",
            "0",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

/// Run `f` on a 256 MiB worker: deep recursion trips the
/// catchable guard there, never a real stack abort.
fn on_worker<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .expect("spawn worker")
        .join()
        .expect("worker panicked")
}

#[test]
fn local_apply_depth_guard() {
    // Deep local-head recursion throws the catchable
    // `StackOverflowError` (like the generic path), never aborts.
    on_worker(|| {
        let mut eval = boot();
        for src in [
            "(try ((fn f [n] (if (zero? n) 0 (f (dec n)))) 100000) (catch StackOverflowError e :so))",
            "(try (let [f (fn f2 [n] (if (zero? n) 0 (f2 (dec n))))] (f 100000)) (catch StackOverflowError e :so))",
        ] {
            assert_eq!(run_last(&mut eval, src), ":so", "{src}");
        }
    });
}

#[test]
fn local_apply_arity_errors() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(try (let [f (fn [x] x)] (f 1 2)) (catch clojure.lang.ArityException e :arity))"
        ),
        ":arity"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (let [f (fn [x] x)] (f)) (catch clojure.lang.ArityException e :arity))"
        ),
        ":arity"
    );
}
