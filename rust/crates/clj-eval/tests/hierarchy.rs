// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #57 tests: every pin in
//! `docs/zero-oos-s13-hierarchy/probes/*.out` replayed against the
//! port, plus guards, `HostDef` relations, unknown-class names,
//! and garbage relation values. Class pins name classes by string
//! (the port has no class literals); multi-element sets assert
//! through sorted views (set print order is hash order).

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

// ---- A: make-hierarchy + explicit derive ----

#[test]
fn hier_make_and_explicit_derive() {
    // Oracle `h-make`, `h-derive3`, `h-parents3`,
    // `h-ancestors3`, `h-desc3`, `h-empty` (verbatim —
    // insertion order and namespace-map printing match).
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(make-hierarchy)"),
        "{:parents {}, :descendants {}, :ancestors {}}"
    );
    assert_eq!(
        run(&mut eval, "(derive (make-hierarchy) ::hcat ::hpet)"),
        "{:parents #:user{:hcat #{:user/hpet}}, :ancestors #:user{:hcat #{:user/hpet}}, :descendants #:user{:hpet #{:user/hcat}}}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(parents (derive (make-hierarchy) ::hcat ::hpet) ::hcat)"
        ),
        "#{:user/hpet}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(ancestors (derive (make-hierarchy) ::hcat ::hpet) ::hcat)"
        ),
        "#{:user/hpet}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(descendants (derive (make-hierarchy) ::hcat ::hpet) ::hpet)"
        ),
        "#{:user/hcat}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(list (parents (make-hierarchy) ::nope) (ancestors (make-hierarchy) ::nope) (descendants (make-hierarchy) ::nope))"
        ),
        "(nil nil nil)"
    );
}

#[test]
fn hier_chains_and_diamonds() {
    // Oracle `h-chain`, `h-idem`, `h-diamond`
    // (multi-element sets through sorted views).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::ha ::hb) (derive ::hb ::hc))] [(parents h ::ha) (sort (ancestors h ::ha)) (sort (descendants h ::hc)) (ancestors h ::hb)])"
        ),
        "[#{:user/hb} (:user/hb :user/hc) (:user/ha :user/hb) #{:user/hc}]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) ::hia ::hib)] (= h (derive h ::hia ::hib)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::hd ::hb) (derive ::hd ::hc) (derive ::hb ::ha) (derive ::hc ::ha))] [(sort (ancestors h ::hd)) (sort (descendants h ::ha))])"
        ),
        "[(:user/ha :user/hb :user/hc) (:user/hb :user/hc :user/hd)]"
    );
}

// ---- B: derive errors ----

#[test]
fn hier_derive_errors() {
    // Oracle `h-self`, `h-tagstr`, `h-tagnil`,
    // `h-parent-str`, `h-parent-num` (verbatim assert texts).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) ::hself ::hself)"),
        "java.lang.AssertionError | Assert failed: (not= tag parent)"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) \"hstr\" ::hstrp)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (instance? clojure.lang.Named tag))"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) nil ::hnilp)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (instance? clojure.lang.Named tag))"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) ::hps2 \"hpsstr\")"),
        "java.lang.AssertionError | Assert failed: (instance? clojure.lang.Named parent)"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) ::hpn2 5)"),
        "java.lang.AssertionError | Assert failed: (instance? clojure.lang.Named parent)"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) ::hcp String)"),
        "java.lang.AssertionError | Assert failed: (instance? clojure.lang.Named parent)"
    );
}

#[test]
fn hier_derive_tag_shapes() {
    // Oracle `h-ns-ok`, `h-unnamed-ok`, `h-sym-ok`,
    // `h-mixed-ok` (explicit hierarchies need no namespaces).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(parents (derive (make-hierarchy) ::hps ::hpsnok) ::hps)"
        ),
        "#{:user/hpsnok}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(parents (derive (make-hierarchy) :hplain :hplainp) :hplain)"
        ),
        "#{:hplainp}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(parents (derive (make-hierarchy) 'hsym 'hsymp) 'hsym)"
        ),
        "#{hsymp}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(parents (derive (make-hierarchy) 'hmix ::hmixp) 'hmix)"
        ),
        "#{:user/hmixp}"
    );
}

#[test]
fn hier_derive_relation_errors() {
    // Oracle `h-already`, `h-cycle`, `h-cycle3` (verbatim
    // `print-str`-rendered texts).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::halc ::halb) (derive ::halb ::hal))] (derive h ::halc ::hal))"
        ),
        "java.lang.Exception | :user/halc already has :user/hal as ancestor"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [h (derive (make-hierarchy) ::hcyc ::hcycp)] (derive h ::hcycp ::hcyc))"
        ),
        "java.lang.Exception | Cyclic derivation: :user/hcyc has :user/hcycp as ancestor"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::hc3a ::hc3b) (derive ::hc3b ::hc3c))] (derive h ::hc3c ::hc3a))"
        ),
        "java.lang.Exception | Cyclic derivation: :user/hc3a has :user/hc3c as ancestor"
    );
}

#[test]
fn hier_derive_arity() {
    // Oracle `h-arity1`, `h-arity0` (the house arity text
    // already matches the JVM shape).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(derive ::haronly)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/derive"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/derive"
    );
}

// ---- C: underive ----

