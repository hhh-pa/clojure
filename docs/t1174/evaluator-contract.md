# T1174 S4 evaluator contract (analyzer + tree-walk evaluator)

Normative error texts, spans, and value semantics for `clj-eval`, pinned
against JVM oracle probes 20-29 (`probes/probe20.clj` … `probe29.clj`).
All `CompilerException` causes below are oracle-verified verbatim; wrapper
rules mirror `Compiler.java` (`makeMsg`, `checkSpecs`, `macroexpand1`).

Companion contracts: `reader-contract.md` (S2), `values-contract.md` (S3).

## E1. Error rendering

Every analyzer/evaluator failure is a `CompileError`/`EvalError` with a JVM
class name, a message, and a span. Rendering has two layers:

1. **Wrapper** (analyzer errors only, mirrors `CompilerException`):
   `Syntax error <verb> <sym>at (<source>:<line>:<col>).`
   - `<verb>` is `compiling` (special forms, resolve errors) or
     `macroexpanding` (`let`/`fn` spec failures, `loop`/`letfn` body
     failures, `defn` spec failure in S5).
   - `<sym>` is the operator as written plus a space (`if `, `loop `),
     the resolved macro symbol plus a space for spec failures
     (`clojure.core/let `, `clojure.core/fn `), or empty for errors
     attributed to a subform (`(nil 1)`, resolve errors, unknown ns/var).
   - `<source>` is the file path; eval-fallback forms use line/col of the
     analyzed form. The trailing `.` is part of the message.
   - Macroexpansion failures caused by a `ClassCastException` render
     `Unexpected error` in place of `Syntax error`: the destructure
     walk's `^Named`-hinted `namespace` and entry casts surface as
     `Unexpected error macroexpanding loop at …` (probe25
     `loop-keys-nonsym`, `loop-or-string`).
2. **Cause**: `Class | message` (the informative layer). Tests assert the
   full chain `wrapper <= cause`.

Evaluator (runtime) failures have no wrapper: just `Class | message`.

Spans come from the form's `:line`/`:column` reader meta (lists,
vectors, maps carry it); atoms and meta-less forms inherit the nearest
enclosing span, defaulting to the top-level fallback (1:1 for a fresh
`eval_source`). This matches JVM precision: the JVM likewise has no
position for bare atoms (eval'd forms report the caller's line).

`CompilerException` ex-data (`:phase/:line/:column/:source/:symbol`, see
`Compiler.java` ctor) is NOT reproduced: our errors carry the span
structurally and render it in the wrapper. (`ex-data` on a compile error
value is out of scope; compile errors are Rust-side, not throwable values.)

## E2. Special-form arity/shape errors (analyzer, `compiling` wrapper)

Operator as written names the wrapper (`Syntax error compiling if at …`).

