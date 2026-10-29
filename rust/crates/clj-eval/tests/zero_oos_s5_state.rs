// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5 state introspection: probe replay for `error-handler` /
//! `error-mode` / `future-cancelled?` / `get-validator` /
//! `set-validator!` / `reset-vals!` / `swap-vals!` / `disj!` /
//! `find-var` / `requiring-resolve` / `release-pending-sends` /
//! `await1` / `agent-errors` / `clear-agent-errors` /
//! `replicate` (see `docs/zero-oos-s5-state/probes/state.{clj,out}`).
//! Every label below replays its oracle line verbatim except the
//! future `IRef` casts (the port's reify class carries no numeric
//! suffix) and `identical?` reads (the port has no identity
//! predicate — behavior-equivalent calls stand in).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clj_eval::{Evaluator, SourceError, boot};
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
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                    Ok(_) => {}
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s5-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn lit(&self) -> String {
        let s = self.dir.to_string_lossy();
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}]] {src})",
            self.lit()
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn error_handler_rows() {
    // Oracle state.out: a-eh-none a-eh-fn a-eh-ident a-eh-after-set
    // a-em-default a-em-cont a-em-after-set a-eh-nil a-eh-bad a-em-nil
    // a-em-bad a-eh-arity0 a-eh-arity2 (the `(error-mode)` arity
    // assert is port-only — no `a-em-arity0` label).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(error-handler (agent 0))"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [h (fn [a e])] (fn? (error-handler (agent 0 :error-handler h))))"
        ),
        "true"
    );
    // Oracle `identical?` reads via a behavior-equivalent call.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [h (fn [a e] :called)] (= :called ((error-handler (agent 0 :error-handler h)) nil nil)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) h (fn [a e])] (set-error-handler! a h) (fn? (error-handler a)))"
        ),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(error-mode (agent 0))"), ":fail");
    assert_eq!(
        run_last(&mut eval, "(error-mode (agent 0 :error-mode :continue))"),
        ":continue"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (set-error-mode! a :continue) (error-mode a))"
        ),
        ":continue"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-handler nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.getErrorHandler()\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-handler 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Agent (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Agent is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-mode nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.getErrorMode()\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-mode :x)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.Agent (clojure.lang.Keyword and clojure.lang.Agent are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-handler)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/error-handler"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-handler (agent 0) 1)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/error-handler"
    );
    assert_eq!(
        uncaught(&mut eval, "(error-mode)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/error-mode"
    );
}

#[test]
fn future_cancelled_rows() {
    // Oracle state.out: b-false b-done b-true b-nil b-bad b-arity0
    // (`b-true` blocks on a promise, not `Thread/sleep`).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(future-cancelled? (future 1))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(let [f (future 1)] @f (future-cancelled? f))"),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (future @(promise) :never)] (future-cancel f) (future-cancelled? f))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancelled? nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.isCancelled()\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancelled? 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.concurrent.Future (java.lang.Long and java.util.concurrent.Future are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(future-cancelled?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/future-cancelled?"
    );
}

