// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! CLI contract tests: help/version/usage codes (slice S1 shell) plus
//! the S7A `eval`/`run` wiring (spec 1.3/1.4, pins P4/P5/P7).
//! `repl` landed in S7B/C, `build` in S8. The `build` tests here
//! stay hermetic (emit flags + error exits, no linking); the S8D
//! corpus replay (`build_corpus.rs`) covers full link + execute.

mod common;

use common::{run, run_with_stdin, stderr, stdout};

#[test]
fn help_lists_all_subcommands() {
    let out = run(["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    for cmd in ["eval", "run", "repl", "build"] {
        assert!(text.contains(cmd), "missing subcommand {cmd}:\n{text}");
    }
}

#[test]
fn version_reports_binary_name() {
    let out = run(["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(
        text.starts_with("clojure-rs "),
        "unexpected version line: {text}"
    );
}

#[test]
fn missing_subcommand_is_usage_error() {
    // Spec C8: usage errors exit 2 (clap's default).
    let out = run(Vec::<String>::new());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn eval_missing_form_is_usage_error() {
    let out = run(["eval"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn eval_prints_result() {
    // Spec BDD: integer arithmetic prints exactly "6\n", exit 0.
    let out = run(["eval", "(+ 1 2 3)"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "6\n");
}

#[test]
fn eval_error_reports_position() {
    // Spec BDD + pins P5: exit 1, `file:line:col: message` naming
    // the `<eval>` token, the position, and "Divide by zero".
    // (`Throwable` carries no span, so runtime errors synthesize
    // the single form's start, 1:1 — see main.rs.)
    let out = run(["eval", "(/ 1 0)"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "<eval>:1:1: java.lang.ArithmeticException: Divide by zero\n"
    );
}

#[test]
fn eval_compile_error_reports_position() {
    // Analyzer spans survive the eval wrapper unshifted on line 1.
    let out = run(["eval", "(if)"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "<eval>:1:1: Syntax error compiling if: Too few arguments to if\n"
    );
}

#[test]
fn eval_reader_error_reports_position() {
    // The reader's spans are post-read tracker positions (1:2 is one
    // past the stray closer).
    let out = run(["eval", ")"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "<eval>:1:2: Unmatched delimiter: )\n");
}

#[test]
fn eval_unclosed_input_is_eof_error() {
    // Unclosed input errors with the reader's own span (col 1,
    // line past a non-empty last line; the opener line is the
    // innermost unclosed one).
    let out = run(["eval", "("]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "<eval>:2:1: EOF while reading, starting at line 1\n"
    );
    let out = run(["eval", "((("]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "<eval>:2:1: EOF while reading, starting at line 1\n"
    );
    let out = run(["eval", "(do\n("]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "<eval>:3:1: EOF while reading, starting at line 2\n"
    );
}

#[test]
fn eval_blank_input_is_silent() {
    // JVM `-e ''` parity (pins P5): blank input prints nothing,
    // exit 0 — including vertical-tab-only (unified blank check)
    // and comment-only input.
    for form in ["", "   ", "\u{b}", "  ; nothing but a comment\n"] {
        let out = run(["eval", form]);
        assert_eq!(out.status.code(), Some(0), "form {form:?}");
        assert_eq!(stdout(&out), "", "form {form:?}");
        assert_eq!(stderr(&out), "", "form {form:?}");
    }
}

#[test]
fn eval_ignores_trailing_comments() {
    // Nothing is appended to the input line, so trailing `;`/`#!`
    // comments swallow no closers.
    let out = run(["eval", "(+ 1 2) ; hello"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n");
    let out = run(["eval", "(+ 1 2) #! tail"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n");
    let out = run(["eval", "#! (oops\n(+ 1 2)"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n");
}

#[test]
fn eval_multi_form_prints_each_value() {
    // JVM `-e '1 2'` parity (pins P5): every form prints.
    let out = run(["eval", "1 2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "1\n2\n");
}

#[test]
fn eval_suppresses_nil_results() {
    // JVM `-e` prints nothing for nil (pins P5).
    let out = run(["eval", "nil"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
    let out = run(["eval", "1 nil 2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "1\n2\n");
}

#[test]
fn eval_edn_restricts_reader() {
    // `--edn` rejects the fn-literal dispatch (non-EDN syntax)...
    let out = run(["eval", "--edn", "(map #(inc %) [1])"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "<eval>:1:6: non-EDN reader syntax: '#('\n");
    // ...while the same form evaluates without the flag...
    let out = run(["eval", "(map #(inc %) [1])"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "(2)\n");
    // ...and EDN-clean input passes under the flag.
    let out = run(["eval", "--edn", "(+ 1 2 3)"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "6\n");
}

#[test]
fn eval_edn_rejects_symbolic_and_auto_resolve() {
    // Neither `##NaN` (no EDN literal) nor `::foo` (needs ns
    // context) is EDN — but single-`:` keywords stay valid.
    let out = run(["eval", "--edn", "##NaN"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "<eval>:1:1: non-EDN reader syntax: '##'\n");
    let out = run(["eval", "--edn", "::foo"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "<eval>:1:1: non-EDN reader syntax: '::'\n");
    let out = run(["eval", "--edn", ":foo"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), ":foo\n");
}

#[test]
fn eval_edn_accepts_discard() {
    // `#_` is in the EDN spec: the discarded form vanishes.
    let out = run(["eval", "--edn", "#_ 1 2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "2\n");
}

#[test]
fn run_missing_file_is_io_error() {
    // Spec C8: I/O errors exit 3 and name the file.
    let out = run(["run", "nope-missing.clj"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("nope-missing.clj"));
}

#[test]
#[cfg(unix)]
fn run_unreadable_file_is_io_error() {
    // Read-time IO failures (chmod-000) exit 3 per C8 — not 1.
    // Unix-only: Windows ACLs have no chmod-000 equivalent. The
    // test self-calibrates: when the file stays readable after
    // chmod (running as root, e.g. the quality-gates container),
    // there is no IO failure to assert and the test passes
    // vacuously.
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("clojure-rs-noperm-{}.clj", std::process::id()));
    std::fs::write(&path, "(println :nope)\n").expect("write probe file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("chmod probe file");
    if std::fs::read(&path).is_ok() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("restore probe file");
        std::fs::remove_file(&path).expect("remove probe file");
        eprintln!("skipping: probe file readable despite chmod-000 (root?)");
        return;
    }
    let out = run(["run", path.to_string_lossy().as_ref()]);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("restore probe file");
    std::fs::remove_file(&path).expect("remove probe file");
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr(&out).contains("cannot open"),
        "stderr:\n{}",
        stderr(&out)
    );
}

#[test]
fn run_directory_names_the_real_problem() {
    // A directory reports itself — not "no such file".
    let dir = std::env::temp_dir().join(format!("clojure-rs-isdir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create probe dir");
    let out = run(["run", dir.to_string_lossy().as_ref()]);
    std::fs::remove_dir(&dir).expect("remove probe dir");
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr(&out).contains("is a directory"),
        "stderr:\n{}",
        stderr(&out)
    );
}

#[test]
fn run_hello_script() {
    // Spec BDD: hello world script prints exactly "hello\n", exit 0.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "hello\n");
}

#[test]
fn run_binds_command_line_args() {
    // Pins P4: script args arrive as a seq `("a" "b")`.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/args.clj");
    let out = run(["run", fixture, "a", "b"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "(\"a\" \"b\")\n");
}

#[test]
fn run_no_args_binds_nil() {
    // Pins P4: `*command-line-args*` is nil when no args are given.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/args.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "nil\n");
}

#[test]
fn run_prints_no_per_form_echo() {
    // Pins P7: program output only; defs and values stay silent.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/quiet.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
}

#[test]
fn run_compile_error_names_file_line_col() {
    // Pins P5/P7: exit 1 with a `file:line:col: message` diagnostic
    // naming the path as passed; earlier forms still ran.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/run-err.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "before\n");
    assert_eq!(
        stderr(&out),
        format!("{fixture}:13:1: Syntax error compiling if: Too few arguments to if\n")
    );
}

#[test]
fn run_runtime_error_is_exit_1() {
    // Runtime failures exit 1 naming the file; the 1:1 is
    // synthesized (`Throwable` carries no span — see main.rs).
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/boom.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "boom-start\n");
    assert_eq!(
        stderr(&out),
        format!("{fixture}:1:1: java.lang.ArithmeticException: Divide by zero\n")
    );
}

#[test]
fn repl_streams_one_form_per_result() {
    // Streaming purity (pins P6): piped stdin prints results only —
    // no banner, no prompts — and exits 0 on EOF.
    let out = run_with_stdin(["repl"], "(+ 1 2)\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn repl_streams_multiline_forms() {
    let out = run_with_stdin(["repl"], "(+ 1\n2)\n(* 3 4)\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n12\n");
}

#[test]
fn repl_session_survives_errors() {
    // Errors print, bind `*e`, and the session continues; the exit
    // stays 0 and `*1` keeps the last good value.
    let out = run_with_stdin(["repl"], "(+ 1 2)\n(/ 1 0)\n*1\n");
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.starts_with("3\n"), "first result:\n{text}");
    assert!(text.contains("Divide by zero"), "diagnostic:\n{text}");
    assert!(text.ends_with("3\n"), "star-1 preserved:\n{text}");
}

#[test]
fn repl_empty_input_exits_clean() {
    let out = run_with_stdin(["repl", "--no-history"], "");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
}

#[test]
fn repl_streaming_ignores_history_flags() {
    // Off-tty there is no history write (spec): a bogus history
    // path must not fail the streaming session.
    let out = run_with_stdin(["repl", "--history-file", "/nonexistent/x"], "(+ 1 2)\n");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "3\n");
}

/// A scratch dir for one `build` CLI test (pid-suffixed, removed
/// by the test).
fn build_tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("clj-rs-cli-{}-{name}", std::process::id()))
}

/// `build` argv as uniform `OsStr`s (paths + literals mix).
fn build_argv(args: &[&std::ffi::OsStr]) -> std::process::Output {
    run(args.iter().copied())
}

#[test]
fn build_missing_input_exits_3() {
    // The IO gate runs on every configuration (linked or not).
    use std::ffi::OsStr;
    let dir = build_tmp("missing");
    let missing = dir.join("nope.clj");
    let out = build_argv(&[
        OsStr::new("build"),
        missing.as_os_str(),
        OsStr::new("-o"),
        OsStr::new("prog"),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(err.contains("build: no such file"), "{err}");
}

#[test]
fn build_directory_input_exits_3() {
    use std::ffi::OsStr;
    let dir = build_tmp("dir");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let out = build_argv(&[
        OsStr::new("build"),
        dir.as_os_str(),
        OsStr::new("-o"),
        OsStr::new("prog"),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(err.contains("build: is a directory"), "{err}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(unix)]
fn build_unreadable_input_exits_3() {
    // A Unix socket exists and is not a dir, but can never be
    // read — unlike chmod 000, this also holds for root (R1
    // finding 5: CI containers run privileged).
    use std::ffi::OsStr;
    use std::os::unix::net::UnixListener;
    let dir = build_tmp("unreadable");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("dark.clj");
    let socket = UnixListener::bind(&file).expect("bind socket");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        OsStr::new("prog"),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(err.contains("build: cannot open"), "{err}");
    drop(socket);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(feature = "aot")]
fn build_emit_ir_writes_ir_without_linking() {
    use std::ffi::OsStr;
    let dir = build_tmp("emit-ir");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let ir = dir.join("hello.ll");
    let prog = dir.join("prog");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
        OsStr::new("--emit-ir"),
        ir.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let text = std::fs::read_to_string(&ir).expect("read ir");
    assert!(text.contains("ModuleID"), "not LLVM IR:\n{text}");
    assert!(!prog.exists(), "linking must be skipped");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(feature = "aot")]
fn build_emit_obj_writes_object_without_linking() {
    use std::ffi::OsStr;
    let dir = build_tmp("emit-obj");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let obj = dir.join("hello.o");
    let prog = dir.join("prog");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
        OsStr::new("--emit-obj"),
        obj.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        std::fs::metadata(&obj).expect("obj meta").len() > 0,
        "empty object file"
    );
    assert!(!prog.exists(), "linking must be skipped");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(feature = "aot")]
fn build_plan_error_passes_the_diagnostic_through() {
    // Plan failures print verbatim (no `build: ` prefix), exit 1.
    use std::ffi::OsStr;
    let dir = build_tmp("plan-err");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("bad.clj");
    std::fs::write(&file, "(def x 1)\n").expect("write source");
    let prog = dir.join("prog");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!(
            "{}:1:1: unsupported form in AOT subset: def (only defn defines in the AOT subset)\n",
            file.display()
        )
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(feature = "aot")]
fn build_unwritable_emit_path_exits_3() {
    // Emit writes are IO (exit 3), not compile failures.
    use std::ffi::OsStr;
    let dir = build_tmp("emit-io");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "x").expect("write blocker");
    let bad = blocker.join("hello.ll");
    let prog = dir.join("prog");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
        OsStr::new("--emit-ir"),
        bad.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(err.starts_with("build: "), "{err}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(feature = "aot")]
fn build_missing_runtime_archive_exits_3() {
    // A bogus `--runtime-archive` fails archive discovery (exit 3).
    use std::ffi::OsStr;
    let dir = build_tmp("archive");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let prog = dir.join("prog");
    let missing = dir.join("nope.a");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
        OsStr::new("--runtime-archive"),
        missing.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(err.starts_with("build: "), "{err}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(all(feature = "aot", unix))]
fn build_temp_object_failure_exits_1() {
    // The temp object cannot be written (missing TMPDIR): an LLVM
    // emission failure, exit 1 with `build: ` (not an IO exit 3).
    use std::ffi::OsStr;
    let dir = build_tmp("tmpdir");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let prog = dir.join("prog");
    let out = common::bin()
        .args([
            OsStr::new("build"),
            file.as_os_str(),
            OsStr::new("-o"),
            prog.as_os_str(),
        ])
        .env(
            "TMPDIR",
            dir.join(format!("missing-{}", std::process::id())),
        )
        .output()
        .expect("spawn clojure-rs");
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.starts_with("build: "), "{err}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
#[cfg(not(feature = "aot"))]
fn build_reports_unlinked_llvm() {
    // Readable input reaches the backend report (the IO gate runs
    // first, so a missing file would exit 3 instead).
    use std::ffi::OsStr;
    let dir = build_tmp("unlinked");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("hello.clj");
    std::fs::write(&file, "(print (+ 40 2))\n").expect("write source");
    let prog = dir.join("prog");
    let out = build_argv(&[
        OsStr::new("build"),
        file.as_os_str(),
        OsStr::new("-o"),
        prog.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("LLVM backend not linked"));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn eval_lazy_print_failure_reports_simply() {
    // `map` is lazy, so eval succeeds and the divide-by-zero
    // surfaces at print time with the SIMPLE class.
    let out = run(["eval", "(map (fn [_] (/ 1 0)) [1])"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "<eval>:1:1: ArithmeticException: Divide by zero\n"
    );
}

#[test]
fn eval_bad_print_binding_reports_cast() {
    // A non-numeric `*print-length*` root fails option loading
    // with the verbatim CCE text.
    let out = run([
        "eval",
        "(alter-var-root #'*print-length* (constantly \"x\"))",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "<eval>:1:1: ClassCastException: class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')\n"
    );
}
