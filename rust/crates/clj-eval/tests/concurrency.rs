// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S6A concurrency: probe replay for atoms (`atom`, `deref`, `swap!`,
//! `reset!`, `compare-and-set!`, `add-watch`/`remove-watch`), delays
//! (`delay`, `force`, `realized?`), and `memoize` (see
//! `docs/t1174/s6-pins.md` P1/P2 and `/tmp/s6a-probe*.clj`).

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
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
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

/// Block until `(agent-error ag)` is non-nil (the failure landed):
/// failures always land, so only a genuine hang trips the timeout
/// (never await a racing latch — a stuck barrier hangs by design).
fn wait_failed(eval: &mut Evaluator, ag: &str) {
    let start = std::time::Instant::now();
    while run_last(eval, &format!("(nil? (agent-error {ag}))")) != "false" {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "failure never landed for {ag}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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

fn assert_atom_object(pr: &str) {
    assert_object_shape(pr, "clojure.lang.Atom");
}

fn assert_delay_object(pr: &str) {
    assert_object_shape(pr, "clojure.lang.Delay");
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

#[test]
fn atom_ctor_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(deref (atom 1))"), "1");
    assert_eq!(run_last(&mut eval, "(deref (atom nil))"), "nil");
    assert_eq!(run_last(&mut eval, "(deref (atom false))"), "false");
    assert_eq!(
        run_last(&mut eval, "(meta (atom 1 :meta {:k :v}))"),
        "{:k :v}"
    );
    assert_eq!(run_last(&mut eval, "(meta (atom 1))"), "nil");
    assert_eq!(run_last(&mut eval, "(meta (atom 1 :meta nil))"), "nil");
    // Falsy validators never install; unknown options are ignored.
    assert_eq!(
        run_last(&mut eval, "(do (atom 2 :validator even?) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (atom 1 :validator nil) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (atom 1 :validator false) :ok)"),
        ":ok"
    );
    assert_eq!(run_last(&mut eval, "(do (atom 1 :foo 2) :ok)"), ":ok");
    // Duplicate options: last wins (hash-map build).
    assert_eq!(
        run_last(&mut eval, "(meta (atom 1 :meta {:a 1} :meta {:b 2}))"),
        "{:b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (atom 1 :validator (fn [x] true) :meta {:a 1}))"
        ),
        "{:a 1}"
    );
    assert_eq!(class_of(&mut eval, "(atom 1)"), "clojure.lang.Atom");
}

#[test]
fn atom_ctor_errors() {
    let mut eval = boot();
    // The ctor validates the initial value too.
    assert_eq!(
        uncaught(&mut eval, "(atom 2 :validator odd?)"),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        uncaught(&mut eval, "(atom 1 :validator)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :validator"
    );
    assert_eq!(
        uncaught(&mut eval, "(atom 1 :a 1 :stray)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :stray"
    );
    assert_eq!(
        uncaught(&mut eval, "(atom 1 2)"),
        "java.lang.IllegalArgumentException | No value supplied for key: 2"
    );
    assert_eq!(
        uncaught(&mut eval, "(atom 1 :meta 5)"),
        cce("java.lang.Long", "clojure.lang.IPersistentMap")
    );
    assert_eq!(
        uncaught(&mut eval, "(atom 1 :validator 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(atom)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/atom"
    );
}

#[test]
fn validator_throw_shapes() {
    let mut eval = boot();
    // A validator throwing a RuntimeException propagates as-is (class,
    // message, and ex-data survive) — at publish time and at ctor time
    // (the ctor validator runs over the initial value).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def vt (atom 0 :validator (fn [x] (if (zero? x) true (throw (ex-info \"vx\" {:n 1})))))) (try (reset! vt 5) (catch ExceptionInfo e [(ex-message e) (ex-data e)])))"
        ),
        "[\"vx\" {:n 1}]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(atom 0 :validator (fn [x] (throw (ex-info \"vx\" {:n 1}))))"
        ),
        "clojure.lang.ExceptionInfo | vx"
    );
}

#[test]
fn validator_checked_wraps() {
    let mut eval = boot();
    // No source form builds a checked `Exception`, so a test native
    // throws one: the validator path wraps it (with the cause), like
    // the JVM's `ARef.validate` catch order.
    eval.install_native(
        "user",
        "boom-checked",
        std::sync::Arc::new(|_, _| {
            Err(clj_eval::NativeError::new(
                clj_value::ThrowClass::Exception,
                "checked-boom".to_owned(),
            ))
        }),
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 0 :validator (fn [x] (if (zero? x) true (boom-checked))))] (reset! a 1))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def vca (atom 0 :validator (fn [x] (if (zero? x) true (boom-checked))))) (try (reset! vca 1) (catch IllegalStateException e (ex-message (ex-cause e)))))"
        ),
        "\"checked-boom\""
    );
}

#[test]
fn deref_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(deref (atom 5))"), "5");
    assert_eq!(run_last(&mut eval, "@(atom 5)"), "5");
    assert_eq!(run_last(&mut eval, "(deref (delay 42))"), "42");
    assert_eq!(run_last(&mut eval, "@(delay 42)"), "42");
    // Vars are the pre-existing `IDeref` kin: thread bindings win,
    // else the root.
    assert_eq!(run_last(&mut eval, "(do (def dx 7) (deref #'dx))"), "7");
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic ddy 1) (binding [ddy 2] (deref #'ddy)))"
        ),
        "2"
    );
    assert_eq!(
        class_of(&mut eval, "(do (declare dux) (deref #'dux))"),
        "clojure.lang.Var$Unbound"
    );
}

#[test]
fn deref_errors() {
    let mut eval = boot();
    // Non-`IDeref`s fall through to the `Future` cast (S6B owns the
    // timeout arity; S6A is 1-arity only).
    assert_eq!(
        uncaught(&mut eval, "(deref 1)"),
        cce("java.lang.Long", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref \"s\")"),
        cce("java.lang.String", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.get()\" because \"fut\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(deref)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/deref"
    );
    assert_eq!(
        uncaught(&mut eval, "(deref (atom 1) 50)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/deref"
    );
}

#[test]
fn swap_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(swap! (atom 1) inc)"), "2");
    // Returns the swapped-in value.
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1)] [(swap! a inc) @a])"),
        "[2 2]"
    );
    // Extra args apply (`(apply f old args*)`).
    assert_eq!(run_last(&mut eval, "(swap! (atom 0) + 1 2 3)"), "6");
    assert_eq!(run_last(&mut eval, "(swap! (atom 0) + 1 2 3 4 5)"), "15");
    // Any `IFn` serves (keywords invoke as lookups).
    assert_eq!(run_last(&mut eval, "(swap! (atom {:a 1}) :a)"), "1");
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1)] (swap! a (fn [x])) @a)"),
        "nil"
    );
}

#[test]
fn swap_cas_accounting() {
    let mut eval = boot();
    // A `reset!` inside `f` loses the race once: `f` runs twice and
    // the retry converges (JVM: 110, same shape).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom 0) a (atom 1)] (swap! a (fn [x] (swap! calls inc) (reset! a 10) (+ x 100))) [@a @calls])"
        ),
        "[110 2]"
    );
    // A single uncontended swap calls `f` exactly once.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom 0) a (atom 1)] (swap! a (fn [x] (swap! calls inc) (inc x))) [@a @calls])"
        ),
        "[2 1]"
    );
}

#[test]
fn swap_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(swap! 1 inc)"),
        cce("java.lang.Long", "clojure.lang.IAtom")
    );
    // The receiver checks before the fn (both bad still reports `IAtom`).
    assert_eq!(
        uncaught(&mut eval, "(swap! 1 2)"),
        cce("java.lang.Long", "clojure.lang.IAtom")
    );
    // Nil receivers trip the overload-specific NPE.
    assert_eq!(
        uncaught(&mut eval, "(swap! nil inc)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.swap(clojure.lang.IFn)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! nil + 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.swap(clojure.lang.IFn, Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! nil + 1 2)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.swap(clojure.lang.IFn, Object, Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! nil + 1 2 3)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.swap(clojure.lang.IFn, Object, Object, clojure.lang.ISeq)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/swap!"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1) 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    // Nil fns trip the call-shape NPE.
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1) nil :x)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object, Object)\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1) nil :x :y)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object, Object, Object)\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap! (atom 1) nil :x :y :z)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.applyTo(clojure.lang.ISeq)\" because \"f\" is null"
    );
    // A veto keeps the state and fires no watches.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 0 :validator even?)] (swap! a inc))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def sv (atom 0 :validator even?)) (def svlog (atom [])) (add-watch sv :k (fn [k r o n] (swap! svlog conj n))) (try (swap! sv inc) (catch IllegalStateException e :vetoed)) [@sv @svlog])"
        ),
        "[0 []]"
    );
    // A throwing `f` also keeps the state and fires no watches.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def sf (atom 1)) (def sflog (atom [])) (add-watch sf :k (fn [k r o n] (swap! sflog conj n))) [(try (swap! sf (fn [x] (throw (ex-info \"fx\" {})))) (catch ExceptionInfo e :thrown)) @sf @sflog])"
        ),
        "[:thrown 1 []]"
    );
    // `f` receives exactly `[old]` (plus the extra args).
    let arity = uncaught(&mut eval, "(swap! (atom 1) (fn [a b c] a))");
    assert!(
        arity.starts_with("clojure.lang.ArityException | Wrong number of args (1) passed to: "),
        "unexpected f-arity shape: {arity}"
    );
}

#[test]
fn reset_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(reset! (atom 1) 7)"), "7");
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1)] (reset! a nil) @a)"),
        "nil"
    );
    // Watches fire even when the value is unchanged.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (reset! a 1) @log)"
        ),
        "[[1 1]]"
    );
}

#[test]
fn reset_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(reset! 1 2)"),
        cce("java.lang.Long", "clojure.lang.IAtom")
    );
    assert_eq!(
        uncaught(&mut eval, "(reset! nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.reset(Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset! (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/reset!"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset! (atom 1) 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/reset!"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 0 :validator even?)] (reset! a 3))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def rs (atom 0 :validator even?)) (try (reset! rs 3) (catch IllegalStateException e :vetoed)) @rs)"
        ),
        "0"
    );
}

#[test]
fn cas_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] [(compare-and-set! a 1 2) @a])"
        ),
        "[true 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] [(compare-and-set! a 99 2) @a])"
        ),
        "[false 1]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (atom :a)] (compare-and-set! a :a :b))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v [1] a (atom v)] (compare-and-set! a v :new))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] [(compare-and-set! a 1 nil) @a])"
        ),
        "[true nil]"
    );
    // A lost race fires no watches; a won race fires like any publish.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (compare-and-set! a 99 2) @log)"
        ),
        "[]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (compare-and-set! a 1 2) @log)"
        ),
        "[[1 2]]"
    );
}

// Fence 4a: `compare-and-set!` compares references (the JVM's
// `AtomicReference.compareAndSet` is `==`), not values — a
// distinct-but-equal box misses (oracle P4).
#[test]
fn cas_identity() {
    let mut eval = boot();
    // Cache-hit longs and interned keywords hit by value.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 5)] (compare-and-set! a (+ 2 3) :x))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom :x)] (compare-and-set! a (keyword \"x\") :y))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom \"a\")] (compare-and-set! a \"a\" :x))"
        ),
        "true"
    );
    // Uncached longs, doubles, fresh strings/symbols miss despite `=`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1000)] [(compare-and-set! a (+ 999 1) :x) @a])"
        ),
        "[false 1000]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1.5)] (compare-and-set! a 1.5 :x))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom (str \"a\" \"\"))] [(compare-and-set! a (str \"a\" \"\") :x) @a])"
        ),
        "[false \"a\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (symbol \"q\")] (let [a (atom s)] (compare-and-set! a (symbol \"q\") :y)))"
        ),
        "false"
    );
    // The validator still runs on a reference miss (construction +
    // attempt), and no watch fires.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom 0) a (atom 1000 :validator (fn [_] (swap! calls inc) true))] (compare-and-set! a (+ 999 1) :x) @calls)"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1000)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (compare-and-set! a (+ 999 1) :x) [@log @a])"
        ),
        "[[] 1000]"
    );
}

#[test]
fn cas_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(compare-and-set! 1 1 2)"),
        cce("java.lang.Long", "clojure.lang.IAtom")
    );
    assert_eq!(
        uncaught(&mut eval, "(compare-and-set! nil 1 2)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom.compareAndSet(Object, Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(compare-and-set! (atom 1) 1)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/compare-and-set!"
    );
    // Validates before comparing — even on mismatch.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 0 :validator even?)] (compare-and-set! a 0 3))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 0 :validator even?)] (compare-and-set! a 99 3))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
}

#[test]
fn watch_rows() {
    let mut eval = boot();
    // Both fns return the ref itself (same cell: publishing through
    // the return value is visible through the original).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] (reset! (add-watch a :k (fn [k r o n])) 5) @a)"
        ),
        "5"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] (add-watch a :k (fn [k r o n])) (reset! (remove-watch a :k) 6) @a)"
        ),
        "6"
    );
    // Watch fns get `[key ref old new]` (the ref already publishes).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [k (deref r) o n]))) (reset! a 2) @log)"
        ),
        "[[:k 2 1 2]]"
    );
    // Unconditional fire (the P2 surprise: `identity` still fires).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (swap! a identity) @log)"
        ),
        "[[1 1]]"
    );
    // Removal stops the fire; missing keys are a no-op; keys need not
    // be keywords; re-adding a key replaces the fn.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (remove-watch a :k) (reset! a 2) @log)"
        ),
        "[]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1)] (remove-watch a :nope) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] (add-watch a [1] (fn [k r o n])) :ok)"
        ),
        ":ok"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 0)] (add-watch a :k (fn [k r o n] (swap! log conj :old))) (add-watch a :k (fn [k r o n] (swap! log conj :new))) (reset! a 1) @log)"
        ),
        "[:new]"
    );
    // Nil fns install but never fire.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k nil) (add-watch a :j (fn [k r o n] (swap! log conj :j))) (reset! a 2) @log)"
        ),
        "[:j]"
    );
    // Watches publish again (lock-free fire — the nested publish
    // completes before the outer watch logs).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (when (< n 3) (swap! a inc)) (swap! log conj n))) (reset! a 2) [@a @log])"
        ),
        "[3 [3 2]]"
    );
    // A throwing watch propagates, but the state stays published.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def wa (atom 1)) (add-watch wa :k (fn [k r o n] (throw (ex-info \"wx\" {})))) [(try (reset! wa 2) (catch ExceptionInfo e :caught)) @wa])"
        ),
        "[:caught 2]"
    );
}

