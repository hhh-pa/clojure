// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S7B REPL driver tests (TDD): scripted `Cursor` input through
//! [`Session`](clj_repl::Session), byte-exact transcripts. No PTY
//! anywhere (spec §2.5).

use std::io::{BufReader, Cursor};

use clj_repl::{Session, core_var, is_incomplete_error, read_is_complete};
use clj_value::{List, Value};

/// Run one session transcript: `input` in, `(exit_code, transcript)`
/// out. `prompt` is the session prompt template (`None` = streaming
/// purity: no prompts at all).
fn run(input: &str, prompt: Option<&str>) -> (i32, String) {
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new(input.as_bytes().to_vec()));
    let mut session = Session::new(eval, reader, Vec::new());
    session.set_prompt(prompt.map(str::to_owned));
    let code = session.run();
    let (_, _, out) = session.into_parts();
    let text = String::from_utf8(out).expect("transcript is utf-8");
    (code, text)
}

/// Evaluate one form in a fresh boot and `pr` the last value (for
/// white-box var assertions that need no session).
fn run_last(eval: &mut clj_eval::Evaluator, src: &str) -> String {
    let values = eval.eval_source(src).expect("probe evals clean");
    let last = values.last().expect("probe yields a value");
    clj_value::pr_to_string(last, &clj_value::PrintOpts::pr())
}

#[test]
fn basic_eval_prompt_off() {
    assert_eq!(run("(+ 1 2)\n", None), (0, "3\n".to_owned()));
}

#[test]
fn basic_eval_prompt_on() {
    assert_eq!(
        run("(+ 1 2)\n", Some("{ns}=> ")),
        (0, "user=> 3\nuser=> ".to_owned())
    );
}

#[test]
fn empty_input_exits_clean() {
    assert_eq!(run("", None), (0, String::new()));
    assert_eq!(run("", Some("{ns}=> ")), (0, "user=> ".to_owned()));
}

#[test]
fn missing_trailing_newline_still_evals() {
    assert_eq!(run("(+ 1 2)", None), (0, "3\n".to_owned()));
}

#[test]
fn blank_lines_and_comments_print_nothing() {
    assert_eq!(run("\n\n(+ 1 2)\n\n", None), (0, "3\n".to_owned()));
    assert_eq!(run("; hello\n(+ 1 2)\n", None), (0, "3\n".to_owned()));
}

#[test]
fn multiline_list_continues_without_reprompt() {
    assert_eq!(run("(+ 1\n2)\n", None), (0, "3\n".to_owned()));
    assert_eq!(
        run("(+ 1\n2)\n", Some("{ns}=> ")),
        (0, "user=> 3\nuser=> ".to_owned())
    );
}

#[test]
fn multiline_string_continues() {
    // `pr` escapes the embedded newline (readable output).
    assert_eq!(
        run("\"abc\ndef\"\n", None),
        (0, "\"abc\\ndef\"\n".to_owned())
    );
}

#[test]
fn multiline_regex_continues() {
    assert_eq!(run("#\"a\nb\"\n", None), (0, "#\"a\nb\"\n".to_owned()));
}

#[test]
fn multi_form_line_prints_each_value() {
    assert_eq!(
        run("1 2\n[*1 *2 *3]\n", None),
        (0, "1\n2\n[2 1 nil]\n".to_owned())
    );
}

#[test]
fn per_form_buffer_prints_before_later_error() {
    // Per-form iteration (pins P1): `1` prints and shifts `*1`
    // before `nope` errors — earlier values are never dropped.
    assert_eq!(
        run("1 nope\n*1\n", None),
        (
            0,
            "1\nSyntax error compiling at (user:1:1).\njava.lang.RuntimeException | Unable to resolve symbol: nope in this context\n1\n"
                .to_owned()
        )
    );
}

#[test]
fn per_form_buffer_stops_at_first_eval_error() {
    // The failing form aborts its buffer (`2` never evals) but
    // the session continues; `*1` keeps the last good value.
    assert_eq!(
        run("1 (/ 1 0) 2\n*1\n", None),
        (0, "1\nArithmeticException: Divide by zero\n1\n".to_owned())
    );
}

#[test]
fn read_error_prints_and_continues() {
    // Span 1:2: the tracker sits past the consumed `)` (JVM: REPL:1:2).
    assert_eq!(
        run(")\n(+ 1 2)\n", None),
        (0, "1:2: Unmatched delimiter: )\n3\n".to_owned())
    );
}