| Form | Cause class | Cause message |
|---|---|---|
| `(if)` / `(if 1)` | `RuntimeException` | `Too few arguments to if` |
| `(if 1 2 3 4)` | `RuntimeException` | `Too many arguments to if` |
| `(quote)` / `(quote a b)` | `ExceptionInfo` | `Wrong number of args (N) passed to quote` (N substituted; data nil, see E8) |
| `(def)` | `RuntimeException` | `Too few arguments to def` |
| `(def a 1 2)` | `RuntimeException` | `Too many arguments to def` |
| `(def :k 1)` | `RuntimeException` | `First argument to def must be a Symbol` |
| `(def other/x 1)` | `RuntimeException` | `Can't refer to qualified var that doesn't exist` |
| `(throw)` / `(throw a b)` | `RuntimeException` | `Too few/many arguments to throw, throw expects a single Throwable instance` |
| `(set! x)` | `IllegalArgumentException` | `Malformed assignment, expecting (set! target val)` |
| `(set! 1 2)` | `IllegalArgumentException` | `Invalid assignment target` |
| `(let [x 1] (set! x 2))` | `IllegalArgumentException` | `Cannot assign to non-mutable: x` (wrapper sym is the constant `fn*`, even inside a named `fn`; on the JVM this fires at emit time, hence the enclosing-method attribution) |
| `(var)` | `NullPointerException` | `Cannot read field "ns" because "sym" is null` (stable text = modern JDK; JDK-varying, documented) |
| `(var no-such)` | `RuntimeException` | `Unable to resolve var: no-such in this context` |
| `(var foo/x)` / `(var user/nope)` / `(var local)` | `RuntimeException` | `Unable to resolve var: X in this context` (one shape for unknown-ns, unknown-name, and local syms alike, probe25 `var-foo-ns`/`var-user-nope`/`var-local`) |
| `(var v extra…)` | — | extras ignored, returns `#'user/v` (probe25 `var-extra`) |
| `(var :k)` | `ClassCastException` | stable CCE Keyword→Symbol (E8) |
| `(recur)` without a target | `UnsupportedOperationException` | `Can only recur from tail position` (**divergence**: the JVM hangs in an infinite self-jump here — verified 2× by marker isolation, probe23 notes it; the port rejects like every other enclosure-less `recur`) |
| `(recur 1)` top level | `IllegalArgumentException` | `Mismatched argument count to recur, expected: 0 args, got: 1` |
| non-tail `(recur …)` | `UnsupportedOperationException` | `Can only recur from tail position` |
| `(recur …)` across `try` | `UnsupportedOperationException` | `Cannot recur across try` |
| arity mismatch | `IllegalArgumentException` | `Mismatched argument count to recur, expected: E args, got: G` |

Check order (mirrors `RecurExpr.Parser`): across-`try` first — it beats
tail, target, and arity alike — then tail (targeted only; targetless
never consults it), then arg analysis (failures keep their own
wrappers), then arity. So `(loop [a] (try (recur 1 2) …))` reports
across-`try`; `(loop [a 1] (recur nope q))` reports the resolve error,
never the mismatch (probe30 `recur-args-first`); non-tail
`(recur 1 2)` with a target reports tail position, but targetless
`(do (recur 1) 2)` reports the 0-vs-1 mismatch (probe30
`recur-nontail-targetless`, `recur-nontail-badargs`); and a non-tail
`recur` in a `try` still reports across-`try` — even targetless, where
it beats the 0-vs-N mismatch (probe30 `fence-loop-init`,
`recur-targetless-try`). A `fn` boundary resets the
fence (fresh `NO_RECUR` per `FnExpr`, whole clause — probe30
`fence-fn-try`, `fence-fn-prepost`); a `loop` body resets it too, but
`loop` inits keep the outer fence (probe30 `fence-loop-try` → `7`).
Only the `try` body is fenced: catch/finally bodies reset the fence
and hide the target (empty, never absent), so any `recur` there is
the tail-position error — even targetless or under an outer fence
(probe30 `fence-catch-*`, `fence-finally-*`). A nested `try` body
inside a clause re-arms the fence; a nested `fn`/`loop` re-establishes
a target (probe30 `fence-nested-try-in-catch`, `fence-fn-in-catch`,
`fence-loop-in-catch`).

`set!` analyzes its target first, then requires an assignable: a list
target such as `(set! (x) 2)` fails during analysis
(`Unable to resolve symbol: x in this context`), while an analyzed
non-assignable (`1`) is `Invalid assignment target`.

`(do)` → `nil`; `(do 1 2 3)` → `3`. `do` has no shape errors.

`(def user/q 1)` (qualified, current ns) works and returns `#'user/q`.

`(def d "docstring" 1)` binds `1` and records `:doc` in the var's meta
(auto `:line`/`:column`/`:file`/`:name`/`:ns` plus user meta, probe25
`def-doc-meta`); a 3-arg `def` whose second arg is not a string is
`Too many arguments to def` (probe25 `def-nonstring-doc`,
`def-attrmap` — the special form takes no attr-map).

## E3. `let` / `fn` spec errors (analyzer, `macroexpanding` wrapper)

The JVM runs every `let`/`fn` form through `clojure.spec.alpha`
`macroexpand-check` first (`Compiler.checkSpecs`); any shape violation is:

