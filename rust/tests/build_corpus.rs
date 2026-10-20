// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S8D corpus replay: every `docs/t1174/s8-corpus` file, differentially
//! `run` vs `build`+execute (rules in `MANIFEST.md`). Supported files
//! must print byte-identical stdout on both paths (exit 0, empty
//! stderr); error programs pin the AOT abort stderr (true call-site
//! spans, exit 1); unsupported files pin the exact `build`
//! diagnostic (exit 1) while `run` still works (exit 0).

#![cfg(feature = "aot")]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// A `Command` for the `clojure-rs` binary under test.
fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clojure-rs"))
}

/// The corpus dir (absolute, canonicalized — diagnostics embed the
/// exact path passed to `build`).
fn corpus_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/t1174/s8-corpus");
    dir.canonicalize().expect("corpus dir")
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clj-rs-corpus-{}-{name}", std::process::id()))
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

/// Ensure the static `clj-rt` archive exists (twin of the
/// `clj-aot/tests/build_exe.rs` helper: plain `cargo test` only
/// builds the rlib, so build the staticlib on demand in an isolated
/// `cljrt-archive` target dir with coverage instrumentation
/// stripped — sharing the test target dir would reuse instrumented
/// dependency objects, whose profile-runtime symbols no plain `cc`
/// link can resolve).
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
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
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

/// `run` a corpus file: (stdout bytes, stderr bytes, exit code).
fn run_file(path: &std::path::Path) -> (Vec<u8>, Vec<u8>, i32) {
    let out = bin()
        .arg("run")
        .arg(path)
        .output()
        .expect("spawn clojure-rs run");
    (
        out.stdout,
        out.stderr,
        out.status.code().expect("exit code"),
    )
}

/// `build` a corpus file, then run the binary:
/// (build code, build stderr, exe stdout, exe stderr, exe code).
fn build_and_run(name: &str, path: &std::path::Path) -> (i32, Vec<u8>, Vec<u8>, Vec<u8>, i32) {
    let dir = tmp(name);
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let prog = dir.join(format!("prog{}", exe_suffix()));
    let archive = ensure_archive();
    let built = bin()
        .args([
            OsStr::new("build"),
            path.as_os_str(),
            OsStr::new("-o"),
            prog.as_os_str(),
            OsStr::new("--runtime-archive"),
            archive.as_os_str(),
        ])
        .output()
        .expect("spawn clojure-rs build");
    let build_code = built.status.code().expect("build exit code");
    let build_err = built.stderr;
    let (exe_out, exe_err, exe_code) = if build_code == 0 {
        let ran = Command::new(&prog).output().expect("run binary");
        (
            ran.stdout,
            ran.stderr,
            ran.status.code().expect("exe exit code"),
        )
    } else {
        (Vec::new(), Vec::new(), -1)
    };
    std::fs::remove_dir_all(&dir).expect("cleanup");
    (build_code, build_err, exe_out, exe_err, exe_code)
}

#[test]
fn supported_files_match_run_byte_for_byte() {
    // 00-05: both paths exit 0, empty stderr, stdout == the
    // `run`-seeded golden (which itself matches the JVM golden).
    let corpus = corpus_dir();
    let files = [
        "00-empty",
        "01-arith",
        "02-recursion",
        "03-types",
        "04-let-do-if",
        "05-print",
    ];
    for stem in files {
        let path = corpus.join(format!("{stem}.clj"));
        let golden = std::fs::read(corpus.join(format!("expected/{stem}.out"))).expect("golden");
        let (run_out, run_err, run_code) = run_file(&path);
        assert_eq!(run_code, 0, "{stem}: run exit");
        assert!(run_err.is_empty(), "{stem}: run stderr: {run_err:?}");
        assert_eq!(run_out, golden, "{stem}: run stdout vs golden");
        let (build_code, build_err, exe_out, exe_err, exe_code) = build_and_run(stem, &path);
        assert_eq!(build_code, 0, "{stem}: build exit, stderr: {build_err:?}");
        assert!(build_err.is_empty(), "{stem}: build stderr: {build_err:?}");
        assert_eq!(exe_code, 0, "{stem}: exe exit");
        assert!(exe_err.is_empty(), "{stem}: exe stderr: {exe_err:?}");
        assert_eq!(exe_out, golden, "{stem}: exe stdout vs golden");
    }
}

