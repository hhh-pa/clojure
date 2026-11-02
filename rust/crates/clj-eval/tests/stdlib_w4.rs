// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W4 (entry/IO) stdlib smoke tests: `require` each of `clojure.edn`,
//! `clojure.repl`, `clojure.main`, `clojure.stacktrace`, and
//! `clojure.core-print`, then exercise the primary entry points
//! (`edn/read`, `doc`/`source`, `pst`, `print-method` dispatch) against
//! upstream-documented behavior.

use std::path::PathBuf;

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
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

/// Eval one form, expecting a string result; return its contents.
fn run_str(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    assert_eq!(vs.len(), 1, "expected one value for {src}");
    match &realized(eval, &vs[0]) {
        Value::Str(s) => s.to_string(),
        other => panic!("expected a string for {src}, got: {}", pr(other)),
    }
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
        Ok(vs) => panic!(
            "expected an uncaught throw, got {}",
            vs.iter().map(pr).collect::<Vec<_>>().join(" | ")
        ),
    }
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-w4-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) -> String {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().replace('\\', "/")
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

#[test]
fn edn_read() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.edn :as edn])"),
        "nil"
    );
    // Round-trips across the data shapes.
    assert_eq!(
        run_last(
            &mut eval,
            "(edn/read-string \"{:a [1 2] :b #{3} :c (x y)}\")"
        ),
        "{:a [1 2], :b #{3}, :c (x y)}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(edn/read-string (pr-str {:a [1 2] :b \"s\" :c :k :d 'sym :e 1.5 :f true :g nil :h \\c}))"
        ),
        "{:a [1 2], :b \"s\", :c :k, :d sym, :e 1.5, :f true, :g nil, :h \\c}"
    );
    // Tagged elements: custom readers, the default fn, defaults.
    assert_eq!(
        run_last(
            &mut eval,
            "(edn/read-string {:readers {'foo (fn [x] [:foo x])}} \"#foo 1\")"
        ),
        "[:foo 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(edn/read-string {:default (fn [t v] [:d t v])} \"#zzz 9\")"
        ),
        "[:d zzz 9]"
    );
    assert_eq!(
        run_last(&mut eval, "(edn/read-string \"#inst \\\"2020-01-02\\\"\")"),
        "#inst \"2020-01-02T00:00:00.000-00:00\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(edn/read-string \"#uuid \\\"f81d4fae-7dec-11d0-a765-00a0c91e6bf6\\\"\")"
        ),
        "#uuid \"f81d4fae-7dec-11d0-a765-00a0c91e6bf6\""
    );
    // Unknown tags still fail without a default.
    assert_eq!(
        uncaught(&mut eval, "(edn/read-string \"#zzz 9\")"),
        "java.lang.RuntimeException | No reader function for tag zzz"
    );
    // Eval is rejected: edn binds *read-eval* false.
    assert_eq!(
        uncaught(&mut eval, "(edn/read-string \"#=(+ 1 2)\")"),
        "java.lang.RuntimeException | EvalReader not allowed when *read-eval* is false."
    );
    // Streams: successive reads, then :eof (or a throw without it).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1 2\" [(edn/read) (edn/read)])"),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"1\" [(edn/read) (edn/read {:eof nil} *in*)])"
        ),
        "[1 nil]"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"\" (edn/read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading"
    );
}

#[test]
fn edn_read_string_edges() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.edn :as edn])"),
        "nil"
    );
    // nil and empty read nil under the 1-arity's {:eof nil}.
    assert_eq!(
        run_last(&mut eval, "[(edn/read-string nil) (edn/read-string \"\")]"),
        "[nil nil]"
    );
    assert_eq!(
        run_last(&mut eval, "(edn/read-string {:eof :done} \"\")"),
        ":done"
    );
    // Without :eof, EOF throws.
    assert_eq!(
        uncaught(&mut eval, "(edn/read-string {} \"\")"),
        "java.lang.RuntimeException | EOF while reading"
    );
}

