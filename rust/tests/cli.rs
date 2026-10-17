// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! CLI contract tests (slice S1 shell): help/version/usage codes and
//! honest not-implemented diagnostics. Later slices extend this file
//! as each subcommand lands.

mod common;

use common::{run, stderr, stdout};

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
fn eval_reports_not_implemented() {
    let out = run(["eval", "(+ 1 2)"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not yet implemented"));
}

#[test]
fn run_missing_file_is_io_error() {
    // Spec C8: I/O errors exit 3 and name the file.
    let out = run(["run", "nope-missing.clj"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("nope-missing.clj"));
}

#[test]
fn run_existing_file_reports_not_implemented() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.clj");
    let out = run(["run", fixture]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not yet implemented"));
}

#[test]
fn repl_reports_not_implemented() {
    let out = run(["repl", "--no-history"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not yet implemented"));
}

#[test]
#[cfg(feature = "aot")]
fn build_reports_linked_llvm() {
    let out = run(["build", "f.clj", "-o", "f"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("not yet implemented"), "{err}");
    assert!(err.contains("LLVM backend linked"), "{err}");
}

#[test]
#[cfg(not(feature = "aot"))]
fn build_reports_unlinked_llvm() {
    let out = run(["build", "f.clj", "-o", "f"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("LLVM backend not linked"));
}
