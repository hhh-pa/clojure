// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-124 slice-2 tests: the tools.deps extension SPI — the full
//! `extensions` shell, the `local` / `git` / `deps` extension
//! namespaces, and the `util.io` / `util.session` utilities.
//!
//! Oracle (text only, never executed): `org.clojure/tools.deps`
//! 0.31.1646 (`extensions.clj`, `extensions/local.clj`,
//! `extensions/git.clj`, `extensions/deps.clj`,
//! `extensions/pom.clj`, `util/session.clj`, `util/io.clj`); each
//! test cites the upstream file/behavior it pins. Divergences are
//! the spec's B1-B12/N1/R2 (`docs/gap-124/slice-2.md`).
//!
//! The session cache is process-global with no `with-session`
//! scoping yet (slice 3), so every filesystem case uses a unique
//! `scratch()` dir — session keys never collide.

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

/// Require all nine slice namespaces (the extension namespaces
/// register their SPI methods as a side effect).
fn require_spi(eval: &mut Evaluator) {
    eval.eval_source(
        "(require '[clojure.tools.deps.util.port :as port]
                  '[clojure.tools.deps.util.dir :as dir]
                  '[clojure.tools.deps.edn :as edn]
                  '[clojure.tools.deps.extensions :as ext]
                  '[clojure.tools.deps.util.io :as tio]
                  '[clojure.tools.deps.util.session :as session]
                  'clojure.tools.deps.extensions.local
                  '[clojure.tools.deps.extensions.git :as git]
                  'clojure.tools.deps.extensions.deps)",
    )
    .unwrap();
}

/// A fresh evaluator with the SPI namespaces required.
fn spi_eval() -> Evaluator {
    let mut eval = boot();
    require_spi(&mut eval);
    eval
}

/// A scratch dir unique to this test (parallel-safe), wiped first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clj-td124s2-{name}-{}", std::process::id()));
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

/// A full git sha (40 chars) for canonical-form tests.
const FULL_SHA: &str = "0123456789012345678901234567890123456789";

// ---- load (ns.rs resolve_source) ----

#[test]
fn spi_nss_load_via_require() {
    // The loader serves the five new embedded libs (the slice-1
    // four still load — the shell now carries the full SPI).
    let mut eval = boot();
    require_spi(&mut eval);
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

// ---- port substrate: is-file? ----

#[test]
fn is_file_rows() {
    // Upstream `File/isFile`: true for plain files, false for
    // dirs and missing paths (the slice-2 native).
    let mut eval = spi_eval();
    let dir = scratch("is-file");
    let file = write_dep(&dir, "a.txt", "x");
    assert_eq!(
        run(&mut eval, &format!("(port/is-file? {})", clj_str(&file))),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(port/is-file? {})", clj_str(&dir.to_string_lossy()))
        ),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(port/is-file? {})",
                clj_str(&dir.join("nope.txt").to_string_lossy())
            )
        ),
        "false"
    );
    assert!(uncaught(&mut eval, "(port/is-file? 42)").contains("java.lang.String"));
    assert!(uncaught(&mut eval, "(port/is-file?)").contains("Wrong number of args"));
}

// ---- shell: manifest-types + detect-manifest ----

#[test]
fn manifest_types_pin() {
    // Upstream extensions.clj `manifest-types`: the exact vector
    // (pom detection stays — the `:pom` methods refuse, B9).
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= ext/manifest-types [\"deps.edn\" :deps \"pom.xml\" :pom])"
        ),
        "true"
    );
}

