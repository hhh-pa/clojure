// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #33 stage-1 tests: every feasible pin in
//! `docs/i33-typechain/probes/types.out` reproduced against the
//! port, plus macro-shape errors, fenced-surface errors, and the
//! documented divergences. Pins that need out-of-scope vars
//! (`supers`, `class`, `instance?`) or the JVM's identity hashes
//! assert the port's shape instead; each such case says so.
//!
//! Oracle command (see the probe header):
//! `CP=.../clojure-1.13.0-alpha10.jar:... java -Duser.language=en
//! -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main
//! docs/i33-typechain/probes/types.clj`.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form
/// (earlier `def`-emitting macro values are discarded).
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

/// The `Class | message` of an uncaught throw. Macroexpansion-time
/// throws surface as `Compile` (same class/message the oracle prints
/// uncaught), so both are accepted.
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

// `same-module` ClassCastExceptions render without a loader hash
// (the port has one loader): `class A cannot be cast to class B
// (A and B are in unnamed module of loader 'app')`.
fn cce_same(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} and {to} are in unnamed module of loader 'app')"
    )
}

// Cross-module casts name both loaders.
fn cce_cross(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} is in unnamed module of loader 'app'; {to} is in module java.base of loader 'bootstrap')"
    )
}

// Both sides bootstrap-loaded: one shared `java.base` clause (the
// port's `cce_message` matches the JVM here).
fn cce_boot(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} and {to} are in module java.base of loader 'bootstrap')"
    )
}

// Calling a host value as a function: the port's stable
// analyzer-cast text (`Cannot cast A to B`, as for every other
// non-`IFn` call). The oracle additionally names the defining
// `DynamicClassLoader` (with its identity hash) — unreachable with
// the port's single loader, so the loader clause is a documented
// divergence while the classes match.
fn cce_invoke(from: &str) -> String {
    format!("java.lang.ClassCastException | Cannot cast {from} to clojure.lang.IFn")
}

// ---- deftype ----

#[test]
fn dtor_fields() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) [(.-a (Pair. 10 20)) (.-b (Pair. 10 20))]"
        ),
        "[10 20]"
    );
}

#[test]
fn dtor_descriptor_str() {
    // Oracle `dtor-class` needs `class` (N1); the descriptor prints
    // the same name through `str`.
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(deftype Pair [a b]) (str Pair)"),
        "\"user.Pair\""
    );
    assert_eq!(
        run(&mut eval, "(deftype Pair [a b]) (pr-str Pair)"),
        "\"user.Pair\""
    );
}

#[test]
fn dtor_not_lookupable() {
    // Oracle `dtor-kw`, `dtor-get`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) [(:a (Pair. 1 2)) (get (Pair. 1 2) :a :dflt) (get (Pair. 1 2) :a)]"
        ),
        "[nil :dflt nil]"
    );
}

#[test]
fn dtor_count_unsupported() {
    // Oracle `dtor-count` (verbatim — simple name).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (count (Pair. 1 2))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Pair"
    );
}

#[test]
fn dtor_identity_equality() {
    // Oracle `dtor-eq`: distinct instances are never `=`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) [(= (Pair. 1 2) (Pair. 1 2)) (let [p (Pair. 1 2)] (= p p))]"
        ),
        "[false true]"
    );
}

#[test]
fn dtor_predicates() {
    // Oracle `g-*-dt`, `dtor-record?`, `dtor-map?`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (def d (Pair. 1 2)) [(map? d) (record? d) (coll? d) (associative? d) (counted? d) (seqable? d) (sequential? d) (set? d) (vector? d) (list? d) (ifn? d) (fn? d)]"
        ),
        "[false false false false false false false false false false false false]"
    );
}

#[test]
fn dtor_ctor_arity() {
    // Oracle `dtor-arity1`, `dtor-arity0`, `i-ctor-extra`.
    let mut eval = boot();
    for src in ["(Pair. 1)", "(Pair.)", "(Pair. 1 2 3)"] {
        assert_eq!(
            uncaught(&mut eval, &format!("(deftype Pair [a b]) {src}")),
            "java.lang.IllegalArgumentException | No matching ctor found for class user.Pair",
            "{src}"
        );
    }
}

#[test]
fn dtor_unresolvable_type() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(Nosuch. 1)"),
        "java.lang.RuntimeException | Unable to resolve symbol: Nosuch in this context"
    );
}

#[test]
fn dtor_not_a_type() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(def Pair 5) (Pair. 1)"),
        "java.lang.IllegalArgumentException | Not a type: Pair"
    );
}

#[test]
fn dtor_explicit_object_tostring() {
    // A record-style `Object` listing works for method bodies too.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype P [a] Object (toString [this] (str \"P:\" (.-a this)))) (str (P. 7))"
        ),
        "\"P:7\""
    );
}

#[test]
fn dtor_non_interface() {
    // A resolvable non-interface is `Not an interface`.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord R [x]) (deftype P [a] R (m [this] 1))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: R"
    );
}

#[test]
fn dtor_host_super_unresolvable() {
    // Host classes are interop (N1): since stage 2 `String` resolves
    // (as its class-name string, for `extends?`), naming it as an
    // interface is the accurate `Not an interface` error — still a
    // rejection, one step closer to the JVM.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(deftype P [a] String (s [this] 1))"),
        "java.lang.IllegalArgumentException | Not an interface: String"
    );
}