#[test]
fn validator_rows() {
    // Oracle state.out: c-get-none c-get-atom c-get-agent c-set-ret
    // c-set-readback c-set-nil-clears c-set-bad-current c-set-keeps-old
    // c-set-nonfn c-set-agent c-get-future c-get-delay c-get-volatile
    // c-get-nil c-get-bad c-set-future k-set-nil k-set-bad
    // k-set-nil-badvf k-set-bad-badvf c-set-arity1 c-get-arity0 (the
    // var asserts pin the Non-goal divergence — JVM oracles
    // `c-get-var`/`c-get-var-none`/`c-set-var-bad-root` accept vars).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(get-validator (atom 1))"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 2 :validator even?)] ((get-validator a) 4))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 1 :validator integer?)] ((get-validator a) 1))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(set-validator! (atom 2) even?)"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 2)] (set-validator! a even?) ((get-validator a) 4))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 2 :validator even?)] (set-validator! a nil) (get-validator a))"
        ),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [a (atom 1)] (set-validator! a even?))"),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    // A veto keeps the old validator (read back by behavior).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1 :validator integer?)] (try (set-validator! a even?) (catch IllegalStateException e :threw)) ((get-validator a) 5))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! (atom 1) 42)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 2)] (set-validator! a even?) ((get-validator a) 4))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(get-validator (future 1))"),
        "java.lang.ClassCastException | class clojure.core$future_call$reify cannot be cast to class clojure.lang.IRef (clojure.core$future_call$reify and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(get-validator (delay 1))"),
        "java.lang.ClassCastException | class clojure.lang.Delay cannot be cast to class clojure.lang.IRef (clojure.lang.Delay and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    // Oracle `c-get-volatile` (`state.out:27`): testable now that
    // S10's `volatile!` has landed.
    assert_eq!(
        uncaught(&mut eval, "(get-validator (volatile! 1))"),
        "java.lang.ClassCastException | class clojure.lang.Volatile cannot be cast to class clojure.lang.IRef (clojure.lang.Volatile and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(get-validator nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IRef.getValidator()\" because \"iref\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(get-validator 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IRef (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IRef is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! (future 1) even?)"),
        "java.lang.ClassCastException | class clojure.core$future_call$reify cannot be cast to class clojure.lang.IRef (clojure.core$future_call$reify and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! nil even?)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IRef.setValidator(clojure.lang.IFn)\" because \"iref\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! 1 even?)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IRef (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IRef is in unnamed module of loader 'app')"
    );
    // Checkcast order: the fn casts before the null receiver trips.
    assert_eq!(
        uncaught(&mut eval, "(set-validator! nil 42)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! 1 42)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IRef (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IRef is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(set-validator! (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/set-validator!"
    );
    assert_eq!(
        uncaught(&mut eval, "(get-validator)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/get-validator"
    );
    // L6: vars are rejected with the `IRef` cast (spec Non-goals —
    // the JVM accepts them: oracles `c-get-var`/`c-get-var-none`/
    // `c-set-var-bad-root`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def s5varx 2) (get-validator (var s5varx)))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class clojure.lang.IRef (clojure.lang.Var and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def s5vary 2) (set-validator! (var s5vary) even?))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class clojure.lang.IRef (clojure.lang.Var and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
}

#[test]
fn validator_completion_reads_current() {
    // Port-only: agent completion-timing pin, no oracle label.
    let mut eval = boot();
    // A validator installed after the send but before completion
    // governs the publish (JVM `getValidator`-at-completion).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) gate (promise) hold (promise)] (send a (fn [s] (deliver gate :started) (deref hold) 3)) (deref gate) (set-validator! a even?) (deliver hold :go) (try (await a) (catch RuntimeException e :failed)) [(some? (agent-error a)) @a])"
        ),
        "[true 0]"
    );
}

#[test]
fn vals_rows() {
    // Oracle state.out: d-reset d-reset-watch d-reset-veto d-reset-nil
    // d-reset-bad d-reset-arity1 d-swap d-swap0 d-swap3 d-swap-watch
    // d-swap-veto d-swap-nil d-swap-nil3 k-swap-nil4 k-swap-nil5
    // d-swap-bad d-swap-nilfn d-swap-arity1.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(reset-vals! (atom 1) 2)"), "[1 2]");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1) seen (atom [])] (add-watch a :w (fn [k r o n] (swap! seen conj [o n]))) [(reset-vals! a 2) @seen])"
        ),
        "[[1 2] [[1 2]]]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 2 :validator even?)] (reset-vals! a 3))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-vals! nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom2.resetVals(Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-vals! 1 2)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IAtom2 (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IAtom2 is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-vals! (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/reset-vals!"
    );
    assert_eq!(run_last(&mut eval, "(swap-vals! (atom 1) + 2)"), "[1 3]");
    assert_eq!(run_last(&mut eval, "(swap-vals! (atom 1) inc)"), "[1 2]");
    assert_eq!(
        run_last(&mut eval, "(swap-vals! (atom 1) + 1 2 3)"),
        "[1 7]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 1) seen (atom [])] (add-watch a :w (fn [k r o n] (swap! seen conj [o n]))) [(swap-vals! a + 5) @seen])"
        ),
        "[[1 6] [[1 6]]]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (atom 2 :validator even?)] (swap-vals! a inc))"
        ),
        "java.lang.IllegalStateException | Invalid reference state"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! nil inc)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom2.swapVals(clojure.lang.IFn)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! nil + 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom2.swapVals(clojure.lang.IFn, Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! nil + 1 2)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom2.swapVals(clojure.lang.IFn, Object, Object)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! nil + 1 2 3)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IAtom2.swapVals(clojure.lang.IFn, Object, Object, clojure.lang.ISeq)\" because \"atom\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! 1 inc)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IAtom2 (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IAtom2 is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! (atom 1) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(swap-vals! (atom 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/swap-vals!"
    );
}

