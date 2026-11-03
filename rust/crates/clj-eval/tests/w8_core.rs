// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W8 (core parity + redesigns) smoke tests (issue #204): every added
//! var and redesigned namespace against upstream-documented behavior.
//! Part A: `clojure.core.protocols` (the only missing core-adjacent
//! namespace; `core.clj`/`core_deftype.clj` audited complete) plus
//! `Eduction` class-position pins. Part B: genclass lattice
//! registration + `gen-and-load-class`, the `clojure.inspector` text
//! redesign, and `clojure.java.browse-ui`.

use clj_eval::{Evaluator, SourceError, boot, jdkio};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

/// Eval one form, expecting a string result; return its contents.
fn eval_str(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    assert_eq!(vs.len(), 1, "expected one value for {src}");
    match &vs[0] {
        Value::Str(s) => s.to_string(),
        other => panic!("expected a string for {src}, got: {}", pr(other)),
    }
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw for {src}, got {other}"),
        Ok(_) => panic!("expected an uncaught throw for {src}"),
    }
}

/// Intern a Rust-built value as `user/<name>` (the `jdkio.rs`
/// promise-test pattern): the only way handles enter evaluated code.
fn intern(eval: &mut Evaluator, name: &str, v: Value) {
    let sym = clj_value::Symbol::new(Some("user"), name);
    let id = eval.registry.intern(&sym);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(v);
    });
}

fn ints(n: i64) -> Vec<Value> {
    (1..=n).map(Value::Int).collect()
}

const P: &str = "clojure.core.protocols";

// ---- Part A: clojure.core.protocols ----

#[test]
fn w8_protocols_require_and_resolve() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require 'clojure.core.protocols)"),
        "nil"
    );
    // All eleven upstream publics resolve (5 protocols + 5 fns +
    // `iterator-reduce!`).
    assert_eq!(
        run_last(
            &mut eval,
            "(count (filter some? (map resolve '[clojure.core.protocols/CollReduce clojure.core.protocols/coll-reduce clojure.core.protocols/InternalReduce clojure.core.protocols/internal-reduce clojure.core.protocols/IKVReduce clojure.core.protocols/kv-reduce clojure.core.protocols/Datafiable clojure.core.protocols/datafy clojure.core.protocols/Navigable clojure.core.protocols/nav clojure.core.protocols/iterator-reduce!])))"
        ),
        "11"
    );
}

#[test]
fn w8_coll_reduce_basics() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    let c = |eval: &mut Evaluator, src: &str| run_last(eval, src);
    assert_eq!(c(&mut eval, &format!("({P}/coll-reduce [1 2 3] + 0)")), "6");
    assert_eq!(c(&mut eval, &format!("({P}/coll-reduce [1 2 3] +)")), "6");
    assert_eq!(c(&mut eval, &format!("({P}/coll-reduce nil + 0)")), "0");
    assert_eq!(c(&mut eval, &format!("({P}/coll-reduce nil +)")), "0");
    assert_eq!(c(&mut eval, &format!("({P}/coll-reduce '(1 2) + 0)")), "3");
    assert_eq!(
        eval_str(&mut eval, &format!("({P}/coll-reduce \"ab\" str \"!\")")),
        "!ab"
    );
    assert_eq!(
        c(&mut eval, &format!("({P}/coll-reduce {{:a 1}} conj [])")),
        "[[:a 1]]"
    );
    assert_eq!(
        c(&mut eval, &format!("({P}/coll-reduce #{{1 2}} + 0)")),
        "3"
    );
    assert_eq!(
        c(&mut eval, &format!("({P}/coll-reduce (range 5) + 0)")),
        "10"
    );
    assert_eq!(
        c(&mut eval, &format!("({P}/coll-reduce (map inc [1 2]) + 0)")),
        "5"
    );
    assert_eq!(
        c(
            &mut eval,
            &format!("({P}/coll-reduce (eduction (map inc) [1 2]) + 0)")
        ),
        "5"
    );
}