#[test]
fn detect_manifest_rows() {
    // Upstream extensions.clj `detect-manifest`: the first
    // manifest-types hit wins; nothing found answers nil.
    let mut eval = spi_eval();
    let deps_dir = scratch("detect-deps");
    write_dep(&deps_dir, "deps.edn", "{:paths [\"src\"]}");
    let d = clj_str(&deps_dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(= (ext/detect-manifest {d}) {{:deps/manifest :deps :deps/root {d}}})")
        ),
        "true"
    );
    let pom_dir = scratch("detect-pom");
    write_dep(&pom_dir, "pom.xml", "<project/>");
    let p = clj_str(&pom_dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(= (ext/detect-manifest {p}) {{:deps/manifest :pom :deps/root {p}}})")
        ),
        "true"
    );
    // deps.edn wins when both exist (vector order).
    let both_dir = scratch("detect-both");
    write_dep(&both_dir, "deps.edn", "{}");
    write_dep(&both_dir, "pom.xml", "<project/>");
    let b = clj_str(&both_dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(= (ext/detect-manifest {b}) {{:deps/manifest :deps :deps/root {b}}})")
        ),
        "true"
    );
    // Empty dirs answer nil (no throw).
    let empty_dir = scratch("detect-empty");
    let e = clj_str(&empty_dir.to_string_lossy());
    assert_eq!(run(&mut eval, &format!("(ext/detect-manifest {e})")), "nil");
    // A directory named deps.edn is not a manifest (isFile).
    let subdir = scratch("detect-subdir");
    std::fs::create_dir_all(subdir.join("deps.edn")).unwrap();
    let s = clj_str(&subdir.to_string_lossy());
    assert_eq!(run(&mut eval, &format!("(ext/detect-manifest {s})")), "nil");
}

// ---- shell: coord-type-keys + procurer-types + anti-drift ----

#[test]
fn coord_type_keys_rows() {
    // Upstream `coord-type-keys` methods + `:default`
    // (`:local`/`:git` register from their own namespaces;
    // `:mvn` is the shell refusal stub — B1).
    let mut eval = spi_eval();
    assert_eq!(
        run(&mut eval, "(= (ext/coord-type-keys :local) #{:local/root})"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (ext/coord-type-keys :git) #{:git/url :git/sha :git/tag :sha})"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= (ext/coord-type-keys :mvn) #{:mvn/version})"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= (ext/coord-type-keys :bogus) #{})"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(= (ext/procurer-types) #{:local :git :mvn})"),
        "true"
    );
}

#[test]
fn coord_type_matches_published_keys() {
    // B1 anti-drift: every published kind key dispatches its
    // kind through `coord-type` (probed via `lib-location` —
    // `:local` resolves, `:mvn`/`:git` throw their markers).
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(ext/lib-location 'my/lib {:local/root \"/l\"} nil)"
        ),
        "{:base \"/l\", :path \"\", :type :local}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/lib-location 'my/lib {:mvn/version \"1\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Source-only port: cannot resolve Maven coordinate for my/lib"
    );
    for coord in [
        "{:git/url \"u\"}",
        "{:git/sha \"s\"}",
        "{:git/tag \"t\"}",
        "{:sha \"s\"}",
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(ext/lib-location 'my/lib {coord} nil)")
            ),
            "clojure.lang.ExceptionInfo | Cannot resolve git coordinate for my/lib: deferred to the git-network slice",
            "{coord} dispatches :git"
        );
    }
}

// ---- shell: coord-multimethod defaults ----

#[test]
fn coord_multimethod_defaults() {
    // Upstream `:default` methods: canonicalize passes through;
    // find-versions throws `throw-bad-coord`; summary
    // stringifies; license answers nil.
    let mut eval = spi_eval();
    // canonicalize :default passes [lib coord] through (:mvn has
    // no method — downstream arms refuse or stay unloaded).
    assert_eq!(
        run(
            &mut eval,
            "(= (ext/canonicalize 'my/lib {:mvn/version \"1\"} nil) '[my/lib {:mvn/version \"1\"}])"
        ),
        "true"
    );
    // find-versions :default throws throw-bad-coord.
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/find-versions 'my/lib {:mvn/version \"1\"} :mvn nil)"
        ),
        "clojure.lang.ExceptionInfo | Coordinate type :mvn not loaded for library my/lib in coordinate #:mvn{:version \"1\"}"
    );
    // Non-map coords take the bad-coord shape.
    assert_eq!(
        uncaught(&mut eval, "(ext/dep-id 'my/lib \"1.2.3\" nil)"),
        "clojure.lang.ExceptionInfo | Bad coordinate for library my/lib, expected map: \"1.2.3\""
    );
    // coord-summary :default stringifies lib + type.
    assert_eq!(
        run(
            &mut eval,
            "(ext/coord-summary 'my/lib {:mvn/version \"1\"})"
        ),
        "\"my/lib :mvn\""
    );
    // license-info :default is nil.
    assert_eq!(
        run(
            &mut eval,
            "(ext/license-info 'my/lib {:mvn/version \"1\"} nil)"
        ),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/license-info-mf 'my/lib {} :bogus nil)"),
        "nil"
    );
}

