// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S10 cells, taps, and helpers (`volatile!`, `volatile?`, `vreset!`,
//! `vswap!`, `add-tap`, `remove-tap`, `tap>`, `tap->`, `comparator`,
//! `hash-ordered-coll`, `hash-unordered-coll`,
//! `set-agent-send-executor!`, `set-agent-send-off-executor!`).
//! Every oracle row in `docs/zero-oos-s10-cells/probes/*.out` is
//! pinned here (row ids in comments):
//!
//! - cells (31): `volatile_ctor_print` (`c-new`, `c-new-nil`,
//!   `c-new-coll`), `volatile_pred` (`c-pred-*`), `volatile_deref`
//!   (`c-deref*`), `volatile_vreset` (`c-vreset*`), `volatile_vswap`
//!   (`c-vswap`, `c-vswap-multi`, `c-vswap-noargs`,
//!   `c-vswap-nonfn`, `c-vswap-nonvol`), `volatile_vswap_shape`
//!   (`c-shape*`, `c-double-eval`), `volatile_identity_meta`
//!   (`c-eq-same`, `c-eq-distinct`, `c-identical`, `c-meta`,
//!   `c-ifn`, `c-counted`), plus port-side pins with no oracle
//!   rows: `volatile_hash_identity`, `volatile_map_key`,
//!   `volatile_str`, `volatile_compare_name`.
//! - taps (17): `tap_returns` (`t-no-taps`, `t-add-ret`,
//!   `t-add-nonfn-ret`, `t-remove-ret`, `t-remove-never`),
//!   `tap_deliver` (`t-deliver`, `t-tap-ret-with-tap`),
//!   `tap_thread` (`t-thread`), `tap_nil` (`t-nil`),
//!   `tap_set_semantics` (`t-add-twice`, `t-removed-misses`),
//!   `tap_throw_safe` (`t-throw-safe`), `tap_thread_first`
//!   (`t-tap->-ret`, `t-tap->-delivers`, `t-tap->-nil`,
//!   `t-tap->-false`), `tap_full` (`t-full`).
//! - helpers (35): `comparator_calls` (`k-lt`, `k-gt`, `k-eq`,
//!   `k-flip`, `k-flip2`), `comparator_sort` (`k-sort`,
//!   `k-sorted-map`, `k-truthy-lt`, `k-truthy-eq`, `k-str`),
//!   `comparator_errors` (`k-arity`, `k-nonfn-pred`),
//!   `hash_ordered_values` (`h-ord-123`, `h-ord-empty`,
//!   `h-ord-single`, `h-ord-list`, `h-ord-eq-vec-list`,
//!   `h-ord-eq-hash`, `h-ord-str-els`, `h-ord-nested`, `h-ord-map`,
//!   `h-ord-neg`), `hash_ordered_errors` (`h-ord-nil`,
//!   `h-ord-string`, `h-ord-num`), `hash_unordered_values`
//!   (`h-unord-set`, `h-unord-empty`, `h-unord-single`,
//!   `h-unord-eq-hash`, `h-unord-map`, `h-unord-map-eq-hash`,
//!   `h-unord-vec-order`, `h-unord-vec-val`, `h-entry`,
//!   `h-unord-nil`).
//! - executors (12): `exec_record_send` (`e-set-ret`, `e-nil-set`,
//!   `e-nil-send`, `e-non-exec`, `e-non-exec-pool-ok`),
//!   `exec_record_send_off` (`e-off-ret`, `e-off-nil-set`,
//!   `e-off-non-exec`), `exec_record_cross` (`e-routes`,
//!   `e-off-routes`, `e-cross`, `e-restored`).
//!
//! Each test boots its own evaluator (a fresh tap system and fresh
//! agent pools per test), so deliveries never cross tests; the one
//! test that offers before any tap exists (`tap_thread_first`) boots
//! per assert, so queued items never cross forms either (the queue
//! is FIFO and the dispatcher snapshots the registry at consumption
//! time — a stale item would otherwise reach a later form's tap).
//! The tap dispatcher is daemon (detached); agent tests end with
//! `join_drainers`, so the suite exits cleanly.

use std::sync::{Arc, Mutex};

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, Volatile, pr_to_string};

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