#[test]
fn w8_coll_reduce_reduced() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    // Through the exact PersistentVector arm ...
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/coll-reduce [1 2 3] (fn [a _] (reduced a)) 0)")
        ),
        "0"
    );
    // ... and through the Object-default fallback (lists hold no arm).
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/coll-reduce '(1 2 3) (fn [a _] (reduced a)) 0)")
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/internal-reduce (seq [1 2]) (fn [a _] (reduced a)) 0)")
        ),
        "0"
    );
}

#[test]
fn w8_internal_reduce_basics() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    assert_eq!(
        run_last(&mut eval, &format!("({P}/internal-reduce (seq [1 2]) + 0)")),
        "3"
    );
    assert_eq!(
        run_last(&mut eval, &format!("({P}/internal-reduce nil + 7)")),
        "7"
    );
}

#[test]
fn w8_coll_reduce_user_extend() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(require 'clojure.core.protocols) (deftype W8Box [xs]) \
                 (extend-protocol {P}/CollReduce W8Box \
                   (coll-reduce ([coll f] :w8-two) ([coll f val] :w8-three))) \
                 [({P}/coll-reduce (W8Box. [1]) + 0) ({P}/coll-reduce (W8Box. [1]) +) \
                  (satisfies? {P}/CollReduce (W8Box. [1])) (extends? {P}/CollReduce W8Box)]"
            )
        ),
        "[:w8-three :w8-two true true]"
    );
}

#[test]
fn w8_iterator_reduce() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    let w8i3 = jdkio::iterator(&mut eval, ints(3));
    let w8i3b = jdkio::iterator(&mut eval, ints(3));
    let w8i3c = jdkio::iterator(&mut eval, ints(3));
    let w8i0 = jdkio::iterator(&mut eval, Vec::new());
    intern(&mut eval, "w8i3", w8i3);
    intern(&mut eval, "w8i3b", w8i3b);
    intern(&mut eval, "w8i3c", w8i3c);
    intern(&mut eval, "w8i0", w8i0);
    assert_eq!(
        run_last(&mut eval, &format!("({P}/iterator-reduce! w8i3 + 0)")),
        "6"
    );
    assert_eq!(
        run_last(&mut eval, &format!("({P}/iterator-reduce! w8i3b +)")),
        "6"
    );
    assert_eq!(
        run_last(&mut eval, &format!("({P}/iterator-reduce! w8i0 vector)")),
        "[]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/iterator-reduce! w8i3c (fn [a _] (reduced a)) 0)")
        ),
        "0"
    );
    // A nil iterator is the `.hasNext` NPE, like upstream.
    assert_eq!(
        uncaught(&mut eval, &format!("({P}/iterator-reduce! nil + 0)")),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Iterator.hasNext()\" because \"iter\" is null"
    );
}

#[test]
fn w8_kv_reduce_protocol() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(require 'clojure.core.protocols) \
                 (extend-protocol {P}/IKVReduce clojure.lang.PersistentArrayMap \
                   (kv-reduce [m f init] :w8-kv)) \
                 ({P}/kv-reduce {{:a 1}} + 0)"
            )
        ),
        ":w8-kv"
    );
}

#[test]
fn w8_datafy_nav() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    // Defaults: identity / nil / v.
    assert_eq!(
        run_last(&mut eval, &format!("({P}/datafy {{:a 1}})")),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, &format!("({P}/datafy nil)")), "nil");
    assert_eq!(run_last(&mut eval, &format!("({P}/datafy 1)")), "1");
    assert_eq!(
        run_last(&mut eval, &format!("({P}/nav {{:a 1}} :a 1)")),
        "1"
    );
    // Metadata extension (the `:extend-via-metadata` emulation).
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/datafy (with-meta [1] {{'{P}/datafy (fn [x] :meta!)}}))")
        ),
        ":meta!"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("({P}/nav (with-meta {{}} {{'{P}/nav (fn [c k v] :navmeta)}}) :k :v)")
        ),
        ":navmeta"
    );
    // Explicit extensions win over the default.
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(deftype W8D [x]) (extend-protocol {P}/Datafiable W8D (datafy [o] :w8d)) ({P}/datafy (W8D. 1))"
            )
        ),
        ":w8d"
    );
}