#[test]
fn compare_versions_default_rows() {
    // Upstream `compare-versions :default`: pairs with no vector
    // method (mixed kinds, unregistered kinds) throw "Unable to
    // compare versions" (B6 fallthrough).
    let mut eval = spi_eval();
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/compare-versions 'my/lib {:local/root \"/a\"} {:mvn/version \"1\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Unable to compare versions for my/lib: #:local{:root \"/a\"} and #:mvn{:version \"1\"}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/compare-versions 'my/lib {:git/url \"u\"} {:local/root \"/a\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Unable to compare versions for my/lib: #:git{:url \"u\"} and #:local{:root \"/a\"}"
    );
}

#[test]
fn find_all_versions_doall_throws() {
    // Upstream `find-all-versions` mapcats `find-versions` over
    // the registry (lazy — `doall` forces). `:local` answers nil,
    // `:git` finds no url (nil), `:mvn` is not loaded — so an mvn
    // coord deterministically throws the not-loaded text.
    let mut eval = spi_eval();
    assert_eq!(
        uncaught(
            &mut eval,
            "(doall (ext/find-all-versions 'my/lib {:mvn/version \"1\"} nil))"
        ),
        "clojure.lang.ExceptionInfo | Coordinate type :mvn not loaded for library my/lib in coordinate #:mvn{:version \"1\"}"
    );
}

#[test]
fn manifest_multimethod_defaults() {
    // Upstream `throw-bad-manifest` (both arms) behind every
    // manifest multimethod's `:default`.
    let mut eval = spi_eval();
    for f in [
        "ext/coord-deps",
        "ext/coord-paths",
        "ext/manifest-file",
        "ext/coord-usage",
        "ext/prep-command",
    ] {
        assert_eq!(
            uncaught(&mut eval, &format!("({f} 'my/lib {{}} nil nil)")),
            "clojure.lang.ExceptionInfo | Manifest file not found for my/lib in coordinate {}",
            "{f} nil manifest"
        );
        assert_eq!(
            uncaught(&mut eval, &format!("({f} 'my/lib {{}} :lein nil)")),
            "clojure.lang.ExceptionInfo | Manifest type :lein not loaded for my/lib in coordinate {}",
            "{f} unknown manifest"
        );
    }
}

#[test]
fn pom_manifest_rules() {
    // R1 corollary (B9/B10): `:pom` deps/paths refuse source-only;
    // the manifest file stays a faithful path; usage/prep stay
    // upstream-exact nils; licenses fall to `:default` nil.
    let mut eval = spi_eval();
    for f in ["ext/coord-deps", "ext/coord-paths"] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("({f} 'my/lib {{:deps/root \"/r\"}} :pom nil)")
            ),
            "clojure.lang.ExceptionInfo | Source-only port: cannot read pom manifest for my/lib (no XML in the source-only port)",
            "{f} :pom refuses"
        );
    }
    let dir = scratch("pom-mf");
    write_dep(&dir, "pom.xml", "<project/>");
    let root = clj_str(&dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/manifest-file 'my/lib {{:deps/root {root}}} :pom nil) (port/canonical-path (str {root} \"/pom.xml\")))"
            )
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(ext/coord-usage 'my/lib {} :pom nil)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/prep-command 'my/lib {} :pom nil)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/license-info-mf 'my/lib {} :pom nil)"),
        "nil"
    );
}

// ---- local: canonicalize (BDD) + dep-id + compare ----

