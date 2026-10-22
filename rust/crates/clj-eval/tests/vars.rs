// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5B var completeness: probe37 replay against the booted core
//! (roots, thread bindings, `with-redefs`/`with-bindings`/
//! `bound-fn`, metadata). `declare`/`ns` stay unresolved for S5C,
//! as do the three binding-form vars (the forms work; their vars
//! resolve as macros in S5C).

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

/// The `Class | message` of an uncaught throw (runtime failures
/// surface through `SourceError::Uncaught`).
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

/// The compile chain of a direct top-level analyzer failure
/// (`SourceError::Compile`, unwrapped — unlike `eval`'d forms).
fn compile_err(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src).unwrap_err() {
        SourceError::Compile(e) => e.chain(),
        other => panic!("expected a compile error, got {other}"),
    }
}

#[test]
fn alter_var_root() {
    let mut eval = boot();
    run(&mut eval, "(def av1 1)");
    // `f` sees the root plus the extra args; the result installs.
    assert_eq!(
        run_last(
            &mut eval,
            "(alter-var-root #'av1 (fn [x a b] [x a b]) 10 100)"
        ),
        "[1 10 100]"
    );
    assert_eq!(run_last(&mut eval, "av1"), "[1 10 100]");
    // Unbound roots reach `f` as the `Unbound` object, not a throw.
    run(&mut eval, "(def au1)");
    assert_eq!(
        run_last(&mut eval, "(alter-var-root #'au1 (fn [x] :was-unbound))"),
        ":was-unbound"
    );
    assert_eq!(run_last(&mut eval, "au1"), ":was-unbound");
    // A throwing `f` leaves the root untouched.
    run(&mut eval, "(def avf1 1)");
    uncaught(&mut eval, "(alter-var-root #'avf1 (fn [_] (throw \"x\")))");
    assert_eq!(run_last(&mut eval, "avf1"), "1");
    // `f` sees the ROOT under a thread binding (which still shadows).
    run(&mut eval, "(def ^:dynamic avd1 :root)");
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [avd1 :tl] [(var-get #'avd1) (alter-var-root #'avd1 (fn [cur] cur))])"
        ),
        "[:tl :root]"
    );
    assert_eq!(
        uncaught(&mut eval, "(alter-var-root 5 (fn [x] x))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
    assert_eq!(
        uncaught(&mut eval, "(alter-var-root #'av1 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(
        uncaught(&mut eval, "(alter-var-root #'av1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/alter-var-root"
    );
}

#[test]
fn bound_predicates() {
    let mut eval = boot();
    run(&mut eval, "(def p1 1) (def u1) (def ^:dynamic d1 :root)");
    assert_eq!(run_last(&mut eval, "(bound? #'p1)"), "true");
    assert_eq!(run_last(&mut eval, "(bound? #'u1)"), "false");
    assert_eq!(run_last(&mut eval, "(bound?)"), "true");
    assert_eq!(run_last(&mut eval, "(bound? #'p1 #'u1)"), "false");
    // A thread entry binds a rootless var.
    run(&mut eval, "(def ^:dynamic bu1)");
    assert_eq!(
        run_last(&mut eval, "(binding [bu1 :t] (bound? #'bu1))"),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(bound? 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
    assert_eq!(run_last(&mut eval, "(thread-bound? #'d1)"), "false");
    assert_eq!(
        run_last(&mut eval, "(binding [d1 :t] (thread-bound? #'d1))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(thread-bound?)"), "true");
    // `with-redefs` binds neither flavor.
    assert_eq!(
        run_last(&mut eval, "(with-redefs [p1 2] (thread-bound? #'p1))"),
        "false"
    );
    assert_eq!(
        uncaught(&mut eval, "(thread-bound? 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
}

#[test]
fn var_get_and_set() {
    let mut eval = boot();
    run(&mut eval, "(def p1 1) (def ^:dynamic vs1 :root-vs)");
    assert_eq!(run_last(&mut eval, "(var-get #'p1)"), "1");
    assert_eq!(
        run_last(&mut eval, "(binding [vs1 :t] (var-get #'vs1))"),
        ":t"
    );
    // Unbound reads the sentinel (never a throw, like `deref`).
    run(&mut eval, "(def vu1)");
    let got = run_last(&mut eval, "(var-get #'vu1)");
    assert!(
        got.starts_with("#object[clojure.lang.Var$Unbound 0x"),
        "{got}"
    );
    assert!(got.ends_with(" \"Unbound: #'user/vu1\"]"), "{got}");
    assert_eq!(
        uncaught(&mut eval, "(var-get 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
    // `var-set` writes the top frame and returns the value.
    assert_eq!(
        run_last(&mut eval, "(binding [vs1 :t] [(var-set #'vs1 :t2) vs1])"),
        "[:t2 :t2]"
    );
    assert_eq!(run_last(&mut eval, "vs1"), ":root-vs");
    // Without a frame: the `set!` shape, UNQUALIFIED name.
    assert_eq!(
        uncaught(&mut eval, "(var-set #'vs1 :x)"),
        "java.lang.IllegalStateException | Can't change/establish root binding of: vs1 with set"
    );
    assert_eq!(
        uncaught(&mut eval, "(var-set #'p1 2)"),
        "java.lang.IllegalStateException | Can't change/establish root binding of: p1 with set"
    );
    assert_eq!(
        uncaught(&mut eval, "(var-set 5 6)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
}

#[test]
fn push_pop_get_bindings() {
    let mut eval = boot();
    run(&mut eval, "(def ^:dynamic pb1 :root-pb) (def p1 1)");
    // Push/pop round-trip (state persists across `eval_source`).
    assert_eq!(
        run_last(&mut eval, "(push-thread-bindings {#'pb1 :p})"),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "pb1"), ":p");
    assert_eq!(run_last(&mut eval, "(pop-thread-bindings)"), "nil");
    assert_eq!(run_last(&mut eval, "pb1"), ":root-pb");
    // Non-dynamic is the qualified `IllegalStateException`.
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings {#'p1 2})"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: user/p1"
    );
    // Validation is atomic: a mixed map binds nothing.
    uncaught(&mut eval, "(push-thread-bindings {#'pb1 :p #'p1 2})");
    assert_eq!(run_last(&mut eval, "pb1"), ":root-pb");
    assert_eq!(run_last(&mut eval, "(thread-bound? #'pb1)"), "false");
    // Empty maps push an empty frame; nested pushes shadow.
    assert_eq!(run_last(&mut eval, "(push-thread-bindings {})"), "nil");
    assert_eq!(
        run_last(&mut eval, "(push-thread-bindings {#'pb1 :l1})"),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "pb1"), ":l1");
    assert_eq!(run_last(&mut eval, "(pop-thread-bindings)"), "nil");
    assert_eq!(run_last(&mut eval, "pb1"), ":root-pb");
    assert_eq!(run_last(&mut eval, "(pop-thread-bindings)"), "nil");
    assert_eq!(run_last(&mut eval, "pb1"), ":root-pb");
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings {5 6})"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Var"
    );
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Associative.seq()\" because \"bindings\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Associative"
    );
    // Vectors are `Associative`s of map entries: empty is a no-op,
    // non-empty fails the first element's `IMapEntry` cast.
    assert_eq!(run_last(&mut eval, "(push-thread-bindings [])"), "nil");
    assert_eq!(run_last(&mut eval, "(pop-thread-bindings)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings [#'pb1 :p])"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Var to clojure.lang.IMapEntry"
    );
    // The snapshot folds oldest-first (boot's `*ns*` base first).
    assert_eq!(
        run_last(&mut eval, "(get-thread-bindings)"),
        "{#'clojure.core/*ns* #object[clojure.lang.Namespace 0x2 \"user\"]}"
    );
    run_last(&mut eval, "(push-thread-bindings {#'pb1 :p})");
    assert_eq!(
        run_last(&mut eval, "(get-thread-bindings)"),
        "{#'clojure.core/*ns* #object[clojure.lang.Namespace 0x2 \"user\"], #'user/pb1 :p}"
    );
    run_last(&mut eval, "(pop-thread-bindings)");
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/push-thread-bindings"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop-thread-bindings 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/pop-thread-bindings"
    );
}

#[test]
fn pop_without_push() {
    // The boot `*ns*` base frame pops like the JVM's main-thread
    // frames; the empty stack is the `IllegalStateException`.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(pop-thread-bindings)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(pop-thread-bindings)"),
        "java.lang.IllegalStateException | Pop without matching push"
    );
}

#[test]
fn with_bindings_star() {
    let mut eval = boot();
    run(&mut eval, "(def ^:dynamic wb1 :root-wb) (def p1 1)");
    assert_eq!(
        run_last(&mut eval, "(with-bindings* {#'wb1 :w} (fn [] wb1))"),
        ":w"
    );
    assert_eq!(run_last(&mut eval, "wb1"), ":root-wb");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-bindings* {#'wb1 :w} (fn [a b] [a b wb1]) 1 2)"
        ),
        "[1 2 :w]"
    );
    // The frame pops in a `finally`.
    uncaught(
        &mut eval,
        "(with-bindings* {#'wb1 :w} (fn [] (throw \"x\")))",
    );
    assert_eq!(run_last(&mut eval, "(thread-bound? #'wb1)"), "false");
    assert_eq!(run_last(&mut eval, "wb1"), ":root-wb");
    // Validation runs before `f`: the inner `throw` never fires.
    assert_eq!(
        uncaught(&mut eval, "(with-bindings* {#'p1 1} (fn [] (throw \"x\")))"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: user/p1"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-bindings* {})"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/with-bindings*"
    );
}

#[test]
fn with_redefs() {
    let mut eval = boot();
    run(&mut eval, "(def wr1 1) (def wr2 2)");
    // A parallel root swap: inits see the old roots, roots restore.
    assert_eq!(
        run_last(&mut eval, "[(with-redefs [wr1 20] wr1) wr1]"),
        "[20 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(with-redefs [wr1 wr2 wr2 wr1] [wr1 wr2]) wr1 wr2]"
        ),
        "[[2 1] 1 2]"
    );
    // Restores in a `finally`, even on throw.
    uncaught(&mut eval, "(with-redefs [wr1 99] (throw \"x\"))");
    assert_eq!(run_last(&mut eval, "wr1"), "1");
    // Unbound vars redef and restore to unbound.
    run(&mut eval, "(def wu1)");
    assert_eq!(
        run_last(&mut eval, "[(with-redefs [wu1 9] wu1) (bound? #'wu1)]"),
        "[9 false]"
    );
    // An outer thread binding still shadows the swapped root.
    run(&mut eval, "(def ^:dynamic wrd1 :root)");
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [wrd1 :tl] (with-redefs [wrd1 :mock] wrd1))"
        ),
        ":tl"
    );
    // An odd trailing binding silently drops, like `zipmap`.
    assert_eq!(
        run_last(&mut eval, "[(with-redefs [wr1] :body) wr1]"),
        "[:body 1]"
    );
    // A repeated var keeps the last value (map semantics).
    assert_eq!(
        run_last(&mut eval, "[(with-redefs [wr1 1 wr1 2] wr1) wr1]"),
        "[2 1]"
    );
    // Empty bindings (and empty bodies) run fine.
    assert_eq!(run_last(&mut eval, "(with-redefs [] :body)"), ":body");
    assert_eq!(run_last(&mut eval, "(with-redefs [wr1 2])"), "nil");
    // List bindings work; targets resolve exactly like `var`.
    assert_eq!(run_last(&mut eval, "(with-redefs (wr1 2) wr1)"), "2");
    assert_eq!(
        compile_err(&mut eval, "(with-redefs [5 6] 1)"),
        vec![
            "Syntax error compiling var at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(with-redefs [nope1 1] 1)"),
        vec![
            "Syntax error compiling var at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Unable to resolve var: nope1 in this context".to_owned(),
        ]
    );
    assert_eq!(
        uncaught(&mut eval, "(with-redefs)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-redefs"
    );
    assert_eq!(
        compile_err(&mut eval, "(with-redefs 5 1)"),
        vec![
            "Syntax error macroexpanding with-redefs at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
}

#[test]
fn with_bindings() {
    let mut eval = boot();
    run(&mut eval, "(def ^:dynamic wb1 :root-wb)");
    assert_eq!(
        run_last(&mut eval, "[(with-bindings {#'wb1 :w} wb1) wb1]"),
        "[:w :root-wb]"
    );
    // The map is an evaluated expression (computed maps work).
    assert_eq!(
        run_last(&mut eval, "(let [m {#'wb1 :c}] (with-bindings m wb1))"),
        ":c"
    );
    assert_eq!(run_last(&mut eval, "(with-bindings {#'wb1 :w})"), "nil");
    uncaught(&mut eval, "(with-bindings {#'wb1 :w} (throw \"x\"))");
    assert_eq!(run_last(&mut eval, "(thread-bound? #'wb1)"), "false");
    assert_eq!(
        uncaught(&mut eval, "(with-bindings)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-bindings"
    );
}

#[test]
fn bound_fn() {
    let mut eval = boot();
    run(&mut eval, "(def ^:dynamic bf1 :root-bf)");
    // Conveyed on the creating thread, even outside the bindings.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (binding [bf1 :t] (bound-fn [] bf1))] (f))"
        ),
        ":t"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (binding [bf1 :t] (bound-fn [a b] [a b bf1]))] (f 1 2))"
        ),
        "[1 2 :t]"
    );
    // Inner `binding` still shadows the conveyed value.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [bf1 :t] (let [f (bound-fn [] (binding [bf1 :inner] bf1))] (f)))"
        ),
        ":inner"
    );
    // The frame pops around the call (no leak into the caller).
    assert_eq!(
        run_last(
            &mut eval,
            "[(let [f (binding [bf1 :t] (bound-fn [] bf1))] (f)) (thread-bound? #'bf1)]"
        ),
        "[:t false]"
    );
    // Named `bound-fn`: self-recursion plus conveyance.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (binding [bf1 :t] (bound-fn self [n] (if n (self nil) bf1)))] (f 1))"
        ),
        ":t"
    );
    // `with-meta` keeps the fn callable and conveyed.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (binding [bf1 :t] (bound-fn [] bf1))] [(meta (with-meta f {:a 1})) ((with-meta f {:a 1}))])"
        ),
        "[{:a 1} :t]"
    );
}

