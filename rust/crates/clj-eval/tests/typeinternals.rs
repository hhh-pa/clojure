// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S14 type internals: probe replay for the JVM-internal
//! factories (`->ArrayChunk`, `->Eduction`, `->Vec`, `->VecNode`,
//! `->VecSeq`, `EMPTY-NODE`), the proxy helpers
//! (`construct-proxy`, `get-proxy-class`, `init-proxy`,
//! `proxy-call-with-super`, `proxy-mappings`, `proxy-name`,
//! `proxy-super`, `update-proxy`, `method-sig`), and protocol
//! introspection (`-cache-protocol-fn`, `-reset-methods`,
//! `find-protocol-impl`, `find-protocol-method`, `extenders`).
//! See `docs/zero-oos-s14-typeinternals/probes/*.out`. Every test
//! names the probe labels it replays; port-only pins say so
//! inline, with the JVM value in comments where they diverge.

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

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

// ---------------------------------------------------------------------------
// Factories.
// ---------------------------------------------------------------------------

#[test]
fn factory_print_shapes() {
    // Probes `d-empty`, `d-vecnode`: the shared `#object[Name
    // 0xH "Name@H"]` fallback names the synthetic class.
    let mut eval = boot();
    let v = run_last(&mut eval, "EMPTY-NODE");
    assert!(v.starts_with("#object[clojure.core.VecNode 0x"), "{v}");
    let v = run_last(&mut eval, "(->Vec nil 0 5 EMPTY-NODE [] nil)");
    assert!(v.starts_with("#object[clojure.core.Vec 0x"), "{v}");
    let v = run_last(&mut eval, "(->VecSeq nil nil nil 0 0 nil)");
    assert!(v.starts_with("#object[clojure.core.VecSeq 0x"), "{v}");
    let v = run_last(&mut eval, "(->ArrayChunk nil [] 0 0)");
    assert!(v.starts_with("#object[clojure.core.ArrayChunk 0x"), "{v}");
}

#[test]
fn factory_arities() {
    // Probes `c-vecnode-1`, `c-vecnode-0`, `c-vec-short`,
    // `c-eduction-1`, `c-chunk-3`, `c-vecseq-5` (house arity
    // text; the JVM names the generated fn).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(->VecNode)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/->VecNode"
    );
    assert_eq!(
        uncaught(&mut eval, "(->Vec nil 0 5 EMPTY-NODE)"),
        "clojure.lang.ArityException | Wrong number of args (4) passed to: clojure.core/->Vec"
    );
    assert_eq!(
        uncaught(&mut eval, "(->Eduction (map inc))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/->Eduction"
    );
    assert_eq!(
        uncaught(&mut eval, "(->ArrayChunk nil nil 0)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/->ArrayChunk"
    );
    assert_eq!(
        uncaught(&mut eval, "(->VecSeq nil nil nil 0 0)"),
        "clojure.lang.ArityException | Wrong number of args (5) passed to: clojure.core/->VecSeq"
    );
}

#[test]
fn vecnode_one_arity_and_empty_node() {
    // BDD: `(->VecNode nil)` builds a handle `EMPTY-NODE`
    // equals. The 1-arity is a port extension (the JVM throws
    // `ArityException`, probe `c-vecnode-1`); handles compare by
    // value (the JVM compares by identity, probe `e-fresh`).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(= (->VecNode nil) EMPTY-NODE)"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(= EMPTY-NODE EMPTY-NODE)"), "true");
    // Value-equal handles hash equal too.
    assert_eq!(
        run_last(&mut eval, "(= (hash (->VecNode nil)) (hash EMPTY-NODE))"),
        "true"
    );
    // The defaulted `arr` is the 32-nil vector (probe `f-arr-count`).
    assert_eq!(run_last(&mut eval, "(count (.-arr (->VecNode nil)))"), "32");
    assert_eq!(run_last(&mut eval, "(count (.-arr EMPTY-NODE))"), "32");
}

#[test]
fn factory_fields() {
    // Probes `f-edit`, `f-vec-cnt`, `f-chunk-off`,
    // `f-chunk-end`, `f-vecseq-i`, `f-eduction-coll`: fields
    // read through the shared `.-field` path.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(.-edit EMPTY-NODE)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(.-cnt (->Vec nil 7 5 EMPTY-NODE [] nil))"),
        "7"
    );
    assert_eq!(
        run_last(&mut eval, "(.-off (->ArrayChunk nil [] 1 3))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(.-end (->ArrayChunk nil [] 1 3))"),
        "3"
    );
    assert_eq!(
        run_last(&mut eval, "(.-i (->VecSeq nil nil nil 4 0 nil))"),
        "4"
    );
    // (`.-coll` on the JVM, probe `f-eduction-coll`, has no port
    // analog: the native eduction exposes no field reads.)
}