#[test]
fn bdd_local_canonicalize_absolutizes() {
    // Scenario: local canonicalize absolutizes (local.clj
    // `canonicalize` + `ensure-file` — [lib coord] shape).
    let mut eval = spi_eval();
    let proj = scratch("bdd-local-canon");
    std::fs::create_dir_all(proj.join("rel/lib")).unwrap();
    let lit = clj_str(&proj.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(dir/with-dir {lit} (= (ext/canonicalize 'my/lib {{:local/root \"rel/lib\"}} nil) ['my/lib {{:local/root (dir/canonicalize \"rel/lib\")}}]))"
            )
        ),
        "true"
    );
}

#[test]
fn bdd_missing_local_lib_refused() {
    // Scenario: missing local lib is refused (local.clj
    // `ensure-file` text, over the canonical root).
    let mut eval = spi_eval();
    let proj = scratch("bdd-local-missing");
    let missing = proj.join("nope").to_string_lossy().into_owned();
    let m = clj_str(&missing);
    // The message embeds the RAW path (`str`, not `pr`): unescape
    // the pr-string (strip quotes, undo backslash doubling) so
    // Windows verbatim `\\?\` paths compare correctly.
    let canon = run(&mut eval, &format!("(port/canonical-path {m})"));
    let raw = canon.trim_matches('"').replace("\\\\", "\\");
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/canonicalize 'my/lib {{:local/root {m}}} nil)")
        ),
        format!(
            "clojure.lang.ExceptionInfo | Local lib my/lib not found: {}",
            raw
        )
    );
}

#[test]
fn local_dep_id_find_versions_summary() {
    // Upstream local.clj `dep-id` / `find-versions` /
    // `coord-summary`.
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (ext/dep-id 'my/lib {:local/root \"/l\"} nil) {:lib 'my/lib :root \"/l\"})"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(ext/find-versions 'my/lib {} :local nil)"),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(ext/coord-summary 'my/lib {:local/root \"/l\"})"
        ),
        "\"my/lib /l\""
    );
}

#[test]
fn local_compare_versions_rows() {
    // Upstream `compare-versions [:local :local]` (B6 vector
    // dispatch): same root is 0, different roots throw "No known
    // ancestor".
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(ext/compare-versions 'my/lib {:local/root \"/a\"} {:local/root \"/a\"} nil)"
        ),
        "0"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/compare-versions 'my/lib {:local/root \"/a\"} {:local/root \"/b\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | No known ancestor relationship between local versions for my/lib: /a and /b"
    );
}

// ---- local: manifest-type (R2) + jar rules + license ----

#[test]
fn bdd_jar_root_refused_at_detection() {
    // Scenario: jar root is refused at manifest detection (R2 —
    // local.clj `manifest-type` file arm).
    let mut eval = spi_eval();
    let dir = scratch("bdd-jar");
    let jar = write_dep(&dir, "lib.jar", "fake jar");
    let j = clj_str(&jar);
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/manifest-type 'my/lib {{:local/root {j}}} nil)")
        ),
        "clojure.lang.ExceptionInfo | Source-only port: cannot use jar manifest for my/lib (source directories only)"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(try (ext/manifest-type 'my/lib {{:local/root {j}}} nil) (catch clojure.lang.ExceptionInfo e (ex-data e)))"
            )
        ),
        format!("{{:lib my/lib, :coord #:local{{:root {j}}}}}")
    );
}

#[test]
fn local_manifest_type_rows() {
    // Upstream `manifest-type :local`: explicit `:deps/manifest`
    // wins untouched; dirs detect through; empty dirs answer nil;
    // missing roots throw the not-found text.
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(= (ext/manifest-type 'my/lib {:local/root \"/r\" :deps/manifest :jar} nil) {:deps/manifest :jar :deps/root \"/r\"})"
        ),
        "true"
    );
    let dir = scratch("local-mt");
    write_dep(&dir, "deps.edn", "{}");
    let d = clj_str(&dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/manifest-type 'my/lib {{:local/root {d}}} nil) {{:deps/manifest :deps :deps/root {d}}})"
            )
        ),
        "true"
    );
    let empty = scratch("local-mt-empty");
    let e = clj_str(&empty.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/manifest-type 'my/lib {{:local/root {e}}} nil)")
        ),
        "nil"
    );
    let proj = scratch("local-mt-missing");
    let missing = proj.join("nope").to_string_lossy().into_owned();
    let m = clj_str(&missing);
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/manifest-type 'my/lib {{:local/root {m}}} nil)")
        ),
        format!("clojure.lang.ExceptionInfo | Local lib my/lib not found: {missing}")
    );
}

