// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5A2 loader: probe36 replay against the booted core with
//! `*load-path*` fixtures under a per-test temp dir (the port has no
//! `clojure.string` yet, so libs are tiny `in-ns`/`def` files).

use std::path::PathBuf;

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

fn uncaught_chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let Err(SourceError::Uncaught(t)) = eval.eval_source(src) else {
        panic!("expected an uncaught throw for {src}");
    };
    let mut out = Vec::new();
    let mut cur = Some(&t);
    while let Some(th) = cur {
        out.push(format!(
            "{} | {}",
            th.class.jvm_name(),
            th.message.as_deref().unwrap_or("")
        ));
        cur = th.cause.as_deref();
    }
    out
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s5a2-{}-{name}", std::process::id()));
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

    /// Wrap `src` in a `*load-path*` binding to this dir
    /// (qualified: the current namespace is often a fresh fixture).
    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}]] {src})",
            self.lit()
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn require_marks_and_skips() {
    let fx = Fixture::new("marks");
    fx.write("tlib.clj", "(clojure.core/in-ns 'tlib)\n(def answer 42)\n");
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(require 'tlib)")), "nil");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{tlib}");
    assert!(run_last(&mut eval, "(find-ns 'tlib)").contains("tlib"));
    assert_eq!(run_last(&mut eval, "tlib/answer"), "42");
    // Already loaded: skipped, still nil.
    assert_eq!(run_last(&mut eval, &fx.within("(require 'tlib)")), "nil");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{tlib}");
    // `*pending-paths*` is private: read it through `var` (unchecked,
    // like the JVM) plus `var-get`, not a direct cross-ns reference.
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*pending-paths*))"),
        "()"
    );
}

#[test]
fn require_as_and_refer() {
    let fx = Fixture::new("asrefer");
    fx.write("rlib.clj", "(clojure.core/in-ns 'rlib)\n(def answer 7)\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require '[rlib :as r])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "r/answer"), "7");
    // `:refer` installs into the current namespace.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require '[rlib :refer [answer]])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "answer"), "7");
    // `:refer :all` too.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require '[rlib :refer :all])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "answer"), "7");
}

#[test]
fn require_missing() {
    let fx = Fixture::new("missing");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require 'no.such.lib)")),
        "java.io.FileNotFoundException | Could not locate no/such/lib__init.class, no/such/lib.clj or no/such/lib.cljc on classpath."
    );
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require 'my-lib.sub)")),
        "java.io.FileNotFoundException | Could not locate my_lib/sub__init.class, my_lib/sub.clj or my_lib/sub.cljc on classpath. Please check that namespaces with dashes use underscores in the Clojure file name."
    );
    // A failed load marks nothing and destroys nothing.
    assert_eq!(run_last(&mut eval, "(find-ns 'no.such.lib)"), "nil");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
}

#[test]
fn require_prefix_lists() {
    let fx = Fixture::new("prefix");
    fx.write("pfx/a.clj", "(clojure.core/in-ns 'pfx.a)\n(def av 1)\n");
    fx.write("pfx/b.clj", "(clojure.core/in-ns 'pfx.b)\n(def bv 2)\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require '(pfx [a :as pa] b))")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "pa/av"), "1");
    assert_eq!(run_last(&mut eval, "pfx.b/bv"), "2");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{pfx.a pfx.b}");
}

#[test]
fn use_shapes() {
    let fx = Fixture::new("use");
    fx.write(
        "ulib.clj",
        "(clojure.core/in-ns 'ulib)\n(def a 1)\n(def b 2)\n",
    );
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(use '[ulib :only [a]])")),
        "nil"
    );
    assert!(run_last(&mut eval, "(ns-refers 'user)").contains("a #'ulib/a"));
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(use '[ulib :exclude [a]])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "b"), "2");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(use '[ulib :rename {a a2}])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "a2"), "1");
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(use 'ulib)")), "nil");
    assert_eq!(run_last(&mut eval, "a"), "1");
    assert_eq!(run_last(&mut eval, "b"), "2");
    // `use` honors `:as` too (probe36 use-with-as).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(use '[ulib :as u :only [a]])")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "u/b"), "2");
    assert_eq!(run_last(&mut eval, "a"), "1");
}