#[test]
fn hier_underive() {
    // Oracle `h-under`, `h-under-miss`, `h-under-trans`,
    // `h-under-global`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::hua ::hub) (derive ::hub ::huc)) u (underive h ::hua ::hub)] [(parents u ::hua) (ancestors u ::hua) (parents u ::hub) (descendants u ::huc)])"
        ),
        "[nil nil #{:user/huc} #{:user/hub}]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) ::hum ::hump)] (= h (underive h ::hum ::huother)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::hut ::hum2) (derive ::hum2 ::hup2)) u (underive h ::hum2 ::hup2)] [(ancestors u ::hut) (descendants u ::hup2)])"
        ),
        "[#{:user/hum2} nil]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (derive ::hug ::hugp) (underive ::hug ::hugp))"
        ),
        "nil"
    );
    // A rebuilt hierarchy prints in `make-hierarchy` order
    // (probe `n-under-partial`).
    assert_eq!(
        run(&mut eval, "(underive {:parents {::ua #{::ub}}} ::ua ::ub)"),
        "{:parents {}, :descendants {}, :ancestors {}}"
    );
}

// ---- D: class views ----

#[test]
fn hier_class_views() {
    // Oracle `h-p-string`, `h-a-string`, `h-d-string`,
    // `h-p-kw`, `h-p-vec`, `h-p-amap`, `h-p-hset`,
    // `h-p-union`, `h-a-union` — names, not objects: the port
    // answers class-name strings (no `class `/`interface `
    // prefixes) in port sort order.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String 'java.lang.Object)");
    assert_eq!(
        run(&mut eval, "(sort (map str (parents String)))"),
        "(\"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (ancestors String)))"),
        "(\"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    assert_eq!(
        uncaught(&mut eval, "(descendants String)"),
        "java.lang.UnsupportedOperationException | Can't get descendants of classes"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.Keyword\")))"
        ),
        "(\"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.Named\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Object\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.PersistentVector\")))"
        ),
        "(\"clojure.lang.APersistentVector\" \"clojure.lang.IDrop\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IKVReduce\" \"clojure.lang.IObj\" \"clojure.lang.IReduce\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.PersistentArrayMap\")))"
        ),
        "(\"clojure.lang.APersistentMap\" \"clojure.lang.IDrop\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IKVReduce\" \"clojure.lang.IMapIterable\" \"clojure.lang.IObj\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.PersistentHashSet\")))"
        ),
        "(\"clojure.lang.APersistentSet\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IObj\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) String ::hpu)] (sort (map str (parents h String))))"
        ),
        "(\":user/hpu\" \"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) Object ::hau)] (sort (map str (ancestors h String))))"
        ),
        "(\":user/hau\" \"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    // Oracle `h-classtag` (same union, sorted port order).
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents (derive (make-hierarchy) String ::hcls) String)))"
        ),
        "(\":user/hcls\" \"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    // A class tag's own map entry joins its ancestors.
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) String ::hox)] (sort (map str (ancestors h String))))"
        ),
        "(\":user/hox\" \"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
}

// ---- E: global arities ----

#[test]
fn hier_global_arities() {
    // Oracle `h-gderive`, `h-gisa`, `h-gparents`, `h-ganc`,
    // `h-gdesc`, `h-gparent-plain`, `h-gtag-plain`,
    // `h-gtag-class`, `h-private`.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(run(&mut eval, "(derive ::hgd ::hgdp)"), "nil");
    assert_eq!(run(&mut eval, "(isa? ::hgd ::hgdp)"), "true");
    assert_eq!(run(&mut eval, "(parents ::hgd)"), "#{:user/hgdp}");
    assert_eq!(run(&mut eval, "(ancestors ::hgd)"), "#{:user/hgdp}");
    assert_eq!(run(&mut eval, "(descendants ::hgdp)"), "#{:user/hgd}");
    assert_eq!(
        uncaught(&mut eval, "(derive ::hgpp :hgplain)"),
        "java.lang.AssertionError | Assert failed: (namespace parent)"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive :hgtplain ::hgtp)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (and (instance? clojure.lang.Named tag) (namespace tag)))"
    );
    // The parent runs through `namespace` semantics (hinted
    // `Named` on the JVM — derived from the `namespace` def,
    // same texts as the port's `namespace` native).
    assert_eq!(
        uncaught(&mut eval, "(derive ::hg5 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Named (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Named is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive ::hg6 \"hgstr\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Named (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Named is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive ::hg7 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (derive String ::hgcls) (isa? String ::hgcls))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(:private (meta #'clojure.core/global-hierarchy))"
        ),
        "true"
    );
    // Symbols with namespaces ride the global asserts too.
    assert_eq!(
        run(
            &mut eval,
            "(do (derive 'hgsym/hgt 'hgpsym/hgp) (isa? 'hgsym/hgt 'hgpsym/hgp))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive 'hgtplain2 ::hgtp2)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (and (instance? clojure.lang.Named tag) (namespace tag)))"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive 5 ::hgtp3)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (and (instance? clojure.lang.Named tag) (namespace tag)))"
    );
}

// ---- F: isa? ----

#[test]
fn hier_isa_basics() {
    // Oracle `h-isa-eq`, `h-isa-ne`, `h-isa-class`,
    // `h-isa-vec`, `h-isa-supermap`, `h-isa-explicit`
    // (classes by name string).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Object 'java.lang.Long)",
    );
    assert_eq!(
        run(
            &mut eval,
            "(list (isa? ::hgd ::hgd) (isa? 1 1) (isa? :hx :hx) (isa? nil nil))"
        ),
        "(true true true true)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(list (isa? ::hgd ::hgdnope) (isa? 1 2) (isa? nil ::hx) (isa? \"hs\" \"hsx\"))"
        ),
        "(false false false false)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(list (isa? String Object) (isa? String \"java.lang.CharSequence\") (isa? Long \"java.lang.Number\") (isa? Long Object) (isa? String Long) (isa? \"java.lang.CharSequence\" Object) (isa? Object Object))"
        ),
        "(true true true true false true true)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (-> (make-hierarchy) (derive ::hva ::hvp) (derive ::hvb ::hvp))] [(isa? h [::hva ::hvb] [::hvp ::hvp]) (isa? h [::hva] [::hvp ::hvp]) (isa? h ::hva [::hvp]) (isa? h [::hva ::hvb] [::hvp ::hvx])])"
        ),
        "[true false false false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (derive Object ::hgtop) (list (isa? String ::hgtop) (isa? \"hsv\" ::hgtop) (isa? Object ::hgtop)))"
        ),
        "(true false true)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [h (derive (make-hierarchy) ::hie ::hiep)] (list (isa? h ::hie ::hiep) (isa? ::hie ::hiep)))"
        ),
        "(true false)"
    );
    // A class child with no map anywhere answers false
    // (supers loop misses throughout).
    assert_eq!(run(&mut eval, "(isa? String ::hnope1)"), "false");
}