#[test]
fn read_error_binds_e() {
    assert_eq!(
        run(")\n(ex-message *e)\n", None),
        (
            0,
            "1:2: Unmatched delimiter: )\n\"Unmatched delimiter: )\"\n".to_owned()
        )
    );
}

#[test]
fn compile_error_prints_and_continues() {
    assert_eq!(
        run("nope\n(+ 1 2)\n", None),
        (
            0,
            "Syntax error compiling at (user:1:1).\njava.lang.RuntimeException | Unable to resolve symbol: nope in this context\n3\n"
                .to_owned()
        )
    );
}

#[test]
fn compile_error_binds_e() {
    assert_eq!(
        run("nope\n(ex-message *e)\n", None),
        (
            0,
            "Syntax error compiling at (user:1:1).\njava.lang.RuntimeException | Unable to resolve symbol: nope in this context\n\"Syntax error compiling at (user:1:1).\\njava.lang.RuntimeException | Unable to resolve symbol: nope in this context\"\n"
                .to_owned()
        )
    );
}

#[test]
fn eval_error_binds_e_and_preserves_star1() {
    assert_eq!(
        run("7\n(/ 1 0)\n*1\n", None),
        (0, "7\nArithmeticException: Divide by zero\n7\n".to_owned())
    );
}

#[test]
fn eval_error_then_window_shifts_from_last_good() {
    assert_eq!(
        run("1\n(/ 1 0)\n2\n[*1 *2 *3]\n", None),
        (
            0,
            "1\nArithmeticException: Divide by zero\n2\n[2 1 nil]\n".to_owned()
        )
    );
}

#[test]
fn star_window_shifts_including_nil() {
    assert_eq!(
        run("1\n2\nnil\n[*1 *2 *3]\n", None),
        (0, "1\n2\nnil\n[nil 2 1]\n".to_owned())
    );
}

#[test]
fn prompt_off_purity_no_prompts_anywhere() {
    let (code, text) = run("(+ 1\n2)\n)\n", None);
    assert_eq!(code, 0);
    assert_eq!(text, "3\n1:2: Unmatched delimiter: )\n");
    assert!(!text.contains("=>"), "no prompt leaks: {text:?}");
}

#[test]
fn literal_prompt_without_ns_placeholder() {
    assert_eq!(run("1\n", Some(">>> ")), (0, ">>> 1\n>>> ".to_owned()));
}

#[test]
fn prompt_tracks_star_ns() {
    // A fresh `foo` refers nothing (like the JVM), so the second
    // form qualifies `+`.
    assert_eq!(
        run(
            "(do (in-ns 'foo) :moved)\n(clojure.core/+ 1 1)\n",
            Some("{ns}=> ")
        ),
        (0, "user=> :moved\nfoo=> 2\nfoo=> ".to_owned())
    );
}

#[test]
fn eof_mid_form_prints_diagnostic_and_exits_zero() {
    // The EOF span (2:1) matches the JVM's `(REPL:2:1)` verbatim.
    assert_eq!(
        run("(+ 1", None),
        (0, "2:1: EOF while reading, starting at line 1\n".to_owned())
    );
}

#[test]
fn clojure_version_transcript() {
    assert_eq!(
        run("(clojure-version)\n", None),
        (0, "\"1.13.0-alpha9\"\n".to_owned())
    );
    assert_eq!(
        run("*clojure-version*\n", None),
        (
            0,
            "{:major 1, :minor 13, :incremental 0, :qualifier \"alpha9\"}\n".to_owned()
        )
    );
}

#[test]
fn clojure_version_is_semver() {
    let mut eval = clj_eval::boot();
    // T1197 REL probe `rel-version`: the string and the map track
    // 1.13.0-alpha9 exactly (qualifier-aware shape).
    let version = run_last(&mut eval, "(clojure-version)");
    assert_eq!(version, "\"1.13.0-alpha9\"");
    let inner = version.trim_matches('"');
    let (core, qualifier) = inner.split_once('-').unwrap_or((inner, ""));
    let parts: Vec<&str> = core.split('.').collect();
    assert_eq!(parts.len(), 3, "major.minor.incremental: {inner:?}");
    for part in &parts {
        assert!(
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
            "numeric semver part: {part:?}"
        );
    }
    assert_eq!(qualifier, "alpha9", "release qualifier: {inner:?}");
    // The map agrees with the string (P3 shapes, no :interim key).
    assert_eq!(run_last(&mut eval, "(:major *clojure-version*)"), "1");
    assert_eq!(run_last(&mut eval, "(:minor *clojure-version*)"), "13");
    assert_eq!(run_last(&mut eval, "(:incremental *clojure-version*)"), "0");
    assert_eq!(
        run_last(&mut eval, "(:qualifier *clojure-version*)"),
        "\"alpha9\""
    );
    assert_eq!(run_last(&mut eval, "(:interim *clojure-version*)"), "nil");
}

