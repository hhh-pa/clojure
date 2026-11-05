# I214 subset-contract pins (delta over `s8-pins.md`)

#214's subset delta, relocated here so `docs/spec/t1174` stays
byte-frozen (its `loop`/`recur`/`when` rows are superseded by
this file; design rationale lives in
[`spec.md`](spec.md)). Verified by the planner structural
pins, the FFI differentials, and the
[`corpus/`](corpus/MANIFEST.md) replay.

- Bodies/statements gain `loop` (symbol bindings only, no
  destructuring; `recur` only in tail position of a `loop`
  body, one arg per binding).
- Closed-set calls gain `long`, `int`, `transient`,
  `conj!`, `assoc!`, `persistent!` (19 prims, one
  discriminant order; see `spec.md` §2–§3).
- Reject-table updates (forms/hints are the contract):
  - `(loop [[a b] ...] ...)` → names `loop`
    (destructuring out — plain-symbol loops compile,
    corpus `11-loop`).
  - `(recur ...)` outside a `loop` (in `fn` tail) →
    names `recur` (only loop bodies take `recur`;
    direct self-CALLS are the compilable `fn`
    recursion form).
  - `(when ...)` / `(when-not ...)` compile (desugared
    macro-forms, corpus `12-when`); the remaining
    non-`defn` macros stay out.