#[test]
fn hier_isa_mapentry_and_nested() {
    // Map entries are 2-vectors for `isa?` (a `MapEntry` is
    // an `APersistentVector`, probe `l-mapentry`); recursion
    // keeps the class arm (elements may be classes).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String 'java.lang.Object)");
    assert_eq!(run(&mut eval, "(isa? (first {:a 1}) [:a 1])"), "true");
    assert_eq!(run(&mut eval, "(isa? [:a 1] (first {:a 1}))"), "true");
    assert_eq!(run(&mut eval, "(isa? (first {:a 1}) [:a])"), "false");
    assert_eq!(
        run(&mut eval, "(isa? (make-hierarchy) [String] [Object])"),
        "true"
    );
}

// ---- G: garbage-h tolerance ----

#[test]
fn hier_garbage_h() {
    // Oracle `h-badh-isa`, `h-badh-derive`, `h-badh-views`,
    // `h-badh-under`: mutating paths NPE with the call-site
    // text, views read nil.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(isa? 5 ::hbh ::hbp)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\""
    );
    assert_eq!(
        uncaught(&mut eval, "(derive 5 ::hbdh ::hbdp)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"tp\" is null"
    );
    assert_eq!(
        run(
            &mut eval,
            "(list (parents 5 ::hbh2) (ancestors 5 ::hbh2) (descendants 5 ::hbh2))"
        ),
        "(nil nil nil)"
    );
    assert_eq!(
        uncaught(&mut eval, "(underive 5 ::hbuh ::hbup)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"parentMap\" is null"
    );
    // Oracle `h-clobber`: a clobbered global root NPEs the
    // same way (the engine sees garbage `h`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (alter-var-root #'clojure.core/global-hierarchy (fn [_] 5)) (derive ::hcl ::hclp))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"tp\" is null"
    );
}

// ---- dispatch A: best-method walk ----

#[test]
fn hier_dispatch_walk() {
    // Oracle `x-basic`, `x-specific`, `x-amb`, `x-trans`,
    // `x-def-hier`, `x-vec`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdbas :k) (defmethod hdbas ::xshape [m] :shape) (derive ::xsq ::xshape) (hdbas {:k ::xsq}))"
        ),
        ":shape"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdspc :k) (defmethod hdspc ::xshp [m] :shape) (defmethod hdspc ::xsqr [m] :square) (derive ::xsqr ::xshp) (hdspc {:k ::xsqr}))"
        ),
        ":square"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdamb :k) (defmethod hdamb ::xfly [m] :fly) (defmethod hdamb ::xswim [m] :swim) (derive ::xduck ::xfly) (derive ::xduck ::xswim) (hdamb {:k ::xduck}))"
        ),
        "java.lang.IllegalArgumentException | Multiple methods in multimethod 'hdamb' match dispatch value: :user/xduck -> :user/xswim and :user/xfly, and neither is preferred"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdtrn :k) (defmethod hdtrn ::xtop [m] :top) (derive ::xmid ::xtop) (derive ::xleaf ::xmid) (hdtrn {:k ::xleaf}))"
        ),
        ":top"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hddef :k) (defmethod hddef ::xdshp [m] :shape) (defmethod hddef :default [m] :dflt) (derive ::xdsq ::xdshp) (hddef {:k ::xdsq}))"
        ),
        ":shape"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdvec (fn [x y] [(:a x) (:b y)])) (defmethod hdvec [::xvsh ::xvot] [x y] :hit) (derive ::xvsq ::xvsh) (hdvec {:a ::xvsq} {:b ::xvot}))"
        ),
        ":hit"
    );
    // Reverse insertion: the best holds against a later,
    // less specific challenger.
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrv :k) (defmethod hdrv ::xrvq [m] :square) (defmethod hdrv ::xrvh [m] :shape) (derive ::xrvq ::xrvh) (hdrv {:k ::xrvq}))"
        ),
        ":square"
    );
}

#[test]
fn hier_ambiguity_order() {
    // Oracle `n-amb-order`: challenger first, best second —
    // and the port's table order is insertion order, so the
    // method keys print in that order too.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdmo :k) (defmethod hdmo ::moa [m] :a) (defmethod hdmo ::mob [m] :b) (derive ::moc ::moa) (derive ::moc ::mob) [(map first (seq (methods hdmo))) (try (hdmo {:k ::moc}) (catch IllegalArgumentException e (ex-message e)))])"
        ),
        "[(:user/moa :user/mob) \"Multiple methods in multimethod 'hdmo' match dispatch value: :user/moc -> :user/mob and :user/moa, and neither is preferred\"]"
    );
}

// ---- dispatch B: prefers at dispatch ----