#[test]
fn shadowable_binding_forms() {
    // The three binding forms are macros: locals shadow them.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [with-redefs 1] with-redefs)"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(let [with-bindings 2] with-bindings)"),
        "2"
    );
    assert_eq!(run_last(&mut eval, "(let [bound-fn 3] bound-fn)"), "3");
}

#[test]
fn var_and_ns_meta() {
    let mut eval = boot();
    // Fresh interns carry `:ns`/`:name`.
    run_last(&mut eval, "(intern 'user 'mi1)");
    assert_eq!(
        run_last(&mut eval, "(meta #'mi1)"),
        "{:ns #object[clojure.lang.Namespace 0x2 \"user\"], :name mi1}"
    );
    // `def` adds the auto keys (source `user`, like the boot path).
    run(&mut eval, "(def mm1 1)");
    assert_eq!(
        run_last(&mut eval, "(meta #'mm1)"),
        "{:line 1, :column 1, :file \"user\", :name mm1, :ns #object[clojure.lang.Namespace 0x2 \"user\"]}"
    );
    // Boot vars stamp `:name`/`:ns` (`:dynamic`/`:private` as apt).
    assert_eq!(
        run_last(&mut eval, "(meta #'*ns*)"),
        "{:ns #object[clojure.lang.Namespace 0x1 \"clojure.core\"], :name *ns*, :dynamic true}"
    );
    assert_eq!(
        run_last(&mut eval, "(meta #'clojure.core/*loaded-libs*)"),
        "{:ns #object[clojure.lang.Namespace 0x1 \"clojure.core\"], :name *loaded-libs*, :dynamic true, :private true}"
    );
    // Fresh namespaces read nil.
    assert_eq!(run_last(&mut eval, "(meta (the-ns 'user))"), "nil");
}

