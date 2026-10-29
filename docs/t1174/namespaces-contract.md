# Namespaces contract (S5A, normative)

Resolution, `def`, evaluation-splitting, namespace vars, and loader
rules for the multi-namespace registry. Oracles: probe34 (registry),
probe35 (vars), probe36 (loader). Wrappers are the S4 ones
(`Syntax error compiling … at …`); only the rules below are new.

## Resolution

- Qualified `a/b`: `a` names an alias or a namespace — aliases win
  over real namespaces (probe34 `ns-alias-shadow`). Neither →
  `RuntimeException | No such namespace: a`. The target holds no
  `b` → `RuntimeException | No such var: a/b`, blaming the symbol
  as written (alias preserved, probe34 `ns-alias-novar`).
- A dangling alias (target removed) still counts as a namespace:
  `tt/x` → `No such var: tt/x`, while the bare removed name gives
  `No such namespace` (probe34 `ns-alias-removed`,
  `ns-removed-direct`).
- Unqualified: current namespace, then refers (probe34
  `ns-refer-ok`); miss → `Unable to resolve symbol: x in this
  context` (unchanged).
- `var` sees refers and other namespaces (probe34 `ns-var-refer`,
  `ns-var-cross`); every miss stays the single `Unable to resolve
  var: x in this context` shape.
- Var ids are registry-global and never reused, so `Var`/`Unbound`
  identity and thread bindings stay sound across namespaces.

## `def`

`def` always defines in the current namespace: unqualified and
same-namespace symbols create-or-set; other-namespace symbols fail —
`Can't create defs outside of current ns` when the var exists,
`Can't refer to qualified var that doesn't exist` when it doesn't
(probe34 `ns-intern-def`, `ns-crossdef-missing`). `(declare foo/x)`
fails the same way (it expands to `def`, probe34 `ns-declare-q`).

## Top-level `do` splitting (S5A2/S5C)

`Compiler.eval` (Compiler.java, `eval` method) macroexpands a
top-level form and, when `do`-headed, evals each element as its own
top-level form in turn. Consequences (probe34 `ns-eval-split-*`,
`ns-do-*`): runtime definitions (`intern`, `require`, `defmacro`)
take effect before later elements analyze; nested `do` bodies stay
single analysis units. The port's `eval`/`load` paths must replicate
the split (S5C extends it past macroexpansion).

## Fences

- Stale refers (into a removed namespace) stop resolving; the JVM
  keeps the var object alive. Unobservable without `remove-ns` +
  refer interplay — pathological, fenced.
- `intern` never creates namespaces: missing ns →
  `Exception | No namespace: X found` at runtime (probe34
  `ns-intern-creates`).

## Namespace values and vars (S5A2, probe35)

- Namespaces print `#object[clojure.lang.Namespace 0x{id}
  "{name}"]` with the registry creation id in hex (deterministic,
  like the S4 `#object` ids); `str` is the bare name (probe35
  `ns-print`, `create-ns-fresh`). Equality and hashing are by id
  (JVM identity: a removed-then-recreated namespace is a new
  value).
- `all-ns` returns every live namespace. The JVM order is
  hash-unspecified; the port sorts by name (deterministic,
  probe35 `all-ns-shape`).
- Argument types: `create-ns`/`in-ns`/`find-ns`/`remove-ns` take a
  Symbol (anything else is the §E8 stable cast `Cannot cast X to
  clojure.lang.Symbol`); `the-ns`/`ns-name`/the `ns-*`
  family/`intern`/`ns-resolve`/`ns-unmap`/`ns-unalias` take a
  Symbol or a namespace; `alias` takes a Symbol name and a
  Symbol-or-namespace target; `resolve`'s symbol is strict
  (probe35 `create-ns-string`, `in-ns-ns-arg`, `find-ns-ns-arg`,
  `remove-ns-ns-arg`, `the-ns-string`, `resolve-ns-arg`,
  `alias-ns-target`).
- `create-ns` returns the existing-or-fresh namespace; `in-ns`
  creates-or-finds, makes it current, and returns it. Neither
  refers `clojure.core` (probe35 `create-vs-in-ns-refers`).
- Missing-namespace reads throw `Exception | No namespace: {sym}
  found`: `the-ns`, `ns-name` on a symbol, the `ns-*` family,
  `intern`, `ns-resolve` (probe35 `the-ns-missing`,
  `ns-name-missing`, `ns-map-missing`, `intern-missing-ns`,
  `ns-resolve-missing-ns`).
- `find-ns` on a missing name returns nil; on nil it is the
  modern-JDK `NullPointerException | Cannot invoke
  "Object.hashCode()" because "key" is null` (S4 NPE convention,
  probe35 `find-ns-nil`).