#[test]
fn hier_prefer_dispatch() {
    // Oracle `x-prefer`, `x-prefer-flip`, `x-prefers-view`,
    // `x-prefer-conflict`, `x-prefer-self`, `x-prefer-bare`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdprf :k) (defmethod hdprf ::xpfl [m] :fly) (defmethod hdprf ::xpsw [m] :swim) (derive ::xpdk ::xpfl) (derive ::xpdk ::xpsw) (prefer-method hdprf ::xpsw ::xpfl) (hdprf {:k ::xpdk}))"
        ),
        ":swim"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdprg :k) (defmethod hdprg ::xqfl [m] :fly) (defmethod hdprg ::xqsw [m] :swim) (derive ::xqdk ::xqfl) (derive ::xqdk ::xqsw) (prefer-method hdprg ::xqfl ::xqsw) (hdprg {:k ::xqdk}))"
        ),
        ":fly"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdpv :k) (defmethod hdpv :a [m] :a) (prefer-method hdpv :a :b) (prefers hdpv))"
        ),
        "{:a #{:b}}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdpc :k) (defmethod hdpc :a [m] :a) (defmethod hdpc :b [m] :b) (prefer-method hdpc :a :b) (prefer-method hdpc :b :a))"
        ),
        "java.lang.IllegalStateException | Preference conflict in multimethod 'hdpc': :a is already preferred to :b"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdps :k) (defmethod hdps :a [m] :a) (prefer-method hdps :a :a) (prefers hdps))"
        ),
        "{:a #{:a}}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdpb :k) (prefer-method hdpb :a :b) (prefers hdpb))"
        ),
        "{:a #{:b}}"
    );
    // Oracle `n-prefer-trans`: transitive cycles stay allowed
    // (the check is direct-edge only).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdpt :k) (defmethod hdpt :a [m] :a) (prefer-method hdpt :a :b) (prefer-method hdpt :b :c) (prefer-method hdpt :c :a) (sort (keys (prefers hdpt))))"
        ),
        "(:a :b :c)"
    );
    // `prefers` and `prefer-method` reject non-multifns with
    // the sibling cast.
    assert_eq!(
        uncaught(&mut eval, "(prefers 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (def npv 5) (prefer-method npv :a :b))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
}

// ---- dispatch C: get-method ----

#[test]
fn hier_get_method() {
    // Oracle `x-get-hier`, `x-get-amb`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdget :k) (defmethod hdget ::xgsh [m] :shape) (derive ::xgsq ::xgsh) ((get-method hdget ::xgsq) {:k 1}))"
        ),
        ":shape"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdga :k) (defmethod hdga ::xhfl [m] :fly) (defmethod hdga ::xhsw [m] :swim) (derive ::xhdk ::xhfl) (derive ::xhdk ::xhsw) (get-method hdga ::xhdk))"
        ),
        "java.lang.IllegalArgumentException | Multiple methods in multimethod 'hdga' match dispatch value: :user/xhdk -> :user/xhsw and :user/xhfl, and neither is preferred"
    );
    // A hierarchy miss with a garbage hierarchy NPEs (the
    // walk consults `isa?`, probe `n-badval-miss` shape).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdgbb :k :hierarchy (atom 5)) (defmethod hdgbb :a [m] :a) (get-method hdgbb :b))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\""
    );
}

// ---- dispatch D: class dispatch ----

#[test]
fn hier_class_dispatch() {
    // Oracle `x-class`, `x-class-spec` (dispatch fns answer
    // class-name strings; methods hang off the class-name
    // vars — the port has no `class` fn).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String 'java.lang.Object)");
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdcls (fn [x] \"java.lang.String\")) (defmethod hdcls Object [x] :obj) (hdcls \"hi\"))"
        ),
        ":obj"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdcs2 (fn [x] (if (string? x) \"java.lang.String\" \"java.lang.Long\"))) (defmethod hdcs2 Object [x] :obj) (defmethod hdcs2 String [x] :str) (list (hdcs2 \"hi\") (hdcs2 3)))"
        ),
        "(:str :obj)"
    );
}

// ---- dispatch E: cache + remove interplay ----

#[test]
fn hier_cache_and_remove() {
    // Oracle `x-cache-inv`, `x-remove-hier`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdcin :k :default :xnone) (defmethod hdcin :xnone [m] :dflt) (defmethod hdcin ::xcinv [m] :hit) (derive ::xcinvc ::xleft) (list (hdcin {:k ::xcinvc}) (do (derive ::xcinvc ::xcinv) (underive ::xcinvc ::xleft) (hdcin {:k ::xcinvc}))))"
        ),
        "(:dflt :hit)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrmh :k) (defmethod hdrmh ::xrmp [m] :shape) (defmethod hdrmh :default [m] :dflt) (derive ::xrmc ::xrmp) (remove-method hdrmh ::xrmp) (hdrmh {:k ::xrmc}))"
        ),
        ":dflt"
    );
}

// ---- dispatch F: remove-all-methods ----

#[test]
fn hier_remove_all() {
    // Oracle `x-rmall`, `x-rmall-call`, `x-rmall-bad`,
    // `x-rmall-reuse`.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrma :k) (defmethod hdrma :a [m] :a) (prefer-method hdrma :a :b) (remove-all-methods hdrma) (list (methods hdrma) (prefers hdrma)))"
        ),
        "({} {})"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdrmb :k) (defmethod hdrmb :a [m] :a) (remove-all-methods hdrmb) (hdrmb {:k :a}))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'hdrmb' for dispatch value: :a"
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-all-methods 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrmc :k) (defmethod hdrmc :a [m] :a) (remove-all-methods hdrmc) (defmethod hdrmc :b [m] :b) (hdrmc {:k :b}))"
        ),
        ":b"
    );
    // Removal answers the multifn (still an `IFn`) and rides
    // the locked var path (a quoted symbol resolves; a
    // non-multifn var fails the cast).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrmd :k) (defmethod hdrmd :default [m] :dflt) (ifn? (remove-all-methods hdrmd)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdrms :k) (defmethod hdrms :a [m] :a) (clojure.core/__remove_all_methods* 'hdrms) (methods hdrms))"
        ),
        "{}"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (def nrm 5) (remove-all-methods nrm))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.MultiFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.MultiFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__remove_all_methods*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__remove_all_methods*"
    );
}

