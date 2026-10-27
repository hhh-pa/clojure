# S6 data readers (zero-oos slice 6)

Seven upstream vars wiring the data-reader tag table through
the reader: `*data-readers*` `*default-data-reader-fn*`
`default-data-readers` `reader-conditional`
`reader-conditional?` `tagged-literal` `tagged-literal?`.

Target universe rows come from PR #41's 692-row inventory
(unlanded); this slice adds its 7 rows to the wave-1 matrix at
mirrored positions (`### Previously unlisted — no slice yet`),
flipped straight to `implemented` (+7 rows; header counts =
actual at land). The #41 land unions them later.

## Semantics

- `*data-readers*` — dynamic var, root `{}`, `:dynamic`
  meta true (probes `a-roots`, `a-meta`); symbol→fn
  table consulted first for every tag occurrence.
- `*default-data-reader-fn*` — dynamic var, root nil,
  `:dynamic` meta true; `[tag form]` fallback after the
  table and the defaults (probe `f-table-miss`).
- `default-data-readers` — plain (non-dynamic,
  non-private) map var (probe `a-ddr-meta`), keys
  unqualified `inst`/`uuid` symbols (probe
  `a-ddr-keytype`), values the reader vars on the JVM
  (probe `a-ddr-str`) and 1-arity natives in the port.
- `tagged-literal` — 2-arity `[tag form]` constructor
  over `Value::TaggedLiteral`; symbol tags pass, nil
  coerces to `nil` (probe `b-niltag-vs-nilsym`), other
  tags fail the `Symbol` cast (probes `b-ctor-*`).
- `tagged-literal?` — `pred1!` over
  `Value::TaggedLiteral` only (battery `b-p-*`).
- `reader-conditional` — 2-arity `[form splicing?]`
  constructor over `Value::ReaderConditional`; the form
  is unguarded (probes `c-badform*`), splicing takes
  `Boolean` only (nil is the verbatim NPE, probe
  `c-badsplice-nil`).
- `reader-conditional?` — `pred1!` over
  `Value::ReaderConditional` only (battery `c-p-*`).

Read path (two-phase: `clj-reader` cannot invoke fns, so
tags defer read-side and resolve eval-side):

1. Eval read entry points (`read-string`, `read`,
   `read_forms` — hence load and the REPL) enable defer
   mode in `ReaderOpts`: every non-suppressed tag reads
   to `TaggedLiteral`, and tags read under `#_` are
   additionally recorded as fired-discards (the JVM
   invokes readers under `#_` — probes `e-discard-fires`,
   `e-discard-order` — but never under suppression or
   preservation, probes `g-allow-nomatch-instbad`,
   `g-preserve-instbad`).
2. Eval drains fired-discards in read order (before each
   form, plus at end — probe `e-multiform-order`), then
   post-walks each form bottom-up, skipping
   `ReaderConditional` subtrees and never re-descending
   into fn results (probe `e-no-redescent`).
3. Each tag resolves table → defaults → default-fn →
   throw (probes `e-*`, `f-*`): the table lookup is
   `RT.get` semantics (nil table, nil entry, and vector
   tables miss; anything else non-map fails the
   `ILookup` cast — probes `e-nonmap*`, `e-nilentry`,
   `e-vectable`); unqualified `inst`/`uuid` validate
   (below); the default fn runs with `[tag form]`; else
   `RuntimeException "No reader function for tag {tag}"`
   with no ex-data, recursive or not (probes
   `e-unknown-data`, `e-stream-*`).
4. `read` honors no `:readers`/`:default` opts (probes
   `g-opts-*`): the vars alone feed the table.

S6 boundary (no new value kinds): validated `inst`/`uuid`
payloads read back as `TaggedLiteral` — a readable
round-trip whose type the date/time value-kind slice
later upgrades (landing `inst?`/`inst-ms`/`uuid?`). The
validators mirror the JVM messages verbatim
(`d-inst-*`, `d-uuid-*`): the `inst` grammar is
`YYYY(-MM(-DD(THH:MM(:SS(.frac)?)?)?)?)?` with padded
components, uppercase `T`, and an optional `Z`/`[+-]HH:MM`
zone on any prefix; range failures replay the six
`failed: ...` asserts (leap-aware days, leap-second 60
iff minutes is 59). `str` on either kind stays the
port's readable print (probes `b-str`, `c-str` record
the JVM `@hex` `toString`; print dispatch is not S6).

All natives carry the house arity guard. Constructor
guards, precedence, and every error text replay the
178 oracle pins (`probes/*.out`).

## BDD scenarios

- Given a table-bound tag, reads invoke its fn once per
  occurrence — nested, quoted, and cond-matched alike —
  with the read form.
- Given the table → defaults → default-fn → throw
  precedence, each layer wins in order (including the
  `inst` override skipping validation).
- Given an unknown tag with no fallback, reads throw
  `RuntimeException` verbatim with nil ex-data.
- Given invalid `inst`/`uuid` payloads, reads throw the
  oracle's validation error; valid ones round-trip
  readably.
- Given `#_`, readers still fire in read order (throws
  propagate, effects precede the form and the EOF);
  given preserved conditionals, nested tags stay data.
- Given tagged literals in loaded sources and the REPL,
  the same resolution runs (shared `read_forms` hook).
- Given each battery value, both predicates answer the
  oracle's boolean verbatim; ctor guards and arity
  misses throw verbatim.

## Tests

`rust/crates/clj-eval/tests/zero_oos_s6_datareaders.rs`:
one test per var/pin group replaying probe labels
verbatim, plus `clj-reader` unit tests for the defer
branches and the opts threading. Matrix `notes` name
the tests.

## Non-goals

`#=` eval reader (still N5), dotted record tags (record
slice), `data_readers.clj` classpath autoload (the port
has no classpath), date/time and UUID value kinds (the
`inst?`/`inst-ms`/`uuid?` slice upgrades S6's validated
payloads), `read` `:readers`/`:default` opts (the JVM
ignores them — probes `g-opts-*`).