#[test]
fn load_mismatch_and_bare_require() {
    let fx = Fixture::new("mismatch");
    fx.write("nomark.clj", "(def nx 1)\n");
    fx.write("nomark2.clj", "(def nx 1)\n");
    // `:as` against a file that provides no namespace fails naming
    // the root (probe36 req-mismatch-as).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require '[nomark :as n])")),
        "java.lang.Exception | namespace 'nomark' not found after loading '/nomark'"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    // A bare-symbol require never checks — it marks (probe36
    // req-mismatch-plain), and the stray `def` landed in `user`.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(require 'nomark2)")), "nil");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{nomark2}");
    assert_eq!(run_last(&mut eval, "user/nx"), "1");
}

#[test]
fn require_stale_mark() {
    let fx = Fixture::new("stale");
    fx.write("stlib.clj", "(clojure.core/in-ns 'stlib)\n(def x 1)\n");
    let mut eval = boot();
    run(&mut eval, &fx.within("(require 'stlib)"));
    run(&mut eval, "(remove-ns 'stlib)");
    // Still marked, so the load skips and `:as` finds nothing
    // (probe36 req-stale — no `after loading` suffix).
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require '[stlib :as s])")),
        "java.lang.Exception | namespace 'stlib' not found"
    );
}

#[test]
fn reload_all_marks_nested_loads() {
    let fx = Fixture::new("reloadall");
    fx.write(
        "ra_inner.clj",
        "(clojure.core/in-ns 'ra_inner)\n(def iv 1)\n",
    );
    fx.write(
        "ra_outer.clj",
        "(clojure.core/in-ns 'ra_outer)\n(clojure.core/require 'ra_inner)\n(def ov 2)\n",
    );
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require 'ra_outer :reload-all)")),
        "nil"
    );
    // Nested marks accumulate through the `load-all` frame (a
    // root-writing mark would leave this empty).
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{ra_inner ra_outer}");
    assert_eq!(run_last(&mut eval, "ra_inner/iv"), "1");
    assert_eq!(run_last(&mut eval, "ra_outer/ov"), "2");
}

#[test]
fn require_as_under_ns_star_binding() {
    let fx = Fixture::new("bindas");
    fx.write(
        "bindlib.clj",
        "(clojure.core/in-ns 'bindlib)\n(def answer 7)\n",
    );
    let mut eval = boot();
    run(&mut eval, "(create-ns 'bw)");
    // The `:as` alias lands in the BOUND namespace (the
    // `load-lib` finish reads the synced `*ns*`).
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*ns* (the-ns 'bw)] (require '[bindlib :as b]))")
        ),
        "nil"
    );
    assert!(run_last(&mut eval, "(ns-aliases 'bw)").contains("b #object[clojure.lang.Namespace"));
    assert_eq!(run_last(&mut eval, "(ns-resolve 'user 'b/answer)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'bw 'b/answer)"),
        "#'bindlib/answer"
    );
}

#[test]
fn require_reload() {
    let fx = Fixture::new("reload");
    fx.write("cnt.clj", "(clojure.core/in-ns 'cnt)\n(def n 1)\n");
    let mut eval = boot();
    run(&mut eval, &fx.within("(require 'cnt)"));
    assert_eq!(run_last(&mut eval, "cnt/n"), "1");
    run(&mut eval, "(ns-unmap 'cnt 'n)");
    assert_eq!(run_last(&mut eval, "(ns-resolve 'cnt 'n)"), "nil");
    // A plain re-require skips (still gone).
    run(&mut eval, &fx.within("(require 'cnt)"));
    assert_eq!(run_last(&mut eval, "(ns-resolve 'cnt 'n)"), "nil");
    // `:reload` re-evaluates the file.
    run(&mut eval, &fx.within("(require 'cnt :reload)"));
    assert_eq!(run_last(&mut eval, "cnt/n"), "1");
    // `:reload-all` agrees (the embedded set is flat).
    run(&mut eval, "(ns-unmap 'cnt 'n)");
    run(&mut eval, &fx.within("(require 'cnt :reload-all)"));
    assert_eq!(run_last(&mut eval, "cnt/n"), "1");
}

