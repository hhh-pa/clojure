// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W5 (small libs) smoke tests, unix leg: `clojure.java.browse`,
//! `clojure.java.process`, and the browser-opening half of
//! `clojure.java.javadoc`. Unix-only: the hermetic commands (`echo`, `sh`,
//! `cat`, `true`, `sleep`) have no Windows counterparts (the established
//! `#[cfg(unix)]` pattern).

#![cfg(unix)]

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, pr_to_string};

fn pr(v: &clj_value::Value) -> String {
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

fn require_browse(eval: &mut Evaluator) {
    run(eval, "(require 'clojure.java.browse)");
}

#[test]
fn browse_ns_loads_with_exact_publics() {
    let mut eval = boot();
    require_browse(&mut eval);
    assert_eq!(
        run(&mut eval, "(sort (keys (ns-publics 'clojure.java.browse)))"),
        "(*open-url-script* browse-url)"
    );
    assert_eq!(
        run(&mut eval, "@@#'clojure.java.browse/*open-url-script*"),
        ":uninitialized"
    );
}

#[test]
fn browse_open_command_probe() {
    let mut eval = boot();
    require_browse(&mut eval);
    // Platform answers, asserted loosely: a boolean, and a script path or nil.
    let mac = run(&mut eval, "(@#'clojure.java.browse/macosx?)");
    assert!(mac == "true" || mac == "false", "{mac}");
    let script = run(&mut eval, "(@#'clojure.java.browse/open-url-script-val)");
    assert!(script == "nil" || script.starts_with('"'), "{script}");
    let xdg = run(&mut eval, "(@#'clojure.java.browse/xdg-open-loc)");
    assert!(xdg == "nil" || xdg.starts_with('"'), "{xdg}");
}

#[test]
fn browse_url_uses_bound_script_without_blocking() {
    let mut eval = boot();
    require_browse(&mut eval);
    // /bin/true exits 0 immediately: browse-url answers true at once.
    assert_eq!(
        run(
            &mut eval,
            "(binding [clojure.java.browse/*open-url-script* (atom \"/bin/true\")] \
             (clojure.java.browse/browse-url \"http://example.com\"))"
        ),
        "true"
    );
}

#[test]
fn javadoc_opens_browser_for_known_class() {
    let mut eval = boot();
    run(&mut eval, "(require 'clojure.java.javadoc)");
    assert_eq!(
        run(
            &mut eval,
            "(binding [clojure.java.browse/*open-url-script* (atom \"/bin/true\")] \
             (clojure.java.javadoc/javadoc \"hello\"))"
        ),
        "true"
    );
}

#[test]
fn javadoc_reports_unknown_class() {
    let mut eval = boot();
    run(&mut eval, "(require 'clojure.java.javadoc)");
    assert_eq!(
        run(
            &mut eval,
            "(with-out-str \
              (binding [clojure.java.browse/*open-url-script* (atom \"/bin/true\")] \
               (clojure.java.javadoc/javadoc :kw)))"
        ),
        "\"Could not find Javadoc for clojure.lang.Keyword\\n\""
    );
}

fn require_process(eval: &mut Evaluator) {
    run(eval, "(require '[clojure.java.process :as p])");
}

/// A scratch path under /tmp unique to this test process.
fn scratch(name: &str) -> String {
    format!("/tmp/w5-{name}-{}", std::process::id())
}

#[test]
fn process_ns_loads_with_exact_publics() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(sort (keys (ns-publics 'clojure.java.process)))"
        ),
        "(exec exit-ref from-file io-task start stderr stdin stdout to-file)"
    );
}

#[test]
fn process_redirect_maps() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(
        run(&mut eval, "(p/to-file \"/tmp/x\" :append true)"),
        "{:redirect :to-file, :file \"/tmp/x\", :append true}"
    );
    assert_eq!(
        run(&mut eval, "(p/to-file \"/tmp/x\")"),
        "{:redirect :to-file, :file \"/tmp/x\", :append false}"
    );
    assert_eq!(
        run(&mut eval, "(p/from-file \"/tmp/y\")"),
        "{:redirect :from-file, :file \"/tmp/y\"}"
    );
}