#[test]
fn watch_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(add-watch 1 :k (fn [k r o n]))"),
        cce("java.lang.Long", "clojure.lang.IRef")
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-watch 1 :k)"),
        cce("java.lang.Long", "clojure.lang.IRef")
    );
    assert_eq!(
        uncaught(&mut eval, "(add-watch nil :k (fn [k r o n]))"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IRef.addWatch(Object, clojure.lang.IFn)\" because \"reference\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-watch nil :k)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IRef.removeWatch(Object)\" because \"reference\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(add-watch (atom 1) :k 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(add-watch (atom 1) :k)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/add-watch"
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-watch (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/remove-watch"
    );
}

#[test]
fn delay_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(force (delay 42))"), "42");
    assert_eq!(run_last(&mut eval, "(deref (delay 42))"), "42");
    assert_eq!(run_last(&mut eval, "@(delay 42)"), "42");
    // The body runs once, however forced.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def dc (atom 0)) (def dd (delay (swap! dc inc))) (deref dd) (force dd) @dc)"
        ),
        "1"
    );
    assert_eq!(run_last(&mut eval, "(deref (delay 1 2))"), "2");
    assert_eq!(run_last(&mut eval, "(deref (delay))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(let [d (delay nil)] [(deref d) (realized? d)])"),
        "[nil true]"
    );
    // Lexicals capture at creation; dynamic bindings read at force.
    assert_eq!(
        run_last(&mut eval, "(let [x 40] (force (delay (+ x 2))))"),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic dfd 1) (let [d (delay dfd)] (binding [dfd 2] (force d))))"
        ),
        "2"
    );
    assert_eq!(class_of(&mut eval, "(delay 1)"), "clojure.lang.Delay");
    // `delay*` shadows like `lazy-seq*` (user definitions win).
    assert_eq!(run_last(&mut eval, "(let [delay* 5] delay*)"), "5");
}

#[test]
fn delay_throw_cached() {
    let mut eval = boot();
    // Failures cache (never re-run) and still realize; every force
    // rethrows the same failure.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def dtc (atom 0)) (def dtd (delay (swap! dtc inc) (throw (ex-info \"boom\" {:n 1})))) [(try (deref dtd) (catch ExceptionInfo e :e1)) (try (force dtd) (catch ExceptionInfo e :e2)) @dtc (realized? dtd)])"
        ),
        "[:e1 :e2 1 true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def dte (delay (throw (ex-info \"boom\" {:n 7})))) (try (force dte) (catch ExceptionInfo e :x)) (try (force dte) (catch ExceptionInfo e [(ex-message e) (ex-data e)])))"
        ),
        "[\"boom\" {:n 7}]"
    );
    assert_eq!(
        uncaught(&mut eval, "(force (delay (throw (ex-info \"boom\" {}))))"),
        "clojure.lang.ExceptionInfo | boom"
    );
    // A body forcing its own delay errors cleanly (the JVM recurses
    // into `StackOverflowError` — pinned port divergence).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (declare rd) (def rd (delay @rd)) (force rd))"
        ),
        "java.lang.IllegalStateException | Delay is already being forced"
    );
}

#[test]
fn realized_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(realized? (delay 1))"), "false");
    assert_eq!(
        run_last(&mut eval, "(let [d (delay 1)] (force d) (realized? d))"),
        "true"
    );
    // Lazy seqs are `IPending` too (false until the first step forces).
    assert_eq!(
        run_last(&mut eval, "(realized? (map inc [1 2 3]))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(let [s (map inc [1])] (first s) (realized? s))"),
        "true"
    );
    // Futures and promises (S6B): pending until settled (failures
    // and cancellations count as realized).
    assert_eq!(
        run_last(&mut eval, "(let [f (future @(promise))] (realized? f))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(let [f (future 1)] @f (realized? f))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(let [p (promise)] (deliver p 1) (realized? p))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (future (throw (ex-info \"b\" {})))] (try @f (catch Exception e :x)) (realized? f))"
        ),
        "true"
    );
}

#[test]
fn realized_errors() {
    let mut eval = boot();
    // Atoms are not pending (the P1 surprise).
    assert_eq!(
        uncaught(&mut eval, "(realized? (atom 1))"),
        cce("clojure.lang.Atom", "clojure.lang.IPending")
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? 1)"),
        cce("java.lang.Long", "clojure.lang.IPending")
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? [1])"),
        cce("clojure.lang.PersistentVector", "clojure.lang.IPending")
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IPending.isRealized()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(realized?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/realized?"
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? (delay 1) 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/realized?"
    );
}

#[test]
fn force_rows() {
    let mut eval = boot();
    // Non-delays pass through untouched.
    assert_eq!(run_last(&mut eval, "(force 42)"), "42");
    assert_eq!(run_last(&mut eval, "(force nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(force \"s\")"), "\"s\"");
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 1)] (reset! (force a) 9) @a)"),
        "9"
    );
    assert_eq!(
        uncaught(&mut eval, "(force)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/force"
    );
    assert_eq!(
        uncaught(&mut eval, "(force 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/force"
    );
}

#[test]
fn memoize_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) f (memoize (fn [x] (swap! c inc) (* x 2)))] [(f 21) (f 21) @c (f 1) @c])"
        ),
        "[42 42 1 2 2]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [f (memoize +)] [(f 1 2) (f 1 2 3)])"),
        "[3 6]"
    );
    // Nullary calls and nil results cache too (`find`-based, so a
    // cached nil still hits).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) f (memoize (fn [] (swap! c inc)))] [(f) (f) @c])"
        ),
        "[1 1 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) f (memoize (fn [x] (swap! c inc) nil))] [(f 1) (f 1) @c])"
        ),
        "[nil nil 1]"
    );
    assert_eq!(run_last(&mut eval, "(fn? (memoize +))"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (memoize) (catch clojure.lang.ArityException e :arity))"
        ),
        ":arity"
    );
}

#[test]
fn meta_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(meta (atom 1 :meta {:k :v}))"),
        "{:k :v}"
    );
    assert_eq!(
        run_last(&mut eval, "(alter-meta! (atom 1) assoc :a 1)"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(reset-meta! (atom 1) {:a 1})"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] (alter-meta! a assoc :a 1) (meta a))"
        ),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1 :meta {:a 1})] (reset-meta! a nil) (meta a))"
        ),
        "nil"
    );
    // Atoms are not `IObj` (but are `IReference`); delays are neither.
    // (T1192 FX2: the `IObj` path reports the JVM's cast text;
    // `IReference` keeps the S5B shape below.)
    assert_eq!(
        uncaught(&mut eval, "(with-meta (atom 1) {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.Atom cannot be cast to class clojure.lang.IObj (clojure.lang.Atom and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(vary-meta (atom 1) assoc :a 1)"),
        "java.lang.ClassCastException | class clojure.lang.Atom cannot be cast to class clojure.lang.IObj (clojure.lang.Atom and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta (delay 1) {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.Delay cannot be cast to class clojure.lang.IObj (clojure.lang.Delay and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    assert_eq!(run_last(&mut eval, "(meta (delay 1))"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(alter-meta! (delay 1) assoc :a 1)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Delay to clojure.lang.IReference"
    );
    // `*print-meta*` prefixes atom metas (never delays — not `IMeta`).
    let with_meta = run_last(
        &mut eval,
        "(binding [*print-meta* true] (pr-str (atom 1 :meta {:a 1})))",
    );
    assert!(
        with_meta.starts_with("\"^{:a 1} #object[clojure.lang.Atom 0x"),
        "missing meta prefix: {with_meta}"
    );
    let delay_meta = run_last(
        &mut eval,
        "(binding [*print-meta* true] (pr-str (delay 1)))",
    );
    assert!(
        delay_meta.starts_with("\"#object[clojure.lang.Delay 0x"),
        "unexpected meta prefix: {delay_meta}"
    );
}

#[test]
fn print_str_rows() {
    let mut eval = boot();
    // Transient-style `#object` rendering (never forcing).
    assert_atom_object(&run_last(&mut eval, "(atom 1)"));
    assert_delay_object(&run_last(&mut eval, "(delay 1)"));
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def pd (atom 0)) (def pdd (delay (swap! pd inc))) (pr-str pdd) @pd)"
        ),
        "0"
    );
    // `str` renders the `toString` (`Class@hex`), like the JVM.
    let str_atom = run_last(&mut eval, "(str (atom 1))");
    assert!(
        str_atom.starts_with("\"clojure.lang.Atom@")
            && str_atom.len() > "\"clojure.lang.Atom@\"".len(),
        "bad atom str: {str_atom}"
    );
    let str_delay = run_last(&mut eval, "(str (delay 1))");
    assert!(
        str_delay.starts_with("\"clojure.lang.Delay@")
            && str_delay.len() > "\"clojure.lang.Delay@\"".len(),
        "bad delay str: {str_delay}"
    );
    let str_vec = run_last(&mut eval, "(str [(atom 1)])");
    assert!(
        str_vec.starts_with("\"[#object[clojure.lang.Atom 0x"),
        "bad vec str: {str_vec}"
    );
    let printed = run_last(&mut eval, "(with-out-str (print (atom 1)))");
    assert!(
        printed.starts_with("\"#object[clojure.lang.Atom 0x"),
        "bad print: {printed}"
    );
    assert!(
        run_last(&mut eval, "(format \"%s\" (atom 1))").starts_with("\"clojure.lang.Atom@"),
        "bad format %s"
    );
    // Identity equality (never by value).
    assert_eq!(run_last(&mut eval, "(let [a (atom 1)] (= a a))"), "true");
    assert_eq!(run_last(&mut eval, "(= (atom 1) (atom 1))"), "false");
    assert_eq!(run_last(&mut eval, "(let [d (delay 1)] (= d d))"), "true");
    assert_eq!(run_last(&mut eval, "(= (delay 1) (delay 1))"), "false");
    assert_eq!(run_last(&mut eval, "(= (atom 1) (delay 1))"), "false");
    assert_eq!(run_last(&mut eval, "(not= (atom 1) (atom 1))"), "true");
    // Identity hashes (values never pinned — per-process ids).
    assert_eq!(run_last(&mut eval, "(integer? (hash (atom 1)))"), "true");
    assert_eq!(run_last(&mut eval, "(integer? (hash (delay 1)))"), "true");
    // `seq`/`count`/`nth` error shapes.
    assert_eq!(
        uncaught(&mut eval, "(seq (atom 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Atom"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq (delay 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Delay"
    );
    assert_eq!(
        uncaught(&mut eval, "(first (atom 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Atom"
    );
    assert_eq!(
        uncaught(&mut eval, "(count (atom 1))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Atom"
    );
    assert_eq!(
        uncaught(&mut eval, "(count (delay 1))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Delay"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (atom 1) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Atom"
    );
    // Predicates and collection casts.
    for (src, want) in [
        ("(coll? (atom 1))", "false"),
        ("(seq? (atom 1))", "false"),
        ("(counted? (atom 1))", "false"),
        ("(counted? (delay 1))", "false"),
        ("(fn? (atom 1))", "false"),
        ("(ifn? (delay 1))", "false"),
        ("(map? (atom 1))", "false"),
        ("(vector? (atom 1))", "false"),
        ("(var? (atom 1))", "false"),
        ("(get (atom {:a 1}) :a)", "nil"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "for {src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(find (atom {}) :a)"),
        "java.lang.IllegalArgumentException | find not supported on type: clojure.lang.Atom"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj (atom 1) 2)"),
        cce("clojure.lang.Atom", "clojure.lang.IPersistentCollection")
    );
    assert_eq!(
        uncaught(&mut eval, "(compare (atom 1) 1)"),
        cce("clojure.lang.Atom", "java.lang.Comparable")
    );
    // Invoking an atom keeps the evaluator's cast shape (pre-existing,
    // without the JDK module verbiage).
    assert_eq!(
        uncaught(&mut eval, "((atom 1))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Atom to clojure.lang.IFn"
    );
}

fn assert_future_object(pr: &str) {
    assert_object_shape(pr, "clojure.core$future_call$reify");
}

fn assert_promise_object(pr: &str) {
    assert_object_shape(pr, "clojure.core$promise$reify");
}

#[test]
fn evaluator_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Evaluator>();
}

#[test]
fn future_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "@(future 1)"), "1");
    assert_eq!(run_last(&mut eval, "(deref (future 1 2 3))"), "3");
    assert_eq!(
        run_last(&mut eval, "(deref (future-call (fn [] 41)))"),
        "41"
    );
    // Bindings convey from the spawning thread (P3).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic s6b-dx 1) (binding [s6b-dx 2] @(future s6b-dx)))"
        ),
        "2"
    );
    // ... but stay thread-local: the body cannot rebind the parent.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic s6b-dy 1) @(future (binding [s6b-dy 9] s6b-dy)) s6b-dy)"
        ),
        "1"
    );
    assert_eq!(
        class_of(&mut eval, "(future 1)"),
        "clojure.core$future_call$reify"
    );
    assert_future_object(&run_last(&mut eval, "(future 1)"));
    // `str` renders the `toString` (`Class@hex`).
    let rendered = run_last(&mut eval, "(str (future 1))");
    assert!(
        rendered.starts_with("\"clojure.core$future_call$reify@"),
        "bad future str: {rendered}"
    );
    // Failures never surface at spawn (no eager validation).
    assert_future_object(&run_last(&mut eval, "(future-call 5)"));
    // `force` passes futures through.
    assert_eq!(run_last(&mut eval, "(future? (force (future 1)))"), "true");
    assert_eq!(
        uncaught(&mut eval, "(future-call)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/future-call"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-call (fn []) (fn []))"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/future-call"
    );
}