#[test]
fn stacktrace_fns() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.stacktrace :as st])"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(ex-message (st/root-cause (ex-info \"a\" {} (ex-info \"b\" {}))))"
        ),
        "\"b\""
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (st/print-throwable (ex-info \"boom\" {:x 1})))"
        ),
        "clojure.lang.ExceptionInfo: boom\n{:x 1}"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (st/print-stack-trace (ex-info \"boom\" {:x 1})))"
        ),
        "clojure.lang.ExceptionInfo: boom\n{:x 1}\n at clojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (st/print-stack-trace (ex-info \"boom\" {:x 1}) 1))"
        ),
        "clojure.lang.ExceptionInfo: boom\n{:x 1}\n at clojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (st/print-cause-trace (ex-info \"a\" {} (ex-info \"b\" {}))))"
        ),
        "clojure.lang.ExceptionInfo: a\n{}\n at clojure.lang.ExceptionInfo.unknown (:-1)\nCaused by: clojure.lang.ExceptionInfo: b\n{}\n at clojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (st/print-trace-element (first (:trace (Throwable->map (ex-info \"x\" {}))))))"
        ),
        "clojure.lang.ExceptionInfo.unknown (:-1)"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*e (ex-info \"ee\" {:y 2})] (st/e)))"
        ),
        "clojure.lang.ExceptionInfo: ee\n{:y 2}\n at clojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
}

#[test]
fn core_print_wiring() {
    let mut eval = boot();
    // The load root carries a real namespace, so :as works.
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.core-print :as cp])"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(resolve 'print-method) (resolve 'print-dup) (resolve 'print-simple) (resolve 'print-ctor) (resolve 'char-escape-string) (resolve 'char-name-string) (resolve 'primitives-classnames) (resolve 'StackTraceElement->vec) (resolve 'Throwable->map) (resolve 'PrintWriter-on)]"
        ),
        "[#'clojure.core/print-method #'clojure.core/print-dup #'clojure.core/print-simple #'clojure.core/print-ctor #'clojure.core/char-escape-string #'clojure.core/char-name-string #'clojure.core/primitives-classnames #'clojure.core/StackTraceElement->vec #'clojure.core/Throwable->map #'clojure.core/PrintWriter-on]"
    );
    assert_eq!(
        run_last(&mut eval, "@(var clojure.core/print-initialized)"),
        "true"
    );
    // print-method dispatch: a user method fires through pr-str.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defmethod print-method \"java.lang.String\" [s w] (print-simple \"STR!\" w)) (pr-str \"hi\"))"
        ),
        "\"STR!\""
    );
}

#[test]
fn repl_doc() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.repl :as rp])"),
        "nil"
    );
    let out = run_str(&mut eval, "(with-out-str (rp/doc map))");
    assert!(out.contains("-------------------------"), "{out}");
    assert!(out.contains("clojure.core/map"), "{out}");
    let out = run_str(&mut eval, "(with-out-str (rp/doc if))");
    assert!(out.contains("Special Form"), "{out}");
    assert!(out.contains("(if test then else?)"), "{out}");
    assert!(out.contains("http://clojure.org/special_forms#if"), "{out}");
    let out = run_str(&mut eval, "(with-out-str (rp/doc when))");
    assert!(out.contains("Macro"), "{out}");
    let out = run_str(&mut eval, "(with-out-str (rp/doc clojure.repl))");
    assert!(out.contains("clojure.repl"), "{out}");
    assert!(
        out.contains("Utilities meant to be used interactively at the REPL"),
        "{out}"
    );
    let out = run_str(&mut eval, "(with-out-str (rp/doc :foo/bar))");
    assert!(out.contains(":foo/bar"), "{out}");
    assert!(out.contains("Spec"), "{out}");
    assert_eq!(
        run_str(&mut eval, "(with-out-str (rp/doc no-such-var-xyz))"),
        ""
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (rp/find-doc \"zzz-no-match-xyz\"))"
        ),
        ""
    );
    let out = run_str(
        &mut eval,
        "(with-out-str (rp/find-doc \"regular expression\"))",
    );
    assert!(out.contains("clojure.repl/apropos"), "{out}");
}

