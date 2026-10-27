# Zero-oos S8: print system (issue #51) — Specification

- Status: spec + oracle probes, uncommitted, awaiting owner
  review. No source edits, no commits.
- Issue: #51. Branch: `zero-oos-s8-print`.
- Probes: `docs/zero-oos-s8-print/probes/*.{clj,out}` (7 files,
  178 pins, every file exit 0 and rerun-stable; oracle: alpha10
  jars per each file's header).
- Scope: 8 vars — `*flush-on-newline*`,
  `char-escape-string`, `char-name-string`, `print-ctor`,
  `print-dup`, `print-method`, `print-simple`, `*print-dup*`
  (divergent→implemented).

## Problem

Seven of the eight vars don't resolve; `*print-dup*` resolves
but the printer ignores it (`PrintOpts` has no dup flag;
`print_opts` never reads the var). `newline`/`println` never
flush. Char printing uses hardcoded tables with no vars behind
them. There is no `print-method`/`print-dup` dispatch, so user
`defmethod`s on printing are impossible.

## Solution

Boot `print-method`/`print-dup` as real multimethods on the #37
machinery with flattened per-kind defaults; route the six
pr-family natives (`pr`, `prn`, `print`, `println`, `pr-str`,
`print-str`) through an evaluator-level `pr-on` driver that
invokes them (user methods honored, recursively); thread
`dup: bool` through `PrintOpts`/`print_opts`; expose the two
char maps from single-source tables; add `print-simple` /
`print-ctor` as writer fns; boot `*flush-on-newline*` (root
true) with flush on the `ln` path only; retire the `*print-dup*`
divergence pins; flip all 8 matrix rows to `implemented`.

## The central design decision: the `pr-on` driver

The issue leaves one decision to the spec: how user `defmethod`s
on `print-method`/`print-dup` are honored on the `pr`/`print`
path. The answer is an **evaluator-level `pr-on` driver that
every pr-family call goes through, unconditionally**
(no fast path — one print path, exactly like the JVM):

- `print_impl` (`io.rs`: `pr`/`prn`/`print`/`println`) and
  `pr_print_str` (`core.rs`: `pr-str`/`print-str`) stop calling
  `pr_to_string` on whole values. For each top-level arg they
  `realize_deep` (unchanged), then call
  `pr_on(eval, value, writer, opts)`.
- `pr_on` reads `*print-dup*`, derefs the live `print-method` /
  `print-dup` root, and calls the existing
  `invoke_multifn(eval, mf, &[value, writer])` (`multi.rs`).
  Dispatch, `method_for`, and the `No method` error are the #37
  machinery, unmodified.
- Booted default methods are Rust natives. Scalar/default
  shapes delegate to `print_into` (the engine); collection and
  ctor shapes recurse **through `pr_on` per child** (never
  through whole-value `print_into`), so user methods on nested
  values fire (probes `m-user-nested`, `p-nested`).
- `*out*` handling mirrors the JVM structurally: top-level args
  re-read `*out*` per arg; nested values use the passed writer;
  separators and the `ln` newline are written to the same
  writer, in order. `pr-str`/`print-str` bind `*out*` to a fresh
  string writer and run the same path (so `m-writer-id`'s
  `identical?` holds there too).
- `nil`/non-writer `*out*` is validated upfront with the
  existing `write_out` errors (NPE/CCE), before printing.
- Everything else keeps the pure engine: native `str`,
  `format`, error interpolation, and CCE messages never consult
  the multis (probes `m-str-unaffected`, `m-str-override`,
  `s2-simple-usermethod` pin the split).

Why unconditional (no pure fast path when no user methods
exist): per-kind routing cannot see nested customs without a
tree scan, and the JVM runs the multifn on every value. One
path is simpler to specify, simpler to verify (every existing
print pin exercises it under §1h), and exactly JVM-shaped.
`print_into` remains the rendering engine underneath the
defaults; it is not a second print path for pr-family output.

## Part 1 — Contract

### 1a. Driver paths and writer behavior

| probe id | pins |
|---|---|
| `m-user`, `m-user-nested`, `m-dup-user` | user `print-method`/`print-dup` fire on `pr-str`, recursively; dup methods gated by `*print-dup*` |
| `m-user-spaces`, `m-user-newline` | separators/newline interleave with user output |
| `m-writer-id` | nested values receive the live `*out*` (`identical?`) |
| `m-str-unaffected`, `m-str-override`, `s2-simple-usermethod` | `str`/`print-simple` never consult user methods |
| `m-dup-delegates`, `m-dup-nodelegates` | dup→method delegation goes through the LIVE `print-method` multi (user methods apply under dup); array-map does not delegate |
| `d-queue` | `IPersistentList` dup delegates to `print-method` (informational: the port has no queue kind) |

- The six pr-family natives route through `pr_on` (§0).
  `pr`/`print` write separators between args, no trailing
  space; `prn`/`println` append `\n` (see §1g for the flush).
  Bare `(pr)`/`(print)` write nothing (existing behavior kept).
- `print-simple`'s `(str o)` half and native `str` use
  defaults-only rendering (§1f), never the live tables.

### 1b. Dispatch tokens (the port's dispatch domain)

The JVM dispatches on `Class` objects; the port has no classes
(`class`/`type` are out of scope, N1). The booted multis
dispatch on **stable per-kind strings** (echoing the JVM table
keys where they exist), plus the value `nil` for nil and
user keywords via meta `:type`. Tokens are exact-matched by
`method_for` (no hierarchy, per the #37 fence).

| port kind(s) | token |
|---|---|
| `Nil` | `nil` (the value; JVM-identical key) |
| `Bool`, `Int`, `Double`, `BigInt`, `BigDec`, `Ratio` | `"java.lang.Boolean"`, `"java.lang.Long"`, `"java.lang.Double"`, `"clojure.lang.BigInt"`, `"java.math.BigDecimal"`, `"clojure.lang.Ratio"` |
| `Str`, `Char`, `Symbol`, `Keyword` | `"java.lang.String"`, `"java.lang.Character"`, `"clojure.lang.Symbol"`, `"clojure.lang.Keyword"` |
| `List` (empty and non-empty unified) | `"clojure.lang.PersistentList"` |
| `Vector` | `"clojure.lang.PersistentVector"` |
| `MapEntry` | `"clojure.lang.MapEntry"` |
| `Map` (via `map_class_of`) | `"clojure.lang.PersistentArrayMap"` / `"...PersistentHashMap"` / `"...PersistentTreeMap"` |
| `Set` (via `set_class_of`) | `"clojure.lang.PersistentHashSet"` / `"...PersistentTreeSet"` |
| `Regex`, `TaggedLiteral`, `ReaderConditional` | `"java.util.regex.Pattern"`, `"clojure.lang.TaggedLiteral"`, `"clojure.lang.ReaderConditional"` |
| `Var`, `Unbound` | `"clojure.lang.Var"`, `"clojure.lang.Var$Unbound"` |
| `Throwable` (all classes unified) | `"java.lang.Throwable"` |
| `Transient` (all kinds unified) | `"clojure.lang.ITransientCollection"` |
| `Atom`, `Delay`, `Agent` | `"clojure.lang.Atom"`, `"clojure.lang.Delay"`, `"clojure.lang.Agent"` |
| `Future`, `Promise` (existing `jvm_class`) | `"clojure.core$future_call$reify"`, `"clojure.core$promise$reify"` |
| all seq extensions (`RealizedSeq`, `RangeValue`, `RepeatValue`, `CycleValue`, `IterateValue`, `ConsValue`, `KeySeqValue`, `ValSeqValue`, `PendingSeq`) | `"clojure.lang.ISeq"` |
| `SubVectorValue` | `"clojure.lang.APersistentVector$SubVector"` |
| `Closure`, `NativeFn` | `"clojure.lang.Fn"` |
| `NsHandle` | `"clojure.lang.Namespace"` |
| `Writer` (all kinds unified) | `"java.io.Writer"` |
| `StreamReader` | `"clojure.lang.LineNumberingPushbackReader"` |
| `Reduced` | `"clojure.lang.Reduced"` |
| `Eduction` | `"clojure.core.Eduction"` |
| `HostType::Def` (all descriptors) | `"java.lang.Class"` |
| `HostType::Inst` record | `"clojure.lang.IRecord"` |
| `HostType::Inst` deftype/reify/proxy, `Fx4Object`, any other extension (total fallthrough) | `"java.lang.Object"` |
| `MultiFn` | `"clojure.lang.MultiFn"` |

- Dispatch fns are Rust natives (the JVM's verbatim Clojure
  fns name `class`, which doesn't resolve here):
  `print-dup` dispatches `token_for(x)`; `print-method`
  dispatches the meta `:type` when it is a keyword, else
  `token_for(x)` (probes `m-type-kw`, `m-type-nonstr`,
  `p-type-ignored`).
- Tokens are strings (never keywords), so meta `:type`
  keywords can never collide with kind tokens.
- Coarse tokens branch internally where JVM shapes split
  (array/hash/tree maps, hash/tree sets); per-record and
  per-throwable-class customization is via meta `:type`
  (documented limitation, §2d F3).
- A user `defmethod` on a token the dispatch never produces
  (e.g. `"java.lang.Number"`, an interface) never fires
  (probe `m-iface` documents the JVM behavior the port does
  not replicate — hierarchies are fenced).

### 1c. Booted default tables and per-kind dup shapes

`print-method` entries exist for every token in §1b **plus a
`:default` entry**; `print-dup` entries exist only where the
JVM has a dup shape — kinds without one have **no entry** and
miss to the `No method` throw (probe `m-has-default`:
`[true false]`).

`print-method` defaults:

- Every token's default renders exactly today's `print_into`
  shape for that value (the engine is unchanged for
  `readably`/`print` rendering): scalars, collections with
  length/level/meta/namespace-maps policy, regex quoting,
  `#error`, records, `#object` fallbacks. Collections recurse
  per child through `pr_on` with JVM `print-sequential`
  binding semantics (`*print-level*` decremented per nesting
  level; length truncation with `...`; `", "` map separators).
- `:default` replicates the JVM fn verbatim in structure: if
  the value's kind is in the JVM-`IObj` set (collections,
  `Symbol`, fns, seqs, records — a static kind table, NOT the
  port's `with-meta` ability), strip `:type` from its meta and
  re-invoke `print-method`; else `print-simple`. Probes
  `m-type-unknown-kw`, `m-type-strip-visible`,
  `m-type-nonstr-meta`, `m-type-atom-ctor` (atom ctors carry
  meta without `IObj`, so the atom corner takes the
  `print-simple` arm and prints `toString`). `PendingSeq` is
  excluded from the set even though it is a seq: it is
  meta-invisible and unrebuildable, so strip-and-reinvoke
  would loop — the `print-simple` arm terminates instead.
- `Eduction` and the `IDeref` kinds delegate to the engine
  (known-fidelity follow-ups F1/F2, §2d — the defaults are
  still installed and user-overridable).

`print-dup` defaults (per-kind shapes; `→method` = invoke the
live `print-method` multi on the same `[o w]`, so user methods
apply — probe `m-dup-delegates`):

| probe id | kind(s) | dup shape |
|---|---|---|
| `d-nil`, `d-bool`, `d-keyword`, `d-symbol` | nil/bool/kw/sym | `→method` (`nil`, `true`, `:u/id`, `a/b`) |
| `d-long`, `d-double`, `d-nan`, `d-bigint`, `d-bigdec`, `d-ratio` | all port numbers | `→method` (`5`, `1.5`, `##NaN`, `5N`, `5.0M`, `1/2`) |
| `d-string`, `d-char`, `d-char-named` | str/char | `→method` (quoted/named even when `*print-readably*` is false: `p-str-unreadable`, `p-char-unreadable`, `p-print-str`) |
| `d-regex`, `d-tagged` | regex/tagged | `→method` (`#"a+"`, `#x 1`) |
| `d-list`, `d-vec`, `d-lazy`, `d-range`, `d-repeat`, `d-cons`, `d-stringseq`, `d-keyseq`, `d-valseq`, `d-chunked`, `d-arrayseq` | lists/vectors/all seqs | `→method` (seq shapes) |
| `d-hashmap-shape` | hash-map | `→method` (readable `{...}`, order-unstable — pin is shape facts) |
| `d-hashset` | hash-set | `→method` (`#{1 2}`) |
| `d-arraymap`, `d-sortedmap` | array/tree map | `#=(<class>/create {entries})` with print-meta prefix when meta is non-empty; entries recurse via `pr_on` |
| `d-sortedset` | tree set | `#=(clojure.lang.PersistentTreeSet/create [items])` via `pr_on` |
| `d-mapentry` | map entry | `#=(clojure.lang.MapEntry/create [k v])` via `pr_on` |
| `d-subvec` | subvector | `#=(clojure.lang.APersistentVector$SubVector/create [items])` via `pr_on` |
| `d-var-user`, `d-var-core`, `p-var-meta` | var | `#=(var ns/name)`; no meta prefix (the method skips `print-meta`) |
| `d-ns` | namespace | `#=(find-ns name)` |
| `d-anonfn`, `d-namedfn` | closures + natives | `#=(<munged-display>. )` (via `print-ctor` with empty args; note the `. ` — same shape for port-native fns by analogy) |
| `d-record`, `d-record-str`, `p-record-meta` | record inst | `#Name[v1, v2]` — vals in **field order** (`def.fields`, not content iteration), `", "`-joined, via `pr_on`; print-meta prefix when meta is non-empty; always non-verbose (`*verbose-defrecords*` is out of scope) |
| `d-class`, `d-class-iface` | type descriptors | `#=<name>` |
| `d-object`, `d-deftype`, `d-reify`, `d-atom`, `d-delay`, `d-future`, `d-promise`, `d-agent`, `d-volatile`, `d-reduced`, `d-transient`, `d-throwable`, `d-readercond`, `d-multifn`, `d-writer`, `d-reader`, `d-eduction` | no JVM dup shape | **no table entry**: miss throws `IllegalArgumentException` via the stock #37 `No method` error, naming the §1b token: `No method in multimethod 'print-dup' for dispatch value: "<token>"` (quoted — the token is a string) |
| (source-derived, no oracle row) | `Unbound` | no entry → same throw (`Var$Unbound` has no JVM dup shape; the value is unconstructible from Clojure surface, so no probe) |

JVM-only rows (no port kind; informational, not implemented):
`d-int`, `d-short`, `d-byte`, `d-float`, `d-biginteger`
(non-Long/Double `Number` ctor forms), `d-class-prim`,
`d-class-array`, `d-queue` (queue delegates to `print-method`
via `IPersistentList`).

| probe id | pins |
|---|---|
| `m-shape` | both multis are `MultiFn`-classed values |
| `m-table-method`, `m-table-dup` | the full JVM default tables these flatten (incl. the `""`-rendered `nil` key, `Eduction`, and the `uuid`/`instant`/`gvec`-loaded extras, which have no port kinds) |
| `m-getmethod` | table queries interoperate (`get-method` hits; dup miss on unknown kw is nil) |
| `m-remove`, `m-dup-remove` | `remove-method` restores default / throw |

- `print-method`/`print-dup` are plain (non-dynamic) roots
  (probes `s-pm-nondynamic`, `s-pd-nondynamic`: stock
  `Can't dynamically bind non-dynamic var` errors).
- `(methods print-method)` / `(methods print-dup)` keys are
  exactly the §1b token sets (+ `:default` for `print-method`
  only) — asserted in `printsys.rs`, not oracle-pinned (the
  tokens are port-defined).

### 1d. `*print-dup*` policy (the `dup` flag)

| probe id | pins |
|---|---|
| `p-root` | root is `false` (already true; kept) |
| `p-length`, `p-level`, `p-length-ctor` | length/level ignored under dup (even `0`, even inside ctor args) |
| `p-meta`, `p-meta-tag`, `p-record-meta`, `p-print-str-meta` | meta always prints under dup (and meta *values* print dup-aware: maps → ctor forms, classes → `#=`) |
| `p-nested`, `p-nested-map`, `p-nested-vec-in-set` | ctor/collection args recurse dup-aware |
| `p-str-unreadable`, `p-char-unreadable`, `p-print-str` | dup beats `*print-readably*` false |
| `p-type-ignored` | dup ignores meta `:type` (class/token dispatch only; the `::pdup-mine` dup method never fires — and the trace through `:default` strips the meta, so output is bare `[1]`) |
| `p-false-map`, `p-false-meta` | dup-off controls |
| `p-var-meta` | dup `Var` prints no meta |

- `PrintOpts` gains `dup: bool` (default false; `pr()`/`print()`
  constructors unchanged); `print_opts` reads `*print-dup*`
  (truthiness). The flag is read by the driver policy (ignore
  length/level, force meta, force readable string/char
  rendering in scalar delegation).
- `print_into` with `dup: true` renders the §1c pure shapes for
  non-throw kinds; on throw-kinds it renders the fallback (the
  driver throws before ever asking — no production pure path
  passes `dup: true`, since `str`/`format`/interpolation keep
  `PrintOpts::pr()`/`print()`).
- REPL echo carve-out (the one exception to that claim):
  `Session::print_value`
  (`rust/crates/clj-repl/src/session.rs`) builds its opts via
  `print_opts(eval, None)` — so `dup: true` under a true
  `*print-dup*` root — then renders engine-direct with
  `pr_to_string` (no `pr_on` driver, so the `No method` throw
  is unreachable). Effect: echo honors `*print-dup*` for
  dupable kinds (ctor shapes, convergent with `pr-str`) but
  renders the plain fallback for undupable kinds (`(atom 1)`
  echoes `#object[clojure.lang.Atom ...]`, where `pr-str`
  throws). The echo deliberately does NOT force `dup: false`:
  the JVM REPL honors `*print-dup*` too. Pinned by
  `repl_echo_print_dup_root_with_plain_fallback`
  (`clj-repl/tests/session.rs`).

### 1e. Char maps

| probe id | pins |
|---|---|
| `c-escape`, `c-name` | full sorted contents (7 + 6 entries) |
| `c-counts`, `c-types` | counts and Char→String types |
| `c-escape-lookup`, `c-name-lookup` | lookups hit/miss |
| `c-dynamic`, `c-bind-err` | plain (non-dynamic) roots; stock bind error, message-verbatim |
| `c-uses-escape`, `c-uses-name` | behavioral anchors tying the maps to the printer |
| `c-redef-escape`, `c-redef-name` | informational: the JVM printer consults the vars (the port does not — §2d F4) |

- `char-escape-string` =
  `{\backspace "\\b", \tab "\\t", \newline "\\n",
  \formfeed "\\f", \return "\\r", \" "\\\"", \\ "\\\\"}`;
  `char-name-string` =
  `{\backspace "backspace", \tab "tab", \newline "newline",
  \formfeed "formfeed", \return "return", \space "space"}`.
- Single source, no fork: the printer's hardcoded match arms
  become `const` tables in `clj-value` (exported); `print.rs`
  consults them; boot builds the two vars from them.
  `printsys.rs` asserts both vars equal the oracle literals
  above, entry for entry.
- The vars are plain roots (installed with the non-dynamic
  boot path, like `print-method`/`print-dup`).

### 1f. `print-simple` / `print-ctor`

| probe id | pins |
|---|---|
| `s-arglists`, `s-fn` | `([o w])`, a fn in a var |
| `s-str`, `s-kw`, `s-vec`, `s-onto-out` | writes `(str o)` to `w` (unquoted), incl. `*out*` itself |
| `s-meta`, `s-meta-off`, `s-meta-dup`, `s-meta-unreadable` | `print-meta` prefix iff `(or *print-dup* (and *print-meta* *print-readably*))`; the `(str o)` half ALSO carries the prefix (double prefix — JVM `str` honors ambient bindings) |
| `s-ret` | returns nil |
| `s-arity0`, `s-arity1`, `s-arity3` | stock arity errors |
| `s-cce`, `s-nilw` | non-writer → stock CCE; nil → the `write_out` nil NPE, message-verbatim |
| `t-arglists`, `t-fn` | `([o print-args w])`, a fn in a var |
| `t-basic`, `t-passes-o`, `t-writer-is-w` | `#=(<class>. <args>)`; `print-args` receives `[o w]` with the identical writer |
| `t-ret` | returns nil |
| `t-arity`, `t-cce`, `t-nilw` | stock arity/CCE/NPE errors |
| `t-nilo` | nil `o` → NPE (port message per below; JVM message names `core$class` internals) |
| `s2-meta`, `s2-dup-map`, `s2-dup-vec`, `s2-length`, `s2-level`, `s2-seqlength` | JVM `str` honors ambient meta/dup/length/level for collections — this is what `print-simple`'s `(str o)` half replicates |
| `s2-str-raw`, `s2-dup-str`, `s2-atom`, `s2-cons`, `s2-range`, `s2-lazyseq`, `s2-mapentry-str`, `s2-readably` | non-collections are binding-insensitive (raw strings, `toString`); seq partition (lazy → content-hash, others → contents); entry strings quoted; ambient readably (not forced) |
| `s2-simple-usermethod` | `print-simple` bypasses user methods |

- `(print-simple o w)`: `print-meta` prefix per the rule
  above, then the §1f-`str` rendering of `o`, both written to
  the `Writer` value `w` (validated like `write_out`: nil →
  NPE, non-writer → `cce_err(v, "java.io.Writer")`).
- `(print-ctor o print-args w)`: writes `#=(`,
  `jvm_class_of(o)`, `. `, then invokes `print-args` with
  `[o w]` (a normal `eval.call`, so user closures work), then
  `)`. `o = nil` throws NPE with the port's nil-receiver
  message `Cannot invoke "java.lang.Class.getName()" because
  "o" is null` (pinned in `printsys.rs`; the JVM text is
  unreproducible — it names `core$class` internals).
- The §1f-`str` rendering (print-simple's object half ONLY —
  native `str` is untouched, §2d F6): nil→`""`, strings/chars
  raw, lazy seqs → content-hash, references/hostvals via the
  existing native arms; **all collection-likes
  (`List`/`Vector`/`Map`/`Set`/`MapEntry`/seq extensions/
  `SubVector`) render through the driver in defaults-only
  mode** (booted defaults, ambient bindings, user entries
  bypassed — a static kind→default dispatch, not the live
  tables). This yields the double-prefix rows and `s2-*`
  parity by construction.
- Both fns are plain natives (`install_native`, not dynamic).

### 1g. `*flush-on-newline*`

| probe id | pins |
|---|---|
| `f-root`, `f-dynamic` | root `true`, dynamic |
| `f-sep` | `line.separator` is `"\n"` on the oracle platform |
| `f-newline`, `f-newline-false` | **`newline` never flushes** (0 flushes either way) — this overrules the issue Solution text per the oracle, §2c |
| `f-prn`, `f-prn-false`, `f-println`, `f-println-false` | `prn`/`println` flush exactly once iff `*flush-on-newline*` is truthy |
| `f-pr`, `f-print` | `pr`/`print` never flush |
| `f-prn-empty`, `f-multi`, `f-newline-then-prn` | empty `prn` flushes once; multi-arg `prn` flushes once total; `newline`+`prn` = 1 flush |

- Boot `*flush-on-newline*` as a dynamic root `true` (`ns.rs`,
  next to the other print vars).
- `native_newline`: unchanged (write `"\n"`, no flush step).
- The `ln` paths (`native_prn`, `native_println`, and
  `prn_str`): after writing the newline, flush `*out*` iff
  `*flush-on-newline*` is truthy. Flush = a new `flush_out`
  helper next to `write_out` (same stdout-flush/else-no-op
  policy as `native_flush`, which is re-expressed through it).
- Test-only observability: `flush_out` increments a private
  per-`Evaluator` counter exposed as public
  `Evaluator::flush_count()` (fresh `Evaluator` reads 0;
  parallel-safe — no global, no reset). `printsys.rs`
  transcribes every `f-*` row through it with `with-out-str`
  (buffer) writers.
- (Letter-vs-spirit note: the issue asks for the hook "in
  `write_out`"; a flush carries no bytes, so the hook lives in
  the sibling `flush_out` helper in the same writer layer.)

### 1h. Mandatory re-probe rule

`PrintOpts`/`print_opts`/`print_impl`/`pr_print_str` are the
shipped printer's hot path: **any skew fails the slice**.

- Scope: (a) every Rust test whose name or file covers
  `str`/`print`/`pr`/`prn`/`println`/`format`/`newline`/`flush`/
  `with-out-str` output (at minimum: `clj-value` `print.rs`
  tests, `clj-eval` `tests/str.rs`, `tests/io.rs`,
  `tests/format.rs`, `tests/control.rs` print rows,
  `tests/seqs.rs` seq-print rows, `tests/map_entry.rs` print
  rows, `tests/tower.rs` print rows, `tests/re_rng.rs` regex
  rows, `tests/types.rs` + `tests/protocols.rs` +
  `tests/multimethods.rs` print rows, `tests/concurrency.rs`
  print rows); (b) every differential-corpus entry whose form
  prints (`pr`/`print`/`prn`/`println`/`pr-str`/`print-str`/
  `str`/`format` heads — enumerate by grep at implement time
  across `s5d`/`s6`/`s7`/`s9` corpus dirs).
- Procedure: run the full gates plus the corpus suites on the
  S8 branch; diff every in-scope output against `master`.
- Fail criterion: exactly ONE delta is permitted — corpus
  `20-print-dup.clj` `:dup-atom-true`, which converges to the
  golden (§1i). Every other delta fails the slice, including
  "more correct" deltas (convergence outside §1i needs its own
  slice + pins).
- New-behavior coverage: `printsys.rs` additionally mirrors
  truncation/meta/namespace-maps through the driver (`pr-str`
  with `*print-length*`/`*print-level*`/`*print-meta*`/
  `*print-namespace-maps*`), since existing `print_opts` tests
  address the engine directly.

### 1i. Divergence retirement (`*print-dup*` flip)

The golden already encodes JVM truth; the port converges to
it. The implementer makes exactly these edits (no more):

1. `rust/crates/clj-eval/tests/concurrency.rs`: delete
   `print_dup_flag_ignored_for_atoms`.
2. `rust/crates/clj-eval/tests/printsys.rs` (new, S8-owned):
   add `print_dup_atom_throws` transcribing both golden rows —
   `false` → `pr-str` starts with `"#object[clojure.lang.Atom
   0x`; `true` → uncaught `java.lang.IllegalArgumentException |
   No method in multimethod 'print-dup' for dispatch value:
   "clojure.lang.Atom"`.
3. `docs/t1174/s5d-corpus/20-print-dup.clj`: rewrite the
   `DIVERGENCE` header comment to `CONVERGED (S8/issue #51)` —
   port throws like the JVM now. Golden and forms unchanged.
4. `docs/t1174/s5d-corpus/MANIFEST.md` (`20-print-dup` bullet):
   drop `KNOWN DIVERGENCE`, cite `print_dup_atom_throws`
   (`printsys.rs`) as the transcribing test.
5. `docs/t1174/s5d-contract.md` (§D6 line): `*print-dup*` is
   declared but ignored (divergent) → honored on pr paths
   (S8/issue #51; `printsys.rs`).
6. `docs/t1174/s9-plan.md` (S9E corpus line): "the pinned
   divergence" → "converged by S8/issue #51".
7. `docs/t1192/specification.md`: `D-printdup` line →
   `RESOLVED conformant (S8/issue #51)`; FX4b bucket line drops
   `D-printdup` (keeps `D-streamprint`/`D-astral-fmt`).
8. `rust/core-coverage.md`: the §1j flip (the row's
   `divergent` state is what these pins exist for).

There is no port-side corpus runner for `s5d` (JVM-only runs
+ transcribing row tests per the S9E note), so no runner or
allowlist changes exist to make.

### 1j. Matrix edits

PR #41 is merged, so the 7 rows sit at their final positions:
flip in place (no adds, no moves). `*print-dup*` flips in
place. Header counts become **692 vars: 418 implemented,
1 divergent, 273 out of scope, 0 planned**.

| row (position kept) | new status | new notes (exact) |
|---|---|---|
| `` *flush-on-newline* `` | implemented | `dyn var root true; prn/println flush iff true (S8: printsys.rs flush_rows; probes f-*)` |
| `` char-escape-string `` | implemented | `7-entry map; single-source w/ printer tables (S8: printsys.rs charmaps_rows; probes c-*)` |
| `` char-name-string `` | implemented | `6-entry map; single-source w/ printer tables (S8: printsys.rs charmaps_rows; probes c-*)` |
| `` print-ctor `` | implemented | `[o print-args w] #= writer (S8: printsys.rs simple_ctor_rows; probes t-*)` |
| `` print-dup `` | implemented | `booted multifn, token dispatch, no :default (S8: printsys.rs dispatch_rows + dup_rows; probes m-*/d-*/p-*)` |
| `` print-method `` | implemented | `booted multifn, :type-or-token dispatch, chaining :default (S8: printsys.rs dispatch_rows; probes m-*)` |
| `` print-simple `` | implemented | `[o w] meta+str writer (S8: printsys.rs simple_ctor_rows; probes s-*/s2-*)` |
| `` *print-dup* `` | implemented | `dup routing on pr paths; undupable kinds throw (S8: printsys.rs dup_rows; probes d-*/p-*; corpus 20 converges)` |

### 1k. Tests and fence

- New `rust/crates/clj-eval/tests/printsys.rs` with (names
  normative): `dup_rows` (§1c table + `d-*`), `dup_policy_rows`
  (`p-*`), `charmaps_rows` (`c-*`), `simple_ctor_rows`
  (`s-*`/`t-*`/`s2-*`), `flush_rows` (`f-*`),
  `dispatch_rows` (`m-*`), `print_dup_atom_throws` (§1i),
  `driver_truncation_rows` (§1h), `print_tables_keys` (§1c
  key sets). `print.rs` gains unit tests for pure dup
  rendering (fallback-on-throw-kinds documented).
- Fence: implementation owns `printsys.rs`,
  `PrintOpts`/`print_into` dup routing, the char tables, the
  two booted multis + driver, `io.rs` flush policy, the §1i
  one-line doc/test edits, and the 8 matrix rows + header.
  Never touch `hasheq.rs`/`compare.rs` or any other slice's
  paths; native `str` is behavior-frozen (§2d F6).
- Gates: `cargo test -p clj-eval -p clj-value -p clj-reader`,
  `cargo test -p clj-core --test coverage_matrix`,
  `cargo fmt --all --check`, `cargo clippy --all-targets`
  zero warnings, complexity gate, llvm-cov clean.

## Part 2 — Notes

### 2a. Derivation story

The oracle is behavioral (178 rows), but the design also rests
on source reads: alpha10 `core_print.clj` (all `print-method`/
`print-dup` bodies, `print-sequential`'s dup exemptions,
`print-ctor`, the char maps) and `core.clj` (the `defmulti`
dispatch fns, `pr-on`, `pr`/`prn`/`print`/`println`,
`newline`/`flush`, `*flush-on-newline*`/`*print-dup*` roots).
Two facts surprised and were nailed down by probes rather than
assumed: `newline` never flushes (§2c), and the oracle's
default tables include methods loaded outside core
(`clojure.uuid`, `clojure.instant`, `clojure.gvec` — all
dateless/port-kindless, hence N/A). The port's token domain
(§1b) is then a flattening of exactly the reachable JVM table
over port kinds, verified cell by cell against `m-table-*`.

### 2b. Issue Solution open points, resolved

- Central dispatch design → §0 (unconditional driver).
- Per-kind dup shapes incl. `#=` ctors → §1c table + `d-*`.
- Char map equality → §1e (single-source tables).
- `print-simple`/`print-ctor` arities + writer behavior →
  §1f (incl. the defaults-only `str` half, `s2-*`).
- Flush root + timing → §1g (+ the §2c correction).
- Default-vs-user dispatch order → §1a/§1c (`m-*`).
- Re-probe rule → §1h. Flush hook → §1g. Matrix edits → §1j.
- Pin retirement on the divergent flip → §1i.

### 2c. What the spec does not honor (and why)

1. **"`newline` ... flush[es] `*out*` when true".** The oracle
   says otherwise (`f-newline [0 "\n"]`, `f-newline-false
   [0 "\n"]`), and the alpha10 source is unambiguous
   (`newline` = append + nil; only `prn` flushes). The JVM's
   autoflush `*out*` does not fire on `append` either. The
   spec follows the oracle: `newline` never flushes. Only the
   `ln` paths consult `*flush-on-newline*`.
2. **"Flush-counter hook in `write_out`" (letter).** A flush
   carries no bytes, so the hook lives in the sibling
   `flush_out` helper + a per-`Evaluator` counter (§1g).
   Same layer, test-only visibility, parallel-safe.
3. **"Add the 7 new rows at mirrored PR #41 positions".**
   PR #41 merged before this branch, so the rows exist at
   those positions; the implementer flips in place (§1j).
4. **`print-ctor` nil-`o` message.** The JVM text names
   `core$class` internals (`t-nilo`); the port has no `class`
   fn, so §1f specifies the port's nil-receiver message
   instead (same exception class).

### 2d. Deferred follow-ups (discovered, with evidence)

- **F1. Eduction readable shape.** JVM prints the seq when
  readably (`print-method Eduction`); the port prints fixed
  `#object` (`Eduction::print`). S8's default delegates
  (status quo). Evidence: `core.clj:8121`, `d-eduction`
  (dup correctly throws).
- **F2. `IDeref` deref-map printing.** JVM prints
  `#object[Atom 0xH {:status :ready, :val 1}]` (pending/
  failed states too); the port prints the `Class@hex`
  fallback. S8's defaults delegate (status quo). Evidence:
  throwaway oracle rows (`atom`/`delay`/`agent`/`vol`/`red`
  + ctor-meta + non-readably — all deref-maps; `str` stays
  `toString`). Existing pins are `startsWith`-shaped, so a
  later slice converges without breaking them.
- **F3. Hierarchy dispatch.** Port `method_for` is
  exact-plus-`:default` (#37 fence): interface-level user
  methods (`m-iface`), per-record/per-class methods, and
  `prefer-method` consultation don't replicate. S8 documents
  the token table; per-kind customization beyond it waits on
  hierarchies. (Degenerate corner found while probing:
  removing a core default stack-overflows the JVM itself via
  `Object`-fallback recursion — unpinnable, out of scope.)
- **F4. Char-map redefinition visibility.** `with-redefs` on
  the char maps reaches the JVM printer (`c-redef-*`); the
  port asserts table equality in tests instead (issue's
  "don't fork" direction). Accepted limitation.
- **F5. `*verbose-defrecords*`.** Out of scope (no slice);
  record dup is always non-verbose (`#Name[v, ...]`).
- **F6. Native `str` binding-awareness.** JVM `str` honors
  ambient meta/dup/length/level for collections (`s2-*`)
  and quotes entry strings (`s2-mapentry-str` — the port's
  `MapEntry` arm does not); the port's native `str` is
  behavior-frozen for S8 and `print-simple` carries its own
  defaults-only rendering instead.

### 2e. Open design risks

- The driver reimplements `print-sequential` policy (level
  bindings, `...`, `, `, `#`) at eval level while the engine
  keeps its own copy; §1h + `driver_truncation_rows` guard
  drift, but the two copies are a lasting maintenance pair.
- User methods that `pr` the same value recurse until the
  stack gives out (JVM: `StackOverflowError`) — no guard on
  either side; acceptable parity.
- `realize_deep`-before-print preserves unbounded strict seqs,
  which print the fixed `(...)` shape (`(pr-str (range))`,
  pinned in `dup_policy_rows`); infinite *pending* (lazy) seqs
  still hang in collection. In-flight streaming
  (`D-streamprint`) stays a follow-up.
- A user redefinition of the `print-method`/`print-dup` var
  roots to non-multifns breaks `pr` on both sides; the port's
  exact error is implementer latitude (degenerate, unpinned).
- `*print-dup*` bound to non-booleans follows port truthiness
  (universal behavior, unpinned).
- User methods on kinds nested inside scalar payloads
  (tagged-literal forms, throwable data) do not fire: only
  collection shapes recurse through `pr_on`; scalars render
  whole through the engine. JVM-finer behavior, unpinned,
  follow-up (documented in `printsys.rs`).
