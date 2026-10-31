// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slice 1: channels + blocking ops probe replay (`chan`
//! with fixed/dropping/sliding buffers, `>!!`/`<!!`, `close!`/
//! `closed?`, `alts!!`; oracle `docs/gap-123/g123-*.clj`, pins
//! `docs/gap-123/probes.md`).
//!
//! Blocking futures always deref with a timeout (a stuck op fails
//! the test instead of hanging the suite); short sleeps ride
//! `(deref (promise) ms nil)`.

use clj_eval::{Evaluator, SourceError, boot, jvm_class_of};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| pr(&realized(eval, v)))
        .collect()
}

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// The JVM class of the last value (`class` is out of scope, so
/// class assertions read Rust-side and never force).
fn class_of(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    jvm_class_of(vs.last().unwrap())
}

/// Assert the `#object` rendering with matching hex identities
/// (per-process ids, never pinned literally).
fn assert_object_shape(pr: &str, class: &str) {
    let prefix = format!("#object[{class} 0x");
    let quoted = format!("\"{class}@");
    assert!(
        pr.starts_with(&prefix),
        "missing #object prefix for {class}: {pr}"
    );
    assert!(pr.ends_with("\"]"), "missing #object suffix: {pr}");
    let hex = pr[prefix.len()..].split(' ').next().unwrap();
    assert!(
        !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "bad id hex: {pr}"
    );
    assert!(
        pr.contains(&format!("{quoted}{hex}\"")),
        "mismatched toString hex: {pr}"
    );
}

const CHAN_CLASS: &str = "clojure.core.async.impl.channels.ManyToManyChannel";

/// A fresh evaluator with `clojure.core.async` required as `a`.
fn boot_async() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.async :as a])");
    eval
}

// --- namespace loading --------------------------------------------------

#[test]
fn require_installs_async_ns() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(require 'clojure.core.async)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(some? (find-ns 'clojure.core.async))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (loaded-libs) 'clojure.core.async)"),
        "true"
    );
    // Plain require refers nothing into `user`.
    assert_eq!(run_last(&mut eval, "(resolve 'chan)"), "nil");
    // `:reload` reinstalls idempotently.
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.core.async] :reload)"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(some? (ns-resolve 'clojure.core.async 'chan))"),
        "true"
    );
}

#[test]
fn require_alias_and_refer() {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.async :as a])");
    assert_eq!(
        run_last(&mut eval, "(= (a/<!! (doto (a/chan 1) (a/>!! :x))) :x)"),
        "true"
    );
    let mut eval = boot();
    run(
        &mut eval,
        "(require '[clojure.core.async :refer [chan <!! >!!]])",
    );
    assert_eq!(run_last(&mut eval, "(>!! (chan 1) :y)"), "true");
    assert_eq!(run_last(&mut eval, "(<!! (doto (chan 1) (>!! :z)))"), ":z");
}

#[test]
fn qualified_use_without_require_fails() {
    let mut eval = boot();
    // A compile-time resolution failure (the namespace does not
    // exist until required).
    match eval.eval_source("clojure.core.async/chan") {
        Err(e) => assert_eq!(
            format!("{e}"),
            "Syntax error compiling at (user:1:1).\njava.lang.RuntimeException | No such namespace: clojure.core.async"
        ),
        Ok(vs) => panic!("expected a resolution failure, got {vs:?}"),
    }
}

// --- chan shapes (oracle g123-chan `c-*`) --------------------------------

#[test]
fn chan_shapes_and_classes() {
    let mut eval = boot_async();
    assert_eq!(class_of(&mut eval, "(a/chan)"), CHAN_CLASS);
    assert_eq!(class_of(&mut eval, "(a/chan 3)"), CHAN_CLASS);
    assert_eq!(class_of(&mut eval, "(a/chan nil)"), CHAN_CLASS);
    assert_eq!(class_of(&mut eval, "(a/chan (a/buffer 2))"), CHAN_CLASS);
    assert_eq!(
        class_of(&mut eval, "(a/buffer 2)"),
        "clojure.core.async.impl.buffers.FixedBuffer"
    );
    assert_eq!(
        class_of(&mut eval, "(a/dropping-buffer 2)"),
        "clojure.core.async.impl.buffers.DroppingBuffer"
    );
    assert_eq!(
        class_of(&mut eval, "(a/sliding-buffer 2)"),
        "clojure.core.async.impl.buffers.SlidingBuffer"
    );
    assert_object_shape(&run_last(&mut eval, "(a/chan)"), CHAN_CLASS);
    assert_object_shape(
        &run_last(&mut eval, "(a/dropping-buffer 1)"),
        "clojure.core.async.impl.buffers.DroppingBuffer",
    );
    assert_eq!(run_last(&mut eval, "(let [c (a/chan)] (= c c))"), "true");
    assert_eq!(run_last(&mut eval, "(= (a/chan) (a/chan))"), "false");
    assert_eq!(
        run_last(&mut eval, "(let [c (a/chan)] (= (hash c) (hash c)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (a/buffer 1) (a/buffer 1))"),
        "false"
    );
}