#[test]
fn w8_protocols_satisfies_extends_spots() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.core.protocols)")
        .unwrap();
    // The kept upstream arms answer `extends?` exactly.
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "[(extends? {P}/CollReduce clojure.lang.PersistentVector) (extends? {P}/CollReduce java.lang.Object) (extends? {P}/CollReduce java.lang.Iterable) (extends? {P}/Datafiable java.lang.Object) (satisfies? {P}/CollReduce [1]) (satisfies? {P}/CollReduce nil) (satisfies? {P}/InternalReduce nil)]"
            )
        ),
        "[true true true true true true true]"
    );
}

// ---- Part A: Eduction class positions (no-gap pins) ----

#[test]
fn w8_eduction_class_positions() {
    let mut eval = boot();
    // The `(deftype Eduction ...)` class resolves in every class
    // position (the qualified `resolve` stays nil: upstream defines
    // no `Eduction` var, so `resolve` finds no var on either side).
    assert_eq!(
        run_last(&mut eval, "(resolve 'clojure.core.Eduction)"),
        "clojure.core.Eduction"
    );
    assert_eq!(
        run_last(&mut eval, "(import 'clojure.core.Eduction)"),
        "clojure.core.Eduction"
    );
    assert_eq!(
        run_last(&mut eval, "(class? (resolve 'clojure.core.Eduction))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (resolve 'clojure.core.Eduction) (class (eduction (map inc) [1])))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(instance? clojure.core.Eduction (eduction (map inc) [1]))"
        ),
        "true"
    );
}

// ---- Part B: genclass lattice registration ----

#[test]
fn w8_genclass_dotted_registers() {
    let mut eval = boot();
    // The class resolves for import/resolve after generation ...
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-class :name \"com.w8reg.Foo\" :prefix \"wrfoo-\" :methods [[bar [] String]]) (defn wrfoo-bar [this] \"hi\") [(import 'com.w8reg.Foo) (resolve 'com.w8reg.Foo)]"
        ),
        "[com.w8reg.Foo com.w8reg.Foo]"
    );
    // ... the local shape is unchanged (ctor + prefix-fn dispatch) ...
    assert_eq!(run_last(&mut eval, "(.bar (Foo.))"), "\"hi\"");
    // ... and another namespace imports and uses it through the lattice.
    assert_eq!(
        run_last(
            &mut eval,
            "(in-ns 'w8.other) (clojure.core/import 'com.w8reg.Foo) (.bar (Foo.))"
        ),
        "\"hi\""
    );
}

#[test]
fn w8_genclass_forname() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-class :name \"com.w8reg.Fn\" :prefix \"wrfn-\") (import 'java.lang.Class) (Class/forName \"com.w8reg.Fn\")"
        ),
        "com.w8reg.Fn"
    );
}

#[test]
fn w8_geninterface_dotted_registers() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-interface :name \"com.w8reg.Iface\" :methods [[m [String] String]]) [(import 'com.w8reg.Iface) (resolve 'com.w8reg.Iface)]"
        ),
        "[com.w8reg.Iface com.w8reg.Iface]"
    );
}