#[test]
fn repl_source() {
    let fx = Fixture::new("source");
    let clj = fx.write(
        "w4src.clj",
        ";; comment line 1\n;; comment line 2\n(defn w4src-first [x]\n  x)\n\n(defn w4src-fn [x]\n  (+ x 1))\n",
    );
    let cljc = fx.write("w4src.cljc", "(defn w4src-fn-jc [x] (* x 2))\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.repl :as rp])"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(load-file \"{clj}\")")),
        "#'user/w4src-fn"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(load-file \"{cljc}\")")),
        "#'user/w4src-fn-jc"
    );
    // The by-name search finds each defn; leading comments are skipped.
    assert_eq!(
        run_str(&mut eval, "(rp/source-fn 'w4src-first)"),
        "(defn w4src-first [x]\n  x)"
    );
    assert_eq!(
        run_str(&mut eval, "(rp/source-fn 'w4src-fn)"),
        "(defn w4src-fn [x]\n  (+ x 1))"
    );
    assert_eq!(
        run_str(&mut eval, "(rp/source-fn 'w4src-fn-jc)"),
        "(defn w4src-fn-jc [x] (* x 2))"
    );
    let out = run_str(&mut eval, "(with-out-str (rp/source w4src-fn))");
    assert!(out.contains("(defn w4src-fn [x]"), "{out}");
    assert!(!out.contains("comment"), "{out}");
    // Natives have no :file, unknown syms no var: nil, then the macro text.
    assert_eq!(run_last(&mut eval, "(rp/source-fn 'map)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(rp/source-fn 'no-such-var-xyz)"),
        "nil"
    );
    assert_eq!(
        run_str(&mut eval, "(with-out-str (rp/source map))"),
        "Source not found\n"
    );
}

#[test]
fn repl_dir_apropos() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.repl :as rp])"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(rp/dir-fn 'clojure.repl)"),
        "(apropos demunge dir dir-fn doc find-doc pst root-cause set-break-handler! source source-fn stack-element-str thread-stopper)"
    );
    // Aliases resolve through *ns*.
    assert_eq!(
        run_last(&mut eval, "(= (rp/dir-fn 'rp) (rp/dir-fn 'clojure.repl))"),
        "true"
    );
    assert_eq!(
        run_str(&mut eval, "(with-out-str (rp/dir clojure.repl))"),
        "apropos\ndemunge\ndir\ndir-fn\ndoc\nfind-doc\npst\nroot-cause\nset-break-handler!\nsource\nsource-fn\nstack-element-str\nthread-stopper\n"
    );
    assert_eq!(run_last(&mut eval, "(rp/apropos \"zzz-no-match\")"), "()");
    assert_eq!(
        run_last(
            &mut eval,
            "(some #{'clojure.core/map} (rp/apropos \"map\"))"
        ),
        "clojure.core/map"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(some #{'clojure.core/map} (rp/apropos #\"^map$\"))"
        ),
        "clojure.core/map"
    );
}

#[test]
fn repl_demunge_helpers() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.repl :as rp])"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(rp/demunge \"foo$bar\") (rp/demunge \"my_sym\") (rp/demunge \"a_COLON_b\") (rp/demunge \"a_BANG_b\") (rp/demunge \"plain\")]"
        ),
        "[\"foo/bar\" \"my-sym\" \"a:b\" \"a!b\" \"plain\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(ex-message (rp/root-cause (ex-info \"a\" {} (ex-info \"b\" {}))))"
        ),
        "\"b\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(rp/stack-element-str (first (:trace (Throwable->map (ex-info \"x\" {})))))"
        ),
        "\"clojure.lang.ExceptionInfo.unknown (:-1)\""
    );
}