#[test]
fn dtor_methods_and_overloads() {
    // Oracle `dtor-meth`, `dtor-m1`, `dtor-m2` (fields are not
    // lexicals — method bodies read them through `.-`).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(definterface Adder (add [x y])) (definterface M2 (m [x]) (m [x y])) (deftype P2 [a] Adder (add [this x y] (+ (.-a this) x y))) (deftype D2 [] M2 (m [this x] [:one x]) (m [this x y] [:two x y])) [(.add (P2. 1) 2 3) (.m (D2.) 1) (.m (D2.) 1 2)]"
        ),
        "[6 [:one 1] [:two 1 2]]"
    );
}

#[test]
fn dtor_method_errors() {
    // Oracle `k-meth-arity`, `k-meth-nosuch`, `k-meth-wrongtype`
    // (verbatim — counts always read `{n} args`).
    let mut eval = boot();
    let setup = "(definterface Adder (add [x y])) (deftype P2 [a] Adder (add [this x y] (+ (.-a this) x y))) (deftype Pair [a b])";
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (.add (P2. 1) 2)")),
        "java.lang.IllegalArgumentException | No matching method add found taking 1 args for class user.P2"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (.nosuch (P2. 1))")),
        "java.lang.IllegalArgumentException | No matching field found: nosuch for class user.P2"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (.add (Pair. 1 2) 1 2)")),
        "java.lang.IllegalArgumentException | No matching method add found taking 2 args for class user.Pair"
    );
}

#[test]
fn dtor_method_not_in_interface() {
    // Oracle `ifprobe`: arities outside the signature fail the same
    // lookup.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(definterface I (m [x])) (deftype P [a] I (m [this x y] 1))"
        ),
        "java.lang.IllegalArgumentException | Can't define method not in interfaces: m"
    );
}

#[test]
fn dtor_str_print_shapes() {
    // Oracle `j-str-dt`, `j-tostring-dt`, `m-pr-dt`: same shapes,
    // stable digits (the JVM prints identity hashes).
    let mut eval = boot();
    let s = run(&mut eval, "(deftype Pair [a b]) (str (Pair. 1 2))");
    assert!(s.starts_with("\"user.Pair@"), "{s}");
    // The `.toString` interop form answers the same default.
    let t = run(&mut eval, "(deftype Pair [a b]) (.toString (Pair. 1 2))");
    assert!(t.starts_with("\"user.Pair@"), "{t}");
    assert_eq!(t, s);
    let p = run(&mut eval, "(deftype Pair [a b]) (pr-str (Pair. 1 2))");
    assert!(p.starts_with("\"#object[user.Pair 0x"), "{p}");
    assert!(p.contains("user.Pair@"), "{p}");
    // Deterministic: equal instances render equally.
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (= (str (Pair. 1 2)) (str (Pair. 1 2)))"
        ),
        "true"
    );
}

#[test]
fn dtor_collection_errors() {
    // Oracle `i-assoc`, `i-dissoc`, `i-conj` (the port's loader
    // wording — one loader, no hashes).
    let mut eval = boot();
    let setup = "(deftype Pair [a b])";
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (assoc (Pair. 1 2) :a 1)")),
        cce_same("user.Pair", "clojure.lang.Associative")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (dissoc (Pair. 1 2) :a)")),
        cce_same("user.Pair", "clojure.lang.IPersistentMap")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (conj (Pair. 1 2) [:a 1])")),
        cce_same("user.Pair", "clojure.lang.IPersistentCollection")
    );
}

#[test]
fn dtor_seq_lookup_errors() {
    // Oracle `i-keys`, `i-seq`, `i-contains`, `i-find`, `i-nth`,
    // `i-empty`, `i-invoke` (verbatim except the invoke cast).
    let mut eval = boot();
    let setup = "(deftype Pair [a b])";
    for src in ["(keys (Pair. 1 2))", "(seq (Pair. 1 2))"] {
        assert_eq!(
            uncaught(&mut eval, &format!("{setup} {src}")),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: user.Pair",
            "{src}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (contains? (Pair. 1 2) :a)")),
        "java.lang.IllegalArgumentException | contains? not supported on type: user.Pair"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (find (Pair. 1 2) :a)")),
        "java.lang.IllegalArgumentException | find not supported on type: user.Pair"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (nth (Pair. 1 2) 0)")),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Pair"
    );
    assert_eq!(
        run(&mut eval, &format!("{setup} (empty (Pair. 1 2))")),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} ((Pair. 1 2) :a)")),
        cce_invoke("user.Pair")
    );
}

#[test]
fn dtor_field_errors() {
    // Oracle `k-field-nosuch` (verbatim); other values answer with
    // their own class.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (.-nosuch (Pair. 1 2))"),
        "java.lang.IllegalArgumentException | No matching field found: nosuch for class user.Pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(.-x {:a 1})"),
        "java.lang.IllegalArgumentException | No matching field found: x for class clojure.lang.PersistentArrayMap"
    );
}

#[test]
fn dtor_method_on_other_values() {
    // The fenced surface answers uniformly off-host (including
    // `nil` — a documented divergence from the JVM's NPE).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(.foo 5)"),
        "java.lang.IllegalArgumentException | No matching field found: foo for class java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(.foo 5 1 2)"),
        "java.lang.IllegalArgumentException | No matching method foo found taking 2 args for class java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(.foo nil)"),
        "java.lang.IllegalArgumentException | No matching field found: foo for class null"
    );
}