#[test]
fn error_programs_pin_the_aot_abort() {
    // 06-09: `run` reports `{path}:1:1:` (synthesized span);
    // the AOT binary reports the TRUE call-site span (line 2,
    // col 8 in each file) — class/message match the oracle.
    let corpus = corpus_dir();
    let cases: &[(&str, &str, &str)] = &[
        ("06-err-quot0", "java.lang.ArithmeticException", "/ by zero"),
        ("07-err-nth", "java.lang.IndexOutOfBoundsException", ""),
        (
            "08-err-count",
            "java.lang.UnsupportedOperationException",
            "count not supported on this type: Long",
        ),
        (
            "09-err-arity",
            "clojure.lang.ArityException",
            "Wrong number of args (0) passed to: clojure.core/-",
        ),
    ];
    for (stem, class, message) in cases {
        let path = corpus.join(format!("{stem}.clj"));
        let golden = std::fs::read(corpus.join(format!("expected/{stem}.out"))).expect("golden");
        let runerr =
            std::fs::read(corpus.join(format!("expected/{stem}.runerr"))).expect("runerr golden");
        let (run_out, run_err, run_code) = run_file(&path);
        assert_eq!(run_code, 1, "{stem}: run exit");
        assert_eq!(run_out, golden, "{stem}: run stdout vs golden");
        // The `.runerr` golden pins the class/message after its
        // relative-path prefix; the replayed stderr carries the
        // same text after the absolute replay path (split at the
        // span, never at `/` — Windows paths are backslashed).
        let golden_text = lossy(&runerr);
        let (_, run_message) = golden_text
            .split_once(":1:1: ")
            .expect("runerr golden shape");
        let want = format!("{}:1:1: {run_message}", path.display());
        assert_eq!(lossy(&run_err), want, "{stem}: run stderr");
        let (build_code, build_err, exe_out, exe_err, exe_code) = build_and_run(stem, &path);
        assert_eq!(build_code, 0, "{stem}: build exit, stderr: {build_err:?}");
        assert_eq!(exe_code, 1, "{stem}: exe exit");
        assert_eq!(exe_out, golden, "{stem}: exe stdout vs golden");
        let want = format!("{}:2:8: {class}: {message}\n", path.display());
        assert_eq!(lossy(&exe_err), want, "{stem}: exe stderr");
    }
}

