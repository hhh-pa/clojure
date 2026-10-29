// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #33 stage-2 tests: every pin in
//! `docs/i33-typechain/probes/protocols.out` reproduced against the
//! port, plus predicate edges, macro-shape errors, the `Closeable`
//! contract, and the documented divergences. Pins that need the
//! JVM's fn class names assert the port's shape instead; each such
//! case says so.
//!
//! Oracle command (see the probe header):
//! `CP=.../clojure-1.13.0-alpha10.jar:... java -Duser.language=en
//! -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main
//! docs/i33-typechain/probes/protocols.clj`.

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

/// The shared prelude: two protocols plus stage-1 types (mirrors the
/// probe header so pins read literally).
fn prelude(eval: &mut Evaluator) {
    eval.eval_source(
        "(defprotocol Greet \"greets things\" (greet [this]) (greet2 [this x]))
         (defprotocol Adder2 (add2 [this x y]))
         (deftype PD [a])
         (defrecord PR [x])",
    )
    .unwrap();
}

// ---- A: defprotocol shape ----

#[test]
fn proto_is_map() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(map? Greet)"), "true");
}

#[test]
fn proto_sigs() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (:sigs Greet))))"),
        "(\":greet\" \":greet2\")"
    );
}

#[test]
fn proto_on() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(str (:on Greet))"), "\"user.Greet\"");
}

#[test]
fn proto_fn_no_doc() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(:doc (meta #'greet))"), "nil");
}

#[test]
fn proto_doc() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(:doc (meta #'Greet))"), "\"greets things\"");
}

// Oracle `a-fnclass` needs `class` (still out of scope with
// `supers` / `instance?`): untestable until the type-reflection
// slice lands.

// ---- B: extend-type over stage-1 types ----

#[test]
fn extend_deftype() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PD Greet (greet [this] (str \"pd:\" (.-a this))) (greet2 [this x] [x])) [(greet (PD. 1)) (greet2 (PD. 1) 2)])"
        ),
        "[\"pd:1\" [2]]"
    );
}

#[test]
fn extend_record() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PR Greet (greet [this] (str \"pr:\" (:x this)))) (greet (PR. 5)))"
        ),
        "\"pr:5\""
    );
}

#[test]
fn satisfies_deftype() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PD Greet (greet [this] 1)) (satisfies? Greet (PD. 1)))"
        ),
        "true"
    );
}

#[test]
fn satisfies_record() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PR Greet (greet [this] 1)) (satisfies? Greet (PR. 1)))"
        ),
        "true"
    );
}

#[test]
fn satisfies_no() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(satisfies? Greet 5)"), "false");
}

#[test]
fn satisfies_nil() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(run(&mut eval, "(satisfies? Greet nil)"), "false");
}

#[test]
fn extends_deftype() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PD Greet (greet [this] 1)) (extends? Greet PD))"
        ),
        "true"
    );
}

#[test]
fn extends_no() {
    let mut eval = boot();
    prelude(&mut eval);
    // S16: no default imports — `String` resolves only after an
    // explicit `import` (as a class value).
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (extends? Greet String))"
        ),
        "false"
    );
}

// ---- C: extend-protocol over host types ----

#[test]
fn extend_string() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (extend-protocol Greet String (greet [this] (str \"s:\" this)) (greet2 [this x] x)) [(greet \"hi\") (greet2 \"hi\" 9)])"
        ),
        "[\"s:hi\" 9]"
    );
}

#[test]
fn extend_nil() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-protocol Greet nil (greet [this] \"nil!\")) (greet nil))"
        ),
        "\"nil!\""
    );
}

#[test]
fn extend_long() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Long) (extend-protocol Adder2 Long (add2 [this x y] (+ this x y))) (add2 1 2 3))"
        ),
        "6"
    );
}

#[test]
fn extend_ipm_map() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-protocol Adder2 clojure.lang.IPersistentMap (add2 [this x y] [(:x this) x y])) (add2 {:x 1} 2 3))"
        ),
        "[1 2 3]"
    );
}