#[test]
fn w8_gen_and_load_class() {
    let mut eval = boot();
    // Returns the Class and registers it for import elsewhere.
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-and-load-class :name \"com.w8reg.Loaded\" :prefix \"w8l-\")"
        ),
        "com.w8reg.Loaded"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(in-ns 'w8.luser) (clojure.core/import 'com.w8reg.Loaded)"
        ),
        "com.w8reg.Loaded"
    );
    // Evaluated (Class) specs work as well as quoted (symbol) specs.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-and-load-class :name \"com.w8reg.Ev\" :methods (vector ['m [java.lang.String] java.lang.String]))"
        ),
        "com.w8reg.Ev"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(gen-and-load-class :name \"com.w8reg.Qu\" :methods '[[m [String] String]])"
        ),
        "com.w8reg.Qu"
    );
    // Non-dotted names qualify with the current namespace.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(gen-and-load-class :name \"W8Local\")"),
        "user.W8Local"
    );
    assert_eq!(
        run_last(&mut eval, "(import 'user.W8Local)"),
        "user.W8Local"
    );
}

#[test]
fn w8_gen_and_load_class_errors() {
    let mut eval = boot();
    // Same option contract (and messages) as the macro. Names and
    // specs are strings/quoted data here: as a runtime fn (unlike the
    // macro), bare symbols would fail to resolve before validation.
    assert!(
        uncaught(&mut eval, "(gen-and-load-class :prefix \"x-\")")
            .contains("Must supply :name to gen-class")
    );
    assert!(
        uncaught(&mut eval, "(gen-and-load-class :name \"X\" :bogus 1)")
            .contains("Unsupported gen-class option: :bogus")
    );
    assert!(
        uncaught(
            &mut eval,
            "(gen-and-load-class :name \"X\" :methods '[[m [a]]])"
        )
        .contains("Invalid gen-class method")
    );
    assert!(
        uncaught(&mut eval, "(gen-and-load-class :name \"X\" :prefix 5)")
            .contains("gen-class :prefix must be a string")
    );
    assert!(
        uncaught(
            &mut eval,
            "(gen-and-load-class :name \"X\" :extends 'String)"
        )
        .contains("Cannot extend String: gen-class supports Object only")
    );
    assert!(
        uncaught(&mut eval, "(gen-and-load-class :name 5)")
            .contains("gen-class :name must be a symbol or string")
    );
}

// ---- Part B: clojure.inspector (text redesign) ----

#[test]
fn w8_inspector_require_and_resolve() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(require 'clojure.inspector)"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(count (filter some? (map resolve '[clojure.inspector/atom? clojure.inspector/collection-tag clojure.inspector/is-leaf clojure.inspector/get-child clojure.inspector/get-child-count clojure.inspector/tree-model clojure.inspector/old-table-model clojure.inspector/inspect-tree clojure.inspector/inspect-table clojure.inspector/list-provider clojure.inspector/list-model clojure.inspector/table-model clojure.inspector/inspect])))"
        ),
        "13"
    );
}

#[test]
fn w8_inspector_navigation() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.inspector)").unwrap();
    const I: &str = "clojure.inspector";
    assert_eq!(
        run_last(&mut eval, &format!("[({I}/atom? 1) ({I}/atom? [1])]")),
        "[true false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "[({I}/collection-tag {{:a 1}}) ({I}/collection-tag [1]) ({I}/collection-tag #{{1}}) ({I}/collection-tag 1) ({I}/collection-tag (first {{:a 1}}))]"
            )
        ),
        "[:seqable :seq :seqable :atom :entry]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "[({I}/is-leaf 1) ({I}/is-leaf [1]) ({I}/is-leaf {{:a 1}}) ({I}/is-leaf (first {{:a 1}})) ({I}/is-leaf (first {{:b [1]}}))]"
            )
        ),
        "[true false false true false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("[({I}/get-child [10 20] 1) ({I}/get-child {{:a 1}} 0)]")
        ),
        "[20 [:a 1]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "[({I}/get-child-count [10 20]) ({I}/get-child-count {{:a 1}}) ({I}/get-child-count #{{1 2}})]"
            )
        ),
        "[2 1 2]"
    );
    // `list-provider`: vectors index, maps sort by key, atoms name
    // their class.
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(let [lp ({I}/list-provider [10 20])] [(:nrows lp) ((:get-label lp) 0) ((:get-value lp) 1)])"
            )
        ),
        "[2 0 20]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(let [lp ({I}/list-provider {{:b 2 :a 1}})] [(:nrows lp) ((:get-label lp) 0) ((:get-label lp) 1) ((:get-value lp) 0)])"
            )
        ),
        "[2 :a :b 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(let [lp ({I}/list-provider 42)] [(:nrows lp) ((:get-label lp) 0) ((:get-value lp) 0)])"
            )
        ),
        "[1 \"java.lang.Long\" 42]"
    );
}