/// A `ClassCastException` with the JDK module verbiage (same-module
/// classes combine, different modules split).
fn cce(from: &str, to: &str) -> String {
    fn module(cls: &str) -> &'static str {
        if cls.starts_with("java.") {
            "module java.base of loader 'bootstrap'"
        } else {
            "unnamed module of loader 'app'"
        }
    }
    let (fm, tm) = (module(from), module(to));
    let message = if fm == tm {
        format!("class {from} cannot be cast to class {to} ({from} and {to} are in {fm})")
    } else {
        format!("class {from} cannot be cast to class {to} ({from} is in {fm}; {to} is in {tm})")
    };
    format!("java.lang.ClassCastException | {message}")
}

/// Assert the transient-style `#object` rendering with matching hex
/// identities (per-process ids, never pinned literally).
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

fn assert_volatile_object(pr: &str) {
    assert_object_shape(pr, "clojure.lang.Volatile");
}

// ---- volatile cells ----

#[test]
fn volatile_ctor_print() {
    // c-new, c-new-nil, c-new-coll. Pinned divergence: the JVM
    // renders `{:status :ready, :val …}`; the port takes the
    // transient `#object` convention established for atoms.
    let mut eval = boot();
    assert_volatile_object(&run_last(&mut eval, "(volatile! 0)"));
    assert_volatile_object(&run_last(&mut eval, "(volatile! nil)"));
    assert_volatile_object(&run_last(&mut eval, "(volatile! {:a [1 2]})"));
}

#[test]
fn volatile_pred() {
    // c-pred-t, c-pred-atom, c-pred-int, c-pred-nil, c-pred-str.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(volatile? (volatile! 1))"), "true");
    assert_eq!(run_last(&mut eval, "(volatile? (atom 1))"), "false");
    assert_eq!(run_last(&mut eval, "(volatile? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(volatile? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(volatile? \"x\")"), "false");
}

#[test]
fn volatile_deref() {
    // c-deref, c-deref-reader, c-deref-timeout, c-deref-nil (control).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(deref (volatile! 5))"), "5");
    assert_eq!(run_last(&mut eval, "(do @(volatile! 7))"), "7");
    // Volatiles are not blocking: the timeout arity falls into
    // `deref_timeout`'s existing `other` arm (no new arm).
    assert_eq!(
        uncaught(&mut eval, "(deref (volatile! 1) 100 :t)"),
        cce("clojure.lang.Volatile", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.get()\" because \"fut\" is null"
    );
}

#[test]
fn volatile_vreset() {
    // c-vreset, c-vreset-nil, c-vreset-nonvol, c-vreset-nil-vol.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 0)] [(vreset! v 9) @v])"),
        "[9 9]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 0)] [(vreset! v nil) @v])"),
        "[nil nil]"
    );
    assert_eq!(
        uncaught(&mut eval, "(vreset! 1 2)"),
        cce("java.lang.Long", "clojure.lang.Volatile")
    );
    assert_eq!(
        uncaught(&mut eval, "(vreset! nil 2)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Volatile.reset(Object)\" because \"vol\" is null"
    );
}

#[test]
fn volatile_vswap() {
    // c-vswap, c-vswap-multi, c-vswap-noargs, c-vswap-nonfn,
    // c-vswap-nonvol.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 0)] [(vswap! v + 5) @v])"),
        "[5 5]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] [(vswap! v + 1 2 3) @v])"),
        "[7 7]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] [(vswap! v inc) @v])"),
        "[2 2]"
    );
    // PINNED PORT DIVERGENCE (message only): the oracle's
    // `c-vswap-nonfn` carries the full JDK module verbiage, but the
    // failure surfaces on the port's direct-invoke path, whose
    // short `Cannot cast A to B` shape is established behavior
    // (pinned by `analyzer_contract.rs` invoke rows). The class
    // (`CCE to IFn`) matches; only the message shape differs.
    assert_eq!(
        uncaught(&mut eval, "(let [v (volatile! 1)] (vswap! v 5))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(
        uncaught(&mut eval, "(vswap! (atom 1) inc)"),
        cce("clojure.lang.Atom", "clojure.lang.Volatile")
    );
}

#[test]
fn volatile_vswap_shape() {
    // c-shape, c-shape-multi, c-shape-noargs: the port expansion
    // through its own `vreset!`/`deref` (the JVM shape uses
    // `.reset`/`.deref`). c-double-eval: `vol` evaluates twice.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(vswap! v + 5))"),
        "(vreset! v (+ (deref v) 5))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(vswap! v f 1 2))"),
        "(vreset! v (f (deref v) 1 2))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(vswap! v inc))"),
        "(vreset! v (inc (deref v)))"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) v (volatile! 1)] (vswap! (do (swap! c inc) v) inc) [@v @c])"
        ),
        "[2 2]"
    );
}

