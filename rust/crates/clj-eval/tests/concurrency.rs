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
    // (The `IObj` path keeps the S5B cast shape, without the JDK module
    // verbiage — pre-existing, like `IReference` below.)
    assert_eq!(
        uncaught(&mut eval, "(with-meta (atom 1) {:a 1})"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Atom to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(vary-meta (atom 1) assoc :a 1)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Atom to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta (delay 1) {:a 1})"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Delay to clojure.lang.IObj"
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
            && str_atom.len() > "\"clojure.lang.Atom@\"".len() + 1,
        "bad atom str: {str_atom}"
    );
    let str_delay = run_last(&mut eval, "(str (delay 1))");
    assert!(
        str_delay.starts_with("\"clojure.lang.Delay@")
            && str_delay.len() > "\"clojure.lang.Delay@\"".len() + 1,
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
