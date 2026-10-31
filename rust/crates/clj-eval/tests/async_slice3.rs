// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slice 3: `go` blocks + parking ops (`go`/`go-loop`,
//! `<!`/`>!`/`alts!` inside `go`, `alt!`/`alt!!`; oracle
//! `docs/gap-123/g123-go.clj` + `docs/gap-123/g123-goalt.clj`,
//! pins `docs/gap-123/slice-3.md`).
//!
//! Go-channel takes always ride the `goo` prelude helper (a
//! future + deref timeout — a stuck op fails the test instead of
//! hanging the suite); short sleeps ride `(deref (promise) ms
//! nil)`. `timeout` is a later #123 slice, so probes racing a
//! timeout use a never-ready `(a/chan)` in its place (same
//! non-ready take port). Go-internal failures print the D6
//! one-liner on the go thread (stderr, unasserted here — the
//! shape pins through `default_ex_message`); values assert the
//! `[nil true]` settle.

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

/// The compile chain of a direct analyzer failure.
fn compile_err(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src).unwrap_err() {
        SourceError::Compile(e) => e.chain(),
        other => panic!("expected a compile error, got {other}"),
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

/// A fresh evaluator with `clojure.core.async` required as `a`
/// plus the `goo` take-with-timeout helper.
fn boot_async() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.async :as a])");
    run(
        &mut eval,
        "(defn goo [g] (deref (future (a/<!! g)) 8000 :timeout))",
    );
    eval
}

// --- go basics (oracle `g-*`) --------------------------------------------

#[test]
fn go_basics() {
    let mut eval = boot_async();
    // `g-ret-class`, `g-ch-pr`: a go block returns a channel.
    assert_eq!(class_of(&mut eval, "(a/go 42)"), CHAN_CLASS);
    assert_object_shape(&run_last(&mut eval, "(a/go 42)"), CHAN_CLASS);
    // `g-val`, `g-closed-after`: the value delivers, then close.
    assert_eq!(run_last(&mut eval, "(goo (a/go 42))"), "42");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go 42)] (goo g) [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
    // `g-nil-body` (nil skipped), `g-false-body` (`false` delivers),
    // `g-empty`, `g-multi` (last value).
    assert_eq!(
        run_last(&mut eval, "(let [g (a/go nil)] [(goo g) (a/closed? g)])"),
        "[nil true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go false)] [(goo g) (goo g) (a/closed? g)])"
        ),
        "[false nil true]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [g (a/go)] [(goo g) (a/closed? g)])"),
        "[nil true]"
    );
    assert_eq!(run_last(&mut eval, "(goo (a/go :a :b :c))"), ":c");
    // `g-buf1`: the go channel buffers one value, then closes.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go :v)] (deref (promise) 500 nil) (let [[v1 p1] (a/alts!! [g] :default :pending) [v2 p2] (a/alts!! [g] :default :pending)] [v1 (= p1 g) v2 (= p2 g)]))"
        ),
        "[:v true nil true]"
    );
    // An empty form and a non-symbol head walk uniformly.
    assert_eq!(run_last(&mut eval, "(goo (a/go ()))"), "()");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go ((fn [x] x) (a/<! c)))))"
        ),
        ":v"
    );
}

#[test]
fn go_runs_off_thread() {
    let mut eval = boot_async();
    // `g-thread-diff`/`g-async`: no thread interop exists in the
    // port, so off-thread execution pins behaviorally. Reaching
    // the close proves `go` returned while its body still parked
    // (inline execution would wedge on the empty channel); the
    // close then releases the parked body.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/<! c))] (a/close! c) [(goo g) :returned])"
        ),
        "[nil :returned]"
    );
    // The body runs while the main thread continues (the deliver
    // lands after `go` returned, before the take completes).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) p (promise) g (a/go (deliver p :body-ran) (a/<! c))] (let [flag (deref p 8000 :timeout)] (a/close! c) [flag (goo g)]))"
        ),
        "[:body-ran nil]"
    );
    // `g-async`: the rendezvous completes across the boundary.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/<! c))] (a/>!! c :v) (goo g))"
        ),
        ":v"
    );
}