#[test]
fn future_pred_rows() {
    let mut eval = boot();
    // `future?` is true of futures only (promises are not Futures).
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (map future? [(future 1) (promise) 1 nil (delay 1) \"s\"]))"
        ),
        "[true false false false false false]"
    );
    // `future-done?` reports settledness (completion AND cancel).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (future 1)] [@f (future-done? f) (realized? f)])"
        ),
        "[1 true true]"
    );
    assert_eq!(
        run_last(&mut eval, "(future-done? (future @(promise)))"),
        "false"
    );
    // Promises fail the `Future` cast (they only share the
    // `IBlockingDeref` timeout path).
    assert_eq!(
        uncaught(&mut eval, "(future-done? (promise))"),
        cce("clojure.core$promise$reify", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(future-done? (delay 1))"),
        cce("clojure.lang.Delay", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(future-done? 5)"),
        cce("java.lang.Long", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(future-done? nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.isDone()\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-done?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/future-done?"
    );
}

#[test]
fn future_throw_cached() {
    let mut eval = boot();
    eval.install_native(
        "user",
        "s6b-boom",
        std::sync::Arc::new(|_, _| {
            Err(clj_eval::NativeError::new(
                clj_value::ThrowClass::Exception,
                "boom".to_owned(),
            ))
        }),
    );
    run(&mut eval, "(def s6b-ff (future-call s6b-boom))");
    // A failure rethrows the same shape on every deref (the first
    // deref rendezvous with completion; only then is `realized?`
    // deterministic).
    let shape = "java.util.concurrent.ExecutionException | java.lang.Exception: boom";
    assert_eq!(uncaught(&mut eval, "@s6b-ff"), shape);
    assert_eq!(run_last(&mut eval, "(realized? s6b-ff)"), "true");
    assert_eq!(run_last(&mut eval, "(future-done? s6b-ff)"), "true");
    assert_eq!(uncaught(&mut eval, "@s6b-ff"), shape);
    assert_eq!(uncaught(&mut eval, "(deref s6b-ff 5000 :t)"), shape);
    // Wrappers are fresh per `deref`; the cause is the cached object.
    assert_eq!(
        run_last(
            &mut eval,
            "(try @s6b-ff (catch java.util.concurrent.ExecutionException e1 (try @s6b-ff (catch java.util.concurrent.ExecutionException e2 [(= e1 e2) (= (ex-cause e1) (ex-cause e2))]))))"
        ),
        "[false true]"
    );
    // A non-function fn fails inside the body (never at spawn).
    assert_eq!(
        uncaught(&mut eval, "@(future-call 5)"),
        "java.util.concurrent.ExecutionException | java.lang.ClassCastException: Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    // The `ex-info` message carries its data map (`toString` rule).
    assert_eq!(
        uncaught(&mut eval, "@(future (throw (ex-info \"boom\" {:n 1})))"),
        "java.util.concurrent.ExecutionException | clojure.lang.ExceptionInfo: boom {:n 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try @(future (throw (ex-info \"boom\" {:n 1}))) (catch java.util.concurrent.ExecutionException e [(ex-message e) (ex-message (ex-cause e)) (ex-data (ex-cause e))]))"
        ),
        "[\"clojure.lang.ExceptionInfo: boom {:n 1}\" \"boom\" {:n 1}]"
    );
    assert_eq!(
        uncaught(&mut eval, "@(future (throw (ex-info nil {:n 1})))"),
        "java.util.concurrent.ExecutionException | clojure.lang.ExceptionInfo: null {:n 1}"
    );
    assert_eq!(
        uncaught(&mut eval, "@(future (throw (ex-info \"x\" nil)))"),
        "java.util.concurrent.ExecutionException | clojure.lang.ExceptionInfo: x {}"
    );
    // `ExecutionException` extends `Exception` (not `RuntimeException`).
    assert_eq!(
        run_last(
            &mut eval,
            "(try @(future (throw (ex-info \"b\" {}))) (catch RuntimeException e :r) (catch Exception e :e))"
        ),
        ":e"
    );
}

#[test]
fn deref_timeout_rows() {
    let mut eval = boot();
    // Only futures/promises honor timeouts: atoms, delays, vars, and
    // the rest fail the `Future` cast first (P5).
    assert_eq!(
        uncaught(&mut eval, "(deref (atom 1) 50 :t)"),
        cce("clojure.lang.Atom", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref (delay 1) 50 :t)"),
        cce("clojure.lang.Delay", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(do (def s6b-tv 1) (deref #'s6b-tv 50 :t))"),
        cce("clojure.lang.Var", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref 5 50 :t)"),
        cce("java.lang.Long", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(deref nil 50 :t)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.get(long, java.util.concurrent.TimeUnit)\" because \"fut\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(deref (future 1) 50)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/deref"
    );
    // The timeout converts like `.get`'s `long` (non-numbers fail to
    // `Number`; `nil` trips the `doubleValue` NPE).
    for (msec, want) in [
        ("\"50\"", cce("java.lang.String", "java.lang.Number")),
        (":x", cce("clojure.lang.Keyword", "java.lang.Number")),
        ("true", cce("java.lang.Boolean", "java.lang.Number")),
        ("\\a", cce("java.lang.Character", "java.lang.Number")),
    ] {
        assert_eq!(
            uncaught(&mut eval, &format!("(deref (future 1) {msec} :t)")),
            want,
            "for {msec}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(deref (future 1) nil :t)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    // Doubles truncate (`0.9` waits nothing); negatives and zero
    // check once without waiting.
    assert_eq!(run_last(&mut eval, "(deref (promise) 0.9 :t)"), ":t");
    assert_eq!(run_last(&mut eval, "(deref (promise) -5 :t)"), ":t");
    assert_eq!(run_last(&mut eval, "(deref (promise) 0 :t)"), ":t");
    assert_eq!(run_last(&mut eval, "(deref (future 1) 5000.9 :t)"), "1");
    // Conversion runs before the outcome: bad timeouts fail even on
    // settled futures (and cancelled ones throw only with good ones).
    assert_eq!(
        uncaught(&mut eval, "(let [f (future :v)] @f (deref f \"50\" :t))"),
        cce("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        run_last(&mut eval, "(let [f (future :v)] @f (deref f 50 :t))"),
        ":v"
    );
}

#[test]
fn future_timeout_rows() {
    let mut eval = boot();
    // A pending body times out (the latch rendezvous keeps this
    // deterministic: the body cannot finish before the timeout).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise)] (deref (future @p :never) 50 :t))"
        ),
        ":t"
    );
    // A delivery racing the wait lands first (the main thread
    // delivers before the body can time out).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) f (future (deref p 5000 :slow))] (deliver p :fast) @f)"
        ),
        ":fast"
    );
    assert_eq!(run_last(&mut eval, "(deref (promise) 50 :t)"), ":t");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise)] (deliver p :v) (deref p 50 :t))"
        ),
        ":v"
    );
}

#[test]
fn future_cancel_rows() {
    let mut eval = boot();
    // Cancelling a pending future settles it: both `deref` arities
    // throw `CancellationException` (the timeout value is ignored).
    run(&mut eval, "(def s6b-fc (future @(promise) :never))");
    assert_eq!(run_last(&mut eval, "(future-cancel s6b-fc)"), "true");
    assert_eq!(run_last(&mut eval, "(future-done? s6b-fc)"), "true");
    assert_eq!(run_last(&mut eval, "(realized? s6b-fc)"), "true");
    let cancelled = "java.util.concurrent.CancellationException | ";
    assert_eq!(uncaught(&mut eval, "@s6b-fc"), cancelled);
    assert_eq!(uncaught(&mut eval, "(deref s6b-fc 5000 :t)"), cancelled);
    // There is no message at all (`ex-message` → nil, like the
    // JVM's no-arg `CancellationException`).
    assert_eq!(
        run_last(
            &mut eval,
            "(try @s6b-fc (catch java.util.concurrent.CancellationException e (ex-message e)))"
        ),
        "nil"
    );
    // `CancellationException` extends `IllegalStateException`.
    assert_eq!(
        run_last(
            &mut eval,
            "(try @s6b-fc (catch IllegalStateException e :ise) (catch Throwable t :other))"
        ),
        ":ise"
    );
    // Cancelling twice, or a completed future, reports false.
    assert_eq!(run_last(&mut eval, "(future-cancel s6b-fc)"), "false");
    assert_eq!(
        run_last(&mut eval, "(let [f (future 1)] @f (future-cancel f))"),
        "false"
    );
    // A bad timeout still converts first on a cancelled future.
    assert_eq!(
        uncaught(&mut eval, "(deref s6b-fc \"50\" :t)"),
        cce("java.lang.String", "java.lang.Number")
    );
    // Only futures cancel (promises fail the `Future` cast).
    assert_eq!(
        uncaught(&mut eval, "(future-cancel (promise))"),
        cce("clojure.core$promise$reify", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancel 5)"),
        cce("java.lang.Long", "java.util.concurrent.Future")
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancel nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.cancel(boolean)\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancel)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/future-cancel"
    );
}

#[test]
fn future_cancel_running_discards() {
    let mut eval = boot();
    // A running body cannot be interrupted (pinned divergence): it
    // runs to completion while its late result is discarded. The
    // second latch rendezvous with the body's completion.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) q (promise) log (atom []) f (future (do @p (swap! log conj :ran) (deliver q :body-done) :done))] (future-cancel f) (deliver p :go) [@q @log (future-done? f)])"
        ),
        "[:body-done [:ran] true]"
    );
    run(&mut eval, "(def s6b-fcr (future @(promise) :never))");
    run_last(&mut eval, "(future-cancel s6b-fcr)");
    assert_eq!(
        uncaught(&mut eval, "@s6b-fcr"),
        "java.util.concurrent.CancellationException | "
    );
}

#[test]
fn future_ns_sharing() {
    let mut eval = boot();
    // A `def` inside a future body is globally visible (JVM
    // namespaces are global).
    assert_eq!(
        run_last(&mut eval, "(do @(future (def s6b-shared 42)) s6b-shared)"),
        "42"
    );
    // And a body sees `def`s committed after it spawns (through
    // `resolve`: the analyzer binds eagerly, like the JVM).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) f (future @p (deref (resolve 's6b-late)))] (def s6b-late :late) (deliver p :go) @f)"
        ),
        ":late"
    );
    // Conveyed bindings travel with the body (re-proved across the
    // shared registry: the var resolves on the worker thread).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic s6b-bv 1) (binding [s6b-bv 7] @(future (+ s6b-bv 1))))"
        ),
        "8"
    );
}

#[test]
fn promise_rows() {
    let mut eval = boot();
    // First delivery wins (returning the promise); later ones are nil.
    assert_eq!(
        run_last(&mut eval, "(let [p (promise)] (= p (deliver p 1)))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise)] (deliver p 1) [(deliver p 2) @p (realized? p)])"
        ),
        "[nil 1 true]"
    );
    // A delivered `nil` still realizes (delivery is a transition).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise)] [(= p (deliver p nil)) @p (realized? p) (deliver p 2)])"
        ),
        "[true nil true nil]"
    );
    assert_eq!(run_last(&mut eval, "(realized? (promise))"), "false");
    assert_promise_object(&run_last(&mut eval, "(promise)"));
    assert_eq!(
        class_of(&mut eval, "(promise)"),
        "clojure.core$promise$reify"
    );
    let rendered = run_last(&mut eval, "(str (promise))");
    assert!(
        rendered.starts_with("\"clojure.core$promise$reify@"),
        "bad promise str: {rendered}"
    );
    // Pending `deref` blocks until a delivery lands (the future body
    // below cannot proceed first — the main thread delivers).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) f (future @p)] (deliver p :v) @f)"
        ),
        ":v"
    );
    // Promises invoke (one arg delivers); `deliver` is an invocation.
    assert_eq!(
        run_last(&mut eval, "(let [p (promise)] [(= p (p 5)) @p (p 6)])"),
        "[true 5 nil]"
    );
    assert_eq!(run_last(&mut eval, "(deliver + 5)"), "5");
    assert_eq!(run_last(&mut eval, "(ifn? (promise))"), "true");
    assert_eq!(run_last(&mut eval, "(ifn? (future 1))"), "false");
    // ... so non-functions fail exactly like a call (the evaluator's
    // cast shapes, without the JDK module verbiage).
    assert_eq!(
        uncaught(&mut eval, "(deliver 5 6)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(
        uncaught(&mut eval, "(deliver nil 1)"),
        "java.lang.NullPointerException | Cannot invoke nil as a function"
    );
    // Other arities are the `ArityException` (the JVM throws
    // `AbstractMethodError` for its undeclared reify overloads).
    assert_eq!(
        uncaught(&mut eval, "((promise))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core$promise$reify"
    );
    assert_eq!(
        uncaught(&mut eval, "((promise) 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core$promise$reify"
    );
    assert_eq!(
        uncaught(&mut eval, "(promise 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/promise"
    );
    assert_eq!(
        uncaught(&mut eval, "(deliver (promise))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/deliver"
    );
    assert_eq!(
        uncaught(&mut eval, "(deliver (promise) 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/deliver"
    );
}

#[test]
fn promise_threaded_deliver() {
    use std::sync::{Arc, Barrier};

    // A Rust-spawned deliverer hands the value to a blocked `deref`
    // (barrier rendezvous — no sleeps; either order yields the value,
    // while a broken block would hang or misdeliver).
    let mut eval = boot();
    let vs = eval.eval_source("(promise)").unwrap();
    let Value::Promise(p) = vs[0].clone() else {
        panic!("expected a promise");
    };
    let sym = clj_value::Symbol::new(Some("user"), "s6b-tp");
    let id = eval.registry.intern(&sym);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(Value::Promise(p.clone()));
    });
    let barrier = Arc::new(Barrier::new(2));
    let rendezvous = Arc::clone(&barrier);
    let deliverer = std::thread::spawn(move || {
        rendezvous.wait();
        assert!(p.deliver(Value::Int(7)));
    });
    barrier.wait();
    assert_eq!(run_last(&mut eval, "@s6b-tp"), "7");
    deliverer.join().unwrap();
    assert_eq!(run_last(&mut eval, "@s6b-tp"), "7");
}

#[test]
fn promise_meta_rows() {
    let mut eval = boot();
    // Futures and promises carry `IObj` meta (the reify behavior).
    assert_eq!(run_last(&mut eval, "(meta (promise))"), "nil");
    assert_eq!(run_last(&mut eval, "(meta (future 1))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (promise) {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (future 1) {:a 1}))"),
        "{:a 1}"
    );
    // `with-meta` shares the cell but not the meta — so the copy
    // compares unequal (equality conjoins cell and meta).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) q (with-meta p {:a 1})] [(meta p) (meta q) (= p q) (do (deliver p :v) @q)])"
        ),
        "[nil {:a 1} false :v]"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (vary-meta (promise) assoc :a 1))"),
        "{:a 1}"
    );
    // ... but they are not `IReference`s (no validator/watches;
    // the stable cast shape, like the other `alter-meta!` targets).
    assert_eq!(
        uncaught(&mut eval, "(alter-meta! (promise) assoc :a 1)"),
        "java.lang.ClassCastException | Cannot cast clojure.core$promise$reify to clojure.lang.IReference"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-meta! (future 1) {:a 1})"),
        "java.lang.ClassCastException | Cannot cast clojure.core$future_call$reify to clojure.lang.IReference"
    );
}

#[test]
fn future_stress_swap() {
    // Eight futures race `swap!` over one shared atom (2,000
    // increments, exact total — atoms linearize, like the JVM).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6b-n 8) (def s6b-m 250) (def s6b-a (atom 0)) (def s6b-futs (vec (map (fn [_] (future (dotimes [_ s6b-m] (swap! s6b-a inc)))) (range s6b-n)))) (doseq [f s6b-futs] @f) @s6b-a)"
        ),
        "2000"
    );
}