#[test]
fn process_start_stdout_exit() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(
        run(&mut eval, "(slurp (p/stdout (p/start \"echo\" \"hi\")))"),
        "\"hi\\n\""
    );
    assert_eq!(
        run(
            &mut eval,
            "@(p/exit-ref (p/start \"sh\" \"-c\" \"exit 3\"))"
        ),
        "3"
    );
    assert_eq!(
        run(&mut eval, "@(p/exit-ref (p/start \"echo\" \"x\"))"),
        "0"
    );
}

#[test]
fn process_exec() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(run(&mut eval, "(p/exec \"echo\" \"yo\")"), "\"yo\\n\"");
    assert_eq!(
        uncaught(&mut eval, "(p/exec \"sh\" \"-c\" \"exit 1\")"),
        "clojure.lang.ExceptionInfo | Process failed with exit=1"
    );
}

#[test]
fn process_stdin_then_stdout() {
    let mut eval = boot();
    require_process(&mut eval);
    // Write the child input after start, read the echoed capture.
    assert_eq!(
        run(
            &mut eval,
            "(let [proc (p/start \"cat\")] \
             (spit (p/stdin proc) \"hello\") \
             (slurp (p/stdout proc)))"
        ),
        "\"hello\""
    );
    // Non-:pipe :in has no stdin handle.
    let input = scratch("in");
    std::fs::write(&input, "aaa").unwrap();
    assert_eq!(
        run(
            &mut eval,
            &format!("(p/stdin (p/start {{:in (p/from-file \"{input}\")}} \"cat\"))")
        ),
        "nil"
    );
    std::fs::remove_file(&input).unwrap();
}

#[test]
fn process_stdin_file_is_consumed() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [proc (p/start \"cat\") \
                  path (p/stdin proc)] \
             (spit path \"x\") \
             [(slurp (p/stdout proc)) \
              (try (slurp path) :still-there (catch Throwable _ :gone))])"
        ),
        "[\"x\" :gone]"
    );
}