- wrapper sym = resolved macro (`clojure.core/let` / `clojure.core/fn`),
- cause `ExceptionInfo | Call to clojure.core/let did not conform to spec.`
  (resp. `.../fn ...`), data nil (no spec engine; problems payload dropped,
  documented in E8).

Shapes that fail spec (oracle): non-vector bindings, odd bindings,
numeric / set / list / bare-`&` / qualified bindings, set / list /
numeric / qualified fn params, bare `&` param, no clauses, mixed
single/multi clauses, named-fn bad params, nested `[[1]]` (spec is deep:
every level must be symbol | vector | map), non-map `:or`, dangling `&`
(`[a &]`), `&` at binding top level, non-symbol `:as`. Our analyzer
replicates the routing with a deep spec-shape check for `let`/`fn`:
plain symbol (not `&`, unqualified) | vector (recursively valid; `&`
only as second-to-last with a valid rest form after, `:as` alone may
follow the rest) | map (`:keys` a vector of idents — keywords bind by
name, probe25 `keys-keyword-first` — `:syms` a vector of symbols,
`:strs` a vector of simple symbols, `:or` a map, `:as` a symbol, other
entries `symbol keyword`).
Qualified `::ns-keys` (`:ns/keys`, `:ns/syms`, `:ns/keys!`, `:ns/syms!`)
take simple-symbol vectors; `:ns/strs`, `:ns/strs!`, and qualified
values under a qualified directive are spec failures (probe29
`qual-*`; unqualified `:keys` still takes idents, `:syms` symbols).
Post-spec destructuring is infallible for `let`/`fn` (every spec-passing
shape destructures; shared code with `loop` keeps the error path).

## E4. `loop` / `letfn` body errors (analyzer, `macroexpanding` wrapper)

`loop` and `letfn` have no specs; their bodies throw directly with the
wrapper naming the operator **as written** (`macroexpanding loop at …`):

| Form | Cause class | Cause message |
|---|---|---|
| `(loop [x] x)` | `IllegalArgumentException` | `loop requires an even number of forms in binding vector` (oracle appends ` in user:LINE`; volatile suffix dropped, span carries location) |
| `(loop x …)` | `IllegalArgumentException` | `loop requires a vector for its binding` (same suffix rule) |
| `(loop [1 2] 1)` | `Exception` | `Unsupported binding form: 1` (binding printed) |
| `(letfn [f] 1)` | `IllegalArgumentException` | `Don't know how to create ISeq from: clojure.lang.Symbol` (class name of the offending value) |
| `(letfn [(f)] 1)` | `ExceptionInfo` | `Call to clojure.core/fn did not conform to spec.` (wrapper: `macroexpanding clojure.core/fn`; innermost macro names the error) |
| `(letfn [(1 [x] x)] …)` | `IllegalArgumentException` | `Bad binding form, expected symbol, got: 1` (wrapper: `compiling letfn*`) |
| `(letfn [(u/f …)] …)` | `RuntimeException` | `Can't let qualified name: u/f` (wrapper: `compiling letfn*`; the pre-seed `let` rejects qualified names before the binding-form check, probe31 `letfn-qual-name`) |
| `(loop [a 1 & [2]] …)` | `RuntimeException` | `Can't use & as a local binding` (wrapper: `compiling loop*`; all-symbol bindings skip destructuring) |
| `(loop [u/x 1] …)` | `RuntimeException` | `Can't let qualified name: u/x` (wrapper: `compiling loop*`) |
| `(loop [[a & b c] …] …)` | `Exception` | `Unsupported binding form, only :as can follow & parameter` |
| `(loop [[a &] …] …)` | `Exception` | `Unsupported binding form: ` (nil rest prints empty) |
| bare `(loop)` / `(letfn)` / `(binding)` | `ArityException` | `Wrong number of args (0) passed to: clojure.core/NAME` — no wrapper at all (the macro fn itself rejects 0 args before any body runs, probe25 `loop-bare`/`letfn-bare`/`binding-bare`) |

