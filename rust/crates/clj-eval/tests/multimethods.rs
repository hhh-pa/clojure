// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #33 stage-3 tests: every pin in
//! `docs/i33-typechain/probes/multimethods.out` reproduced against the
//! port, plus predicate edges, macro-shape errors, and the documented
//! divergences. Pins that need the JVM's `class` core fn assert the
//! port's mechanism instead (dispatch values evaluate, so the
//! class-name vars ride through); each such case says so.
//!
//! Oracle command (see the probe header):
//! `CP=.../clojure-1.13.0-alpha10.jar:... java -Duser.language=en
//! -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main
//! docs/i33-typechain/probes/multimethods.clj`.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw (macroexpansion-time
/// throws surface as `Compile`, like the oracle's uncaught text).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// The shared prelude (mirrors the probe header so pins read
/// literally).
fn prelude(eval: &mut Evaluator) {
    eval.eval_source(
        "(defmulti encounter (fn [x y] [(:species x) (:species y)]))
         (defmethod encounter [\"Bunny\" \"Lion\"] [x y] :run-away)
         (defmethod encounter [\"Lion\" \"Bunny\"] [x y] :eat)
         (defmethod encounter :default [x y] :ignore)
         (defmulti area :shape)
         (defmethod area :square [m] (* (:s m) (:s m)))
         (defmethod area :circle [m] (* 3 (:r m) (:r m)))",
    )
    .unwrap();
}

// ---- A: dispatch ----

#[test]
fn multi_dispatch_exact_and_default() {
    // Oracle `a-exact`, `a-flip`, `a-default` (compound dispatch
    // values match by value; misses fall to `:default`).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(encounter {:species \"Bunny\"} {:species \"Lion\"})"
        ),
        ":run-away"
    );
    assert_eq!(
        run(
            &mut eval,
            "(encounter {:species \"Lion\"} {:species \"Bunny\"})"
        ),
        ":eat"
    );
    assert_eq!(
        run(
            &mut eval,
            "(encounter {:species \"Bunny\"} {:species \"Bunny\"})"
        ),
        ":ignore"
    );
}

#[test]
fn multi_dispatch_keyword_fn() {
    // Oracle `a-kw`, `a-circle` (the dispatch fn is an ordinary
    // `IFn` — keywords included).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(area {:shape :square :s 3})"), "9");
    assert_eq!(run(&mut eval, "(area {:shape :circle :r 2})"), "12");
}

// ---- B: table views ----

#[test]
fn multi_methods_view() {
    // Oracle `b-methods` (verbatim — sorted key strings) and
    // `b-default-entry` (the default rides in the table).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (methods encounter))))"),
        "(\":default\" \"[\\\"Bunny\\\" \\\"Lion\\\"]\" \"[\\\"Lion\\\" \\\"Bunny\\\"]\")"
    );
    assert_eq!(
        run(&mut eval, "(contains? (methods encounter) :default)"),
        "true"
    );
}

#[test]
fn multi_get_method() {
    // Oracle `b-get` (the fetched fn runs on the call args) and
    // `b-get-miss` (a miss answers the default fn, proven by
    // calling through — the oracle's `#object` hex is JVM-live).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "((get-method encounter [\"Bunny\" \"Lion\"]) {:species \"Bunny\"} {:species \"Lion\"})"
        ),
        ":run-away"
    );
    assert_eq!(
        run(
            &mut eval,
            "((get-method encounter [\"Nope\" \"Nope\"]) {:species \"Bunny\"} {:species \"Bunny\"})"
        ),
        ":ignore"
    );
    // With no default entry either, `get-method` is nil (dispatch
    // still throws — see `multi_no_method`).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti strictm (fn [x] (:k x))) (defmethod strictm 1 [m] :one) (get-method strictm 999))"
        ),
        "nil"
    );
}

// ---- C: remove-method ----

#[test]
fn multi_remove_method() {
    // Oracle `c-remove` (verbatim error after removal) and
    // `c-remove-miss` (removing a missing entry is a no-op).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti tmpm :k) (defmethod tmpm 1 [m] :one) (remove-method tmpm 1) (tmpm {:k 1}))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'tmpm' for dispatch value: 1"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti tmpn :k) (defmethod tmpn 1 [m] :one) (remove-method tmpn 2) (tmpn {:k 1}))"
        ),
        ":one"
    );
    // Removal answers the multifn (callable through the return)
    // and leaves the other entries alone.
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti tmpo :k) (defmethod tmpo 1 [m] :one) (defmethod tmpo 2 [m] :two) ((remove-method tmpo 1) {:k 2}))"
        ),
        ":two"
    );
}

// ---- D: prefer-method ----