// ---- dispatch G: :hierarchy ----

#[test]
fn hier_explicit_hierarchy() {
    // Oracle `x-atom-miss`, `x-atom-live`, `x-hier-var`,
    // `x-hier-iso`, `x-hier-map`, `x-hier-5`, `x-hier-nil`,
    // `x-hier-badval`, `n-badval-miss`.
    let mut eval = boot();
    eval.eval_source(
        "(def hatm (atom (make-hierarchy)))
         (defmulti hamlive :k :hierarchy hatm)
         (defmethod hamlive ::xshape [m] :xshape)",
    )
    .unwrap();
    assert_eq!(
        uncaught(&mut eval, "(hamlive {:k ::xx})"),
        "java.lang.IllegalArgumentException | No method in multimethod 'hamlive' for dispatch value: :user/xx"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (swap! hatm derive ::xx ::xshape) (hamlive {:k ::xx}))"
        ),
        ":xshape"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdvg :k :hierarchy #'clojure.core/global-hierarchy) (defmethod hdvg ::xxvg [m] :hit) (derive ::xxvgc ::xxvg) (hdvg {:k ::xxvgc}))"
        ),
        ":hit"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (def hh (atom (make-hierarchy))) (defmulti hdiso :k :hierarchy hh) (defmethod hdiso ::xiso [m] :hit) (derive ::xisoc ::xiso) (try (hdiso {:k ::xisoc}) (catch IllegalArgumentException e :no-match)))"
        ),
        ":no-match"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdmm :k :hierarchy {}) (hdmm {:k :a}))"
        ),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class clojure.lang.IRef (clojure.lang.PersistentArrayMap and clojure.lang.IRef are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdm5 :k :hierarchy 5) (hdm5 {:k :a}))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IRef (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IRef is in unnamed module of loader 'app')"
    );
    // A nil hierarchy NPEs at the call with the house
    // deref-nil text (the oracle's names `this.hierarchy`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdmn :k :hierarchy nil) (hdmn {:k :a}))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.util.concurrent.Future.get()\" because \"fut\" is null"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdmbv :k :hierarchy (atom 5)) (defmethod hdmbv :a [m] :a) (hdmbv {:k :a}))"
        ),
        ":a"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defmulti hdbm2 :k :hierarchy (atom 5)) (defmethod hdbm2 :a [m] :a) (hdbm2 {:k :b}))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\""
    );
    // Agents dereference too (read path only — no sends).
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdag :k :hierarchy (agent (derive (make-hierarchy) ::xagc ::xag))) (defmethod hdag ::xag [m] :hit) (hdag {:k ::xagc}))"
        ),
        ":hit"
    );
    // The option value evaluates, combines with `:default`,
    // and last wins.
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti hdhe :k :default :xnil :hierarchy (atom (derive (make-hierarchy) ::xhec ::xhe))) (defmethod hdhe ::xhe [m] :hit) (defmethod hdhe :xnil [m] :dflt) [(hdhe {:k ::xhec}) (hdhe {:k :other})])"
        ),
        "[:hit :dflt]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (def hfirst (atom (make-hierarchy))) (def hsecond (atom (derive (make-hierarchy) ::xwc ::xw))) (defmulti hdlw :k :hierarchy hfirst :hierarchy hsecond) (defmethod hdlw ::xw [m] :hit) (hdlw {:k ::xwc}))"
        ),
        ":hit"
    );
}

// ---- follow-ups: partial hierarchies + arm order ----