#[test]
fn dtor_compare_rseq_transient() {
    // Oracle `n-compare`, `n-rseq`, `h-transient-type` (the port's
    // loader wording on the casts).
    let mut eval = boot();
    let setup = "(deftype Pair [a b]) (defrecord Point [x y])";
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("{setup} (compare (->Point 1 2) (->Point 1 2))")
        ),
        cce_cross("user.Point", "java.lang.Comparable")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (rseq (->Point 1 2))")),
        cce_same("user.Point", "clojure.lang.Reversible")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} (transient (->Point 1 2))")),
        cce_same("user.Point", "clojure.lang.IEditableCollection")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("{setup} (compare (Pair. 1 2) (Pair. 1 2))")
        ),
        cce_cross("user.Pair", "java.lang.Comparable")
    );
}

#[test]
fn dtor_hash_deterministic() {
    // No oracle exists (the JVM uses identity): the port mixes
    // content deterministically instead.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (= (hash (Pair. 1 2)) (hash (Pair. 1 2)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (integer? (hash (Pair. 1 2)))"
        ),
        "true"
    );
}

#[test]
fn dtor_macro_shapes() {
    // Bad names, bad fields, orphan and variadic methods, bad
    // specs — all macro-time IAE.
    let mut eval = boot();
    for (src, msg) in [
        (
            "(deftype 5 [a])",
            "First argument to deftype must be a symbol",
        ),
        (
            "(deftype foo.Bar [a])",
            "deftype names must be simple symbols",
        ),
        (
            "(deftype P 5)",
            "deftype fields must be a vector of simple symbols",
        ),
    ] {
        let got = uncaught(&mut eval, src);
        assert!(got.contains(msg), "{src} -> {got}");
    }
    assert!(
        uncaught(&mut eval, "(deftype P [a] (m [this] 1))")
            .contains("Can't define method not in interfaces: m")
    );
    assert!(
        uncaught(
            &mut eval,
            "(definterface I (m [x])) (deftype P [a] I (m [this & xs] 1))"
        )
        .contains("Can't define variadic method: m")
    );
    assert!(
        uncaught(&mut eval, "(deftype P [a] :load-ns)")
            .contains("Unsupported deftype option: :load-ns")
    );
}

#[test]
fn dtor_doc_and_attr() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype D \"docs\" {:a 1} [x]) [(.-x (D. 5)) (:doc (meta #'D)) (:a (meta #'D))]"
        ),
        "[5 \"docs\" 1]"
    );
}

// ---- defrecord ----

#[test]
fn rec_factories() {
    // Oracle `rec-pos`, `rec-mapf`, `rec-mapf-extra`, `rec-ctor`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(:x (->Point 1 2)) (:y (map->Point {:x 1 :y 2})) (:z (map->Point {:x 1 :y 2 :z 3})) (:x (Point. 1 2))]"
        ),
        "[1 2 3 1]"
    );
}

#[test]
fn rec_mapness() {
    // Oracle `rec-record?`, `rec-map?`, `rec-count`, `rec-keys`,
    // `rec-vals`, `rec-seq`, `rec-get`, `rec-contains`,
    // `rec-find`, `rec-kw-dflt`, `g-coll-rec`, `g-assoc-rec`,
    // `g-counted-rec`, `g-seqable-rec`, `g-ifn-rec` and the
    // false predicates.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(record? p) (map? p) (count p) (keys p) (vals p) (seq p) (get p :nope :dflt) (contains? p :y) (find p :x) (:nope p :dflt) (coll? p) (associative? p) (counted? p) (seqable? p) (ifn? p) (sequential? p) (set? p) (vector? p) (list? p) (fn? p)]"
        ),
        "[true true 2 (:x :y) (1 2) ([:x 1] [:y 2]) :dflt true [:x 1] :dflt true true true true false false false false false false]"
    );
}

#[test]
fn rec_get_hit_and_misses() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(get p :x) (contains? p :nope) (find p :nope) (:x p) (:nope p :d)]"
        ),
        "[1 false nil 1 :d]"
    );
}

#[test]
fn rec_assoc_conj_dissoc() {
    // Oracle `rec-assoc-type`, `rec-assoc-count`, `rec-assoc-val`,
    // `rec-dissoc-type`, `rec-dissoc-count`, `h-conj-vec`,
    // `h-conj-vec-type`, `h-conj-map-type`,
    // `h-dissoc-missing-type`, `h-assoc-existing`, `h-update-type`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(record? (assoc p :z 3)) (count (assoc p :z 3)) (:z (assoc p :z 3)) (record? (dissoc p :x)) (map? (dissoc p :x)) (count (dissoc p :x)) (conj p [:z 3]) (record? (conj p [:z 3])) (record? (conj p {:z 3})) (record? (dissoc p :nope)) (assoc p :x 9) (record? (update p :x inc))]"
        ),
        "[true 3 3 false true 1 #user.Point{:x 1, :y 2, :z 3} true true true #user.Point{:x 9, :y 2} true]"
    );
}

#[test]
fn rec_equality() {
    // Oracle `rec-eq`, `rec-eq-map` (false!), `rec-eq-diff`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(= (->Point 1 2) (->Point 1 2)) (= (->Point 1 2) {:x 1 :y 2}) (= (->Point 1 2) (->Point 1 3))]"
        ),
        "[true false false]"
    );
}

#[test]
fn rec_merge_into_select() {
    // Oracle `rec-merge-type`, `rec-into`, `rec-select`,
    // `h-getin`, `h-destructure`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(record? (merge p {:z 3})) (into {} p) (select-keys p [:y]) (get-in p [:x]) (let [{:keys [x]} p] x)]"
        ),
        "[true {:x 1, :y 2} {:y 2} 1 1]"
    );
}