// --- buffers (oracle `f-*`, `d-*`, `s-*`) ---------------------------------

#[test]
fn fixed_buffer_fill_drain() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2)] [(a/>!! c :a) (a/>!! c :b) (a/<!! c) (a/<!! c)])"
        ),
        "[true true :a :b]"
    );
}

#[test]
fn dropping_buffer_overflow() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/dropping-buffer 2))] [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3) (a/<!! c) (a/<!! c)])"
        ),
        "[true true true 1 2]"
    );
}

#[test]
fn sliding_buffer_overflow() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer 2))] [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3) (a/<!! c) (a/<!! c)])"
        ),
        "[true true true 2 3]"
    );
}

// --- close (oracle `x-*`, `u-*`) ------------------------------------------

#[test]
fn close_drains_then_nil() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2)] (a/>!! c :x) [(a/close! c) (a/closed? c) (a/>!! c :y) (a/<!! c) (a/<!! c) (a/close! c) (a/closed? c)])"
        ),
        "[nil true false :x nil nil true]"
    );
}

#[test]
fn close_empty_unbuffered() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (a/close! c) [(a/<!! c) (a/>!! c 1) (a/closed? c)])"
        ),
        "[nil false true]"
    );
    assert_eq!(run_last(&mut eval, "(a/closed? (a/chan))"), "false");
}

// --- errors (oracle `e-*`) -------------------------------------------------

#[test]
fn chan_errors() {
    let mut eval = boot_async();
    assert_eq!(
        uncaught(&mut eval, "(a/>!! (a/chan 1) nil)"),
        "java.lang.IllegalArgumentException | Can't put nil on channel"
    );
    for src in ["(a/buffer 0)", "(a/buffer -3)", "(a/chan 0)"] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.AssertionError | Assert failed: fixed buffers must have size > 0\n(pos? n)",
            "{src}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(a/<!! 42)"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/>!! :kw 1)"),
        "java.lang.IllegalArgumentException | No implementation of method: :put! of protocol: #'clojure.core.async.impl.protocols/WritePort found for class: clojure.lang.Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/<!! nil)"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/close! \"s\")"),
        "java.lang.IllegalArgumentException | No implementation of method: :close! of protocol: #'clojure.core.async.impl.protocols/Channel found for class: java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/closed? \"s\")"),
        "java.lang.IllegalArgumentException | No implementation of method: :closed? of protocol: #'clojure.core.async.impl.protocols/Channel found for class: java.lang.String"
    );
    // Eager (the JVM stores the garbage and fails on first use).
    assert_eq!(
        uncaught(&mut eval, "(a/chan \"x\")"),
        "java.lang.IllegalArgumentException | No implementation of method: :full? of protocol: #'clojure.core.async.impl.protocols/Buffer found for class: java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/<! (a/chan))"),
        "java.lang.AssertionError | Assert failed: <! used not in (go ...) block\nnil"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/>! (a/chan) 1)"),
        "java.lang.AssertionError | Assert failed: >! used not in (go ...) block\nnil"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/alts! [(a/chan)])"),
        "java.lang.AssertionError | Assert failed: alts! used not in (go ...) block\nnil"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/chan 1 2 3 4)"),
        "clojure.lang.ArityException | Wrong number of args (4) passed to: clojure.core.async/chan"
    );
}