#[test]
fn factory_predicates() {
    // Probes `g-map`, `g-seq-chunk`: handles are opaque, never
    // collections. `(vector? (->Vec …))` is port-false (JVM
    // true, probe `g-vec`) — the gvec vector protocol is not
    // modeled.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(map? EMPTY-NODE)"), "false");
    assert_eq!(
        run_last(&mut eval, "(seq? (->ArrayChunk nil [] 0 0))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(vector? (->Vec nil 0 5 EMPTY-NODE [] nil))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(counted? (->VecNode nil))"), "false");
}

#[test]
fn eduction_factory_is_native() {
    // Probe `e-eduction`: `->Eduction` builds the native
    // eduction value (identity equality).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(= (->Eduction (map inc) [1 2]) (->Eduction (map inc) [1 2]))"
        ),
        "false"
    );
    // Same reduction `(eduction …)` runs (the port's eduction
    // reduces but never seqs, so the `h-ed-seq`/`h-ed-first`
    // seq pins have no port analog).
    assert_eq!(
        run_last(&mut eval, "(reduce + 0 (->Eduction (map inc) [1 2 3]))"),
        "9"
    );
    assert_eq!(
        run_last(&mut eval, "(into [] (->Eduction (map inc) [1 2 3]))"),
        "[2 3 4]"
    );
    // Port-false like the native eduction (JVM true, probe
    // `g-sequential-ed`) — T1200's established behavior.
    assert_eq!(
        run_last(&mut eval, "(sequential? (->Eduction (map inc) [1 2]))"),
        "false"
    );
}

#[test]
fn handle_opacity_errors() {
    // Handles are not live gvec values: `count`/`nth`/`seq`
    // take the house errors (the JVM answers through the
    // gvec machinery — probes `h-chunk-count`, `h-chunk-nth`,
    // `h-vec-count`, `h-vecseq-seq` — or NPEs on the nil
    // manager the port never models).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(count (->ArrayChunk nil [1 2 3] 0 3))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: ArrayChunk"
    );
    let e = uncaught(&mut eval, "(seq (->VecSeq nil nil nil 0 0 nil))");
    assert!(
        e.starts_with("java.lang.IllegalArgumentException | "),
        "{e}"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (->VecNode nil) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: VecNode"
    );
}

// ---------------------------------------------------------------------------
// Proxy helpers.
// ---------------------------------------------------------------------------

#[test]
fn proxy_name_shapes() {
    // Probes `a-name-obj`, `a-name-runnable`, `a-name-two`,
    // `a-name-str`, `a-name-vec`: exact JVM name strings
    // (including the set hash).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(proxy-name Object [])"),
        "\"user.proxy$java.lang.Object$ff19274a\""
    );
    assert_eq!(
        run_last(&mut eval, "(proxy-name Object [\"java.lang.Runnable\"])"),
        "\"user.proxy$java.lang.Object$Runnable$c4146801\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(proxy-name Object [\"java.lang.Runnable\" \"java.io.Serializable\"])"
        ),
        "\"user.proxy$java.lang.Object$Serializable$Runnable$7c83e70d\""
    );
    assert_eq!(
        run_last(&mut eval, "(proxy-name \"java.lang.String\" [])"),
        "\"user.proxy$java.lang.String$ff19274a\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(proxy-name Object (list \"java.lang.Runnable\"))"
        ),
        "\"user.proxy$java.lang.Object$Runnable$c4146801\""
    );
    // Interface defs contribute their names.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (definterface PNJ (j [x])) (proxy-name Object [PNJ]))"
        ),
        "\"user.proxy$java.lang.Object$PNJ$41812c95\""
    );
}

