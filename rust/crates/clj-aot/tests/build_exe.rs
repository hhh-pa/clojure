// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Full link + execute: real `cc`/`link` against the real archive,
//! then run the binary and pin stdout/exit (the S8D corpus replay
//! generalizes this over every corpus file).

use std::path::PathBuf;
use std::process::Command;

use clj_aot::{EmitOptions, LinkOptions, compile_str};

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clj-aot-exe-{}-{name}", std::process::id()))
}

/// The workspace target dir (this test binary lives in
/// `<target>/<profile>/deps/` whatever `CARGO_TARGET_DIR` is).
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current exe");
    exe.parent()
        .expect("deps")
        .parent()
        .expect("profile")
        .to_path_buf()
}

/// Ensure the static archive exists (plain `cargo test` only builds
/// the rlib — build the staticlib on demand, cached by fingerprints).
/// Builds in an isolated `cljrt-archive` target dir with coverage
/// instrumentation stripped: sharing the test target dir would reuse
/// instrumented dependency objects, whose profile-runtime symbols no
/// plain `cc` link can resolve. Built once per test process — parallel
/// tests must never spawn competing nested cargo builds.
fn ensure_archive() -> PathBuf {
    static ARCHIVE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ARCHIVE.get_or_init(build_archive).clone()
}

fn build_archive() -> PathBuf {
    let dir = target_dir();
    let name = if cfg!(windows) {
        "cljrt.lib"
    } else {
        "libcljrt.a"
    };
    let tops = dir.parent().expect("target root").join("cljrt-archive");
    let archive = tops.join("debug").join(name);
    if archive.is_file() {
        return archive;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("workspace")
        .join("Cargo.toml");
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--locked", "-p", "clj-rt", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(dir.parent().expect("target root").join("cljrt-archive"));
    cmd.env_remove("RUSTC_WRAPPER")
        .env_remove("LLVM_PROFILE_FILE");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().contains("LLVM_COV") {
            cmd.env_remove(key);
        }
    }
    let built = cmd.status().expect("run cargo build");
    assert!(built.success(), "cargo build -p clj-rt failed");
    assert!(archive.is_file(), "missing {}", archive.display());
    archive
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

/// Build `source` to a binary and run it (stdout + exit pinned).
fn build_and_run(name: &str, source: &str) -> (String, String, i32) {
    let dir = tmp(name);
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let out = dir.join(format!("prog{}", exe_suffix()));
    compile_str(
        source,
        "f.clj",
        &out,
        &EmitOptions::default(),
        &LinkOptions {
            runtime_archive: Some(ensure_archive()),
            exe_dir: dir.clone(),
            linker: None,
        },
    )
    .expect("compile");
    let ran = Command::new(&out).output().expect("run binary");
    let code = ran.status.code().expect("exit code");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&ran.stderr).into_owned();
    if code != 0 || !stderr.is_empty() {
        eprintln!("binary failed: {code} stdout={stdout:?} stderr={stderr:?}");
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
    (stdout, stderr, code)
}

#[test]
fn linked_binary_prints() {
    let (stdout, stderr, code) =
        build_and_run("hello", "(defn f [x] (+ x 1)) (print (f 41) \"done\")");
    assert_eq!((stdout.as_str(), stderr.as_str(), code), ("42 done", "", 0));
}

#[test]
fn linked_binary_runs_control_flow() {
    let (stdout, stderr, code) = build_and_run(
        "control",
        "(let [x 7] (if (> x 5) (print \"big\" x) (print \"small\"))) (print (nth [1 2] 1))",
    );
    assert_eq!((stdout.as_str(), stderr.as_str(), code), ("big 72", "", 0));
}

#[test]
fn linked_binary_survives_untaken_let_branches() {
    // T73 item 9: a `let` in an untaken branch never stores — the
    // scope-end drop must see nil-inited slots, not stack garbage
    // (pre-fix: correct stdout, then SIGABRT/SIGSEGV in teardown).
    let (stdout, stderr, code) = build_and_run(
        "cond-let",
        "(defn f [c] (if c (let [x 1] x) (let [y 2] y))) \
         (print (f false) (f true) (if false (let [z 3] z) 4))",
    );
    assert_eq!((stdout.as_str(), stderr.as_str(), code), ("2 1 4", "", 0));
}

#[test]
fn linked_binary_flows_keywords() {
    // T73 item 8: keywords are subset values — the vector-001
    // shape that used to fail the build with `unsupported form
    // in AOT subset: :miss` (JVM prints the same line).
    let (stdout, stderr, code) = build_and_run(
        "keyword",
        "(print [(get [1 2] 5 :miss) (= :a :a) (= :a :b) :x :u/id])",
    );
    assert_eq!(
        (stdout.as_str(), stderr.as_str(), code),
        ("[:miss true false :x :u/id]", "", 0)
    );
}

#[test]
fn linked_binary_reports_runtime_errors() {
    let (stdout, stderr, code) = build_and_run("divzero", "(print (quot 1 0))");
    assert_eq!(stdout, "");
    assert_eq!(code, 1);
    assert_eq!(
        stderr,
        "f.clj:1:8: java.lang.ArithmeticException: / by zero\n"
    );
}