`(loop [[a] [1]] a)` → `1` (loop destructures like `let`).
`(loop [[a & b :as m] [1 2]] …)` → `a=1 b=(2) m=[1 2]` (rest + `:as`).

## E5. `fn*` errors (analyzer, `compiling fn*` wrapper)

Multi-clause validation failures name `fn*`:

| Form | Cause class | Cause message |
|---|---|---|
| dup arity | `RuntimeException` | `Can't have 2 overloads with same arity` |
| two variadic overloads | `RuntimeException` | `Can't have more than 1 variadic overload` (probe25 `fn-two-variadic`) |
| fixed > variadic-fixed (any order) | `RuntimeException` | `Can't have fixed arity function with more params than variadic function` |
| 21+ params | `RuntimeException` | `Can't specify more than 20 params` (20 OK) |

`([x] …)` + `([x & r] …)` is legal (`((fn ([x] x) ([x & r] r)) 1 2)` → `(2)`).
`([& r] …)` alone is legal. Pre/post: pre conditions become leading
`(assert c)` … in the body (`(fn* ([x] (assert (pos? x)) (inc x)))`);
post wraps the body: `(let [% body] (assert c) %)`; combined pre+post is
`(do pre-asserts… (let [% body] post-asserts… %))`.
Failure: `AssertionError | Assert failed: (cond-form)` (form printed).

Var-arg rest with no args binds `nil` (probe21 `variadic0 [1 nil]`).

## E6. `try`/`catch`/`finally` shapes (analyzer, `compiling try` wrapper)

| Form | Cause class | Cause message |
|---|---|---|
| `(catch)` (no class) | `IllegalArgumentException` | `Unable to resolve classname: null` |
| `(catch String)` (no binding) | `IllegalArgumentException` | `Bad binding form, expected symbol, got: null` |
| `(catch String "s" …)` | `IllegalArgumentException` | `Bad binding form, expected symbol, got: s` |
| `(catch Exception q/e …)` | `RuntimeException` | `Can't bind qualified name:q/e` (no space) |
| `(catch Nope …)` | `IllegalArgumentException` | `Unable to resolve classname: Nope` |
| `(catch q/Nope …)` | `IllegalArgumentException` | `Unable to resolve classname: q/Nope` |
| clause after `finally` | `RuntimeException` | `finally clause must be last in try expression` |
| non-clause after `catch` | `RuntimeException` | `Only catch or finally clause can follow catch in try expression` |

`(try 1 (catch Exception e))` → `1` (empty catch body is legal);
`(try)` → `nil`; `(try 1 2 3)` → `3`; `(try 1 (finally))` → `1`.
A `finally` that throws masks the in-flight exception (body or
handler path), propagating its own throw instead (probe29
`finally-masks-*`).

Catch classes resolve against the default import table (`java.lang.*`,
`clojure.lang.*` short names); anything else is `Unable to resolve
classname: X`. Real-but-unthrowable classes (`String`, `Object`,
`Number`, …) resolve and never match (**divergence**: the JVM emits
the handler and dies with `VerifyError | Catch type is not a subclass
of Throwable …` at class load, probe25 `catch-string`; the port has no
bytecode verifier, so the handler is dead code). Catch matching is by
class hierarchy (S4 builtin hierarchy, see E9). A bare `(catch …)`
outside `try` is an invoke of the unresolvable symbol `catch` (E7).

## E7. Resolve + invocation errors

- Operator unresolvable: wrapper with **empty** sym
  (`Syntax error compiling at (…)`), cause
  `RuntimeException | Unable to resolve symbol: X in this context`.
  Same when the whole form is the bare symbol (span = form).
- Qualified operator/value, unknown ns:
  `RuntimeException | No such namespace: foo`.
- Qualified operator/value, unknown name in known ns:
  `RuntimeException | No such var: user/nope-1`.
- Static nil operator: `IllegalArgumentException | Can't call nil, form: F`
  (F = whole form printed, e.g. `(nil 1)`).
- Runtime nil operator (`(let [f nil] (f 1))`):
  `NullPointerException | Cannot invoke nil as a function`
  (stable text; JVM NPE text varies by JDK, documented).