#[test]
fn proxy_name_errors() {
    // Probes `a-name-noclass`, `a-name-arity` (exact JVM
    // texts, via the shared `cce_message`). `a-name-nil` NPEs
    // on the JVM with JDK-only text; the port answers the
    // `Class` cast instead.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(proxy-name 5 [])"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(proxy-name nil [])"),
        "java.lang.ClassCastException | class null cannot be cast to class java.lang.Class (null is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(proxy-name Object)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/proxy-name"
    );
}

#[test]
fn get_proxy_class_rows() {
    // Probes `b-name`, `b-name-iface`, `b-default-super`: the
    // class-name string (S16 upgrades to `Class` values).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(get-proxy-class Object)"),
        "\"user.proxy$java.lang.Object$ff19274a\""
    );
    assert_eq!(
        run_last(&mut eval, "(get-proxy-class Object \"java.lang.Runnable\")"),
        "\"user.proxy$java.lang.Object$Runnable$c4146801\""
    );
    // The JVM answers a cached `Class` (probe `b-cached`); the
    // port answers equal strings.
    assert_eq!(
        run_last(
            &mut eval,
            "(= (get-proxy-class Object) (get-proxy-class Object))"
        ),
        "true"
    );
    // Leading interfaces default the superclass to `Object`.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (definterface GPI (g [x])) (get-proxy-class GPI))"
        ),
        "\"user.proxy$java.lang.Object$GPI$e17cecc2\""
    );
    assert_eq!(
        run_last(&mut eval, "(get-proxy-class \"java.lang.Runnable\")"),
        "\"user.proxy$java.lang.Object$Runnable$c4146801\""
    );
    // Zero bases default the same way (the JVM NPEs, probe
    // `b-none` — the docstring's contract honored instead).
    assert_eq!(
        run_last(&mut eval, "(get-proxy-class)"),
        "\"user.proxy$java.lang.Object$ff19274a\""
    );
}

#[test]
fn get_proxy_class_errors() {
    // Probe `b-nonclass` (house text). Non-`Object`
    // superclasses are the `__proxy*` fence error (the JVM
    // raises `IncompatibleClassChangeError` for final
    // `String`, probe `b-final` — unreachable here since only
    // `Object` passes).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(get-proxy-class 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (deftype GPB [x]) (get-proxy-class GPB))"),
        "java.lang.IllegalArgumentException | Cannot extend user.GPB: proxy supports Object only"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (deftype GPC [x]) (get-proxy-class Object GPC))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: user.GPC"
    );
}

#[test]
fn construct_proxy_rows() {
    // Probes `c-class`, `c-mappings-fresh`: an instance of the
    // named class with a nil fn map.
    let mut eval = boot();
    let v = run_last(
        &mut eval,
        "(construct-proxy (get-proxy-class Object \"java.lang.Runnable\"))",
    );
    assert!(
        v.starts_with("#object[user.proxy$java.lang.Object$Runnable$c4146801 0x"),
        "{v}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(proxy-mappings (construct-proxy (get-proxy-class Object)))"
        ),
        "nil"
    );
    // `update-proxy` over the nil map assocs from `{}`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (construct-proxy (get-proxy-class Object))] (update-proxy p {\"toString\" (fn [] \"up\")}) (str p))"
        ),
        "\"up\""
    );
}

#[test]
fn construct_proxy_errors() {
    // Probes `c-ctor-args`, `c-nonclass`, `c-arity`.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(construct-proxy (get-proxy-class Object) 1 2)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class user.proxy$java.lang.Object$ff19274a"
    );
    assert_eq!(
        uncaught(&mut eval, "(construct-proxy 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(construct-proxy)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/construct-proxy"
    );
}