#[test]
fn repl_pst() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.repl :as rp])"),
        "nil"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out*] (rp/pst (ex-info \"boom\" {:x 1}))))"
        ),
        "ExceptionInfo boom {:x 1}\n\tclojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out*] (rp/pst (ex-info \"a\" {} (ex-info \"b\" {})))))"
        ),
        "ExceptionInfo a {}\n\tclojure.lang.ExceptionInfo.unknown (:-1)\nCaused by:\nExceptionInfo b {}\n\tclojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    // The 1-arity takes a depth off *e; the 0-arity defaults to 12.
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out* *e (ex-info \"ee\" {})] (rp/pst 4)))"
        ),
        "ExceptionInfo ee {}\n\tclojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out* *e (ex-info \"ee\" {})] (rp/pst)))"
        ),
        "ExceptionInfo ee {}\n\tclojure.lang.ExceptionInfo.unknown (:-1)\n"
    );
    // The reader/compiler phase note.
    let out = run_str(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (rp/pst (ex-info \"m\" {:clojure.error/phase :read-source}))))",
    );
    assert!(
        out.contains("Note: The following stack trace applies to the reader or compiler"),
        "{out}"
    );
    // No threads or signals here: both fns throw.
    assert_eq!(
        uncaught(&mut eval, "(rp/thread-stopper)"),
        "clojure.lang.ExceptionInfo | thread-stopper is not supported on this runtime (no threads to stop)."
    );
    assert_eq!(
        uncaught(&mut eval, "(rp/set-break-handler! prn)"),
        "clojure.lang.ExceptionInfo | set-break-handler! is not supported on this runtime (no signal handlers)."
    );
}

#[test]
fn main_helpers() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(m/demunge \"foo$bar\") (m/demunge \"a_BANG_b\")]"
        ),
        "[\"foo/bar\" \"a!b\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(ex-message (m/root-cause (ex-info \"a\" {} (ex-info \"b\" {}))))"
        ),
        "\"b\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(m/stack-element-str (first (:trace (Throwable->map (ex-info \"x\" {})))))"
        ),
        "\"clojure.lang.ExceptionInfo.unknown (:-1)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(m/with-bindings (str *print-namespace-maps* \"|\" *compile-path* \"|\" *1 \"|\" *e))"
        ),
        "\"true|classes||\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*read-eval* :unknown] (m/with-read-known *read-eval*))"
        ),
        "true"
    );
    assert_eq!(
        run_str(&mut eval, "(with-out-str (m/repl-prompt))"),
        "user=> "
    );
    assert_eq!(
        uncaught(&mut eval, "(m/skip-if-eol nil)"),
        "clojure.lang.ExceptionInfo | skip-if-eol is not supported on this runtime (readers expose no single-char read/unread)."
    );
    assert_eq!(
        uncaught(&mut eval, "(m/skip-whitespace nil)"),
        "clojure.lang.ExceptionInfo | skip-whitespace is not supported on this runtime (readers expose no single-char read/unread)."
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"(+ 1 2)\" (m/renumbering-read {:read-cond :allow} *in* 1))"
        ),
        "(+ 1 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"42\" (m/repl-read :p :e))"),
        "42"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (m/repl-read :p :e))"),
        ":e"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(ex-message (m/repl-exception (ex-info \"a\" {} (ex-info \"b\" {}))))"
        ),
        "\"b\""
    );
    assert_eq!(
        run_last(&mut eval, "m/repl-requires"),
        "[[clojure.repl :refer (source apropos dir pst doc find-doc)] [clojure.java.javadoc :refer (javadoc)] [clojure.pprint :refer (pp pprint)] [clojure.repl.deps :refer (add-libs add-lib sync-deps)]]"
    );
}

#[test]
fn main_triage() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(:clojure.error/phase (m/ex-triage (Throwable->map (ex-info \"boom\" {})))) (:clojure.error/cause (m/ex-triage (Throwable->map (ex-info \"boom\" {})))) (:clojure.error/class (m/ex-triage (Throwable->map (ex-info \"boom\" {}))))]"
        ),
        "[:execution \"boom\" clojure.lang.ExceptionInfo]"
    );
    // The phase recovers from the outermost link's ex-data.
    assert_eq!(
        run_last(
            &mut eval,
            "[(:clojure.error/phase (m/ex-triage (Throwable->map (ex-info \"m\" {:clojure.error/phase :read-source :clojure.error/line 5 :clojure.error/source \"a.clj\"})))) (:clojure.error/source (m/ex-triage (Throwable->map (ex-info \"m\" {:clojure.error/phase :read-source :clojure.error/line 5 :clojure.error/source \"a.clj\"})))) (:clojure.error/line (m/ex-triage (Throwable->map (ex-info \"m\" {:clojure.error/phase :read-source :clojure.error/line 5 :clojure.error/source \"a.clj\"}))))]"
        ),
        "[:read-source \"a.clj\" 5]"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(m/ex-str (m/ex-triage (Throwable->map (ex-info \"boom\" {}))))"
        ),
        "Execution error (ExceptionInfo) at (REPL:1).\nboom\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(m/err->msg (ex-info \"boom\" {:clojure.error/phase :read-source :clojure.error/source \"a.clj\" :clojure.error/line 3}))"
        ),
        "Syntax error reading source at (a.clj:3).\nboom\n"
    );
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out*] (m/repl-caught (ex-info \"cb\" {}))))"
        ),
        "Execution error (ExceptionInfo) at (REPL:1).\ncb\n"
    );
}

#[test]
fn main_repl_loop() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    // Two evals shift *1/*2/*3; every hook fires in order.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [inputs (atom ['a 'b :exit]) seen (atom [])] (m/repl :read (fn [p x] (let [i (first @inputs)] (swap! inputs rest) (if (= i :exit) x i))) :eval (fn [form] form) :print (fn [v] (swap! seen conj [*1 *2 *3])) :prompt (fn []) :need-prompt (fn [] false)) @seen)"
        ),
        "[[a nil nil] [b a nil]]"
    );
    // Read errors reach :caught and set *e; the loop then exits.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (atom 0) caught (atom nil)] (m/repl :read (fn [p x] (if (zero? @n) (do (swap! n inc) (throw (ex-info \"rd\" {}))) x)) :caught (fn [t] (reset! caught (ex-message t))) :prompt (fn []) :need-prompt (fn [] false)) @caught)"
        ),
        "\"rd\""
    );
}