#[test]
fn rec_seq_first_rest_into_vec() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(first p) (rest p) (into [] p) (conj p nil)]"
        ),
        "[[:x 1] ([:y 2]) [[:x 1] [:y 2]] #user.Point{:x 1, :y 2}]"
    );
}

#[test]
fn rec_factory_arity() {
    // The factories are named closures: stock `ArityException`
    // naming `user/evalN/->Point--M` (the oracle's own counters,
    // `user/evalN/->Point--N`, never appear verbatim).
    let mut eval = boot();
    let one = uncaught(&mut eval, "(defrecord Point [x y]) (->Point 1)");
    assert!(
        one.starts_with(
            "clojure.lang.ArityException | Wrong number of args (1) passed to: user/eval"
        ),
        "{one}"
    );
    assert!(one.contains("->Point--"), "{one}");
    let two = uncaught(&mut eval, "(defrecord Point [x y]) (map->Point 1 2)");
    assert!(
        two.starts_with(
            "clojure.lang.ArityException | Wrong number of args (2) passed to: user/eval"
        ),
        "{two}"
    );
    assert!(two.contains("map->Point--"), "{two}");
}

#[test]
fn rec_empty_transient_nth() {
    // Oracle `h-empty`, `h-empty?`, `h-nth`.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (empty (->Point 1 2))"),
        "java.lang.UnsupportedOperationException | Can't create empty: user.Point"
    );
    // Oracle `h-empty-type` wraps the throw in `class` (N1,
    // out of scope) — the throw itself is pinned above.
    assert_eq!(
        run(&mut eval, "(defrecord Point [x y]) (empty? (->Point 1 2))"),
        "false"
    );
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (nth (->Point 1 2) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Point"
    );
}

#[test]
fn rec_invoke_not_supported() {
    // Oracle `h-invoke-hit`, `h-invoke-miss`, `h-invoke-arity`:
    // records are not `IFn` (unlike plain maps).
    let mut eval = boot();
    let setup = "(defrecord Point [x y])";
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} ((->Point 1 2) :x)")),
        cce_invoke("user.Point")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("{setup} ((->Point 1 2) :nope :d)")),
        cce_invoke("user.Point")
    );
}

#[test]
fn rec_map_factory_shapes() {
    // Oracle `h-mapnil`, `h-mapempty`; ad-hoc `map->int`,
    // `map->vec`, `map->str` (flatten through `seq`).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(map->Point nil) (map->Point {})]"
        ),
        "[#user.Point{:x nil, :y nil} #user.Point{:x nil, :y nil}]"
    );
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (map->Point 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (map->Point [1 2])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (map->Point \"ab\")"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Character"
    );
    assert_eq!(
        uncaught(&mut eval, "(defrecord Point [x y]) (map->Point [[:a]])"),
        "java.lang.IllegalArgumentException | No value supplied for key: :a"
    );
}

#[test]
fn rec_hash_exact() {
    // Oracle `l-hash-rec`, `l-hash-m13`, `l-hash-p13`,
    // `l-hash-swapped`, `l-hash-ext`: content-hash XOR the plain
    // munged-class-name symbol hash (solved §L).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(hash (->Point 1 2)) (hash {:x 1 :y 3}) (hash (->Point 1 3)) (hash (->Point 2 1)) (hash (assoc (->Point 1 2) :z 3))]"
        ),
        "[-445655453 -656809281 -977466775 151370679 -126088769]"
    );
}

#[test]
fn rec_hash_code_matches_map() {
    // Oracle `l-rec-hashcode`, `l-map-hashcode`: records share the
    // content map's `hashCode` (both go through the `.hashCode`
    // interop form, as the probe does).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (.hashCode (->Point 1 2))"
        ),
        "2027824014"
    );
    // Oracle `l-map-hashcode` pins the same int for the plain map;
    // `.hashCode` on non-host values is a pre-existing gap outside
    // stage 1 (`.toString`/`.hashCode` have no `Object` fallback
    // off host), so the map half is not re-proven here.
}

#[test]
fn rec_hash_symbol_vectors() {
    // The formula's symbol half answers the oracle's ints
    // (`l-hash-strname`, `l-hash-symname` — equal by `=`).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(hash (symbol \"user.Point\")) (hash 'user.Point) (hash \"user.Point\")]"
        ),
        "[493152470 493152470 1912907433]"
    );
}

#[test]
fn rec_dashed_name() {
    // Oracle `n-hash-dashed`, `n-map-x1`, `n-sym-underscore`,
    // `n-sym-dash`, `n-pr-dashed`: only the namespace munges.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord my-rec [x]) [(hash (my-rec. 1)) (hash {:x 1}) (hash (symbol \"user.my_rec\")) (hash (symbol \"user.my-rec\")) (pr-str (my-rec. 1))]"
        ),
        "[-760237083 461195660 1783825782 -908920215 \"#user.my-rec{:x 1}\"]"
    );
}

#[test]
fn rec_print_str_shapes() {
    // Oracle `j-str-rec`, `j-pr-rec`, `j-print-rec`: `pr`/`print`
    // use the tagged form; `str` uses `Name@hex`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) [(pr-str p) (with-out-str (print p))]"
        ),
        "[\"#user.Point{:x 1, :y 2}\" \"#user.Point{:x 1, :y 2}\"]"
    );
    // The fallback hex is the record hash (`rec_hash_exact` locks
    // the int; `0xe56fd663` is its hex).
    let s = run(&mut eval, "(defrecord Point [x y]) (str (->Point 1 2))");
    assert_eq!(s, "\"user.Point@e56fd663\"", "{s}");
}

