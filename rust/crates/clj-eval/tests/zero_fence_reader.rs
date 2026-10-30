// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-fence reader: `data_readers.clj` autoload over the port
//! `*load-path*` (upstream oracle: `load-data-readers` /
//! `load-data-reader-file` / `data-reader-var` in
//! `src/clj/clojure/core.clj`, tag dispatch in
//! `src/jvm/clojure/lang/LispReader.java` `CtorReader.readTagged`).
//! The port has no classpath, so every read entry scans each
//! `*load-path*` dir for `data_readers.clj` / `data_readers.cljc`
//! (upstream order: all `.clj` first, then all `.cljc`) and merges
//! new files into the `*data-readers*` root. Error texts and
//! ex-data keys replay upstream verbatim; `:url` carries the file
//! path (the port has no `java.net.URL`).

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

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-zerofence-{}-{name}", std::process::id()));
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

    /// The `:url` a scan of `rel` reports: the joined path with
    /// `/` separators (like `*file*` during loads).
    fn url(&self, rel: &str) -> String {
        self.dir.join(rel).to_string_lossy().replace('\\', "/")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Define a data-reader fn var (autoload interns the same var when
/// the namespace already exists).
fn def_reader(eval: &mut Evaluator, ns: &str, name: &str, body: &str) {
    run(
        eval,
        &format!("(do (ns {ns}) (defn {name} [x] {body}) (in-ns 'user))"),
    );
}

#[test]
fn autoload_clj_tag_invokes_defined_reader() {
    let fx = Fixture::new("basic");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(&mut eval, &fx.within("(read-string \"#my/tag [1 2]\")"));
    assert_eq!(got, "[:read [1 2]]");
}

#[test]
fn autoload_merges_into_data_readers_root() {
    // Even a tagless read triggers the entry scan, and the merge
    // lands on the root (visible after the `*load-path*` binding
    // pops — upstream merges at startup the same way).
    let fx = Fixture::new("root");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(
        &mut eval,
        &fx.within("(do (read-string \"1\") (contains? @#'*data-readers* 'my/tag))"),
    );
    assert_eq!(got, "true");
    // The root survives the binding pop, so later reads resolve.
    assert_eq!(
        run_last(&mut eval, "(read-string \"#my/tag 1\")"),
        "[:read 1]"
    );
}

#[test]
fn autoload_undefined_reader_interns_unbound_var() {
    // Upstream `data-reader-var` creates the namespace and interns
    // the var without loading anything: the namespace exists
    // afterwards, and invoking the tag reports the unbound call.
    let fx = Fixture::new("unbound");
    fx.write("data_readers.clj", "{gone/tag gone.ns/f}");
    let mut eval = boot();
    let got = run_last(
        &mut eval,
        &fx.within(
            "(do (try (read-string \"#gone/tag 1\") (catch IllegalStateException e :caught)) (some? (find-ns 'gone.ns)))",
        ),
    );
    assert_eq!(got, "true");
    assert_eq!(
        uncaught(&mut eval, &fx.within("(read-string \"#gone/tag 1\")")),
        "java.lang.IllegalStateException | Attempting to call unbound fn: #'gone.ns/f"
    );
}

#[test]
fn autoload_cljc_plain_map() {
    let fx = Fixture::new("cljc");
    fx.write("data_readers.cljc", "{jc/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(&mut eval, &fx.within("(read-string \"#jc/tag 1\")"));
    assert_eq!(got, "[:read 1]");
}

#[test]
fn autoload_cljc_read_cond_honored() {
    // `.cljc` mapping files read with `:read-cond :allow`
    // (upstream `load-data-reader-file`).
    let fx = Fixture::new("cljc-cond");
    fx.write(
        "data_readers.cljc",
        "#?(:clj {jc/tag my.lib/reader} :default {})",
    );
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(&mut eval, &fx.within("(read-string \"#jc/tag 1\")"));
    assert_eq!(got, "[:read 1]");
}

#[test]
fn autoload_clj_rejects_read_cond() {
    // `.clj` mapping files read with `{:eof nil}` only, so `#?`
    // fails like a `read` on a line-numbering reader (upstream
    // wraps in `ReaderException`).
    let fx = Fixture::new("clj-cond");
    fx.write("data_readers.clj", "#?(:clj {a/tag my.lib/reader})");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(read-string \"1\")")),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
}

#[test]
fn autoload_inst_override_skips_default_validation() {
    // `*data-readers*` promises inst/uuid "may be overridden in
    // data_readers.clj": the table wins and the default validator
    // never sees the payload (not even garbage).
    let fx = Fixture::new("inst-override");
    fx.write("data_readers.clj", "{inst my.lib/myinst}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "myinst", "[:custom x]");
    let got = run_last(
        &mut eval,
        &fx.within("(read-string \"#inst \\\"garbage\\\"\")"),
    );
    assert_eq!(got, "[:custom \"garbage\"]");
}

#[test]
fn autoload_bad_map() {
    // Upstream `load-data-reader-file`: non-map content throws
    // ex-info with `{:url url}` (`:url` is the file path here).
    let fx = Fixture::new("badmap");
    fx.write("data_readers.clj", "[my/tag my.lib/reader]");
    let mut eval = boot();
    let got = run_last(
        &mut eval,
        &fx.within(
            "(try (read-string \"1\") (catch clojure.lang.ExceptionInfo e [(ex-message e) (:url (ex-data e))]))",
        ),
    );
    assert_eq!(
        got,
        format!(
            "[\"Not a valid data-reader map\" \"{}\"]",
            fx.url("data_readers.clj")
        )
    );
}

#[test]
fn autoload_empty_file_is_not_a_valid_map() {
    // `{:eof nil}` reads an empty file as nil, which is not a map
    // (upstream behaves the same — the `map?` check rejects nil).
    let fx = Fixture::new("empty");
    fx.write("data_readers.clj", "");
    let mut eval = boot();
    let got = run_last(
        &mut eval,
        &fx.within("(try (read-string \"1\") (catch clojure.lang.ExceptionInfo e (ex-message e)))"),
    );
    assert_eq!(got, "\"Not a valid data-reader map\"");
}

#[test]
fn autoload_bad_key() {
    // Non-symbol keys throw ex-info with `{:url :form}`
    // (upstream `load-data-reader-file`).
    let fx = Fixture::new("badkey");
    fx.write("data_readers.clj", "{:notasym my.lib/reader}");
    let mut eval = boot();
    let got = run_last(
        &mut eval,
        &fx.within(
            "(try (read-string \"1\") (catch clojure.lang.ExceptionInfo e [(ex-message e) (:form (ex-data e)) (:url (ex-data e))]))",
        ),
    );
    assert_eq!(
        got,
        format!(
            "[\"Invalid form in data-reader file\" :notasym \"{}\"]",
            fx.url("data_readers.clj")
        )
    );
}

#[test]
fn autoload_conflict_across_files() {
    // The same tag bound to different vars in two files throws
    // ex-info with `{:url :conflict :mappings}` (upstream
    // `load-data-reader-file` checks each file against the
    // accumulator of the files before it).
    let fa = Fixture::new("conflict-a");
    fa.write("data_readers.clj", "{c/tag lib.a/ra}");
    let fb = Fixture::new("conflict-b");
    fb.write("data_readers.clj", "{c/tag lib.b/rb}");
    let mut eval = boot();
    let src = format!(
        "(binding [clojure.core/*load-path* [{} {}]] (try (read-string \"1\") (catch clojure.lang.ExceptionInfo e [(ex-message e) (:conflict (ex-data e)) (:url (ex-data e))])))",
        fa.lit(),
        fb.lit()
    );
    let got = run_last(&mut eval, &src);
    assert_eq!(
        got,
        format!(
            "[\"Conflicting data-reader mapping\" c/tag \"{}\"]",
            fb.url("data_readers.clj")
        )
    );
}

#[test]
fn autoload_same_var_twice_is_no_conflict() {
    // Equal vars (same interned var) never conflict, even across
    // files (upstream compares with `not=`).
    let fa = Fixture::new("same-a");
    fa.write("data_readers.clj", "{c/tag lib.a/ra}");
    let fb = Fixture::new("same-b");
    fb.write("data_readers.cljc", "{c/tag lib.a/ra}");
    let mut eval = boot();
    def_reader(&mut eval, "lib.a", "ra", "[:a x]");
    let src = format!(
        "(binding [clojure.core/*load-path* [{} {}]] (read-string \"#c/tag 1\"))",
        fa.lit(),
        fb.lit()
    );
    assert_eq!(run_last(&mut eval, &src), "[:a 1]");
}

#[test]
fn autoload_bound_table_shadows_root() {
    // The merge lands on the root, so a thread-local
    // `*data-readers*` binding still shadows it (upstream binds
    // the same var over the startup-merged root).
    let fx = Fixture::new("shadow");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within("(binding [*data-readers* {}] (read-string \"#my/tag 1\"))")
        ),
        "java.lang.RuntimeException | No reader function for tag my/tag"
    );
}

