// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Issue #259 native-compiler replay: every
//! `docs/spec/i259-native-compiler/corpus` program runs through the
//! full native compiler (`jit`: compiled in-process, every `rt_n_*`
//! shim live) and must match `run` byte for byte — stdout, stderr,
//! and exit code. `err_*` programs abort (exit 1, the throw on
//! stderr); every other program exits 0 with empty stderr.

#![cfg(feature = "aot")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The corpus dir (absolute: diagnostics embed the path).
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/spec/i259-native-compiler/corpus")
        .canonicalize()
        .expect("i259 corpus dir")
}

/// `clojure-rs <mode> <file> args…` run from the corpus dir (relative file
/// name, so spans match across modes).
fn run_mode(mode: &str, dir: &Path, file: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clojure-rs"))
        .args([mode, file, "arg-1", "arg two"])
        .current_dir(dir)
        .output()
        .expect("spawn clojure-rs")
}

#[test]
fn native_jit_matches_run_on_the_i259_corpus() {
    let dir = corpus_dir();
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("read corpus")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".clj"))
        .collect();
    files.sort();
    assert!(files.len() >= 9, "corpus shrank: {files:?}");
    for file in &files {
        let run = run_mode("run", &dir, file);
        let jit = run_mode("jit", &dir, file);
        let expect_code = if file.starts_with("err_") { 1 } else { 0 };
        assert_eq!(run.status.code(), Some(expect_code), "{file}: run exit");
        if expect_code == 0 {
            assert!(
                run.stderr.is_empty(),
                "{file}: run stderr {}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
        assert_eq!(
            (
                String::from_utf8_lossy(&jit.stdout),
                String::from_utf8_lossy(&jit.stderr),
                jit.status.code()
            ),
            (
                String::from_utf8_lossy(&run.stdout),
                String::from_utf8_lossy(&run.stderr),
                run.status.code()
            ),
            "{file}: jit vs run"
        );
    }
}