#[test]
fn require_flags_and_libspec_errors() {
    let fx = Fixture::new("libspec");
    fx.write("flg.clj", "(clojure.core/in-ns 'flg)\n(def x 1)\n");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(require)"),
        "java.lang.Exception | Nothing specified to load"
    );
    assert_eq!(
        uncaught(&mut eval, "(require :reload)"),
        "java.lang.Exception | Nothing specified to load"
    );
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require 'flg :bogus)")),
        "java.lang.Exception | Unsupported option(s) supplied: :bogus"
    );
    assert_eq!(
        uncaught(&mut eval, "(require \"xy\")"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Character"
    );
    assert_eq!(
        uncaught(&mut eval, "(require 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(require '[5 :as x])"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Named"
    );
    assert_eq!(
        uncaught(&mut eval, "(require '[nil :as x])"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(require '(aa [bb.cc]))"),
        "java.lang.Exception | Found lib name 'bb.cc' containing period with prefix 'aa'.  lib names inside prefix lists must not contain periods"
    );
    assert_eq!(
        uncaught(&mut eval, "(require '(nil foo))"),
        "java.lang.Exception | prefix cannot be nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(require '[flg :as])"),
        "java.lang.IllegalArgumentException | No value supplied for key: true"
    );
    // Top-level `:as` is a supported flag, so it aliases `true`
    // down the general path (probe36 req-opts-bare).
    let mut eval = boot();
    run(&mut eval, &fx.within("(require 'flg)"));
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require 'flg :as)")),
        "java.lang.ClassCastException | Cannot cast java.lang.Boolean to clojure.lang.Symbol"
    );
}

#[test]
fn as_alias_creates_without_loading() {
    let fx = Fixture::new("asalias");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(require '[ghost.lib :as-alias g])")),
        "nil"
    );
    assert!(run_last(&mut eval, "(find-ns 'ghost.lib)").contains("ghost.lib"));
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert!(run_last(&mut eval, "(ns-aliases 'user)").contains("g #object[clojure.lang.Namespace"));
}

#[test]
fn load_roots() {
    let fx = Fixture::new("roots");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(load \"x-nofile\")")),
        "java.io.FileNotFoundException | Could not locate x-nofile__init.class, x-nofile.clj or x-nofile.cljc on classpath."
    );
    run(&mut eval, "(clojure.core/in-ns 'aa.bb)");
    assert_eq!(
        uncaught(&mut eval, &fx.within("(clojure.core/load \"y-nofile\")")),
        "java.io.FileNotFoundException | Could not locate aa/y-nofile__init.class, aa/y-nofile.clj or aa/y-nofile.cljc on classpath."
    );
    // Extensions never strip (probe36 load-ext).
    assert_eq!(
        uncaught(&mut eval, &fx.within("(clojure.core/load \"x.clj\")")),
        "java.io.FileNotFoundException | Could not locate aa/x.clj__init.class, aa/x.clj.clj or aa/x.clj.cljc on classpath."
    );
    assert_eq!(run_last(&mut eval, "(clojure.core/load)"), "nil");
}

#[test]
fn load_file_and_load_string() {
    let fx = Fixture::new("filefns");
    fx.write("lffile.clj", "(def lff *file*)\n");
    fx.write("lfn.clj", "(clojure.core/in-ns 'lfn)\n");
    let mut eval = boot();
    // `*file*` is the path as opened (here the joined temp path).
    let rel = format!("{}/lffile.clj", fx.dir.to_string_lossy().replace('\\', "/"));
    run(&mut eval, &format!("(load-file \"{rel}\")"));
    assert_eq!(run_last(&mut eval, "lff"), format!("\"{rel}\""));
    // `load-file` does not isolate `*ns*` (Compiler.loadFile has no
    // push): the file's `in-ns` escapes.
    let rel = format!("{}/lfn.clj", fx.dir.to_string_lossy().replace('\\', "/"));
    run(&mut eval, &format!("(load-file \"{rel}\")"));
    assert_eq!(
        run_last(&mut eval, "(clojure.core/ns-name clojure.core/*ns*)"),
        "lfn"
    );
    // `load-string` binds `*file*` nil.
    let mut eval = boot();
    run(&mut eval, "(load-string \"(def lsv 7) (def lsf *file*)\")");
    assert_eq!(run_last(&mut eval, "lsv"), "7");
    assert_eq!(run_last(&mut eval, "lsf"), "nil");
    // Load failures surface as a real `CompilerException` wrapper
    // with the structured cause (probe36 loadstring/loadfile
    // errors, S5D control chains).
    assert_eq!(
        uncaught_chain(&mut eval, "(load-string \"(if)\")"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error compiling if at (1:1)."
                .to_owned(),
            "java.lang.RuntimeException | Too few arguments to if".to_owned(),
        ]
    );
}

#[test]
fn load_file_missing_is_portable_prefix() {
    let mut eval = boot();
    // The tail is the OS message (varies by platform and locale, so
    // the test pins the class plus the path prefix; probe36
    // `loadfile-missing` pins the macOS/Linux text exactly).
    let err = uncaught(&mut eval, "(load-file \"no-such-dir-36/no.clj\")");
    assert!(
        err.starts_with("java.io.FileNotFoundException | no-such-dir-36/no.clj ("),
        "unexpected: {err}"
    );
}

