// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Stdlib-all W7 (tools track, issue #204): smoke tests for the four
//! ported namespaces — `clojure.java.basis`,
//! `clojure.java.basis.impl`, `clojure.tools.deps.interop`, and
//! `clojure.repl.deps`.
//!
//! Oracle (text only, never executed): upstream `clojure/clojure`
//! `src/clj/clojure/{java/basis,java/basis/impl,tools/deps/interop,
//! repl/deps}.clj`. Each test cites the upstream behavior it pins.
//! Divergences: resolution runs in-process over the already-embedded
//! tools.deps namespaces (no CLI subprocess), added libs land on
//! `*load-path*` (no `DynamicClassLoader`), and
//! `RuntimeException.` sites throw `ex-info` with identical
//! messages (the spec_alpha.clj precedent).
//!
//! Each test boots its own `Evaluator`, so the basis delays and the
//! `*load-path*` root cannot leak across tests; every filesystem
//! case uses a unique `scratch()` dir. Multi-element collections
//! assert by in-eval `=` (order-free), never by pr-string order.

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

/// A Rust string as a Clojure string literal.
fn clj_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Require the four W7 namespaces (plus the `port` substrate for
/// envelope parsing in the interop tests).
fn require_w7(eval: &mut Evaluator) {
    run(
        eval,
        "(require '[clojure.java.basis :as basis]
                  '[clojure.java.basis.impl :as bimpl]
                  '[clojure.tools.deps.interop :as tool]
                  '[clojure.tools.deps.util.port :as port]
                  '[clojure.repl.deps :as rd])",
    );
}

/// A scratch dir unique to this test (parallel-safe), wiped first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clj-w7-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `rel` under `dir`, returning the absolute path string.
fn write_dep(dir: &std::path::Path, rel: &str, content: &str) -> String {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

/// Write a lib dir `root/name` with `deps.edn` content plus a `src/`
/// dir, returning the lib root path string.
fn write_lib(root: &std::path::Path, name: &str, content: &str) -> String {
    let lib = root.join(name);
    std::fs::create_dir_all(lib.join("src")).unwrap();
    write_dep(&lib, "deps.edn", content);
    lib.to_string_lossy().into_owned()
}

// ---- load (ns.rs resolve_source) ----

#[test]
fn w7_namespaces_load_via_require() {
    // The loader serves the four new embedded libs; the dep chain
    // (basis -> impl -> port; interop standalone; repl.deps ->
    // basis + impl + interop) resolves without touching
    // `*load-path*`.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(do (require 'clojure.java.basis 'clojure.java.basis.impl
                         'clojure.tools.deps.interop 'clojure.repl.deps) :ok)"
        ),
        ":ok"
    );
}

// ---- clojure.java.basis ----

#[test]
fn w7_basis_nil_without_cli() {
    // Upstream `initial-basis`: "nil if unknown (process not started
    // by CLI)". The port answers nil for the `clojure.basis`
    // property, so both entry points are nil on a fresh evaluator.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(run(&mut eval, "(basis/initial-basis)"), "nil");
    assert_eq!(run(&mut eval, "(basis/current-basis)"), "nil");
}

#[test]
fn w7_basis_update_round_trip() {
    // `update-basis!` applies f to the current basis (an atom behind
    // the delay); `initial-basis` still answers the launch value.
    let mut eval = boot();
    require_w7(&mut eval);
    run(
        &mut eval,
        "(bimpl/update-basis! (fn [_] {:libs {'a/a {:local/root \"/x\"}}}))",
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (basis/current-basis) {:libs {'a/a {:local/root \"/x\"}}})"
        ),
        "true"
    );
    assert_eq!(run(&mut eval, "(basis/initial-basis)"), "nil");
    run(
        &mut eval,
        "(bimpl/update-basis! update :libs merge {'b/b {:local/root \"/y\"}})",
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:libs (basis/current-basis)) {'a/a {:local/root \"/x\"} 'b/b {:local/root \"/y\"}})"
        ),
        "true"
    );
}

#[test]
fn w7_basis_reads_single_value_edn_file() {
    // The port `read-edn` (slurp + `port/read-edn-string`) answers
    // the file's single value and rejects a second one, like the
    // upstream two-read `PushbackReader` shape. Reached through the
    // private var (public API has no file entry; the property is
    // unset in this runtime).
    let mut eval = boot();
    require_w7(&mut eval);
    let dir = scratch("basis-file");
    let one = write_dep(&dir, "basis.edn", "{:libs {a/a {:local/root \"/x\"}}}");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (@#'clojure.java.basis.impl/read-basis {}) {{:libs {{'a/a {{:local/root \"/x\"}}}}}})",
                clj_str(&one)
            )
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(@#'clojure.java.basis.impl/read-basis nil)"),
        "nil"
    );
    let missing = dir.join("nope.edn").to_string_lossy().into_owned();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(@#'clojure.java.basis.impl/read-basis {})",
                clj_str(&missing)
            )
        ),
        "nil"
    );
    let two = write_dep(&dir, "two.edn", "{} {}");
    assert!(
        uncaught(
            &mut eval,
            &format!("(@#'clojure.java.basis.impl/read-basis {})", clj_str(&two))
        )
        .starts_with("clojure.lang.ExceptionInfo | "),
        "second value must throw ex-info"
    );
}

// ---- clojure.tools.deps.interop ----

#[test]
fn w7_interop_validates_tool_selection() {
    // Upstream `invoke-tool` guards, verbatim messages.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(tool/invoke-tool {:fn 'clojure.core/identity})"),
        "clojure.lang.ExceptionInfo | Either :tool-alias or :tool-name must be provided"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(tool/invoke-tool {:tool-alias :deps :fn \"nope\"})"
        ),
        "clojure.lang.ExceptionInfo | :fn should be a symbol nope"
    );
}