#[test]
fn main_load_script() {
    let fx = Fixture::new("load-script");
    let file = fx.write("w4script.clj", "(def w4script-loaded (+ 40 2))\n");
    fx.write("w4at/w4res.clj", "(def w4res-loaded :yes)\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(m/load-script \"{file}\")")),
        "#'user/w4script-loaded"
    );
    assert_eq!(run_last(&mut eval, "w4script-loaded"), "42");
    // @-paths resolve against *load-path* (extension optional).
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(binding [*load-path* [{}]] (m/load-script \"@/w4at/w4res\"))",
                fx.lit()
            )
        ),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "w4res-loaded"), ":yes");
}

#[test]
fn main_entry() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    assert_eq!(
        run_str(&mut eval, "(with-out-str (m/main \"-e\" \"(+ 1 2)\"))"),
        "3\n"
    );
    assert_eq!(run_last(&mut eval, "(m/main \"-e\" \"(+ 1 2)\")"), "nil");
    let help = run_str(&mut eval, "(with-out-str (m/main \"--help\"))");
    assert!(
        help.contains("Usage: java -cp clojure.jar clojure.main"),
        "{help}"
    );
    assert!(help.contains("-r, --repl"), "{help}");
    // No args with an empty stdin runs the greeting, fails the default
    // requires (pprint/javadoc/deps ship in other slices), then exits
    // the loop at EOF instead of hanging.
    let out = run_str(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (with-in-str \"\" (m/main))))",
    );
    assert!(out.contains("Clojure"), "{out}");
    // Error reporting: none prints nothing, stderr prints the report.
    assert_eq!(
        run_str(
            &mut eval,
            "(with-out-str (binding [*err* *out*] (m/report-error (ex-info \"r\" {}) :target \"none\")))"
        ),
        ""
    );
    let out = run_str(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (m/report-error (ex-info \"rpt\" {}) :target \"stderr\")))",
    );
    assert!(out.contains("Execution error"), "{out}");
    assert!(out.contains("rpt"), "{out}");
}

#[test]
fn main_entry_files() {
    let fx = Fixture::new("entry-files");
    fx.write(
        "w4mfile.clj",
        "(ns w4mfile)\n(defn -main [& args] (apply str \"main:\" args))\n",
    );
    let script = fx.write("w4run.clj", "(def w4run-done :done)\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(require '[clojure.main :as m])"),
        "nil"
    );
    // -m resolves and applies -main with the trailing args.
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(binding [*load-path* [{}]] (m/main \"-m\" \"w4mfile\" \"a\" \"b\"))",
                fx.lit()
            )
        ),
        "\"main:ab\""
    );
    // A bare path runs as a script.
    assert_eq!(
        run_last(&mut eval, &format!("(m/main \"{script}\")")),
        "#'user/w4run-done"
    );
    assert_eq!(run_last(&mut eval, "w4run-done"), ":done");
    // The file target writes a session-unique report; remove it after.
    let out = run_str(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (m/report-error (ex-info \"rptfile\" {}) :target \"file\")))",
    );
    assert!(out.contains("Full report at:"), "{out}");
    let path = out.lines().last().unwrap().to_owned();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("rptfile"), "{body}");
    std::fs::remove_file(&path).unwrap();
}