#[test]
fn proxy_mappings_round_trip() {
    // BDD plus probes `d-init`, `d-call`, `d-update`, `d-nil`,
    // `d-macro-map`: the map round-trips verbatim (port fns
    // take no `this`, per the #33 fence).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (construct-proxy (get-proxy-class Object)) m {\"toString\" (fn [] \"hi\")} q (init-proxy p m)] (= (proxy-mappings p) m))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (construct-proxy (get-proxy-class Object))] (init-proxy p {\"toString\" (fn [] \"hi\")}) (str p))"
        ),
        "\"hi\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (construct-proxy (get-proxy-class Object))] (init-proxy p {\"toString\" (fn [] \"hi\") \"hashCode\" (fn [] 42)}) (update-proxy p {\"toString\" (fn [] \"yo\")}) [(str p) (sort (keys (proxy-mappings p)))])"
        ),
        "[\"yo\" (\"hashCode\" \"toString\")]"
    );
    // Nil values stay visible in the map …
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (construct-proxy (get-proxy-class Object))] (init-proxy p {\"toString\" (fn [] \"hi\")}) (update-proxy p {\"toString\" nil}) [(contains? (proxy-mappings p) \"toString\") (get (proxy-mappings p) \"toString\")])"
        ),
        "[true nil]"
    );
    // … and revert dispatch to super.
    let v = run_last(
        &mut eval,
        "(let [p (proxy [Object] [] (toString [] \"mx\"))] (update-proxy p {\"toString\" nil}) (str p))",
    );
    assert!(v.starts_with("\"user$proxy@"), "{v}");
    // Macro-built proxies start mapped.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (proxy [Object] [] (toString [] \"mx\"))] [(sort (keys (proxy-mappings p))) (str p)])"
        ),
        "[(\"toString\") \"mx\"]"
    );
}

#[test]
fn proxy_mutation_errors() {
    // Probes `d-init-nonproxy`, `d-init-nonmap`,
    // `d-update-nonproxy`, `d-update-nonmap`,
    // `d-mappings-nonproxy` (house cast texts).
    let mut eval = boot();
    for (src, msg) in [
        (
            "(init-proxy 5 {})",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IProxy (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IProxy is in unnamed module of loader 'app')",
        ),
        (
            "(let [p (proxy [Object] [] (toString [] \"x\"))] (init-proxy p 5))",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')",
        ),
        (
            "(update-proxy 5 {})",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IProxy (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IProxy is in unnamed module of loader 'app')",
        ),
        (
            "(let [p (proxy [Object] [] (toString [] \"x\"))] (update-proxy p 5))",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')",
        ),
        (
            "(proxy-mappings 5)",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IProxy (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IProxy is in unnamed module of loader 'app')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), msg, "{src}");
    }
    // A non-proxy host instance is not an `IProxy` either.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (deftype PMI [x]) (proxy-mappings (PMI. 1)))"
        ),
        "java.lang.ClassCastException | class user.PMI cannot be cast to class clojure.lang.IProxy (user.PMI and clojure.lang.IProxy are in unnamed module of loader 'app')"
    );
}

#[test]
fn proxy_multi_arity_cell() {
    // Port shape: multi-overload methods keep the `{argc →
    // fn}` submap in the cell and dispatch per arity.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (definterface PMA (m [x]) (m [x y])) (let [p (proxy [Object PMA] [] (m [x] :one) (m [x y] :two))] (sort (keys (get (proxy-mappings p) \"m\")))))"
        ),
        "(1 2)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (definterface PMB (m [x]) (m [x y])) (let [p (proxy [Object PMB] [] (m [x] :one) (m [x y] :two))] [(.m p 1) (.m p 1 2)]))"
        ),
        "[:one :two]"
    );
}

#[test]
fn method_sig_rows() {
    // Probes `e-tostring`, `e-valueof`, `e-shape`: the JVM
    // triple shape over the port descriptor contract
    // (classes travel as strings).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(method-sig [\"toString\" nil \"java.lang.String\"])"
        ),
        "[\"toString\" nil \"java.lang.String\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(method-sig [\"valueOf\" [\"java.lang.Object\"] \"java.lang.String\"])"
        ),
        "[\"valueOf\" (\"java.lang.Object\") \"java.lang.String\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [[n ps r] (method-sig [\"toString\" [] \"java.lang.String\"])] [(string? n) (nil? ps) r])"
        ),
        "[true true \"java.lang.String\"]"
    );
}

