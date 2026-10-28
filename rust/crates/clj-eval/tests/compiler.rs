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

use std::path::PathBuf;

use clj_eval::{Evaluator, SourceError, boot};
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

/// A per-test fixture dir (unique across parallel tests, removed on
/// drop) holding `*load-path*` sources and `*compile-path*` outputs.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s18-{name}-{}", std::process::id()));
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

    /// The dir as a Clojure string literal.
    fn lit(&self) -> String {
        let s = self.dir.to_string_lossy();
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

/// The add answers nil and prints the verbatim WARNING (`a-warning`,
/// `a-warn-text`, `a-ret`).
#[test]
fn warns_and_returns_nil() {
    let fx = Fixture::new("addcp-warn");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", fx.lit())),
        "nil"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(with-out-str (add-classpath {}))", fx.lit())
        ),
        "\"WARNING: add-classpath is deprecated\\n\""
    );
}

/// Appends in order; the first-added dir wins resolution; later dirs
/// resolve new libs (`a-urls`, `a-order`, `a-effect`, `a-ret`,
/// `a-add2`).
#[test]
fn appends_first_wins() {
    let d1 = Fixture::new("addcp-d1");
    let d2 = Fixture::new("addcp-d2");
    d1.write("my/lib.clj", "(ns my.lib)\n(def marker \"d1\")\n");
    d2.write("my/lib.clj", "(ns my.lib)\n(def marker \"d2\")\n");
    d2.write("new/lib.clj", "(ns new.lib)\n(def marker \"new\")\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d1.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d2.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{} {}]", d1.lit(), d2.lit())
    );
    assert_eq!(
        run_last(&mut eval, "(require 'my.lib) @#'my.lib/marker"),
        "\"d1\""
    );
    assert_eq!(
        run_last(&mut eval, "(require 'new.lib) @#'new.lib/marker"),
        "\"new\""
    );
}

/// Re-adding is a no-op returning nil (`a-dup-ret`, `a-dup-urls`).
#[test]
fn dup_is_noop() {
    let d1 = Fixture::new("addcp-dup1");
    let d2 = Fixture::new("addcp-dup2");
    let mut eval = boot();
    run_last(&mut eval, &format!("(add-classpath {})", d1.lit()));
    run_last(&mut eval, &format!("(add-classpath {})", d2.lit()));
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d1.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{} {}]", d1.lit(), d2.lit())
    );
}

/// Nonexistent dirs are accepted silently (`a-missing`).
#[test]
fn missing_accepted() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(add-classpath \"/nonexistent-s18-dir\")"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        "[\"/nonexistent-s18-dir\"]"
    );
}

/// Non-strings throw CCE `String` after the WARNING
/// (`a-broken-file/num`, port-narrowed).
#[test]
fn nonstr_rejected() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (try (add-classpath 42) (catch Exception _ :caught)))"
        ),
        "\"WARNING: add-classpath is deprecated\\n\""
    );
    assert_eq!(
        uncaught(&mut eval, "(add-classpath 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

/// Arity ≠ 1 throws with no WARNING (`a-arity0/2`; the row-1
/// `compile` sections join this test — §1.10 names both `arity`).
#[test]
fn arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(add-classpath)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/add-classpath"
    );
    assert_eq!(
        uncaught(&mut eval, "(add-classpath \"a\" \"b\")"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/add-classpath"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (try (add-classpath) (catch Exception _ :caught)))"
        ),
        "\"\""
    );
}

/// A nil `*load-path*` root normalizes to `[]` first (append holds).
#[test]
fn nil_path_normalizes() {
    let fx = Fixture::new("addcp-nil");
    let mut eval = boot();
    run_last(&mut eval, "(alter-var-root #'*load-path* (fn [_] nil))");
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", fx.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{}]", fx.lit())
    );
}

/// Non-vector coll roots get `conj` semantics (lists prepend).
#[test]
fn list_root_conj_semantics() {
    let mut eval = boot();
    run_last(
        &mut eval,
        "(alter-var-root #'*load-path* (fn [_] (quote (\"a\"))))",
    );
    assert_eq!(run_last(&mut eval, "(add-classpath \"b\")"), "nil");
    assert_eq!(run_last(&mut eval, "*load-path*"), "(\"b\" \"a\")");
}