// ---- D: extend map form ----

#[test]
fn extend_map_form() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Double) (extend Double Greet {:greet (fn [this] \"dbl\") :greet2 (fn [this x] x)}) (greet (double 1.5)))"
        ),
        "\"dbl\""
    );
}

#[test]
fn satisfies_extended_host() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Double) (extend Double Greet {:greet (fn [this] \"dbl\")}) (satisfies? Greet (double 1.5)))"
        ),
        "true"
    );
}

// ---- E: dispatch errors ----

#[test]
fn no_impl() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(greet :kw)"),
        "java.lang.IllegalArgumentException | No implementation of method: :greet of protocol: #'user/Greet found for class: clojure.lang.Keyword"
    );
}

#[test]
fn arity_error() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(greet (PD. 1) 2 3)"),
        "java.lang.IllegalArgumentException | No single method: greet of interface: user.Greet found for function: greet of protocol: Greet"
    );
}

#[test]
fn extend_type_nonproto() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(extend-type PD :notaproto (m [this] 1))"),
        "java.lang.IllegalArgumentException | :notaproto is not a protocol"
    );
}

#[test]
fn extend_nonproto() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(extend String :notaproto {:m (fn [this] 1)})"),
        "java.lang.IllegalArgumentException | :notaproto is not a protocol"
    );
}

// ---- F: inline protocol impls ----

#[test]
fn inline_deftype() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype Q [a] Greet (greet [this] (str \"q:\" (.-a this)))) (greet (Q. 7)))"
        ),
        "\"q:7\""
    );
}

#[test]
fn inline_record() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord QR [x] Greet (greet [this] (str \"qr:\" (:x this)))) (greet (QR. 8)))"
        ),
        "\"qr:8\""
    );
}

#[test]
fn inline_reify() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(greet (reify Greet (greet [this] \"reified\")))"
        ),
        "\"reified\""
    );
}

#[test]
fn satisfies_inline_reify() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(satisfies? Greet (reify Greet (greet [this] \"r\")))"
        ),
        "true"
    );
}

// ---- G: with-open ----

#[test]
fn with_open_closes() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (with-open [c (reify java.io.Closeable (close [this] (swap! seen conj :closed)))] :body) @seen)"
        ),
        "[:closed]"
    );
}

#[test]
fn with_open_value() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(with-open [c (reify java.io.Closeable (close [this] nil))] :ret)"
        ),
        ":ret"
    );
}

#[test]
fn with_open_on_throw() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (try (with-open [c (reify java.io.Closeable (close [this] (swap! seen conj :closed)))] (throw (ex-info \"boom\" {}))) (catch clojure.lang.ExceptionInfo e @seen)))"
        ),
        "[:closed]"
    );
}

#[test]
fn with_open_reverse_order() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (with-open [a (reify java.io.Closeable (close [this] (swap! seen conj :a))) b (reify java.io.Closeable (close [this] (swap! seen conj :b)))] :go) @seen)"
        ),
        "[:b :a]"
    );
}

#[test]
fn with_open_nonclose() {
    // S16: `close/0` is a real surface member (writers), so
    // closing a Long is a gate miss — method-shape (the
    // `m-isempty-long-miss` rule), not the old 0-arg field-shape.
    // (The JVM spells this call `(. c close)` — symbol syntax,
    // field-shaped — but the port's `with-open` emits the
    // `__meth*` call the dot-forms read as.)
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(with-open [c 5] c)"),
        "java.lang.IllegalArgumentException | No matching method close found taking 0 args for class java.lang.Long"
    );
}

// ---- edges beyond the pins ----

#[test]
fn descriptor_never_dispatches() {
    // Calling through a descriptor is the no-impl error, never an
    // instance call (descriptors are classes, like the JVM — only
    // their instances dispatch).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (extend-type PD Greet (greet [this] 1)) (greet PD))"
        ),
        "java.lang.IllegalArgumentException | No implementation of method: :greet of protocol: #'user/Greet found for class: user.PD"
    );
}

