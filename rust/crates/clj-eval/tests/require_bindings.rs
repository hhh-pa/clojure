// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Stdlib-all integration (issue #204): `require` of namespaces whose
//! top level `set!`s a dynamic var.
//!
//! Upstream `clojure.string`/`clojure.set` are vendored verbatim, and
//! `string.clj` opens with `(set! *warn-on-reflection* true)`. `set!`
//! needs a thread-local binding; on the JVM that frame comes from the
//! entry point (`clojure.main/with-bindings`, used by `load-script`,
//! the REPL, and `-e`), never from `require`/`load` itself — and a
//! bare embedded `require` throws `IllegalStateException` there too.
//! This runtime matches that shape exactly: the binary wraps every
//! script in the same bindings (`main.rs`, "vars that often need to
//! be set!"), while a bare harness `Evaluator` has none. So:
//!
//! * inside a `binding` frame, `require` of the verbatim namespaces
//!   works and the loaded vars behave;
//! * a bare `require` throws the JVM-identical ISE — pinned, not
//!   fixed, because fixing it (binding inside `load`) would deviate
//!   from upstream `require` semantics (observable via
//!   `thread-bound?` during load).
//!
//! Harnesses (including the #205 algorithms runner) must evaluate
//! inside `binding`/`with-bindings`, exactly like upstream entry
//! points do.

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

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

#[test]
fn string_and_set_require_inside_main_like_bindings() {
    let mut eval = boot();
    let out = run(
        &mut eval,
        r#"(do
              (binding [*warn-on-reflection* false]
                (require '[clojure.string :as str] '[clojure.set :as set]))
              (= [(str/join "," [1 2 3]) (set/union #{1} #{2})]
                 ["1,2,3" #{1 2}]))"#,
    );
    assert_eq!(out, "true");
}

#[test]
fn bare_require_of_setbang_namespace_throws_like_jvm() {
    let mut eval = boot();
    let err = uncaught(&mut eval, "(require 'clojure.string)");
    assert!(
        err.contains("IllegalStateException"),
        "expected ISE, got: {err}"
    );
    assert!(
        err.contains("Can't change/establish root binding"),
        "expected root-binding message, got: {err}"
    );
}

/// A failed load leaves the upstream-identical residue: the `ns` form
/// self-marks (upstream `core.clj` `ns` macro), the `load-lib` catch
/// removes the half-built ns (upstream `load-lib`), the mark stays,
/// and only `:reload` retries — a plain retry is skipped as loaded.
#[test]
fn failed_require_leaves_jvm_identical_residue_reload_recovers() {
    let mut eval = boot();
    let out = run(
        &mut eval,
        r#"(do
              (try (require 'clojure.string)
                (catch Exception e :threw))
              (let [marked (contains? (loaded-libs) 'clojure.string)
                    gone (nil? (find-ns 'clojure.string))]
                (binding [*warn-on-reflection* false]
                  (require 'clojure.string))
                (let [skipped (nil? (find-ns 'clojure.string))]
                  (binding [*warn-on-reflection* false]
                    (require 'clojure.string :reload))
                  [:threw marked gone skipped
                   (some? (find-ns 'clojure.string))])))"#,
    );
    assert_eq!(out, "[:threw true true true true]");
}
