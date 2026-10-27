# S7 read gates (zero-oos slice 7)

Six upstream vars over the reader/load machinery, no new value
kinds: `*read-eval*` `load-reader` `read+string`
`*reader-resolver*` `*suppress-read*` `with-loading-context`.

Target universe rows come from PR #41's 691-row inventory
(unlanded); this slice adds its 6 rows to the wave-1 matrix
at mirrored positions, flipped straight to `implemented`
with test notes. The #41 land unions them later.

## Semantics

- `*read-eval*` — dynamic var, root true (oracle pins). `#=`
  evaluates the following form through an evaluator callback
  when true (new `ReaderOpts` hook; the reader crate stays
  eval-free via an `Arc<dyn>` callback), and throws the JVM
  gate text when false. Both arms oracle-pinned, including
  the `*ns*` context of the eval.
- `*suppress-read*` — dynamic var, root false (oracle pins);
  when true `read_form` consumes and returns nil (oracle
  pins consume-vs-skip and error behavior).
- `*reader-resolver*` — dynamic var, root nil (oracle pins);
  when bound, `::alias/x` and class resolution consult it
  (oracle pins the JVM role first — triage directive).
- `read+string` — native over the `read` stream machinery
  (`reader.rs` `pick_stream`/`check_stream_type`); oracle
  pins arity and the `[form string]` return.
- `load-reader` — native over the `ns.rs` load core
  (`native_load_file` region); oracle pins arg shape and
  return.
- `with-loading-context` — macro matching the JVM expansion
  (probe `macroexpand-1` + arglists); converges with the
  inlined `*pending-paths*` guard, doesn't fork it.

All natives carry the house arity guard; arity misses throw
the house arity text (pinned). Read errors keep the
`ReaderException`-wrap shape of the `read`/`read-string`
natives (probes pin which wrapper each entry point uses).

## BDD scenarios

- Given `(binding [*read-eval* false] (read-string "#=(+ 1 2)"))`,
  the read throws the JVM gate text; with the default true
  root it evaluates to `3`.
- Given `(binding [*suppress-read* true] (read-string "42"))`,
  the read consumes and returns nil.
- Given `(read+string (java.io.PushbackReader. ...))`, the
  call returns `[form string]` with the oracle's string.
- Given `(load-reader rdr)`, the reader's forms load with
  the oracle's arg shape and return.
- Given `with-loading-context` around a cyclic `load`, the
  cycle guard fires; its `macroexpand-1` matches the JVM
  expansion.

## Tests

`tests/zero_oos_s7_readgates.rs`: one test per var replaying
its probe labels verbatim, plus shared arity-miss tests.
Matrix `notes` name the tests.

## Non-goals

Tag-table dispatch (`DataReader` arm — S6), `*data-readers*`
/ `*default-data-reader-fn*` (S6), `read` `:read-cond` /
`:features` changes (landed), print-dup behavior (S8).

## Triage (oracle pins, probes `gates` + `trio`)

- `#=` is NOT full eval: the head symbol resolves in the
  current `*ns*` (namespaces + aliases; specials never
  resolve) and the var applies to the RAW operand forms
  (`c-rawlist`, `c-map`, `c-nested`). Bare symbols go to
  class lookup (the port has no classes: always
  `ClassNotFoundException`, probe `d-class-miss`); `(var s)`
  returns the (created) var; `C.`/static heads need host
  classes (`d-ctor`, `d-static-miss`). Unknown qualified
  heads throw `ClassNotFoundException` of the ns part
  (`c-qualmiss`); unresolvable heads throw `Can't resolve
  {sym}`; anything else throws `Unsupported #= form`.
- `#=` runs during reader-conditional discard (`d-discard`)
  and ignores `*suppress-read*` (`e-readeval`); the gate
  still applies (`e-readeval-false`). Load forces
  `*read-eval*` true, visibly (`l-gate`, `m-*`).
- `*suppress-read*` (root nil) affects only `#tag` reads
  (`e-tag-sup`); plain reads never consult it (`e-basic`,
  `e-consume`). `#inst`/`#uuid` bypass suppression on the
  JVM (`e-inst-sup`) — the port keeps its existing
  suppress-then-tag order until S6 lands default readers
  (S6 handoff, documented divergence).
- `*reader-resolver*` (root nil) is a JVM interface; the
  port binds a 2-arity fn `(f op sym)` with `op` in
  `:current-ns :resolve-alias :resolve-class :resolve-var`
  (nil sym for `:current-ns`), returning a symbol or nil.
  Consult sites mirror the JVM: `::` auto-resolve (the
  return's NAME becomes the keyword ns, `f-alias-2part`)
  and syntax-quote (unqualified: class, then var, else
  current-ns qualify; qualified: class, then alias, else
  as-is; `C.`: class then re-dot; `f-sq-*`). A bound
  non-fn fails every read with the `Resolver` cast text
  (`f-nonresolver*`).
- Wraps: port readers are `LineNumberingPushbackReader`s,
  so `read`/`read+string` wrap `#=` outcomes in
  `ReaderException` (`h-div-lnpr`, `h-gate-lnpr`, `p-gate`)
  with entry span for the gate (`(1,3)`) and post-operand
  span for applications (`(1,10)`); entry checks
  (`:unknown`, resolver cast) stay raw (`h-unknown-lnpr`,
  `h-resolver-lnpr`). `read-string` keeps its landed
  `RuntimeException`-message wrap (exact for
  `RuntimeException`-classed outcomes, same flattening as
  the pre-existing `NumberFormatException` case otherwise).
  Load wraps read-phase outcomes in `Syntax error reading
  source` (`l-err-*`, `m-loadstring-*`) and entry-cast
  failures in `Syntax error executing` (`m-load-resolver`,
  `l-resolver-mid`).
- `read+string` captures the consumed text (leading trivia
  included) and Java-trims it (`p-comment`, `p-tabs`); nil
  streams throw the `getString` NPE before opts parsing
  (`p-nil-badopts`), non-readers fail the hint cast
  (`p-strarg`). `load-reader` returns the last value (nil
  when empty), binds `*file*` nil, isolates `*ns*`, leaves
  `*pending-paths*` alone (`l-*`).
- Check order in `read`: stream cast, resolver cast,
  `:unknown`, opts, nil NPE (`g-*`, `h-order*`).
- Known port divergences (out of fence): load reads all
  forms before evaluating (the JVM interleaves:
  `l-err-read-defined`, `m-loadstring-trunc-defined`),
  `#inst` under suppression, non-`RuntimeException` `#=`
  classes under `read-string`, `:added`/`:doc` metas on
  boot vars, `#=` host heads (classes/ctors/statics).
