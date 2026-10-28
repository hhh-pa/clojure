# S6 data readers (zero-oos slice 6)

Seven upstream vars wiring the data-reader tag table through
the reader: `*data-readers*` `*default-data-reader-fn*`
`default-data-readers` `reader-conditional`
`reader-conditional?` `tagged-literal` `tagged-literal?`.

Target universe rows come from the landed 692-row inventory
(`rust/core-coverage.md`, via PR #41); this slice flips its 7
rows to `implemented` in place (header counts = actual).

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

Read path (two-phase: the consult runs read-side with
span, invocation stays eval-side — `clj-reader` cannot
call fns):

1. Eval read entry points (`read-string`, `read`,
   `read+string`, `read_forms` — hence load and the REPL)
   enable defer mode in `ReaderOpts`, snapshotting the
   live table keys (or the borked-table class) plus the
   default-fn flag per form. `read+string` shares `read`'s
   `run_read` consult, so its S6 cell rides along with no
   separate pin: same precedence, and suppression skips
   resolve there too.
2. The ctor path consults read-side: dotted record tags
   keep the record deferral (not S6); a borked table
   fails the `ILookup` cast; table hits defer to
   `TaggedLiteral` for eval-side invocation (overrides
   skip default validation); `inst`/`uuid` string
   payloads validate inline (valid ⇒ the final
   `TaggedLiteral`; invalid ⇒ the spanful validator
   error); non-string defaults defer for the eval-side
   type dispatch; the default-fn flag defers the rest;
   else the `UnknownTag` throw lands exactly where the
   non-defer path throws. Tags read under `#_` are
   additionally logged as fired (the JVM invokes
   readers under `#_` — probes `e-discard-fires`,
   `e-discard-order` — but never under suppression or
   preservation, probes `g-allow-nomatch-instbad`,
   `g-preserve-instbad`).
3. Eval drains fired-discards in read order (before each
   form, plus at end — probe `e-multiform-order`,
   including ahead of EOF and read errors), then
   post-walks each form bottom-up, skipping
   `ReaderConditional` subtrees and never re-descending
   into fn results (probe `e-no-redescent`). Each tag
   resolves table → defaults → default-fn → throw
   (probes `e-*`, `f-*`), with `RT.get` table semantics
   (nil table, nil entry, and vector tables miss;
   anything else non-map fails the cast).
4. `read` honors no `:readers`/`:default` opts (probes
   `g-opts-*`): the vars alone feed the table.
5. Errors follow the reader (probes `h-*`): `read-string`
   reports bare (no wrap, nil ex-data); `read` wraps in
   `ReaderException` with line/column ex-data (the
   port's readers are line-numbering), recursive reads
   rethrow raw, and fn-failure causes keep their data.

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

A `^meta` directly on a tagged form errors `MetaTarget`
(the reader attaches before the walk resolves; the JVM
attaches to the fn result — probe `e-meta-vec`).

All natives carry the house arity guard. Constructor
guards, precedence, and every error text replay the
192 oracle pins (`probes/*.out`).

## BDD scenarios

- Given a table-bound tag, reads invoke its fn once per
  occurrence — nested, quoted, and cond-matched alike —
  with the read form.
- Given the table → defaults → default-fn → throw
  precedence, each layer wins in order (including the
  `inst` override skipping validation).
- Given an unknown tag with no fallback, reads throw
  the oracle's error verbatim (`read-string` bare with
  nil ex-data, `read` wrapped with line/column).
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