#[test]
fn unsupported_files_fail_build_but_run() {
    // 01-16: `run` still works (exit 0, `.runout` stdout, empty
    // stderr — the BDD clause); `build` exits 1 naming the first
    // unsupported form: `{path}:{line}:{col}: unsupported form in
    // AOT subset: {form} ({hint})`. Forms/hints are the S8A
    // rejection-table contract; spans read off the sources.
    let corpus = corpus_dir();
    let cases: &[(&str, u32, u32, &str, &str)] = &[
        ("01-def", 1, 1, "def", "only defn defines in the AOT subset"),
        (
            "02-loop",
            1,
            8,
            "loop",
            "the AOT subset is loop-free; use direct self-recursion",
        ),
        (
            "03-recur",
            1,
            27,
            "recur",
            "recur is outside the subset; call the function by name",
        ),
        (
            "04-try",
            1,
            8,
            "try",
            "error handling is outside the AOT subset",
        ),
        ("05-var", 1, 8, "var", "vars are outside the AOT subset"),
        (
            "06-future",
            1,
            1,
            "clojure.core/deref",
            "concurrency is outside the AOT subset",
        ),
        ("07-lazy-seq", 1, 8, "take", "not in the closed call set"),
        (
            "08-println",
            1,
            1,
            "println",
            "only print writes in the AOT subset",
        ),
        (
            "09-ns",
            1,
            1,
            "ns",
            "AOT programs are single-file, without namespaces",
        ),
        (
            "10-when",
            1,
            1,
            "when",
            "macros are not expanded in the AOT subset",
        ),
        (
            "11-defmacro",
            1,
            1,
            "defmacro",
            "macros at runtime are outside the subset",
        ),
        (
            "12-fn",
            1,
            9,
            "fn",
            "only defn defines functions in the AOT subset",
        ),
        (
            "13-quote",
            1,
            8,
            "quote",
            "quoted values are outside the value flow",
        ),
        (
            "14-defn-multi",
            1,
            1,
            "defn",
            "multi-arity; use a single vector clause",
        ),
        (
            "15-defn-prepost",
            1,
            1,
            "defn",
            "pre/post conditions are runtime checks",
        ),
        (
            "16-let-destructure",
            1,
            1,
            "let",
            "destructuring is outside the subset; bind plain symbols",
        ),
    ];
    for (stem, line, col, form, hint) in cases {
        let path = corpus.join(format!("unsupported/{stem}.clj"));
        let runout = std::fs::read(corpus.join(format!("expected/unsupported/{stem}.runout")))
            .expect("runout golden");
        let (run_out, run_err, run_code) = run_file(&path);
        assert_eq!(run_code, 0, "{stem}: run exit");
        assert!(run_err.is_empty(), "{stem}: run stderr: {run_err:?}");
        assert_eq!(run_out, runout, "{stem}: run stdout vs golden");
        let (build_code, build_err, _, _, _) = build_and_run(stem, &path);
        assert_eq!(build_code, 1, "{stem}: build exit");
        let want = format!(
            "{}:{line}:{col}: unsupported form in AOT subset: {form} ({hint})\n",
            path.display()
        );
        assert_eq!(lossy(&build_err), want, "{stem}: build stderr");
    }
}

#[test]
fn malformed_files_report_direct_spans() {
    // 17: the analyzer diagnostic is byte-identical on both paths
    // (same invocation path). 18: `build` reports the raw reader
    // message WITHOUT `run`'s load-file wrapper — pinned divergence.
    let corpus = corpus_dir();
    let p17 = corpus.join("unsupported/17-analyzer.clj");
    let (run_out, run_err, run_code) = run_file(&p17);
    assert_eq!(run_code, 1, "17: run exit");
    assert!(run_out.is_empty(), "17: run stdout: {run_out:?}");
    let (build_code, build_err, _, _, _) = build_and_run("17-analyzer", &p17);
    assert_eq!(build_code, 1, "17: build exit");
    assert_eq!(build_err, run_err, "17: build stderr == run stderr");
    let want17 = format!(
        "{}:1:1: Syntax error compiling if: Too few arguments to if\n",
        p17.display()
    );
    assert_eq!(lossy(&build_err), want17, "17: exact text");

    let p18 = corpus.join("unsupported/18-reader.clj");
    let (run_out, run_err, run_code) = run_file(&p18);
    assert_eq!(run_code, 1, "18: run exit");
    assert!(run_out.is_empty(), "18: run stdout: {run_out:?}");
    assert!(
        lossy(&run_err).contains("Syntax error reading source: "),
        "18: run keeps the load wrapper: {run_err:?}"
    );
    let (build_code, build_err, _, _, _) = build_and_run("18-reader", &p18);
    assert_eq!(build_code, 1, "18: build exit");
    let want18 = format!("{}:1:2: Unmatched delimiter: )\n", p18.display());
    assert_eq!(lossy(&build_err), want18, "18: exact text");
}

/// Lossy text of captured bytes.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