fn assert_agent_object(pr: &str) {
    assert_object_shape(pr, "clojure.lang.Agent");
}

#[test]
fn agent_ctor_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(deref (agent 1))"), "1");
    assert_eq!(run_last(&mut eval, "(deref (agent nil))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(meta (agent 1 :meta {:k :v}))"),
        "{:k :v}"
    );
    assert_eq!(run_last(&mut eval, "(meta (agent 1))"), "nil");
    assert_eq!(run_last(&mut eval, "(meta (agent 1 :meta nil))"), "nil");
    // Falsy validators/handlers never install; unknown options are ignored.
    assert_eq!(
        run_last(&mut eval, "(do (agent 2 :validator even?) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (agent 1 :validator nil) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (agent 1 :error-handler nil) :ok)"),
        ":ok"
    );
    assert_eq!(run_last(&mut eval, "(do (agent 1 :foo 2) :ok)"), ":ok");
    // Duplicate options: last wins (hash-map build).
    assert_eq!(
        run_last(&mut eval, "(meta (agent 1 :meta {:a 1} :meta {:b 2}))"),
        "{:b 2}"
    );
    // A bogus mode installs (behaves as `:fail` — pinned in the mode test).
    assert_eq!(
        run_last(&mut eval, "(do (agent 1 :error-mode :bogus) :ok)"),
        ":ok"
    );
    assert_eq!(class_of(&mut eval, "(agent 1)"), "clojure.lang.Agent");
    assert_agent_object(&run_last(&mut eval, "(agent 1)"));
    let rendered = run_last(&mut eval, "(str (agent 1))");
    assert!(
        rendered.starts_with("\"clojure.lang.Agent@"),
        "bad agent str: {rendered}"
    );
    // Identity equality (never by value).
    assert_eq!(run_last(&mut eval, "(let [a (agent 1)] (= a a))"), "true");
    assert_eq!(run_last(&mut eval, "(= (agent 1) (agent 1))"), "false");
    assert_eq!(run_last(&mut eval, "(= (agent 1) (atom 1))"), "false");
    assert_eq!(run_last(&mut eval, "(integer? (hash (agent 1)))"), "true");
    // Every send fn returns the agent itself.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] [(= a (send a inc)) (await a) @a])"
        ),
        "[true nil 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] [(= a (send-off a inc)) (await a) @a])"
        ),
        "[true nil 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] [(= a (send-via (executor) a inc)) (await a) @a])"
        ),
        "[true nil 1]"
    );
}

#[test]
fn agent_ctor_errors() {
    let mut eval = boot();
    // The ctor validates the initial value too.
    assert_eq!(
        uncaught(&mut eval, "(agent 2 :validator odd?)"),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        uncaught(&mut eval, "(agent 1 :validator)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :validator"
    );
    assert_eq!(
        uncaught(&mut eval, "(agent 1 :meta 5)"),
        cce("java.lang.Long", "clojure.lang.IPersistentMap")
    );
    assert_eq!(
        uncaught(&mut eval, "(agent 1 :validator 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(agent 1 :error-handler 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    // A truthy non-keyword mode fails the `Keyword` cast (falsy modes
    // fall back to the default).
    assert_eq!(
        uncaught(&mut eval, "(agent 1 :error-mode 5)"),
        cce("java.lang.Long", "clojure.lang.Keyword")
    );
    assert_eq!(
        run_last(&mut eval, "(do (agent 1 :error-mode nil) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (agent 1 :error-mode false) :ok)"),
        ":ok"
    );
    assert_eq!(
        uncaught(&mut eval, "(agent)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/agent"
    );
}

#[test]
fn agent_error_mode_rows() {
    let mut eval = boot();
    // `:continue`: a failed action leaves the state unchanged, clears
    // the error, and later sends process (`await` returns nil).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0 :error-mode :continue)] (send a (fn [s] (throw (ex-info \"x\" {})))) (send a inc) (await a) [@a (agent-error a)])"
        ),
        "[1 nil]"
    );
    // The default mode is `:fail` without a handler: the state parks
    // and the error latches (the landed latch orders every later
    // assertion; awaiting a racing latch could hang on a stuck
    // barrier instead of throwing).
    run(&mut eval, "(def s6c-fm (agent 0))");
    run(
        &mut eval,
        "(send s6c-fm (fn [s] (throw (ex-info \"boom\" {:n 1}))))",
    );
    wait_failed(&mut eval, "s6c-fm");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-fm) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[(nil? (agent-error s6c-fm)) @s6c-fm]"),
        "[false 0]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(ex-message (agent-error s6c-fm)) (ex-data (agent-error s6c-fm))]"
        ),
        "[\"boom\" {:n 1}]"
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6c-fm)"),
        "clojure.lang.ExceptionInfo"
    );
    // Every send fn and both awaits throw while failed.
    let failed = "java.lang.RuntimeException | Agent is failed, needs restart";
    assert_eq!(uncaught(&mut eval, "(send s6c-fm inc)"), failed);
    assert_eq!(uncaught(&mut eval, "(send-off s6c-fm inc)"), failed);
    assert_eq!(uncaught(&mut eval, "(send-via nil s6c-fm inc)"), failed);
    assert_eq!(uncaught(&mut eval, "(await s6c-fm)"), failed);
    assert_eq!(uncaught(&mut eval, "(await-for 5000 s6c-fm)"), failed);
    // The failure carries the latched error as its cause.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (send s6c-fm inc) (catch RuntimeException e [(ex-message (ex-cause e)) (ex-data (ex-cause e))]))"
        ),
        "[\"boom\" {:n 1}]"
    );
    // The default mode is `:continue` with a handler: the handler
    // observes `[agent error]` and the queue proceeds.
    run(&mut eval, "(def s6c-hp (promise))");
    run(
        &mut eval,
        "(def s6c-ha (agent 0 :error-handler (fn [ag ex] (deliver s6c-hp [(= ag s6c-ha) (ex-message ex)]))))",
    );
    run(
        &mut eval,
        "(send s6c-ha (fn [s] (throw (ex-info \"kb\" {}))))",
    );
    run(&mut eval, "(send s6c-ha inc)");
    assert_eq!(
        run_last(
            &mut eval,
            "[@s6c-hp (await s6c-ha) @s6c-ha (agent-error s6c-ha)]"
        ),
        "[[true \"kb\"] nil 1 nil]"
    );
    // ... but an explicit `:fail` wins over the handler default.
    run(
        &mut eval,
        "(def s6c-fh (agent 0 :error-mode :fail :error-handler (fn [ag ex] :seen)))",
    );
    run(
        &mut eval,
        "(send s6c-fh (fn [s] (throw (ex-info \"x\" {}))))",
    );
    wait_failed(&mut eval, "s6c-fh");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-fh) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[(nil? (agent-error s6c-fh)) @s6c-fh]"),
        "[false 0]"
    );
    // A bogus mode behaves as `:fail`.
    run(&mut eval, "(def s6c-bg (agent 0 :error-mode :bogus))");
    run(
        &mut eval,
        "(send s6c-bg (fn [s] (throw (ex-info \"x\" {}))))",
    );
    wait_failed(&mut eval, "s6c-bg");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-bg) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[(nil? (agent-error s6c-bg)) @s6c-bg]"),
        "[false 0]"
    );
}

#[test]
fn agent_restart_rows() {
    let mut eval = boot();
    // Failing with a queued follower: the gate holds the failing
    // action until the follower queues (both sends land while
    // healthy, deterministically).
    run(
        &mut eval,
        "(def s6c-ra (agent 0)) (def s6c-ra-gate (promise)) (send s6c-ra (fn [s] @s6c-ra-gate (throw (ex-info \"x\" {})))) (send s6c-ra inc) (deliver s6c-ra-gate :go)",
    );
    wait_failed(&mut eval, "s6c-ra");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-ra) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    // `restart-agent` returns the new state, clears the error, and
    // the held queue proceeds — while the restart itself fires no
    // watches (only the follower and the barrier do).
    run(&mut eval, "(def s6c-rlog (atom []))");
    run(
        &mut eval,
        "(add-watch s6c-ra :k (fn [k r o n] (swap! s6c-rlog conj [o n])))",
    );
    assert_eq!(run_last(&mut eval, "(restart-agent s6c-ra 10)"), "10");
    assert_eq!(
        run_last(&mut eval, "[(await s6c-ra) @s6c-ra (agent-error s6c-ra)]"),
        "[nil 11 nil]"
    );
    assert_eq!(run_last(&mut eval, "(first @s6c-rlog)"), "[10 11]");
    // `:clear-actions` drops the held queue (the barrier proves the
    // queue drained: anything held would run before it).
    run(&mut eval, "(def s6c-rclog (atom []))");
    run(
        &mut eval,
        "(def s6c-rc (agent 0)) (def s6c-rc-gate (promise)) (send s6c-rc (fn [s] @s6c-rc-gate (throw (ex-info \"x\" {})))) (send s6c-rc (fn [s] (swap! s6c-rclog conj :held) 99)) (deliver s6c-rc-gate :go)",
    );
    wait_failed(&mut eval, "s6c-rc");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-rc) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(restart-agent s6c-rc 10 :clear-actions true)"),
        "10"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(await s6c-rc) [@s6c-rc @s6c-rclog (agent-error s6c-rc)]"
        ),
        "[10 [] nil]"
    );
    // A falsy `:clear-actions` keeps the queue.
    run(
        &mut eval,
        "(def s6c-rk (agent 0)) (def s6c-rk-gate (promise)) (send s6c-rk (fn [s] @s6c-rk-gate (throw (ex-info \"x\" {})))) (send s6c-rk inc) (deliver s6c-rk-gate :go)",
    );
    wait_failed(&mut eval, "s6c-rk");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-rk) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(restart-agent s6c-rk 10 :clear-actions false)"),
        "10"
    );
    assert_eq!(run_last(&mut eval, "(await s6c-rk) @s6c-rk"), "11");
    // The new state validates: a veto keeps the agent failed with
    // its old state and error.
    run(&mut eval, "(def s6c-rv (agent 0 :validator even?))");
    run(
        &mut eval,
        "(send s6c-rv (fn [s] (throw (ex-info \"x\" {}))))",
    );
    wait_failed(&mut eval, "s6c-rv");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-rv) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        uncaught(&mut eval, "(restart-agent s6c-rv 3)"),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        run_last(&mut eval, "[@s6c-rv (nil? (agent-error s6c-rv))]"),
        "[0 false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(restart-agent s6c-rv 4) @s6c-rv (agent-error s6c-rv)]"
        ),
        "[4 4 nil]"
    );
    // Restarting an unfailed agent throws (before validating).
    assert_eq!(
        uncaught(&mut eval, "(restart-agent (agent 1) 2)"),
        "java.lang.RuntimeException | Agent does not need a restart"
    );
    assert_eq!(
        uncaught(&mut eval, "(restart-agent (agent 0 :validator even?) 3)"),
        "java.lang.RuntimeException | Agent does not need a restart"
    );
    // ... but options still parse first, and receivers check first.
    assert_eq!(
        uncaught(&mut eval, "(restart-agent (agent 1) 2 :clear-actions)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :clear-actions"
    );
    assert_eq!(
        uncaught(&mut eval, "(restart-agent (agent 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/restart-agent"
    );
    assert_eq!(
        uncaught(&mut eval, "(restart-agent nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.restart(Object, boolean)\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(restart-agent 5 1)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(agent-error)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/agent-error"
    );
    assert_eq!(
        uncaught(&mut eval, "(agent-error nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.getError()\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(agent-error 5)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(run_last(&mut eval, "(agent-error (agent 1))"), "nil");
}

#[test]
fn agent_await_rows() {
    let mut eval = boot();
    // `await` joins every agent (nil); `await-for` reports bool.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 10)] (send a inc) (send b inc) (await a b) [@a @b])"
        ),
        "[1 11]"
    );
    assert_eq!(run_last(&mut eval, "(await)"), "nil");
    assert_eq!(run_last(&mut eval, "(await-for 50)"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (send a inc) [(await-for 5000 a) @a])"
        ),
        "[true 1]"
    );
    // A gated action times out deterministically (the barrier cannot
    // run before the delivery), then completes after it.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] @p :never)) [(await-for 50 a) (do (deliver p :go) (await a) @a)])"
        ),
        "[false :never]"
    );
    // Negative timeouts wait nothing (the gated barrier cannot run).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] @p :never)) (await-for -5 a))"
        ),
        "false"
    );
    // ... and an idle agent times out too (the barrier loses the race).
    assert_eq!(run_last(&mut eval, "(await-for -5 (agent 0))"), "false");
    // Doubles truncate like `deref` timeouts.
    assert_eq!(run_last(&mut eval, "(await-for 5000.9 (agent 0))"), "true");
    // `await` refuses inside actions (a plain `Exception`, even with
    // no agents — and so does `await-for`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await a) (catch RuntimeException e :runtime) (catch Exception e :plain))) s)) @p)"
        ),
        ":plain"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await) (catch Exception e (ex-message e)))) s)) @p)"
        ),
        "\"Can't await in agent action\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await-for 50 a) (catch Exception e (ex-message e)))) s)) @p)"
        ),
        "\"Can't await in agent action\""
    );
    // Receivers validate before anything waits (agents before the
    // timeout converts — the `doseq` precedes the latch wait).
    assert_eq!(
        uncaught(&mut eval, "(await 1)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(await nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.dispatch(clojure.lang.IFn, clojure.lang.ISeq, java.util.concurrent.Executor)\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(await-for \"50\" 1)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    // ... then the timeout converts like `deref`.
    assert_eq!(
        uncaught(&mut eval, "(await-for \"50\" (agent 0))"),
        cce("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(await-for nil (agent 0))"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(await-for)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/await-for"
    );
    // A failing second agent still throws (the first barrier queued fine).
    run(&mut eval, "(def s6c-aw (agent 0))");
    run(&mut eval, "(def s6c-awf (agent 0))");
    run(
        &mut eval,
        "(send s6c-awf (fn [s] (throw (ex-info \"x\" {}))))",
    );
    wait_failed(&mut eval, "s6c-awf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-awf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        uncaught(&mut eval, "(await s6c-aw s6c-awf)"),
        "java.lang.RuntimeException | Agent is failed, needs restart"
    );
}

#[test]
fn agent_nested_rows() {
    let mut eval = boot();
    // A nested send runs after the enclosing action. The inner action
    // delivers `p2` after logging, so `@p2` proves `:inner` is in the
    // log no matter when the await barrier lands (a barrier sent on
    // `@p` may run before the outbox releases — on either runtime —
    // so awaiting alone cannot prove the nest ran).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) p (promise) p2 (promise) a (agent 0)] (send a (fn [s] (send a (fn [s2] (swap! log conj :inner) (deliver p2 :d) s2)) (swap! log conj :outer) (deliver p :sent) s)) @p @p2 (await a) @log)"
        ),
        "[:outer :inner]"
    );
    // Doubly nested sends order FIFO (no self-deadlock at any depth).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) p (promise) a (agent 0)] (send a (fn [s] (send a (fn [s2] (send a (fn [s3] (swap! log conj :in3) (deliver p :d) s3)) (swap! log conj :in2) s2)) (swap! log conj :out) s)) @p (await a) @log)"
        ),
        "[:out :in2 :in3]"
    );
    // Several nests from one action run in send order (the last nest
    // delivers `p2` after logging, for the same barrier race as above).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) p (promise) p2 (promise) a (agent 0)] (send a (fn [s] (send a (fn [x] (swap! log conj :n1) x)) (send a (fn [x] (swap! log conj :n2) (deliver p2 :d) x)) (swap! log conj :o) (deliver p :d) s)) @p @p2 (await a) @log)"
        ),
        "[:o :n1 :n2]"
    );
}