#[test]
fn jar_manifest_rules() {
    // R2/B10 downstream: explicit-`:jar` deps/paths refuse;
    // manifest-file/usage/prep stay upstream-exact nils;
    // licenses fall to `:default` nil.
    let mut eval = spi_eval();
    for f in ["ext/coord-deps", "ext/coord-paths"] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("({f} 'my/lib {{:local/root \"/r.jar\"}} :jar nil)")
            ),
            "clojure.lang.ExceptionInfo | Source-only port: cannot use jar manifest for my/lib (source directories only)",
            "{f} :jar refuses"
        );
    }
    assert_eq!(
        run(
            &mut eval,
            "(ext/manifest-file 'my/lib {:local/root \"/r.jar\"} :jar nil)"
        ),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/coord-usage 'my/lib {} :jar nil)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/prep-command 'my/lib {} :jar nil)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(ext/license-info-mf 'my/lib {} :jar nil)"),
        "nil"
    );
}

#[test]
fn local_license_info_rows() {
    // Upstream `license-info :local`: merges `manifest-type`,
    // delegates to `-mf` (deps manifests have no license reader
    // — `:default` nil; jar files refuse at detection).
    let mut eval = spi_eval();
    let dir = scratch("local-lic");
    write_dep(&dir, "deps.edn", "{}");
    let d = clj_str(&dir.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/license-info 'my/lib {{:local/root {d}}} nil)")
        ),
        "nil"
    );
    // Explicit-`:jar` merges through to the `:default` nil.
    assert_eq!(
        run(
            &mut eval,
            "(ext/license-info 'my/lib {:local/root \"/r\" :deps/manifest :jar} nil)"
        ),
        "nil"
    );
    // Jar files refuse at detection (R2).
    let jdir = scratch("local-lic-jar");
    let jar = write_dep(&jdir, "lib.jar", "x");
    let j = clj_str(&jar);
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/license-info 'my/lib {{:local/root {j}}} nil)")
        ),
        "clojure.lang.ExceptionInfo | Source-only port: cannot use jar manifest for my/lib (source directories only)"
    );
}

// ---- deps: coord-deps + coord-paths (BDD) ----

#[test]
fn bdd_deps_manifest_deps_read() {
    // Scenario: deps manifest deps are read (deps.clj
    // `coord-deps` — nested deps.edn, session-cached).
    let mut eval = spi_eval();
    let lib = scratch("bdd-coord-deps");
    write_dep(
        &lib,
        "deps.edn",
        "{:paths [\"src\"] :deps {a/a {:local/root \"../a\"}}}",
    );
    let r = clj_str(&lib.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/coord-deps 'my/lib {{:deps/root {r}}} :deps nil) '([a/a {{:local/root \"../a\"}}]))"
            )
        ),
        "true"
    );
    // Missing manifests read as root-deps ({}) — no deps, nil.
    let bare = scratch("bdd-coord-deps-bare");
    let b = clj_str(&bare.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/coord-deps 'my/lib {{:deps/root {b}}} :deps nil)")
        ),
        "nil"
    );
}

#[test]
fn bdd_deps_manifest_paths_resolve() {
    // Scenario: deps manifest paths resolve (deps.clj
    // `coord-paths` — canonical strings vec).
    let mut eval = spi_eval();
    let lib = scratch("bdd-coord-paths");
    write_dep(&lib, "deps.edn", "{:paths [\"src\" \"test\"]}");
    let r = clj_str(&lib.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/coord-paths 'my/lib {{:deps/root {r}}} :deps nil) [(dir/canonicalize (str {r} \"/src\")) (dir/canonicalize (str {r} \"/test\"))])"
            )
        ),
        "true"
    );
    // Missing manifests have no paths ([]).
    let bare = scratch("bdd-coord-paths-bare");
    let b = clj_str(&bare.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/coord-paths 'my/lib {{:deps/root {b}}} :deps nil)")
        ),
        "[]"
    );
}