#[test]
fn volatile_identity_meta() {
    // c-eq-same, c-eq-distinct, c-meta, c-ifn, c-counted.
    // c-identical: `identical?` is out of scope in the port
    // ("reference identity: no slice (values clone)"), so the
    // shared cell is pinned behaviorally instead — a reset
    // through one handle reads through the other.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] (= v v))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (volatile! 1) (volatile! 1))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1) w v] (vreset! v 2) @w)"),
        "2"
    );
    assert_eq!(run_last(&mut eval, "(meta (volatile! 1))"), "nil");
    assert_eq!(run_last(&mut eval, "(ifn? (volatile! 1))"), "false");
    assert_eq!(run_last(&mut eval, "(counted? (volatile! 1))"), "false");
}

#[test]
fn volatile_hash_identity() {
    // Port-side pin (no oracle rows): volatiles hash by identity
    // (the per-process cell id) under both `hash` (`hasheq`) and
    // the private `hash-code` (Java `.hashCode`, called from
    // `clojure.core` like `control.rs hash_code_spots`). Identity
    // hashes are never exact values — consistency only.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] (= (hash v) (hash v)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (volatile! nil)))"),
        "true"
    );
    run(&mut eval, "(in-ns 'clojure.core)");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v (volatile! 1)] (= (hash-code v) (hash-code v)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash-code (volatile! nil)))"),
        "true"
    );
}

#[test]
fn volatile_map_key() {
    // Port-side pin (no oracle rows): volatiles hash (std `Hash`)
    // by identity, so they serve as map keys and set members with
    // cell-identity semantics — the same cell hits, a distinct
    // equal-valued cell misses.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] (get {v :found} v))"),
        ":found"
    );
    assert_eq!(
        run_last(&mut eval, "(let [v (volatile! 1)] (contains? #{v} v))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(get {(volatile! 1) :a} (volatile! 1) :miss)"),
        ":miss"
    );
}

#[test]
fn volatile_str() {
    // Port-side pin (no oracle rows): `str` renders the `toString`
    // (`Class@hex`), never the `#object[…]` print form (mirrors
    // the `concurrency.rs` atom `str` rows).
    let mut eval = boot();
    let s = run_last(&mut eval, "(str (volatile! 1))");
    assert!(
        s.starts_with("\"clojure.lang.Volatile@") && s.len() > "\"clojure.lang.Volatile@\"".len(),
        "bad volatile str: {s}"
    );
}

#[test]
fn volatile_compare_name() {
    // Port-side pin (no oracle rows): `clj_value::compare` (not the
    // `compare` native, which routes through `jvm_compare`)
    // reports the `Volatile` type name on incomparable pairs.
    let v = Value::Volatile(Volatile::new(Value::Int(1)));
    let err = clj_value::compare(&v, &Value::Int(1)).expect_err("volatile is incomparable");
    assert_eq!(err.message(), "Cannot compare Volatile to Long");
}

// ---- taps ----

#[test]
fn tap_returns() {
    // t-no-taps, t-add-ret, t-add-nonfn-ret, t-remove-ret,
    // t-remove-never.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(tap> :lonely)"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def t1 (fn [_] nil)) (let [r (add-tap t1)] (remove-tap t1) r))"
        ),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(do (add-tap 42) (remove-tap 42))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def fr (fn [_] nil)) (add-tap fr) [(remove-tap fr) (remove-tap fr)])"
        ),
        "[nil nil]"
    );
    assert_eq!(run_last(&mut eval, "(remove-tap (fn [_] nil))"), "nil");
}

#[test]
fn tap_deliver() {
    // t-deliver, t-tap-ret-with-tap.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p1 (promise)) (def f1 (fn [x] (deliver p1 x))) (add-tap f1) (tap> :x) (let [v (deref p1 5000 :TIMEOUT)] (remove-tap f1) v))"
        ),
        ":x"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def fw (fn [_] nil)) (add-tap fw) (let [r (tap> :w)] (remove-tap fw) r))"
        ),
        "true"
    );
}

