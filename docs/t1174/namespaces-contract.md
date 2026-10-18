# Namespaces contract (S5A, normative)

Resolution, `def`, and evaluation-splitting rules for the
multi-namespace registry. Oracle: probe34. Wrappers are the S4 ones
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