#[test]
fn deps_coord_paths_external_warns() {
    // Upstream deps.clj warn: `:paths` outside the root print a
    // WARNING on *err* but still resolve.
    let mut eval = spi_eval();
    let lib = scratch("deps-warn");
    write_dep(&lib, "deps.edn", "{:paths [\"../outside\"]}");
    let r = clj_str(&lib.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (with-out-str (binding [*err* *out*] (ext/coord-paths 'my/lib {{:deps/root {r}}} :deps nil))) (str \"WARNING: Deprecated use of path \" (port/canonical-path (str {r} \"/../outside\")) \" external to project \" {r} \"\\n\"))"
            )
        ),
        "true"
    );
}

// ---- deps: manifest-file + usage + prep ----

#[test]
fn deps_manifest_file_rows() {
    // Upstream deps.clj `manifest-file`: canonical path when the
    // manifest exists, nil otherwise (B3).
    let mut eval = spi_eval();
    let lib = scratch("deps-mf");
    write_dep(&lib, "deps.edn", "{}");
    let r = clj_str(&lib.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/manifest-file 'my/lib {{:deps/root {r}}} :deps nil) (port/canonical-path (str {r} \"/deps.edn\")))"
            )
        ),
        "true"
    );
    let bare = scratch("deps-mf-bare");
    let b = clj_str(&bare.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/manifest-file 'my/lib {{:deps/root {b}}} :deps nil)")
        ),
        "nil"
    );
}

#[test]
fn deps_usage_prep_rows() {
    // Upstream deps.clj `coord-usage` / `prep-command`: usage
    // maps and prep+exec-args pass through; absent answers nil.
    let mut eval = spi_eval();
    let lib = scratch("deps-usage");
    write_dep(
        &lib,
        "deps.edn",
        "{:tools/usage {:ns-default foo :ns-aliases {s foo.bar}} :deps/prep-lib {:ensure \"c\" :alias :b :fn b/f} :aliases {:b {:exec-args {:x 1}}}}",
    );
    let r = clj_str(&lib.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/coord-usage 'my/lib {{:deps/root {r}}} :deps nil) '{{:ns-default foo :ns-aliases {{s foo.bar}}}})"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/prep-command 'my/lib {{:deps/root {r}}} :deps nil) '{{:ensure \"c\" :alias :b :fn b/f :exec-args {{:x 1}}}})"
            )
        ),
        "true"
    );
    let bare = scratch("deps-usage-bare");
    let b = clj_str(&bare.to_string_lossy());
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/coord-usage 'my/lib {{:deps/root {b}}} :deps nil)")
        ),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(ext/prep-command 'my/lib {{:deps/root {b}}} :deps nil)")
        ),
        "nil"
    );
}

// ---- util.session + util.io ----

#[test]
fn session_retrieve_rows() {
    // Upstream session.clj `retrieve`: absent keys compute,
    // store, and answer; present keys answer without calling the
    // fn; the 1-arity reads (nil on miss).
    let mut eval = spi_eval();
    assert_eq!(run(&mut eval, "(session/retrieve :s2-missing)"), "nil");
    assert_eq!(run(&mut eval, "(session/retrieve :s2-k (fn [] 41))"), "41");
    // Present: the fn is not called (it would throw).
    assert_eq!(
        run(
            &mut eval,
            "(session/retrieve :s2-k (fn [] (throw (ex-info \"called\" {}))))"
        ),
        "41"
    );
    assert_eq!(run(&mut eval, "(session/retrieve :s2-k)"), "41");
}

#[test]
fn io_printerrln_pin() {
    // Upstream io.clj `printerrln`: space-joined + `\n` (B12) on
    // *err*.
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            "(with-out-str (binding [*err* *out*] (tio/printerrln \"a\" 1 :k)))"
        ),
        "\"a 1 :k\\n\""
    );
}

// ---- git: url + sha helpers ----

