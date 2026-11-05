Hi! Thanks for your interest in clojure-rs!

This repository (`hhh-pa/clojure`) is a native Clojure port in Rust —
not upstream JVM Clojure (`clojure/clojure`). Upstream language
questions still belong with the Clojure community, but anything about
this port (bugs, gaps, build issues, contributions) belongs here, on
GitHub.

> **Stability note:** `master` is under active development and may be
> unstable. Tagged `v*` releases are the tested, stable snapshots —
> please rely on the latest release for anything important.

## I want to ask a question

- About the Clojure language in general: use the official Ask Clojure
  forum at https://ask.clojure.org, or the forums listed under
  [Clojure Discussion](https://clojure.org/community/resources#_clojure_discussion).
- About this Rust port (behavior, build, LLVM setup, roadmap): please
  [open a GitHub issue](https://github.com/hhh-pa/clojure/issues).

## I want to file a bug / suggest an enhancement

Please [open a GitHub issue](https://github.com/hhh-pa/clojure/issues)
(not Ask Clojure, not JIRA). Include:

- What you ran and what you expected vs. what happened.
- A minimal repro (a `.clj` snippet or command line).
- The version: release tag (e.g. `v1.13.0-alpha9`) or `master` SHA —
  and prefer reproducing against the latest release first, since
  `master` moves fast.
- Your platform (`uname -a` / Windows version), Rust version
  (`rustc --version`), and whether the `aot` feature / LLVM 22 was
  enabled.
- For behavior questions, the JVM Clojure output for the same input,
  if you have it — the port is differentially verified against it.

## I want to provide a patch / PR

Pull requests are welcome here (unlike upstream, this project works
entirely through GitHub PRs).

1. Fork, branch off `master`, and open a PR against `master`.
2. Sign your commits — branch protection requires verified signatures
   (SSH: `gpg.format=ssh`, `commit.gpgsign=true`, key registered under
   GitHub Settings → SSH and GPG keys → Signing keys).
3. Keep CI green and get one approving review (`CODEOWNERS`
   auto-requests it) before merge.

Before opening the PR, run at least (from `rust/`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Notes for contributors:

- MSRV is stable Rust ≥ 1.85. The default `aot` feature needs LLVM 22
  — see [LLVM setup](rust/README.md#llvm-setup-the-aot-feature). Code
  must also build with `--no-default-features` (interpreter-only).
- Add or update tests with every behavior change. CI enforces 100%
  line + patch coverage floors, plus `cargo-deny`, `cargo-geiger`,
  `cargo-udeps`, `cargo-audit`, loom schedule tests, and TLA+ model
  checks — a PR that drops coverage or trips a gate will not merge.
- Behavior changes should match JVM Clojure (same input, same printed
  output). If a difference is intentional, document and pin it: update
  the [design spec](docs/t1174/specification.md) / per-area contracts
  (`docs/t1174/*-contract.md`) and the normative
  [core coverage list](rust/core-coverage.md), which CI machine-checks.
- New code is `Copyright (c) Hao Hu` under EPL-1.0; keep the
  `Copyright (c) Rich Hickey` notices on vendored upstream sources,
  and use dual notices on adapted files.

## I am looking for documentation

- This port: the [root README](README.md) (scope, CLI, accepted
  differences), the [workspace README](rust/README.md) (LLVM setup,
  build commands), the [design spec](docs/t1174/specification.md), and
  the [core coverage list](rust/core-coverage.md).
- The Clojure language: [reference](https://clojure.org/reference),
  [guides](https://clojure.org/guides), and [API](https://clojure.org/api/api).

## What release should I use?

Use the latest tagged `v*` release:
https://github.com/hhh-pa/clojure/releases

Each release attaches per-platform `clojure-rs-<target>.tar.gz`
archives (binary + runtime archive). `master` tracks upstream Clojure
master and is where development happens — it may be broken or
inconsistent at any given commit, so count on the released version.