#[test]
fn satisfies_descriptor_is_false() {
    // The JVM's classes never satisfy (only instances do).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-type PD Greet (greet [this] 1)) (satisfies? Greet PD))"
        ),
        "false"
    );
}

#[test]
fn extends_inline_descriptor() {
    // Inline claims show through the descriptor too.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype Q [a] Greet (greet [this] 1)) (extends? Greet Q))"
        ),
        "true"
    );
}

#[test]
fn extend_type_later_wins() {
    // A later `extend-type` replaces the inline impl (JVM order).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype Q [a] Greet (greet [this] :inline)) (extend-type Q Greet (greet [this] :extended)) (greet (Q. 1)))"
        ),
        ":extended"
    );
}

#[test]
fn multiarity_protocol() {
    let mut eval = boot();
    eval.eval_source("(defprotocol M (m [this] [this x]))")
        .unwrap();
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Long) (extend-protocol M Long (m ([this] :zero) ([this x] x))) [(m 1) (m 1 2)])"
        ),
        "[:zero 2]"
    );
}

#[test]
fn proxy_closeable() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [seen (atom [])] (with-open [c (proxy [Object Closeable] [] (close [] (swap! seen conj :p)))] :go) @seen)"
        ),
        "[:p]"
    );
}

#[test]
fn deftype_closeable() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype C [] Closeable (close [this] :shut)) (with-open [c (C.)] :go))"
        ),
        ":go"
    );
}

#[test]
fn extend_two_protocols() {
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Long) (extend Long Greet {:greet (fn [this] \"g\")} Adder2 {:add2 (fn [this x y] 0)}) [(greet 1) (add2 1 2 3)])"
        ),
        "[\"g\" 0]"
    );
}

#[test]
fn extend_merges_without_clobber() {
    // Extends merge per method/arity under the var-alter lock: a
    // later extend replaces its own arities and keeps the rest (the
    // lock itself is loom-modelled with the var lock in
    // `loom_var_lock.rs`; the merge is pure under it).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.Long) (extend-protocol Greet Long (greet [this] :first) (greet2 [this x] :g2)) (extend-protocol Greet Long (greet [this] :second)) [(greet 1) (greet2 1 2)])"
        ),
        "[:second :g2]"
    );
}

#[test]
fn handbuilt_maps_are_not_protocols() {
    // A map with `:on`/`:sigs` but not the full constructor shape
    // (`:impls` plus the `:interface` descriptor) is `not a
    // protocol` on every path — never a dispatch panic (round-1
    // review H1).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(satisfies? {:on \"X\" :sigs {}} 1)"),
        "java.lang.IllegalArgumentException | {:on \"X\", :sigs {}} is not a protocol"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.String) (extends? {:on \"X\" :sigs {}} String))",
        ),
        "java.lang.IllegalArgumentException | {:on \"X\", :sigs {}} is not a protocol"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def P {:on \"X\" :sigs {:m {:arglists '([this])}}}) (clojure.core/__protocol_invoke* (var P) \"m\" [1]))"
        ),
        "java.lang.IllegalArgumentException | {:on \"X\", :sigs {:m {:arglists ([this])}}} is not a protocol"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.String) (def P {:on \"X\" :sigs {:m {:arglists '([this])}}}) (clojure.core/__extend* (var P) String {:m (fn [this] 1)}))"
        ),
        "java.lang.IllegalArgumentException | {:on \"X\", :sigs {:m {:arglists ([this])}}} is not a protocol"
    );
    // A near-complete shape (with `:impls`, without `:interface`)
    // still is not one — so `deftype` over it is the interface
    // error, never a panic; likewise a non-descriptor
    // `:interface`.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def P {:on \"X\" :sigs {:m {:arglists '([this])}} :impls {}}) (deftype T [x] P (m [this] 1)))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: P"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def Q {:on \"X\" :sigs {} :impls {} :interface 5}) (deftype T [x] Q (m [this] 1)))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: Q"
    );
}

