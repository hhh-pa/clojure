# T1174 S4 evaluator contract (analyzer + tree-walk evaluator)

Normative error texts, spans, and value semantics for `clj-eval`, pinned
against JVM oracle probes 20-23 (`probes/probe20.clj` … `probe23.clj`).
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
| `(var :k)` | `ClassCastException` | stable CCE Keyword→Symbol (E8) |
| `(recur)` top level | `UnsupportedOperationException` | `Can only recur from tail position` (**divergence**: the JVM hangs in an infinite self-jump here — verified 2× by marker isolation, probe23 notes it; the port rejects like every other enclosure-less `recur`) |
| `(recur 1)` top level | `IllegalArgumentException` | `Mismatched argument count to recur, expected: 0 args, got: 1` |
| non-tail `(recur …)` | `UnsupportedOperationException` | `Can only recur from tail position` |
| `(recur …)` across `try` | `UnsupportedOperationException` | `Cannot recur across try` |
| arity mismatch | `IllegalArgumentException` | `Mismatched argument count to recur, expected: E args, got: G` |

Check order (mirrors `RecurExpr.Parser`, probe23 order pins): tail/enclosure
→ across-`try` → arity. So `(loop [a] (try (recur 1 2) …))` reports
across-`try`, and non-tail `(recur 1 2)` reports tail position, never the
mismatch.

`set!` analyzes its target first, then requires an assignable: a list
target such as `(set! (x) 2)` fails during analysis
(`Unable to resolve symbol: x in this context`), while an analyzed
non-assignable (`1`) is `Invalid assignment target`.

`(do)` → `nil`; `(do 1 2 3)` → `3`. `do` has no shape errors.

`(def user/q 1)` (qualified, current ns) works and returns `#'user/q`.

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
follow the rest) | map (keys `:keys`/`:strs`/`:syms` vectors of symbols,
`:or` a map, `:as` a symbol, other entries `symbol keyword`).
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
| `(loop [a 1 & [2]] …)` | `RuntimeException` | `Can't use & as a local binding` (wrapper: `compiling loop*`; all-symbol bindings skip destructuring) |
| `(loop [u/x 1] …)` | `RuntimeException` | `Can't let qualified name: u/x` (wrapper: `compiling loop*`) |
| `(loop [[a & b c] …] …)` | `Exception` | `Unsupported binding form, only :as can follow & parameter` |
| `(loop [[a &] …] …)` | `Exception` | `Unsupported binding form: ` (nil rest prints empty) |

`(loop [[a] [1]] a)` → `1` (loop destructures like `let`).
`(loop [[a & b :as m] [1 2]] …)` → `a=1 b=(2) m=[1 2]` (rest + `:as`).

## E5. `fn*` errors (analyzer, `compiling fn*` wrapper)

Multi-clause validation failures name `fn*`:

| Form | Cause class | Cause message |
|---|---|---|
| dup arity | `RuntimeException` | `Can't have 2 overloads with same arity` |
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

Catch classes resolve against the default import table (`java.lang.*`,
`clojure.lang.*` short names); anything else is `Unable to resolve
classname: X`. Catch matching is by class hierarchy (S4 builtin hierarchy,
see E9). A bare `(catch …)` outside `try` is an invoke of the unresolvable
symbol `catch` (E7).

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
  (mirrors `canBePAM`; order beyond insertion is unavailable to our map,
  so the 9–64 mixed-key band uses all-keys-keyword, documented); vector →
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

## E12. Destructure value semantics (probes 21/23)

- Vector: sequential access; missing elements bind `nil`
  (`(let [[a b] [1]] [a b])` → `[1 nil]`); `& rest` binds the seq remainder
  (`(let [[a & r] [1 2 3]] …)` → `a=1 r=(2 3)`); `&` first binds the whole
  seq (`(let [[& r] [1 2]] r)` → `(1 2)`); rest itself destructures
  (`[a & [b c]]` → `[1 2 3]`); `:as` binds the whole
  (`[a :as v]` → `[1 [1 2]]`).
- Map: `symbol keyword` entries look up by keyword; `:keys`/`:strs`/`:syms`
  bind by name/string/symbol keys; `:as` binds the whole map; `:or`
  supplies defaults on MISSING keys only — a present `nil` stays `nil`
  (`(let [{x :x :or {x 5}} {:x nil}] x)` → `nil`); `:or` applies to
  `:keys` bindings too (`(let [{:keys [a] :or {a 1}} {}] a)` → `1`).
- Nesting composes (`[1 2 3]`, `[1 9]` in probe21); param hints
  (`[^String x]`) are accepted and ignored.
- `:or` defaults are evaluated expressions, not literals
  (`(let [{x :x :or {x (+ 1 2)}} {}] x)` → `3`).

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
  implemented minimally in S4 (push/pop bindings around a body).
- `monitor-enter`/`monitor-exit`: not supported (resolve errors).