#[test]
fn tap_thread() {
    // t-thread: delivery runs on a thread named exactly
    // `clojure.core/tap-loop`. The port has no
    // `Thread/currentThread`, so a test native records the
    // dispatching thread's name instead.
    let mut eval = boot();
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen_tap = Arc::clone(&seen);
    eval.install_native(
        "user",
        "record-thread",
        Arc::new(move |_, _| {
            *seen_tap.lock().unwrap() = std::thread::current().name().map(str::to_owned);
            Ok(Value::Nil)
        }),
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def pn (promise)) (def fnt (fn [_] (record-thread) (deliver pn :done))) (add-tap fnt) (tap> :t) (let [v (deref pn 5000 :TIMEOUT)] (remove-tap fnt) v))"
        ),
        ":done"
    );
    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some("clojure.core/tap-loop")
    );
}

#[test]
fn tap_nil() {
    // t-nil: nil round-trips through the queue (first-class
    // `Value::Nil` — no sentinel needed).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def pn2 (promise)) (def fn2 (fn [x] (deliver pn2 [:got x]))) (add-tap fn2) (tap> nil) (let [v (deref pn2 5000 :TIMEOUT)] (remove-tap fn2) v))"
        ),
        "[:got nil]"
    );
}

#[test]
fn tap_set_semantics() {
    // t-add-twice: set semantics — added twice, delivered ONCE.
    // The promise rendezvous replaces the oracle's 500 ms sleep:
    // exactly one item is offered, so once the tap runs the count
    // is final. t-removed-misses: a tap removed before the offer
    // never fires.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def c1 (atom 0)) (def pd (promise)) (def fc (fn [_] (swap! c1 inc) (deliver pd :done))) (add-tap fc) (add-tap fc) (tap> :once) (deref pd 5000 :TIMEOUT) (remove-tap fc) @c1)"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p3 (promise)) (def f3 (fn [x] (deliver p3 x))) (add-tap f3) (remove-tap f3) (tap> :missed) (deref p3 300 :TIMEOUT))"
        ),
        ":TIMEOUT"
    );
}

#[test]
fn tap_throw_safe() {
    // t-throw-safe: a throwing tap is swallowed; later taps still
    // fire (the per-tap catch-all).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p4 (promise)) (def boom (fn [_] (throw (ex-info \"boom\" {})))) (def f4 (fn [x] (deliver p4 x))) (add-tap boom) (add-tap f4) (tap> :after-boom) (let [v (deref p4 5000 :TIMEOUT)] (remove-tap boom) (remove-tap f4) v))"
        ),
        ":after-boom"
    );
}

#[test]
fn tap_thread_first() {
    // t-tap->-ret, t-tap->-delivers, t-tap->-nil, t-tap->-false:
    // `tap->` returns x AND delivers it. Each assert boots a FRESH
    // evaluator (a fresh tap system): the queue is FIFO and the
    // dispatcher snapshots the registry at consumption time, so an
    // item a form offers with no taps yet (the bare `(tap-> 5)`)
    // could otherwise still be queued when a later form registers
    // its tap — delivering the stale item to the wrong promise
    // (`[:threaded 5]`, the Windows CI flake). The JVM races the
    // same way (queue + take-then-`@tapset`); per-form isolation
    // keeps the race out of the asserts.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(tap-> 5)"), "5");
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p5 (promise)) (def f5 (fn [x] (deliver p5 x))) (add-tap f5) (let [r (tap-> :threaded) v (deref p5 5000 :TIMEOUT)] (remove-tap f5) [r v]))"
        ),
        "[:threaded :threaded]"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p6 (promise)) (def f6 (fn [x] (deliver p6 [:got x]))) (add-tap f6) (let [r (tap-> nil) v (deref p6 5000 :TIMEOUT)] (remove-tap f6) [r v]))"
        ),
        "[nil [:got nil]]"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def p7 (promise)) (def f7 (fn [x] (deliver p7 [:got x]))) (add-tap f7) (let [r (tap-> false) v (deref p7 5000 :TIMEOUT)] (remove-tap f7) [r v]))"
        ),
        "[false [:got false]]"
    );
}

#[test]
fn tap_full() {
    // t-full: full-queue offer counts. Gated on the blocked
    // consumer (the `entered` rendezvous proves the dispatcher is
    // parked inside `slow` with an empty queue), so no timing
    // enters the asserts: 1500 offers against capacity 1024 count
    // `[1024 476]` deterministically.
    let mut eval = boot();
    run_last(
        &mut eval,
        "(do (def gate (promise)) (def entered (promise)) (def slow (fn [_] (deliver entered :in) @gate)) (add-tap slow) (tap> :blocker) :setup)",
    );
    assert_eq!(run_last(&mut eval, "(deref entered 5000 :TIMEOUT)"), ":in");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [rs (doall (repeatedly 1500 (fn [] (tap> :flood)))) trues (count (filter true? rs)) falses (count (filter false? rs))] [trues falses])"
        ),
        "[1024 476]"
    );
    run_last(&mut eval, "(do (deliver gate :go) (remove-tap slow) :done)");
}