#[test]
fn method_sig_errors() {
    // Probe `e-arity` (house text). Shape errors are
    // port-defined (the JVM input type cannot exist here).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(method-sig)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/method-sig"
    );
    assert_eq!(
        uncaught(&mut eval, "(method-sig \"toString\")"),
        "java.lang.IllegalArgumentException | method-sig requires a [name params return] triple"
    );
    assert_eq!(
        uncaught(&mut eval, "(method-sig [\"toString\" nil])"),
        "java.lang.IllegalArgumentException | method-sig requires a [name params return] triple"
    );
    assert_eq!(
        uncaught(&mut eval, "(method-sig [5 nil \"java.lang.String\"])"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(method-sig [\"toString\" 5 \"java.lang.String\"])"
        ),
        "java.lang.IllegalArgumentException | method-sig params must be nil or sequential"
    );
}

#[test]
fn proxy_call_with_super_rows() {
    // Probes `f-super`, `f-restored`, `f-arity`: the mapping is
    // masked during the call, then restored. (The JVM calls
    // from inside the method; the port calls from outside —
    // `let` inits cannot self-name — over the same
    // mask/call/restore machinery.)
    let mut eval = boot();
    let v = run_last(
        &mut eval,
        "(let [p (proxy [Object] [] (toString [] \"mx\"))] (proxy-call-with-super (fn [] (.toString p)) p \"toString\"))",
    );
    assert!(v.starts_with("\"user$proxy@"), "{v}");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (proxy [Object] [] (toString [] \"mx\"))] (proxy-call-with-super (fn [] :called) p \"toString\") [(sort (keys (proxy-mappings p))) (str p)])"
        ),
        "[(\"toString\") \"mx\"]"
    );
    assert_eq!(
        uncaught(&mut eval, "(proxy-call-with-super (fn [] 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/proxy-call-with-super"
    );
    // The `this` slot takes the same `IProxy` cast.
    assert_eq!(
        uncaught(
            &mut eval,
            "(proxy-call-with-super (fn [] 1) 5 \"toString\")"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IProxy (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IProxy is in unnamed module of loader 'app')"
    );
}

#[test]
fn proxy_super_macro() {
    // Probe `f-macro`: the JVM expansion modulo the call
    // shape — the port has no `.`-head `(. this meth …)`
    // interop form, so the macro emits the port `.meth` call
    // (capturing `this`, which the caller binds).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(proxy-super toString))"),
        "(clojure.core/proxy-call-with-super (clojure.core/fn [] (.toString this)) this \"toString\")"
    );
    // Probe `f-supermacro`: the super call runs (the atom
    // carries the instance into its own method body, which a
    // `let` init cannot self-name).
    let v = run_last(
        &mut eval,
        "(let [pc (atom nil) p (proxy [Object] [] (toString [] (let [this @pc] (str \"S:\" (proxy-super toString)))))] (reset! pc p) (str p))",
    );
    assert!(v.starts_with("\"S:user$proxy@"), "{v}");
}

// ---------------------------------------------------------------------------
// Protocol introspection.
// ---------------------------------------------------------------------------

/// The probe `protocols.clj` setup: `RP` extended over `String`
/// (both methods), `Long` (`:rm` only), `Number`, `Object`, and
/// `IPersistentMap`; `RI` implements inline; `RQ` is empty.
const PROTO_SETUP: &str = "(do (defprotocol RP (rm [x]) (rn [x y])) (extend-protocol RP String (rm [x] :str-m) (rn [x y] :str-n)) (extend-protocol RP Long (rm [x] :long-m)) (extend-protocol RP java.lang.Number (rm [x] :number-m) (rn [x y] :number-n)) (extend-protocol RP clojure.lang.IPersistentMap (rm [x] :map-m)) (extend-protocol RP Object (rm [x] :obj-m)) (deftype RI [x] RP (rm [this] :inline-m)) (defprotocol RQ (qm [x])) :setup)";

#[test]
fn find_protocol_impl_rows() {
    // Probes `a-exact-keys`, `a-inline`, `a-miss`, `a-nil`:
    // the normalized `{keyword → fn}` map, the instance
    // itself for inline impls, nil on misses.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        run_last(&mut eval, "(sort (keys (find-protocol-impl RP \"hi\")))"),
        "(:rm :rn)"
    );
    // Inline answers the instance itself (not a map).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [i (RI. 1) r (find-protocol-impl RP i)] [(map? r) (= r i)])"
        ),
        "[false true]"
    );
    assert_eq!(run_last(&mut eval, "(find-protocol-impl RQ \"hi\")"), "nil");
    // No nil entry yet — and `nil` skips the `Object`
    // fallback (the JVM's `(and c …)` guard).
    assert_eq!(run_last(&mut eval, "(find-protocol-impl RP nil)"), "nil");
}