#[test]
fn disj_bang_rows() {
    // Oracle state.out: e-basic e-multi e-missing e-arity1 e-nil
    // e-persistent e-map-t e-vec-t e-used e-arity0 (the `(disj! 42)`/
    // `(disj! nil)` 1-arity passthrough asserts are port-only).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(= #{1 3} (persistent! (disj! (transient #{1 2 3}) 2)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= #{} (persistent! (disj! (transient #{1 2 3}) 1 2 3)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= #{1} (persistent! (disj! (transient #{1}) 99)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= #{1} (persistent! (disj! (transient #{1}))))"),
        "true"
    );
    // The 1-arity is the identity over any value (JVM decl).
    assert_eq!(run_last(&mut eval, "(disj! 42)"), "42");
    assert_eq!(run_last(&mut eval, "(disj! nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(disj! nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientSet.disjoin(Object)\" because \"set\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(disj! #{1} 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentHashSet cannot be cast to class clojure.lang.ITransientSet (clojure.lang.PersistentHashSet and clojure.lang.ITransientSet are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(disj! (transient {1 2}) 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap$TransientArrayMap cannot be cast to class clojure.lang.ITransientSet (clojure.lang.PersistentArrayMap$TransientArrayMap and clojure.lang.ITransientSet are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(disj! (transient [1]) 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector$TransientVector cannot be cast to class clojure.lang.ITransientSet (clojure.lang.PersistentVector$TransientVector and clojure.lang.ITransientSet are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [t (transient #{1})] (persistent! t) (disj! t 1))"
        ),
        "java.lang.IllegalAccessError | Transient used after persistent! call"
    );
    assert_eq!(
        uncaught(&mut eval, "(disj!)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/disj!"
    );
}

#[test]
fn find_var_rows() {
    // Oracle state.out: f-found f-missing-var f-missing-ns
    // f-unqualified f-nil f-bad f-arity0 f-refer-blind.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(= (var clojure.core/map) (find-var 'clojure.core/map))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(find-var 'clojure.core/no-such-var-xyz)"),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-var 'no-such-ns-xyz/foo)"),
        "java.lang.IllegalArgumentException | No such namespace: no-such-ns-xyz"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-var 'map)"),
        "java.lang.IllegalArgumentException | Symbol must be namespace-qualified"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-var nil)"),
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"nsQualifiedSym\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-var \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Symbol (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-var)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/find-var"
    );
    // Refers are blind: `find-var` sees interns only.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (create-ns 'frefns) (binding [*ns* (find-ns 'frefns)] (refer 'clojure.core)) (find-var 'frefns/map))"
        ),
        "nil"
    );
}

#[test]
fn requiring_resolve_rows() {
    // Oracle state.out: g-loaded g-unqualified g-nil g-bad g-missing
    // g-arity0 g-arity2.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(= (var clojure.core/map) (requiring-resolve 'clojure.core/map))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve 'map)"),
        "java.lang.IllegalArgumentException | Not a qualified symbol: map"
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve nil)"),
        "java.lang.IllegalArgumentException | Not a qualified symbol: "
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve 42)"),
        "java.lang.IllegalArgumentException | Not a qualified symbol: 42"
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve 'no-such-ns-xyz-123/foo)"),
        "java.io.FileNotFoundException | Could not locate no_such_ns_xyz_123__init.class, no_such_ns_xyz_123.clj or no_such_ns_xyz_123.cljc on classpath. Please check that namespaces with dashes use underscores in the Clojure file name."
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/requiring-resolve"
    );
    assert_eq!(
        uncaught(&mut eval, "(requiring-resolve 'a/b 1)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/requiring-resolve"
    );
}

