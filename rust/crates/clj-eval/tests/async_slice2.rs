// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slice 2: channel transducers (`chan` xform/ex-handler
//! arities, stepping on the put path, completion on close and
//! closed takes; oracle `docs/gap-123/g123-xform.clj`, pins
//! `docs/gap-123/slice-2.md`).
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

/// A fresh evaluator with `clojure.core.async` required as `a`.
fn boot_async() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.async :as a])");
    eval
}

// --- basic transforms (oracle `m-*`, `f-*`, `c-*`, `k-*`) -------------

#[test]
fn xform_map_filter_comp() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map inc))] [(a/>!! c 1) (a/>!! c 2) (a/<!! c) (a/<!! c)])"
        ),
        "[true true 2 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (filter odd?))] [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3) (a/<!! c) (a/<!! c)])"
        ),
        "[true true true 1 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 4 (mapcat identity))] [(a/>!! c [1 2]) (a/<!! c) (a/<!! c)])"
        ),
        "[true 1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 4 (comp (map inc) (filter odd?)))] [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3) (a/<!! c)])"
        ),
        "[true true true 3]"
    );
}

#[test]
fn xform_windowed_buffers() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/dropping-buffer 1) (mapcat identity))] [(a/>!! c [1 2]) (a/<!! c) (a/alts!! [c] :default :empty)])"
        ),
        "[true 1 [:empty :default]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer 1) (map inc))] [(a/>!! c 1) (a/>!! c 2) (a/<!! c)])"
        ),
        "[true true 3]"
    );
}

#[test]
fn xform_stateful_dedupe_partition() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 4 (dedupe))] [(a/>!! c 1) (a/>!! c 1) (a/>!! c 2) (a/<!! c) (a/<!! c)])"
        ),
        "[true true true 1 2]"
    );
    // The port's `partition-all` has no transducer arity, so the
    // flush tests use an equivalent pair-buffer (same channel
    // paths as the oracle's `p-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p2 (fn [rf] (let [b (atom nil)] (fn ([r] (if (nil? @b) r (let [x @b] (reset! b nil) (rf r [x])))) ([r v] (if (nil? @b) (do (reset! b v) r) (let [x @b] (reset! b nil) (rf r [x v]))))))) c (a/chan 2 p2)] (a/>!! c 1) (a/>!! c 2) (let [t1 (a/<!! c)] (a/>!! c 3) (a/close! c) [t1 (a/<!! c) (a/<!! c)]))"
        ),
        "[[1 2] [3] nil]"
    );
}

// --- reduced closes the channel (oracle `t-*`) --------------------------

#[test]
fn xform_take_closes_channel() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (take 2))] [(a/>!! c 1) (a/>!! c 2) (a/closed? c) (a/<!! c) (a/<!! c) (a/<!! c) (a/>!! c 9)])"
        ),
        "[true true true 1 2 nil false]"
    );
}

#[test]
fn xform_take_parked_abort() {
    let mut eval = boot_async();
    // The reduced put parks behind the full buffer; the take's
    // refill consumes it and aborts (oracle `t-park-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (take 2))] (a/>!! c 1) (let [f (future (a/>!! c 2))] (deref (promise) 800 nil) [(a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout) (a/closed? c)]))"
        ),
        "[1 2 nil true true]"
    );
}

#[test]
fn xform_abort_completes_queued_puts() {
    let mut eval = boot_async();
    // A put parked behind the reduced one still completes `true`
    // (its value is lost to the abort; oracle `t-g-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (take 2))] (a/>!! c 1) (let [f (future (a/>!! c 2))] (deref (promise) 800 nil) (let [g (future (a/>!! c 3))] (deref (promise) 800 nil) [(a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout) (deref g 8000 :timeout)])))"
        ),
        "[1 2 nil true true]"
    );
}

#[test]
fn xform_abort_drains_queued_puts() {
    let mut eval = boot_async();
    // A transducer that keeps emitting after reducing: the refill
    // consumes the reduced put and the abort completes the live
    // putter behind it (a `take` follower would emit nothing and
    // never park, so only values pin that case above).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (atom 0) txf (fn [rf] (fn ([r] (rf r)) ([r v] (let [c (swap! n inc) res (rf r v)] (if (= c 2) (reduced res) res))))) c (a/chan 1 txf)] (a/>!! c :a) (let [f (future (a/>!! c :b))] (deref (promise) 800 nil) (let [g (future (a/>!! c :c))] (deref (promise) 800 nil) [(a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout) (deref g 8000 :timeout)])))"
        ),
        "[:a :b nil true true]"
    );
}