#[test]
fn meta_reads() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(meta 's1)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta 's1 {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(meta [1])"), "nil");
    assert_eq!(run_last(&mut eval, "(meta {:a 1})"), "nil");
    assert_eq!(run_last(&mut eval, "(meta #{1})"), "nil");
    // Reader `:line`/`:column` rides lists only (the `(` is at 1:8).
    assert_eq!(run_last(&mut eval, "(meta '(1))"), "{:line 1, :column 8}");
    assert_eq!(run_last(&mut eval, "(meta 1)"), "nil");
    assert_eq!(run_last(&mut eval, "(meta \"s\")"), "nil");
    assert_eq!(run_last(&mut eval, "(meta nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(meta :k)"), "nil");
    assert_eq!(run_last(&mut eval, "(meta (fn [] 1))"), "nil");
    assert_eq!(run_last(&mut eval, "(meta *out*)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(meta 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/meta"
    );
}

#[test]
fn with_meta() {
    let mut eval = boot();
    run(&mut eval, "(def wm1 1)");
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta [1] {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (with-meta (with-meta [1] {:a 1 :b 2}) {:c 3}))"
        ),
        "{:c 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (with-meta [1] {:a 1}) nil))"),
        "nil"
    );
    // Values keep their shape and type (print ignores meta).
    assert_eq!(run_last(&mut eval, "(with-meta [1] {:a 1})"), "[1]");
    assert_eq!(run_last(&mut eval, "(with-meta 's1 {:a 1})"), "s1");
    assert_eq!(run_last(&mut eval, "(with-meta #{1} {:a 1})"), "#{1}");
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta #{1} {:a 1}))"),
        "{:a 1}"
    );
    // Seqs take metadata like the JVM's (T1192 FX1).
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (seq [1 2]) {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(with-meta (seq [1 2]) {:a 1})"),
        "(1 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (range 3) {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (with-meta (with-meta (seq [1]) {:a 1}) nil))"
        ),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (vary-meta (with-meta (seq [1]) {:a 1}) assoc :b 2))"
        ),
        "{:a 1, :b 2}"
    );
    // Functions stay callable under new meta.
    assert_eq!(
        run_last(&mut eval, "((with-meta (fn [x] x) {:a 1}) 5)"),
        "5"
    );
    // Natives keep meta and stay callable (pinned at the Rust
    // level in `vars.rs`: no source form reaches a native value
    // until `deref` lands in S5D).
    assert_eq!(
        uncaught(&mut eval, "(with-meta 1 {:a 1})"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta nil {:a 1})"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IObj.withMeta(clojure.lang.IPersistentMap)\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta #'wm1 {:a 1})"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Var to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta (the-ns 'user) {:a 1})"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Namespace to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta [1] 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentMap"
    );
}