#[test]
fn extend_by_symbol_reference() {
    // `__extend*` also takes a quoted protocol symbol (resolved at
    // call time); resolving to a non-protocol (or nothing) is the
    // stock error.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (clojure.core/__extend* 'Greet 'String {:greet (fn [this] :sym)}) (greet \"x\"))"
        ),
        ":sym"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__extend* 'PD 'String {:greet (fn [this] 1)})"
        ),
        "java.lang.IllegalArgumentException | PD is not a protocol"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__extend* 'Nope 'String {:greet (fn [this] 1)})"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: Nope in this context"
    );
    // A qualified name through an unknown namespace misses the
    // same way.
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__extend* 'foo/Nope 'String {:greet (fn [this] 1)})"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: foo/Nope in this context"
    );
}

#[test]
fn extend_class_values() {
    // S16: extensions target classes by value — an evaluated
    // import through direct `__extend*`, or a var holding a class
    // through the quoting macros.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (clojure.core/__extend* (var Greet) String {:greet (fn [this] :byval)}) (greet \"x\"))"
        ),
        ":byval"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (def strcls String) (extend-protocol Greet strcls (greet2 [this x] :byvar)) (greet2 \"y\" 1))"
        ),
        ":byvar"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (extends? Greet String))"
        ),
        "true"
    );
}

#[test]
fn protocol_native_guards() {
    // Direct `__protocol*` misuse fails the casts, and only an
    // interface var builds a protocol.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__protocol* 5 (var Greet-interface))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def p (PD. 1)) (clojure.core/__protocol* {} (var p)))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: user/p"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def n 5) (clojure.core/__protocol* {} (var n)))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: user/n"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__protocol* {} 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Var (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Var is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__protocol* {} (var PD))"),
        "java.lang.IllegalArgumentException | Not an interface: user.PD"
    );
}

#[test]
fn extend_target_shapes() {
    // Extension targets beyond plain symbols: slash-qualified
    // names, descriptor values, and the `not a type` rejections.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (clojure.core/__extend* 'Greet 'user/PD {:greet (fn [this] :ns)}) (greet (PD. 1)))"
        ),
        ":ns"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (clojure.core/__extend* 'Greet PD {:greet (fn [this] :dv)}) (greet (PD. 2)))"
        ),
        ":dv"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__extend* 'Greet 5 {:greet (fn [this] 1)})"
        ),
        "java.lang.IllegalArgumentException | Not a type: 5"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def p (PD. 1)) (clojure.core/__extend* 'Greet 'p {:greet (fn [this] 1)}))"
        ),
        "java.lang.IllegalArgumentException | Not a type: p"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def n 5) (clojure.core/__extend* 'Greet 'n {:greet (fn [this] 1)}))"
        ),
        "java.lang.IllegalArgumentException | Not a type: n"
    );
    // An unresolvable type name stays unresolvable (a typo must
    // fail, not extend a phantom `java.lang` class).
    assert_eq!(
        uncaught(&mut eval, "(extend-type NoSuchType Greet (greet [this] 1))"),
        "java.lang.RuntimeException | Unable to resolve symbol: NoSuchType in this context"
    );
    // A var holding a class-name string names its class (the
    // legacy core `String`/`Long` shape; S16 retired the core
    // stubs, so user vars carry it now).
    assert_eq!(
        run(
            &mut eval,
            "(do (def StrType \"java.lang.String\") (clojure.core/__extend* 'Greet 'StrType {:greet (fn [this] :str)}) (greet \"hi\"))"
        ),
        ":str"
    );
}

#[test]
fn extend_native_guards() {
    // Non-method keys in the methods map are skipped, and the
    // methods arg must be a map.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.lang.String) (map? (clojure.core/__extend* 'Greet 'String {5 6})))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__extend* 'Greet 'String 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
}