#[test]
fn rec_meta() {
    // Oracle `n-meta`, `n-pr-meta`; ad-hoc `assoc-meta`,
    // `conj-meta`, `dissoc-meta`, `ctor-meta`, `new-meta`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(meta (with-meta (->Point 1 2) {:a 1})) (binding [*print-meta* true] (pr-str (with-meta (->Point 1 2) {:a 1}))) (meta (Point. 1 2)) (meta (->Point 1 2))]"
        ),
        "[{:a 1} \"^{:a 1} #user.Point{:x 1, :y 2}\" nil nil]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (def m (with-meta (->Point 1 2) {:a 1})) [(meta (assoc m :z 3)) (meta (conj m [:z 3])) (meta (dissoc m :x)) (meta (dissoc m :nope))]"
        ),
        "[{:a 1} {:a 1} {:a 1} {:a 1}]"
    );
}

#[test]
fn rec_custom_tostring() {
    // Records can define `Object` methods; `str` runs them while
    // `pr` keeps the tagged form.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord R [x] Object (toString [this] (str \"R:\" (.-x this)))) (def r (->R 5)) [(str r) (pr-str r)]"
        ),
        "[\"R:5\" \"#user.R{:x 5}\"]"
    );
}

#[test]
fn rec_tostring_nonstring() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord R [x] Object (toString [this] 5)) (str (->R 1))"
        ),
        cce_boot("java.lang.Long", "java.lang.String")
    );
}

#[test]
fn rec_key_set_consistency() {
    // Equal records hash equally: usable as map keys and set
    // members (`=`-consistency the row demands).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) [(get {(->Point 1 2) :v} (->Point 1 2)) (contains? #{(->Point 1 2)} (->Point 1 2)) (distinct? (->Point 1 2) (->Point 1 2))]"
        ),
        "[:v true false]"
    );
}

#[test]
fn rec_redef_mints_new_type() {
    // Like the JVM's new class: old instances stop equalling new
    // ones after a re-`def`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord R [x]) (def r1 (->R 1)) (defrecord R [x]) (= r1 (->R 1))"
        ),
        "false"
    );
}

#[test]
fn rec_realizes_lazy_fields() {
    // `realize_deep` walks record content like map content (a seq
    // field prints as a seq — records hold values as written).
    let mut eval = boot();
    let vs = eval
        .eval_source("(defrecord R [x]) (->R (map inc [1 2]))")
        .unwrap();
    let r = eval.realize_deep(vs.last().unwrap()).unwrap();
    assert_eq!(pr(&r), "#user.R{:x (2 3)}");
}

// ---- reify / proxy ----

#[test]
fn reify_str_and_print() {
    // Oracle `reify-str`, `m-pr-reify`, `m-print-reify`: `str`
    // runs `toString`; `pr` keeps the default shape with the
    // fallback embedded (print is pure — a documented divergence).
    let mut eval = boot();
    let out = run(
        &mut eval,
        "(def r (reify Object (toString [this] \"hi!\"))) [(str r) (pr-str r) (with-out-str (print r))]",
    );
    let v = &out;
    assert!(v.starts_with("[\"hi!\" \"#object[user$reify 0x"), "{v}");
    assert!(v.contains("user$reify@"), "{v}");
}

#[test]
fn reify_methods_identity() {
    // Oracle `reify-meth`, `reify-self-eq`, `reify-distinct`,
    // `reify-objinst`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(definterface Adder (add [x y])) (def r (reify Adder (add [this x y] (* x y)))) [(.add r 3 4) (= r r) (= (reify Object) (reify Object))]"
        ),
        "[12 true false]"
    );
}

#[test]
fn reify_bare_object_str() {
    // No `toString` method: the stable `user$reify@hex` fallback
    // (the JVM embeds eval counters — a documented divergence).
    let mut eval = boot();
    let s = run(&mut eval, "(str (reify Object))");
    assert!(s.starts_with("\"user$reify@"), "{s}");
}

#[test]
fn reify_orphan_method() {
    let mut eval = boot();
    assert!(
        uncaught(&mut eval, "(reify Object (nosuch [this] 1))")
            .contains("Can't define method not in interfaces: nosuch")
    );
}

#[test]
fn reify_non_interface() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(defrecord R [x]) (reify R (m [this] 1))"),
        "java.lang.IllegalArgumentException | Not an interface: R"
    );
}

#[test]
fn proxy_str_iface() {
    // Oracle `proxy-str`, `proxy-objinst`, `proxy-iface`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(definterface Adder (add [x y])) (def p (proxy [Object] [] (toString [] \"prox\"))) [(str p) (.add (proxy [Object Adder] [] (add [x y] (- x y))) 10 4)]"
        ),
        "[\"prox\" 6]"
    );
}

#[test]
fn proxy_fences() {
    // Only `Object` superclasses, no ctor args: host classes are
    // interop (N1).
    let mut eval = boot();
    assert!(
        uncaught(&mut eval, "(proxy [String] [] (toString [] \"x\"))")
            .contains("Cannot extend String: proxy supports Object only")
    );
    assert_eq!(
        uncaught(&mut eval, "(proxy [Object] [\"x\"] (toString [] \"x\"))"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.Object"
    );
    assert!(
        uncaught(&mut eval, "(proxy [] [] (toString [] \"x\"))")
            .contains("proxy superclass must be a nonempty vector of symbols")
    );
    assert!(
        uncaught(&mut eval, "(proxy [Object] \"x\" (toString [] \"x\"))")
            .contains("proxy args must be a vector")
    );
}

// ---- definterface / gen-interface ----

#[test]
fn iface_descriptor() {
    // Oracle `rec-iface`: descriptors print `interface Name`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(definterface Adder (add [x y])) [(str Adder) (pr-str Adder)]"
        ),
        "[\"interface user.Adder\" \"interface user.Adder\"]"
    );
}