#[test]
fn find_protocol_method_walk() {
    // BDD plus probes `a-exact-call`, `a-long-call`,
    // `a-long-missing`, `a-super-call`, `a-iface-call`,
    // `a-object-call`: the class walk, step by step.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        run_last(&mut eval, "((find-protocol-method RP :rm \"hi\") \"hi\")"),
        ":str-m"
    );
    assert_eq!(
        run_last(&mut eval, "((find-protocol-method RP :rm 5) 5)"),
        ":long-m"
    );
    // The exact entry wins wholesale: `Long` lacks `:rn`,
    // so the `Number` `:rn` does not show through.
    assert_eq!(
        run_last(&mut eval, "(find-protocol-method RP :rn 5)"),
        "nil"
    );
    // Superclass step: the port has no `Short` value (the
    // probe's `a-super-call` spelling), so a `Number`-only
    // protocol walks `Long` → `Number`, and a map entry walks
    // `MapEntry` → `AMapEntry` → `APersistentVector`.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol SP (sm [x])) (extend-protocol SP java.lang.Number (sm [x] :number-m)) ((find-protocol-method SP :sm 5) 5))"
        ),
        ":number-m"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol ME (me [x])) (extend-protocol ME clojure.lang.APersistentVector (me [x] :avec)) ((find-protocol-method ME :me (first {:a 1})) (first {:a 1})))"
        ),
        ":avec"
    );
    // Interface step (`IPersistentMap`).
    assert_eq!(
        run_last(&mut eval, "((find-protocol-method RP :rm {:a 1}) {:a 1})"),
        ":map-m"
    );
    // `Object` step.
    assert_eq!(
        run_last(&mut eval, "((find-protocol-method RP :rm :kw) :kw)"),
        ":obj-m"
    );
}

#[test]
fn find_protocol_method_edges() {
    // Probes `b-inline-nil`, `b-str-key`, `b-missing-k`.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        run_last(&mut eval, "(find-protocol-method RP :rm (RI. 1))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(find-protocol-method RP \"rm\" \"hi\")"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(find-protocol-method RP :nope \"hi\")"),
        "nil"
    );
}

#[test]
fn find_protocol_errors() {
    // Probes `a-arity`, `b-arity` (house texts). Non-protocols
    // NPE on the JVM with JDK-only text (probes
    // `a-nonproto`, `b-nonproto`); the port answers the house
    // `is not a protocol`, like `satisfies?`/`extends?`.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        uncaught(&mut eval, "(find-protocol-impl 5 \"hi\")"),
        "java.lang.IllegalArgumentException | 5 is not a protocol"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-protocol-method 5 :rm \"hi\")"),
        "java.lang.IllegalArgumentException | 5 is not a protocol"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-protocol-impl RP)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/find-protocol-impl"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-protocol-method RP :rm)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/find-protocol-method"
    );
}

#[test]
fn find_multi_arity_and_pref() {
    // Multi-arity methods answer the smallest-arity fn (port
    // rule, pinned); `pref` picks the more specific
    // interface (`IPersistentMap` over `Associative`).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol MP (mm [x] [x y])) (extend-protocol MP String (mm ([x] :one) ([x y] :two))) ((find-protocol-method MP :mm \"s\") \"s\"))"
        ),
        ":one"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol PF (pf [x])) (extend-protocol PF clojure.lang.Associative (pf [x] :assoc)) (extend-protocol PF clojure.lang.IPersistentMap (pf [x] :map)) ((find-protocol-method PF :pf {:a 1}) {:a 1}))"
        ),
        ":map"
    );
}

#[test]
fn extenders_rows() {
    // Probes `c-list`, `c-empty`, `c-nonproto`, `c-nil`,
    // `c-arity`: class-name strings (S16 upgrades to
    // classes), nil when empty; total like the JVM.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        run_last(&mut eval, "(sort (extenders RP))"),
        "(\"clojure.lang.IPersistentMap\" \"java.lang.Long\" \"java.lang.Number\" \"java.lang.Object\" \"java.lang.String\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (set (extenders RP)) #{\"java.lang.String\" \"java.lang.Long\" \"java.lang.Number\" \"clojure.lang.IPersistentMap\" \"java.lang.Object\"})"
        ),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(extenders RQ)"), "nil");
    assert_eq!(run_last(&mut eval, "(extenders 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(extenders nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(extenders)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/extenders"
    );
}