#[test]
fn go_quote_and_shadow() {
    let mut eval = boot_async();
    // `g-quote` (quoted parking ops are data), `g-var`.
    assert_eq!(run_last(&mut eval, "(goo (a/go '(a/<! 1)))"), "(a/<! 1)");
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (var a/<!)))"),
        "#'clojure.core.async/<!"
    );
    // `g-squote`: unquoted parking ops run.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :q) (goo (a/go `(got ~(a/<! c)))))"
        ),
        "(user/got :q)"
    );
    // `g-shadow` (an `&env` local shadows), `g-shadow-in` (a `let`
    // on the walk path shadows).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [<! (fn [_] :shadowed)] (goo (a/go (<! :ignored))))"
        ),
        ":shadowed"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go (let [<! (fn [_] :in)] (<! :ignored))))"
        ),
        ":in"
    );
    // Destructured bindings shadow too (sequential, then map).
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go (let [[<!] [(fn [_] :seq)]] (<! :ignored))))"
        ),
        ":seq"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go (let [{<! :k} {:k (fn [_] :map)}] (<! :ignored))))"
        ),
        ":map"
    );
    // Sequential `&`/`:as` and map `:keys`/`:as`/`:or` on the walk
    // path (`:or` defaults evaluate as code, so a parking op
    // there parks).
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (let [[x & _] [1 2]] x)))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (let [[x :as v] [1 2]] v)))"),
        "[1 2]"
    );
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (let [{:keys [k]} {:k :v}] k)))"),
        ":v"
    );
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (let [{a :x :as m} {:x 1}] [a m])))"),
        "[1 {:x 1}]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :dflt) (goo (a/go (let [{a :x :or {a (a/<! c)}} {}] a))))"
        ),
        ":dflt"
    );
    // `g-binding`: thread bindings convey into the body.
    run(&mut eval, "(def ^:dynamic *dq* :root)");
    assert_eq!(
        run_last(&mut eval, "(binding [*dq* :bound] (goo (a/go *dq*)))"),
        ":bound"
    );
}

// --- parking takes/puts (oracle `p-*`) ------------------------------------

#[test]
fn go_parking_basics() {
    let mut eval = boot_async();
    // `p-take-then-put`, `p-put-then-take` + `p-put-ret`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/<! c))] (a/>!! c :v) (goo g))"
        ),
        ":v"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/>! c :w))] [(a/<!! c) (goo g)])"
        ),
        "[:w true]"
    );
    // `p-seq-takes`, `p-let-take`, `p-if-take`, `p-if-take-nil`
    // (parking in arg, let, and test positions).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2)] (a/>!! c 1) (a/>!! c 2) (goo (a/go (+ (a/<! c) (a/<! c)))))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c 41) (goo (a/go (let [x (a/<! c)] (inc x)))))"
        ),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (if (a/<! c) :t :f))))"
        ),
        ":t"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (a/close! c) (goo (a/go (if (a/<! c) :t :f))))"
        ),
        ":f"
    );
    // `p-multi-expr`, `p-doseq` (`doseq` expands to a loop — no fn
    // boundary).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (a/<! c) :done)))"
        ),
        ":done"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (doseq [ch [c]] (a/<! ch)) :drained)))"
        ),
        ":drained"
    );
    // `p-nested-go` (blocking takes work inside go — D9),
    // `p-nested-go-chan`.
    assert_eq!(
        run_last(&mut eval, "(goo (a/go (a/<!! (a/go :inner))))"),
        ":inner"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [inner (a/go :in)] (goo (a/go (a/<! inner))))"
        ),
        ":in"
    );
    // Parking in map and set literals walks too.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go {:a (a/<! c)})))"
        ),
        "{:a :v}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go #{(a/<! c)})))"
        ),
        "#{:v}"
    );
    // `p-try`, `p-try-throw`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (try (a/<! c) (catch Exception e :caught)))))"
        ),
        ":v"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go (try (throw (ex-info \"in\" {})) (catch clojure.lang.ExceptionInfo e :caught))))"
        ),
        ":caught"
    );
}