#[test]
fn agent_nested_discarded_on_failure() {
    let mut eval = boot();
    // A nested send from a failing action never runs — not even
    // after a restart without `:clear-actions` (the JVM discards
    // the unreleased `nested` buffer on failure). The landed latch
    // orders the restart (awaiting a racing latch could hang on the
    // stuck barrier instead of throwing).
    run(&mut eval, "(def s6c-nd-log (atom []))");
    run(&mut eval, "(def s6c-nd (agent 0))");
    run(
        &mut eval,
        "(send s6c-nd (fn [s] (send s6c-nd (fn [s2] (swap! s6c-nd-log conj :nested) s2)) (throw (ex-info \"boom\" {}))))",
    );
    wait_failed(&mut eval, "s6c-nd");
    assert_eq!(
        run_last(
            &mut eval,
            "(restart-agent s6c-nd 0) (await s6c-nd) [@s6c-nd @s6c-nd-log]"
        ),
        "[0 []]"
    );
    // ... same under `:continue` (the nest drops with the failure).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (agent 0 :error-mode :continue)] (send a (fn [s] (send a (fn [s2] (swap! log conj :nested) s2)) (throw (ex-info \"boom\" {})))) (await a) [@a @log])"
        ),
        "[0 []]"
    );
}

#[test]
fn agent_validator_rows() {
    let mut eval = boot();
    // A veto inside a `:continue` action keeps the state, clears the
    // error, and `await` returns nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0 :validator even? :error-mode :continue)] (send a (fn [s] 3)) (await a) [@a (agent-error a)])"
        ),
        "[0 nil]"
    );
    // ... while `:fail` parks with the veto latched.
    run(&mut eval, "(def s6c-va (agent 0 :validator even?))");
    run(&mut eval, "(send s6c-va (fn [s] 3))");
    wait_failed(&mut eval, "s6c-va");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-va) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[@s6c-va (ex-message (agent-error s6c-va))]"),
        "[0 \"Invalid reference state\"]"
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6c-va)"),
        "java.lang.IllegalStateException"
    );
    // A throwing validator fails the action with its own throw
    // (the ctor's initial value still passes).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0 :validator (fn [x] (if (zero? x) true (throw (ex-info \"vv\" {})))) :error-mode :continue)] (send a inc) (await a) [@a (agent-error a)])"
        ),
        "[0 nil]"
    );
}

#[test]
fn agent_watch_rows() {
    let mut eval = boot();
    // Watches fire `[key ref old new]` on the action's publish (the
    // first entry is deterministic: the action's fire completes
    // before the `await` barrier signals).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (agent 0)] (add-watch a :k (fn [k r o n] (swap! log conj [k (= r a) o n]))) (send a inc) (await a) (first @log))"
        ),
        "[:k true 0 1]"
    );
    // Unconditional fire (like atoms: `identity` still fires).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (agent 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (send a identity) (await a) (first @log))"
        ),
        "[1 1]"
    );
    // Removal stops the fire; both fns return the agent.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (agent 0)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (remove-watch a :k) (send a inc) (await a) @log)"
        ),
        "[]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1)] (= a (add-watch a :k (fn [k r o n]))))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1)] (add-watch a :k (fn [k r o n])) (= a (remove-watch a :k)))"
        ),
        "true"
    );
    // Nil fns install but never fire.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) a (agent 0)] (add-watch a :k nil) (add-watch a :j (fn [k r o n] (swap! log conj :j))) (send a inc) (await a) (first @log))"
        ),
        ":j"
    );
    // A throwing watch fails the action AFTER publishing: `:continue`
    // clears, `:fail` latches the published state.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1 :error-mode :continue)] (add-watch a :k (fn [k r o n] (throw (ex-info \"w\" {})))) (send a inc) (await a) [@a (agent-error a)])"
        ),
        "[2 nil]"
    );
    run(&mut eval, "(def s6c-wf (agent 1))");
    run(
        &mut eval,
        "(add-watch s6c-wf :k (fn [k r o n] (throw (ex-info \"w\" {}))))",
    );
    run(&mut eval, "(send s6c-wf inc)");
    // The failure must land before the rendezvous: a mid-wait
    // failure leaves the barrier stuck, hanging `await` by design.
    wait_failed(&mut eval, "s6c-wf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-wf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[@s6c-wf (nil? (agent-error s6c-wf))]"),
        "[2 false]"
    );
    // The barrier's own publish fires watches too (like the JVM's
    // `count-down`): shutdown no longer joins (T1192 FX1), so poll
    // for the full log instead of reading it once (a single read
    // would race the barrier's fire). Last: shutdown poisons this
    // evaluator.
    run(
        &mut eval,
        "(def shw-log (atom [])) (def shw-a (agent 5)) (add-watch shw-a :k (fn [k r o n] (swap! shw-log conj [o n]))) (send shw-a inc) (await shw-a) (shutdown-agents)",
    );
    let start = std::time::Instant::now();
    while run_last(&mut eval, "@shw-log") != "[[5 6] [6 6]]" {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "barrier fire never logged"
        );
    }
}

#[test]
fn agent_star_rows() {
    let mut eval = boot();
    // `*agent*` binds the agent inside actions (nil outside).
    assert_eq!(run_last(&mut eval, "(nil? *agent*)"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (= *agent* a)) s)) @p)"
        ),
        "true"
    );
    // A nested send observes its own agent.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0) b (agent 0)] (send a (fn [s] (send b (fn [t] (deliver p [(= *agent* b) (= *agent* a)]) t)) s)) @p)"
        ),
        "[true false]"
    );
    // Bindings convey from the sending thread (like futures).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic s6c-dx 1) (let [p (promise) a (agent 0)] (binding [s6c-dx 2] (send a (fn [s] (deliver p s6c-dx) s))) @p))"
        ),
        "2"
    );
    // ... and watches (like the error handler) still observe it
    // bound — only the validator runs bare (probed oracle truth).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0)] (add-watch a :k (fn [k r o n] (deliver p (nil? *agent*)))) (send a inc) @p)"
        ),
        "false"
    );
}

#[test]
fn agent_send_shapes() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(send 1 inc)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(send-off 1 inc)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(send-via nil 1 inc)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    let dispatch_npe = "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.dispatch(clojure.lang.IFn, clojure.lang.ISeq, java.util.concurrent.Executor)\" because \"a\" is null";
    assert_eq!(uncaught(&mut eval, "(send nil inc)"), dispatch_npe);
    assert_eq!(uncaught(&mut eval, "(send-off nil inc)"), dispatch_npe);
    assert_eq!(uncaught(&mut eval, "(send-via nil nil inc)"), dispatch_npe);
    assert_eq!(
        uncaught(&mut eval, "(send (agent 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/send"
    );
    assert_eq!(
        uncaught(&mut eval, "(send-off (agent 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/send-off"
    );
    assert_eq!(
        uncaught(&mut eval, "(send-via nil (agent 1))"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/send-via"
    );
    // The agent checkcast beats the executor checkcast (probed).
    assert_eq!(
        uncaught(&mut eval, "(send-via 42 1 inc)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    // Extra args apply (`(apply f old args*)`); a non-executor
    // `send-via` target is the JVM checkcast CCE (fence 3c).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (send a + 1 2 3) (await a) @a)"
        ),
        "6"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (send-off a + 1 2) (await a) @a)"
        ),
        "3"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (agent 0)] (send-via :exec a + 1 2 3 4))"
        ),
        cce("clojure.lang.Keyword", "java.util.concurrent.Executor")
    );
    assert_eq!(
        uncaught(&mut eval, "(let [a (agent 0)] (send-via 42 a + 1 2 3 4))"),
        cce("java.lang.Long", "java.util.concurrent.Executor")
    );
    // The fn is never validated eagerly: a bad fn fails inside the
    // action (the evaluator's call shapes, latched).
    run(&mut eval, "(def s6c-bf (agent 0))");
    run(&mut eval, "(send s6c-bf 5)");
    wait_failed(&mut eval, "s6c-bf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-bf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message (agent-error s6c-bf))"),
        "\"Cannot cast java.lang.Long to clojure.lang.IFn\""
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6c-bf)"),
        "java.lang.ClassCastException"
    );
    run(&mut eval, "(def s6c-nf (agent 0))");
    run(&mut eval, "(send s6c-nf nil)");
    wait_failed(&mut eval, "s6c-nf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-nf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message (agent-error s6c-nf))"),
        "\"Cannot invoke nil as a function\""
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6c-nf)"),
        "java.lang.NullPointerException"
    );
}

#[test]
fn agent_panic_rows() {
    let mut eval = boot();
    // A panicking native inside an action (or its validator) latches
    // an `Error` instead of killing the pool worker.
    eval.install_native(
        "user",
        "s6c-panic",
        std::sync::Arc::new(|_, _| panic!("s6c-boom")),
    );
    run(
        &mut eval,
        "(def s6c-pa (agent 0 :validator (fn [x] (if (zero? x) true (s6c-panic))) :error-mode :continue))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(send s6c-pa s6c-panic) (await s6c-pa) [@s6c-pa (agent-error s6c-pa)]"
        ),
        "[0 nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(send s6c-pa inc) (await s6c-pa) [@s6c-pa (agent-error s6c-pa)]"
        ),
        "[0 nil]"
    );
    // The latched panic is an `Error` carrying the payload.
    run(&mut eval, "(def s6c-pf (agent 0))");
    run(&mut eval, "(send s6c-pf s6c-panic)");
    wait_failed(&mut eval, "s6c-pf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-pf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message (agent-error s6c-pf))"),
        "\"s6c-boom\""
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6c-pf)"),
        "java.lang.Error"
    );
}

#[test]
fn agent_value_shapes() {
    let mut eval = boot();
    // `deref` reads agents (1-arity only — the timeout arity casts
    // to `Future` first, like atoms and delays).
    assert_eq!(run_last(&mut eval, "(deref (agent 5))"), "5");
    assert_eq!(run_last(&mut eval, "@(agent 5)"), "5");
    assert_eq!(
        uncaught(&mut eval, "(deref (agent 1) 50 :t)"),
        cce("clojure.lang.Agent", "java.util.concurrent.Future")
    );
    // Agents are not pending (but `force` passes them through).
    assert_eq!(
        uncaught(&mut eval, "(realized? (agent 1))"),
        cce("clojure.lang.Agent", "clojure.lang.IPending")
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (agent 1)] (= a (force a)))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(future? (agent 1))"), "false");
    // Agents carry `IReference` meta (never `IObj`).
    assert_eq!(
        run_last(&mut eval, "(alter-meta! (agent 1) assoc :a 1)"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(reset-meta! (agent 1) {:a 1})"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1)] (alter-meta! a assoc :a 1) (meta a))"
        ),
        "{:a 1}"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta (agent 1) {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.Agent cannot be cast to class clojure.lang.IObj (clojure.lang.Agent and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(vary-meta (agent 1) assoc :a 1)"),
        "java.lang.ClassCastException | class clojure.lang.Agent cannot be cast to class clojure.lang.IObj (clojure.lang.Agent and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    let with_meta = run_last(
        &mut eval,
        "(binding [*print-meta* true] (pr-str (agent 1 :meta {:a 1})))",
    );
    assert!(
        with_meta.starts_with("\"^{:a 1} #object[clojure.lang.Agent 0x"),
        "missing meta prefix: {with_meta}"
    );
    // `seq`/`count`/`nth` error shapes.
    assert_eq!(
        uncaught(&mut eval, "(seq (agent 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Agent"
    );
    assert_eq!(
        uncaught(&mut eval, "(first (agent 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Agent"
    );
    assert_eq!(
        uncaught(&mut eval, "(count (agent 1))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Agent"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (agent 1) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Agent"
    );
    // Predicates and collection casts.
    for (src, want) in [
        ("(coll? (agent 1))", "false"),
        ("(seq? (agent 1))", "false"),
        ("(counted? (agent 1))", "false"),
        ("(fn? (agent 1))", "false"),
        ("(ifn? (agent 1))", "false"),
        ("(map? (agent 1))", "false"),
        ("(vector? (agent 1))", "false"),
        ("(var? (agent 1))", "false"),
        ("(get (agent {:a 1}) :a)", "nil"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "for {src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(find (agent {}) :a)"),
        "java.lang.IllegalArgumentException | find not supported on type: clojure.lang.Agent"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj (agent 1) 2)"),
        cce("clojure.lang.Agent", "clojure.lang.IPersistentCollection")
    );
    assert_eq!(
        uncaught(&mut eval, "(compare (agent 1) 1)"),
        cce("clojure.lang.Agent", "java.lang.Comparable")
    );
    // Invoking an agent keeps the evaluator's cast shape (like atoms).
    assert_eq!(
        uncaught(&mut eval, "((agent 1))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Agent to clojure.lang.IFn"
    );
    assert_eq!(
        uncaught(&mut eval, "(deliver (agent 1) 2)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Agent to clojure.lang.IFn"
    );
}

#[test]
fn agent_stress() {
    // Eight agents race 250 `send` increments each (exact totals —
    // per-agent FIFO serializes, like the JVM), then again over
    // `send-off`.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6c-n 8) (def s6c-m 250) (def s6c-as (vec (map (fn [_] (agent 0)) (range s6c-n)))) (doseq [a s6c-as] (dotimes [_ s6c-m] (send a inc))) (doseq [a s6c-as] (await a)) (vec (map deref s6c-as)))"
        ),
        "[250 250 250 250 250 250 250 250]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6c-bs (vec (map (fn [_] (agent 0)) (range s6c-n)))) (doseq [a s6c-bs] (dotimes [_ s6c-m] (send-off a inc))) (doseq [a s6c-bs] (await a)) (vec (map deref s6c-bs)))"
        ),
        "[250 250 250 250 250 250 250 250]"
    );
}

#[test]
fn agent_shutdown_rows() {
    let mut eval = boot();
    // `shutdown-agents` is nil (idempotent, even with no pools yet).
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(shutdown-agents 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/shutdown-agents"
    );
    // Sends after shutdown return the agent but never run (like the
    // JVM's swallowed executor rejection); the agent keeps its state.
    run(&mut eval, "(def s6c-sa (agent 0))");
    assert_agent_object(&run_last(&mut eval, "(send s6c-sa inc)"));
    assert_agent_object(&run_last(&mut eval, "(send-off s6c-sa inc)"));
    assert_agent_object(&run_last(&mut eval, "(send-via nil s6c-sa inc)"));
    assert_eq!(run_last(&mut eval, "@s6c-sa"), "0");
    // ... and `await-for` times out false (its barriers never run).
    // Plain `await` would hang on both runtimes (unpinned parity).
    assert_eq!(run_last(&mut eval, "(await-for 100 s6c-sa)"), "false");
}

#[test]
fn agent_restart_resumed_after_shutdown() {
    let mut eval = boot();
    // A restart resumed after shutdown drops its head (the restart
    // itself succeeds and clears the error; the held queue never
    // runs again). Fresh evaluator: the failure setup must run
    // while the pools are alive.
    run(
        &mut eval,
        "(def s6c-rs2 (agent 0)) (def s6c-rs2-g (promise)) (send s6c-rs2 (fn [s] @s6c-rs2-g (throw (ex-info \"x\" {})))) (send s6c-rs2 inc) (deliver s6c-rs2-g :go)",
    );
    wait_failed(&mut eval, "s6c-rs2");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-rs2) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    assert_eq!(run_last(&mut eval, "(restart-agent s6c-rs2 5)"), "5");
    assert_eq!(
        run_last(&mut eval, "[@s6c-rs2 (agent-error s6c-rs2)]"),
        "[5 nil]"
    );
}

#[test]
fn agent_shutdown_async() {
    let mut eval = boot();
    // `shutdown-agents` returns at once (T1192 FX1): rendezvous
    // explicitly while healthy, then shut down and read.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) q (promise) a (agent 0) b (agent 0)] (send a (fn [s] @p 1)) (send-off b (fn [s] @q 2)) (deliver p :go) (deliver q :go) (await a) (await b) (shutdown-agents) [@a @b])"
        ),
        "[1 2]"
    );
    // Futures are unaffected by the shutdown itself: deref
    // rendezvouses, then the cell reads done (fresh evaluator —
    // the first half's shutdown poisoned its lineage).
    let mut eval2 = boot();
    assert_eq!(
        run_last(
            &mut eval2,
            "(let [p (promise) f (future @p :done)] (deliver p :go) (shutdown-agents) [(deref f 8000 :t) (future-done? f)])"
        ),
        "[:done true]"
    );
}