#[test]
fn vary_meta() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (vary-meta (with-meta [1] {:a 1}) (fn [m] {:b 2})))"
        ),
        "{:b 2}"
    );
    // Nil meta reaches `f` as nil.
    assert_eq!(
        run_last(&mut eval, "(meta (vary-meta [1] (fn [m] m)))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (vary-meta [1] (fn [m a] {:k a}) :v))"),
        "{:k :v}"
    );
    // A non-map result is the `IPersistentMap` cast.
    assert_eq!(
        uncaught(&mut eval, "(vary-meta [1] (fn [m] 5))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(vary-meta 1 (fn [m] m))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
}

#[test]
fn alter_and_reset_meta() {
    let mut eval = boot();
    run(&mut eval, "(def am1 1) (def rm1 1)");
    assert_eq!(
        run_last(&mut eval, "(alter-meta! #'am1 (fn [m] {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(meta #'am1)"), "{:a 1}");
    assert_eq!(
        run_last(
            &mut eval,
            "(alter-meta! #'am1 (fn [m a b] {:k [a b]}) :x :y)"
        ),
        "{:k [:x :y]}"
    );
    // Namespaces are references too.
    assert_eq!(
        run_last(&mut eval, "(alter-meta! (the-ns 'user) (fn [m] {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(meta (the-ns 'user))"), "{:a 1}");
    // A throwing `f` changes nothing.
    uncaught(&mut eval, "(alter-meta! #'am1 (fn [_] (throw \"x\")))");
    assert_eq!(run_last(&mut eval, "(meta #'am1)"), "{:k [:x :y]}");
    assert_eq!(
        uncaught(&mut eval, "(alter-meta! [1] (fn [m] m))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.IReference"
    );
    // `reset-meta!` installs a map or nil and returns it.
    assert_eq!(run_last(&mut eval, "(reset-meta! #'rm1 {:a 1})"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(meta #'rm1)"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(reset-meta! #'rm1 nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(meta #'rm1)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(reset-meta! #'rm1 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-meta! [1] {})"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.IReference"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-meta! *out* nil)"),
        "java.lang.ClassCastException | Cannot cast java.io.OutputStreamWriter to clojure.lang.IReference"
    );
}

#[test]
fn with_meta_receiver_first() {
    // The object checks before the meta arg: a both-bad call
    // reports `IObj`, and a nil object with bad meta is still the
    // `withMeta` NPE.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(with-meta 1 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta nil 5)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IObj.withMeta(clojure.lang.IPersistentMap)\" because \"x\" is null"
    );
    // `vary-meta` shares the `with-meta` path, so the both-bad
    // order holds there too.
    assert_eq!(
        uncaught(&mut eval, "(vary-meta 1 (fn [m] 5))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
}