#[test]
fn load_failure_texts() {
    let fx = Fixture::new("loaderr");
    fx.write("bad.clj", "(if)\n");
    fx.write("broken.clj", "(def x [1 2)\n");
    let mut eval = boot();
    let bad = format!("{}/bad.clj", fx.dir.to_string_lossy().replace('\\', "/"));
    assert_eq!(
        uncaught_chain(&mut eval, &format!("(load-file \"{bad}\")")),
        vec![
            format!(
                "clojure.lang.Compiler$CompilerException | Syntax error compiling if at ({bad}:1:1)."
            ),
            "java.lang.RuntimeException | Too few arguments to if".to_owned(),
        ]
    );
    let broken = format!("{}/broken.clj", fx.dir.to_string_lossy().replace('\\', "/"));
    assert_eq!(
        uncaught_chain(&mut eval, &format!("(load-file \"{broken}\")")),
        vec![
            format!(
                "clojure.lang.Compiler$CompilerException | Syntax error reading source at ({broken}:1:13)."
            ),
            "java.lang.RuntimeException | Unmatched delimiter: )".to_owned(),
        ]
    );
}

#[test]
fn cyclic_loads_fail_clean() {
    let fx = Fixture::new("cyclic");
    fx.write(
        "cyc/a.clj",
        "(clojure.core/in-ns 'cyc.a)\n(clojure.core/require 'cyc.b)\n",
    );
    fx.write(
        "cyc/b.clj",
        "(clojure.core/in-ns 'cyc.b)\n(clojure.core/require 'cyc.a)\n",
    );
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(require 'cyc.a)")),
        "java.lang.Exception | Cyclic load dependency: [ /cyc/a ]->/cyc/b->[ /cyc/a ]"
    );
    // Both half-made namespaces are unwound, nothing marked.
    assert_eq!(run_last(&mut eval, "(find-ns 'cyc.a)"), "nil");
    assert_eq!(run_last(&mut eval, "(find-ns 'cyc.b)"), "nil");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    // `*pending-paths*` is private: read it through `var` (unchecked,
    // like the JVM) plus `var-get`, not a direct cross-ns reference.
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*pending-paths*))"),
        "()"
    );
}

#[test]
fn self_load_is_exempt() {
    let fx = Fixture::new("selfload");
    fx.write(
        "selfl.clj",
        "(clojure.core/in-ns 'selfl)\n(clojure.core/load \"/selfl\")\n(def sx 42)\n",
    );
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(require 'selfl)")), "nil");
    assert_eq!(run_last(&mut eval, "selfl/sx"), "42");
}

#[test]
fn load_isolates_ns() {
    let fx = Fixture::new("isolate");
    fx.write("iso.clj", "(clojure.core/in-ns 'iso)\n(def ix 1)\n");
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(load \"/iso\")")), "nil");
    assert_eq!(run_last(&mut eval, "(ns-name *ns*)"), "user");
    assert_eq!(run_last(&mut eval, "iso/ix"), "1");
}