#[test]
fn requiring_resolve_loads() {
    // Port-only: fixture-library load pin, no oracle label.
    let fx = Fixture::new("rrlib");
    fx.write(
        "s5rrlib.clj",
        "(clojure.core/in-ns 's5rrlib)\n(def answer 42)\n",
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within(
                "[(some? (requiring-resolve 's5rrlib/answer)) @(requiring-resolve 's5rrlib/answer)]"
            )
        ),
        "[true 42]"
    );
    assert_eq!(run_last(&mut eval, "s5rrlib/answer"), "42");
}

#[test]
fn release_pending_sends_rows() {
    // Oracle state.out: h-outside h-inside h-ctrl h-arity1 (the
    // empty-inside and post-shutdown asserts are port-only).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(release-pending-sends)"), "0");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 10) r (promise)] (send a (fn [s] (send b inc) (deliver r (release-pending-sends)) s)) (await a) (await b) [@r @b])"
        ),
        "[1 11]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 10)] (send a (fn [s] (send b inc) s)) (await a) (await b) @b)"
        ),
        "11"
    );
    // Inside an action with nothing buffered: 0, outbox stays armed.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 10) r (promise)] (send a (fn [s] (deliver r [(release-pending-sends) (do (send b inc) (release-pending-sends))]) s)) (await a) (await b) [(deref r) @b])"
        ),
        "[[0 1] 11]"
    );
    // Post-shutdown the count still answers; the head drops.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 10) gate (promise) hold (promise) r (promise)] (send a (fn [s] (send b inc) (deliver gate :ready) (deref hold) (deliver r (release-pending-sends)) s)) (deref gate) (shutdown-agents) (deliver hold :go) [(deref r) @b])"
        ),
        "[1 10]"
    );
    assert_eq!(
        uncaught(&mut eval, "(release-pending-sends 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/release-pending-sends"
    );
}

#[test]
fn await1_rows() {
    // Oracle state.out: i-basic i-returns-agent i-nil i-bad i-arity0
    // i-private k-await1-idle-in-action k-await1-self-idle
    // k-await1-busy-in-action (`i-static` unpinned per Non-goals;
    // `k-await1-busy-in-action` blocks on gate/hold promises, not
    // `Thread/sleep`).
    // `i-basic`: the gate/hold form pins the observable
    // (await1 waits, @a answers 1). Its outcome is stable, but
    // its *coverage* of the `await1` blocking path races pool
    // scheduling (a stalled main thread can find the queue
    // drained — observed as a floor flake); the white-box pin
    // below owns that path deterministically.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 0) gate (promise) hold (promise)] (send a (fn [s] (deliver gate :started) (deref hold) s)) (send a inc) (deref gate) (send b (fn [s] (deliver hold :go) s)) (await1 a) @a)"
        ),
        "1"
    );
    // Deterministic `await1` blocking-path pin (white-box): a
    // releaser thread delivers `hold` only after observing the
    // barrier's queue-count bump, which `send_barriers`
    // performs strictly after `await1`'s nonempty check — so
    // the check provably sees a nonempty queue in every
    // interleaving (no sleeps, no margins; the 30s backstop
    // only converts a hang into a loud failure).
    {
        run_last(
            &mut eval,
            "(do (def wb-a (agent 0)) (def wb-gate (promise)) (def wb-hold (promise)) nil)",
        );
        let agent = match eval.eval_source("wb-a").unwrap().pop().unwrap() {
            Value::Agent(a) => a,
            other => panic!("wb-a not an agent: {other:?}"),
        };
        let hold = match eval.eval_source("wb-hold").unwrap().pop().unwrap() {
            Value::Promise(p) => p,
            other => panic!("wb-hold not a promise: {other:?}"),
        };
        run_last(
            &mut eval,
            "(do (send wb-a (fn [s] (deliver wb-gate :started) (deref wb-hold) s)) (send wb-a inc) nil)",
        );
        eval.eval_source("(deref wb-gate)").unwrap();
        // Frozen: the head runs on `hold`, `inc` queues behind.
        assert_eq!(agent.queue_count(), 2);
        let releaser = std::thread::spawn({
            let agent = agent.clone();
            let hold = hold.clone();
            move || {
                let start = Instant::now();
                loop {
                    if agent.queue_count() > 2 {
                        assert!(hold.deliver(Value::Nil), "hold delivered once");
                        return;
                    }
                    if start.elapsed() > Duration::from_secs(30) {
                        panic!("await1 never enqueued its barrier");
                    }
                    std::thread::yield_now();
                }
            }
        });
        assert_eq!(run_last(&mut eval, "(= wb-a (await1 wb-a))"), "true");
        releaser.join().expect("releaser finished");
        assert_eq!(run_last(&mut eval, "@wb-a"), "1");
    }
    assert_eq!(
        run_last(&mut eval, "(let [a (agent 0)] (= a (await1 a)))"),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(await1 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Agent.getQueueCount()\" because \"a\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(await1 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Agent (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Agent is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(await1)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/await1"
    );
    // L6; oracle `i-private` (`state.out:91`).
    assert_eq!(run_last(&mut eval, "(:private (meta (var await1)))"), "nil");
    // An idle agent skips the await — and its in-action guard.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) b (agent 5) r (promise)] (send a (fn [s] (deliver r (= b (await1 b))) s)) (await a) (deref r))"
        ),
        "true"
    );
    // The running head counts: awaiting self inside an action throws.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) r (promise)] (send a (fn [s] (deliver r (try (await1 a) :no-throw (catch Throwable t :threw))) s)) (await a) (deref r))"
        ),
        ":threw"
    );
    // Queued work behind the running head likewise awaits (and throws).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) r (promise) gate (promise) hold (promise)] (send a (fn [s] (deliver gate :started) (deref hold) (deliver r (try (await1 a) :no-throw (catch Throwable t :threw))) s)) (send a inc) (deref gate) (deliver hold :go) (deref r))"
        ),
        ":threw"
    );
}

