# Macros contract (S5C, normative)

`defmacro`, `macroexpand-1`/`macroexpand`, `gensym`, macro
invocation during analysis, the `declare`/`ns` builtins, the
builtin-macro vars, the `*ns*`/registry sync, and the early
list-building natives syntax-quote needs. Oracles: probe38 (92
tx), probe37 `declare-*`/`ns-*`, corner probes `s5ccorner*`,
`Compiler.java` (`macroexpand1`, `analyzeSeq`, `isMacro`,
`makeMsg`), core.clj (`declare`, `ns`).

## Macro values and `defmacro`

- A macro is a var whose `:macro` meta is truthy (`def`,
  `defmacro`, and `alter-meta!` all confer it, probe38
  `63-def-as-macro`, `64-alter-meta-macro`); re-`def`ing a macro
  as a plain fn clears it, like every `def` meta reset
  (`65-redefine-macro-fn`, `66-redefine-fn-macro`).
- `defmacro` is `(def name (fn [&form &env & args] ...))` with
  `:macro true` and `:arglists` (the user arglists, vector of
  vectors, probe38 `01-defmacro-ret`): docstring, attrmap (which
  wins over the docstring for `:doc`, like `ns`), multi-arity,
  pre/post all flow through the `fn` machinery. An explicit
  leading `&form` param shadows the implicit one positionally
  (probe38 `83-explicit-ampersand`), because it is purely
  positional prepending. The closure display is the qualified
  var name, so direct calls and arity errors name `user/m`
  (probe38 `05-macro-fn-direct`, `06-macro-fn-arity`).
- `defmacro` shape failures (missing/non-symbol name, empty or
  misshapen fdecl) are the spec shape `Syntax error
  macroexpanding clojure.core/defmacro` + `ExceptionInfo | Call
  to clojure.core/defmacro did not conform to spec.` (the JVM
  routes through `defn`; the port names its own validator —
  fence). Clause *contents* (params, destructuring, pre/post)
  validate through the `fn` parser with `fn`-flavored errors.
- Referring to a macro var in value position is `RuntimeException
  | Can't take value of a macro: #'ns/m` (probe38
  `22-macro-value`, `Compiler.java` `analyzeSymbol`); locals
  shadow macros (`67-local-shadows`).

## Expansion

- Analysis macroexpands like `analyzeSeq`: specials first (exact
  unqualified symbols — `(user/do 1 2)` is a var lookup that
  misses with `No such var: user/do`, corner `a-*`), then
  macros (locals shadow, `isMacro`), then the private check,
  then invocation. Macro calls re-analyze the expansion
  (probe38 `03-defmacro-multi`), including nested calls
  (`31-eval-nested-arith`); `quote` blocks expansion
  (`32-eval-quote-blocks`). A `Var` operator expands when its
  var is a macro (probe38 `19-me1-var-op`, `20-eval-var-op`);
  any other non-symbol head invokes (`21-eval-var-call-form`).
- The macro fn runs with `(form, env, args...)`: `&form` is the
  whole call form with its reader meta (probe38 `37-showform`,
  `38-formmeta-load`, `39-formmeta-offset`), `&env` is nil at
  top level and a local-name map inside bindings (probe38
  `40-envkeys-top`, `41-envkeys-let`, `42-envnil-top`). `&env`
  values are the JVM's `LocalBinding` objects; the port maps
  names to `true` (keys/`contains?`/truthiness all behave —
  fence). Outside macro calls `&form`/`&env` are ordinary
  unresolvable symbols (probe38 `80-amp-outside`).