// ---- comparator ----

#[test]
fn comparator_calls() {
    // k-lt, k-gt, k-eq, k-flip, k-flip2.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((comparator <) 1 2)"), "-1");
    assert_eq!(run_last(&mut eval, "((comparator <) 2 1)"), "1");
    assert_eq!(run_last(&mut eval, "((comparator <) 1 1)"), "0");
    assert_eq!(run_last(&mut eval, "((comparator >) 1 2)"), "1");
    assert_eq!(run_last(&mut eval, "((comparator >) 2 1)"), "-1");
}

#[test]
fn comparator_sort() {
    // k-sort, k-sorted-map, k-truthy-lt, k-truthy-eq, k-str. The
    // truthy rows pin the `cond` (not `if`): non-bool pred
    // results order.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(sort (comparator >) [1 2 3])"),
        "(3 2 1)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(keys (into (sorted-map-by (comparator >)) {1 :a 2 :b 3 :c}))"
        ),
        "(3 2 1)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((comparator (fn [x y] (when (< x y) :lt))) 1 2)"
        ),
        "-1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((comparator (fn [x y] (when (< x y) :lt))) 2 2)"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(sort (comparator (fn [x y] (< (count x) (count y)))) [\"ccc\" \"a\" \"bb\"])"
        ),
        "(\"a\" \"bb\" \"ccc\")"
    );
}

#[test]
fn comparator_errors() {
    // k-arity, k-nonfn-pred. The arity display carries boot-order
    // `evalN`/`fn--N` counters, so the shape (not the numbers) is
    // pinned: `Wrong number of args (1) passed to: user/evalN/fn--M`.
    let mut eval = boot();
    let msg = uncaught(&mut eval, "((comparator <) 1)");
    let prefix = "clojure.lang.ArityException | Wrong number of args (1) passed to: user/eval";
    let tail = msg
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("bad arity shape: {msg}"));
    let (eval_n, fn_n) = tail
        .split_once("/fn--")
        .unwrap_or_else(|| panic!("bad arity shape: {msg}"));
    assert!(
        !eval_n.is_empty() && eval_n.chars().all(|c| c.is_ascii_digit()),
        "bad arity shape: {msg}"
    );
    assert!(
        !fn_n.is_empty() && fn_n.chars().all(|c| c.is_ascii_digit()),
        "bad arity shape: {msg}"
    );
    // PINNED PORT DIVERGENCE (message only): same invoke-path
    // short-CCE convention as `volatile_vswap`'s `c-vswap-nonfn`
    // above — the class (`CCE to IFn`) matches the oracle's
    // `k-nonfn-pred`.
    assert_eq!(
        uncaught(&mut eval, "((comparator 5) 1 2)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

// ---- hash helpers ----

#[test]
fn hash_ordered_values() {
    // h-ord-123, h-ord-empty, h-ord-single, h-ord-list,
    // h-ord-eq-vec-list, h-ord-eq-hash, h-ord-str-els,
    // h-ord-nested, h-ord-map, h-ord-neg — every integer verbatim
    // from the oracle.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll [1 2 3])"),
        "736442005"
    );
    assert_eq!(run_last(&mut eval, "(hash-ordered-coll [])"), "-2017569654");
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll [42])"),
        "1127286853"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll '(1 2))"),
        "156247261"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash-ordered-coll [1 2]) (hash-ordered-coll '(1 2)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (hash [1 2 3]) (hash-ordered-coll [1 2 3]))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll [\"a\" \"b\"])"),
        "-757264838"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll [[1] #{2} {:a 1}])"),
        "1393116264"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll {:a 1})"),
        "1242899341"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-ordered-coll [-1 0])"),
        "-1894195675"
    );
}

#[test]
fn hash_ordered_errors() {
    // h-ord-nil, h-ord-string, h-ord-num. Strings and numbers are
    // NOT accepted even though strings seq: the JVM takes
    // `Iterable`, and neither is `Iterable`.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(hash-ordered-coll nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Iterable.iterator()\" because \"xs\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(hash-ordered-coll \"ab\")"),
        cce("java.lang.String", "java.lang.Iterable")
    );
    assert_eq!(
        uncaught(&mut eval, "(hash-ordered-coll 5)"),
        cce("java.lang.Long", "java.lang.Iterable")
    );
}