#[test]
fn w8_inspector_models() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.inspector)").unwrap();
    const I: &str = "clojure.inspector";
    // Trees render indented, top nodes unindented.
    assert_eq!(
        eval_str(&mut eval, &format!("({I}/tree-model [1 [2]])")),
        "1\n  2\n"
    );
    assert_eq!(
        eval_str(&mut eval, &format!("({I}/tree-model {{:a 1}})")),
        "[:a 1]\n"
    );
    assert_eq!(eval_str(&mut eval, &format!("({I}/tree-model 42)")), "42\n");
    // Tables align columns; map rows name their keys.
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/old-table-model [[1 2 3] [4 5 6]])")
        ),
        "0 1 2\n1 2 3\n4 5 6\n"
    );
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/old-table-model [{{:a 1 :b 2}} {{:a 3 :b 4}}])")
        ),
        "a b\n1 2\n3 4\n"
    );
    // Ragged widths pad out.
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/old-table-model [[1 222] [3333 4]])")
        ),
        "0    1  \n1    222\n3333 4  \n"
    );
    // List models pair labels with printed values.
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/list-model ({I}/list-provider [10 20]))")
        ),
        "0 10\n1 20\n"
    );
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/list-model ({I}/list-provider []))")
        ),
        ""
    );
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/list-model ({I}/list-provider {{:b 2 :a 1}}))")
        ),
        ":a 1\n:b 2\n"
    );
    assert_eq!(
        eval_str(
            &mut eval,
            &format!("({I}/list-model ({I}/list-provider 42))")
        ),
        "java.lang.Long 42\n"
    );
    assert_eq!(
        eval_str(&mut eval, &format!("({I}/table-model 42)")),
        "class java.lang.Long 42\n"
    );
}

#[test]
fn w8_inspector_views() {
    let mut eval = boot();
    eval.eval_source("(require 'clojure.inspector)").unwrap();
    const I: &str = "clojure.inspector";
    // Views print to `*out*` and return nil.
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(def w8o (with-out-str (def w8r ({I}/inspect [1 2])))) [w8o w8r]")
        ),
        "[\"0 1\\n1 2\\n\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(def w8o (with-out-str (def w8r ({I}/inspect-tree {{:a 1}})))) [w8o w8r]")
        ),
        "[\"[:a 1]\\n\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(def w8o (with-out-str (def w8r ({I}/inspect-table [[1 2] [3 4]])))) [w8o w8r]"
            )
        ),
        "[\"0 1\\n1 2\\n3 4\\n\" nil]"
    );
}

// ---- Part B: clojure.java.browse-ui (text redesign) ----

#[test]
fn w8_browse_ui() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require 'clojure.java.browse-ui)"),
        "nil"
    );
    // The entry point keeps its (private) name ...
    assert_eq!(
        run_last(
            &mut eval,
            "(some? (ns-resolve 'clojure.java.browse-ui 'open-url-in-swing))"
        ),
        "true"
    );
    // ... prints the URL to `*out*`, and returns nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(def w8o (with-out-str (def w8r ((deref (ns-resolve 'clojure.java.browse-ui 'open-url-in-swing)) \"http://example.com\")))) [w8o w8r]"
        ),
        "[\"Browsing http://example.com\\n\" nil]"
    );
}