#[test]
fn iface_bad_sig() {
    let mut eval = boot();
    assert!(
        uncaught(&mut eval, "(definterface I (m [x] [x y]))")
            .contains("Invalid interface signature")
    );
    assert!(uncaught(&mut eval, "(definterface I 5)").contains("Invalid interface signature"));
}

#[test]
fn iface_doc_and_attr() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(definterface I \"docs\" {:a 1} (m [x])) [(:doc (meta #'I)) (:a (meta #'I)) (str I)]"
        ),
        "[\"docs\" 1 \"interface user.I\"]"
    );
}

#[test]
fn geniface_descriptor() {
    // Oracle `geniface`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(gen-interface :name user.Greetable :methods [[greet [String] String]]) (str user/Greetable)"
        ),
        "\"interface user.Greetable\""
    );
}

#[test]
fn geniface_fences() {
    let mut eval = boot();
    assert!(
        uncaught(&mut eval, "(gen-interface :methods [])")
            .contains("Must supply :name to gen-interface")
    );
    assert!(
        uncaught(&mut eval, "(gen-interface :name X :bogus 1)")
            .contains("Unsupported gen-interface option: :bogus")
    );
    assert!(
        uncaught(&mut eval, "(gen-interface :name X :methods [[m [a]]])")
            .contains("Invalid gen-interface method")
    );
    assert!(
        uncaught(&mut eval, "(gen-interface :name 5)")
            .contains("gen-interface :name must be a symbol or string")
    );
}

// ---- gen-class ----

#[test]
fn genclass_construct_and_forward() {
    // Divergence: the descriptor constructs (the JVM only
    // materializes the class under AOT — oracle `genclass` throws
    // there), and methods forward to `prefix` fns.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(gen-class :name user.Klass :prefix \"klass-\" :methods [[hello [] String]]) (defn klass-hello [this] \"yo\") [(.hello (Klass.)) (str Klass)]"
        ),
        "[\"yo\" \"user.Klass\"]"
    );
}

#[test]
fn genclass_default_prefix_and_string_name() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(gen-class :name \"com.Foo.Bar\" :methods [[hello [] String]]) (defn Bar-hello [this] \"bar\") (.hello (Bar.))"
        ),
        "\"bar\""
    );
}

#[test]
fn genclass_missing_fn() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(gen-class :name user.Klass :prefix \"klass-\" :methods [[hello [] String]]) (.hello (Klass.))"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: user/klass-hello in this context"
    );
}

#[test]
fn genclass_unlisted_method() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(gen-class :name user.Klass :methods []) (.nosuch (Klass.))"
        ),
        "java.lang.IllegalArgumentException | No matching method nosuch found taking 0 args for class user.Klass"
    );
}

#[test]
fn genclass_fences() {
    let mut eval = boot();
    assert!(
        uncaught(&mut eval, "(gen-class :methods [])").contains("Must supply :name to gen-class")
    );
    assert!(
        uncaught(&mut eval, "(gen-class :name X :init {})")
            .contains("Unsupported gen-class option: :init")
    );
    assert!(
        uncaught(&mut eval, "(gen-class :name X :extends String)")
            .contains("Cannot extend String: gen-class supports Object only")
    );
    assert!(
        uncaught(&mut eval, "(gen-class :name X :methods [[m [a]]])")
            .contains("Invalid gen-class method")
    );
    assert!(
        uncaught(&mut eval, "(gen-class :name X :prefix 5)")
            .contains("gen-class :prefix must be a string")
    );
    assert!(
        uncaught(&mut eval, "(gen-class :name X :implements [5])")
            .contains("Invalid gen-class interface")
    );
    // Odd option lists fail in `hash-map` like any odd call.
    assert!(uncaught(&mut eval, "(gen-class :name)").contains("No value supplied for key"));
}

// ---- fenced call surface ----

#[test]
fn dot_arity_fallthrough() {
    // `(.-a)` is not a call shape: the stock unresolvable error.
    let mut eval = boot();
    match eval.eval_source("(.-a)") {
        Err(SourceError::Compile(e)) => assert_eq!(
            format!("{} | {}", e.class, e.message),
            "java.lang.RuntimeException | Unable to resolve symbol: .-a in this context"
        ),
        other => panic!("expected a compile error, got {other:?}"),
    }
}

#[test]
fn dot_interception_skips_non_dot_heads() {
    // A list headed by a non-dot symbol is never dot-rewritten
    // (e.g. `..`, which the port does not define — a pre-existing
    // gap outside stage 1 — still resolves as an ordinary call
    // when bound by the user).
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(def .. (fn [x] [:dotdot x])) (.. 1)"),
        "[:dotdot 1]"
    );
}

#[test]
fn cross_ns_construction() {
    // Dotted names resolve as namespace + name (no import table).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(ns foo-bar) (defrecord R2 [a]) (ns user) (:a (foo-bar.R2. 5))"
        ),
        "5"
    );
}

#[test]
fn cross_ns_hash_formula() {
    // The record-hash formula crosses namespaces with munging
    // (`foo-bar` → `foo_bar`, solved §L): `468488341`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(ns foo-bar) (defrecord R2 [a]) (ns user) (hash (foo-bar.R2. 5))"
        ),
        "468488341"
    );
}