#[test]
fn go_fan_in_and_scale() {
    let mut eval = boot_async();
    // `p-fan-in`, `p-scale16` (one thread per block — D9).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 8) gs (doall (for [i (range 8)] (a/go (a/>! c i))))] [(sort (for [_ (range 8)] (a/<!! c))) (sort (for [g gs] (goo g)))])"
        ),
        "[(0 1 2 3 4 5 6 7) (true true true true true true true true)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [cs (doall (for [_ (range 16)] (a/chan))) gs (doall (for [c cs] (a/go (a/<! c))))] (doseq [[i c] (map-indexed vector cs)] (a/>!! c i)) (sort (for [g gs] (goo g))))"
        ),
        "(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)"
    );
}

// --- close/buffer interplay (oracle `c-*`, `b-*`) --------------------------

#[test]
fn go_close_interplay() {
    let mut eval = boot_async();
    // `c-take-closed`, `c-put-closed`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (a/close! c) (goo (a/go (a/<! c))))"
        ),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (a/close! c) (goo (a/go (a/>! c :v))))"
        ),
        "false"
    );
    // `c-park-released`: a parked `<!` is released by close.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/<! c))] (future (deref (promise) 300 nil) (a/close! c)) (goo g))"
        ),
        "nil"
    );
    // `c-put-full-then-close` + `c-put-full-ret`: a put parked
    // behind a full buffer survives the close.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :a) (let [g (a/go (a/>! c :b))] (deref (promise) 300 nil) (a/close! c) [(a/<!! c) (a/<!! c) (goo g)]))"
        ),
        "[:a :b true]"
    );
    // `c-drain`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2)] (a/>!! c :x) (a/>!! c :y) (a/close! c) (goo (a/go [(a/<! c) (a/<! c) (a/<! c)])))"
        ),
        "[:x :y nil]"
    );
}

#[test]
fn go_buffer_interplay() {
    let mut eval = boot_async();
    // `b-full-park` + `b-full-ret`: a `>!` to a full fixed buffer
    // parks until a take frees room.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :a) (let [g (a/go (a/>! c :b))] (deref (promise) 300 nil) [(a/<!! c) (a/<!! c) (goo g)]))"
        ),
        "[:a :b true]"
    );
    // `b-drop`, `b-slide`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/dropping-buffer 1))] (a/>!! c :keep) (goo (a/go (a/>! c :drop) (a/<! c))))"
        ),
        ":keep"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan (a/sliding-buffer 1))] (a/>!! c :old) (goo (a/go (a/>! c :new) (a/<! c))))"
        ),
        ":new"
    );
}

// --- transducer interplay (oracle `x-*`) -----------------------------------

#[test]
fn go_xform_interplay() {
    let mut eval = boot_async();
    // `x-map`, `x-filter`, `x-mult`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map inc))] (goo (a/go (a/>! c 41) (a/<! c))))"
        ),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (filter odd?))] (goo (a/go (a/>! c 2) (a/>! c 3) (a/<! c))))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 4 (mapcat identity))] (goo (a/go (a/>! c [1 2]) [(a/<! c) (a/<! c)])))"
        ),
        "[1 2]"
    );
    // `x-reduced` + `x-reduced-ret`: `(take 2)` aborts through a
    // go put.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (take 2)) g (a/go (a/>! c 1) (a/>! c 2))] [(goo g) (a/<!! c) (a/<!! c) (a/<!! c) (a/closed? c)])"
        ),
        "[true 1 2 nil true]"
    );
    // `x-exh`, `x-dflt-exh` (the default handler's line prints on
    // the go thread — stderr, unasserted).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [_] (throw (ex-info \"x\" {})))) (fn [t] :fb))] (goo (a/go (a/>! c 1) (a/<! c))))"
        ),
        ":fb"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 2 (map (fn [x] (throw (ex-info \"dflt\" {:x x})))))] (goo (a/go (a/>! c 1) (a/alts!! [c] :default :empty))))"
        ),
        "[:empty :default]"
    );
}

// --- go errors (oracle `e-*`) ------------------------------------------------

#[test]
fn go_errors_settle_nil_and_close() {
    let mut eval = boot_async();
    // `e-throw`: a body throw closes with nothing delivered (the
    // D6 one-liner prints on the go thread — stderr, unasserted).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go (throw (ex-info \"boom\" {:n 1})))] [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
    // `e-fn-boundary`: `fn` bodies are opaque to the rewrite, so
    // the inner `<!` hits the parking stub at runtime.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (let [g (a/go ((fn [] (a/<! c))))] [(goo g) (a/closed? g)]))"
        ),
        "[nil true]"
    );
    // `e-bang-nil`, `e-take-nonchan`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1) g (a/go (a/>! c nil))] [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go (a/<! 42))] [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
    // `e-arity` (D10: the `<!!` arity reports through the blocking
    // layer).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (a/<!))] [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
    // `e-for-fn` (D11): parking ops hidden behind fn-creating
    // macros execute (blocking) instead of asserting.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (let [g (a/go (doall (for [ch [c]] (a/<! ch))))] [(goo g) (a/closed? g)]))"
        ),
        "[(:v) true]"
    );
    // `#()` reads through `fn*`, so it bounds the rewrite like
    // `fn`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (let [g (a/go (#(a/<! %) c))] [(goo g) (a/closed? g)]))"
        ),
        "[nil true]"
    );
}