#[test]
fn repl_var_roots_and_dynamics() {
    let mut eval = clj_eval::boot();
    for name in ["*1", "*2", "*3", "*e", "*command-line-args*"] {
        assert_eq!(run_last(&mut eval, name), "nil", "{name} root is nil");
        assert_eq!(
            run_last(&mut eval, &format!("(boolean (:dynamic (meta #'{name})))")),
            "true",
            "{name} is dynamic"
        );
    }
    assert_eq!(
        run_last(&mut eval, "(boolean (:dynamic (meta #'*clojure-version*)))"),
        "false",
        "*clojure-version* is a plain root"
    );
    // All seven resolve unqualified in `user` (coverage-matrix shape).
    for name in [
        "*1",
        "*2",
        "*3",
        "*e",
        "*command-line-args*",
        "clojure-version",
        "*clojure-version*",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(some? (resolve '{name}))")),
            "true",
            "{name} resolves"
        );
    }
}

#[test]
fn command_line_args_bind_path_round_trip() {
    // The S7A `run` bind path over the small public API: push a
    // `*command-line-args*` frame, eval against it, pop back to nil.
    let mut eval = clj_eval::boot();
    let handle = core_var(&eval, "*command-line-args*").expect("var is booted");
    let args = Value::List(List::from_vec(vec![
        Value::Str("a".into()),
        Value::Str("b".into()),
    ]));
    let depth = eval.push_binding(handle.clone(), args);
    assert_eq!(run_last(&mut eval, "(first *command-line-args*)"), "\"a\"");
    assert_eq!(run_last(&mut eval, "(count *command-line-args*)"), "2");
    eval.pop_bindings(depth);
    assert_eq!(run_last(&mut eval, "(nil? *command-line-args*)"), "true");
}

#[test]
fn incomplete_detection_per_variant() {
    // Each incomplete reader shape continues (true); anything else
    // — hard read errors, clean reads — does not (false).
    let mut eval = clj_eval::boot();
    for snippet in ["(+ 1", "\"abc", "#\"abc", "\\"] {
        match eval.eval_source(snippet) {
            Err(e) => assert!(
                is_incomplete_error(&e),
                "{snippet:?} continues: {}",
                snippet_message(&e)
            ),
            Ok(_) => panic!("{snippet:?} should not read clean"),
        }
    }
    for snippet in [")", "(def)", "#foo", "1 2)"] {
        match eval.eval_source(snippet) {
            Err(e) => assert!(
                !is_incomplete_error(&e),
                "{snippet:?} is a hard error: {}",
                snippet_message(&e)
            ),
            Ok(_) => panic!("{snippet:?} should not read clean"),
        }
    }
    assert!(eval.eval_source("(+ 1 2)").is_ok());
    assert!(eval.eval_source("").is_ok());
    assert!(eval.eval_source("; only a comment\n").is_ok());
}

/// Render a source error for assertion messages (never panics).
fn snippet_message(err: &clj_eval::SourceError) -> String {
    format!("{err}")
}

/// A writer that always fails (IO-failure exit paths).
struct FailWriter;

impl std::io::Write for FailWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("boom"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("boom"))
    }
}

/// A reader that always fails.
struct FailReader;

impl std::io::Read for FailReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("boom"))
    }
}

impl std::io::BufRead for FailReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("boom"))
    }

    fn consume(&mut self, _amt: usize) {}
}

#[test]
fn prompt_write_failure_exits_one() {
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new("(+ 1 2)\n".as_bytes().to_vec()));
    let mut session = Session::new(eval, reader, FailWriter);
    session.set_prompt(Some("{ns}=> ".to_owned()));
    assert_eq!(session.run(), 1);
}

#[test]
fn read_failure_exits_one() {
    let eval = clj_eval::boot();
    let mut session = Session::new(eval, FailReader, Vec::new());
    assert_eq!(session.run(), 1);
}

#[test]
fn print_write_failure_exits_one() {
    // No prompt, so the failure lands in `emit` → `write_line`
    // → `Done(1)`.
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new("1\n".as_bytes().to_vec()));
    let mut session = Session::new(eval, reader, FailWriter);
    assert_eq!(session.run(), 1);
}