#[test]
fn agent_shutdown_returns_while_blocked() {
    let mut eval = boot();
    // The action starts (started-gate), then shutdown returns at
    // once while it stays blocked — no join (T1192 FX1).
    run(
        &mut eval,
        "(def sh-g (promise)) (def sh-a (agent 0)) (def sh-started (promise))",
    );
    run(
        &mut eval,
        "(send sh-a (fn [s] (deliver sh-started :yes) @sh-g 1))",
    );
    assert_eq!(run_last(&mut eval, "(deref sh-started 8000 :t)"), ":yes");
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    // Draining still completes: release the gate and the worker
    // publishes (heartbeat — post-shutdown `await` would hang on
    // its dropped barriers, like the JVM, so poll instead).
    run(&mut eval, "(deliver sh-g :go)");
    let start = std::time::Instant::now();
    while run_last(&mut eval, "@sh-a") != "1" {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "drain never published"
        );
    }
}

#[test]
fn future_rejected_after_shutdown() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    // Like the JVM's pool `execute` rejection (T1192 FX1); the
    // message is port-fixed — the JVM's embeds pool identity.
    for form in [
        "(future 1)",
        "(future-call (fn [] 1))",
        "(doall (pcalls (fn [] 1)))",
        "(doall (pvalues 1))",
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.util.concurrent.RejectedExecutionException | future rejected after shutdown",
            "{form}"
        );
    }
}

#[test]
fn agent_restart_after_shutdown() {
    let mut eval = boot();
    // A restart after shutdown drops the resumed head (the JVM's
    // rejected re-`execute`); the gate holds the failing action
    // until the follower queues (both sends land while healthy).
    run(
        &mut eval,
        "(def s6c-rs (agent 0)) (def s6c-rs-gate (promise)) (send s6c-rs (fn [s] @s6c-rs-gate (throw (ex-info \"x\" {})))) (send s6c-rs inc) (deliver s6c-rs-gate :go)",
    );
    wait_failed(&mut eval, "s6c-rs");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6c-rs) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(run_last(&mut eval, "(shutdown-agents)"), "nil");
    assert_eq!(run_last(&mut eval, "(restart-agent s6c-rs 10)"), "10");
    assert_eq!(run_last(&mut eval, "@s6c-rs"), "10");
}

#[test]
fn set_error_handler_rows() {
    let mut eval = boot();
    // The `void` setter returns nil — installing and clearing alike.
    run(&mut eval, "(def s6d-sh (agent 0 :error-mode :continue))");
    assert_eq!(
        run_last(&mut eval, "(set-error-handler! s6d-sh (fn [a e] :h))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(set-error-handler! s6d-sh nil)"),
        "nil"
    );
    // The installed handler fires on the NEXT failure (observed via
    // a promise; the queue proceeds under `:continue`).
    run(&mut eval, "(def s6d-hp (promise))");
    run(
        &mut eval,
        "(set-error-handler! s6d-sh (fn [ag ex] (deliver s6d-hp [(= ag s6d-sh) (ex-message ex)])))",
    );
    run(
        &mut eval,
        "(send s6d-sh (fn [s] (throw (ex-info \"sh\" {}))))",
    );
    run(&mut eval, "(send s6d-sh inc)");
    assert_eq!(
        run_last(
            &mut eval,
            "[@s6d-hp (await s6d-sh) @s6d-sh (agent-error s6d-sh)]"
        ),
        "[[true \"sh\"] nil 1 nil]"
    );
    // A cleared handler stays silent on later failures.
    run(&mut eval, "(set-error-handler! s6d-sh nil)");
    run(&mut eval, "(def s6d-hp2 (promise))");
    run(
        &mut eval,
        "(send s6d-sh (fn [s] (throw (ex-info \"sh2\" {}))))",
    );
    run(&mut eval, "(send s6d-sh inc)");
    assert_eq!(
        run_last(
            &mut eval,
            "[(await s6d-sh) @s6d-sh (agent-error s6d-sh) (realized? s6d-hp2)]"
        ),
        "[nil 2 nil false]"
    );
    // Shapes: nil-agent NPE, non-agent `Agent` cast, non-fn `IFn`
    // cast (the single overload compiles to a checkcast, P6).
    assert_eq!(
        uncaught(&mut eval, "(set-error-handler! nil (fn [a e]))"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.setErrorHandler(clojure.lang.IFn)\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-handler! 42 (fn [a e]))"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-handler! s6d-sh 42)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-handler! s6d-sh)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/set-error-handler!"
    );
}

#[test]
fn set_error_mode_rows() {
    let mut eval = boot();
    // The `void` setter returns nil; `:continue` clears the next failure.
    run(&mut eval, "(def s6d-sm (agent 0))");
    assert_eq!(
        run_last(&mut eval, "(set-error-mode! s6d-sm :continue)"),
        "nil"
    );
    run(
        &mut eval,
        "(send s6d-sm (fn [s] (throw (ex-info \"x\" {}))))",
    );
    run(&mut eval, "(send s6d-sm inc)");
    assert_eq!(
        run_last(&mut eval, "[(await s6d-sm) @s6d-sm (agent-error s6d-sm)]"),
        "[nil 1 nil]"
    );
    // `:fail` latches the next failure instead.
    run(&mut eval, "(set-error-mode! s6d-sm :fail)");
    run(
        &mut eval,
        "(send s6d-sm (fn [s] (throw (ex-info \"y\" {}))))",
    );
    wait_failed(&mut eval, "s6d-sm");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6d-sm) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "[(nil? (agent-error s6d-sm)) @s6d-sm]"),
        "[false 1]"
    );
    // A bogus mode behaves as `:fail` (like the ctor).
    run(&mut eval, "(restart-agent s6d-sm 1)");
    run(&mut eval, "(set-error-mode! s6d-sm :bogus)");
    run(
        &mut eval,
        "(send s6d-sm (fn [s] (throw (ex-info \"z\" {}))))",
    );
    wait_failed(&mut eval, "s6d-sm");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6d-sm) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    // ... as does nil (the checkcast passes it through, P6).
    run(&mut eval, "(restart-agent s6d-sm 1)");
    assert_eq!(run_last(&mut eval, "(set-error-mode! s6d-sm nil)"), "nil");
    run(
        &mut eval,
        "(send s6d-sm (fn [s] (throw (ex-info \"w\" {}))))",
    );
    wait_failed(&mut eval, "s6d-sm");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6d-sm) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    // Shapes: nil-agent NPE, non-agent `Agent` cast, non-keyword
    // `Keyword` cast.
    assert_eq!(
        uncaught(&mut eval, "(set-error-mode! nil :continue)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.setErrorMode(clojure.lang.Keyword)\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-mode! 42 :continue)"),
        cce("java.lang.Long", "clojure.lang.Agent")
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-mode! s6d-sm \"continue\")"),
        cce("java.lang.String", "clojure.lang.Keyword")
    );
    assert_eq!(
        uncaught(&mut eval, "(set-error-mode! s6d-sm)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/set-error-mode!"
    );
}

#[test]
fn locking_rows() {
    let mut eval = boot();
    // The body's value (implicit `do`; empty body is nil).
    assert_eq!(run_last(&mut eval, "(locking \"x\" 1 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "(locking \"x\")"), "nil");
    // The lockee evaluates once, before the body.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (atom 0)] (locking (do (swap! n inc) :l) :body) @n)"
        ),
        "1"
    );
    // Reentrant: nested `locking` on one thread proceeds.
    assert_eq!(
        run_last(&mut eval, "(locking :a (locking :b :inner))"),
        ":inner"
    );
    // A throwing body still releases: the next `locking` proceeds.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (locking :a (throw (ex-info \"t\" {}))) (catch Exception e :caught)) (locking :a :after)"
        ),
        ":after"
    );
    // A nil lockee is the synchronized-block NPE naming the expansion's
    // lock local (F8b: JVM-verbatim gensym; fresh boots fix the counter —
    // the shared eval's counter has moved on).
    assert_eq!(
        uncaught(&mut boot(), "(locking nil 1)"),
        "java.lang.NullPointerException | Cannot enter synchronized block because \"locklocal__2__auto__\" is null"
    );
    // A nil lockee nested in a `fn` body takes the analyzer path, which
    // names the fixed lock local (port artifact: the JVM macroexpands
    // first, so no such path exists there — F8b covers the expansion).
    assert_eq!(
        uncaught(&mut boot(), "((fn [] (locking nil 1)))"),
        "java.lang.NullPointerException | Cannot enter synchronized block because \"lock\" is null"
    );
    // Bare arity matches the macro's.
    assert_eq!(
        uncaught(&mut eval, "(locking)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/locking"
    );
    // `locking` expands (gensym ids depend on earlier evals).
    let expanded = run_last(&mut eval, "(macroexpand-1 '(locking x 1))");
    assert!(
        expanded.starts_with("(clojure.core/let [lockee__")
            && expanded.contains("monitor-enter")
            && expanded.contains("monitor-exit")
            && !expanded.contains("(locking"),
        "{expanded}"
    );
}

#[test]
fn locking_object_kinds() {
    // Fence 3b: per-object monitors accept every lockee kind
    // (cells, host instances, arrays included).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(locking (atom 1) :ok)"), ":ok");
    assert_eq!(run_last(&mut eval, "(locking (ref 1) :ok)"), ":ok");
    assert_eq!(run_last(&mut eval, "(locking (agent 1) :ok)"), ":ok");
    assert_eq!(
        run_last(&mut eval, "(locking (int-array [1 2]) :ok)"),
        ":ok"
    );
    assert_eq!(
        run_last(&mut eval, "(do (deftype LkT [x]) (locking (LkT. 1) :ok))"),
        ":ok"
    );
    // Boxed lockees (`find`/`first` entries): the exit arg is a
    // fresh clone — content keys keep enter/exit matched.
    assert_eq!(run_last(&mut eval, "(locking (find {:a 1} :a) :ok)"), ":ok");
    assert_eq!(run_last(&mut eval, "(locking (first {:a 1}) :ok)"), ":ok");
    // Fence 3c rebase: executors lock by minted id (monitor_key
    // arm 40, same shape as agents).
    assert_eq!(run_last(&mut eval, "(locking (executor) :ok)"), ":ok");
}

#[test]
fn locking_symbol_exit_matches_by_identity() {
    // Gap #114: symbol lockees key by minted id (JVM identity),
    // so a distinct-but-equal symbol fails the exit with IMS
    // instead of releasing; the identical binding still exits
    // cleanly (clones preserve the id).
    let mut eval = boot();
    run(&mut eval, "(in-ns 'clojure.core)");
    // The macro path stays balanced over a symbol lockee (enter
    // and exit read the same binding).
    assert_eq!(run_last(&mut eval, "(locking 'g114-sym :ok)"), ":ok");
    // The distinct exit IMSes, the held monitor stays held, and
    // the identical exit releases (no leaked guard).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s 'g114-sym] (fx4-monitor-enter s) [(try (fx4-monitor-exit 'g114-sym) :released (catch IllegalMonitorStateException e :ims)) (fx4-monitor-exit s)])"
        ),
        "[:ims nil]"
    );
}

#[test]
fn locking_mutual_exclusion() {
    let mut eval = boot();
    // Racing read-modify-writes under the monitor total exactly (a
    // lost update would park the sum below the trip count).
    run(&mut eval, "(def s6d-lc (atom 0))");
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (map deref (map (fn [_] (future (dotimes [_ 200] (locking s6d-lc (let [v @s6d-lc] (reset! s6d-lc (inc v))))))) (range 8)))) @s6d-lc"
        ),
        "1600"
    );
}