#[test]
fn autoload_wipe_stays_wiped() {
    // Files merge once each (upstream scans once at startup): a
    // root replaced afterwards is not re-merged on later reads.
    let fx = Fixture::new("wipe");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(
        &mut eval,
        &fx.within(
            "(do (read-string \"1\") (alter-var-root #'*data-readers* (constantly {})) (read-string \"2\") (count @#'*data-readers*))",
        ),
    );
    assert_eq!(got, "0");
}

#[test]
fn autoload_borked_load_path_is_lenient() {
    // Reads never consulted `*load-path*` before: a useless value
    // skips the scan instead of breaking the read, and garbage
    // entries never poison the sibling dirs that do scan.
    let fx = Fixture::new("lenient");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [clojure.core/*load-path* 42] (read-string \"1\"))"
        ),
        "1"
    );
    let src = format!(
        "(binding [clojure.core/*load-path* [42 {}]] (read-string \"#my/tag 1\"))",
        fx.lit()
    );
    assert_eq!(run_last(&mut eval, &src), "[:read 1]");
}

#[test]
fn autoload_borked_data_readers_root() {
    // Merging into a non-map root fails like `assoc` onto it
    // (upstream `assoc`s each entry onto the root value).
    let fx = Fixture::new("borked-root");
    fx.write("data_readers.clj", "{my/tag my.lib/reader}");
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within(
                "(do (alter-var-root #'*data-readers* (constantly 42)) (read-string \"1\"))"
            )
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Associative (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
}

#[test]
fn autoload_new_dir_skips_already_loaded_files() {
    // Widening the path merges only the new dir's files: entries
    // surgically removed from the root are not re-merged from
    // files the scan already consumed.
    let fa = Fixture::new("skip-a");
    fa.write("data_readers.clj", "{a/tag lib.a/ra}");
    let fb = Fixture::new("skip-b");
    fb.write("data_readers.clj", "{b/tag lib.b/rb}");
    let mut eval = boot();
    def_reader(&mut eval, "lib.a", "ra", "[:a x]");
    def_reader(&mut eval, "lib.b", "rb", "[:b x]");
    assert_eq!(
        run_last(&mut eval, &fa.within("(read-string \"#a/tag 1\")")),
        "[:a 1]"
    );
    run_last(&mut eval, "(alter-var-root #'*data-readers* dissoc 'a/tag)");
    let src = format!(
        "(binding [clojure.core/*load-path* [{} {}]] (read-string \"#b/tag 1\"))",
        fa.lit(),
        fb.lit()
    );
    assert_eq!(run_last(&mut eval, &src), "[:b 1]");
    assert_eq!(
        run_last(&mut eval, "(contains? @#'*data-readers* 'a/tag)"),
        "false"
    );
}

#[test]
fn autoload_keyword_value_interns_like_symbol() {
    // Upstream `data-reader-var` routes values through
    // `namespace`/`name`, so keywords (also `Named`) intern
    // exactly like the equivalent symbol.
    let fx = Fixture::new("kwval");
    fx.write("data_readers.clj", "{my/tag :my.lib/reader}");
    let mut eval = boot();
    def_reader(&mut eval, "my.lib", "reader", "[:read x]");
    let got = run_last(&mut eval, &fx.within("(read-string \"#my/tag 1\")"));
    assert_eq!(got, "[:read 1]");
}

#[test]
fn autoload_garbage_values_fail_like_namespace() {
    // Non-`Named` values fail the `(namespace v)` call inside
    // `data-reader-var` (nil NPEs, the rest CCE).
    let fx = Fixture::new("garbage");
    fx.write("data_readers.clj", "{my/tag nil}");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(read-string \"1\")")),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null"
    );
    let fx2 = Fixture::new("garbage2");
    fx2.write("data_readers.clj", "{my/tag 42}");
    assert_eq!(
        uncaught(&mut eval, &fx2.within("(read-string \"1\")")),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Named (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Named is in unnamed module of loader 'app')"
    );
}

#[test]
fn autoload_unqualified_value_needs_namespace() {
    // A value without a namespace fails `(symbol nil)` with IAE
    // (upstream `data-reader-var` behaves the same).
    let fx = Fixture::new("unqual");
    fx.write("data_readers.clj", "{my/tag reader}");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(read-string \"1\")")),
        "java.lang.IllegalArgumentException | no conversion to symbol"
    );
}

#[test]
fn autoload_malformed_file_reports_reader_error() {
    // Truncated mapping files fail like a line-numbered `read`
    // (upstream reads with `LineNumberingPushbackReader`).
    let fx = Fixture::new("malformed");
    fx.write("data_readers.clj", "(unclosed");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(read-string \"1\")")),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading, starting at line 1"
    );
}

#[test]
fn autoload_load_reader_reports_scan_errors() {
    // `load-reader` runs the same per-form consult: a bad mapping
    // file fails the load with the raw ex-info.
    let fx = Fixture::new("loadreader");
    fx.write("data_readers.clj", "[my/tag my.lib/reader]");
    let mut eval = boot();
    let got = run_last(
        &mut eval,
        &fx.within(
            "(try (with-in-str \"1\" (load-reader *in*)) (catch clojure.lang.ExceptionInfo e (ex-message e)))",
        ),
    );
    assert_eq!(got, "\"Not a valid data-reader map\"");
}

#[test]
fn autoload_require_end_to_end() {
    // The user story: a lib dir ships its reader fn next to its
    // `data_readers.clj`; requiring the lib makes the tag readable
    // (the root merge outlives the `*load-path*` binding).
    let fx = Fixture::new("e2e");
    fx.write("data_readers.clj", "{sh/tag sh.lib/read-sh}");
    fx.write("sh/lib.clj", "(ns sh.lib) (defn read-sh [x] [:sh x])");
    let mut eval = boot();
    run_last(&mut eval, &fx.within("(require 'sh.lib)"));
    assert_eq!(
        run_last(&mut eval, "(read-string \"#sh/tag 1\")"),
        "[:sh 1]"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? @#'*data-readers* 'sh/tag)"),
        "true"
    );
}

#[test]
fn eval_qualified_head_ignores_refers() {
    // B1: upstream `maybeResolveIn` is interns-only — `+` is
    // referred (not interned) in `user`, so `#=(user/+ 1 2)`
    // throws `Can't resolve` like the JVM.
    let mut eval = boot();
    let err = uncaught(&mut eval, "(read-string \"#=(user/+ 1 2)\")");
    assert_eq!(err, "java.lang.RuntimeException | Can't resolve user/+");
    // Controls: interned-qualified and unqualified-referred heads
    // still call.
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(clojure.core/+ 1 2)\")"),
        "3"
    );
    assert_eq!(run_last(&mut eval, "(read-string \"#=(+ 1 2)\")"), "3");
}

#[test]
fn autoload_merge_failure_fails_loudly() {
    // B2: mid-scan user code (a tag firing while a mapping file
    // reads) borks the live `*data-readers*` root, so the merge
    // assoc fails the read loudly with the assoc CCE.
    let fx = Fixture::new("mergeloud");
    fx.write("data_readers.clj", "{bork/tag my.lib/borker}");
    let mut eval = boot();
    run(
        &mut eval,
        "(do (ns my.lib) (defn borker [x] (do (alter-var-root #'clojure.core/*data-readers* (constantly 5)) 'my.lib/reader)) (in-ns 'user))",
    );
    run_last(&mut eval, &fx.within("(read-string \"1\")"));
    fx.write("data_readers.cljc", "{my/tag #bork/tag [1]}");
    let err = uncaught(&mut eval, &fx.within("(read-string \"1\")"));
    assert_eq!(
        err,
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Associative (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
    // The mark-revert + retry half is pinned by the
    // `autoload_merge_failure_reverts_mark_for_retry` unit test:
    // healing through reads would advance the mark and mask the
    // revert, so it needs direct-field surgery.
}