- `(throw 42)`: `ClassCastException` Long→Throwable (stable CCE text, E8).
  `(throw nil)`: `NullPointerException | Cannot throw exception because
  "null" is null` (Clojure-constructed, stable — not JDK NPE text,
  probe29 `throw-nil`).
- `(throw (ex-info …))` + catch: `ExceptionInfo` values carry message+data
  (probe22 roundtrip).

`IFn` invoke table (probe24; N = true count passed, `> 20` past 20):

| Callee | Arities | Miss | Wrong arity |
|---|---|---|---|
| closure / native fn | per-arities (+ rest) | — | `ArityException | Wrong number of args (N) passed to: NAME` (NAME volatile for anonymous fns, E10) |
| keyword | 1–2 (lookup + not-found) | `nil` (any coll incl. numbers/`nil`) | name is the printed keyword (`:k`), true count |
| symbol | 1–2 (lookup + not-found) | `nil` | name is `clojure.lang.Symbol`, true count |
| vector | exactly 1 (`nth`; OOB → `IndexOutOfBoundsException` with empty message; non-`Int` key → `IllegalArgumentException | Key must be integer`) | IOOBE | name is `clojure.lang.PersistentVector`, true count |
| map | 1–2 (lookup + not-found) | `nil` | name is `clojure.lang.PersistentArrayMap` / `clojure.lang.PersistentHashMap` (size rule, E8), true count |
| set | 1–2 (member + notFound) | `nil` | name is `clojure.lang.PersistentHashSet`, true count |
| var | deref, then invoke root | — | unbound → `IllegalStateException | Attempting to call unbound fn: #'ns/name`; non-fn root → CCE root→`IFn` |
| unbound object | — | — | `IllegalStateException | Attempting to call unbound fn: #'ns/name` (any arity) |
| anything else | — | — | `ClassCastException` X→`clojure.lang.IFn` (stable, E8) |

## E8. Stable texts for JVM-volatile messages (documented divergences)

These JVM texts vary by JDK/vendor (modules, helpful-NPE, identity hashes)
so the port pins stable equivalents:

- CCE: `ClassCastException | Cannot cast <from> to <to>` with JVM class
  names (`java.lang.Long`, `clojure.lang.IFn`, `java.lang.Throwable`,
  `java.lang.Number`, `clojure.lang.Var$Unbound`, …). Structure names are
  size-dependent like the JVM's (`AFn.name = getClass().getName()`):
  empty list → `clojure.lang.PersistentList$EmptyList`, else
  `clojure.lang.PersistentList`; map with ≤8 pairs, or ≤64 pairs whose
  keys past the first 8 are all keywords, →
  `clojure.lang.PersistentArrayMap`, else `clojure.lang.PersistentHashMap`
  (mirrors `canBePAM` exactly — our maps keep insertion order, so keys
  past the first 8 pairs are checked for keyword-ness like the JVM);
  vector →
  `clojure.lang.PersistentVector`, set → `clojure.lang.PersistentHashSet`.
- Runtime nil invoke: `NullPointerException | Cannot invoke nil as a
  function`.
- `(var)`: `NullPointerException | Cannot read field "ns" because "sym"
  is null` (= modern-JDK text, pinned stable).
- `ExceptionInfo` causes (quote/spec): data is nil (no spec-problems
  payload, no quote ex-data).
- `loop requires …`: volatile ` in NS:LINE` suffix dropped.
- Anonymous fn names / `#object[…]` hexes: volatile shape, E10.

## E9. Values new in S4 (extends `values-contract.md`)

- **Closure**: captures environment; prints
  `#object[user$eval7$fn__8 0x<hex> "user$eval7$fn__8@<hex>"]`
  (munged name, address hex; shape-faithful, values volatile).
- **NativeFn** (test/builtin callables): prints like a closure under its
  given name.
- **Unbound**: evaluating an unbound var yields the `Unbound` sentinel
  object; `str` → `Unbound: #'user/x`; prints
  `#object[clojure.lang.Var$Unbound 0x<hex> "Unbound: #'user/x"]`;
  numeric use → CCE `clojure.lang.Var$Unbound`→`java.lang.Number`;
  `(= ub ub)` → `true`.
