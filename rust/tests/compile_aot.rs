// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S18 (issue #62) row-1c: real-driver object emission.
//! Gated on the whole file: the MSRV and no-default legs compile
//! this target empty.
//!
//! Hermetic conventions: the driver never links (`--emit-obj`
//! equivalent — no linker, no runtime archive); fixtures live under
//! per-test temp dirs created by the tests; assertions are
//! existence/non-emptiness only, never byte pins; diagnostics assert
//! stable prefixes, never OS error text.

#![cfg(feature = "aot")]

use std::path::{Path, PathBuf};

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

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
        let dir = std::env::temp_dir().join(format!("clj-s18-aot-{name}-{}", std::process::id()));
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
        lit(&self.dir)
    }

    /// The `classes` output dir (created).
    fn classes_dir(&self) -> PathBuf {
        let dir = self.dir.join("classes");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Ensure the output subdir for a load root exists (`compile`
    /// creates no directories — the caller owns the layout).
    fn ensure_obj_dir(&self, root: &str) {
        let mut dir = self.classes_dir();
        if let Some(parent) = Path::new(root).parent() {
            dir = dir.join(parent);
        }
        std::fs::create_dir_all(&dir).unwrap();
    }

    /// Wrap `src` in `*load-path*` + `*compile-path*` bindings to
    /// this dir.
    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}] clojure.core/*compile-path* {}] {src})",
            self.lit(),
            lit(&self.classes_dir())
        )
    }
}

/// A path as a Clojure string literal.
fn lit(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Install the real driver (idempotent and stateless — parallel-safe).
fn use_real_driver() {
    clj_aot::driver::install_driver();
}

fn assert_object(obj: &Path) {
    assert!(obj.is_file(), "no object at {}", obj.display());
    let len = std::fs::metadata(obj).unwrap().len();
    assert!(len > 0, "empty object at {}", obj.display());
}

/// The real driver emits one non-empty object at the §1.4 path.
#[test]
fn emits_object_layout() {
    let fx = Fixture::new("emit");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    fx.ensure_obj_dir("my/plain");
    use_real_driver();
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_object(&fx.classes_dir().join("my/plain.o"));
}

/// Recompiling overwrites cleanly (idempotent, no error).
#[test]
fn overwrite_idempotent() {
    let fx = Fixture::new("overwrite");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    fx.ensure_obj_dir("my/plain");
    use_real_driver();
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_object(&fx.classes_dir().join("my/plain.o"));
}

/// Missing output parents fail as `Io` (no `mkdirs`): `build:
/// {path}: ...` (the OS reason after the prefix is platform text,
/// never pinned).
#[test]
fn no_mkdirs_real() {
    let fx = Fixture::new("nomkdirs");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    use_real_driver();
    let mut eval = boot();
    let deep = fx.dir.join("nested/deep");
    let src = format!(
        "(binding [clojure.core/*load-path* [{}] clojure.core/*compile-path* {}] (compile 'my.plain))",
        fx.lit(),
        lit(&deep)
    );
    let err = uncaught(&mut eval, &src);
    let prefix = format!(
        "java.io.IOException | build: {}: ",
        deep.join("my/plain.o").display()
    );
    assert!(
        err.starts_with(&prefix) && err.len() > prefix.len(),
        "unexpected error: {err}"
    );
}

/// The check failure still leaves the root-derived object behind.
#[test]
fn mismatch_leaves_root_object() {
    let fx = Fixture::new("mismatch");
    fx.write("my/mismatch.clj", "(ns other.thing)\n(defn m [] 1)\n");
    fx.ensure_obj_dir("my/mismatch");
    use_real_driver();
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.mismatch)")),
        "java.lang.Exception | namespace 'my.mismatch' not found after loading '/my/mismatch'"
    );
    assert_object(&fx.classes_dir().join("my/mismatch.o"));
}

/// An `ns`-only file emits a valid (empty) object: lowering accepts
/// the empty program (R5 — this test is the pin).
#[test]
fn empty_ns_only() {
    let fx = Fixture::new("empty");
    fx.write("my/e.clj", "(ns my.e)");
    fx.ensure_obj_dir("my/e");
    use_real_driver();
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(compile 'my.e)")), "my.e");
    assert_object(&fx.classes_dir().join("my/e.o"));
}

/// Plan failures emit nothing and mark nothing (D3, real driver).
#[test]
fn broken_leaves_nothing() {
    let fx = Fixture::new("broken");
    fx.write(
        "my/broken.clj",
        "(ns my.broken)\n(defn f [x] (no-such-fn-xyz x))\n",
    );
    use_real_driver();
    let mut eval = boot();
    let err = uncaught(&mut eval, &fx.within("(compile 'my.broken)"));
    assert!(err.starts_with("clojure.lang.Compiler$CompilerException | "));
    assert!(
        !fx.classes_dir().join("my/broken.o").exists(),
        "stale object emitted"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
}

/// `gen-class` descriptors survive a real compile round-trip.
#[test]
fn genclass_survives_real() {
    let fx = Fixture::new("gcsurv");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    fx.ensure_obj_dir("my/plain");
    use_real_driver();
    let mut eval = boot();
    run_last(
        &mut eval,
        "(gen-class :name user.Klass :prefix \"klass-\" :methods [[hello [] String]]) (defn klass-hello [this] \"yo\")",
    );
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_eq!(
        run_last(&mut eval, "[(.hello (Klass.)) (str Klass)]"),
        "[\"yo\" \"user.Klass\"]"
    );
    assert_object(&fx.classes_dir().join("my/plain.o"));
}