#[test]
fn read_is_complete_splits_clean_and_partial() {
    assert!(read_is_complete("(+ 1 2)"));
    assert!(read_is_complete(""));
    assert!(!read_is_complete("(+ 1"));
    assert!(!read_is_complete("\"abc"));
}

#[test]
fn evaluator_handles_expose_session_state() {
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let mut session = Session::new(eval, reader, Vec::new());
    assert_eq!(session.evaluator().current_ns_name(), "user");
    let vals = session
        .evaluator_mut()
        .eval_source("(+ 1 2)")
        .expect("probe evals");
    assert!(matches!(vals.as_slice(), [Value::Int(3)]));
}

#[test]
fn render_prompt_tracks_ns_or_none() {
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let mut session = Session::new(eval, reader, Vec::new());
    assert_eq!(session.render_prompt(), None);
    session.set_prompt(Some("{ns}=> ".to_owned()));
    assert_eq!(session.render_prompt(), Some("user=> ".to_owned()));
}

#[test]
fn submit_complete_evals_or_reports() {
    let eval = clj_eval::boot();
    let reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let mut session = Session::new(eval, reader, Vec::new());
    assert_eq!(session.submit_complete("(+ 1 2)"), 0);
    assert_eq!(session.submit_complete("(+ 1"), 0);
    let (_, _, out) = session.into_parts();
    assert_eq!(
        out,
        b"3\n2:1: EOF while reading, starting at line 1\n".to_vec()
    );
}

#[test]
fn lazy_print_failure_binds_e_and_continues() {
    assert_eq!(
        run("(map (fn [_] (/ 1 0)) [1])\n(ex-message *e)\n", None),
        (
            0,
            "ArithmeticException: Divide by zero\n\"Divide by zero\"\n".to_owned()
        )
    );
}

#[test]
fn bad_print_binding_reports_cast_and_continues() {
    assert_eq!(
        run("(alter-var-root #'*print-length* (constantly \"x\"))\n", None),
        (
            0,
            "ClassCastException: class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')\n"
                .to_owned()
        )
    );
}

#[test]
fn repl_echo_print_dup_root_with_plain_fallback() {
    // S8/issue #51 §1d carve-out: the echo path composes
    // `print_opts` (`dup: true` under a true `*print-dup*` root)
    // with engine-direct `pr_to_string` (no `pr_on` driver). A
    // dupable kind echoes its dup shape (convergent with
    // `pr-str`); an undupable kind echoes the plain fallback
    // instead of the `No method` throw (pinned divergence — the
    // JVM REPL honors dup too, so the echo never forces
    // `dup: false`).
    assert_eq!(
        run(
            "(alter-var-root #'*print-dup* (constantly true))\n{:a 1}\n",
            None
        ),
        (
            0,
            "true\n#=(clojure.lang.PersistentArrayMap/create {:a 1})\n".to_owned()
        )
    );
    // The atom identity hash is allocator-shaped, so the
    // undupable pin is a prefix plus the no-throw fact.
    let (code, text) = run(
        "(alter-var-root #'*print-dup* (constantly true))\n(atom 1)\n",
        None,
    );
    assert_eq!(code, 0);
    assert!(
        text.starts_with("true\n#object[clojure.lang.Atom 0x"),
        "{text:?}"
    );
    assert!(!text.contains("No method"), "{text:?}");
    // Fn/namespace echoes converge with `pr-str` dup shapes (the
    // engine `dup_shape` hook); realized seqs echo the plain
    // delegation (the hook's `None` arm).
    let (code, text) = run(
        "(alter-var-root #'*print-dup* (constantly true))\ninc\n(find-ns 'user)\n(map inc [1 2])\n",
        None,
    );
    assert_eq!(code, 0);
    assert!(text.contains("#=(clojure.core$inc. )"), "{text:?}");
    assert!(text.contains("#=(find-ns user)"), "{text:?}");
    assert!(text.contains("(2 3)"), "{text:?}");
    // Closures converge too (allocator-shaped display, so
    // contains-pins like `d-anonfn`).
    let (code, text) = run(
        "(alter-var-root #'*print-dup* (constantly true))\n(fn [x] x)\n",
        None,
    );
    assert_eq!(code, 0);
    assert!(
        text.contains("#=(") && text.contains("$fn__") && text.contains(". )"),
        "{text:?}"
    );
}
