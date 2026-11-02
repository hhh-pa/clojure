// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W1 `clojure.datafy` smoke tests: require + `datafy`/`nav` against
//! upstream-documented behavior (identity default with
//! `:clojure.datafy/obj`+`:clojure.datafy/class` metadata on
//! transformed IObjs; Throwable/IRef/Namespace/Class branches; nav
//! returning v).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

/// Eval, returning the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

fn require_datafy(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.datafy :as df])")
        .unwrap();
}

#[test]
fn datafy_identity_default() {
    let mut eval = boot();
    require_datafy(&mut eval);
    // Unknown values datafy to themselves (upstream Datafiable
    // Object/nil default), with no metadata attached.
    assert_eq!(run(&mut eval, "(df/datafy 42)"), "42");
    assert_eq!(run(&mut eval, "(df/datafy \"s\")"), "\"s\"");
    assert_eq!(run(&mut eval, "(df/datafy [1 2])"), "[1 2]");
    assert_eq!(run(&mut eval, "(df/datafy {:a 1})"), "{:a 1}");
    assert_eq!(run(&mut eval, "(df/datafy nil)"), "nil");
    assert_eq!(run(&mut eval, "(meta (df/datafy [1 2]))"), "nil");
}

#[test]
fn datafy_atom() {
    let mut eval = boot();
    require_datafy(&mut eval);
    // IRef branch: [deref] carrying the ref's own metadata plus
    // ::obj/::class describing the original.
    assert_eq!(run(&mut eval, "(df/datafy (atom 5))"), "[5]");
    assert_eq!(
        run(
            &mut eval,
            "(:clojure.datafy/class (meta (df/datafy (atom 5))))"
        ),
        "clojure.lang.Atom"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom 5)] (identical? (:clojure.datafy/obj (meta (df/datafy a))) a))"
        ),
        "true"
    );
    // NOTE: upstream also propagates the ref's own metadata onto
    // the datafied value, but the port's atoms are not IObj
    // (with-meta on an atom throws ClassCastException), so no atom
    // can carry metadata to propagate — pre-existing core gap, out
    // of W1 scope.
}

#[test]
fn datafy_throwable() {
    let mut eval = boot();
    require_datafy(&mut eval);
    assert_eq!(
        run(&mut eval, "(:cause (df/datafy (ex-info \"boom\" {:k 1})))"),
        "\"boom\""
    );
    assert_eq!(
        run(&mut eval, "(:data (df/datafy (ex-info \"boom\" {:k 1})))"),
        "{:k 1}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(:clojure.datafy/class (meta (df/datafy (ex-info \"boom\" {}))))"
        ),
        "clojure.lang.ExceptionInfo"
    );
}

#[test]
fn datafy_namespace() {
    let mut eval = boot();
    require_datafy(&mut eval);
    eval.eval_source("(def datafy-ns-probe 1)").unwrap();
    assert_eq!(
        run(&mut eval, "(:name (df/datafy (find-ns 'user)))"),
        "\"user\""
    );
    // Sorted maps of publics/imports/interns.
    assert_eq!(
        run(
            &mut eval,
            "(sorted? (:publics (df/datafy (find-ns 'user))))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(get (:publics (df/datafy (find-ns 'user))) 'datafy-ns-probe)"
        ),
        "#'user/datafy-ns-probe"
    );
    assert_eq!(
        run(
            &mut eval,
            "(get (:imports (df/datafy (find-ns 'user))) 'String)"
        ),
        "java.lang.String"
    );
    assert_eq!(
        run(
            &mut eval,
            "(:clojure.datafy/class (meta (df/datafy (find-ns 'user))))"
        ),
        "clojure.lang.Namespace"
    );
}

#[test]
fn datafy_class() {
    let mut eval = boot();
    require_datafy(&mut eval);
    // Class branch: the reflect map plus :name, members grouped by
    // name into a sorted map.
    assert_eq!(
        run(&mut eval, "(:name (df/datafy (class 1)))"),
        "java.lang.Long"
    );
    assert_eq!(
        run(&mut eval, "(sorted? (:members (df/datafy (class 1))))"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(:clojure.datafy/class (meta (df/datafy (class 1))))"
        ),
        "java.lang.Class"
    );
}

#[test]
fn datafy_nav_returns_v() {
    let mut eval = boot();
    require_datafy(&mut eval);
    // Upstream Navigable default: return v.
    assert_eq!(run(&mut eval, "(df/nav {:a 1} :a 1)"), "1");
    assert_eq!(run(&mut eval, "(df/nav [1 2] 0 1)"), "1");
    assert_eq!(run(&mut eval, "(df/nav nil nil :x)"), ":x");
}