- Expansion failures wrap per `Compiler.macroexpand1`
  (source-read): a macro's *own* `ArityException` rethrows bare
  with 2 hidden (`06-macro-fn-arity`, `68-me1-macro-arity`,
  `84-inner-arity` is an inner fn's, also bare);
  `IllegalArgumentException` (including `NumberFormatException`),
  `IllegalStateException`, `ExceptionInfo`, and exactly
  `java.lang.Exception` wrap as `Syntax error macroexpanding
  {op}`; everything else wraps as `Unexpected error
  macroexpanding {op}` (probe38 `70-boom-me1`, corners
  `e/f/p1/p2/p3`). `{op}` is the operator as written (absent
  for `Var` operators, per the source); the span is the form's
  reader meta or the inherited analyzer span. `CompilerException`
  cannot occur (the port has no such class): a nested `eval` of
  uncompilable code inside a macro wraps as `Unexpected` where
  the JVM rethrows bare (fence). Analysis also intercepts
  `clojure.core`-qualified builtins (`(clojure.core/let …)` as
  emitted by syntax-quote) while the core var is intact;
  qualified real specials stay var lookups that miss.
- `macroexpand-1` (1-arity) expands one head step: non-seqs,
  `()`, weird operators, quoted/special forms (including
  `clojure.core/do`), and unresolvable operators return
  unchanged (probe38 `10-me1-nonmacro-seq` … `18-me1-special`,
  `81-me1-weird-ops`, `87-unresolvable-qualified`); the nine
  analyzer builtins (`let`, `loop`, `fn`, `binding`, `letfn`,
  `with-redefs`, `with-bindings`, `bound-fn`, `locking`) expand
  one step to the alpha9 shapes (`let*`/`fn*`/`loop*` and the
  qualified helper calls; probe `fx4-macroexpand`); `defmacro`,
  `declare`, `ns`, `assert`, `time`, and `with-out-str` stay
  the identity fence (the JVM expands them); non-macro
  `Var` operators return unchanged (corner `k-me1-varinc`).
  `&env` is nil (the native has no locals); the wrap blames the
  call site (probed: a nested call reports the call's position,
  never the form's meta; top-level calls fall back to `1:1` per
  the S5A2 eval-span fence).
- `macroexpand` (1-arity) loops `macroexpand-1` while the head
  is a macro (equivalent to the JVM's identity loop in every
  terminating case, and hangs in the same self-similar cases,
  probe38 `34-me-full-chain`, `28-me-full-thread`); only the
  head expands, never nested forms (probe38 `29-me-nested-head-only`).
- Private macros referenced cross-namespace escape bare —
  `IllegalStateException | var: #'ns/m is not public` with no
  wrapper — from every expansion path: direct, nested,
  chained, `macroexpand-1`, and `macroexpand` (all probed;
  probe38 `85-private-macro`, corner `g-*`). Private *fns*
  differ by position: invocation wraps as `Syntax error
  compiling {op-as-written}` with the `#'` target (the
  `isInline` analog); value position wraps without an operator
  (`Syntax error compiling at …`) and spells the symbol as
  written without `#'` (`var: fx/privfn is not public`, probed).
  Value-position macros likewise wrap without an operator
  (`Can't take value of a macro: #'ns/m`, probed; private is
  checked first). Same-namespace uses are exempt. The `var`
  form never checks (source: `TheVarExpr`).

## `*ns*`/registry sync (lifts the S5A fence)

- The current namespace is the thread-local `*ns*` handle when
  bound (boot binds it; `in-ns` keeps both in sync), else the
  registry current. Analysis, `def` installation, `refer`,
  `alias`, `load` roots, and reader options all read the synced
  name (probe38 `45-whichns-pair`, `88-expansion-ns-irrelevant`).

## `declare` and `ns` (builtins)

- `(declare a b c)` is per-name `def`-with-no-init plus
  `:declared true`, returning the last var: roots and existing
  meta survive, `^:dynamic` works, re-declares are identical
  (probe37 `declare-ret`, `declare-keep-root`,
  `declare-dynamic`, `declare-twice`). Qualified names fail
  through `def` (`Can't refer...`/`Can't create defs...` under
  `Syntax error compiling def`, probe37 `declare-qualified`);
  non-symbols are the JDK `IObj` checkcast text (probe37
  `declare-num`, fx2-mapentry18 `declare-5` — the JVM reaches it
  through `vary-meta` during expansion; the port raises it in
  the parser, same shape).
- `(ns name doc? attr? clause*)` validates up front (anything
  misshapen — non-symbol or qualified name, misplaced
  docstring/attrmap, non-list or unknown-keyword clause — is
  `Syntax error macroexpanding clojure.core/ns` +
  `ExceptionInfo | Call to clojure.core/ns did not conform to
  spec.`, probe37 `ns-bad-clause`, `ns-num-name`, corners
  `o-ns-string`), so a failed `ns` never switches namespaces
  (probe37 `ns-bad-clause-ns`). Evaluation runs `in-ns`,
  unconditional `resetMeta` (attrmap wins over the docstring
  for `:doc`, corner `h-doc-both`; absent attrmap clears),
  default-or-clause `refer-clojure`, each `:require`/`:use`
  clause in order through the loader (flags like `:reload`
  flow through, corner `m-ns-reload`; load failures surface
  raw), then marks the lib loaded and returns nil (probe37
  `ns-create`, `ns-switches`, `ns-created-state`,
  `ns-require-clause`, `ns-dup`, probe36 `ns-marks-loaded`).
  `:import`/`:gen-class`/`:load` clauses are ns-spec errors
  (N1: no host classes — loud, never silently skipped; the
  JVM's `:import` destructure accidents are bare `ClassCastException`s — fence).

## Builtin-macro vars

- `let`, `loop`, `fn`, `binding`, `letfn`, `with-redefs`,
  `with-bindings`, `bound-fn`, and `locking` are real macro
  fns (`:macro true`). `macroexpand-1` matches alpha9 one step
  (gensym ids are the port counter — `prefixn` / `prefix__n__auto__`
  — not the JVM's boot id). Direct invocation is that fn: a
  short call is `ArityException` on `clojure.core/{name}` with
  the raw count (no `&form`/`&env` strip). Destructuring
  `let`/`loop`/`fn` rewrites through `destructure` before
  analyze. There is no spec instrumentation, so shapes the JVM
  rejects in spec raise the macro body's own
  `IllegalArgumentException` / `Exception` under `Syntax error
  macroexpanding {op-as-written}`. `(let)` with no args is the
  arity error (the JVM's spec message fires first).
- `defmacro`, `declare`, `ns`, `assert`, `time`, and
  `with-out-str` stay placeholder natives: direct invocation is
  `IllegalStateException | Can't invoke builtin macro:
  clojure.core/{name}`, and `macroexpand-1` is the identity
  (the JVM expands these).
- Top-level `do` splitting macroexpands first (full,
  `Compiler.eval`): a `do`-headed expansion splits and each
  element re-enters `eval` (macros defining macros before
  later-element analysis see them); nested `do` stays single.

## `gensym` and auto-gensyms

- `(gensym)`/`(gensym prefix)`: `str` of the prefix (`G__` by
  default, `""` for nil) plus a shared per-boot counter from
  1 (`G__1`, `p2`, `3`, `55`; probe38 `53-gensym-raw`,
  `54-gensym-shape`, `55-gensym-nil-num-empty`). 2 args is the
  arity error (`56-gensym-arity`).
- Syntax-quote `sym#` resolves at read time to a fixed
  `sym__id__auto__` per form (repeat expansions are `=`,
  probe38 `52-auto-distinct`), and qualification is read-time
  too (`92-read-ns-dependence`); both already work in the S2
  reader once options are populated. The reader counter is a
  separate process-global (the JVM shares one `nextID`; shapes
  match, exact ids vary — fence).

## Early natives (S5D rows, needed now)

- Syntax-quote expansions call `seq`/`concat`/`list`/`apply`/
  `vector`/`hash-map`/`hash-set` (shapes verified against the
  JVM, corners `i-*`). S5C implemented all seven eagerly; S5D
  moved `seq`/`concat` to the lazy `lazy`/`seqs` roots per C4
  (the eager copies were deleted once shadowing was proven —
  later installs overwrite both roots before any eval), leaving
  `list`, `vector`, `hash-map`/`hash-set` (odd `hash-map` args
  are `No value supplied for key:`, like `array-map`), `apply`
  (2+, last arg spread, nil spreads nothing) here. Pinned by
  probe39 `seq-first-rest`, `vec-list-ctor`,
  `zipmap-ctor-distinct`, `concat-cycle`, `apply-edges`.

## Fences

- `macroexpand-1` identity on `defmacro`/`declare`/`ns`/
  `assert`/`time`/`with-out-str` (the other nine analyzer
  builtins expand; above); `defmacro`
  validation attribution (`clojure.core/defmacro` top-level,
  `clojure.core/fn` for clause contents; the JVM says `defn`
  throughout); `&env` values (`true`, not `LocalBinding`s);
  nested-`eval` compile errors inside macros (`Unexpected`,
  not bare); rootless-macro calls (the unbound-invoke error
  wrapped per partition); separate reader/`gensym` counters;
  `seq`/`concat` went lazy in S5D (eager copies removed);
  map seqs yield vectors (S5D may add `MapEntry`); `String`
  syntax-quote has no import (`user/String`, N1).