#[test]
fn xform_reduced_serves_queued_takers() {
    let mut eval = boot_async();
    // Queued takers split the buffered value and nil (taker order
    // is racy, so sort — like slice 1's close test).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (take 1)) t1 (future (a/<!! c)) t2 (future (a/<!! c))] (deref (promise) 800 nil) [(a/>!! c 1) (sort [(deref t1 8000 :timeout) (deref t2 8000 :timeout)])])"
        ),
        "[true (nil 1)]"
    );
}

#[test]
fn xform_put_serves_parked_taker() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc)) t (future (a/<!! c))] (deref (promise) 800 nil) [(a/>!! c 1) (deref t 8000 :timeout)])"
        ),
        "[true 2]"
    );
}

#[test]
fn xform_mapcat_parked_multi() {
    let mut eval = boot_async();
    // A multi-emit put parks whole; the refill moves every emit
    // (oracle `q-m-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (mapcat identity))] (a/>!! c [1]) (let [f (future (a/>!! c [2 3]))] (deref (promise) 800 nil) [(a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout)]))"
        ),
        "[1 2 3 true]"
    );
}

#[test]
fn xform_mapcat_take_shortcircuit() {
    let mut eval = boot_async();
    // `mapcat` stops at `reduced`: `:c` never emits (oracle
    // `t-m-*` — the port runs the same transducer code).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (comp (mapcat identity) (take 2)))] (a/>!! c [:a]) (let [f (future (a/>!! c [:b :c]))] (deref (promise) 800 nil) [(a/<!! c) (a/<!! c) (a/<!! c) (a/<!! c) (deref f 8000 :timeout)]))"
        ),
        "[:a :b nil nil true]"
    );
}

// --- ex-handler application (oracle `h-*`) -------------------------------

#[test]
fn xform_exh_nil_drops() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom nil) c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {:n v})))) (fn [t] (reset! seen t) nil))] [(a/>!! c 1) (ex-message @seen) (ex-data @seen) (a/alts!! [c] :default :empty)])"
        ),
        "[true \"boom\" {:n 1} [:empty :default]]"
    );
    assert_eq!(
        class_of(
            &mut eval,
            "(let [seen (atom nil) c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {:n v})))) (fn [t] (reset! seen t) nil))] (a/>!! c 1) @seen)"
        ),
        "clojure.lang.ExceptionInfo"
    );
}

#[test]
fn xform_exh_replacement() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {})))) (fn [t] :fallback))] [(a/>!! c 1) (a/<!! c)])"
        ),
        "[true :fallback]"
    );
}

#[test]
fn xform_exh_false_buffers() {
    let mut eval = boot_async();
    // Only nil drops: `false` buffers like any value (oracle
    // `h-false-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {})))) (fn [t] false))] [(a/>!! c 1) (a/<!! c)])"
        ),
        "[true false]"
    );
}

#[test]
fn xform_default_ex_handler() {
    let mut eval = boot_async();
    // The default handler prints one line (a full trace on the
    // JVM) and drops the value; a `false` ex-handler selects it.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {})))))] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {})))) false)] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
}

#[test]
fn xform_exh_throws() {
    let mut eval = boot_async();
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [v] (throw (ex-info \"boom\" {})))) (fn [t] (throw (ex-info \"from-exh\" {}))))] (a/>!! c 1))"
        ),
        "clojure.lang.ExceptionInfo | from-exh"
    );
}

#[test]
fn xform_completion_throws() {
    let mut eval = boot_async();
    // A throwing completion routes through the ex-handler too: a
    // nil result lets `close!` succeed, a throwing handler fails
    // it (oracle `h-cdone-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [txf (fn [rf] (fn ([r] (throw (ex-info \"c-done\" {}))) ([r v] (rf r v)))) c (a/chan 1 txf (fn [t] nil))] [(a/close! c)])"
        ),
        "[nil]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [txf (fn [rf] (fn ([r] (throw (ex-info \"c-done\" {}))) ([r v] (rf r v)))) c (a/chan 1 txf (fn [t] (throw (ex-info \"from-exh\" {}))))] (a/close! c))"
        ),
        "clojure.lang.ExceptionInfo | from-exh"
    );
}

#[test]
fn xform_evict_default_drops() {
    let mut eval = boot_async();
    // The sliding-zero evict crash swallows through the default
    // handler (oracle `h-evict-*`): the put returns `true` and
    // the value is lost.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer 0) (map inc))] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
}

