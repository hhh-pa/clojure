// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #205: the hhh-pa/clojure-algorithms green-run harness.
//!
//! Entry-point choice: a Rust integration test (this file), not a
//! binary subcommand. The runner lives next to the evaluator and
//! stdlib it exercises, runs under the existing `cargo nextest run
//! -p clj-eval` gate, and skips cleanly (with an explanatory
//! message) when the checkout is absent — so CI stays green without
//! it. A test binary's argv belongs to the test harness, hence the
//! checkout dir travels via `CLOJURE_ALGORITHMS_DIR`, never via a
//! compile-time `/tmp` path.
//!
//! Full green evidence comes from a run with the env var set:
//!
//! ```sh
//! CLOJURE_ALGORITHMS_DIR=/tmp/w-algorithms \
//!   cargo nextest run -p clj-eval --test algorithms -- --nocapture
//! ```
//!
//! Design (see `require_bindings.rs` for why): every `require` and
//! every `test-ns` evaluates inside a `clojure.main/with-bindings`
//! style `binding` frame — the verbatim `string.clj` top-level
//! `set!` needs the frame, and a bare require throws the
//! JVM-identical ISE. `*load-path*` binds to the checkout's
//! `src`+`test`+`resources` dirs (`resources/` so `io/resource`
//! finds the bundled word lists, mirroring upstream `:paths`). Each
//! `*-test` namespace loads once and runs once via
//! `clojure.test/test-ns`; the returned summary vector feeds the
//! per-namespace failure list and the total tally. The runner
//! `set_current_dir`s into a fresh temp work dir first, so tests
//! that write relative files (`encrypt.out`, …) never dirty the
//! repo — load-path entries are absolute, so loads are unaffected.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clj_eval::{Evaluator, boot};
use clj_value::Value;

/// Env var carrying the algorithms checkout dir (see module docs).
const ENV_DIR: &str = "CLOJURE_ALGORITHMS_DIR";

/// Optional substring filter over namespace names (debugging aid:
/// run one namespace with e.g.
/// `CLOJURE_ALGORITHMS_FILTER=matrix-chain-multiplication`).
/// Empty/absent runs everything.
const ENV_FILTER: &str = "CLOJURE_ALGORITHMS_FILTER";

/// Optional substring exclusion over namespace names (debugging
/// aid: census everything but a known-slow benchmark with e.g.
/// `CLOJURE_ALGORITHMS_EXCLUDE=matrix-exponentiation`, while the
/// excluded namespace grinds in a separate run).
/// Empty/absent excludes nothing.
const ENV_EXCLUDE: &str = "CLOJURE_ALGORITHMS_EXCLUDE";

/// Worker-thread stack (256 MiB): deep-but-legitimate recursion
/// (e.g. memoized `matrix-chain-order` over 99 items) overflows the
/// default 2 MiB test-thread stack under the tree-walking
/// evaluator. Truly infinite recursion still aborts — that is a
/// real bug, not a sizing issue.
const WORKER_STACK: usize = 256 * 1024 * 1024;

/// Per-namespace counters from one `test-ns` summary vector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    test: i64,
    pass: i64,
    fail: i64,
    error: i64,
}

/// The outcome of one green-run pass.
#[derive(Debug, Default, PartialEq, Eq)]
struct Summary {
    /// Test namespaces discovered (`*_test.clj` under `test/`).
    discovered: usize,
    /// Namespaces that loaded (`require` ok).
    loaded: usize,
    /// `(ns, error)` for namespaces whose `require` threw.
    load_errors: Vec<(String, String)>,
    /// `(ns, error)` for namespaces whose `test-ns` threw.
    run_errors: Vec<(String, String)>,
    /// `(ns, counts)` for loaded namespaces with fail/error > 0.
    failing: Vec<(String, Counts)>,
    /// Totals over all `test-ns` runs.
    total: Counts,
}