// --- `alts!` inside go (oracle `s-*`) ----------------------------------------

#[test]
fn go_alts_basics() {
    let mut eval = boot_async();
    // `s-take`, `s-put`, `s-closed`, `s-put-closed`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :ready) (goo (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c1)]))))"
        ),
        "[:ready true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan 1)] (goo (a/go (let [[v p] (a/alts! [[c2 :v]])] [v (= p c2) (a/<! c2)]))))"
        ),
        "[true true :v]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (a/close! c1) (goo (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c1)]))))"
        ),
        "[nil true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (a/close! c2) (goo (a/go (let [[v p] (a/alts! [[c2 :v]])] [v (= p c2)]))))"
        ),
        "[false true]"
    );
    // `s-default`, `s-default-nil`, `s-default-expr`,
    // `s-ready-beats-default`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (goo (a/go (a/alts! [c1 c2] :default :d))))"
        ),
        "[:d :default]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan)] (goo (a/go (a/alts! [c1 c2] :default nil))))"
        ),
        "[nil :default]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go (a/alts! [(a/chan)] :default (do :dflt))))"
        ),
        "[:dflt :default]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan)] (a/>!! c1 :ready) (goo (a/go (let [[v p] (a/alts! [c1 c2] :default :d)] [v (= p c1)]))))"
        ),
        "[:ready true]"
    );
    // `s-priority`, `s-priority-rev`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (goo (a/go (let [[v p] (a/alts! [c1 c2] :priority true)] [v (= p c1)]))))"
        ),
        "[:first true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (goo (a/go (let [[v p] (a/alts! [c2 c1] :priority true)] [v (= p c2)]))))"
        ),
        "[:second true]"
    );
    // `s-set`, `s-unknown-opt`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :in-set) (goo (a/go (let [[v p] (a/alts! #{c})] [v (= p c)]))))"
        ),
        "[:in-set true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :x) (goo (a/go (let [[v p] (a/alts! [c] :bogus 1 :default :d)] [v (= p c)]))))"
        ),
        "[:x true]"
    );
}

#[test]
fn go_alts_wakeups_and_errors() {
    let mut eval = boot_async();
    // `s-woken`, `s-put-woken`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan) g (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c2)]))] (future (deref (promise) 300 nil) (a/>!! c2 :late)) (goo g))"
        ),
        "[:late true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) g (a/go (a/alts! [[c1 :pv]]))] (deref (promise) 300 nil) (goo (a/go (let [took (a/<! c1) [v p] (a/<! g)] [took v (= p c1)]))))"
        ),
        "[:pv true true]"
    );
    // `s-empty`, `s-nil-put`, `s-nonchan`, `s-odd-opts` (D10: the
    // blocking odd-opts text reports through the blocking layer).
    for src in [
        "(a/go (a/alts! []))",
        "(let [c (a/chan 1)] (a/go (a/alts! [[c nil]])))",
        "(a/go (a/alts! [42]))",
        "(let [c (a/chan)] (a/go (a/alts! [c] :default)))",
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(let [g {src}] [(goo g) (a/closed? g)])")
            ),
            "[nil true]",
            "{src}"
        );
    }
    // `t-empty-default`: zero op clauses still pass `[]`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [g (a/go (a/alt! :default :d))] [(goo g) (a/closed? g)])"
        ),
        "[nil true]"
    );
}