#[test]
fn record_factory_cross_ns() {
    // Factories capture the descriptor: they build the defining
    // type from any namespace.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(ns foo) (defrecord R [a]) (ns user) (:a (foo/->R 9))"
        ),
        "9"
    );
}

// Bootstrap-to-app casts name the bootstrap loader first (the
// port's `cce_message` mirror of the JVM clause order).
fn cce_from_boot(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} is in module java.base of loader 'bootstrap'; {to} is in unnamed module of loader 'app')"
    )
}

// ---- hidden-native defensive errors (no oracle: direct misuse) ----

#[test]
fn stage1_native_arg_guards() {
    // Every `*_arg` fallback: wrong-typed direct native args fail
    // with the stock cast.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__deftype* 5 \"P\" [] {} [])"),
        cce_boot("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__deftype* \"ns\" \"P\" 5 {} [])"),
        cce_from_boot("java.lang.Long", "clojure.lang.IPersistentVector")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__deftype* \"ns\" \"P\" [] 5 [])"),
        cce_from_boot("java.lang.Long", "clojure.lang.IPersistentMap")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__deftype* \"ns\" \"P\" [] {} 5)"),
        cce_from_boot("java.lang.Long", "clojure.lang.IPersistentVector")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__proxy* 5 [] [] {})"),
        cce_from_boot("java.lang.Long", "clojure.lang.Symbol")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__proxy* 'Object 5 [] {})"),
        cce_from_boot("java.lang.Long", "clojure.lang.IPersistentVector")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__new* 5)"),
        cce_from_boot("java.lang.Long", "clojure.lang.Symbol")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__field* 5 6)"),
        cce_boot("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__meth* 5 6)"),
        cce_boot("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__record_map* 5 {})"),
        cce_from_boot("java.lang.Long", "clojure.lang.Symbol")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__interface* \"ns\" \"I\" {5 [2]})"
        ),
        cce_boot("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__interface* \"ns\" \"I\" {\"m\" [\"x\"]})"
        ),
        cce_boot("java.lang.String", "java.lang.Long")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__deftype* \"ns\" \"P\" [] {5 {1 (fn [this] 1)}} [])"
        ),
        cce_boot("java.lang.Long", "java.lang.String")
    );
}

#[test]
fn stage1_native_arity_guards() {
    // Bare hidden-native calls fail the stock arity.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__new*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__new*"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__meth*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__meth*"
    );
}

#[test]
fn stage1_define_type_guards() {
    // Bad field/interface shapes fail before any type is minted.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.core/__deftype* \"ns\" \"P\" '[a 5] {} [])"
        ),
        "java.lang.IllegalArgumentException | Field names must be simple symbols"
    );
    // An instance where an interface goes is not a type (also
    // reachable through the `deftype` macro over a var).
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord Point [x y]) (def p (->Point 1 2)) (deftype D [] p (m [this] 1))"
        ),
        "java.lang.IllegalArgumentException | Not a type: p"
    );
    // A constructed instance is not a type either.
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord Point [x y]) (clojure.core/__new* (->Point 1 2) 1 2)"
        ),
        "java.lang.IllegalArgumentException | Not a type"
    );
    // Interfaces have no constructors.
    assert_eq!(
        uncaught(&mut eval, "(definterface Adder (add [x y])) (Adder.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class user.Adder"
    );
}

#[test]
fn stage1_record_map_guards() {
    // `map->` targets must be the defining record.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord Point [x y]) (deftype D [a]) (clojure.core/__record_map* 'D {})"
        ),
        "java.lang.IllegalArgumentException | Not a record: D"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(deftype D [a]) (clojure.core/__record_map* D {})"
        ),
        "java.lang.IllegalArgumentException | Not a record: user.D"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(defrecord Point [x y]) (clojure.core/__record_map* (->Point 1 2) {})"
        ),
        "java.lang.IllegalArgumentException | Not a record"
    );
    // One level through `seq` still builds the record.
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (map->Point [[:x 1] [:y 2]])"
        ),
        "#user.Point{:x 1, :y 2}"
    );
}

#[test]
fn stage1_descriptor_level() {
    // Descriptors answer at the class level: dotted names resolve
    // anywhere, descriptors compare by identity, hash
    // deterministically, and reject collection casts.
    let mut eval = boot();
    // Bare dotted references do not resolve (constructors do —
    // there is no import table, so only the `Type.` dot form splits
    // namespace from name); the dotted registry path is covered by
    // the cross-namespace constructor tests.
    assert_eq!(
        uncaught(
            &mut eval,
            "(ns foo-bar) (defrecord R2 [a]) (ns user) (str foo-bar.R2)"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: foo-bar.R2 in this context"
    );
    assert_eq!(run(&mut eval, "(deftype Pair [a b]) (= Pair Pair)"), "true");
    assert_eq!(
        run(&mut eval, "(defrecord Point [x y]) (= Point (->Point 1 2))"),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (= (hash Point) (hash Point))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (conj Pair [:a 1])"),
        cce_same("user.Pair", "clojure.lang.IPersistentCollection")
    );
    // Methods do not resolve on descriptors.
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (.toString Pair)"),
        "java.lang.IllegalArgumentException | No matching field found: toString for class user.Pair"
    );
    // `meta` of a descriptor is nil (only instances carry it).
    assert_eq!(run(&mut eval, "(deftype Pair [a b]) (meta Pair)"), "nil");
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (binding [*print-meta* true] (pr-str Pair))"
        ),
        "\"user.Pair\""
    );
}

