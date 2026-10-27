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
}

#[test]
fn validator_completion_reads_current() {
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
    assert_eq!(
        uncaught(&mut eval, "(release-pending-sends 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/release-pending-sends"
    );
}

#[test]
fn await1_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [a (agent 0)] (send a inc) (await1 a) @a)"),
        "1"
    );
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