// --- `alt!` inside go (oracle `t-*`) -----------------------------------------

#[test]
fn go_alt_basics() {
    let mut eval = boot_async();
    // `t-take`, `t-put` (a never-ready channel stands in for the
    // out-of-slice `timeout`), `t-multi`, `t-bind`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan)] (a/>!! c1 :ready) (goo (a/go (a/alt! c1 ([v] [:got v]) c2 ([v] [:other v])))))"
        ),
        "[:got :ready]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (goo (a/go (a/alt! [[c :v]] :wrote (a/chan) :timeout))))"
        ),
        ":wrote"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan 1)] (a/>!! c2 :second) (goo (a/go (a/alt! [c1 c2] ([v p] [:got v (= p c2)])))))"
        ),
        "[:got :second true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (a/alt! c ([v p] [v (some? p)])))))"
        ),
        "[:v true]"
    );
    // `t-default`, `t-priority`, `t-default-take`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (goo (a/go (a/alt! c :x :default 42))))"
        ),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (goo (a/go (a/alt! :priority true c1 :first c2 :second))))"
        ),
        ":first"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (goo (a/go (a/alt! (a/chan) :t :default :d))))"
        ),
        ":d"
    );
    // `t-nested-take`: a `<!` in the result expr parks after the
    // win. `t-putval-take` + `t-putval-took`: a `<!` in the put
    // value runs before the choice.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan 1)] (a/>!! c2 :v2) (let [g (a/go (a/alt! c1 (a/<! c2)))] (future (deref (promise) 300 nil) (a/>!! c1 :v1)) (goo g)))"
        ),
        ":v2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1) d (a/chan 1)] (a/>!! d :dv) [(goo (a/go (a/alt! [[c (a/<! d)]] :x))) (a/<!! c)])"
        ),
        "[:x :dv]"
    );
}

#[test]
fn go_alt_outside_and_unbalanced() {
    let mut eval = boot_async();
    // `t-outside`: `alt!` expands over `alts!`, so outside `go` it
    // fails with the `alts!` parking assert at runtime.
    assert_eq!(
        uncaught(&mut eval, "(a/alt! [(a/chan)] :x)"),
        "java.lang.AssertionError | Assert failed: alts! used not in (go ...) block\nnil"
    );
    // `t-unbalanced`: the `do-alt` assert wraps as `Unexpected
    // error` naming the operator as written.
    assert_eq!(
        compile_err(&mut eval, "(a/alt! [(a/chan)])"),
        vec![
            "Unexpected error macroexpanding a/alt! at (user:1:1).".to_owned(),
            "java.lang.AssertionError | Assert failed: unbalanced clauses\n(even? (count clauses))"
                .to_owned(),
        ]
    );
    // `u-unbalanced`: same shape through `alt!!`.
    assert_eq!(
        compile_err(&mut eval, "(a/alt!! [(a/chan)])"),
        vec![
            "Unexpected error macroexpanding a/alt!! at (user:1:1).".to_owned(),
            "java.lang.AssertionError | Assert failed: unbalanced clauses\n(even? (count clauses))"
                .to_owned(),
        ]
    );
}

// --- `alt!!` (oracle `u-*`) --------------------------------------------------

