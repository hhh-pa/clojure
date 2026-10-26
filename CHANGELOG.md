# Changelog

All notable changes to the Rust port. Versions track the upstream
`clojure/clojure` release they are equivalent to.

## [1.13.0-alpha9] — 2026-10-26

First versioned release: the Rust equivalent of Clojure
1.13.0-alpha9 (validated against the 1.13.0-alpha9 jars).

Behavioral scope: the full ported surface on `master`, including
reader, values, analyzer builtins (`macroexpand-1` over the nine
builtins), real `MapEntry` values with entry-conformant natives,
`reversible?`, destructuring outlets, and the LLVM AOT path —
every behavior pinned by oracle probes under `docs/t1192/probes/`
and the `clj-core` coverage matrix.

Known divergences: the `divergent` rows of
`rust/core-coverage.md` (deliberate JVM wedges, kept) plus the
`out-of-scope` rows (host interop surface not yet ported).
`clojure-version` / `*clojure-version*` report
`1.13.0-alpha9` / `{:major 1, :minor 13, :incremental 0,
:qualifier "alpha9"}`.

Quality bar: `cargo fmt`, `cargo clippy`, MSRV 1.85, complexity
ratchet, loom + TLA+ model checks, and 100% line and patch
coverage, all green; reviewed to approval before landing.