#[test]
fn hier_partial_hierarchies() {
    // Oracle `n-isa-partial`, `n-derive-partial`,
    // `n-derive-partial2`, `n-derive-edgeonly`,
    // `n-derive-badparents`, `n-isa-badanc`, `n-par-partial`,
    // `n-par-badh-class`, `n-anc-partial`, `n-desc-partial`
    // (`n-nilret-call` / `n-ancnil-call` stay oracle-only:
    // general nil-invocation is pre-existing port behavior
    // with house text, outside this slice).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(
        run(&mut eval, "(isa? {:ancestors {::pa #{::pb}}} ::pa ::pb)"),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive {:parents {}} ::da ::db)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"ta\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive {:parents {} :ancestors {}} ::dx ::dy)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"sources\" is null"
    );
    assert_eq!(
        run(&mut eval, "(derive {:parents {::ea #{::eb}}} ::ea ::eb)"),
        "{:parents #:user{:ea #{:user/eb}}}"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive {:parents 5} ::fa ::fb)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(isa? {:ancestors 5} ::ga ::gb)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        run(&mut eval, "(parents {:parents {::qa #{::qb}}} ::qa)"),
        "#{:user/qb}"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (parents 5 String)))"),
        "(\"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")"
    );
    assert_eq!(
        run(&mut eval, "(ancestors {:ancestors {::ra #{::rb}}} ::ra)"),
        "#{:user/rb}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(descendants {:descendants {::sb #{::sa}}} ::sb)"
        ),
        "#{:user/sa}"
    );
    // A missing `:parents` NPEs before any other touch.
    assert_eq!(
        uncaught(&mut eval, "(derive {} ::ha ::hb)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"tp\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(underive {} ::ua ::ub)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"parentMap\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(underive {:parents 5} ::ua ::ub)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
}

#[test]
fn hier_isa_arm_order() {
    // Oracle `n-isa-eq-badh`, `n-isa-class-badh`,
    // `n-isa-vec-badh`, `n-isa-ne-badh`: `=` and the class
    // arm run before the ancestors touch.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String 'java.lang.Object)");
    assert_eq!(run(&mut eval, "(isa? 5 ::ox ::ox)"), "true");
    assert_eq!(run(&mut eval, "(isa? 5 String Object)"), "true");
    assert_eq!(run(&mut eval, "(isa? 5 [::ova] [::ova])"), "true");
    assert_eq!(
        uncaught(&mut eval, "(isa? 5 ::ox ::oy)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\""
    );
}

#[test]
fn hier_interfaces() {
    // Oracle `n-iface-obj`, `n-iface-iface`, `n-obj-iface`,
    // `n-par-ifn`, `n-range`, `n-unbound`: interfaces are
    // assignable to `Object`; bare interfaces carry no
    // ancestry.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.Object)");
    assert_eq!(run(&mut eval, "(isa? \"clojure.lang.IFn\" Object)"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(isa? \"clojure.lang.ISeq\" \"clojure.lang.Seqable\")"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(isa? Object \"clojure.lang.IFn\")"),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (parents \"clojure.lang.IFn\")))"),
        "(\"java.lang.Runnable\" \"java.util.concurrent.Callable\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.Range\")))"
        ),
        "(\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IReduce\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(sort (map str (parents \"clojure.lang.Var$Unbound\")))"
        ),
        "(\"clojure.lang.AFn\")"
    );
    // Bare interfaces: `isa?` to `Object` holds, views read
    // nil, descendants still throws.
    assert_eq!(
        run(
            &mut eval,
            "(list (isa? \"java.io.Serializable\" Object) (parents \"java.io.Serializable\") (ancestors \"java.io.Serializable\"))"
        ),
        "(true nil nil)"
    );
    assert_eq!(
        uncaught(&mut eval, "(descendants \"java.io.Serializable\")"),
        "java.lang.UnsupportedOperationException | Can't get descendants of classes"
    );
}

// ---- garbage relation values (derived rules) ----

#[test]
fn hier_garbage_relation_values() {
    // Non-set relation values: `isa?` membership reads them
    // as absent (`contains?` rule); every build/view path
    // fails the `IPersistentSet` cast.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.String)");
    let set_cce = "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentSet (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentSet is in unnamed module of loader 'app')";
    assert_eq!(
        run(&mut eval, "(isa? {:ancestors {::ja 5}} ::ja ::jb)"),
        "false"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(derive {:parents {::ka 5} :ancestors {} :descendants {}} ::ka ::kb)"
        ),
        set_cce
    );
    assert_eq!(
        uncaught(&mut eval, "(parents {:parents {::la 5}} ::la)"),
        set_cce
    );
    // A non-map relation value reads nil (JVM `get`
    // tolerates, probe `h-badh-views` shape).
    assert_eq!(run(&mut eval, "(parents {:parents 5} ::qa)"), "nil");
    assert_eq!(run(&mut eval, "(ancestors {:ancestors 5} ::qa)"), "nil");
    assert_eq!(
        uncaught(
            &mut eval,
            "(ancestors {:ancestors {\"java.lang.Object\" 5}} String)"
        ),
        set_cce
    );
    assert_eq!(run(&mut eval, "(parents {:parents nil} ::ma)"), "nil");
    assert_eq!(
        run(&mut eval, "(parents {:parents {::na nil}} ::na)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(parents {:parents {::oa #{}}} ::oa)"),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(derive {:parents {::pa nil} :ancestors {} :descendants {}} ::pa ::pb)"
        ),
        "{:parents #:user{:pa #{:user/pb}}, :ancestors #:user{:pa #{:user/pb}}, :descendants #:user{:pb #{:user/pa}}}"
    );
    // `underive` replays surviving edges through `derive`,
    // so garbage tags fail the replay assert.
    assert_eq!(
        uncaught(
            &mut eval,
            "(underive {:parents {5 #{::qb} 6 #{::qc}}} 5 ::qb)"
        ),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (instance? clojure.lang.Named tag))"
    );
}

// ---- HostDef relations ----

#[test]
fn hier_hostdef_relations() {
    // Port-defined classes: types claim `Object` plus their
    // interfaces, interfaces claim nothing; descriptors fold
    // to their name string at the hierarchy boundary.
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.Object)");
    eval.eval_source("(deftype HPet [n]) (definterface HSwim (swim [x])) (deftype HDuck [n] HSwim (swim [this x] x))")
        .unwrap();
    assert_eq!(run(&mut eval, "(isa? HPet Object)"), "true");
    assert_eq!(
        run(&mut eval, "(sort (parents HPet))"),
        "(\"java.lang.Object\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (ancestors HPet))"),
        "(\"java.lang.Object\")"
    );
    assert_eq!(run(&mut eval, "(isa? HDuck HSwim)"), "true");
    assert_eq!(
        run(&mut eval, "(sort (parents HDuck))"),
        "(\"java.lang.Object\" \"user.HSwim\")"
    );
    assert_eq!(
        uncaught(&mut eval, "(descendants HPet)"),
        "java.lang.UnsupportedOperationException | Can't get descendants of classes"
    );
    // Interfaces are assignable to `Object` but parentless.
    assert_eq!(run(&mut eval, "(isa? HSwim Object)"), "true");
    assert_eq!(run(&mut eval, "(parents HSwim)"), "nil");
    assert_eq!(run(&mut eval, "(ancestors HSwim)"), "nil");
    // Instances are never classes.
    assert_eq!(run(&mut eval, "(isa? (HPet. 1) Object)"), "false");
    assert_eq!(run(&mut eval, "(parents (HPet. 1))"), "nil");
    // Descriptors derive by name: the entry stores under the
    // name string, so both spellings find it (the descriptor
    // view keeps its `Object` base).
    assert_eq!(
        run(
            &mut eval,
            "(do (derive HPet ::hpet) (list (isa? HPet ::hpet) (isa? \"user.HPet\" ::hpet) (count (parents HPet)) (contains? (parents HPet) ::hpet) (parents \"user.HPet\")))"
        ),
        "(true true 2 true #{:user/hpet})"
    );
    // Descriptors compare by identity, so same-name defs from
    // a re-definition still unify by name in `isa?`.
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype HRD [x]) (def hrd1 HRD) (deftype HRD [x]) (list (= hrd1 HRD) (isa? hrd1 HRD)))"
        ),
        "(false true)"
    );
}