#[test]
fn process_to_file_redirect() {
    let mut eval = boot();
    require_process(&mut eval);
    let out = scratch("out");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [proc (p/start {{:out (p/to-file \"{out}\")}} \"echo\" \"hi\")] \
                 [(deref (p/exit-ref proc)) (slurp \"{out}\") (slurp (p/stdout proc))])"
            )
        ),
        "[0 \"hi\\n\" \"\"]"
    );
    std::fs::remove_file(&out).unwrap();
}

#[test]
fn process_to_file_append() {
    let mut eval = boot();
    require_process(&mut eval);
    let out = scratch("app");
    std::fs::write(&out, "A").unwrap();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [proc (p/start {{:out (p/to-file \"{out}\" :append true)}} \
                                \"sh\" \"-c\" \"printf B\")] \
                 [(deref (p/exit-ref proc)) (slurp \"{out}\")])"
            )
        ),
        "[0 \"AB\"]"
    );
    std::fs::remove_file(&out).unwrap();
}

#[test]
fn process_from_file_and_dir_env() {
    let mut eval = boot();
    require_process(&mut eval);
    let input = scratch("from");
    std::fs::write(&input, "aaa").unwrap();
    assert_eq!(
        run(
            &mut eval,
            &format!("(p/exec {{:in (p/from-file \"{input}\")}} \"cat\")")
        ),
        "\"aaa\""
    );
    std::fs::remove_file(&input).unwrap();
    // :dir runs the child elsewhere (/tmp is a macOS symlink; accept both).
    let dir = run(&mut eval, "(p/exec {:dir \"/tmp\"} \"pwd\")");
    assert!(
        dir == "\"/tmp\\n\"" || dir == "\"/private/tmp\\n\"",
        "{dir}"
    );
    // :clear-env with a full :env is exact.
    assert_eq!(
        run(
            &mut eval,
            "(p/exec {:env {\"W5Q\" \"qq\"} :clear-env true} \"sh\" \"-c\" \"echo $W5Q\")"
        ),
        "\"qq\\n\""
    );
}

#[test]
fn process_inherit_discard_stdout_merge() {
    let mut eval = boot();
    require_process(&mut eval);
    // :out :inherit lands on *out* when the child exits.
    assert_eq!(
        run(
            &mut eval,
            "(with-out-str \
              (let [proc (p/start {:out :inherit} \"echo\" \"inh\")] \
               (deref (p/exit-ref proc))))"
        ),
        "\"inh\\n\""
    );
    // :err :inherit lands on *err* (rebound to the capture here).
    assert_eq!(
        run(
            &mut eval,
            "(with-out-str \
              (binding [*err* *out*] \
               (deref (p/exit-ref \
                 (p/start {:err :inherit} \"sh\" \"-c\" \"echo e 1>&2\")))))"
        ),
        "\"e\\n\""
    );
    // :discard drops the capture.
    assert_eq!(
        run(
            &mut eval,
            "(let [proc (p/start {:out :discard} \"echo\" \"gone\")] \
             [(deref (p/exit-ref proc)) (slurp (p/stdout proc))])"
        ),
        "[0 \"\"]"
    );
    // :err :stdout folds stderr into the stdout reader.
    assert_eq!(
        run(
            &mut eval,
            "(slurp (p/stdout (p/start {:err :stdout} \"sh\" \"-c\" \"echo o; echo e 1>&2\")))"
        ),
        "\"o\\ne\\n\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(slurp (p/stderr (p/start {:err :stdout} \"sh\" \"-c\" \"echo o; echo e 1>&2\")))"
        ),
        "\"\""
    );
    // Plain :pipe stderr reads the child error stream.
    assert_eq!(
        run(
            &mut eval,
            "(slurp (p/stderr (p/start \"sh\" \"-c\" \"echo e 1>&2\")))"
        ),
        "\"e\\n\""
    );
}

#[test]
fn process_exit_ref_timeout_and_readers() {
    let mut eval = boot();
    require_process(&mut eval);
    // Timeout deref answers the fallback; the later deref the exit code.
    assert_eq!(
        run(
            &mut eval,
            "(let [er (p/exit-ref (p/start \"sleep\" \"2\"))] \
             [(deref er 100 :timed-out) (deref er)])"
        ),
        "[:timed-out 0]"
    );
    // stdout answers the same cached reader on repeat calls ...
    assert_eq!(
        run(
            &mut eval,
            "(let [proc (p/start \"echo\" \"x\")] \
             (identical? (p/stdout proc) (p/stdout proc)))"
        ),
        "true"
    );
    // ... and like a stream it reads once (slurp drains + closes).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (p/stdout (p/start \"echo\" \"x\"))] (slurp r) (slurp r))"
        ),
        "java.io.IOException | Stream closed"
    );
}

#[test]
fn process_io_task() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(run(&mut eval, "(deref (p/io-task (fn [] 41)))"), "41");
}

#[test]
fn process_errors() {
    let mut eval = boot();
    require_process(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(p/start {:out :bogus} \"echo\")"),
        "clojure.lang.ExceptionInfo | Invalid out redirect: :bogus"
    );
    assert_eq!(
        uncaught(&mut eval, "(p/stdout 42)"),
        "clojure.lang.ExceptionInfo | Not a process handle: 42"
    );
    assert_eq!(
        uncaught(&mut eval, "(slurp (p/stdout (p/start)))"),
        "java.lang.IllegalArgumentException | sh requires at least one command string"
    );
    // A missing program surfaces through the exit-ref deref.
    let err = uncaught(
        &mut eval,
        "(deref (p/exit-ref (p/start \"nonexistent-w5-prog-xyz\")))",
    );
    assert!(
        err.starts_with("java.util.concurrent.ExecutionException | ")
            && err.contains("nonexistent-w5-prog-xyz"),
        "{err}"
    );
}
