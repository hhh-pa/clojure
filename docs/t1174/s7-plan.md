# S7 slice plan: REPL + CLI (T1174)

Branch `T1174-s7-repl-cli`. Charter: spec 1.4/1.5 (`eval`/`run`/
`repl` wiring over the S1 CLI shell and the `clj-repl` stub;
`build` stays stubbed for S8), the 7 S7 `core-coverage.md` flips
(A5: `*1` `*2` `*3` `*e`, `*command-line-args*`,
`clojure-version`, `*clojure-version*`), packaging smoke on
artifacts (A1/A3). TDD: PTY-less REPL driver tests + CLI smoke
tests first (spec §2.5).

## S7A. `eval` + `run` wiring (CLI core)

- `eval <form> [--edn]`: read one form (the `--edn` flag
  restricts the reader), eval, print the result + `\n`; errors
  exit 1 (C8) with a `file:line:col: message` diagnostic on
  stderr (the `<eval>` file token and empty-input behavior are
  oracle-probe items — JVM `clojure.main -e` truth first).
- `run <file> [args...]`: missing file keeps exit 3 naming the
  file (already in the shell); else eval every form, program
  output only (no per-form echo), `*command-line-args*` bound
  per probe (shape + empty case), errors name `file:line:col`
  and exit 1. `*file*` binding follows the `load-file` probe.
- `clojure-version` / `*clojure-version*` rows (probe the JVM
  string + map shapes; the port reports its own version vector
  in the same shapes).
- CLI smoke tests in `rust/tests/cli.rs` replace the
  not-implemented asserts (TDD first); fixtures under
  `rust/tests/fixtures/`.
- Out: `build` (S8), `*file*`/`load-file` beyond what `run`
  needs (stays unlisted unless the probe forces it).

## S7B. REPL core (`clj-repl`, PTY-less)

- `Session` over generic IO (`BufRead` + `Write` — no tty
  dependency): prompt → read → eval → print → loop.
- `*1`/`*2`/`*3`/`*e` update rules per pins (nil results?
  errors? multi-form lines? — probe `clojure.main/repl`
  first).
- Multiline continuation: incomplete-form reader errors
  (`EofDelimited`/`EofString`/`EofChar`/`EofRegex` inside a
  form — unit test per variant) read more lines instead of
  erroring; other reader errors print and continue.
- Prompt text mirrors `clojure.main`: `<ns>=> ` (probe the
  ns-tracking + piped-stdin prompt policy first; the
  streaming fallback prints what the pins say — deviation
  documented if we skip prompts off-tty).
- EOF (Ctrl-D/empty) exits 0; read/eval errors print the
  diagnostic, bind `*e`, and continue — the session never
  dies on user input.
- Driver tests: scripted input via `Cursor`, byte-exact
  transcripts (TDD first). No PTY anywhere (spec §2.5).
- Out: line editing/history/completion (S7C).

## S7C. tty editing (rustyline)

- `rustyline` dependency (spec pre-approved pending gate
  pass): check `rust-version` ≤ 1.85, MIT license, and
  `multiple-versions = "deny"` impact BEFORE `cargo add`;
  the lock update commits in-slice.
- History file `~/.clojure-rs/history` (+ `--history-file`/
  `--no-history`, already in clap): load/append round-trip;
  unreadable/unwritable history warns on stderr, never kills
  the session.
- Tab completion over interned `clojure.core` names (unit
  tests over the completer fn, no tty).
- Ctrl-C cancels the current input (fresh prompt), Ctrl-D on
  empty input exits 0 (interrupt mapping unit-tested).
- Multiline on tty reuses the S7B incomplete-form check
  (rustyline validator); rustyline itself is untested
  (dependency).
- Windows/macOS/Linux: `is_terminal` via std; home-dir
  lookup without new deps (`HOME`/`USERPROFILE` chain).

## S7D. Packaging smoke + transcript corpus

- `rust-release.yml`: after each platform artifact builds,
  extract it and run `clojure-rs eval '(+ 1 2)'` → exactly
  `3` (dry-run evidence in the slice PR).
- `s7-corpus/`: scripted REPL transcripts through the
  streaming driver vs JVM piped-stdin goldens (spec G3/A2 —
  same input, same printed output); stability re-runs per
  the S6 recipe.

## Design (spikes before S7C)

D1. `rustyline` 19.0.0: MIT, edition 2024 (needs ≥1.85,
    matching the MSRV floor); `cargo deny check`
    (advisories/bans/licenses/sources) green; `cargo tree -d`
    clean workspace-wide. Chosen over hand-rolled editing:
    maintained, tty-correct on 3 OSes, spec-pre-approved.
    (No `rust-version` keys exist yet to enforce the floor —
    tracked as S9 hardening: add the keys + a 1.85-toolchain
    check.)
D2. Incomplete-form detection: exactly which `ReaderError`
    variants mean "continue" (probe + unit test per variant;
    `Eof` at top level means done, never continue).
D3. History/tty failures are session-local (warn + continue);
    no `unwrap` on any IO path in the loop.
D4. No new `unsafe` (C-spec); no PTY/test-only tty crates.

## TDD order

1. Oracle probes → `s7-pins.md` (`*1`/`*2`/`*3`/`*e` rules,
   `clojure-version` shapes, `*command-line-args*` shape,
   `clojure.main -e` error shape, piped-stdin prompts,
   ns prompt, `*file*` under `load-file`).
2. CLI smoke tests (`eval`/`run`/codes/diagnostics) then
   S7A impl.
3. REPL driver tests (transcripts, multiline, error-continue,
   EOF) then S7B impl.
4. rustyline adapter unit tests (completion, history
   round-trip, interrupt mapping) then S7C impl.
5. Packaging smoke step + dry-run run (S7D) + transcript
   corpus with goldens.
6. Coverage flips (7 S7 rows) + matrix green.

## Acceptance

- Zero `out-of-scope | S7` rows; BDD 1.3 `eval`/`run`/`repl`
  scenarios pass (`build` stays S8-stubbed).
- Driver + smoke suites green; 3-OS CI (rustyline builds
  everywhere); release dry-run shows the smoke step passing
  per artifact.
- Per-slice gates (fmt/clippy/quality-gates) via PR CI.
- Spec + contracts update in-slice (`Ref T1174` + `AI-Tool:`
  trailers per commit).