- `remove-ns` returns the removed namespace or nil; a removed
  handle keeps its name for `ns-name` (probe35 `ns-name-removed`),
  and removing the current namespace leaves `*ns*` dangling
  (probe35 `remove-ns-current`). Removing `clojure.core` is
  `IllegalArgumentException | Cannot remove clojure namespace`
  (probe36 `remove-ns-core`). Removed namespaces survive as
  tombstones (S5A2 supersedes S5A1's drop): stale handles read the
  frozen mappings and captured vars keep their roots — only
  resolution treats the name as gone.
- `in-ns` is a thread-local `set` (source: `RT.java`
  `inNamespace`): it writes the top `*ns*` frame when bound (boot
  pushes a permanent base frame, so the `*ns*` root never moves),
  else the root — and always the registry current. `ns-resolve`
  sees aliases for qualified symbols (probe36
  `ns-resolve-alias`). `intern` adopts meta from the name symbol
  (source: core.clj `intern`).
- `alias` installs into the current namespace and returns nil.
  Re-aliasing to a different target is `IllegalStateException |
  Alias {a} already exists in namespace {ns}, aliasing {old}`;
  the same target is a silent no-op (probe35 `alias-overwrite`,
  `alias-same-twice`); a missing target is the `No namespace:`
  error (probe35 `alias-bad-target`). Aliases shadow real
  namespaces for lookup while `find-ns` still sees the real one
  (probe35 `alias-shadow`, §Resolution).
- `ns-map` is interns plus refers plus imports keyed by
  unqualified symbol; aliases are excluded and interns win over
  refers on collision (probe35 `ns-map-fx`). The JVM also prints
  a `REJECTED: … you must ns-unmap first` warning when `refer`
  lands on an intern; the port omits the warning until `*err*`
  printing lands in S5D (probe35 `refer-shadow`).
- `ns-publics` excludes `^:private` vars (probe35
  `ns-publics-fx`). The port orders the family maps by symbol
  (deterministic; JVM hash order unspecified).
- `resolve`/`ns-resolve` return the Var or nil on a miss (probe35
  `resolve-miss`, `ns-resolve-ns-arg`). `resolve` also sees
  imports on the JVM (`(resolve 'String)` is the class); the port
  has no classes, so that miss is nil (N1 divergence, probe35
  `resolve-class`).
- `intern` returns the var (the same object across overwrites —
  3-arity resets the root, probe35 `intern-overwrite`); 2-arity
  leaves it unbound.
- `refer`/`refer-clojure` return nil and are idempotent (probe35
  `refer-twice`); `refer-clojure` adds every core public minus
  `:exclude` (probe35 `refer-clojure-exclude`). `declare` is the
  same internals behind its S5C macro (probe35
  `declare-visible`). `refer` errors: missing lib `Exception | No
  namespace: {sym}` (no `found` suffix); private var
  `IllegalAccessError | {sym} is not public`; absent var
  `IllegalAccessError | {sym} does not exist`; non-sequential
  `:only`/`:refer` `Exception | :only/:refer value must be a
  sequential collection of symbols` (probe36 `refer-missing-lib`,
  `refer-private`, `refer-missing-var`, `req-refer-scalar`). A
  non-map `:rename` is invoked as a function, with nil falling
  back to the plain name (probe36 `refer-rename-scalar`); a scalar
  `:exclude` is the ISeq coercion error (probe36
  `refer-exclude-scalar`).
- `ns-unmap`/`ns-unalias` return nil and tolerate absence
  (probe35 `ns-unmap-effect`, `ns-unalias-effect`,
  `ns-unmap-sym-ns`, `ns-unalias-sym-ns`).
- `ns-imports` is always `{}` and `ns-map` carries no imports
  (N1: no host classes; `import` out of scope).
- `binding [*ns* …]` does not redirect analysis or installation:
  resolution, `refer`, `alias`, and `def` read the registry
  current only, until S5B syncs the two (fence).
- `alias`/`refer` into a removed current namespace recreate
  it (the JVM installs into the stale object — unprobed corner,
  fenced). `def`/`declare`/`defmacro` install into the tombstone
  instead (T73 finding 4, like the JVM's detached `*ns*`).
- `eval`-native error spans use the form's reader meta or 1:1
  (natives receive no caller span — fenced).

## Loader (S5A2, probe36)

- `require`/`use`/`load` return nil; `loaded-libs` is a set of
  symbols. The JVM set is sorted (`sorted?` true); the port has
  no sorted sets, so it prints in load order and `sorted?` is
  false (divergence, probe36 `libs-initial`,
  `loaded-libs-shape`). `*loaded-libs*` holds the set directly
  (the JVM wraps it in a Ref; the single-threaded port conjs the
  root instead).
- Membership — not namespace existence — decides the load:
  `require` loads iff the lib is absent from `loaded-libs`, then
  applies `:as`/`:refer` even on skip (probe36 `alias-on-skip`,
  `req-mem-fixture`, `mem-survives`). A failed load marks nothing
  and destroys nothing (probe36 `req-missing-clean`).
- The `ns` form marks its lib loaded (probe36 `ns-marks-loaded`,
  macro in S5C); `load` alone never marks (probe36
  `load-no-nsform`).
- Libspec grammar: a symbol, `[lib & opts]`, or prefix lists
  `(prefix & args)`; `:as` plus `:refer` (a vector or `:all`)
  (probe36 `req-prefix`, `req-as-refer`, `req-refer-all`). `use`
  is require-plus-refer with `:only`/`:exclude`/`:rename`
  (probe36 `use-only`, `use-exclude`, `use-rename`, `use-bare`)
  and honors `:as` too (probe36 `use-with-as`). A libspec is a
  symbol or a vector whose second item is nil or a keyword —
  lists and strings take the prefix path (source: core.clj
  `libspec?`; probe36 `req-string`). `require` is `load-libs`
  with `:require`, `use` with `:require :use` (source). Top-level
  `:as` is a supported flag, so `(require 'lib :as)` aliases
  `true` and fails the symbol cast down the general path (probe36
  `req-opts-bare`).
- `:as-alias` creates the empty namespace without loading,
  aliases it, and never marks the lib (probe36 `as-alias-opt`).
  A dotted lib under a prefix is `Exception | Found lib name
  '{lib}' containing period with prefix '{prefix}'.  lib names
  inside prefix lists must not contain periods` (two spaces); a
  nil prefix is `Exception | prefix cannot be nil` (probe36
  `req-prefix-period`, `req-nil-prefix`).
- Flags: `:reload` forces the load; `:reload-all` forces the load
  plus transitive embedded deps (the embedded set is flat, so it
  behaves as `:reload`); `:verbose` echoes each action —
  `(clojure.core/in-ns '…)` and `(clojure.core/alias …)` on skip,
  `(clojure.core/load "…")` on load (probe36 `verbose-*`). The
  echo is suppressed until `*out*` printing lands in S5D.
- Missing file: `java.io.FileNotFoundException | Could not locate
  {root}__init.class, {root}.clj or {root}.cljc on classpath.`
  plus ` Please check that namespaces with dashes use underscores
  in the Clojure file name.` iff the root contains `_` (probe36
  `req-missing`, `req-dash-map`, `req-underscore-note`). Dashes
  map to underscores, dots to slashes.
- `load` roots: a leading `/` is absolute, otherwise relative to
  the current namespace's namespace-part (`aa.bb` + `"y"` →
  `aa/y`; a one-segment namespace leaves the root bare, probe36
  `load-rel-user`, `load-rel-nested`). Extensions never strip:
  `"x.clj"` tries `x.clj.clj` (probe36 `load-ext`).
- Post-load checks: `:as`/`:refer` against a missing namespace is
  `Exception | namespace '{lib}' not found`, or `… not found after
  loading '{root}'` when a load just ran (probe36 `req-stale`,
  `req-mismatch-as`, `req-mismatch-stale`); a bare-symbol require
  never checks (probe36 `req-mismatch-plain`).
- `(require)` is `Exception | Nothing specified to load`;
  unknown flags are `Exception | Unsupported option(s) supplied:
  {kw}` (`, `-interposed past the first, source: core.clj
  `load-libs`; probe36 `req-empty`, `req-unsupported`). The
  `throw-if` sites wrap in `CompilerException` directly (source);
  the port surfaces the inner cause (S4 E1: runtime failures have
  no wrapper), so catching `CompilerException` itself is
  unsupported (fence). `throw-if` is a function: `load-lib` names
  the lib eagerly, and non-`Named` libs fail the `Named` cast (or
  the nil `getName` NPE) before any check (probe36 `req-num-lib`,
  `req-nil-lib`).
- `load` guards re-entry with `*pending-paths*`: a repeat path is
  `Exception | Cyclic load dependency: {chain}` (`->`-interposed,
  repeats bracketed `[ p ]`); loading the file already on top is
  exempt (probe36 `cyclic-msg`).
- `load` binds `*file*` to the path as opened; `load-file` to the
  absolute path (`..` preserved, probe36 `file-dotdot`);
  `load-string` binds `*file*` nil (probe36 `file-loadstring`).
  A missing `load-file` is `java.io.FileNotFoundException |
  {path} ({strerror})` with the path as given (probe36
  `loadfile-missing`). `load` arguments cast to String.
- `*load-path*` searches each directory for `.clj` first, then
  `.cljc` (source: `RT.java` lookup order); embedded libs win
  over files.
- Each libspec must be a symbol or sequential, else the ISeq
  coercion error (`(require "x")` → `IllegalArgumentException |
  Don't know how to create ISeq from: java.lang.Character`,
  probe36 `req-string`).
- The port resolves libs from the embedded set first, then
  `*load-path*` files (matrix note).