// ---- unknown class names ----

#[test]
fn hier_unknown_class_names() {
    // Never-real names are not classes (the S13 lattice is
    // fixed; S16 extends it without touching these pins).
    let mut eval = boot();
    // S16 merge: the class-name string stubs are retired;
    // import the classes (S13 reads them by name).
    run(&mut eval, "(import 'java.lang.Object)");
    assert_eq!(
        run(
            &mut eval,
            "(list (isa? \"Nope\" Object) (isa? Object \"Nope\") (isa? \"Nope\" \"Nope\") (parents \"Nope\") (ancestors \"Nope\") (descendants \"Nope\"))"
        ),
        "(false false true nil nil nil)"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) \"Nope\" ::nx)"),
        "java.lang.AssertionError | Assert failed: (or (class? tag) (instance? clojure.lang.Named tag))"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive (make-hierarchy) ::nx \"Nope\")"),
        "java.lang.AssertionError | Assert failed: (instance? clojure.lang.Named parent)"
    );
}

// ---- global var missing ----

#[test]
fn hier_global_unmapped() {
    // A user-unmapped global hierarchy fails resolution on
    // every short arity (fresh evaluator — the var is gone
    // for good here).
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(ns-unmap 'clojure.core 'global-hierarchy)"),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(isa? ::ua ::ub)"),
        "java.lang.RuntimeException | Unable to resolve var: global-hierarchy in this context"
    );
    assert_eq!(
        uncaught(&mut eval, "(derive ::ua ::ub)"),
        "java.lang.RuntimeException | Unable to resolve var: global-hierarchy in this context"
    );
    assert_eq!(
        uncaught(&mut eval, "(parents ::ua)"),
        "java.lang.RuntimeException | Unable to resolve var: global-hierarchy in this context"
    );
}

// ---- arity guards ----

#[test]
fn hier_arity_guards() {
    // Every native carries the house arity guard.
    let mut eval = boot();
    for (src, want) in [
        (
            "(make-hierarchy 1)",
            "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/make-hierarchy",
        ),
        (
            "(derive ::a ::b ::c ::d)",
            "clojure.lang.ArityException | Wrong number of args (4) passed to: clojure.core/derive",
        ),
        (
            "(underive ::a)",
            "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/underive",
        ),
        (
            "(isa? ::a)",
            "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/isa?",
        ),
        (
            "(parents)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/parents",
        ),
        (
            "(ancestors 1 2 3)",
            "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/ancestors",
        ),
        (
            "(descendants)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/descendants",
        ),
        (
            "(prefers)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/prefers",
        ),
        (
            "(prefers 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/prefers",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want);
    }
}