#[test]
fn xform_evict_replacement_recrash() {
    let mut eval = boot_async();
    // The replacement buffers outside the try, so a re-crash
    // escapes the put (oracle `h-evict-rep-put`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer 0) (map inc) (fn [t] :r))] (a/>!! c 1))"
        ),
        "java.util.NoSuchElementException | "
    );
}

// --- completion arity (oracle `c-*`, `q-d-*`) ----------------------------

#[test]
fn xform_completion_counts() {
    let mut eval = boot_async();
    // A parked take runs no completion; `close!` runs it once and
    // every closed-with-empty take runs it again (oracle `c-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [cnt (atom 0) txf (fn [rf] (fn ([r] (swap! cnt inc) (rf r)) ([r v] (rf r v)))) c (a/chan 1 txf) t (future (a/<!! c))] (deref (promise) 500 nil) (let [parked @cnt] (a/>!! c 1) (let [tv (deref t 8000 :timeout) pre @cnt] (a/close! c) (let [c1 @cnt t1 (a/<!! c) c2 @cnt t2 (a/<!! c) c3 @cnt t3 (a/<!! c) c4 @cnt] [parked tv pre c1 t1 c2 t2 c3 t3 c4]))))"
        ),
        "[0 1 0 1 nil 2 nil 3 nil 4]"
    );
    // Closing twice is a no-op (and runs no completion).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc))] (a/close! c) [(a/close! c) (a/closed? c)])"
        ),
        "[nil true]"
    );
}

#[test]
fn xform_close_skips_completion_with_queued_puts() {
    let mut eval = boot_async();
    // `close!` skips the completion while puts are queued; a
    // later closed take flushes instead (oracle `c-flush`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p2 (fn [rf] (let [b (atom nil)] (fn ([r] (if (nil? @b) r (let [x @b] (reset! b nil) (rf r [x])))) ([r v] (if (nil? @b) (do (reset! b v) r) (let [x @b] (reset! b nil) (rf r [x v]))))))) c (a/chan 1 p2)] (a/>!! c 1) (a/>!! c 2) (let [f (future (a/>!! c 3))] (deref (promise) 500 nil) (let [f3 (deref f 100 :pending)] (let [g (future (a/>!! c 4))] (deref (promise) 800 nil) (let [h (future (a/>!! c 5))] (deref (promise) 500 nil) (let [h5 (deref h 100 :pending)] (a/close! c) [(a/<!! c) (a/<!! c) (a/<!! c) (a/<!! c) f3 h5 (deref g 8000 :timeout)]))))))"
        ),
        "[[1 2] [3 4] [5] nil true true true]"
    );
}

#[test]
fn xform_drain_skips_completion() {
    let mut eval = boot_async();
    // Draining the buffer runs no completion (oracle `q-d-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [cnt (atom 0) txf (fn [rf] (fn ([r] (swap! cnt inc) (rf r)) ([r v] (rf r v)))) c (a/chan 1 txf)] (a/>!! c 1) (a/close! c) (let [pre @cnt t1 (a/<!! c) mid @cnt t2 (a/<!! c) end @cnt] [pre t1 mid t2 end]))"
        ),
        "[1 1 1 nil 2]"
    );
}

#[test]
fn xform_completion_reduced_ignored() {
    let mut eval = boot_async();
    // A `reduced` completion is not an abort (oracle `c-red-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [txf (fn [rf] (fn ([r] (reduced r)) ([r v] (rf r v)))) c (a/chan 1 txf)] (a/>!! c 1) (a/close! c) [(a/<!! c) (a/<!! c)])"
        ),
        "[1 nil]"
    );
}

// --- errors, edges, and stepping granularity -----------------------------

#[test]
fn xform_chan_errors() {
    let mut eval = boot_async();
    assert_eq!(
        uncaught(&mut eval, "(a/chan 1 42)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    // The transducer applies before the buffer validates.
    assert_eq!(
        uncaught(&mut eval, "(a/chan \"x\" 42)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/chan 0 (map inc))"),
        "java.lang.AssertionError | Assert failed: fixed buffers must have size > 0\n(pos? n)"
    );
    assert_eq!(
        uncaught(&mut eval, "(a/chan \"x\" (map inc))"),
        "java.lang.IllegalArgumentException | No implementation of method: :full? of protocol: #'clojure.core.async.impl.protocols/Buffer found for class: java.lang.String"
    );
}

#[test]
fn xform_degenerate_nonvector() {
    let mut eval = boot_async();
    // A step that never calls its `rf` buffers nothing, but the
    // put still completes `true` (oracle `e-degen-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (fn [rf] (fn ([r] r) ([r v] 42))))] [(a/>!! c 1) (a/alts!! [c] :default :empty)])"
        ),
        "[true [:empty :default]]"
    );
}