- **Var**: `(def d 42)` returns the Var; `pr` → `#'user/d`;
  `(:private (meta (def ^:private p 1)))` → `true`; auto `:line` in meta.
- **Throwable**: `ExceptionInfo` (message+data+cause), `RuntimeException`,
  `IllegalArgumentException`, `IllegalStateException`,
  `UnsupportedOperationException`, `AssertionError`, `ArityException`,
  `NullPointerException`, `ClassCastException`,
  `IndexOutOfBoundsException`, `ArithmeticException`, plain `Exception`;
  catch matching uses this closed hierarchy.
- **Recur**: `recur` in tail position loops with re-bound args; `(recur)`
  with no values outside a recursion point is E2 (tail-position error).
  Each iteration gets a fresh frame: closures from iteration N keep
  observing iteration N's bindings after the rebind (probe27
  `recur-alias-loop`/`recur-alias-fn` → `2`).

## E12. Destructure value semantics (probes 21/23/25/26/27/28)

Mirrors `destvec*`/`destmap*` (`core.clj`), including the 1.13 directives.

- Vector without `&`: positional `(nth v n nil)`; missing → `nil`
  (`(let [[a b] [1]] [a b])` → `[1 nil]`); strings yield chars
  (`(let [[a b] "xy"] …)` → `[\x \y]`); wrong-type target (number, map,
  closure, …) → `UnsupportedOperationException | nth not supported on
  this type: SIMPLE` (SIMPLE = `getSimpleName` rule: full name for
  generated classes, e.g. `user$eval447$vec__444__448` with volatile
  ids, probe25 `closure-nth-simple`); `nil` → `nil`s.
- Vector with `&`: fixed via `first`/`next` over `(seq v)`; `& rest`
  binds the remainder seq (`(let [[a & r] [1 2 3]] …)` → `a=1 r=(2 3)`);
  exhausted remainder binds `nil` (`(let [[a & r] [1]] …)` → `[1 nil]`,
  `(let [[a & r] {:a 1}] …)` → `[[:a 1] nil]`); `&` first binds the
  whole seq (`(let [[& r] [1 2]] r)` → `(1 2)`); rest itself
  destructures (`[a & [b c]]` → `[1 2 3]`); `:as` binds the whole
  (`[a :as v]` → `[1 [1 2]]`). The seq conversion is `(seq v)`
  (`destvec*` `gseq`, probe26): empty/nil target → `nil` rest,
  strings → char seqs, and non-seqables → `IllegalArgumentException |
  Don't know how to create ISeq from: CLASS` (`(let [[& r] 5] r)`,
  unlike the no-`&` `nth` path).
- Map: `symbol keyword` entries `get` by keyword; `:keys`/`:strs`/`:syms`
  bind by name/string/symbol keys, qualified `:ns/keys`/`:ns/syms`
  (and `!`) qualifying the lookup (`:ns/name`, `ns/name`); `:as`
  binds the whole (post seq-conversion) map; nesting composes; param
  hints ignored.
- Seq targets convert first (`seq-to-map-for-destructuring`): empty →
  `{}`, one element → the element, 2+ → pairs as if by assoc (last wins)
  plus the trailing rule for odd counts, applied LAST so the trailer
  wins duplicates (probe27 `trailing-wins`/`trailing-order`):
  trailing map merges, trailing
  2-vector assocs (other lengths → `IllegalArgumentException | Vector arg
  to map conj must be a pair`), trailing `nil` is a no-op, trailing
  sequential contributors must be map entries (ours never are — 2-vectors
  included — so `ClassCastException` X→`java.util.Map$Entry`), anything
  else → `IllegalArgumentException | Don't know how to create ISeq from:
  CLASS`.
