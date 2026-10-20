# S8 oracle pins: LLVM AOT subset (T1174)

Primary oracle: our interpreter (`run`/`eval` — spec §2.5 S8
is differential vs the interpreter). JVM cross-checks apply
where subset semantics overlap. Probe inputs: ad-hoc `eval`
below (verified this slice). Port deviations are explicit.

## P1. Compilable surface (spec 2.4, made exact)

- Top-level `defn`, single vector clause, fixed arity, no
  `&`, no prepost map: `(defn f [n] (if (= n 0) 0
  (f (+ n -1))))` compiles; calls to sibling `defn`s (incl.
  direct self-calls) are plain calls. Optional docstring /
  attr-map accepted-and-ignored (no runtime effect
  in-subset — verified: expansion metadata never feeds
  subset values).
- Bodies/statements: literals (Int/Double/Bool/Str/Vector
  of subset), `if`/`do`/`let` (symbol bindings only, no
  destructuring), closed-set calls (`+ - * quot < <= >
  >= = count nth get conj`, unqualified or
  `clojure.core/`-qualified — verified
  `(clojure.core/+ 1 2)` → `3`), `print` intrinsic.
- `main` = non-`defn` subset tops in load order, values
  discarded (mirrors `run`'s no-echo rule: `run` of the
  `defn`+`print` probe prints only `0`, never `#'user/f`).
- Forward references fail in the analyze pass exactly like
  the interpreter (compile in order, no pre-declare).

## P2. Named rejections (each still exits 0 under `run`)

`build` exits 1 naming the FIRST unsupported form in source
order; every file below exits 0 under `run`/`eval`:

- `(def x 41)` → names `def` (vars out).
- `(loop [i 0] ...)` → names `loop` (loop-free subset).
- `(recur ...)` (in `fn` tail) → names `recur` (no TCO
  machinery; direct self-CALLS are the compilable form).
- `(try ...)` / `(throw ...)` → names `try`/`throw`.
- `(var print)` → names `var`; `set!` → names `set!`.
- `(future 7)`, `(delay 1)`, `(atom 1)` → names the
  operator (concurrency out).
- `(lazy-seq ...)` → names `lazy-seq` (laziness out).
- `(println :x)` → names `println` (only `print` in).
- `(ns foo)` → names `ns`.
- `(when true ...)` → names `when` (non-`defn` macros
  out — NO macroexpansion in AOT; verified `(when true
  (print :w))` prints `:w` under the interpreter).
- `(defmacro m [x] x)` → names `defmacro`.
- `((fn [x] x) 7)` → names `fn` (only `defn` tops).
- `(quote 5)` → names `quote` (outside value flow).
- Multi-clause / variadic / prepost `defn` → names `defn`
  with a hint (verified all three run: `2`, and `#'user/`
  echoes with exit 0).
- `(let [[a b] [1 2]] ...)` → names `let` with a
  destructuring hint (verified → `3`).

## P3. Error shapes + exit codes

- `(if)` → analyzer diagnostic through `build` with direct
  file spans: `f:1:1: Syntax error compiling if: Too few
  arguments to if` (verified shape via `eval`; BETTER than
  `run`'s load-wrapper 1:1 synthesis — pinned improvement).
- Reader errors reuse reader spans/messages, exit 1.
- Runtime arity is RUNTIME: `(-)` → exit 1
  `clojure.lang.ArityException: Wrong number of args (0)
  passed to: clojure.core/-` (verified) — AOT reproduces
  via the same native, never at compile time.
- `quot`/`nth`/`count` edges (verified): `(quot 1 0)` →
  `java.lang.ArithmeticException: / by zero`; `(nth [] 5)`
  → `java.lang.IndexOutOfBoundsException: ` (empty msg);
  `(count 1)` → `java.lang.UnsupportedOperationException:
  count not supported on this type: Long`.
- Built binaries: normal end → exit 0; helper-detected
  error → `path:line:col: Class: message` on stderr
  (TRUE call-site spans baked at codegen — verified
  superior to `run`'s synthesized 1:1; error-case outputs
  are ours-pinned, not byte-identical to `run`).
- IO before codegen: missing/unreadable input, unwritable
  output, missing runtime archive/linker → exit 3
  `build: ...`, identical with/without the `aot` feature.

## P4. Primitive evaluation rules (codegen must mirror)

- `(< "s")` → `true` unchecked (verified); `(< 2 1 "s")`
  → `false` with NO error (verified early exit before
  coercing `"s"`): n-ary comparisons evaluate args
  left-to-right, coerce/compare pairwise, exit false
  early. AOT delegates to the same `chain_cmp` native.
- `(= 1 1.0)` → `false` (verified same-category).
- `(print 1 2)` → `1 2` with NO trailing newline,
  returns nil (verified): `print` is space-joined
  human-readable, unquoted strings.
- 1-arg `+`/`*` passthrough unchecked; 0-arg `+`→`0`,
  `*`→`1`; 1-arg `-` negates checked. 0-arg `-`,
  comparisons, `=` → arity error at runtime.
- `print` with default (unbound) `*print-*` vars =
  `print_to_string` exactly; in-subset programs cannot
  rebind them (`binding` out), so no observable gap.

## P5. Explicit non-goals (S8 limits)

- Host-only compilation; no cross-compilation, no debug
  info, no TCO guarantee (deep self-calls behave like
  the interpreter: plain calls).
- `*command-line-args*` unconveyed to built binaries
  (no arg surface in S8; document).
- No unboxing fast paths (D1); release archives grow by
  one runtime archive per platform.