#[test]
fn pmap_rows() {
    let mut eval = boot();
    // Values in order (the port's lookahead is 6: sizes below, on,
    // and above the boundary all agree).
    for n in [0, 1, 5, 6, 7, 20, 100] {
        let got = run_last(&mut eval, &format!("(vec (pmap #(* 2 %) (range {n})))"));
        let want: Vec<String> = (0..n).map(|i| (2 * i).to_string()).collect();
        assert_eq!(
            got,
            format!("[{}]", want.join(" ")),
            "pmap over (range {n})"
        );
    }
    // Multi-colls stop at the shortest (like `map`).
    assert_eq!(
        run_last(&mut eval, "(vec (pmap + [1 2 3] [10 20]))"),
        "[11 22]"
    );
    assert_eq!(
        run_last(&mut eval, "(vec (pmap vector [1 2] [:a :b] [true false]))"),
        "[[1 :a true] [2 :b false]]"
    );
    // A lazy seq: unrealized until consumed ...
    run(
        &mut eval,
        "(def s6d-pm (pmap (fn [x] (throw (ex-info \"p\" {}))) [1 2 3]))",
    );
    assert_eq!(
        run_last(&mut eval, "[(seq? s6d-pm) (realized? s6d-pm)]"),
        "[true false]"
    );
    // ... and failures surface (wrapped, like futures) only on
    // consumption.
    assert_eq!(
        uncaught(&mut eval, "(doall s6d-pm)"),
        "java.util.concurrent.ExecutionException | clojure.lang.ExceptionInfo: p {}"
    );
    // Bodies see the spawning thread's bindings (`future-call`
    // conveys them).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def ^:dynamic s6d-px 1) (binding [s6d-px 2] (vec (pmap (fn [x] s6d-px) [1 2 3]))))"
        ),
        "[2 2 2]"
    );
}

#[test]
fn pcalls_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(vec (pcalls (fn [] 1) (fn [] 2) (fn [] 3)))"),
        "[1 2 3]"
    );
    assert_eq!(run_last(&mut eval, "(vec (pcalls))"), "[]");
    assert_eq!(run_last(&mut eval, "(seq? (pcalls (fn [] 1)))"), "true");
}

#[test]
fn pvalues_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(vec (pvalues 1 (+ 1 1) :a))"),
        "[1 2 :a]"
    );
    assert_eq!(run_last(&mut eval, "(vec (pvalues))"), "[]");
    // Every expr evaluates (all side effects land).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (atom 0)] (doall (pvalues (swap! n inc) (swap! n inc) (swap! n inc))) @n)"
        ),
        "3"
    );
}

#[test]
fn shutdown_agents_from_future_body() {
    let mut eval = boot();
    // `shutdown-agents` inside a future body returns at once
    // (draining is async — nothing joins, T1192 FX1).
    assert_eq!(
        run_last(
            &mut eval,
            "(deref (future (shutdown-agents) :done) 8000 :t)"
        ),
        ":done"
    );
}

#[test]
fn shutdown_agents_from_agent_action() {
    let mut eval = boot();
    // `shutdown-agents` inside an action returns at once
    // (async drain, T1192 FX1): the action completes, and the
    // post-shutdown barrier drops — so `await-for` waits out its
    // deadline to `false` (fence 3c: like the JVM's latch
    // timeout, never the old early `true`).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6-sha (agent 0)) (send s6-sha (fn [s] (shutdown-agents) (inc s))) [(await-for 300 s6-sha) @s6-sha])"
        ),
        "[false 1]"
    );
}

#[test]
fn agent_star_bound_over_watches_and_handler() {
    let mut eval = boot();
    // Action fns, watches, and the error handler all observe
    // `*agent*` bound; only the validator runs bare.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom nil) a (agent 0)] (add-watch a :w (fn [k r o n] (reset! seen (nil? *agent*)))) (send a inc) (await a) @seen)"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom nil) a (agent 0 :error-handler (fn [ag e] (reset! seen (nil? *agent*))))] (send a (fn [_] (throw (ex-info \"kb\" {})))) (await a) @seen)"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom nil) a (agent 0 :validator (fn [n] (reset! seen (nil? *agent*)) true))] (send a inc) (await a) @seen)"
        ),
        "true"
    );
}

#[test]
fn delay_cross_thread_force_shares_outcome() {
    let mut eval = boot();
    // Eight gated futures race one slow delay: all share the one
    // published outcome — no fellow thread reports
    // `IllegalStateException`.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6-dgate (promise)) (def s6-dd (delay (reduce + 0 (range 100000)))) (def s6-dfs (vec (map (fn [_] (future @s6-dgate (force s6-dd))) (range 8)))) (deliver s6-dgate :go) (let [vs (vec (map deref s6-dfs))] [(count vs) (apply = vs) (first vs)]))"
        ),
        "[8 true 4999950000]"
    );
}

#[test]
fn restart_agent_race_serializes() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (def s6-ra (agent 0)) (send s6-ra (fn [_] (throw (ex-info \"boom\" {})))))",
    );
    // The failure must land before the rendezvous (a mid-wait
    // failure would hang the `await` under load).
    wait_failed(&mut eval, "s6-ra");
    run(
        &mut eval,
        "(try (await s6-ra) (catch RuntimeException e :failed))",
    );
    // Two gated futures race `restart-agent` on the failed agent:
    // exactly one wins (returning its new state) while the other
    // reports "does not need a restart" — order-insensitive.
    let out = run_last(
        &mut eval,
        "(do (def s6-rgate (promise)) (def s6-r1 (future @s6-rgate (try (restart-agent s6-ra 1) (catch RuntimeException e :restart-err)))) (def s6-r2 (future @s6-rgate (try (restart-agent s6-ra 2) (catch RuntimeException e :restart-err)))) (deliver s6-rgate :go) [(deref s6-r1) (deref s6-r2)])",
    );
    let body = out
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .expect("a two-vector");
    let mut parts: Vec<&str> = body.split(' ').collect();
    parts.sort_unstable();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[1], ":restart-err");
    assert!(
        parts[0] == "1" || parts[0] == "2",
        "unexpected winner: {out}"
    );
    assert_eq!(run_last(&mut eval, "@s6-ra"), parts[0]);
}

#[test]
fn identity_meta_equality_rows() {
    let mut eval = boot();
    // Every identity type equals itself ...
    assert_eq!(run_last(&mut eval, "(let [a (atom 1)] (= a a))"), "true");
    assert_eq!(run_last(&mut eval, "(let [d (delay 1)] (= d d))"), "true");
    assert_eq!(run_last(&mut eval, "(let [f (future 1)] (= f f))"), "true");
    assert_eq!(run_last(&mut eval, "(let [p (promise)] (= p p))"), "true");
    assert_eq!(run_last(&mut eval, "(let [a (agent 1)] (= a a))"), "true");
    // ... but a `with-meta` copy shares the cell with different
    // meta, so it compares unequal (futures/promises carry `IObj`
    // meta; atoms/agents/delays reject `with-meta` outright).
    assert_eq!(
        run_last(&mut eval, "(let [f (future 1)] (= f (with-meta f {:a 1})))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(let [p (promise)] (= p (with-meta p {:a 1})))"),
        "false"
    );
    // Atoms/agents hold meta behind the shared cell (`reset-meta!`
    // mutates it in place), so same-cell handles stay equal ...
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1)] (reset-meta! a {:a 1}) (= a a))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1)] (reset-meta! a {:a 1}) (= a a))"
        ),
        "true"
    );
    // ... and distinct cells never compare equal.
    assert_eq!(run_last(&mut eval, "(= (atom 1) (atom 1))"), "false");
    assert_eq!(run_last(&mut eval, "(= (delay 1) (delay 1))"), "false");
    assert_eq!(run_last(&mut eval, "(= (future 1) (future 1))"), "false");
    assert_eq!(run_last(&mut eval, "(= (promise) (promise))"), "false");
    assert_eq!(run_last(&mut eval, "(= (agent 1) (agent 1))"), "false");
}

#[test]
fn agent_shutdown_rejects() {
    let mut eval = boot();
    run(&mut eval, "(def s6x-done (promise))");
    run(&mut eval, "(def s6x-seen2 (atom nil))");
    run(&mut eval, "(def s6x-seen3 (atom nil))");
    run(&mut eval, "(def s6x-a (agent 0))");
    run(
        &mut eval,
        "(def s6x-a2 (agent 0 :error-handler (fn [ag t] (reset! s6x-seen2 t))))",
    );
    run(
        &mut eval,
        "(def s6x-a3 (agent 0 :error-handler (fn [ag t] (reset! s6x-seen3 t))))",
    );
    run(
        &mut eval,
        "(send s6x-a (fn [s] (shutdown-agents) (send s6x-a2 inc) (send-off s6x-a3 inc) (deliver s6x-done :ok) 5))",
    );
    run(&mut eval, "(send s6x-a inc)");
    assert_eq!(run_last(&mut eval, "@s6x-done"), ":ok");
    // The publish lands after the deliver; poll (bounded) for it.
    // Once 5 is visible, the outbox release (both handlers) has run
    // too — the release precedes the publish.
    let start = std::time::Instant::now();
    while run_last(&mut eval, "@s6x-a") != "5" {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "action never published"
        );
    }
    // The action published; everything queued or nested after the
    // shutdown was rejected, never run.
    assert_eq!(run_last(&mut eval, "@s6x-a2"), "0");
    assert_eq!(run_last(&mut eval, "@s6x-a3"), "0");
    // Both nested heads reached the handlers as rejections.
    assert_eq!(
        run_last(&mut eval, "(ex-message @s6x-seen2)"),
        "\"agent action rejected after shutdown\""
    );
    assert_eq!(
        class_of(&mut eval, "@s6x-seen2"),
        "java.util.concurrent.RejectedExecutionException"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message @s6x-seen3)"),
        "\"agent action rejected after shutdown\""
    );
    assert_eq!(
        class_of(&mut eval, "@s6x-seen3"),
        "java.util.concurrent.RejectedExecutionException"
    );
}

#[test]
fn agent_pump_drops_queued_after_shutdown() {
    let mut eval = boot();
    // The worker blocks on the gate, so the second send surely
    // queues behind the running action (no timing assumption);
    // the shutdown then strands it in the pump, which drops it.
    run(&mut eval, "(def s6x-gate (promise))");
    run(&mut eval, "(def s6x-pd (agent 0))");
    run(
        &mut eval,
        "(send s6x-pd (fn [s] @s6x-gate (shutdown-agents) 5))",
    );
    run(&mut eval, "(send s6x-pd inc)");
    run(&mut eval, "(deliver s6x-gate :go)");
    // The release needs a heartbeat: the worker publishes after
    // the deliver lands.
    let start = std::time::Instant::now();
    while run_last(&mut eval, "@s6x-pd") != "5" {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "action never published"
        );
    }
}

#[test]
fn agent_panic_foreign_payload() {
    let mut eval = boot();
    eval.install_native(
        "user",
        "s6x-panic-any",
        std::sync::Arc::new(|_, _| std::panic::panic_any(42_i32)),
    );
    run(&mut eval, "(def s6x-pf (agent 0))");
    run(&mut eval, "(send s6x-pf s6x-panic-any)");
    wait_failed(&mut eval, "s6x-pf");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6x-pf) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message (agent-error s6x-pf))"),
        "\"agent action panicked\""
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6x-pf)"),
        "java.lang.Error"
    );
}

#[test]
fn agent_panic_owned_payload() {
    let mut eval = boot();
    eval.install_native(
        "user",
        "s6x-panic-owned",
        std::sync::Arc::new(|_, _| panic!("{}", "s6x-boom-owned")),
    );
    run(&mut eval, "(def s6x-po (agent 0))");
    run(&mut eval, "(send s6x-po s6x-panic-owned)");
    wait_failed(&mut eval, "s6x-po");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (await s6x-po) (catch RuntimeException e :failed))"
        ),
        ":failed"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message (agent-error s6x-po))"),
        "\"s6x-boom-owned\""
    );
    assert_eq!(
        class_of(&mut eval, "(agent-error s6x-po)"),
        "java.lang.Error"
    );
}

#[test]
fn future_panicking_native() {
    let mut eval = boot();
    eval.install_native(
        "user",
        "s6x-fpanic",
        std::sync::Arc::new(|_, _| panic!("s6x-boom")),
    );
    assert_eq!(
        uncaught(&mut eval, "(deref (future (s6x-fpanic)))"),
        "java.util.concurrent.ExecutionException | java.lang.Error: s6x-boom"
    );
}

#[test]
fn future_empty_message_cause() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(deref (future (assoc! (transient [1]) -1 :x)))"),
        "java.util.concurrent.ExecutionException | java.lang.IndexOutOfBoundsException"
    );
}

#[test]
fn pvalues_error_wraps_execution_exception() {
    // S9B: a failing expr surfaces on realization as an
    // ExecutionException wrapping the cause (oracle-verbatim).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(doall (pvalues 1 (/ 1 0) 3))"),
        "java.util.concurrent.ExecutionException | java.lang.ArithmeticException: Divide by zero"
    );
}

#[test]
fn print_dup_atom_throws_per_flag() {
    // CONVERGED (zero-oos S8 #51 — corpus `20-print-dup.clj`
    // `:dup-atom-true` meets the golden): `false` prints the
    // `#object` shape, `true` throws the JVM's
    // IllegalArgumentException (no `print-dup` method for Atom).
    let mut eval = boot();
    let plain = run_last(&mut eval, "(binding [*print-dup* false] (pr-str (atom 1)))");
    assert!(
        plain.starts_with("\"#object[clojure.lang.Atom 0x"),
        "no atom shape under false: {plain}"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*print-dup* true] (pr-str (atom 1)))"),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.lang.Atom\""
    );
}