#[test]
fn multi_prefer_method() {
    // Oracle `d-prefer` (preferring answers the multifn — proven
    // by calling through the return, since the oracle's
    // `#object` hex is JVM-live) and `e-prefer-nonmulti`
    // (non-multifns fail the cast).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti tmpx :k) (defmethod tmpx :a [m] :a) (defmethod tmpx :b [m] :b) ((prefer-method tmpx :a :b) {:k :a}))"
        ),
        ":a"
    );
    assert_eq!(
        uncaught(&mut eval, "(prefer-method :notmulti :a :b)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.MultiFn (clojure.lang.Keyword and clojure.lang.MultiFn are in unnamed module of loader 'app')"
    );
    // A second edge on the same preferred value joins its set
    // (still recorded-only — dispatch is unaffected).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti p1 :k) (defmethod p1 :a [m] :a) (prefer-method p1 :a :b) ((prefer-method p1 :a :c) {:k :a}))"
        ),
        ":a"
    );
}

// ---- E: errors ----

#[test]
fn multi_no_method() {
    // Oracle `e-nomethod` (verbatim — the name quotes unqualified,
    // the value prints).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti strictm (fn [x] (:k x))) (defmethod strictm 1 [m] :one) (strictm {:k 2}))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'strictm' for dispatch value: 2"
    );
    // Keyword dispatch values print with their colon.
    assert_eq!(
        uncaught(&mut eval, "(do (defmulti kwm :k) (kwm {:k :nope}))"),
        "java.lang.IllegalArgumentException | No method in multimethod 'kwm' for dispatch value: :nope"
    );
}

#[test]
fn multi_defmethod_missing() {
    // Oracle `e-nomulti`: the JVM fails at compile time (the
    // `defmethod` macro resolves the var). The port's analyzer
    // likewise rejects the `(var ...)` form for a missing var —
    // sequentially-defined multifns still work, since `do`
    // elements analyze after the previous ones evaluate.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(defmethod nosuchmulti 1 [m] 1)"),
        "java.lang.RuntimeException | Unable to resolve var: nosuchmulti in this context"
    );
}

#[test]
fn multi_bad_dispatch() {
    // Oracle `e-bad-dispatch`: `defmulti` stores any dispatch
    // value — the failure comes at call time, invoking it.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(do (defmulti tmpd 5) (tmpd 1))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

// ---- F: class dispatch (oracle-only mechanics) ----

#[test]
fn multi_class_dispatch_analog() {
    // The JVM pins need the `class` core fn: dispatch values
    // evaluate, so the imported class rides through as the table
    // key (S16: no default imports — `String` resolves only after
    // an explicit `import`).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (defmulti foo (fn [s] (if (string? s) String :other))) (defmethod foo String [s] :string) (foo \"hi\"))"
        ),
        ":string"
    );
    // ...and `class` itself is in scope (S16 implements it):
    // dispatch on it matches the imported class by name.
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (defmulti foo2 class) (defmethod foo2 String [s] :str!) (foo2 \"hi\"))"
        ),
        ":str!"
    );
}

// ---- G: multi-arity methods ----

#[test]
fn multi_arity_methods() {
    // Oracle `g-zero`, `g-one`: both the dispatch fn and the
    // method fns keep their arities (methods run on the call
    // args, whatever their count).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti marity (fn ([x] (:k x)) ([x y] (:k x)))) (defmethod marity :a ([m] :zero) ([m y] [:one y])) (marity {:k :a}))"
        ),
        ":zero"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti marity (fn ([x] (:k x)) ([x y] (:k x)))) (defmethod marity :a ([m] :zero) ([m y] [:one y])) (marity {:k :a} 9))"
        ),
        "[:one 9]"
    );
}

// ---- H: var shape ----

#[test]
fn multi_var_shape() {
    // Oracle `h-map` (never a map) and `h-doc` (docstrings and
    // attr maps land on the var).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(map? encounter)"), "false");
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti diet \"eats things\" (fn [x] (:eats x))) (:doc (meta #'diet)))"
        ),
        "\"eats things\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti diet2 \"d\" {:a 1} (fn [x] 1)) (:a (meta #'diet2)))"
        ),
        "1"
    );
    // Attr entries evaluate (like the JVM — not quoted forms).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti qm {:a (+ 1 1)} :k) (:a (meta #'qm)))"
        ),
        "2"
    );
    // `defmulti` answers the var (like the JVM's `def` family).
    assert_eq!(run(&mut eval, "(var? (defmulti vm :k))"), "true");
    // Names are simple symbols.
    assert_eq!(
        uncaught(&mut eval, "(defmulti 5 :k)"),
        "java.lang.IllegalArgumentException | First argument to defmulti must be a symbol"
    );
    assert_eq!(
        uncaught(&mut eval, "(defmulti foo/bar :k)"),
        "java.lang.IllegalArgumentException | defmulti names must be simple symbols"
    );
}

#[test]
fn multi_defmethod_returns_multifn() {
    // Like the JVM, `defmethod` answers the multifn (callable
    // through the return).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti rm :k) ((defmethod rm :t [m] :t) {:k :t}))"
        ),
        ":t"
    );
}

// ---- options ----

#[test]
fn multi_custom_default() {
    // `:default` overrides the fallback value (evaluated, like
    // the JVM's options map).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti dm :k :default :none) (defmethod dm :none [m] :fell-back) (dm {:k :whatever}))"
        ),
        ":fell-back"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti dm :k :default :none) (defmethod dm :none [m] :fell-back) (contains? (methods dm) :none))"
        ),
        "true"
    );
}