- `:or` maps binding-local-or-key → default expression. Defaults are
  evaluated eagerly, once, before any lookup (even unused ones fire).
  Nested patterns bind once: outlet forcing projects `subs` from the
  single bind (never rebinds), so nested `:or` defaults fire exactly
  once however many parent outlets are requested (probe28
  `or-nested-*`, side-log `[7]` each; nested defaults flow into the
  parent outlet via `subs`).
  A default fires on MISSING keys only — present `nil` stays `nil`.
  Lookup is local-first, then key (`(let [{x :x :or {x 5}} {}] x)` →
  `5`, `(let [{x :x :or {:x 7}} {}] x)` → `7`, probe26). Both a local
  and a key default for one binding →
  `Exception | Multiple :or defaults for same key: K 'local'`
  (compile time, `macroexpanding let`/`loop` as-written wrapper — but
  `fn` params destructure through the emitted `let`, so the wrapper is
  `macroexpanding clojure.core/let`, probe25 `fn-or-multi`).
- 1.13 directives: `:keys!`/`:syms!`/`:strs!` require presence, else
  `IllegalArgumentException | Missing required key: K` at runtime (K
  printed bare); a required key with any `:or` default →
  `Exception | Can't supply default value for required key: K`
  (compile time, probe26; required-ness comes only from the `!`
  suffix — `:req` is not a directive and fails `let`/`fn` spec);
  `:select s` binds `(select-keys (merge dm m subs) sel)`;
  `:all m` binds `(merge dm m subs)`; `:excess e` binds the leftover map
  (`(merge (not-empty (dissoc m sel)) subs)`); `:missing m` starts `nil`
  and assocs `nil` for each missing required key; `&` inside a key list
  adds later keys to `sel` without binding them; `:defaults` is silently
  ignored (its "binding" never exists). With `:select`/`:all`, `:or`
  keys must name bindings, else `IllegalArgumentException | symbol X in
  :or does not refer to a binding`, and keys outside the selection →
  `IllegalArgumentException | keys #{…} appear only in :or` (both
  compile time, as-written wrapper — `clojure.core/let` for `fn`
  params, same emitted-`let` rule). Unknown map directives →
  `Exception | Unsupported map directive: K` (loop-destructure path;
  `let`/`fn` spec rejects them first).
- `get` (shared lookup): map → val/nil; vector with `Int` key → elem
  (`nil`/notFound on OOB), non-`Int` key → `nil`/notFound; string with
  `Int` key → char (`nil`/notFound on OOB); set → member/`nil`; list and
  everything else → `nil`/notFound; `nil` coll → `nil`/notFound. `get`
  never throws on OOB (unlike `nth` and vector-invoke).

## E10. Volatility log (shape-faithful, value-volatile)

- Anonymous fn names `user/eval<N>/fn--<M>` (eval/fn counters vary).
- `#object[…]` hex suffixes (identity hashes).
- `loop requires … in NS:LINE` (dropped, see E4).
- CompilerException ex-data map (not reproduced, see E1).

## E11. S4 scope fence

- Single namespace `user`; `def` creates vars there; no `ns`/`in-ns`/`require`
  (S5). Qualified `user/x` resolves; other namespaces → `No such namespace`.
- No host interop: `new`/`.`/`Class.`/static forms fall through to resolve
  errors (`Unable to resolve symbol: new in this context`, `No such
  namespace: String`, …). JVM sanity: `(new String "xy")` → `"xy"`,
  `(. "xy" toString)` → `"xy"`. S5+ may add interop.
- No macros/`defn`/`apply`/`eval`/`macroexpand` (S5). `defn` non-symbol
  already pins the S5 message: `ExceptionInfo | Call to
  clojure.core/defn did not conform to spec.`
- `set!` requires an established thread-local binding: missing binding →
  `IllegalStateException | Can't change/establish root binding of: X with
  set` whether or not the var is `^:dynamic` (probe20/21). `binding` is
  implemented minimally in S4 (push/pop bindings around a body): every
  init evaluates before any var validates, so an init throw beats
  `IllegalStateException | Can't dynamically bind non-dynamic var: X`
  (probe29 `binding-*`).
- `monitor-enter`/`monitor-exit`: not supported (resolve errors).