#[test]
fn alt_block_basics() {
    let mut eval = boot_async();
    // `u-take`, `u-put`, `u-multi`, `u-bind`, `u-mixed`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan)] (a/>!! c1 :ready) (a/alt!! c1 ([v] [:got v]) c2 ([v] [:other v])))"
        ),
        "[:got :ready]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] [(a/alt!! [[c :v]] :wrote) (a/<!! c)])"
        ),
        "[:wrote :v]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan) c2 (a/chan 1)] (a/>!! c2 :second) (a/alt!! [c1 c2] ([v p] [:got v (= p c2)])))"
        ),
        "[:got :second true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (a/alt!! c ([v p] [v (some? p)])))"
        ),
        "[:v true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan)] (a/>!! c1 :v) (a/alt!! [[c2 :w] c1] ([v p] [:got v (= p c1)])))"
        ),
        "[:got :v true]"
    );
    // `u-default`, `u-priority`, `u-dup-default` (later pairs win).
    assert_eq!(
        run_last(&mut eval, "(let [c (a/chan)] (a/alt!! c :x :default 42))"),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c1 (a/chan 1) c2 (a/chan 1)] (a/>!! c1 :first) (a/>!! c2 :second) (a/alt!! :priority true c1 :first c2 :second))"
        ),
        ":first"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan)] (a/alt!! c :x :default 1 :default 2))"
        ),
        "2"
    );
    // `u-badport`: a non-vector clause is a take.
    assert_eq!(
        uncaught(&mut eval, "(a/alt!! 42 :x)"),
        "java.lang.IllegalArgumentException | No implementation of method: :take! of protocol: #'clojure.core.async.impl.protocols/ReadPort found for class: java.lang.Long"
    );
    // `u-empty-default`: the call passes `[]` even with `:default`.
    assert_eq!(
        uncaught(&mut eval, "(a/alt!! :default :x)"),
        "java.lang.AssertionError | Assert failed: alts must have at least one channel operation\n(pos? (count ports))"
    );
    // Short put vectors destructure leniently (a missing val is
    // nil — the multi-op asserts it, like upstream's EXP1V).
    for src in [
        "(let [c (a/chan 1)] (a/alt!! [[c]] :x))",
        "(a/alt!! [[]] :x)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.AssertionError | Assert failed: can't put nil on channel\n(some? (port 1))",
            "{src}"
        );
    }
    // A list result not led by a vector stands as-is.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (a/alt!! c (+ 1 2)))"
        ),
        "3"
    );
}

// --- `go-loop` (oracle `l-*`) -------------------------------------------------

#[test]
fn go_loop_basics() {
    let mut eval = boot_async();
    // `l-drain`: take until nil, `recur` across the parking point.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 3)] (doseq [x [:a :b :c]] (a/>!! c x)) (a/close! c) (goo (a/go-loop [acc []] (let [v (a/<! c)] (if (nil? v) acc (recur (conj acc v)))))))"
        ),
        "[:a :b :c]"
    );
    // `l-count`, `l-single` (`alt!` in the loop; a never-ready
    // channel stands in for the out-of-slice `timeout`).
    assert_eq!(
        run_last(
            &mut eval,
            "(goo (a/go-loop [i 5 acc 0] (if (zero? i) acc (recur (dec i) (+ acc i)))))"
        ),
        "15"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :only) (goo (a/go-loop [] (a/alt! c ([v] [:got v]) (a/chan) :timeout))))"
        ),
        "[:got :only]"
    );
    // `go-loop` needs its bindings (the macro's own arity strips
    // the hidden `&form`/`&env` and rethrows bare).
    assert_eq!(
        uncaught(&mut eval, "(a/go-loop)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core.async/go-loop"
    );
}

// --- port fidelity (beyond the oracle) ---------------------------------------