#[test]
fn extend_instance_value_is_not_a_type() {
    // An instance *value* in type position names no class (its
    // print form carries the deterministic identity digits).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__extend* 'Greet (PD. 1) {:greet (fn [this] 1)})"
        ),
        "java.lang.IllegalArgumentException | Not a type: #object[user.PD 0xf0ca55f6 \"user.PD@f0ca55f6\"]"
    );
}

#[test]
fn partial_extension_misses() {
    // Declared-but-unextended methods miss to the no-impl error:
    // both the missing-method and the missing-arity shapes.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.Long) (extend-protocol Greet Long (greet [this] :g)) (greet2 1 2))"
        ),
        "java.lang.IllegalArgumentException | No implementation of method: :greet2 of protocol: #'user/Greet found for class: java.lang.Long"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.Long) (defprotocol P2 (m [this] [this x])) (extend-type Long P2 (m [this] :zero)) (m 1 2))"
        ),
        "java.lang.IllegalArgumentException | No implementation of method: :m of protocol: #'user/P2 found for class: java.lang.Long"
    );
}

#[test]
fn vector_and_broken_arglists() {
    // `:arglists` tolerate vectors (JVM shape is lists); broken
    // shapes degrade to the arity error, never a panic.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def PV (clojure.core/__protocol* '{:m {:arglists [[this]]}} (var Greet-interface))) (clojure.core/__protocol_invoke* (var PV) \"m\" [1]))"
        ),
        "java.lang.IllegalArgumentException | No implementation of method: :m of protocol: #'user/PV found for class: java.lang.Long"
    );
    for sigs in ["'{:m {:arglists 5}}", "'{:m 5}", "'{:m {:arglists (5)}}"] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!(
                    "(do (def PB (clojure.core/__protocol* {sigs} (var Greet-interface))) (clojure.core/__protocol_invoke* (var PB) \"m\" [1]))"
                )
            ),
            "java.lang.IllegalArgumentException | No single method: m of interface: user.Greet found for function: m of protocol: PB"
        );
    }
}

#[test]
fn protocol_invoke_guards() {
    // Direct `__protocol_invoke*` misuse fails the casts, and an
    // empty call vector fails arity.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__protocol_invoke* (var Greet) 5 [\"x\"])"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__protocol_invoke* (var Greet) \"greet\" \"x\")"
        ),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentVector (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__protocol_invoke* (var Greet) \"greet\" [])"
        ),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__protocol_invoke*"
    );
}

#[test]
fn proxy_inline_protocol_dispatch() {
    // A proxy claiming a protocol dispatches from its own table
    // (bodies exclude `this`, like every proxy call).
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (defprotocol PP (pm [this])) (def px (proxy [Object PP] [] (pm [] :px))) (pm px))"
        ),
        ":px"
    );
}

#[test]
fn extends_guards() {
    // `extends?` takes a class: instances and scalars fail the cast.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(extends? Greet (PD. 1))"),
        "java.lang.ClassCastException | class user.PD cannot be cast to class java.lang.Class (user.PD is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(extends? Greet 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn extends_ipm_fallback_for_records() {
    // A record descriptor answers through the `IPersistentMap`
    // entry when it has no direct extension.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (extend-protocol Adder2 clojure.lang.IPersistentMap (add2 [this x y] 0)) (extends? Adder2 PR))"
        ),
        "true"
    );
}

#[test]
fn invoke_on_unclaimed_instance() {
    // An instance whose type claims nothing misses to the no-impl
    // error naming its class.
    let mut eval = boot();
    prelude(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.Long) (deftype Q2 [a]) (extend-protocol Greet Long (greet [this] :g)) (greet (Q2. 1)))"
        ),
        "java.lang.IllegalArgumentException | No implementation of method: :greet of protocol: #'user/Greet found for class: user.Q2"
    );
}