#[test]
fn hash_unordered_values() {
    // h-unord-set, h-unord-empty, h-unord-single, h-unord-eq-hash,
    // h-unord-map, h-unord-map-eq-hash, h-unord-vec-order,
    // h-unord-vec-val, h-unord-nil. h-entry reads the entry's pair
    // through `key`/`val` (the oracle uses Java interop).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(hash-unordered-coll #{1 2 3})"),
        "439094965"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-unordered-coll #{})"),
        "-15128758"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-unordered-coll #{42})"),
        "-1234344236"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash #{1 2 3}) (hash-unordered-coll #{1 2 3}))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-unordered-coll {:a 1 :b 2})"),
        "161871944"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash {:a 1 :b 2}) (hash-unordered-coll {:a 1 :b 2}))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash-unordered-coll [1 2]) (hash-unordered-coll [2 1]))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(hash-unordered-coll [1 2])"),
        "460223544"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [e (first {:a 1})] [(hash e) (hash-ordered-coll [(key e) (val e)]) (= (hash e) (hash-ordered-coll [(key e) (val e)]))])"
        ),
        "[-1809393901 -1809393901 true]"
    );
    assert_eq!(
        uncaught(&mut eval, "(hash-unordered-coll nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Iterable.iterator()\" because \"xs\" is null"
    );
}

// ---- executor vars (accept-and-record) ----

#[test]
fn exec_record_send() {
    // e-set-ret: the setter returns the executor itself (`set!`
    // value semantics). `identical?` is out of scope in the port,
    // so the roundtrip pins `=` on a distinctive value.
    // e-nil-set: nil is accepted at set time. e-nil-send and
    // e-non-exec are PINNED DIVERGENCES: the port has no host
    // executors (so custom routing is unimplementable) and no
    // `ExecutorService` host type to check against (so the JVM's
    // set-time CCE is unimplementable too) — every value is
    // accepted, and dispatch stays on the fixed pools.
    // e-non-exec-pool-ok: the pool is untouched after a
    // would-be rejection (`:set-ok`, not `:caught`).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ex1 :fake-send-exec) (= (set-agent-send-executor! ex1) ex1))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(do (set-agent-send-executor! nil) :restored)"),
        ":restored"
    );
    // Recorded nil changes nothing: the send completes on the
    // fixed pool (the JVM silently drops it).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-executor! nil) (def a0 (agent 0)) (send a0 inc) (await a0) @a0)"
        ),
        "1"
    );
    // Non-executors are accepted, never CCE'd (the JVM rejects
    // `:kw` with a CCE to `ExecutorService`).
    assert_eq!(run_last(&mut eval, "(set-agent-send-executor! :kw)"), ":kw");
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def r (try (do (set-agent-send-executor! :kw) :set-ok) (catch Throwable t :caught))) (def aok (agent 0)) (send aok inc) (await aok) [r @aok])"
        ),
        "[:set-ok 1]"
    );
    eval.join_drainers();
}

#[test]
fn exec_record_send_off() {
    // e-off-ret, e-off-nil-set, e-off-non-exec: the `send-off`
    // mirror of `exec_record_send` (same accept-and-record
    // contract, same pinned divergences).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ex3 :fake-send-off-exec) (= (set-agent-send-off-executor! ex3) ex3))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-off-executor! nil) :restored)"
        ),
        ":restored"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-off-executor! nil) (def a1 (agent 0)) (send-off a1 inc) (await a1) @a1)"
        ),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(set-agent-send-off-executor! 42)"),
        "42"
    );
    eval.join_drainers();
}

#[test]
fn exec_record_cross() {
    // e-routes / e-off-routes: PINNED DIVERGENCES — the JVM runs
    // subsequent actions on the supplied pool's threads; the port
    // always completes on the fixed pools (the action still runs
    // and the value still updates; only the thread differs).
    // e-cross: pools are independent (holds trivially — the
    // recorded values are never consulted). e-restored: there are
    // no JVM statics to restore; dispatch is unaffected by every
    // set above, which the completions pin.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-executor! :elsewhere) (def a1 (agent 0)) (send a1 inc) (await a1) @a1)"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-off-executor! :elsewhere-off) (def a2 (agent 0)) (send-off a2 inc) (await a2) @a2)"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (set-agent-send-off-executor! :cross) (def a3 (agent 0)) (send a3 inc) (await a3) @a3)"
        ),
        "1"
    );
    eval.join_drainers();
}