#[test]
fn go_resolution_styles() {
    let mut eval = boot_async();
    // Bare heads through `:refer` rename (resolve-aware, not
    // alias-aware) — in a scratch ns, so the refers stay out of
    // `user`.
    run(&mut eval, "(create-ns 'referstyle)");
    run(&mut eval, "(in-ns 'referstyle)");
    run(&mut eval, "(clojure.core/refer 'clojure.core)");
    run(
        &mut eval,
        "(require '[clojure.core.async :refer [go <! >!! chan <!!]])",
    );
    run(
        &mut eval,
        "(defn goo [g] (deref (future (<!! g)) 8000 :timeout))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chan) g (go (<! c))] (>!! c :v) (goo g))"
        ),
        ":v"
    );
    run(&mut eval, "(in-ns 'user)");
    // Fully-qualified heads rename too.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan) g (a/go (clojure.core.async/<! c))] (a/>!! c :v) (goo g))"
        ),
        ":v"
    );
    // Another var of the same name is left alone (a local `<!`
    // compiles as an ordinary call even when the alias exists).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [<! (fn [x] [:local x])] (goo (a/go (<! :q))))"
        ),
        "[:local :q]"
    );
    // `let*` bindings track like `let` (expansion-internal form,
    // same shape).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (let* [x (a/<! c)] x))))"
        ),
        ":v"
    );
    // A qualified `let` over the intact builtin tracks too.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/>!! c :v) (goo (a/go (clojure.core/let [x (a/<! c)] x))))"
        ),
        ":v"
    );
    // A `:rename`d parking op still parks (the rename follows the
    // target var).
    run(&mut eval, "(create-ns 'renamestyle)");
    run(&mut eval, "(in-ns 'renamestyle)");
    run(&mut eval, "(clojure.core/refer 'clojure.core)");
    run(
        &mut eval,
        "(require '[clojure.core.async :refer [go <! >!! chan <!!]])",
    );
    run(&mut eval, "(refer 'clojure.core.async :rename '{<! take!})");
    run(
        &mut eval,
        "(defn goo [g] (deref (future (<!! g)) 8000 :timeout))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chan) g (go (take! c))] (>!! c :v) (goo g))"
        ),
        ":v"
    );
    run(&mut eval, "(in-ns 'user)");
    // A local named `let` shadows the builtin for the walk and
    // the analyzer alike (the inner form compiles as a call, so
    // its bindings vector is a literal of unbound symbols).
    assert_eq!(
        compile_err(
            &mut eval,
            "(let [let 5] (let [g (a/go (let 6))] [(goo g) (a/closed? g)]))"
        ),
        vec![
            "Syntax error compiling at (user:1:14).".to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: g in this context".to_owned(),
        ]
    );
    // A global `let` shadows too: the inner form compiles as a
    // call of the user var (defined last — later `let` forms in
    // this evaluator would compile as calls).
    run(&mut eval, "(def let (fn [& _] :user-let))");
    assert_eq!(run_last(&mut eval, "(goo (a/go (let 6)))"), ":user-let");
}

#[test]
fn go_install_and_call() {
    let mut eval = boot_async();
    // `:reload` reinstalls cleanly (macros stay macros).
    run(&mut eval, "(require '[clojure.core.async] :reload)");
    assert_eq!(run_last(&mut eval, "(goo (a/go 7))"), "7");
    assert_eq!(
        run_last(&mut eval, "(let [c (a/chan)] (a/alt!! c :x :default :d))"),
        ":d"
    );
    // `go-call` directly (D12): delivers the thunk's value, then
    // closes; nil delivers nothing.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/go-call c (fn [] :v)) [(goo c) (a/closed? c)])"
        ),
        "[:v true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/go-call c (fn [] nil)) [(goo c) (a/closed? c)])"
        ),
        "[nil true]"
    );
    // A direct `go-call` throw settles the same way (one-liner on
    // the go thread — stderr, unasserted).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (a/chan 1)] (a/go-call c (fn [] (throw (ex-info \"direct\" {})))) [(goo c) (a/closed? c)])"
        ),
        "[nil true]"
    );
    // `go-call` validates its channel eagerly (the put protocol).
    assert_eq!(
        uncaught(&mut eval, "(a/go-call 42 (fn [] 1))"),
        "java.lang.IllegalArgumentException | No implementation of method: :put! of protocol: #'clojure.core.async.impl.protocols/WritePort found for class: java.lang.Long"
    );
    // ... and its arity.
    assert_eq!(
        uncaught(&mut eval, "(a/go-call (a/chan 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core.async/go-call"
    );
    // The block macros have no value (existing machinery, new
    // vars).
    assert_eq!(
        compile_err(&mut eval, "a/go"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Can't take value of a macro: #'clojure.core.async/go"
                .to_owned(),
        ]
    );
}
