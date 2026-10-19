// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S7 transcript corpus (`docs/t1174/s7-corpus/`): replay every
//! session script through the built binary's streaming `repl` and
//! assert the pinned transcript byte-exact (no JVM needed; the JVM
//! differential half lives in the corpus MANIFEST).

mod common;

use common::{run, run_with_stdin, stderr, stdout};
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/t1174/s7-corpus")
        .canonicalize()
        .expect("s7-corpus dir")
}

fn check_session(name: &str) {
    let dir = corpus_dir();
    let input =
        std::fs::read_to_string(dir.join(format!("{name}.in"))).expect("read session script");
    let expected = std::fs::read_to_string(dir.join(format!("expected/{name}.ours.out")))
        .expect("read pinned transcript")
        // Belt-and-braces over .gitattributes (which pins LF): a stale
        // CRLF checkout must not fail the comparison.
        .replace("\r\n", "\n");
    let out = run_with_stdin(["repl", "--no-history"], &input);
    assert_eq!(out.status.code(), Some(0), "{name}: exit code");
    assert_eq!(stdout(&out), expected, "{name}: transcript");
    assert!(stderr(&out).is_empty(), "{name}: stderr:\n{}", stderr(&out));
}

#[test]
fn repl_help_documents_history_flags() {
    let out = run(["repl", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    for flag in ["--history-file", "--no-history"] {
        assert!(text.contains(flag), "missing {flag}:\n{text}");
    }
}

#[test]
fn transcript_basics() {
    check_session("01-basics");
}

#[test]
fn transcript_multiline() {
    check_session("02-multiline");
}

#[test]
fn transcript_errors() {
    check_session("03-errors");
}

#[test]
fn transcript_ns() {
    check_session("04-ns");
}