#[test]
fn nil_extension_row() {
    // Probe `f-nil-call`: an explicit `"null"` entry hits for
    // `nil` (which otherwise skips the walk).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, PROTO_SETUP), ":setup");
    assert_eq!(
        run_last(
            &mut eval,
            "(do (extend-protocol RP nil (rm [x] :nil-m)) ((find-protocol-method RP :rm nil) nil))"
        ),
        ":nil-m"
    );
}

#[test]
fn cache_protocol_fn_rows() {
    // Probes `d-hit`, `d-interf`: the lookup hit and the
    // `isInstance` shortcut (the port keeps no
    // `MethodImplCache`, so nothing is stored — every
    // protocol call reads the live registry).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol RC (rcm [x])) (extend-protocol RC String (rcm [x] :cached)) (deftype RD [] RC (rcm [this] :d)) :setup)"
        ),
        ":setup"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((-cache-protocol-fn rcm \"hi\" (:on RC) :INTERF) \"hi\")"
        ),
        ":cached"
    );
    assert_eq!(
        run_last(&mut eval, "(-cache-protocol-fn rcm (RD.) (:on RC) :INTERF)"),
        ":INTERF"
    );
    // A def names its class too (the proxy claims `RCI`).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (definterface RCI (q [x])) (-cache-protocol-fn rcm (proxy [Object RCI] [] (q [x] 1)) RCI :INTERF))"
        ),
        ":INTERF"
    );
}

#[test]
fn cache_protocol_fn_errors() {
    // Probes `d-miss`, `d-nil`, `d-arity` (exact texts) and
    // `G` (left-to-right hint checks). `d-badpf` NPEs on the
    // JVM with JDK-only text; the port answers `Not a
    // protocol fn`.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol RC (rcm [x])) (extend-protocol RC String (rcm [x] :cached)) :setup)"
        ),
        ":setup"
    );
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn rcm 5 (:on RC) :INTERF)"),
        "java.lang.IllegalArgumentException | No implementation of method: :rcm of protocol: #'user/RC found for class: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn rcm nil (:on RC) :INTERF)"),
        "java.lang.IllegalArgumentException | No implementation of method: :rcm of protocol: #'user/RC found for class: nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(-cache-protocol-fn (fn [x] x) \"hi\" (:on RC) :INTERF)"
        ),
        "java.lang.IllegalArgumentException | Not a protocol fn"
    );
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn rcm \"hi\")"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/-cache-protocol-fn"
    );
    // `pf` checks before `c`, `c` before `interf`.
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn 5 \"hi\" 6 :INTERF)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.AFunction (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.AFunction is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn rcm \"hi\" 6 7)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(-cache-protocol-fn rcm \"hi\" (:on RC) 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
}

#[test]
fn reset_methods_rows() {
    // Probes `e-ok`, `e-nonproto`, `e-nil`, `e-arity`: total,
    // answering nil (no cache exists to rebuild).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol RC (rcm [x])) (extend-protocol RC String (rcm [x] :cached)) :setup)"
        ),
        ":setup"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (-reset-methods RC) [(rcm \"hi\") (-reset-methods RC)])"
        ),
        "[:cached nil]"
    );
    assert_eq!(run_last(&mut eval, "(-reset-methods 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(-reset-methods nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(-reset-methods)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/-reset-methods"
    );
}

#[test]
fn proxy_protocol_inline_limitation() {
    // Known seam (pinned, not fixed): protocol dispatch
    // through proxy instances reads the baked table
    // (`inline_impl`, #33), not the live proxy map cell — so
    // `update-proxy` removal does not affect protocol calls.
    // (Proxy bodies exclude `this`, so the method takes no
    // args.)
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol PX (px [x])) (def p (proxy [Object PX] [] (px [] :baked))) [(px p) (do (update-proxy p {\"px\" nil}) (px p))])"
        ),
        "[:baked :baked]"
    );
}