#[test]
fn chan_transducer_arities() {
    let mut eval = boot_async();
    // Slice 2 accepts transducers (deep pins live in async_slice2).
    assert_eq!(
        run_last(&mut eval, "(a/<!! (doto (a/chan 1 (map inc)) (a/>!! 41)))"),
        "42"
    );
    // The transducer-without-buffer assert still fires first.
    assert_eq!(
        uncaught(&mut eval, "(a/chan nil (map inc))"),
        "java.lang.AssertionError | Assert failed: buffer must be supplied when transducer is\nbuf-or-n"
    );
    // An ex-handler alone is a silent no-op (as on the JVM).
    assert_eq!(
        run_last(
            &mut eval,
            "(a/<!! (doto (a/chan 1 nil (fn [t])) (a/>!! :x)))"
        ),
        ":x"
    );
}

// --- buffer sizes beyond plain ints ---------------------------------------

#[test]
fn fractional_buffer_size_ceils() {
    let mut eval = boot_async();
    // `1.5` behaves as `2` (`(>= size 1.5)`); the third put would
    // block, so probe fullness with a defaulted `alts!!` put.
    for src in ["(a/chan 1.5)", "(a/chan 3/2)", "(a/chan 1.5M)"] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!(
                    "(let [c {src}] (a/>!! c 1) (a/>!! c 2) (a/alts!! [[c 3]] :default :full))"
                )
            ),
            "[:full :default]",
            "{src}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(a/chan ##NaN)"),
        "java.lang.AssertionError | Assert failed: fixed buffers must have size > 0\n(pos? n)"
    );
    // `##Inf` is positive and unbounded.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan ##Inf)] (a/>!! c 1) (a/>!! c 2) [(a/<!! c) (a/<!! c)])"
        ),
        "[1 2]"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/dropping-buffer \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn zero_and_negative_windowed_buffers() {
    let mut eval = boot_async();
    // Dropping-zero accepts and drops everything.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/dropping-buffer 0))] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
    // Sliding-zero evicts from empty (the JVM crashes the same
    // way; its default ex-handler then swallows the trace and
    // drops the value — the port surfaces the error).
    assert_eq!(
        uncaught(&mut eval, "(a/>!! (a/chan (a/sliding-buffer 0)) 1)"),
        "java.util.NoSuchElementException | "
    );
    // Negative dropping never retains; negative sliding never evicts.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/dropping-buffer -1))] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer -1))] (a/>!! c 1) (a/<!! c))"
        ),
        "1"
    );
}

// --- blocking rendezvous (oracle g123-block) -------------------------------

#[test]
fn unbuffered_rendezvous_both_orders() {
    let mut eval = boot_async();
    // Taker first (either order rendezvouses the same way).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) f (future (a/<!! c))] [(a/>!! c 42) (deref f 8000 :timeout)])"
        ),
        "[true 42]"
    );
    // Putter first.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) f (future (a/>!! c :v))] [(a/<!! c) (deref f 8000 :timeout)])"
        ),
        "[:v true]"
    );
}

#[test]
fn pending_take_woken_by_close_with_nil() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) f (future (a/<!! c))] (deref (promise) 1000 nil) (a/close! c) (deref f 8000 :timeout))"
        ),
        "nil"
    );
}

#[test]
fn pending_put_survives_close() {
    let mut eval = boot_async();
    // The close lands while the put is parked; a later taker
    // releases it (oracle `k-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) f (future (a/>!! c 7))] (deref (promise) 1000 nil) (a/close! c) [(a/<!! c) (deref f 8000 :timeout) (a/<!! c)])"
        ),
        "[7 true nil]"
    );
    // Same through a full fixed buffer (oracle `b-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1) _ (a/>!! c :first) f (future (a/>!! c :second))] (deref (promise) 1000 nil) (a/close! c) [(a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout)])"
        ),
        "[:first :second nil true]"
    );
}

#[test]
fn close_hands_each_taker_one_buffered_value() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1) _ (a/>!! c :only) f1 (future (a/<!! c)) f2 (future (a/<!! c))] (deref (promise) 1500 nil) (a/close! c) (sort [(deref f1 8000 :t1) (deref f2 8000 :t2)]))"
        ),
        "(nil :only)"
    );
}

#[test]
fn many_to_many_fan_in() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 4) puts (doall (for [i (range 8)] (future (a/>!! c i)))) takes (doall (for [_ (range 8)] (future (a/<!! c))))] [(sort (map (fn [f] (deref f 8000 :t)) puts)) (sort (map (fn [f] (deref f 8000 :t)) takes))])"
        ),
        "[(true true true true true true true true) (0 1 2 3 4 5 6 7)]"
    );
    // Unbuffered fan-in through pure rendezvous.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) takes (doall (for [_ (range 4)] (future (a/<!! c))))] (deref (promise) 1500 nil) (doseq [i (range 4)] (a/>!! c i)) (sort (map (fn [f] (deref f 8000 :t)) takes)))"
        ),
        "(0 1 2 3)"
    );
}