#[test]
fn executor_value_semantics() {
    // Fence 3c: the `(executor)` kind — single-thread pools
    // `send-via` honors. Identity equality, cell print shapes,
    // no metadata.
    let mut eval = boot();
    let class = "java.util.concurrent.Executors$FinalizableDelegatedExecutorService";
    assert_eq!(class_of(&mut eval, "(executor)"), class);
    assert_object_shape(&run_last(&mut eval, "(executor)"), class);
    let rendered = run_last(&mut eval, "(str (executor))");
    assert!(
        rendered.starts_with(&format!("\"{class}@")),
        "bad executor str: {rendered}"
    );
    assert_eq!(run_last(&mut eval, "(let [e (executor)] (= e e))"), "true");
    assert_eq!(run_last(&mut eval, "(= (executor) (executor))"), "false");
    assert_eq!(
        run_last(&mut eval, "(let [e (executor)] (= (hash e) (hash e)))"),
        "true"
    );
    // `case` dispatches on `.hashCode` (covers the id row).
    assert_eq!(
        run_last(&mut eval, "(let [e (executor)] (case e 1 :one :other))"),
        ":other"
    );
    assert_eq!(run_last(&mut eval, "(meta (executor))"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(executor 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/executor"
    );
}

#[test]
fn send_via_custom_executor_survives_shutdown() {
    // Fence 3c: custom executors never shut down (like the JVM —
    // `shutdown-agents` touches the two pools only), so a
    // post-shutdown custom send runs while pool sends drop.
    let mut eval = boot();
    run(
        &mut eval,
        "(def svx (executor)) (def sva (agent 0)) (def svp (promise))",
    );
    run(&mut eval, "(shutdown-agents)");
    run(
        &mut eval,
        "(send-via svx sva (fn [s] (deliver svp :ran) 7))",
    );
    assert_eq!(run_last(&mut eval, "(deref svp 8000 :timeout)"), ":ran");
    assert_eq!(run_last(&mut eval, "@sva"), "7");
    // ... and a second send reuses the born-shut pools.
    run(&mut eval, "(def svp2 (promise))");
    run(
        &mut eval,
        "(send-via svx sva (fn [s] (deliver svp2 :ran2) 8))",
    );
    assert_eq!(run_last(&mut eval, "(deref svp2 8000 :timeout)"), ":ran2");
    assert_eq!(run_last(&mut eval, "@sva"), "8");
}

#[test]
fn send_via_nil_wedges_with_handler_npe() {
    // Fence 3c: a nil executor sends (probed) — the execute attempt
    // NPEs into the error handler and the head wedges at the queue
    // front, so follow-ups never run.
    let mut eval = boot();
    run(&mut eval, "(def nvp (promise)) (def nve (atom nil))");
    run(
        &mut eval,
        "(def nva (agent 0 :error-handler (fn [ag e] (reset! nve e) (deliver nvp :saw))))",
    );
    run(&mut eval, "(send-via nil nva (fn [s] :never))");
    assert_eq!(run_last(&mut eval, "(deref nvp 8000 :timeout)"), ":saw");
    assert_eq!(
        class_of(&mut eval, "@nve"),
        "java.lang.NullPointerException"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-message @nve)"),
        "\"Cannot invoke \\\"java.util.concurrent.Executor.execute(java.lang.Runnable)\\\" because \\\"this.exec\\\" is null\""
    );
    run(&mut eval, "(send nva (fn [s] :second))");
    assert_eq!(run_last(&mut eval, "(await-for 200 nva)"), "false");
    assert_eq!(run_last(&mut eval, "@nva"), "0");
    // Without a handler the wedge is silent but identical.
    run(&mut eval, "(def nvb (agent 0))");
    run(&mut eval, "(send-via nil nvb (fn [s] :never))");
    assert_eq!(run_last(&mut eval, "(await-for 200 nvb)"), "false");
}

#[test]
fn release_nested_nil_wedges_through_dispatch_with_pools_up() {
    // Fence 3c coverage: a nested send-via-nil released while the
    // pools are up dispatches through `dispatch_head`'s Nil arm
    // (fresh sends wedge without dispatching, so only this path
    // covers it).
    let mut eval = boot();
    run(&mut eval, "(def rqp (promise)) (def rqe (atom nil))");
    run(&mut eval, "(def rqa (agent 0))");
    run(
        &mut eval,
        "(def rqb (agent 0 :error-handler (fn [ag e] (reset! rqe e) (deliver rqp :saw))))",
    );
    run(
        &mut eval,
        "(send rqa (fn [s] (send-via nil rqb (fn [x] :never)) (release-pending-sends) s))",
    );
    assert_eq!(run_last(&mut eval, "(deref rqp 8000 :timeout)"), ":saw");
    assert_eq!(
        class_of(&mut eval, "@rqe"),
        "java.lang.NullPointerException"
    );
    assert_eq!(run_last(&mut eval, "(await-for 200 rqb)"), "false");
}

#[test]
fn release_pending_sends_post_shutdown_dispatches_own_exec() {
    // Fence 3c B2: an explicit `release-pending-sends` after
    // shutdown still runs each head on its OWN executor (like
    // upstream `releasePendingSends` → `execute`): custom heads
    // run, nil heads wedge — only pool heads reject. The outer
    // action is held in flight across the shutdown so the release
    // takes the pools-gone branch, and the nested sends target
    // IDLE agents (a nested send to the running agent would queue
    // for the completion handover instead of the release).
    let mut eval = boot();
    run(
        &mut eval,
        "(def rpx (executor)) (def rpa (agent 0)) (def rpb (agent 0)) (def rpe (agent 0)) (def rpg (promise)) (def rph (promise)) (def rpd (promise)) (def rpw2 (promise)) (def rpx2 (promise)) (def rph2 (promise)) (def rpc-e (atom nil)) (def rpc (agent 0 :error-handler (fn [ag e] (reset! rpc-e e) (deliver rpw2 :wedged))))",
    );
    run(
        &mut eval,
        "(send rpa (fn [s] (deliver rpg :started) (deref rph) (send-via rpx rpb (fn [x] (deliver rpd :done) (inc x))) (send-via nil rpc (fn [x] :never)) (send rpe (fn [x] (deliver rpx2 :ran) (inc x))) (send rpa (fn [s] (deliver rph2 :self) (inc s))) (release-pending-sends) s))",
    );
    assert_eq!(run_last(&mut eval, "(deref rpg 8000 :timeout)"), ":started");
    run(&mut eval, "(shutdown-agents)");
    run(&mut eval, "(deliver rph :go)");
    // The custom head ran on its executor despite the shutdown...
    assert_eq!(run_last(&mut eval, "(deref rpd 8000 :timeout)"), ":done");
    assert_eq!(run_last(&mut eval, "@rpb"), "1");
    // ... the pool head rejected (dropped, never runs)...
    assert_eq!(run_last(&mut eval, "(deref rpx2 200 :timeout)"), ":timeout");
    assert_eq!(run_last(&mut eval, "@rpe"), "0");
    // ... and the nil head wedged at `rpc`'s queue front (the NPE
    // reached the handler): a follow-up on the live executor never
    // runs (a dropped head would have let it through).
    assert_eq!(run_last(&mut eval, "(deref rpw2 8000 :timeout)"), ":wedged");
    // The wedge (not a drop) reached the handler as the execute NPE.
    assert_eq!(
        class_of(&mut eval, "@rpc-e"),
        "java.lang.NullPointerException"
    );
    run(
        &mut eval,
        "(def rpw (promise)) (send-via rpx rpc (fn [x] (deliver rpw :ran2) x))",
    );
    assert_eq!(run_last(&mut eval, "(deref rpw 200 :timeout)"), ":timeout");
    assert_eq!(run_last(&mut eval, "(await-for 200 rpc)"), "false");
    // The nested send to the RUNNING agent found no head at release
    // (it was still executing) and queued behind; post-shutdown the
    // handover rejects the pool head, so it never runs.
    assert_eq!(run_last(&mut eval, "(deref rph2 200 :timeout)"), ":timeout");
    assert_eq!(run_last(&mut eval, "@rpa"), "0");
}

#[test]
fn await_fail_during_hangs_until_restart() {
    // Fence 3c: an agent that fails after its barrier queued leaves
    // the barrier stuck, so `await` hangs (like the JVM) — a plain
    // restart resumes the barrier and the wait returns.
    // The future signals liveness immediately before the await and
    // the main thread rendezvous on it first: a late-starting
    // future thread (seen on loaded Windows runners) would
    // otherwise await an already-failed agent and throw instead
    // of parking (#128).
    let mut eval = boot();
    run(
        &mut eval,
        "(def hdg (promise)) (def hds (promise)) (def hda (agent 0))",
    );
    run(
        &mut eval,
        "(send hda (fn [s] @hdg (throw (ex-info \"x\" {}))))",
    );
    run(
        &mut eval,
        "(def hdf (future (deliver hds :started) (await hda) :waited))",
    );
    assert_eq!(run_last(&mut eval, "(deref hds 8000 :timeout)"), ":started");
    // CI-1 margin (see await_clear_actions_restart_hangs): the
    // rendezvous proves liveness, not parking — hold `:go` until
    // the future has parked in `await`.
    std::thread::sleep(std::time::Duration::from_millis(500));
    run(&mut eval, "(deliver hdg :go)");
    wait_failed(&mut eval, "hda");
    assert_eq!(run_last(&mut eval, "(realized? hdf)"), "false");
    assert_eq!(run_last(&mut eval, "(restart-agent hda 0)"), "0");
    assert_eq!(run_last(&mut eval, "(deref hdf 8000 :timeout)"), ":waited");
}

#[test]
fn await_clear_actions_restart_hangs() {
    // Fence 3c: a `:clear-actions` restart drops the barrier with
    // the queue, so the wait never returns (like the JVM). The
    // parked waiter is abandoned (its thread dies with the process).
    // The future signals liveness immediately before the await and
    // the main thread rendezvous on it first: a late-starting
    // future thread (seen on loaded Windows runners) would
    // otherwise await a post-restart clean agent and complete
    // instead of parking (CI-1).
    let mut eval = boot();
    run(
        &mut eval,
        "(def hcg (promise)) (def hcs (promise)) (def hca (agent 0))",
    );
    run(
        &mut eval,
        "(send hca (fn [s] @hcg (throw (ex-info \"x\" {}))))",
    );
    run(
        &mut eval,
        "(def hcf (future (deliver hcs :started) (await hca) :waited))",
    );
    assert_eq!(run_last(&mut eval, "(deref hcs 8000 :timeout)"), ":started");
    // CI-1 margin: the rendezvous proves liveness, not parking —
    // hold `:go` until the future has parked in `await` (the 500 ms
    // matches await_for_failure_waits_out_deadline's blessed
    // margin and dwarfs thread-scheduling jitter).
    std::thread::sleep(std::time::Duration::from_millis(500));
    run(&mut eval, "(deliver hcg :go)");
    wait_failed(&mut eval, "hca");
    assert_eq!(
        run_last(&mut eval, "(restart-agent hca 0 :clear-actions true)"),
        "0"
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(run_last(&mut eval, "(realized? hcf)"), "false");
}

#[test]
fn await_for_failure_waits_out_deadline() {
    // Fence 3c: `await-for` waits through a mid-wait failure to
    // `false` (like the JVM's latch timeout) instead of throwing.
    // The failure lands mid-wait by construction (a delayed
    // delivery — the barrier queues in microseconds, the failure
    // hundreds of milliseconds later, the deadline seconds after
    // that).
    let mut eval = boot();
    run(&mut eval, "(def fdg (promise)) (def fda (agent 0))");
    run(
        &mut eval,
        "(send fda (fn [s] @fdg (throw (ex-info \"x\" {}))))",
    );
    run(
        &mut eval,
        "(future (deref (promise) 500 :t) (deliver fdg :go))",
    );
    assert_eq!(run_last(&mut eval, "(await-for 2000 fda)"), "false");
    wait_failed(&mut eval, "fda");
}

#[test]
fn custom_handover_survives_shutdown() {
    // Fence 3c: a custom head queued behind a pool head still
    // dispatches when the pool head completes post-shutdown (its
    // executor never shut down).
    let mut eval = boot();
    run(
        &mut eval,
        "(def hog (promise)) (def hoe (executor)) (def hoa (agent 0)) (def hop (promise))",
    );
    run(&mut eval, "(send hoa (fn [s] @hog :pool-done))");
    run(
        &mut eval,
        "(send-via hoe hoa (fn [s] (deliver hop :custom-ran) :custom-done))",
    );
    run(&mut eval, "(shutdown-agents)");
    run(&mut eval, "(deliver hog :go)");
    assert_eq!(
        run_last(&mut eval, "(deref hop 8000 :timeout)"),
        ":custom-ran"
    );
    assert_eq!(run_last(&mut eval, "@hoa"), ":custom-done");
}

#[test]
fn restart_custom_resume_runs_post_shutdown() {
    // Fence 3c: a held custom head resumes onto its own executor
    // after a post-shutdown restart (like the JVM's re-`execute`).
    let mut eval = boot();
    run(
        &mut eval,
        "(def rse (executor)) (def rsa (agent 0)) (def rsp (promise)) (def rsg (promise))",
    );
    run(
        &mut eval,
        "(send-via rse rsa (fn [s] @rsg (throw (ex-info \"x\" {}))))",
    );
    run(
        &mut eval,
        "(send-via rse rsa (fn [s] (deliver rsp :resumed) 9))",
    );
    run(&mut eval, "(deliver rsg :go)");
    wait_failed(&mut eval, "rsa");
    run(&mut eval, "(shutdown-agents)");
    assert_eq!(run_last(&mut eval, "(restart-agent rsa 0)"), "0");
    assert_eq!(run_last(&mut eval, "(deref rsp 8000 :timeout)"), ":resumed");
    assert_eq!(run_last(&mut eval, "@rsa"), "9");
}

#[test]
fn restart_nil_head_rewedges() {
    // Fence 3c: a held nil head re-wedges on resume (the JVM
    // re-attempts the NPE'd `execute`): the handler fires again and
    // the queue stays stuck. Explicit `:fail` (a bare handler
    // defaults to `:continue`, like the JVM); the counting handler
    // pins both notifications without any timing.
    let mut eval = boot();
    run(&mut eval, "(def rnc (atom 0)) (def rng (promise))");
    run(
        &mut eval,
        "(def rna (agent 0 :error-handler (fn [ag e] (swap! rnc inc)) :error-mode :fail))",
    );
    run(
        &mut eval,
        "(send rna (fn [s] @rng (throw (ex-info \"x\" {}))))",
    );
    run(&mut eval, "(send-via nil rna (fn [s] :never))");
    run(&mut eval, "(deliver rng :go)");
    wait_failed(&mut eval, "rna");
    assert_eq!(run_last(&mut eval, "@rnc"), "1");
    assert_eq!(run_last(&mut eval, "(restart-agent rna 0)"), "0");
    assert_eq!(run_last(&mut eval, "@rnc"), "2");
    run(&mut eval, "(send rna (fn [s] :never2))");
    assert_eq!(run_last(&mut eval, "(await-for 200 rna)"), "false");
}

#[test]
fn await_post_shutdown_hangs() {
    // Fence 3c: post-shutdown barriers drop, so `await` never
    // returns (like the JVM — the old nil return was fenced). The
    // waiter spawns before the shutdown (a post-shutdown `future`
    // is rejected) and parks on a gate past it; the parked waiter
    // is abandoned (its thread dies with the process).
    let mut eval = boot();
    run(&mut eval, "(def psg (promise)) (def psa (agent 0))");
    run(&mut eval, "(def psf (future @psg (await psa) :waited))");
    run(&mut eval, "(shutdown-agents)");
    run(&mut eval, "(deliver psg :go)");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(run_last(&mut eval, "(realized? psf)"), "false");
}