#[test]
fn w7_interop_dispatches_in_process() {
    // No CLI subprocess: the `:fn` resolves against the embedded
    // namespaces and runs with the args map (which still carries
    // `:clojure.exec/invoke :fn`, exactly what the CLI hands over).
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(= (tool/invoke-tool {:tool-alias :deps :fn 'clojure.core/identity :args {:hi :there}})
                {:hi :there :clojure.exec/invoke :fn})"
        ),
        "true"
    );
}

#[test]
fn w7_interop_preserve_envelope_round_trip() {
    // `:preserve-envelope true` answers the emulated CLI envelope:
    // `{:tag :ret, :val (pr-str ret)}` on success, `{:tag :err,
    // :val ...}` (cause + data) on tool failure.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [resp (tool/invoke-tool {:tool-alias :deps :fn 'clojure.core/identity
                                           :args {:hi :there} :preserve-envelope true})]
               [(= (:tag resp) :ret)
                (= (port/read-edn-string (:val resp) \"t\") {:hi :there :clojure.exec/invoke :fn})])"
        ),
        "[true true]"
    );
    run(
        &mut eval,
        "(defn boom [m] (throw (ex-info \"boom-x\" {:k 1})))",
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (tool/invoke-tool {:tool-alias :deps :fn 'user/boom :args {} :preserve-envelope true})
                {:tag :err :val (pr-str {:cause \"boom-x\" :data {:k 1}})})"
        ),
        "true"
    );
    // Without the envelope the tool error itself propagates (the CLI
    // would re-wrap it in ex-info; in-process there is nothing to
    // re-wrap across).
    assert_eq!(
        uncaught(
            &mut eval,
            "(tool/invoke-tool {:tool-alias :deps :fn 'user/boom :args {}})"
        ),
        "clojure.lang.ExceptionInfo | boom-x"
    );
}

#[test]
fn w7_interop_unknown_fn_throws() {
    // An unresolvable tool fn (here the X1-excluded
    // `find-latest-version`) throws ex-info naming it.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        uncaught(
            &mut eval,
            "(tool/invoke-tool {:tool-alias :deps :fn 'clojure.tools.deps/find-latest-version
                                :args {:lib 'a/a}})"
        ),
        "clojure.lang.ExceptionInfo | Tool fn is not available in this runtime: clojure.tools.deps/find-latest-version"
    );
}

// ---- clojure.repl.deps ----

#[test]
fn w7_add_libs_requires_repl() {
    // Upstream guard, identical message via ex-info (the
    // spec_alpha.clj `RuntimeException.` precedent).
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(rd/add-libs {'a/a {:local/root \"/x\"}})"),
        "clojure.lang.ExceptionInfo | add-libs is only available at the REPL"
    );
}

#[test]
fn w7_add_lib_local_lib_end_to_end() {
    // `add-lib` of a `:local/root` lib: resolves in-process, lands
    // the lib src dir on `*load-path*` (the classloader step),
    // merges into the basis, and answers the sorted added-lib
    // vector. A second add of the same lib answers nil ("Libs
    // already on the classpath are not updated").
    let root = scratch("add-lib");
    let lib = write_lib(&root, "lib", "{:paths [\"src\"]}");
    write_dep(
        &root.join("lib"),
        "src/acme/lib.clj",
        "(ns acme.lib) (defn answer [] 42)",
    );
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(binding [*repl* true] (rd/add-lib 'acme/lib {{:local/root {}}}))",
                clj_str(&lib)
            )
        ),
        "[acme/lib]"
    );
    // The new src dir is require-able through the loader now.
    assert_eq!(
        run(&mut eval, "(do (require 'acme.lib) (acme.lib/answer))"),
        "42"
    );
    // Basis merged; the lib is known now, so re-adding is a nil.
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:libs (basis/current-basis)) 'acme/lib)"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(binding [*repl* true] (rd/add-lib 'acme/lib {{:local/root {}}}))",
                clj_str(&lib)
            )
        ),
        "nil"
    );
}

#[test]
fn w7_add_lib_single_arity_throws_without_mvn() {
    // The 1-arity form needs `find-latest-version` (network `:mvn`),
    // excluded from the embedded tools.deps (gap-124 X1): the tool
    // dispatch throws before any version lookup.
    let mut eval = boot();
    require_w7(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(binding [*repl* true] (rd/add-lib 'acme/lib))"),
        "clojure.lang.ExceptionInfo | Tool fn is not available in this runtime: clojure.tools.deps/find-latest-version"
    );
}

#[test]
fn w7_sync_deps_adds_missing_libs() {
    // `sync-deps` re-runs `create-basis` from the stored
    // `:basis-config` (nil root/user sources keep the test
    // hermetic) and adds whatever the project declares.
    let root = scratch("sync-deps");
    let lib = write_lib(&root, "other", "{:paths [\"src\"]}");
    write_dep(
        &root.join("other"),
        "src/acme/other.clj",
        "(ns acme.other) (defn answer [] 7)",
    );
    let proj = write_dep(
        &root,
        "deps.edn",
        &format!("{{:deps {{acme/other {{:local/root {}}}}}}}", clj_str(&lib)),
    );
    let mut eval = boot();
    require_w7(&mut eval);
    run(
        &mut eval,
        &format!(
            "(bimpl/update-basis! (fn [_] {{:basis-config {{:root nil :user nil :project {}}} :libs {{}}}}))",
            clj_str(&proj)
        ),
    );
    assert_eq!(
        run(&mut eval, "(binding [*repl* true] (rd/sync-deps))"),
        "[acme/other]"
    );
    assert_eq!(
        run(&mut eval, "(do (require 'acme.other) (acme.other/answer))"),
        "7"
    );
}
