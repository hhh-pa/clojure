# Issue #29: D-macroexpand row flips — Specification

- Status: ready (docs-only slice under the divergence program).
- Parent: divergence/out-of-scope reduction. Issue: #29.
  Branch: `29-macroexpand-rows`.

## Part 1 — User/functional specification

### 1.1 Goals

1. The nine analyzer-builtin rows that `macroexpand-1` expands
   (`let`, `loop`, `fn`, `binding`, `letfn`, `with-redefs`,
   `with-bindings`, `bound-fn`, `locking`) move from `divergent`
   to `implemented` in `rust/core-coverage.md`, with notes that
   name the expansion target and the pinning test.
2. The `macroexpand-1` / `macroexpand` row notes stop claiming
   builtins are identity: only real specials and the six fence
   macros are identity now.

### 1.2 Non-goals (explicit)

- No behavior change: the FX4 expanders (`builtin_expand.rs`,
  wired in `ns.rs`, pinned by `tests/fx4_macroexpand.rs`) already
  expand all nine heads; verified shape-for-shape against the
  1.13.0-alpha9 jars (modulo gensym counters).
- No change to `ns-imports` (`N1` interop slice) or `*print-dup*`
  (printer slice): they stay divergent.

### 1.3 Acceptance criteria

- [ ] Nine rows flipped; two macroexpand row notes corrected.
- [ ] Port-vs-JVM expansion evidence recorded below for all nine.
- [ ] `coverage_matrix` green; full gate suite green.

## Part 2 — Technical specification

Oracle evidence (port `clojure-rs eval` vs alpha10 `macroexpand-1`):

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

CI impact: `rust/core-coverage.md` is under `rust/**`; the
`coverage_matrix` test re-parses it (resolution-only). Markdown-only
diff, no coverage delta expected.
