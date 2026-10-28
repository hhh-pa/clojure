// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S18 (issue #62) compiler-surface rows: the ten matrix
//! rows' covering tests (spec §1.10), the fake-driver LOCAL suite
//! (gating, paths, the T1–T12 taxonomy mapping, atomicity,
//! gen-class survival), and the loom mirror of the driver registry.
//!
//! Port adaptations (spec-directed, never silent): artifacts are one
//! root-derived `.o` (never JVM `.class`); plan-first atomicity (no
//! partial marks); `*compile-path*` root `"classes"` (folded
//! launcher binding); `add-classpath` appends strings to
//! `*load-path*`; `primitives-classnames` keys are class-name
//! strings; cast texts are the port's stable `Cannot cast` shape.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
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
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    pr(&realized(eval, v))
}

/// `*compile-path*` root (folded `"classes"`), slot-only dynamics
/// (no `:dynamic` meta), and rebinding (`k-compile-path`,
/// `k-root-compile-path`, `k-dyn-compile-path`, `k-bind-compile-path`).
#[test]
fn root_classes_and_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compile-path*"), "\"classes\"");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compile-path*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*compile-path* \"/tmp/x\"] *compile-path*)"
        ),
        "\"/tmp/x\""
    );
}

/// `*compile-files*` root false, slot-only (`k-compile-files`,
/// `k-root-compile-files`, `k-dyn-compile-files`).
#[test]
fn root_false_and_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compile-files*"), "false");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compile-files*))"),
        "nil"
    );
}

/// `*compiler-options*` and `*fn-loader*` roots nil, slot-only,
/// rebindable (`k-compiler-options`, `k-root-compiler-options`,
/// `k-dyn-compiler-options`, `k-bind-options`, `k-root-fn-loader`,
/// `k-dyn-fn-loader` — one test: §1.10 names both rows
/// `root_nil_slot_only_and_binds`).
#[test]
fn root_nil_slot_only_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compiler-options*"), "nil");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compiler-options*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*compiler-options* {:a 1}] *compiler-options*)"
        ),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "*fn-loader*"), "nil");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*fn-loader*))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*fn-loader* 7] *fn-loader*)"),
        "7"
    );
}

/// `*source-path*` root `"NO_SOURCE_FILE"`, slot-only
/// (`k-root-source-path`, `k-dyn-source-path`).
#[test]
fn root_no_source_file_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*source-path*"), "\"NO_SOURCE_FILE\"");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*source-path*))"),
        "nil"
    );
}

/// `*use-context-classloader*` root true, slot-only, rebindable
/// (`k-use-context`, `k-root-use-context`, `k-dyn-use-context`,
/// `k-bind-use-context`).
#[test]
fn root_true_slot_only_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*use-context-classloader*"), "true");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*use-context-classloader*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*use-context-classloader* false] *use-context-classloader*)"
        ),
        "false"
    );
}

/// `*verbose-defrecords*` root false, `:dynamic` true, rebindable
/// (`k-verbose`, `k-root-verbose`, `k-dyn-verbose`).
#[test]
fn root_false_dynamic_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*verbose-defrecords*"), "false");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*verbose-defrecords*))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*verbose-defrecords* true] *verbose-defrecords*)"
        ),
        "true"
    );
}

/// `primitives-classnames` count, values, string keys, lookup, and
/// plain-def meta (`k-prims-count/vals/keys/lookup`, `k-dyn-prims`).
#[test]
fn shape_count_vals_keys() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(count primitives-classnames)"), "8");
    assert_eq!(
        run_last(&mut eval, "(sort (vals primitives-classnames))"),
        "(\"Boolean/TYPE\" \"Byte/TYPE\" \"Character/TYPE\" \"Double/TYPE\" \"Float/TYPE\" \"Integer/TYPE\" \"Long/TYPE\" \"Short/TYPE\")"
    );
    assert_eq!(
        run_last(&mut eval, "(sort (keys primitives-classnames))"),
        "(\"boolean\" \"byte\" \"char\" \"double\" \"float\" \"int\" \"long\" \"short\")"
    );
    assert_eq!(
        run_last(&mut eval, "(primitives-classnames \"long\")"),
        "\"Long/TYPE\""
    );
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'primitives-classnames))"),
        "nil"
    );
}

/// Unknown classes miss to nil (`k-prims-miss`, derived shape).
#[test]
fn miss_is_nil() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(primitives-classnames \"java.lang.String\")"),
        "nil"
    );
}
