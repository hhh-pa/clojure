# S7 oracle pins: REPL + CLI (T1174)

Oracle: `java -Duser.language=en -Duser.country=US
-Duser.timezone=UTC -cp clojure.jar clojure.main` (1.13.0-master-SNAPSHOT).
Probe inputs: `/tmp/s7probe/{session,env}.clj` (not committed). JVM
truth first; port deviations are explicit and spec-justified.

## P1. `*1`/`*2`/`*3` update rules

- Fresh session: all three nil (`:star1 3 :star2 nil :star3 nil`
  after one value).
- EVERY successful top-level form shifts the window — including
  `nil` results (`:after-nil nil nil 3` traces `(3,nil,nil)` →
  `(nil,3,nil)` [the prn] → `(nil,nil,3)` [the nil]).
- Errors do NOT shift (`:star-after-err 3 nil nil` — `*1` keeps
  the last good value) and set `*e` (`:e? true`).
- The session iterates PER-FORM: each form evals, prints, and
  shifts in turn, stopping at the first error — so `1 nope`
  prints `1` first, then the `Unable to resolve` diagnostic
  (verified against the piped JVM; earlier values are never
  dropped and `*1` shifts per form).

## P2. `*e`

- Set to the throwable on any read/eval/print error; the session
  continues. Initial value nil (probed implicitly: `:e?` false
  before the first error — the session probe errors only once).

## P3. `clojure-version` / `*clojure-version*`

- `(clojure-version)` → `"1.13.0-master-SNAPSHOT"` (plain string).
- `*clojure-version*` → `{:major 1, :minor 13, :incremental 0,
  :qualifier "master", :interim true}` (note `:interim`, no
  `:patch`). PORT: same keys over the cargo version
  (`{:major 0, :minor 1, :incremental 0, :qualifier nil,
  :interim true}` + `(clojure-version)` → `"0.1.0"`).

## P4. `*command-line-args*`

- Script args arrive as a SEQ: `("a" "b")` (pr-str shows a list).
- Nil when no args. PORT: a `Value::List` (nil when empty).

## P5. Error shapes + exit codes

- `clojure.main -e '(/ 1 0)'`: `Execution error
  (ArithmeticException) at user/eval1 (REPL:1).\nDivide by
  zero\n\nFull report at: ...edn`, exit 1. Eval errors carry
  `file:line` (no col); compile syntax errors carry
  `file:line:col` (`Syntax error compiling at (REPL:1:1)`).
- `-e '(+ 1 2)'` prints `3`, exit 0.
- `-e '1 2'` prints EVERY form (`1\n2`), exit 0 (multi-form
  `-e` is per-form, verified by execution).
- `-e ''` prints nothing, exit 0 (blank input is silent,
  verified by execution).
- `-e` binds no `*1` (`-e '1 *1'` prints just `1` — the
  second form reads nil — verified by execution).
- `-e` suppresses nil results (`-e '1 nil 2'` prints `1\n2`,
  verified by execution). PORT (open deviation, flagged in
  PR #8 round-1 fixes): `eval` prints EVERY value including
  nil (`1\nnil\n2`) — awaiting a reviewer call.
- Missing script file: `Execution error
  (FileNotFoundException) ... (No such file or directory)` +
  full report, exit 1. PORT (spec C8, deliberate): exit 3
  naming the file, no JVM exception verbiage.
- PORT (spec 1.4): all eval/run diagnostics are
  `file:line:col: message` on stderr (col always present —
  a superset of the JVM's sometimes-col-less shapes), exit
  1, no `Full report at` file (documented deviation).

## P6. Prompts, banner, namespaces

- Piped stdin still gets the banner (`Clojure
  1.13.0-master-SNAPSHOT`) AND `user=> ` prompts (even with
  empty stdin: banner + `user=> `, exit 0).
- The prompt tracks `*ns*` (`foo=> ` after `(in-ns 'foo)`).
- PORT (streaming purity, deliberate): tty prints a
  `clojure-rs <version>` banner once + `<ns>=> ` prompts
  (ns-tracked, mirroring the JVM); non-tty stdin prints
  NEITHER banner NOR prompts — one form in, one result out
  (spec 1.4 "streaming eval"). Pipelines stay machine-readable;
  the JVM's piped prompts are not reproduced.

## P7. `run` ≡ load + args

- `*file*` binds to the script path during load (already
  implemented: `load-file` binds it). PORT: `run` reuses the
  evaluator's `load-file` path (same `*file*` binding, same
  whole-file semantics) with `*command-line-args*` bound
  around it; per-form echo stays off (program output only).