#[test]
fn stage1_descriptor_defined_methods_miss() {
    // Defined methods do not resolve on descriptors either: the
    // call misses with the field/method error, never invokes with
    // the descriptor as `this` (round-1 review M1).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(definterface I (m []) (n [x])) (deftype T [x] I (m [this] :called) (n [this y] y)) (.m T)"
        ),
        "java.lang.IllegalArgumentException | No matching field found: m for class user.T"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(definterface I (m []) (n [x])) (deftype T [x] I (m [this] :called) (n [this y] y)) (.n T 1)"
        ),
        "java.lang.IllegalArgumentException | No matching method n found taking 1 args for class user.T"
    );
    // Gen-class descriptors miss the same way (no prefix-fn
    // forwarding off a descriptor, even when the fn exists).
    assert_eq!(
        uncaught(
            &mut eval,
            "(gen-class :name user.Klass :prefix \"klass-\" :methods [[hello [] String]]) (defn klass-hello [this] \"yo\") (.hello Klass)"
        ),
        "java.lang.IllegalArgumentException | No matching field found: hello for class user.Klass"
    );
}

#[test]
fn stage1_symbol_named_calls() {
    // Field/method names also travel as symbols (the macros emit
    // strings, so only direct calls exercise this).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (clojure.core/__field* 'a (Pair. 10 20))"
        ),
        "10"
    );
    assert_eq!(
        run(
            &mut eval,
            "(deftype P [] Object (toString [this] \"x\")) (clojure.core/__meth* 'toString (P.))"
        ),
        "\"x\""
    );
    // The symbol path resolves records too.
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (clojure.core/__record_map* 'Point {:x 1 :y 2})"
        ),
        "#user.Point{:x 1, :y 2}"
    );
}

#[test]
fn stage1_descriptor_field_probe() {
    // A field-named zero-arg call on a descriptor falls through to
    // the field error (there is no instance to read).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (.a Pair)"),
        "java.lang.IllegalArgumentException | No matching field found: a for class user.Pair"
    );
    // Slash-qualified construction resolves without an import.
    assert_eq!(
        uncaught(&mut eval, "(definterface Adder (add [x y])) (user/Adder.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class user.Adder"
    );
    // Descriptors of different types are never `=`.
    assert_eq!(
        run(
            &mut eval,
            "(deftype Pair [a b]) (definterface Adder (add [x y])) (= Pair Adder)"
        ),
        "false"
    );
    // Records vectorize through their entries.
    assert_eq!(
        run(&mut eval, "(defrecord Point [x y]) (vec (->Point 1 2))"),
        "[[:x 1] [:y 2]]"
    );
    // Deftypes have no seq (into steps, like vec).
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (into [] (Pair. 1 2))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: user.Pair"
    );
    // Stepping through a lazy chain fails the same way.
    assert_eq!(
        uncaught(
            &mut eval,
            "(deftype Pair [a b]) (vec (map inc (Pair. 1 2)))"
        ),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: user.Pair"
    );
    // ... and no `hashCode` fallback either (records do).
    assert_eq!(
        uncaught(&mut eval, "(deftype Pair [a b]) (.hashCode (Pair. 1 2))"),
        "java.lang.IllegalArgumentException | No matching field found: hashCode for class user.Pair"
    );
}

#[test]
fn stage1_dot_fallthrough() {
    // A malformed `.-` form is not intercepted (the symbol keeps its
    // unresolvable error).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(.-a)"),
        "java.lang.RuntimeException | Unable to resolve symbol: .-a in this context"
    );
}

#[test]
fn stage1_dot_field_call_shapes() {
    // Zero-arg dot calls read fields before methods.
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(deftype Pair [a b]) (.a (Pair. 10 20))"),
        "10"
    );
    // A defined zero-arg method dispatches through interop too.
    assert_eq!(
        run(
            &mut eval,
            "(deftype P [] Object (toString [this] \"x\")) (.toString (P.))"
        ),
        "\"x\""
    );
}

#[test]
fn stage1_genclass_prefix_unbound() {
    // A declared-but-unbound prefix fn is the resolve miss (same
    // text as the missing fn).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(gen-class :name K :methods [[hi [] String]]) (def K-hi) (.hi (K.))"
        ),
        "java.lang.RuntimeException | Unable to resolve symbol: user/K-hi in this context"
    );
    // `:implements` entries resolve like reify interfaces.
    assert_eq!(
        run(
            &mut eval,
            "(definterface Adder (add [x y])) (gen-class :name G :implements [Adder] :methods [[add [x] String]]) (str G)"
        ),
        "\"user.G\""
    );
}

#[test]
fn stage1_deftype_meta_rejected() {
    // Deftype instances are not `IObj` (records are).
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(deftype Pair [a b]) (meta (Pair. 1 2))"),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(deftype Pair [a b]) (with-meta (Pair. 1 2) {:a 1})"
        ),
        cce_same("user.Pair", "clojure.lang.IObj")
    );
}

#[test]
fn stage1_seqable_and_tostring_throw() {
    // Records seq; deftypes do not.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord Point [x y]) (seqable? (->Point 1 2))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(deftype Pair [a b]) (seqable? (Pair. 1 2))"),
        "false"
    );
    // A throwing `toString` propagates through `str`.
    assert_eq!(
        uncaught(
            &mut eval,
            "(deftype P [] Object (toString [this] (/ 1 0))) (str (P.))"
        ),
        "java.lang.ArithmeticException | Divide by zero"
    );
}