#[test]
fn git_auto_url_full_sha_rows() {
    // Upstream git.clj `auto-git-url` (all six services + nils)
    // and `full-sha?`.
    let mut eval = spi_eval();
    for (lib, url) in [
        ("io.github.foo/bar", "\"https://github.com/foo/bar.git\""),
        ("com.github.foo/bar", "\"https://github.com/foo/bar.git\""),
        ("io.gitlab.foo/bar", "\"https://gitlab.com/foo/bar.git\""),
        (
            "org.codeberg.foo/bar",
            "\"https://codeberg.org/foo/bar.git\"",
        ),
        (
            "io.bitbucket.foo/bar",
            "\"https://bitbucket.org/foo/bar.git\"",
        ),
        (
            "com.beanstalkapp.foo/bar",
            "\"https://foo.git.beanstalkapp.com/bar.git\"",
        ),
        ("ht.sr.foo/bar", "\"https://git.sr.ht/~foo/bar\""),
    ] {
        assert_eq!(
            run(&mut eval, &format!("(git/auto-git-url '{lib})")),
            url,
            "{lib}"
        );
    }
    // No match (plain groups, unqualified libs) answers nil.
    assert_eq!(run(&mut eval, "(git/auto-git-url 'my/lib)"), "nil");
    assert_eq!(run(&mut eval, "(git/auto-git-url 'lib)"), "nil");
    // full-sha?: 40 chars, non-nil.
    assert_eq!(
        run(&mut eval, &format!("(git/full-sha? \"{FULL_SHA}\")")),
        "true"
    );
    assert_eq!(run(&mut eval, "(git/full-sha? \"abc\")"), "false");
    assert_eq!(run(&mut eval, "(git/full-sha? nil)"), "false");
}

// ---- git: canonicalize (BDD) ----

#[test]
fn bdd_git_full_sha_canonicalizes() {
    // Scenario: git full-sha canonicalizes (git.clj
    // `canonicalize` standard form — explicit and inferred urls,
    // legacy `:sha` key).
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/canonicalize 'my/lib {{:git/url \"https://x/r.git\" :git/sha \"{FULL_SHA}\"}} nil) '[my/lib {{:git/url \"https://x/r.git\" :git/sha \"{FULL_SHA}\"}}])"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/canonicalize 'io.github.foo/bar {{:git/sha \"{FULL_SHA}\"}} nil) '[io.github.foo/bar {{:git/url \"https://github.com/foo/bar.git\" :git/sha \"{FULL_SHA}\"}}])"
            )
        ),
        "true"
    );
    // Legacy `:sha` standardizes (`:sha` dissoced).
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/canonicalize 'my/lib {{:git/url \"https://x/r.git\" :sha \"{FULL_SHA}\"}} nil) '[my/lib {{:git/url \"https://x/r.git\" :git/sha \"{FULL_SHA}\"}}])"
            )
        ),
        "true"
    );
}

#[test]
fn bdd_git_validation_rejects() {
    // Scenario: git validation rejects bad coords (git.clj
    // `canonicalize` — upstream-exact messages).
    let mut eval = spi_eval();
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/canonicalize 'mylib {{:git/sha \"{FULL_SHA}\"}} nil)")
        ),
        "clojure.lang.ExceptionInfo | Invalid lib name: mylib"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                "(ext/canonicalize 'my/lib {{:git/url \"https://x\" :sha \"s\" :git/sha \"{FULL_SHA}\"}} nil)"
            )
        ),
        "clojure.lang.ExceptionInfo | git coord has both :sha and :git/sha for my/lib"
    );
    // Both :tag and :git/tag (upstream passes `tag` — B11 note).
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/canonicalize 'my/lib {:git/url \"https://x\" :tag \"v1\" :git/tag \"v1\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | git coord has both :tag and :git/tag for v1"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(ext/canonicalize 'my/lib {{:git/sha \"{FULL_SHA}\"}} nil)")
        ),
        "clojure.lang.ExceptionInfo | Failed to infer git url for: my/lib"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/canonicalize 'my/lib {:git/url \"https://x\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Library my/lib has coord with missing sha"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/canonicalize 'my/lib {:git/url \"https://x\" :git/sha \"abc123\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Library my/lib has prefix sha, use full sha or add tag"
    );
}

#[test]
fn bdd_git_tags_defer_to_network() {
    // Scenario: git tags defer to the network slice (N1/B11 —
    // `:git/tag` and legacy `:tag` both defer).
    let mut eval = spi_eval();
    for coord in [
        format!("{{:git/url \"https://x\" :git/sha \"{FULL_SHA}\" :git/tag \"v1\"}}"),
        format!("{{:git/url \"https://x\" :git/sha \"{FULL_SHA}\" :tag \"v1\"}}"),
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(ext/canonicalize 'my/lib {coord} nil)")
            ),
            "clojure.lang.ExceptionInfo | Cannot canonicalize git tag for my/lib: deferred to the git-network slice",
            "{coord} defers"
        );
    }
}

