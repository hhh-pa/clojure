# Issue #29: D-macroexpand row flips — Specification

- Status: ready (docs-only slice under the divergence program).
- Parent: divergence/out-of-scope reduction. Issue: #29.
  Branch: `29-macroexpand-rows`.

## Problem

Nine analyzer builtins (`let`, `loop`, `fn`, `binding`,
`letfn`, `with-redefs`, `with-bindings`, `bound-fn`, `locking`)
are implemented as expanding macros — the FX4 expanders in
`rust/crates/clj-eval/src/builtin_expand.rs`, installed as macro
vars by `boot_install_builtin_macros` (`ns.rs`), pinned by
`tests/fx4_macroexpand.rs` — yet `rust/core-coverage.md` still
lists them as `divergent` with stale "`macroexpand-1` identity"
notes. The file claims 353/11/58 while the tree implements
362/2/58, so the inventory lies about nine vars and about what
`macroexpand-1` does to builtins.

## Solution

Flip the nine rows to `implemented` with notes naming the
expansion target and the pinning test; correct the
`macroexpand-1` / `macroexpand` notes (builtins expand — only
real specials and the six fence macros are identity); update the
counts to 362/2/58. No behavior change: port-vs-JVM expansion
shapes were verified equal (table below) before flipping.

## Part 1 — User/functional specification

### 1.1 Goals

1. Every `macroexpand-1` behavior the port implements is reported
   honestly: no `divergent` row may describe behavior the port
   already matches.
2. Each flipped note names the machine-checkable pin (test fn),
   so a regression fails CI instead of silently re-diverging.

### 1.2 Non-goals (explicit)

- No behavior change to expansion, evaluation, or errors.
- No change to `ns-imports` (`N1` interop slice) or `*print-dup*`
  (printer slice): they stay divergent.
- No new tests: `expand_nine_heads`,
  `expand_locking_and_destructure_gensyms`, and `macros.rs`
  `macroexpand_one` already pin all nine heads.

### 1.3 Acceptance criteria

- [ ] Nine rows flipped; two macroexpand row notes corrected.
- [ ] Counts read 362 implemented, 2 divergent, 58 out of scope.
- [ ] `coverage_matrix` green; full gate suite green.

### 1.4 BDD scenarios (user-observable)

- Given `(macroexpand-1 '(let [a 1] a))`, when evaluated, then
  `(let* [a 1] a)` — the JVM shape, not the input form.
- Given `(macroexpand-1 '(locking x body))`, when evaluated,
  then the monitor-enter/exit form with fresh `__auto__` gensyms.
- Given `(macroexpand-1 '(do 1 2))`, when evaluated, then the
  form unchanged (real specials stay identity).
- Given `(macroexpand-1 '(declare a))`, when evaluated, then the
  form unchanged (fence macros stay identity).

## Part 2 — Technical specification

- Source of truth: `clojure/clojure` `core.clj` macro bodies at
  the alpha10 tag, confirmed by running each expansion against
  the alpha10 jars.
- Port shape: unchanged (`builtin_expand.rs` FX4 expanders +
  `is_expand_macro` exemption in `is_identity_head`).
- Contracts pinned (already): one-step expansion per head with
  nil `&env`; nested `let`/`fn` not expanded by one step;
  gensym discipline for `locking`/destructuring; fence-six and
  real-special identity; error shapes for bad binding vectors.
- TDD plan: none new (pins exist). Had they not, the plan would
  be: failing `macroexpand-1` assertions per head first (forms
  below), then the expander, then the fence/identity guards.
- CI impact: no workflow changes; `rust.yml` gates apply
  (fmt, clippy `-D warnings`, 100% line+patch floors,
  complexity ratchet). Markdown-only diff, no coverage delta.

### Constraints

EPL-1.0 headers on new files; MSRV 1.85 (no let-chains); signed
commits (m-hu) with `Ref #29`; green-before-push; macOS work runs
from `rust/`; oracle command + output committed for every new
behavior test (none new here — evidence table instead).

### Oracle evidence (port `clojure-rs eval` vs alpha10 `macroexpand-1`)

| head | port | JVM | match |
| --- | --- | --- | --- |
| `(let [a 1] a)` | `(let* [a 1] a)` | `(let* [a 1] a)` | exact |
| `(loop [a 1] a)` | `(loop* [a 1] a)` | `(loop* [a 1] a)` | exact |
| `(fn [x] x)` | `(fn* ([x] x))` | `(fn* ([x] x))` | exact |
| `(binding [x 1] x)` | `(clojure.core/let [] (push-thread-bindings …) …)` | same | exact |
| `(letfn [(f [x] x)] (f 1))` | `(letfn* [f (clojure.core/fn f [x] x)] (f 1))` | same | exact |
| `(with-redefs [a 1] a)` | `(clojure.core/with-redefs-fn {(var a) 1} (clojure.core/fn [] a))` | same | exact |
| `(with-bindings {#'*out* *out*} …)` | `(clojure.core/with-bindings* …)` | same shape | exact |
| `(locking x body)` | `(clojure.core/let [lockee__N__auto__ …] …)` | same | modulo gensym counter |
| `(bound-fn [x] x)` | `(clojure.core/bound-fn* (clojure.core/fn [x] x))` | same | exact |