impl Summary {
    /// Green iff every discovered namespace loaded and ran with
    /// zero failures and zero errors.
    fn is_green(&self) -> bool {
        self.discovered > 0
            && self.loaded == self.discovered
            && self.load_errors.is_empty()
            && self.run_errors.is_empty()
            && self.failing.is_empty()
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "namespaces: discovered={} loaded={} load-errors={} run-errors={} failing={}",
            self.discovered,
            self.loaded,
            self.load_errors.len(),
            self.run_errors.len(),
            self.failing.len()
        )?;
        writeln!(
            f,
            "assertions: tests={} pass={} fail={} error={}",
            self.total.test, self.total.pass, self.total.fail, self.total.error
        )
    }
}

/// A Rust string as a Clojure string literal.
fn clj_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Wrap `body` in the entry-point `binding` frame (the `main.rs`
/// `cmd_run` set, plus `*load-path*` over `dirs`).
fn in_bindings(dirs: &[String], body: &str) -> String {
    let mut paths = String::new();
    for (i, d) in dirs.iter().enumerate() {
        if i > 0 {
            paths.push(' ');
        }
        paths.push_str(&clj_string(d));
    }
    format!(
        "(binding [*ns* *ns* \
         *warn-on-reflection* *warn-on-reflection* \
         *math-context* *math-context* \
         *print-meta* *print-meta* \
         *print-length* *print-length* \
         *print-level* *print-level* \
         *print-namespace-maps* true \
         *data-readers* *data-readers* \
         *default-data-reader-fn* *default-data-reader-fn* \
         *compile-path* *compile-path* \
         *command-line-args* nil \
         *unchecked-math* *unchecked-math* \
         *assert* *assert* \
         *1 nil *2 nil *3 nil *e nil \
         clojure.core/*load-path* [{paths}]] \
         {body})"
    )
}

/// Map a `test/`-relative `.clj` path to its namespace symbol
/// (`foo/bar_baz_test.clj` → `foo.bar-baz-test`), or `None` when
/// the file is not a `*_test.clj`.
fn test_ns_of(rel: &Path) -> Option<String> {
    let stem = rel.file_stem()?.to_str()?;
    if !stem.ends_with("_test") {
        return None;
    }
    let mut parts: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str())
                .map(|s| s.replace('_', "-"))
                .collect()
        })
        .unwrap_or_default();
    parts.push(stem.replace('_', "-"));
    Some(parts.join("."))
}

/// Discover every `*-test` namespace under `test_dir`, sorted.
fn discover_tests(test_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![test_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|_| panic!("algorithms: cannot list {}", dir.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("algorithms: bad dir entry: {e}"))
                .path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "clj") {
                let rel = path.strip_prefix(test_dir).unwrap_or(&path);
                if let Some(ns) = test_ns_of(rel) {
                    out.push(ns);
                }
            }
        }
    }
    out.sort();
    out
}