// --- alts!! (oracle g123-alts) ---------------------------------------------

#[test]
fn alts_ready_ops() {
    let mut eval = boot_async();
    // Ready take wins.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :ready) (let [[v p] (a/alts!! [c1 c2])] [v (= p c1)]))"
        ),
        "[:ready true]"
    );
    // Ready put wins with `true`; the value lands.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan 1)] (let [[v p] (a/alts!! [[c2 :v]])] [v (= p c2) (a/<!! c2)]))"
        ),
        "[true true :v]"
    );
    // A closed channel completes immediately with nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (a/close! c1) (let [[v p] (a/alts!! [c1 c2])] [v (= p c1)]))"
        ),
        "[nil true]"
    );
    // A put to a closed channel completes with `false`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (a/close! c2) (let [[v p] (a/alts!! [[c2 :v]])] [v (= p c2)]))"
        ),
        "[false true]"
    );
}

#[test]
fn alts_default_and_priority() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] [(a/alts!! [c1 c2] :default :dflt) (a/alts!! [c1 c2] :default nil)])"
        ),
        "[[:dflt :default] [nil :default]]"
    );
    // A ready op beats `:default`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan)] (a/>!! c1 :ready) (let [[v p] (a/alts!! [c1 c2] :default :dflt)] [v (= p c1)]))"
        ),
        "[:ready true]"
    );
    // `:priority` tries in order.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (let [[v p] (a/alts!! [c1 c2] :priority true)] [v (= p c1)]))"
        ),
        "[:first true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (let [[v p] (a/alts!! [c2 c1] :priority true)] [v (= p c2)]))"
        ),
        "[:second true]"
    );
}

#[test]
fn alts_blocking_woken() {
    let mut eval = boot_async();
    // A parked take-alts is woken by a later put.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan) f (future (a/alts!! [c1 c2]))] (deref (promise) 1000 nil) (a/>!! c2 :late) (let [[v p] (deref f 8000 :timeout)] [v (= p c2)]))"
        ),
        "[:late true]"
    );
    // A parked put-alts is woken by a later take.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) f (future (a/alts!! [[c1 :pv]]))] (deref (promise) 1000 nil) (let [took (a/<!! c1) [v p] (deref f 8000 :timeout)] [took v (= p c1)]))"
        ),
        "[:pv true true]"
    );
}

#[test]
fn alts_ops_and_options() {
    let mut eval = boot_async();
    // A `MapEntry` is a 2-vector put-pair (JVM `vector?`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1) e (first {c :via-entry})] (let [[[v p]] [(a/alts!! [e])]] [v (= p c) (a/<!! c)]))"
        ),
        "[true true :via-entry]"
    );
    // A set of ports works (the JVM vecs first).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :in-set) (let [[v p] (a/alts!! #{c})] [v (= p c)]))"
        ),
        "[:in-set true]"
    );
    // Unknown options are ignored.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :x) (let [[v p] (a/alts!! [c] :bogus 1 :default :d)] [v (= p c)]))"
        ),
        "[:x true]"
    );
}

#[test]
fn alts_errors() {
    let mut eval = boot_async();
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [])"),
        "java.lang.AssertionError | Assert failed: alts must have at least one channel operation\n(pos? (count ports))"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [[(a/chan 1) nil]])"),
        "java.lang.AssertionError | Assert failed: can't put nil on channel\n(some? (port 1))"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [42])"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [(a/chan 1)] :default)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :default"
    );
    // A list op is not a vector — a take from a list.
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [(list (a/chan 1) :v)])"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: clojure.lang.PersistentList"
    );
    // A short vector op is the `(port 1)` IOOBE.
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! [[(a/chan 1)]])"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // Non-sequential ports are the stock ISeq error.
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! 42)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // `nil` ports vec to `[]` (the empty-ports assert).
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! nil)"),
        "java.lang.AssertionError | Assert failed: alts must have at least one channel operation\n(pos? (count ports))"
    );
    // A string ports seqs to chars (a take from a char).
    assert_eq!(
        uncaught(&mut eval, "(a/alts!! \"x\")"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: java.lang.Character"
    );
}
