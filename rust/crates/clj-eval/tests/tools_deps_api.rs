// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-124 slice-3 tests: the tools.deps API namespace —
//! `clojure.tools.deps` (expansion, basis, classpath, print-tree),
//! `util.concurrent`, and the finished `util.session`.
//!
//! Oracle (text only, never executed): `org.clojure/tools.deps`
//! 0.31.1646 (`deps.clj`, `util/session.clj`,
//! `util/concurrent.clj`); `clojure.set` / `clojure.string`
//! (`blank?`/`join`) for the local helpers. Each test cites the
//! upstream behavior it pins. Divergences are the spec's C1–C14,
//! R4, X1 (`docs/gap-124/slice-3.md`).
//!
//! Each test boots its own `Evaluator`, so the global session var
//! cannot leak across tests; every filesystem case uses a unique
//! `scratch()` dir. Multi-element sets assert by in-eval `=`
//! (order-free), never by pr-string order.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
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

/// A Rust string as a Clojure string literal.
fn clj_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Require the API stack (the API ns pulls the local/git/deps
/// extensions itself — C14).
fn require_api(eval: &mut Evaluator) {
    eval.eval_source(
        "(require '[clojure.tools.deps.util.port :as port]
                  '[clojure.tools.deps.util.dir :as dir]
                  '[clojure.tools.deps.edn :as edn]
                  '[clojure.tools.deps.extensions :as ext]
                  '[clojure.tools.deps.util.session :as session]
                  '[clojure.tools.deps.util.concurrent :as conc]
                  '[clojure.tools.deps :as td])",
    )
    .unwrap();
}

/// A fresh evaluator with the API namespaces required.
fn api_eval() -> Evaluator {
    let mut eval = boot();
    require_api(&mut eval);
    eval
}

/// A scratch dir unique to this test (parallel-safe), wiped first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clj-td124s3-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `name` under `dir`, returning the absolute path string.
fn write_dep(dir: &std::path::Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

/// Write a lib dir `root/name` with `deps.edn` content plus a `src/`
/// dir, returning the lib root path string.
fn write_lib(root: &std::path::Path, name: &str, content: &str) -> String {
    let lib = root.join(name);
    std::fs::create_dir_all(lib.join("src")).unwrap();
    write_dep(&lib, "deps.edn", content);
    lib.to_string_lossy().into_owned()
}

// ---- load (ns.rs resolve_source) ----

#[test]
fn api_nss_load_via_require() {
    // The loader serves the two new embedded libs; requiring the
    // API ns auto-loads the local/git/deps extensions (C14 —
    // upstream `load`, `maven`/`pom` never load per R1/R2).
    let mut eval = boot();
    eval.eval_source("(require 'clojure.tools.deps)").unwrap();
    for ns in [
        "clojure.tools.deps.util.port",
        "clojure.tools.deps.util.dir",
        "clojure.tools.deps.edn",
        "clojure.tools.deps.extensions",
        "clojure.tools.deps.util.io",
        "clojure.tools.deps.util.session",
        "clojure.tools.deps.extensions.local",
        "clojure.tools.deps.extensions.git",
        "clojure.tools.deps.extensions.deps",
        "clojure.tools.deps.util.concurrent",
        "clojure.tools.deps",
    ] {
        assert_eq!(
            run(&mut eval, &format!("(some? (find-ns '{ns}))")),
            "true",
            "{ns} loads"
        );
        assert_eq!(
            run(&mut eval, &format!("(contains? (loaded-libs) '{ns})")),
            "true",
            "{ns} marks loaded"
        );
    }
}

// ---- port substrate: available-processors, path-separator, thread-id ----

#[test]
fn available_processors_rows() {
    // Upstream `Runtime/availableProcessors` via the slice-3
    // native: a positive long (the default `:threads`).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(and (int? (port/available-processors)) (pos? (port/available-processors)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (port/available-processors) (port/available-processors))"
        ),
        "true"
    );
    assert!(uncaught(&mut eval, "(port/available-processors 1)").contains("Wrong number of args"));
}

#[test]
fn path_separator_rows() {
    // Upstream `File/pathSeparator` via the slice-3 native:
    // platform-exact (C3), one char, `:` or `;`.
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(contains? #{\":\" \";\"} (port/path-separator))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= 1 (count (port/path-separator)))"),
        "true"
    );
    assert!(uncaught(&mut eval, "(port/path-separator :x)").contains("Wrong number of args"));
}

#[test]
fn thread_id_rows() {
    // Upstream `Thread/currentThread .getId` via the slice-3
    // native: stable on one thread, distinct across `future`
    // (C10 — the `retrieve-local` scope key).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(and (int? (port/thread-id)) (= (port/thread-id) (port/thread-id)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(not= (port/thread-id) (deref (future (port/thread-id))))"
        ),
        "true"
    );
    assert!(uncaught(&mut eval, "(port/thread-id nil)").contains("Wrong number of args"));
}

// ---- util.concurrent ----

#[test]
fn concurrent_processors_pin() {
    // Upstream concurrent.clj `processors`: the native count.
    let mut eval = api_eval();
    assert_eq!(
        run(&mut eval, "(= conc/processors (port/available-processors))"),
        "true"
    );
}