const LATTICE_CASES: &[(&str, &[&str], &[&str])] = &[
    (
        "clojure.lang.APersistentVector$SubVector",
        &[
            "clojure.lang.APersistentVector",
            "clojure.lang.IKVReduce",
            "clojure.lang.IObj",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentVector",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.Agent",
        &["clojure.lang.ARef"],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "clojure.lang.IDeref",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "java.lang.Object",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.Atom",
        &["clojure.lang.ARef", "clojure.lang.IAtom2"],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "clojure.lang.IAtom",
            "clojure.lang.IAtom2",
            "clojure.lang.IDeref",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "java.lang.Object",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.BigInt",
        &["clojure.lang.IHashEq", "java.lang.Number"],
        &[
            "clojure.lang.IHashEq",
            "java.io.Serializable",
            "java.lang.Number",
            "java.lang.Object",
        ],
    ),
    (
        "clojure.lang.Delay",
        &[
            "clojure.lang.IDeref",
            "clojure.lang.IPending",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.IDeref",
            "clojure.lang.IPending",
            "java.lang.Object",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.IFn",
        &["java.lang.Runnable", "java.util.concurrent.Callable"],
        &["java.lang.Runnable", "java.util.concurrent.Callable"],
    ),
    (
        "clojure.lang.ISeq",
        &["clojure.lang.IPersistentCollection"],
        &["clojure.lang.IPersistentCollection", "clojure.lang.Seqable"],
    ),
    (
        "clojure.lang.Keyword",
        &[
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.LazySeq",
        &[
            "clojure.lang.IHashEq",
            "clojure.lang.IPending",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Sequential",
            "java.util.List",
        ],
        &[
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.LongRange",
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IDrop",
            "clojure.lang.IReduce",
        ],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.MapEntry",
        &["clojure.lang.AMapEntry"],
        &[
            "clojure.lang.AFn",
            "clojure.lang.AMapEntry",
            "clojure.lang.APersistentVector",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMapEntry",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.Map$Entry",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.MultiFn",
        &["clojure.lang.AFn"],
        &[
            "clojure.lang.AFn",
            "clojure.lang.IFn",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentArrayMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IKVReduce",
            "clojure.lang.IMapIterable",
            "clojure.lang.IObj",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMapIterable",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentArrayMap$TransientArrayMap",
        &["clojure.lang.ATransientMap"],
        &[
            "clojure.lang.AFn",
            "clojure.lang.ATransientMap",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientAssociative",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientMap",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IKVReduce",
            "clojure.lang.IMapIterable",
            "clojure.lang.IObj",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMapIterable",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashSet",
        &[
            "clojure.lang.APersistentSet",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IObj",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentSet",
            "clojure.lang.Counted",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentSet",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.Set",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashSet$TransientHashSet",
        &["clojure.lang.ATransientSet"],
        &[
            "clojure.lang.AFn",
            "clojure.lang.ATransientSet",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientSet",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentList",
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IPersistentList",
            "clojure.lang.IReduce",
            "java.util.List",
        ],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentList",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentList$EmptyList",
        &[
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IPersistentList",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "java.util.List",
        ],
        &[
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentList",
            "clojure.lang.IPersistentStack",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentTreeMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IKVReduce",
            "clojure.lang.IObj",
            "clojure.lang.Reversible",
            "clojure.lang.Sorted",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sorted",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentTreeSet",
        &[
            "clojure.lang.APersistentSet",
            "clojure.lang.IObj",
            "clojure.lang.Reversible",
            "clojure.lang.Sorted",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentSet",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentSet",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sorted",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.Set",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentVector",
        &[
            "clojure.lang.APersistentVector",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IKVReduce",
            "clojure.lang.IObj",
            "clojure.lang.IReduce",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentVector",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentVector$TransientVector",
        &[
            "clojure.lang.AFn",
            "clojure.lang.Counted",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientVector",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientAssociative",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientVector",
            "clojure.lang.Indexed",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.Range",
        &[
            "clojure.lang.ASeq",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IReduce",
        ],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Obj",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Object",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Ratio",
        &["java.lang.Comparable", "java.lang.Number"],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.Object",
        ],
    ),
    (
        "clojure.lang.ReaderConditional",
        &["clojure.lang.ILookup", "java.lang.Object"],
        &["clojure.lang.ILookup", "java.lang.Object"],
    ),
    (
        "clojure.lang.Symbol",
        &[
            "clojure.lang.AFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IObj",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.TaggedLiteral",
        &["clojure.lang.ILookup", "java.lang.Object"],
        &["clojure.lang.ILookup", "java.lang.Object"],
    ),
    (
        "clojure.lang.Var",
        &[
            "clojure.lang.ARef",
            "clojure.lang.IFn",
            "clojure.lang.IRef",
            "clojure.lang.Settable",
            "java.io.Serializable",
        ],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "clojure.lang.IDeref",
            "clojure.lang.IFn",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "clojure.lang.Settable",
            "java.io.Serializable",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.Var$Unbound",
        &["clojure.lang.AFn"],
        &[
            "clojure.lang.AFn",
            "clojure.lang.IFn",
            "java.lang.Object",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "java.lang.Boolean",
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
        ],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
        ],
    ),
    ("java.lang.CharSequence", &[], &[]),
    (
        "java.lang.Character",
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
        ],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
        ],
    ),
    (
        "java.lang.Double",
        &[
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.Object",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.lang.Long",
        &[
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.Object",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.lang.Number",
        &["java.io.Serializable", "java.lang.Object"],
        &["java.io.Serializable", "java.lang.Object"],
    ),
    ("java.lang.Object", &[], &[]),
    (
        "java.lang.String",
        &[
            "java.io.Serializable",
            "java.lang.CharSequence",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
        &[
            "java.io.Serializable",
            "java.lang.CharSequence",
            "java.lang.Comparable",
            "java.lang.Object",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.math.BigDecimal",
        &["java.lang.Comparable", "java.lang.Number"],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.Object",
        ],
    ),
    (
        "java.math.BigInteger",
        &["java.lang.Comparable", "java.lang.Number"],
        &[
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Number",
            "java.lang.Object",
        ],
    ),
    (
        "java.util.regex.Pattern",
        &["java.io.Serializable", "java.lang.Object"],
        &["java.io.Serializable", "java.lang.Object"],
    ),
];

/// A sorted-names expectation in pr form (`()` when empty).
fn sorted_names(want: &[&str]) -> String {
    if want.is_empty() {
        return "()".to_owned();
    }
    let inner: Vec<String> = want.iter().map(|s| format!("\"{s}\"")).collect();
    format!("({})", inner.join(" "))
}

#[test]
fn hier_lattice_parents() {
    // Every full lattice row's bases through `parents`
    // (generated from the `l-*` / `n-range` / `n-unbound`
    // probes — port order is plain-ASCII over names).
    let mut eval = boot();
    assert_eq!(LATTICE_CASES.len(), 41);
    for (class, bases, _) in LATTICE_CASES {
        // The probe lists are themselves sorted, but sort
        // again: the assertion is order-exact either way.
        let mut want = bases.to_vec();
        want.sort_unstable();
        let got = run(
            &mut eval,
            &format!("(sort (map str (parents \"{class}\")))"),
        );
        assert_eq!(got, sorted_names(&want), "parents of {class}");
    }
}

#[test]
fn hier_lattice_ancestors() {
    // Every full lattice row's supers through `ancestors`
    // (generated from the same probes).
    let mut eval = boot();
    for (class, _, supers) in LATTICE_CASES {
        let mut want = supers.to_vec();
        want.sort_unstable();
        let got = run(
            &mut eval,
            &format!("(sort (map str (ancestors \"{class}\")))"),
        );
        assert_eq!(got, sorted_names(&want), "ancestors of {class}");
    }
}