#[test]
fn deprecated_trio_rows() {
    // Oracle state.out: j-errors-none j-errors-fail j-errors-equiv
    // j-clear j-clear-unfailed j-clear-equiv j-replicate j-replicate0
    // j-replicate-equiv j-meta-errors j-meta-clear j-meta-replicate
    // (`j-errors-fail` uses `ex-info`/`ex-message` for the probe's
    // `Exception.`/`.getMessage`).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(agent-errors (agent 0))"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (send (agent 0) (fn [_] (throw (ex-info \"boom\" {}))))] (try (await a) (catch RuntimeException e :failed)) (map ex-message (agent-errors a)))"
        ),
        "(\"boom\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (send (agent 0) (fn [_] (throw (ex-info \"boom\" {}))))] (try (await a) (catch RuntimeException e :failed)) (= (agent-errors a) (list (agent-error a))))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (send (agent 7) (fn [_] (throw (ex-info \"boom\" {}))))] (try (await a) (catch RuntimeException e :failed)) (clear-agent-errors a) [(agent-error a) @a])"
        ),
        "[nil 7]"
    );
    assert_eq!(
        uncaught(&mut eval, "(clear-agent-errors (agent 0))"),
        "java.lang.RuntimeException | Agent does not need a restart"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (send (agent 7) (fn [_] (throw (ex-info \"boom\" {})))) b (send (agent 7) (fn [_] (throw (ex-info \"boom\" {}))))] (try (await a) (catch RuntimeException e :failed)) (try (await b) (catch RuntimeException e :failed)) [(clear-agent-errors a) (restart-agent b @b)])"
        ),
        "[7 7]"
    );
    assert_eq!(run_last(&mut eval, "(replicate 3 :x)"), "(:x :x :x)");
    assert_eq!(run_last(&mut eval, "(replicate 0 :x)"), "()");
    assert_eq!(
        run_last(&mut eval, "(= (replicate 3 :x) (take 3 (repeat :x)))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(= {:added \"1.0\" :deprecated \"1.2\"} (select-keys (meta (var agent-errors)) [:added :deprecated])) (:arglists (meta (var agent-errors)))]"
        ),
        "[true ([a])]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(= {:added \"1.0\" :deprecated \"1.2\"} (select-keys (meta (var clear-agent-errors)) [:added :deprecated])) (:arglists (meta (var clear-agent-errors)))]"
        ),
        "[true ([a])]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(= {:added \"1.0\" :deprecated \"1.3\"} (select-keys (meta (var replicate)) [:added :deprecated])) (:arglists (meta (var replicate)))]"
        ),
        "[true ([n x])]"
    );
}