#[test]
fn multi_options_rejected() {
    // `:default` and `:hierarchy` are supported (S13);
    // anything else fails; options come in pairs.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(defmulti hm (fn [x] x) :hierarchy (quote {}))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class clojure.lang.IRef (clojure.lang.PersistentArrayMap and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(defmulti om (fn [x] x) :bogus 1)"),
        "java.lang.IllegalArgumentException | Unsupported defmulti option: :bogus"
    );
    assert_eq!(
        uncaught(&mut eval, "(defmulti qm (fn [x] x) :default)"),
        "java.lang.IllegalArgumentException | defmulti options must be key/value pairs"
    );
    assert_eq!(
        uncaught(&mut eval, "(defmulti nm)"),
        "java.lang.IllegalArgumentException | defmulti requires a dispatch fn"
    );
}

// ---- guards ----

#[test]
fn multi_multifn_native_guards() {
    // The constructor takes a name string (only reachable by
    // calling the hidden native directly — `defmulti` always
    // passes one).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__multifn* 5 (fn [x] x) :default nil false)"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn multi_defmethod_guards() {
    // `defmethod` on a non-multifn var fails the cast, and
    // malformed tails fail at expansion.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(do (def n 5) (defmethod n 1 [m] 1))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    // The hidden native also takes a quoted symbol (resolved at
    // call time); resolving to a non-multifn (or nothing) is the
    // stock error.
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti sym1 :k) (clojure.core/__defmethod* 'sym1 :t (fn [m] :t)) (sym1 {:k :t}))"
        ),
        ":t"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def nnn 5) (clojure.core/__defmethod* 'nnn 1 (fn [m] 1)))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__defmethod* 'nosuchsyb 1 (fn [m] 1))"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: nosuchsyb in this context"
    );
    // A non-symbol target quotes through to the cast.
    assert_eq!(
        uncaught(&mut eval, "(defmethod :litm 1 [m] 1)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.MultiFn (clojure.lang.Keyword and clojure.lang.MultiFn are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (defmulti gm :k) (defmethod gm 1))"),
        "java.lang.IllegalArgumentException | defmethod requires a function tail"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti gm :k) (defmethod gm 1 ([m] 1) 5))"
        ),
        "java.lang.IllegalArgumentException | Invalid defmethod arity: 5"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti gm :k) (defmethod gm 1 ([m] 1) ((m) 1)))"
        ),
        "java.lang.IllegalArgumentException | Invalid defmethod arity: ((m) 1)"
    );
}

#[test]
fn multi_value_guards() {
    // The value natives reject non-multifns with the cast.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(get-method 5 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(methods 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-method :m 1)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.MultiFn (clojure.lang.Keyword and clojure.lang.MultiFn are in unnamed module of loader 'app')"
    );
}

#[test]
fn multi_native_arity_guards() {
    // Bare hidden-native calls fail the stock arity.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__multifn*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__multifn*"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__defmethod* (var area))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/__defmethod*"
    );
}

// ---- value semantics ----

#[test]
fn multi_value_semantics() {
    // Identity equality (like the JVM): the same var twice
    // holds, two separate multifns never do — even with equal
    // tables.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(= encounter encounter)"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti a1 :k) (defmulti a2 :k) (= a1 a2))"
        ),
        "false"
    );
    // Print and str take the JVM's `#object` / `toString` shapes
    // (stable digits here, derived from the name).
    assert_eq!(
        run(&mut eval, "(pr-str encounter)"),
        "\"#object[clojure.lang.MultiFn 0x4dba66eb \\\"clojure.lang.MultiFn@4dba66eb\\\"]\""
    );
    assert_eq!(
        run(&mut eval, "(str encounter)"),
        "\"clojure.lang.MultiFn@4dba66eb\""
    );
    // Equal roots hash equally (deterministic — pinned twice).
    assert_eq!(
        run(&mut eval, "(= (hash encounter) (hash encounter))"),
        "true"
    );
    // Multifns are `IFn`s (usable higher-order, like the JVM).
    assert_eq!(run(&mut eval, "(ifn? encounter)"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(vec (map area [{:shape :square :s 3} {:shape :circle :r 2}]))"
        ),
        "[9 12]"
    );
    // ... and usable as map keys / set members (name hash).
    assert_eq!(
        run(&mut eval, "(do (defmulti k1 :k) (get {k1 :v} k1))"),
        ":v"
    );
    assert_eq!(
        run(&mut eval, "(do (defmulti k1 :k) (contains? #{k1} k1))"),
        "true"
    );
    // (`.hashCode` is pinned by unit test — the only caller is
    // the private `hash-code` native.)
    // Never a collection: lookup misses and counts fail.
    assert_eq!(run(&mut eval, "(get encounter :k :nf)"), ":nf");
    assert_eq!(
        uncaught(&mut eval, "(count encounter)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: MultiFn"
    );
}