#[test]
fn xform_filtered_full_completes() {
    let mut eval = boot_async();
    // Documented deviation: a filtered put behind a full buffer
    // completes at once (the JVM parks it until a refill steps
    // it — oracle `b-full-drop` is `:pending` there).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (filter odd?))] [(a/>!! c 1) (a/>!! c 2) (a/<!! c) (a/alts!! [c] :default :empty)])"
        ),
        "[true true 1 [:empty :default]]"
    );
}

#[test]
fn xform_no_step_after_close() {
    let mut eval = boot_async();
    // Puts on a closed channel neither step nor buffer (oracle
    // `b-closed-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [cnt (atom 0) c (a/chan 1 (map (fn [v] (swap! cnt inc) v)))] (a/>!! c 1) (a/close! c) [(a/>!! c 2) @cnt])"
        ),
        "[false 1]"
    );
    // A close racing a slow step wins: the put reports `false`
    // and its emits are discarded (the JVM holds its mutex
    // across the step, so the put would win there instead).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map (fn [v] (deref (promise) 1500 nil) v))) f (future (a/>!! c 1))] (deref (promise) 500 nil) (a/close! c) [(deref f 8000 :timeout) (a/<!! c)])"
        ),
        "[false nil]"
    );
}

#[test]
fn xform_abort_step_divergence() {
    let mut eval = boot_async();
    // Documented deviation: puts step at arrival, so a put later
    // lost to an abort still ran its transducer (the JVM steps
    // queued puts at refill and never steps it — oracle `q-*`
    // records `[0]` / `[0 1]` there). Values agree regardless.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom []) txf (comp (map (fn [v] (swap! calls conj v) v)) (take 2)) c (a/chan 1 txf)] (a/>!! c 0) (let [f (future (a/>!! c 1))] (deref (promise) 800 nil) (let [g (future (a/>!! c 2))] (deref (promise) 800 nil) (let [parked @calls t1 (a/<!! c) after @calls fv (deref f 8000 :timeout) gv (deref g 8000 :timeout) t2 (a/<!! c) t3 (a/<!! c)] [parked t1 after fv gv t2 t3]))))"
        ),
        "[[0 1 2] 0 [0 1 2] true true 1 nil]"
    );
}

// --- alts!! with transducers (oracle `a-*`) ------------------------------

#[test]
fn xform_alts_full_default() {
    let mut eval = boot_async();
    // A put to a full transducer channel parks, so `:default`
    // wins (oracle `a-full-dflt`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc))] (a/>!! c 0) (a/alts!! [[c 41]] :default :d))"
        ),
        "[:d :default]"
    );
}

#[test]
fn xform_alts_closed() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc))] (a/close! c) (let [[v p] (a/alts!! [c])] [v (= p c)]))"
        ),
        "[nil true]"
    );
    // A closed take that flushes completes with the value.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (fn [rf] (fn ([r] (conj r :flushed)) ([r v] (rf r v)))))] (a/close! c) [(a/<!! c) (let [[v p] (a/alts!! [c])] [v (= p c)])])"
        ),
        "[:flushed [:flushed true]]"
    );
}

#[test]
fn xform_alts_stale_multi() {
    let mut eval = boot_async();
    // A defaulted multi-emit put never resurrects (oracle
    // `a-stale-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (mapcat identity))] (a/>!! c [1]) [(a/alts!! [[c [2 3]]] :default :d) (a/<!! c) (a/alts!! [c] :default :empty)])"
        ),
        "[[:d :default] 1 [:empty :default]]"
    );
}

#[test]
fn xform_alts_put_ready_and_woken() {
    let mut eval = boot_async();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc))] (let [[v p] (a/alts!! [[c 41]])] [v (= p c) (a/<!! c)]))"
        ),
        "[true true 42]"
    );
    // A parked put-alts is woken by a later take (oracle `a-w-*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1 (map inc))] (a/>!! c 0) (let [f (future (a/alts!! [[c 41]]))] (deref (promise) 800 nil) (let [t1 (a/<!! c) [v p] (deref f 8000 :timeout) t2 (a/<!! c)] [t1 v (= p c) t2])))"
        ),
        "[1 true true 42]"
    );
}