#[test]
fn concurrent_submit_and_shutdown() {
    // Upstream `new-executor` (opaque `{:threads n}` token, C4),
    // `submit-task` (value + `with-dir` binding conveyance, C4),
    // `shutdown-on-error` (nil no-op, C4).
    let mut eval = api_eval();
    assert_eq!(
        run(&mut eval, "(= (conc/new-executor 3) {:threads 3})"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= 42 (deref (conc/submit-task (conc/new-executor 2) (fn [] 42))))"
        ),
        "true"
    );
    let dir = scratch("conc-bind");
    let lit = clj_str(&dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (dir/with-dir {lit} (deref (conc/submit-task (conc/new-executor 2) (fn [] dir/*the-dir*)))) (dir/canonicalize {lit}))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(nil? (conc/shutdown-on-error (conc/new-executor 1)))"
        ),
        "true"
    );
}

// ---- util.session: with-session + retrieve-local ----

#[test]
fn with_session_scoping() {
    // Upstream session.clj `with-session`: a fresh session
    // inside, the prior atom restored after — including on
    // throw (C9 `finally`).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(do (session/retrieve :k (fn [] :outer))
                 [(session/with-session (session/retrieve :k))
                  (session/with-session (session/retrieve :k (fn [] :inner)))
                  (session/retrieve :k)])"
        ),
        "[nil :inner :outer]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do [(try (session/with-session
                         (session/retrieve :k2 (fn [] 1))
                         (throw (ex-info \"x\" {})))
                       (catch clojure.lang.ExceptionInfo e :thrown))
                 (session/retrieve :k2)])"
        ),
        "[:thrown nil]"
    );
}

#[test]
fn retrieve_local_rows() {
    // Upstream session.clj `retrieve-local`: compute-once per
    // thread key, nil on a bare miss, never shared across
    // threads (C10).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(let [n (atom 0)] [(session/retrieve-local :c (fn [] (swap! n inc)))
                                (session/retrieve-local :c (fn [] (swap! n inc)))
                                @n])"
        ),
        "[1 1 1]"
    );
    assert_eq!(
        run(&mut eval, "(nil? (session/retrieve-local :missing))"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (session/retrieve-local :k (fn [] :main))
                 [(session/retrieve-local :k)
                  (deref (future (session/retrieve-local :k (fn [] :worker))))
                  (session/retrieve-local :k)])"
        ),
        "[:main :worker :main]"
    );
}

// ---- local helpers (C1/C2/C5-C8), via var deref ----

#[test]
fn set_helpers_nil_arms() {
    // Verbatim set.clj 2-arity bodies (C2): the nil arms drive
    // same-version narrowing (`intersection(nil,X)` → nil,
    // `difference(X,nil)` → X).
    let mut eval = api_eval();
    for (src, want) in [
        ("(@#'td/set-intersection #{:a :b} #{:b :c})", "#{:b}"),
        ("(@#'td/set-intersection nil #{:a})", "nil"),
        ("(@#'td/set-intersection #{:a} nil)", "nil"),
        ("(@#'td/set-intersection #{} nil)", "#{}"),
        ("(@#'td/set-difference #{:a :b} #{:b :c})", "#{:a}"),
        ("(@#'td/set-difference #{:a} nil)", "#{:a}"),
        ("(@#'td/set-difference nil #{:a})", "nil"),
        ("(@#'td/set-difference nil nil)", "nil"),
    ] {
        assert_eq!(
            run(&mut eval, &format!("(= {src} {want})")),
            "true",
            "{src}"
        );
    }
}

#[test]
fn str_helpers_rows() {
    // Local `str-blank?` (C5: nil/empty/whitespace → true),
    // `str-join` (C6), `split-dollar` (C7: Java-split `$`
    // semantics incl. the trailing-empty drop and `[\"\"]` for
    // the empty input), `qualified-ident?` (C8).
    let mut eval = api_eval();
    for (src, want) in [
        ("(@#'td/str-blank? nil)", "true"),
        ("(@#'td/str-blank? \"\")", "true"),
        ("(@#'td/str-blank? \" \\t\\r\\n\")", "true"),
        ("(@#'td/str-blank? \"x\")", "false"),
        ("(@#'td/str-blank? \" x \")", "false"),
        ("(@#'td/str-join \":\" [])", "\"\""),
        ("(@#'td/str-join \":\" [\"a\"])", "\"a\""),
        ("(@#'td/str-join \":\" [\"a\" \"b\"])", "\"a:b\""),
        ("(@#'td/split-dollar \"a$b\")", "[\"a\" \"b\"]"),
        ("(@#'td/split-dollar \"a$\")", "[\"a\"]"),
        ("(@#'td/split-dollar \"$\")", "[]"),
        ("(@#'td/split-dollar \"\")", "[\"\"]"),
        ("(@#'td/split-dollar \"abc\")", "[\"abc\"]"),
        ("(@#'td/split-dollar \"$a\")", "[\"\" \"a\"]"),
        ("(@#'td/qualified-ident? 'a/b)", "true"),
        ("(@#'td/qualified-ident? 'a)", "false"),
        ("(@#'td/qualified-ident? :k/j)", "true"),
        ("(@#'td/qualified-ident? \"s\")", "false"),
        ("(@#'td/qualified-ident? nil)", "false"),
    ] {
        assert_eq!(
            run(&mut eval, &format!("(= {src} {want})")),
            "true",
            "{src}"
        );
    }
}

#[test]
fn queue_helpers_rows() {
    // Queue-vectors (C1): `q-peek` is nil-safe, `q-pop` on
    // empty stays empty (queue-pop parity), FIFO order holds.
    let mut eval = api_eval();
    assert_eq!(run(&mut eval, "(nil? (@#'td/q-peek []))"), "true");
    assert_eq!(run(&mut eval, "(= (@#'td/q-pop []) [])"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(let [q (-> [] (conj :a) (conj :b))] [(@#'td/q-peek q) (@#'td/q-pop q) (@#'td/q-peek (@#'td/q-pop q))])"
        ),
        "[:a [:b] :b]"
    );
}

// ---- expansion: diamond, errors, exclusions, overrides ----

/// The A→{B,C}→D diamond roots, keyed by lib dir name.
fn diamond(root: &std::path::Path) -> (String, String, String, String) {
    let d = write_lib(root, "D", "{:paths [\"src\"]}");
    let b = write_lib(
        root,
        "B",
        &format!(
            "{{:paths [\"src\"] :deps {{d/d {{:local/root {}}}}}}}",
            clj_str(&d)
        ),
    );
    let c = write_lib(
        root,
        "C",
        &format!(
            "{{:paths [\"src\"] :deps {{d/d {{:local/root {}}}}}}}",
            clj_str(&d)
        ),
    );
    let a = write_lib(
        root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{b/b {{:local/root {}}} c/c {{:local/root {}}}}}}}",
            clj_str(&b),
            clj_str(&c)
        ),
    );
    (a, b, c, d)
}

#[test]
fn bdd_diamond_resolves_once() {
    // Scenario: a local diamond resolves D once (deps.clj
    // `expand-deps`/`resolve-deps`): canonical roots, manifest
    // keys, dependents, parents, and paths.
    let mut eval = api_eval();
    let root = scratch("bdd-diamond");
    let (a, _, _, d) = diamond(&root);
    let (a, d) = (clj_str(&a), clj_str(&d));
    let src = format!(
        "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} nil))
         [(sort (keys m))
          (= (:parents (get m 'd/d)) '#{{[a/a b/b] [a/a c/c]}})
          (= (set (:dependents (get m 'd/d))) '#{{b/b c/c}})
          (= (:local/root (get m 'd/d)) (port/canonical-path {d}))
          (= (:deps/manifest (get m 'd/d)) :deps)
          (= (:deps/root (get m 'd/d)) (:local/root (get m 'd/d)))
          (= (:paths (get m 'd/d)) [(port/canonical-path (str {d} \"/src\"))])
          (= (:paths (get m 'a/a)) [(port/canonical-path (str {a} \"/src\"))])]"
    );
    assert_eq!(
        run(&mut eval, &src),
        "[(a/a b/b c/c d/d) true true true true true true true]"
    );
}

#[test]
fn bdd_trace_records_expansion() {
    // Scenario: `:trace true` records the lib/include/reason log
    // (deps.clj `trace+`), BFS order, same-version tail.
    let mut eval = api_eval();
    let root = scratch("bdd-trace");
    let (a, _, _, _) = diamond(&root);
    let a = clj_str(&a);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} {{:trace true}}))
                 (= (mapv (juxt :lib :reason) (:log (:trace (meta m))))
                    '[[a/a :new-top-dep] [b/b :new-dep] [c/c :new-dep] [d/d :new-dep] [d/d :same-version]])"
            )
        ),
        "true"
    );
}

#[test]
fn bdd_two_roots_throw() {
    // Scenario: the same lib at two local roots is an error
    // (deps.clj `dominates?` → local `compare-versions` throw —
    // `:local` has no ancestor relation).
    let mut eval = api_eval();
    let root = scratch("bdd-two-roots");
    let d1 = write_lib(&root, "D1", "{:paths [\"src\"]}");
    let d2 = write_lib(&root, "D2", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{d/d {{:local/root {}}}}}}}",
            clj_str(&d1)
        ),
    );
    let b = write_lib(
        &root,
        "B",
        &format!(
            "{{:paths [\"src\"] :deps {{d/d {{:local/root {}}}}}}}",
            clj_str(&d2)
        ),
    );
    let (a, b, d1, d2) = (clj_str(&a), clj_str(&b), clj_str(&d1), clj_str(&d2));
    let err = uncaught(
        &mut eval,
        &format!(
            "(td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'b/b {{:local/root {b}}}}}}} nil)"
        ),
    );
    assert!(err.starts_with("clojure.lang.ExceptionInfo | "), "{err}");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(try (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'b/b {{:local/root {b}}}}}}} nil)
                      \"no-throw\"
                      (catch clojure.lang.ExceptionInfo e
                        (= (ex-message e)
                           (str \"No known ancestor relationship between local versions for d/d: \"
                                (port/canonical-path {d2}) \" and \" (port/canonical-path {d1})))))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(try (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'b/b {{:local/root {b}}}}}}} nil)
                      \"no-throw\"
                      (catch clojure.lang.ExceptionInfo e
                        (let [d (ex-data e)]
                          [(= (:lib d) 'd/d)
                           (= (:local/root (:x d)) (port/canonical-path {d2}))
                           (= (:local/root (:y d)) (port/canonical-path {d1}))])))"
            )
        ),
        "[true true true]"
    );
}

#[test]
fn top_dep_wins_without_compare() {
    // A top dep at one root beats a transitive dep at another
    // WITHOUT throwing: `:use-top` fires before `dominates?`
    // ever compares (deps.clj `include-coord?` order).
    let mut eval = api_eval();
    let root = scratch("top-wins");
    let d1 = write_lib(&root, "D1", "{:paths [\"src\"]}");
    let d2 = write_lib(&root, "D2", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{d/d {{:local/root {}}}}}}}",
            clj_str(&d2)
        ),
    );
    let (a, d1) = (clj_str(&a), clj_str(&d1));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'd/d {{:local/root {d1}}}}}}} {{:trace true}}))
                 [(= (:local/root (get m 'd/d)) (port/canonical-path {d1}))
                  (= (mapv :reason (:log (:trace (meta m)))) [:new-top-dep :new-top-dep :use-top])]"
            )
        ),
        "[true true]"
    );
}

#[test]
fn bdd_exclusion_narrowing() {
    // Scenario: per-path exclusions narrow on re-encounter
    // (deps.clj `update-excl` same-version arm — the C2 nil
    // arms): x cut under A is included via B's plain path.
    let mut eval = api_eval();
    let root = scratch("bdd-excl");
    let x = write_lib(&root, "X", "{:paths [\"src\"]}");
    let l = write_lib(
        &root,
        "L",
        &format!(
            "{{:paths [\"src\"] :deps {{x/x {{:local/root {}}}}}}}",
            clj_str(&x)
        ),
    );
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{l/l {{:local/root {} :exclusions [x/x]}}}}}}",
            clj_str(&l)
        ),
    );
    let b = write_lib(
        &root,
        "B",
        &format!(
            "{{:paths [\"src\"] :deps {{l/l {{:local/root {}}}}}}}",
            clj_str(&l)
        ),
    );
    let (a, b) = (clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'b/b {{:local/root {b}}}}}}} {{:trace true}}))
                 [(sort (keys m))
                  (= (:parents (get m 'x/x)) '#{{[b/b l/l]}})
                  (= (mapv (juxt :lib :reason) (:log (:trace (meta m))))
                     '[[a/a :new-top-dep] [b/b :new-top-dep] [l/l :new-dep] [l/l :same-version] [x/x :new-dep]])]"
            )
        ),
        "[(a/a b/b l/l x/x) true true]"
    );
}

#[test]
fn excluded_subtree_omits() {
    // A lib under an excluded path is omitted with reason
    // `:excluded` (deps.clj `excluded?` subtree walk): A
    // excludes x, L sits between, x never resolves.
    let mut eval = api_eval();
    let root = scratch("excl-omit");
    let x = write_lib(&root, "X", "{:paths [\"src\"]}");
    let l = write_lib(
        &root,
        "L",
        &format!(
            "{{:paths [\"src\"] :deps {{x/x {{:local/root {}}}}}}}",
            clj_str(&x)
        ),
    );
    let a = clj_str(&write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{l/l {{:local/root {}}}}}}}",
            clj_str(&l)
        ),
    ));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a} :exclusions ['x/x]}}}}}} {{:trace true}}))
                 [(sort (keys m))
                  (= (mapv (juxt :lib :reason) (:log (:trace (meta m))))
                     '[[a/a :new-top-dep] [l/l :new-dep] [x/x :excluded]])]"
            )
        ),
        "[(a/a l/l) true]"
    );
}

#[test]
fn bdd_overrides_and_defaults() {
    // Scenario: `override-deps` swaps the coord (as-written —
    // overrides bypass `canonicalize-deps`, as upstream) and
    // `default-deps` fills a nil coord; the trace records the
    // override coord.
    let mut eval = api_eval();
    let root = scratch("bdd-over");
    let b1 = write_lib(&root, "B1", "{:paths [\"src\"]}");
    let b2 = write_lib(&root, "B2", "{:paths [\"src\"]}");
    let d = write_lib(&root, "D", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{b/b {{:local/root {}}}}}}}",
            clj_str(&b1)
        ),
    );
    let (a, b2, d) = (clj_str(&a), clj_str(&b2), clj_str(&d));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'd/d nil}}}}
                                       {{:override-deps {{'b/b {{:local/root {b2}}}}}
                                         :default-deps {{'d/d {{:local/root {d}}}}}
                                         :trace true}}))
                 [(= (:local/root (get m 'b/b)) {b2})
                  (= (:local/root (get m 'd/d)) {d})
                  (= (:override-coord (first (filter #(= 'b/b (:lib %)) (:log (:trace (meta m))))))
                     {{:local/root {b2}}})]"
            )
        ),
        "[true true true]"
    );
}

#[test]
fn resolve_deps_deprecated_arity() {
    // The 3-arity merges settings into the args map (deps.clj
    // `resolve-deps` backwards-compatibility arity).
    let mut eval = api_eval();
    let root = scratch("resolve-3arity");
    let (a, _, _, _) = diamond(&root);
    let a = clj_str(&a);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} {{:threads 2}} {{:trace true}})
                    (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} {{:threads 2 :trace true}}))"
            )
        ),
        "true"
    );
}

#[test]
fn threads_and_extra_deps_args() {
    // `:threads` changes nothing observable (C4 token) and
    // `:extra-deps` joins the top deps (deps.clj `resolve-deps`
    // arg handling).
    let mut eval = api_eval();
    let root = scratch("threads-extra");
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(&root, "A", "{:paths [\"src\"]}");
    let (a, b) = (clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} {{:threads 2}})
                    (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} nil))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(sort (keys (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}}
                                              {{:extra-deps {{'b/b {{:local/root {b}}}}}}})))"
            )
        ),
        "(a/a b/b)"
    );
}

#[test]
fn nil_coord_edges() {
    // Nil coord + no default throws "Bad coordinate" (upstream
    // `throw-bad-coord` non-map arm — `coord-type` of nil is
    // nil); a missing local root throws "not found"
    // (`ensure-file`).
    let mut eval = api_eval();
    let err = uncaught(&mut eval, "(td/resolve-deps {:deps {'d/d nil}} nil)");
    assert!(
        err.contains("Bad coordinate for library d/d, expected map: nil"),
        "{err}"
    );
    let root = scratch("nil-coord");
    let missing = clj_str(&root.join("nope").to_string_lossy());
    let err = uncaught(
        &mut eval,
        &format!("(td/resolve-deps {{:deps {{'a/a {{:local/root {missing}}}}}}} nil)"),
    );
    assert!(err.contains("Local lib a/a not found:"), "{err}");
}

#[test]
fn empty_resolve_and_trace_npe() {
    // Empty deps resolve to `{}`; tracing an empty graph hits
    // the shared upstream/port `with-meta`-on-nil NPE (pinned
    // parity, not fixed).
    let mut eval = api_eval();
    assert_eq!(run(&mut eval, "(= (td/resolve-deps {} nil) {})"), "true");
    assert_eq!(
        run(&mut eval, "(= (td/resolve-deps {:deps {}} nil) {})"),
        "true"
    );
    let err = uncaught(&mut eval, "(td/resolve-deps {} {:trace true})");
    assert!(err.contains("java.lang.NullPointerException"), "{err}");
    assert!(err.contains("withMeta"), "{err}");
}

// ---- refusals and deferrals through the API ----

#[test]
fn bdd_mvn_and_jar_fail_through_api() {
    // Scenario: `:mvn` throws "not loaded" from `manifest-type`
    // during expansion (R1 holds — `lib-location` is never
    // called on the resolve path); jar roots refuse via R2.
    let mut eval = api_eval();
    let err = uncaught(
        &mut eval,
        "(td/resolve-deps {:deps {'m/m {:mvn/version \"1.0\"}}} nil)",
    );
    assert!(
        err.contains("Coordinate type :mvn not loaded for library m/m"),
        "{err}"
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (td/resolve-deps {:deps {'m/m {:mvn/version \"1.0\"}}} nil)
                  \"no-throw\"
                  (catch clojure.lang.ExceptionInfo e (ex-data e)))"
        ),
        // Namespace-map printing, as in the slice-1 pins.
        "{:lib m/m, :coord #:mvn{:version \"1.0\"}}"
    );
    let root = scratch("bdd-jar");
    let jar = write_dep(&root, "lib.jar", "fake-jar");
    let jar = clj_str(&jar);
    let err = uncaught(
        &mut eval,
        &format!("(td/resolve-deps {{:deps {{'j/j {{:local/root {jar}}}}}}} nil)"),
    );
    assert!(err.contains("cannot use jar manifest for j/j"), "{err}");
}

#[test]
fn git_n1_through_api() {
    // Full-sha git coords canonicalize (pure arm), then
    // `manifest-type` defers to the git-network slice (N1 holds
    // through `resolve-deps`).
    let mut eval = api_eval();
    let sha = "0123456789012345678901234567890123456789";
    let err = uncaught(
        &mut eval,
        &format!(
            "(td/resolve-deps {{:deps {{'g/g {{:git/url \"https://example.com/r.git\" :git/sha \"{sha}\"}}}}}} nil)"
        ),
    );
    assert!(err.contains("deferred to the git-network slice"), "{err}");
}

// ---- classpath emission ----

#[test]
fn bdd_calc_basis() {
    // Scenario: `calc-basis` assembles libs + classpath +
    // classpath-roots (deps.clj `calc-basis`): project paths
    // first and as-written, then canonical lib paths.
    let mut eval = api_eval();
    let root = scratch("bdd-basis");
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{b/b {{:local/root {}}}}}}}",
            clj_str(&b)
        ),
    );
    let (proj, a, b) = (clj_str(&proj.to_string_lossy()), clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {proj}
                   (def basis (td/calc-basis {{:paths [\"src\"] :deps {{'a/a {{:local/root {a}}}}}}})))
                 [(sort (keys (:libs basis)))
                  (= (:classpath-roots basis)
                     [\"src\" (port/canonical-path (str {a} \"/src\")) (port/canonical-path (str {b} \"/src\"))])
                  (= (:classpath basis)
                     {{\"src\" {{:path-key :paths}}
                       (port/canonical-path (str {a} \"/src\")) {{:lib-name 'a/a}}
                       (port/canonical-path (str {b} \"/src\")) {{:lib-name 'b/b}}}})]"
            )
        ),
        "[(a/a b/b) true true]"
    );
}

#[test]
fn calc_basis_argmaps() {
    // The 2-arity threads `:resolve-args` and `:classpath-args`
    // (deps.clj `calc-basis` argmaps); the 1-arity equals the
    // 2-arity with nil.
    let mut eval = api_eval();
    let root = scratch("calc-argmaps");
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join("test")).unwrap();
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(&root, "A", "{:paths [\"src\"]}");
    let (proj, a, b) = (clj_str(&proj.to_string_lossy()), clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (dir/with-dir {proj} (td/calc-basis {{:paths [\"src\"] :deps {{'a/a {{:local/root {a}}}}}}}))
                    (dir/with-dir {proj} (td/calc-basis {{:paths [\"src\"] :deps {{'a/a {{:local/root {a}}}}}}} nil)))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {proj}
                   (def basis (td/calc-basis {{:paths [\"src\"] :deps {{'a/a {{:local/root {a}}}}}}}
                                                {{:resolve-args {{:extra-deps {{'b/b {{:local/root {b}}}}}}}
                                                 :classpath-args {{:extra-paths [\"test\"]}}}})))
                 [(sort (keys (:libs basis)))
                  (= (:classpath-roots basis)
                     [\"test\" \"src\" (port/canonical-path (str {a} \"/src\")) (port/canonical-path (str {b} \"/src\"))])]"
            )
        ),
        "[(a/a b/b) true]"
    );
}

#[test]
fn bdd_classpath_overrides() {
    // Scenario: overrides replace a lib's paths, blank ("")
    // removes the lib (`str-blank?`, C5), unknown libs are
    // ignored (deps.clj `flatten-libs`).
    let mut eval = api_eval();
    let root = scratch("bdd-overrides");
    let (a, _, c, d) = diamond(&root);
    let (a, c, d) = (clj_str(&a), clj_str(&c), clj_str(&d));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} nil))
                 [(= (:classpath-roots (td/make-classpath-map {{:paths [\"src\"]}} m {{:classpath-overrides {{'b/b \"OV\"}}}}))
                     [\"src\" (port/canonical-path (str {a} \"/src\")) \"OV\"
                      (port/canonical-path (str {c} \"/src\")) (port/canonical-path (str {d} \"/src\"))])
                  (= (:classpath-roots (td/make-classpath-map {{:paths [\"src\"]}} m {{:classpath-overrides {{'b/b \"\"}}}}))
                     [\"src\" (port/canonical-path (str {a} \"/src\"))
                      (port/canonical-path (str {c} \"/src\")) (port/canonical-path (str {d} \"/src\"))])
                  (= (td/make-classpath-map {{:paths [\"src\"]}} m {{:classpath-overrides {{'zzz/q \"OV\"}}}})
                     (td/make-classpath-map {{:paths [\"src\"]}} m nil))]"
            )
        ),
        "[true true true]"
    );
}

#[test]
fn bdd_alias_chase() {
    // Scenario: `:extra-paths` entries resolve alias keys
    // recursively (deps.clj `chase-key`), ahead of `:paths`.
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (:classpath-roots (td/make-classpath-map {:paths [\"p\"] :aliases {:more [\"m1\" \"m2\"]}} {} {:extra-paths [:more]}))
                [\"m1\" \"m2\" \"p\"])"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (get (:classpath (td/make-classpath-map {:paths [\"p\"] :aliases {:more [\"m1\"]}} {} {:extra-paths [:more]})) \"m1\")
                {:path-key :more})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:classpath-roots (td/make-classpath-map {:paths [] :aliases {:l1 [:l2] :l2 [\"deep\"]}} {} {:extra-paths [:l1]}))
                [\"deep\"])"
        ),
        "true"
    );
}

#[test]
fn lib_order_is_canonical() {
    // Same-level libs sort alphabetically regardless of file
    // order (deps.clj `sort-paths` over a stable sort).
    let mut eval = api_eval();
    let root = scratch("lib-order");
    let a = write_lib(&root, "A", "{:paths [\"src\"]}");
    let z = write_lib(&root, "Z", "{:paths [\"src\"]}");
    let (a, z) = (clj_str(&a), clj_str(&z));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'z/z {{:local/root {z}}} 'a/a {{:local/root {a}}}}}}} nil))
                 (= (:classpath-roots (td/make-classpath-map {{:paths []}} m nil))
                    [(port/canonical-path (str {a} \"/src\")) (port/canonical-path (str {z} \"/src\"))])"
            )
        ),
        "true"
    );
}

#[test]
fn external_paths_warn_but_resolve() {
    // `:paths` outside the project warn on *err* but stay on
    // the classpath (deps.clj `validate-paths`).
    let mut eval = api_eval();
    let root = scratch("ext-warn");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let lit = clj_str(&proj.to_string_lossy());
    // The capture holds the raw line (as in the slice-2 warn
    // pin — the port's `pr` does not re-quote under the
    // `*print-readably*` nil binding).
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (dir/with-dir {lit}
                      (with-out-str (binding [*err* *out*]
                                     (td/make-classpath-map {{:paths [\"../outside\"]}} {{}} nil))))
                    (str \"WARNING: Use of :paths external to the project has been deprecated, please remove: ../outside\" \"\\n\"))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {lit} (:classpath-roots (td/make-classpath-map {{:paths [\"../outside\"]}} {{}} nil)))"
            )
        ),
        "[\"../outside\"]"
    );
}

#[test]
fn join_classpath_rows() {
    // Upstream `join-classpath` over the platform separator
    // (C3) and the local join (C6).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (td/join-classpath [\"a\" \"b\"]) (str \"a\" (port/path-separator) \"b\"))"
        ),
        "true"
    );
    assert_eq!(run(&mut eval, "(td/join-classpath [\"a\"])"), "\"a\"");
    assert_eq!(run(&mut eval, "(td/join-classpath [])"), "\"\"");
}

#[test]
fn make_classpath_deprecated() {
    // The deprecated 3-arity joins `make-classpath-map` roots
    // (deps.clj `make-classpath`).
    let mut eval = api_eval();
    let root = scratch("make-cp");
    let (a, _, _, _) = diamond(&root);
    let a = clj_str(&a);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}}}}}} nil))
                 (= (td/make-classpath m [\"p\"] nil)
                    (td/join-classpath (:classpath-roots (td/make-classpath-map {{:paths [\"p\"]}} m nil))))"
            )
        ),
        "true"
    );
}

#[test]
fn tool_rows() {
    // Upstream `tool`: tool `:deps`/`:paths` replace the
    // project's (deps.clj `tool`).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (td/tool {:deps {'a/a 1} :paths [\"s\"]} {:replace-deps {'b/b 2} :replace-paths [\"p\"]})
                {:deps {'b/b 2} :paths [\"p\"]})"
        ),
        "true"
    );
    // Plain `:deps`/`:paths` also replace (the docstring:
    // "Replaces project-edn :deps and :paths with tool :deps
    // and :paths").
    assert_eq!(
        run(
            &mut eval,
            "(= (td/tool {:deps {'a/a 1}} {:deps {'b/b 2}}) {:deps {'b/b 2}})"
        ),
        "true"
    );
    assert_eq!(run(&mut eval, "(nil? (td/tool nil {}))"), "true");
}

// ---- basis ----

#[test]
fn bdd_create_basis() {
    // Scenario: `create-basis` runs end to end (deps.clj
    // `create-basis`): sources merge, the alias argmap applies,
    // `:libs`, `:classpath-roots`, `:argmap`, and
    // `:basis-config` agree.
    let mut eval = api_eval();
    let root = scratch("bdd-create");
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(&root, "A", "{:paths [\"src\"]}");
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join("test")).unwrap();
    write_dep(
        &proj,
        "deps.edn",
        &format!(
            "{{:paths [\"src\"] :deps {{a/a {{:local/root {}}}}} :aliases {{:extra {{:extra-deps {{b/b {{:local/root {}}}}} :extra-paths [\"test\"]}}}}}}",
            clj_str(&a),
            clj_str(&b)
        ),
    );
    let (proj, a, b) = (clj_str(&proj.to_string_lossy()), clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def basis (td/create-basis {{:dir {proj} :user nil :aliases [:extra]}}))
                 [(sort (keys (:libs basis)))
                  (= (:classpath-roots basis)
                     [\"test\" \"src\" (port/canonical-path (str {a} \"/src\")) (port/canonical-path (str {b} \"/src\"))])
                  (= (:argmap basis) {{:extra-deps {{'b/b {{:local/root {b}}}}} :extra-paths [\"test\"]}})
                  (= (:basis-config basis) {{:user nil :aliases [:extra]}})]"
            )
        ),
        "[(a/a b/b) true true true]"
    );
}

#[test]
fn create_basis_skip_cp() {
    // The UNSUPPORTED `:skip-cp` flag returns the merged edn
    // with an argmap and no libs (deps.clj `create-basis`
    // short-circuit).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(def b (td/create-basis {:user nil :project {:paths [\"src\"]} :args {:skip-cp true}}))
             [(nil? (:libs b)) (= (:paths b) [\"src\"]) (= (:argmap b) {:skip-cp true})
              (= (:basis-config b) {:project {:paths [\"src\"]} :user nil :args {:skip-cp true}})]"
        ),
        "[true true true true]"
    );
}

#[test]
fn create_basis_replace_path() {
    // Alias `:replace-deps`/`:replace-paths` flow through
    // `tool` into the merged edn (deps.clj `create-basis` +
    // `tool`).
    let mut eval = api_eval();
    let root = scratch("create-replace");
    let c = write_lib(&root, "C", "{:paths [\"src\"]}");
    let a = write_lib(&root, "A", "{:paths [\"src\"]}");
    let (a, c) = (clj_str(&a), clj_str(&c));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def b (td/create-basis {{:user nil
                                           :project {{:paths [\"src\"] :deps {{'a/a {{:local/root {a}}}}}}}
                                           :aliases [:rep]
                                           :extra {{:aliases {{:rep {{:replace-deps {{'c/c {{:local/root {c}}}}}
                                                                     :replace-paths [\"r\"]}}}}}}}}))
                 [(= (:deps b) {{'c/c {{:local/root {c}}}}})
                  (= (:paths b) [\"r\"])
                  (= (sort (keys (:libs b))) '(c/c))]"
            )
        ),
        "[true true true]"
    );
}

// ---- print-tree ----

#[test]
fn bdd_print_tree() {
    // Scenario: `print-tree` draws each root tree with
    // `coord-summary` lines indented under parents (deps.clj
    // `print-tree` + `make-tree`).
    let mut eval = api_eval();
    let root = scratch("bdd-tree");
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{b/b {{:local/root {}}}}}}}",
            clj_str(&b)
        ),
    );
    let c = write_lib(&root, "C", "{:paths [\"src\"]}");
    let (a, b, c) = (clj_str(&a), clj_str(&b), clj_str(&c));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'a/a {{:local/root {a}}} 'c/c {{:local/root {c}}}}}}} nil))
                 (= (with-out-str (td/print-tree m))
                    (str \"a/a \" (port/canonical-path {a}) \"\\n\"
                         \"  b/b \" (port/canonical-path {b}) \"\\n\"
                         \"c/c \" (port/canonical-path {c}) \"\\n\"))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= (with-out-str (td/print-tree {})) \"\")"),
        "true"
    );
}

// ---- prep-libs! ----

/// A lib with a `:deps/prep-lib` entry; returns the root.
fn preplib(root: &std::path::Path, name: &str) -> String {
    write_lib(
        root,
        name,
        "{:paths [\"src\"] :deps/prep-lib {:alias :build :fn prep/go :ensure \"classes\"}}",
    )
}

#[test]
fn bdd_prep_detection() {
    // Scenario: default `:error` lists unprepped libs,
    // already-prepped and prep-less libs stay silent (deps.clj
    // `prep-libs!` detection — faithful under R4).
    let mut eval = api_eval();
    let root = scratch("bdd-prep");
    let p1 = preplib(&root, "P1");
    let p2 = preplib(&root, "P2");
    std::fs::create_dir_all(root.join("P2").join("classes")).unwrap();
    let plain = write_lib(&root, "L", "{:paths [\"src\"]}");
    let (p1, p2, plain) = (clj_str(&p1), clj_str(&p2), clj_str(&plain));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'p1/p1 {{:local/root {p1}}}
                                                      'p2/p2 {{:local/root {p2}}}
                                                      'l/l {{:local/root {plain}}}}}}} nil))
                 (try (td/prep-libs! m {{}} nil)
                      \"silent\"
                      (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)]))"
            )
        ),
        "[\"The following libs must be prepared before use: [p1/p1]\" {:unprepped [p1/p1]}]"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m2 (td/resolve-deps {{:deps {{'p2/p2 {{:local/root {p2}}} 'l/l {{:local/root {plain}}}}}}} nil))
                 (nil? (td/prep-libs! m2 {{}} nil))"
            )
        ),
        "true"
    );
}

#[test]
fn prep_exec_refuses_source_only() {
    // `:prep` on an unprepped lib and `:force` anywhere reach
    // `exec-prep!`, which refuses source-only (R4); `:prep` on
    // a prepped lib short-circuits silently.
    let mut eval = api_eval();
    let root = scratch("prep-r4");
    let p1 = preplib(&root, "P1");
    let p2 = preplib(&root, "P2");
    std::fs::create_dir_all(root.join("P2").join("classes")).unwrap();
    let (p1, p2) = (clj_str(&p1), clj_str(&p2));
    for (lib, action) in [(&p1, ":prep"), (&p2, ":force")] {
        let err = uncaught(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'p/p {{:local/root {lib}}}}}}} nil))
                 (td/prep-libs! m {{:action {action}}} nil)"
            ),
        );
        assert!(
            err.contains("Source-only port: cannot exec prep fn prep/go"),
            "{err}"
        );
    }
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'p/p {{:local/root {p1}}}}}}} nil))
                 (try (td/prep-libs! m {{:action :prep}} nil)
                      \"silent\"
                      (catch clojure.lang.ExceptionInfo e (ex-data e)))"
            )
        ),
        "{:fn prep/go}"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m2 (td/resolve-deps {{:deps {{'p/p {{:local/root {p2}}}}}}} nil))
                 (nil? (td/prep-libs! m2 {{:action :prep}} nil))"
            )
        ),
        "true"
    );
}

#[test]
fn prep_debug_and_current() {
    // `:debug` logs each lib considered; `:current true`
    // preps the project itself (deps.clj `prep-libs!`
    // `:current` + `:log` handling).
    let mut eval = api_eval();
    let root = scratch("prep-debug");
    let p2 = preplib(&root, "P2");
    std::fs::create_dir_all(root.join("P2").join("classes")).unwrap();
    let plain = write_lib(&root, "L", "{:paths [\"src\"]}");
    let (p2, plain) = (clj_str(&p2), clj_str(&plain));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def m (td/resolve-deps {{:deps {{'p2/p2 {{:local/root {p2}}} 'l/l {{:local/root {plain}}}}}}} nil))
                 (= (with-out-str (td/prep-libs! m {{:log :debug}} nil))
                    \"p2/p2 - prepped\\nl/l - no prep\\n\")"
            )
        ),
        "true"
    );
    // The current project, unprepped, is reported under its
    // synthetic lib name.
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    write_dep(
        &proj,
        "deps.edn",
        "{:paths [\"src\"] :deps/prep-lib {:alias :build :fn prep/go :ensure \"classes\"}}",
    );
    let lit = clj_str(&proj.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {lit}
                   (try (td/prep-libs! {{}} {{:current true}} nil)
                        \"silent\"
                        (catch clojure.lang.ExceptionInfo e (ex-data e))))"
            )
        ),
        // Symbols print raw, so the synthetic lib reads with a
        // bare space (same as the JVM).
        "{:unprepped [current project]}"
    );
}

// ---- resolve-added-libs ----

#[test]
fn resolve_added_libs_rows() {
    // Upstream `resolve-added-libs`: added libs resolve with
    // paths, no-ops return `{}`, and a changed `:local` root
    // throws (the compare has no "older" — `:conflict` is
    // unreachable for `:local`, as upstream).
    let mut eval = api_eval();
    let root = scratch("added-libs");
    let b = write_lib(&root, "B", "{:paths [\"src\"]}");
    let a = write_lib(
        &root,
        "A",
        &format!(
            "{{:paths [\"src\"] :deps {{b/b {{:local/root {}}}}}}}",
            clj_str(&b)
        ),
    );
    let (a, b) = (clj_str(&a), clj_str(&b));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(def r (td/resolve-added-libs {{:existing {{'a/a {{:local/root {a}}}}}
                                                  :add {{'b/b {{:local/root {b}}}}}}}))
                 [(= (keys (:added r)) '(b/b))
                  (nil? (:conflict r))
                  (= (:paths (get (:added r) 'b/b)) [(port/canonical-path (str {b} \"/src\"))])]"
            )
        ),
        "[true true true]"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (td/resolve-added-libs {{:existing {{'a/a {{:local/root {a}}} 'b/b {{:local/root {b}}}}}
                                             :add {{'b/b {{:local/root {b}}}}}}})
                    {{}})"
            )
        ),
        "true"
    );
    let b2 = write_lib(&root, "B2", "{:paths [\"src\"]}");
    let b2 = clj_str(&b2);
    let err = uncaught(
        &mut eval,
        &format!(
            "(td/resolve-added-libs {{:existing {{'b/b {{:local/root {b}}}}}
                                      :add {{'b/b {{:local/root {b2}}}}}}})"
        ),
    );
    assert!(
        err.contains("No known ancestor relationship between local versions"),
        "{err}"
    );
}

// ---- deprecated delegates, metadata, exclusions ----

#[test]
fn lib_location_delegate() {
    // `lib-location` delegates to the SPI (deps.clj
    // `lib-location`).
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (td/lib-location 'my/lib {:local/root \"/x\"} nil) (ext/lib-location 'my/lib {:local/root \"/x\"} nil))"
        ),
        "true"
    );
}

#[test]
fn deprecated_meta_pins() {
    // The deprecation metadata survives the port (C13):
    // `{:deprecated}` on the delegates, `:arglists` on
    // `resolve-deps`.
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (:deprecated (meta #'td/slurp-deps)) \"1.0\")"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:deprecated (meta #'td/make-classpath)) \"0.9.745\")"
        ),
        "true"
    );
    // Port-canonical: `defn` stores attr-map values
    // unevaluated, so the upstream `:arglists '([deps-map
    // args-map])` reads back quoted (upstream evaluates it to
    // `([deps-map args-map])`). Pinned as-is; if core `defn`
    // ever evaluates attr-maps, update to the upstream shape.
    assert_eq!(
        run(
            &mut eval,
            "(= (:arglists (meta #'td/resolve-deps)) '(quote ([deps-map args-map])))"
        ),
        "true"
    );
}

#[test]
fn find_latest_version_absent() {
    // X1: `find-latest-version` is excluded, not stubbed — no
    // such var resolves. (If a future slice ports it, update
    // the spec and this test together.)
    let mut eval = api_eval();
    assert_eq!(
        run(
            &mut eval,
            "(nil? (resolve 'clojure.tools.deps/find-latest-version))"
        ),
        "true"
    );
}

// ---- find-edn-maps (SERIAL env) ----

/// Save an env var's current value for later restore.
fn save_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Restore an env var to a saved value (or unset it).
fn restore_env(name: &str, saved: Option<String>) {
    // SAFETY: single-threaded within this test; every mutated var
    // is restored before it returns.
    unsafe {
        match saved {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

/// Panic-safe env restore for the serial test: dropping the
/// guard re-applies the entry saves even if an assert unwinds.
struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..) {
            restore_env(name, value);
        }
    }
}

#[test]
fn find_edn_maps_serial() {
    // THE serial env test: the only test here that mutates
    // process env, with save/restore. Deprecated `find-edn-maps`
    // remaps `create-edn-maps` keys (`:root` → `:root-edn`,
    // etc.); `CLJ_CONFIG` pins the user source to an empty dir
    // so the assertion is hermetic.
    let _guard = EnvGuard {
        saved: vec![("CLJ_CONFIG", save_env("CLJ_CONFIG"))],
    };
    let root = scratch("find-maps");
    // SAFETY: serial test; restored by the guard on exit.
    unsafe { std::env::set_var("CLJ_CONFIG", &root) };
    let proj = write_dep(&root, "proj-deps.edn", "{:paths [\"p\"]}");
    let mut eval = api_eval();
    // `with-dir` pins the `:standard` project source to the
    // scratch dir (no `deps.edn` there), so the 0-arity call
    // cannot see the test runner's cwd.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {} (td/find-edn-maps))",
                clj_str(&root.to_string_lossy())
            )
        ),
        "{:root-edn {}}"
    );
    assert_eq!(
        run(&mut eval, &format!("(td/find-edn-maps {})", clj_str(&proj))),
        "{:root-edn {}, :project-edn {:paths [\"p\"]}}"
    );
    assert_eq!(run(&mut eval, "(= (td/root-deps) (edn/root-deps))"), "true");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (td/slurp-deps {}) (edn/read-deps {}))",
                clj_str(&proj),
                clj_str(&proj)
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (td/create-edn-maps {:root nil :user {:paths [\"u\"]}}) (edn/create-edn-maps {:root nil :user {:paths [\"u\"]}}))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (td/merge-edns [{:paths [\"p\"]}]) (edn/merge-edns [{:paths [\"p\"]}]))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (td/combine-aliases {:aliases {:a {:extra-paths [\"x\"]}}} [:a]) (edn/combine-aliases {:aliases {:a {:extra-paths [\"x\"]}}} [:a]))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= (td/user-deps-path) (edn/user-deps-path))"),
        "true"
    );
}