#[test]
fn file_star_during_load() {
    let fx = Fixture::new("filestar");
    fx.write(
        "deffile.clj",
        "(clojure.core/in-ns 'ff)\n(def fff clojure.core/*file*)\n",
    );
    let mut eval = boot();
    run(&mut eval, &fx.within("(load \"/deffile\")"));
    let want = format!(
        "{}/deffile.clj",
        fx.dir.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(run_last(&mut eval, "ff/fff"), format!("\"{want}\""));
}

#[test]
fn clj_beats_cljc_across_dirs() {
    let fx = Fixture::new("extorder");
    std::fs::create_dir_all(fx.dir.join("d1")).unwrap();
    std::fs::create_dir_all(fx.dir.join("d2")).unwrap();
    fx.write("d1/xo.cljc", "(clojure.core/in-ns 'xo)\n(def src :cljc)\n");
    fx.write("d2/xo.clj", "(clojure.core/in-ns 'xo)\n(def src :clj)\n");
    let d1 = fx.dir.join("d1").to_string_lossy().replace('\\', "/");
    let d2 = fx.dir.join("d2").to_string_lossy().replace('\\', "/");
    let mut eval = boot();
    // `.clj` wins across every dir before `.cljc` is tried anywhere
    // (`RT.load` lookup order), even in the later dir.
    run(
        &mut eval,
        &format!("(binding [*load-path* [\"{d1}\" \"{d2}\"]] (require 'xo))"),
    );
    assert_eq!(run_last(&mut eval, "xo/src"), ":clj");
}

#[test]
fn load_string_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(load-string 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

#[test]
fn load_file_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(load-file 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

#[test]
fn load_file_relative() {
    let name = format!("clj-relfile-{}-loadfilerel.clj", std::process::id());
    std::fs::write(&name, "(def rlf *file*)\n").unwrap();
    let mut eval = boot();
    run(&mut eval, &format!("(load-file \"{name}\")"));
    let star = run_last(&mut eval, "rlf");
    std::fs::remove_file(&name).unwrap();
    assert!(star.starts_with('"'), "not a string: {star}");
    assert!(star.ends_with(&format!("{name}\"")), "not joined: {star}");
    if !cfg!(windows) {
        assert!(!star.contains('\\'), "separators pinned: {star}");
    }
}

#[test]
fn load_pending_paths_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [clojure.core/*pending-paths* 42] (load \"/x\"))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentList"
    );
}

#[test]
fn load_star_ns_fallback() {
    let mut eval = boot();
    // An absolute path skips the `*ns*` read, so the load proceeds
    // to the missing-file error whatever `*ns*` holds.
    assert_eq!(
        uncaught(&mut eval, "(binding [*ns* 42] (load \"/x\"))"),
        "java.io.FileNotFoundException | Could not locate x__init.class, x.clj or x.cljc on classpath."
    );
    // The relative form reads `*ns*` (the absolute path skips
    // the read entirely) and the JVM fails the `Symbol` cast
    // instead of falling back (oracle REL42).
    assert_eq!(
        uncaught(&mut eval, "(binding [*ns* 42] (load \"x\"))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Symbol (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
}

#[test]
fn load_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(load 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

#[test]
fn load_unreadable_bytes() {
    let fx = Fixture::new("badbytes");
    std::fs::write(fx.dir.join("badlib.clj"), [0xffu8, 0xfe, 0x00]).unwrap();
    let mut eval = boot();
    let err = uncaught(&mut eval, &fx.within("(require 'badlib)"));
    assert!(
        err.starts_with("java.io.FileNotFoundException | "),
        "class: {err}"
    );
    assert!(
        err.ends_with("(stream did not contain valid UTF-8)"),
        "message: {err}"
    );
}

#[test]
fn load_path_entry_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [clojure.core/*load-path* [42]] (load \"/x\"))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

#[test]
fn require_vector_keyword() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(require [:foo])"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to clojure.lang.Symbol"
    );
}

#[test]
fn require_vector_string() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(require [\"foo\"])"),
        "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Symbol"
    );
}

#[test]
fn loaded_libs_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [clojure.core/*loaded-libs* 42] (require 'user))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentSet"
    );
}

#[test]
fn eval_bare_macro_arity() {
    let mut eval = boot();
    // Bare: no `CompilerException` wrapper, no cause.
    assert_eq!(
        uncaught_chain(&mut eval, "(eval '(when))"),
        vec![
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/when"
                .to_owned()
        ]
    );
}

#[test]
fn load_string_ratio_deferral() {
    let mut eval = boot();
    assert_eq!(
        uncaught_chain(&mut eval, "(load-string \"1/2\")"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
                .to_owned(),
            "java.lang.Exception | Ratio literals are not supported in this build (big-number follow-up N3): 1/2"
                .to_owned(),
        ]
    );
}

#[test]
fn load_string_macro_arity_is_bare() {
    let mut eval = boot();
    // Through `load`, a bare macro-arity failure escapes unwrapped
    // (single, no `CompilerException` wrapper — like `eval`).
    assert_eq!(
        uncaught_chain(&mut eval, "(load-string \"(when)\")"),
        vec![
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/when"
                .to_owned()
        ]
    );
}

#[test]
fn load_relative_missing_reports_ns_root() {
    let mut eval = boot();
    // A relative load resolves against the current namespace's root
    // directory (`user` here).
    assert_eq!(
        uncaught(&mut eval, "(load \"nonexistent_xyz\")"),
        "java.io.FileNotFoundException | Could not locate nonexistent_xyz__init.class, nonexistent_xyz.clj or nonexistent_xyz.cljc on classpath. Please check that namespaces with dashes use underscores in the Clojure file name."
    );
}

#[test]
fn load_string_invalid_number() {
    let mut eval = boot();
    assert_eq!(
        uncaught_chain(&mut eval, "(load-string \"08\")"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
                .to_owned(),
            "java.lang.NumberFormatException | Invalid number: 08".to_owned(),
        ]
    );
}