// ---- git: dep-id + summary + compare + network deferrals ----

#[test]
fn git_dep_id_summary_rows() {
    // Upstream git.clj `dep-id` / `coord-summary` via
    // `to-canonical` (pure for full-sha coords).
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(= (ext/dep-id 'my/lib {{:git/url \"https://x/r.git\" :git/sha \"{FULL_SHA}\"}} nil) {{:git/url \"https://x/r.git\" :git/sha \"{FULL_SHA}\"}})"
            )
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(ext/coord-summary 'my/lib {{:git/url \"https://x\" :git/sha \"{FULL_SHA}\"}})"
            )
        ),
        "\"my/lib 0123456\""
    );
}

#[test]
fn git_compare_rows() {
    // Upstream `compare-versions [:git :git]`: same commit is 0;
    // distinct commits need the ancestor check (N1).
    let mut eval = spi_eval();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(ext/compare-versions 'my/lib {{:git/url \"https://x\" :git/sha \"{FULL_SHA}\"}} {{:git/url \"https://y\" :git/sha \"{FULL_SHA}\"}} nil)"
            )
        ),
        "0"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/compare-versions 'my/lib {:git/url \"https://x\" :git/sha \"0123456789012345678901234567890123456789\"} {:git/url \"https://x\" :git/sha \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Cannot compare git versions for my/lib: deferred to the git-network slice"
    );
}

#[test]
fn git_network_arms_defer() {
    // N1/B11: procure, tag listing, and license (via
    // manifest-type) defer; validation still runs first.
    let mut eval = spi_eval();
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                "(ext/manifest-type 'my/lib {{:git/url \"https://x\" :git/sha \"{FULL_SHA}\"}} nil)"
            )
        ),
        "clojure.lang.ExceptionInfo | Cannot procure git coordinate for my/lib: deferred to the git-network slice"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/manifest-type 'mylib {:git/url \"https://x\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Invalid lib name: mylib"
    );
    // find-versions defers with a url, nil without (upstream).
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/find-versions 'my/lib {:git/url \"https://x\"} :git nil)"
        ),
        "clojure.lang.ExceptionInfo | Cannot list git versions for my/lib: deferred to the git-network slice"
    );
    assert_eq!(
        run(&mut eval, "(ext/find-versions 'my/lib {} :git nil)"),
        "nil"
    );
    // license-info throws via manifest-type.
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                "(ext/license-info 'my/lib {{:git/url \"https://x\" :git/sha \"{FULL_SHA}\"}} nil)"
            )
        ),
        "clojure.lang.ExceptionInfo | Cannot procure git coordinate for my/lib: deferred to the git-network slice"
    );
}

// ---- redesign: mvn never resolves (BDD) ----

#[test]
fn bdd_mvn_never_resolves() {
    // Scenario: mvn never resolves (R1 + corollary —
    // lib-location refuses; dep-id/manifest-type say "not
    // loaded"; compare says "unable").
    let mut eval = spi_eval();
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/lib-location 'my/lib {:mvn/version \"1\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Source-only port: cannot resolve Maven coordinate for my/lib"
    );
    assert_eq!(
        uncaught(&mut eval, "(ext/dep-id 'my/lib {:mvn/version \"1\"} nil)"),
        "clojure.lang.ExceptionInfo | Coordinate type :mvn not loaded for library my/lib in coordinate #:mvn{:version \"1\"}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/manifest-type 'my/lib {:mvn/version \"1\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Coordinate type :mvn not loaded for library my/lib in coordinate #:mvn{:version \"1\"}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(ext/compare-versions 'my/lib {:mvn/version \"1\"} {:mvn/version \"2\"} nil)"
        ),
        "clojure.lang.ExceptionInfo | Unable to compare versions for my/lib: #:mvn{:version \"1\"} and #:mvn{:version \"2\"}"
    );
}