/// Read `[test pass fail error]` off a realized `test-ns` vector.
fn counts_of(v: &Value) -> Option<Counts> {
    let Value::Vector(items) = v else {
        return None;
    };
    let nums: Vec<i64> = items
        .iter()
        .map(|x| match x {
            Value::Int(n) => Some(*n),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let [test, pass, fail, error] = nums.as_slice() else {
        return None;
    };
    Some(Counts {
        test: *test,
        pass: *pass,
        fail: *fail,
        error: *error,
    })
}

/// Require one namespace inside the frame; `Ok(())` on success.
fn require_ns(eval: &mut Evaluator, dirs: &[String], ns: &str) -> Result<(), String> {
    let src = in_bindings(dirs, &format!("(require '{ns})"));
    eval.eval_source(&src)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Run one namespace's tests inside the frame; `Ok(counts)` (even
/// when assertions failed — those live in the counts, not in a
/// throw).
fn run_ns(eval: &mut Evaluator, dirs: &[String], ns: &str) -> Result<Counts, String> {
    let src = in_bindings(
        dirs,
        &format!(
            "(let [s (clojure.test/test-ns '{ns})] \
             [(:test s) (:pass s) (:fail s) (:error s)])"
        ),
    );
    let vs = eval.eval_source(&src).map_err(|e| e.to_string())?;
    let last = vs
        .last()
        .ok_or_else(|| "test-ns returned no values".to_owned())?;
    let realized = eval
        .realize_deep(last)
        .map_err(|s| format!("realize failed: {s:?}"))?;
    counts_of(&realized).ok_or_else(|| format!("bad test-ns shape: {realized:?}"))
}

/// Run the whole checkout: require every `*-test` ns, then
/// `test-ns` each loaded one. Progress and failures stream to
/// `log`; the returned [`Summary`] tallies everything. `filter`
/// narrows to namespaces whose name contains it (empty = all).
fn run_algorithms(
    dir: &Path,
    filter: &str,
    exclude: &str,
    log: &mut dyn std::fmt::Write,
) -> Summary {
    let src_dir = dir.join("src");
    let test_dir = dir.join("test");
    let res_dir = dir.join("resources");
    let dirs: Vec<String> = [&src_dir, &test_dir, &res_dir]
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let namespaces: Vec<String> = discover_tests(&test_dir)
        .into_iter()
        .filter(|ns| {
            (filter.is_empty() || ns.contains(filter))
                && (exclude.is_empty() || !ns.contains(exclude))
        })
        .collect();
    let _ = writeln!(
        log,
        "algorithms: {} test namespaces in {}",
        namespaces.len(),
        dir.display()
    );

    let mut eval = boot();
    let mut summary = Summary {
        discovered: namespaces.len(),
        ..Summary::default()
    };
    // Stdlib libs first (inside the frame, like every require): 59 suite files
    // call `clojure.string/`/`clojure.set/`/`clojure.java.io/` fully qualified
    // WITHOUT requiring them (JVM runner has them ambient via load order).
    let setup = in_bindings(
        &dirs,
        "(require 'clojure.test 'clojure.string 'clojure.set 'clojure.edn 'clojure.java.io)",
    );
    if let Err(e) = eval.eval_source(&setup) {
        let _ = writeln!(log, "algorithms: FATAL require clojure.test: {e}");
        for ns in &namespaces {
            summary
                .load_errors
                .push((ns.clone(), format!("setup failed: {e}")));
        }
        return summary;
    }
    let mut loaded = Vec::with_capacity(namespaces.len());
    for (i, ns) in namespaces.iter().enumerate() {
        match require_ns(&mut eval, &dirs, ns) {
            Ok(()) => loaded.push(ns.clone()),
            Err(e) => {
                let _ = writeln!(log, "algorithms: LOAD-ERROR {ns}: {e}");
                summary.load_errors.push((ns.clone(), e));
            }
        }
        if (i + 1) % 200 == 0 {
            let _ = writeln!(
                log,
                "algorithms: …loaded {}/{} ({} errors)",
                i + 1,
                namespaces.len(),
                summary.load_errors.len()
            );
        }
    }
    summary.loaded = loaded.len();
    let _ = writeln!(
        log,
        "algorithms: load phase done: {}/{} loaded",
        summary.loaded, summary.discovered
    );
    for (i, ns) in loaded.iter().enumerate() {
        match run_ns(&mut eval, &dirs, ns) {
            Ok(c) => {
                summary.total.test += c.test;
                summary.total.pass += c.pass;
                summary.total.fail += c.fail;
                summary.total.error += c.error;
                if c.fail > 0 || c.error > 0 {
                    let _ = writeln!(
                        log,
                        "algorithms: FAIL {ns}: tests={} pass={} fail={} error={}",
                        c.test, c.pass, c.fail, c.error
                    );
                    summary.failing.push((ns.clone(), c));
                }
            }
            Err(e) => {
                let _ = writeln!(log, "algorithms: RUN-ERROR {ns}: {e}");
                summary.run_errors.push((ns.clone(), e));
            }
        }
        if (i + 1) % 200 == 0 {
            let _ = writeln!(
                log,
                "algorithms: …ran {}/{} ({} failing)",
                i + 1,
                loaded.len(),
                summary.failing.len() + summary.run_errors.len()
            );
        }
    }
    // JVM parity: joined drainers (see `main.rs` `cmd_eval`).
    eval.join_drainers();
    summary
}

/// The full green run. Needs `CLOJURE_ALGORITHMS_DIR`; without it
/// (or with a dir missing `src/`+`test/`) the test skips with an
/// explanatory message instead of failing.
#[test]
fn algorithms_green() {
    let Some(dir) = std::env::var(ENV_DIR).ok().filter(|s| !s.is_empty()) else {
        eprintln!(
            "algorithms: skipping — set {ENV_DIR} to the hhh-pa/clojure-algorithms checkout; \
             e.g. CLOJURE_ALGORITHMS_DIR=/tmp/w-algorithms cargo nextest run -p clj-eval --test algorithms"
        );
        return;
    };
    let dir = PathBuf::from(&dir);
    if !dir.join("src").is_dir() || !dir.join("test").is_dir() {
        eprintln!(
            "algorithms: skipping — {ENV_DIR}={dir:?} has no src/+test/ dirs (not a checkout?)"
        );
        return;
    }
    // Fresh temp work dir: relative-output tests (`encrypt.out`,
    // …) never dirty the repo. Load-path entries are absolute.
    let work = std::env::temp_dir().join(format!("clj-algo-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("work dir");
    std::env::set_current_dir(&work).expect("set_current_dir to work dir");
    let filter = std::env::var(ENV_FILTER).unwrap_or_default();
    if !filter.is_empty() {
        println!("algorithms: filter={filter:?} (debugging aid — full green needs no filter)");
    }
    let exclude = std::env::var(ENV_EXCLUDE).unwrap_or_default();
    if !exclude.is_empty() {
        println!("algorithms: exclude={exclude:?} (debugging aid — full green needs no exclude)");
    }
    // Big-stack worker (see WORKER_STACK): the evaluator is
    // tree-walking, and deep legitimate recursion overflows the
    // default test-thread stack.
    let summary = std::thread::Builder::new()
        .name("algorithms-green".to_owned())
        .stack_size(WORKER_STACK)
        .spawn(move || {
            let mut log = String::new();
            let summary = run_algorithms(&dir, &filter, &exclude, &mut log);
            print!("{log}");
            summary
        })
        .expect("spawn algorithms worker")
        .join()
        .expect("algorithms worker panicked");
    println!("{summary}");
    for (ns, e) in summary.load_errors.iter().take(20) {
        println!("LOAD-ERROR {ns}: {e}");
    }
    for (ns, e) in summary.run_errors.iter().take(20) {
        println!("RUN-ERROR {ns}: {e}");
    }
    assert!(
        summary.is_green(),
        "algorithms run not green: {summary:?}\n{summary}"
    );
}

/// A scratch checkout exercising every runner path (pass, fail,
/// error, load-error, non-test helper).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-algo-fx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["src/fx", "test/fx", "resources"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn helpers_map_namespaces_escape_strings_and_read_counts() {
    assert_eq!(
        test_ns_of(Path::new("clojure_algorithms/foo_bar_test.clj")),
        Some("clojure-algorithms.foo-bar-test".to_owned())
    );
    assert_eq!(
        test_ns_of(Path::new("a/b/c_test.clj")),
        Some("a.b.c-test".to_owned())
    );
    assert_eq!(test_ns_of(Path::new("fx/test_utils.clj")), None);
    assert_eq!(test_ns_of(Path::new("fx/core.clj")), None);
    assert_eq!(clj_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    assert_eq!(clj_string("tab\there"), "\"tab\\there\"");
    assert_eq!(clj_string("a\x07b"), "\"a\\u0007b\"");
    assert_eq!(counts_of(&Value::Nil), None);
    // A bad shape (wrong arity) is not counts.
    let mut eval = boot();
    let vs = eval.eval_source("[1 2 3]").unwrap();
    assert_eq!(counts_of(&vs[0]), None);
    // A good shape reads back.
    let vs = eval.eval_source("[4 3 1 0]").unwrap();
    assert_eq!(
        counts_of(&vs[0]),
        Some(Counts {
            test: 4,
            pass: 3,
            fail: 1,
            error: 0
        })
    );
}

#[test]
fn runner_reports_pass_fail_error_and_load_error() {
    let fx = Fixture::new("mixed");
    fx.write("src/fx/core.clj", "(ns fx.core) (defn one [] 1)\n");
    fx.write(
        "test/fx/pass_test.clj",
        "(ns fx.pass-test (:require [clojure.test :refer [deftest is]] [fx.core :as c]))\n\
         (deftest ok (is (= 1 (c/one))))\n",
    );
    fx.write(
        "test/fx/fail_test.clj",
        "(ns fx.fail-test (:require [clojure.test :refer [deftest is]]))\n\
         (deftest bad (is (= 1 2)))\n",
    );
    fx.write(
        "test/fx/err_test.clj",
        "(ns fx.err-test (:require [clojure.test :refer [deftest is]]))\n\
         (deftest boom (is (throw (ex-info \"k\" {}))))\n",
    );
    fx.write(
        "test/fx/broken_test.clj",
        "(ns fx.broken-test (:require [fx.nope]))\n",
    );
    fx.write("test/fx/helper.clj", "(ns fx.helper) (def h 1)\n");
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "", "", &mut log);
    assert_eq!(summary.discovered, 4);
    assert_eq!(summary.loaded, 3);
    assert_eq!(summary.load_errors.len(), 1);
    assert_eq!(summary.load_errors[0].0, "fx.broken-test");
    assert!(summary.run_errors.is_empty());
    assert_eq!(summary.failing.len(), 2);
    assert_eq!(summary.total.test, 3);
    assert_eq!(summary.total.pass, 1);
    assert_eq!(summary.total.fail, 1);
    assert_eq!(summary.total.error, 1);
    assert!(!summary.is_green());
    assert!(log.contains("LOAD-ERROR fx.broken-test"));
    assert!(log.contains("FAIL fx.fail-test"));
    assert!(log.contains("FAIL fx.err-test"));
    let text = summary.to_string();
    assert!(text.contains("discovered=4 loaded=3"));
    assert!(text.contains("tests=3 pass=1 fail=1 error=1"));
}

#[test]
fn runner_green_fixture_is_green() {
    let fx = Fixture::new("green");
    fx.write(
        "test/fx/only_test.clj",
        "(ns fx.only-test (:require [clojure.test :refer [deftest is]]))\n\
         (deftest ok (is (= 1 1)))\n",
    );
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "", "", &mut log);
    assert_eq!(summary.discovered, 1);
    assert!(summary.is_green());
    assert!(log.contains("1 test namespaces"));
    // The filter narrows discovery (and an unmatched filter finds none).
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "only", "", &mut log);
    assert_eq!(summary.discovered, 1);
    assert!(summary.is_green());
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "zzz-no-match", "", &mut log);
    assert_eq!(summary.discovered, 0);
    assert!(!summary.is_green());
}

#[test]
fn runner_exclude_drops_namespaces() {
    let fx = Fixture::new("exclude");
    for name in ["slow", "fast"] {
        fx.write(
            &format!("test/fx/{name}_test.clj"),
            &format!(
                "(ns fx.{name}-test (:require [clojure.test :refer [deftest is]]))\n\
                 (deftest ok (is (= 1 1)))\n"
            ),
        );
    }
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "", "", &mut log);
    assert_eq!(summary.discovered, 2);
    assert!(summary.is_green());
    // The exclusion drops one namespace; filter and exclusion compose.
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "", "slow", &mut log);
    assert_eq!(summary.discovered, 1);
    assert!(summary.is_green());
    assert!(log.contains("1 test namespaces"));
    let mut log = String::new();
    let summary = run_algorithms(&fx.dir, "fx.", "fast", &mut log);
    assert_eq!(summary.discovered, 1);
    assert!(summary.is_green());
}

#[test]
fn empty_summary_is_not_green() {
    assert!(!Summary::default().is_green());
}
