# Issue #36: S-instream slice — Specification

- Status: ready (research complete: `io.rs` Writer survey,
  `with-out-str` analyzer path, CharStream/Reader streaming design,
  JVM `core.clj` arity survey, repo-history `LispReader`/`LNPR`
  mechanics, 78 alpha10 oracle pins).
- Parent: out-of-scope reduction. Issue: #36. Branch: `36-instream`.
  Probes: `docs/i36-instream/probes/instream.{clj,out}`
  (to be written; exit 0 required).

## Problem

Five `clojure.core` input vars are `out-of-scope` with no slice and no
code in the tree: `*in*`, `read`, `read-line`, `line-seq`, `with-in-str`.
Any program reading from `*in*` fails at resolve time (`Unable to resolve
symbol`); `read`-driven loops and `line-seq` pipelines cannot be expressed.

## Solution

Two stages, each test-first against committed oracle probes:

1. **Readers** (`reader.rs` + `READER_NATIVES`, `*in*` root): a
   `StreamReader` ExtensionValue mirroring `Writer` (sequence id,
   `#object[...]` print): `Stdin` (process stdin through a persistent
   `BufReader`, shared by `read` and `read-line`) and `Str`
   (string-backed with a persistent `clj_reader::Reader`, so line
   numbers survive across calls like the JVM's
   `LineNumberingPushbackReader`). `*in*` boots as a dynamic root on the
   stdin reader (`ns.rs`, beside `*out*`/`*err*`). `read-line` consumes
   one JVM-`readLine` line (nil at EOF).
2. **Forms** (`read`, `line-seq`, `with-in-str`): `read` serves all five
   JVM arities over the bound reader (opts map `:read-cond`/`:features`
   onto `ReaderOpts`, default deny; stdin grows the buffer line by line
   until a form completes or EOF; every read failure surfaces the
   `RuntimeException` root — wrapped in `ReaderException` with ex-data
   line/column unless `recursive?` is true). `line-seq` is a `core.clj`
   defn with an eager first pull plus a lazy tail (zero Rust, like
   `random-sample`); it consumes the port's readable by design (the
   JVM's BufferedReader-only gate is unrepresentable without host
   classes — probes `ls-basic`–`ls-empty` document the JVM side).
   `with-in-str` is an analyzer builtin mirroring
   `parse_with_out_str`/`Expr::WithOutStr` (body non-tail, returns the
   body's value).
3. **Drive-by** (one line + one test): `read-string` passes
   `ReadCond::Deny` — oracle `rs-cond` proves the JVM rejects
   reader conditionals there, and the port currently allows them.

## Part 1 — User/functional specification

### 1.1 Goals

1. All five rows become `implemented`, with probe-backed notes.
2. `(with-in-str "a\nb" (read-line))` → `"a"`; a second `(read-line)` →
   `"b"`; a third → nil (EOF, no throw).
3. `(with-in-str "(a)\n(b)" (read) (read))` → `(a)` then `(b)`: the
   stream advances and the second form keeps absolute line 2 in meta.
4. `read` arities: `[]` reads `*in*`; `[stream]`; `[stream eof-error?
   eof-value]`; `[stream eof-error? eof-value recursive?]`; `[opts
   stream]` with `:read-cond`/`:features`/`:eof`.
5. `line-seq` is lazy, nil-terminated, and never closes (JVM parity;
   `with-open` stays out of scope).
6. `with-in-str` binds a fresh string reader, runs the body, returns the
   body's value; the outer `*in*` is untouched afterwards.
7. `read` failures match the JVM wrapper rule (history
   `LispReader.java:310-317`, probes `r-*-outer`): non-recursive reads
   throw `ReaderException` (message `{cause}: {root}`, ex-data
   `:clojure.error/line` + `:clojure.error/column` from the error span,
   root cause attached); `recursive?` reads rethrow the raw root.
8. `read` and `read-string` reject reader conditionals unless the opts
   map opts in (`r-cond-plain`, `rs-cond`); `:eof` present (even nil)
   means no-throw, absent means throw (`r-opts-eofnil`).

### 1.2 Non-goals (explicit)

- `read+string`, `*read-eval*`, `load-reader`: absent from the 422-row
  coverage universe — flagged as follow-up, not fixed here.
- `with-open` (needs Closeable/protocol: issue #33's territory,
  untouched). String readers need no close, so `with-in-str` without
  `with-open` is observably identical to the JVM.
- `#=` eval reader and custom `data_readers.clj` (stay out of scope per
  README); with no eval reader, `read` never executes code.
- `recursive?` beyond acceptance: accepted at the 4-arity; any
  observable effect is oracle-pinned (expected: none without custom
  data readers).
- REPL interplay: `read`/`read-line` on the stdin root block on
  process stdin like the JVM; no rustyline integration.
- `read-string`'s JVM 2-arity (`[opts s]`): still missing after this
  slice (only the deny-default drive-by lands here) — follow-up, not
  fixed here.

### 1.3 Acceptance criteria

- The five rows flip to `implemented`; `coverage_matrix` green.
- Every new behavior test pins a committed oracle probe row verbatim
  (probe id in the test name or comment).
- `read`/`read-line` interleave on one stream with a shared position:
  `(with-in-str "a\n(b)" (read-line) (read))` → `"a"`, `(b)`.
- Truncated input throws even with `eof-error?` false: `(with-in-str
  "(a" (read *in* false :eof))` → `ReaderException` over a
  `RuntimeException` root (the JVM returns eof-value only at clean
  form boundaries).
- `line-seq` pulls eagerly: `(line-seq nil)` throws at call time, no
  `doall` needed (`ls-eager`, `ls-str-eager`); values are `Cons` with
  a lazy tail (`ls-class2`, `ls-lazy2`).
- No changes under `docs/i33-typechain/` or typechain code paths
  (parallel-agent fence).

### 1.4 BDD scenarios (user-observable)

- Given `*in*` bound by `(with-in-str "a\nb" …)`, when the body calls
  `(read-line)` twice then once more, then it observes `"a"`, `"b"`,
  nil.
- Given `(with-in-str "(a)\n(b)" (read) (read))`, when both forms are
  read, then the values are `(a)` and `(b)` and the second carries
  `:line 2`.
- Given `(with-in-str "" (read *in* false :eof))`, when the stream is
  empty at a form boundary, then the result is `:eof` (no throw).
- Given `(with-in-str "(a" (read *in* false :eof))`, when the form is
  truncated, then an `EOFException` naming the start line is thrown.
- Given `(with-in-str "x\ny" (doall (line-seq *in*)))`, when the seq
  realizes, then the result is `("x" "y")` and `*in*` still binds the
  same (exhausted, unclosed) reader.
- Given `(with-in-str "1 2" (read) (read-line))`, when a form read is
  followed by a line read, then the results are `1` and `" 2"`.
- Given `(with-in-str "(a" (read))`, when the form is truncated, then
  a `ReaderException` is caught by both `(catch ReaderException …)`
  and `(catch RuntimeException …)`, its message is
  `"java.lang.RuntimeException: EOF while reading, starting at line
  1"`, and its ex-data is `{:clojure.error/line 2,
  :clojure.error/column 1}`.
- Given `(line-seq nil)`, when it is called without forcing, then it
  still throws (the first pull is eager, like the JVM).

## Part 2 — Technical specification

- Source of truth: `core.clj` at the alpha10 tag (`read` five arities
  plus opts keys; `read-line` 0-arity over `*in*`; `line-seq` lazy cons
  over `.readLine`; `with-in-str` fresh-`StringReader` binding) plus
  `BufferedReader.readLine` terminator semantics (`\r`, `\n`, `\r\n`),
  confirmed by the probes.
- Port shape: new `rust/crates/clj-eval/src/reader.rs` (`StreamReader`
  extension + `stdin_reader`/`string_reader` constructors +
  `READER_NATIVES`, chained in `ns.rs` beside `IO_NATIVES`);
  `ThrowClass::ReaderException` in `clj-value/src/extension.rs`
  (`clojure.lang.LispReader$ReaderException`, parent
  `java.lang.RuntimeException` in `throw_parent`, catch-name arms in
  `throw_class_for`, mirroring #32's `PatternSyntaxException`);
  `*in*` dynamic root in `ns.rs` beside `*out*`/`*err*` (plus the
  `boot_refer_user` entry); `line-seq` appended to `core.clj` as
  `(defn line-seq [rdr] ((fn step [] (binding [*in* rdr]
  (when-let [line (read-line)] (cons line (lazy-seq (step))))))))`
  (eager first pull, lazy tail — no `doall` needed to throw);
  `Expr::WithInStr` analyzer builtin in `expr.rs`/`eval.rs`
  mirroring `WithOutStr` (variant + `SpecialOp` + table row +
  shadowable arm + dispatch + non-tail body) plus the leading `s`
  expression, returning the body's value; AOT arms
  (`read`/`read-line`/`line-seq` join the "only print writes" hint,
  `with-in-str` joins "macros are not expanded"); `read-string`
  drive-by: `read_cond: ReadCond::Deny` in `native_read_string`.
- clj-reader additions (small, unit-tested there): `CharStream::extend`
  (append normalized chars, coalescing a `\r\n` split across chunks),
  `CharStream::read_line` (one `\n`-split line via repeated `read()`,
  so tracking rides free; `None` at EOF), `Reader::set_opts` (fresh
  ns/aliases per `read`, like `read-string`), plus a `Send + Sync`
  compile-assert on `Reader` (it lives behind the stream `Mutex`).
  Verify `ReadCond::Deny` already reports `Conditional read not
  allowed`, else fix it there.
- Stream state: `Arc<Mutex<StreamState>>` holding one persistent
  `clj_reader::Reader` plus an optional `BufReader<Stdin>`; `read` and
  `read-line` share the position, so interleaving matches the JVM.
  Stdin `read` extends the buffer line by line while `read_form`
  reports `Eof*` and stdin is live; anything else throws through the
  `read-string` error mapping. Both reader kinds print as
  `clojure.lang.LineNumberingPushbackReader` (the JVM class of the
  `*in*` root and the `with-in-str` binding alike); the id is
  port-owned like `Writer`'s (fenced equivalence, probe-shaped).
- Error texts oracle-pinned (`instream.out`): nil `*in*` for
  `read-line` is the verbatim Var-`get()` NPE (`rl-nil-in`); nil
  stream arg for `read` is the verbatim `Reader.read()` NPE
  (`r-nil-stream`); non-readers are stable-shape CCEs via
  `cce_message` (JVM texts carry module-loader suffixes, so the
  probes document them while the port stays stable, like `read-string`
  already does); `ReaderException` message is
  `{cause-jvm-class}: {root-message}` with ex-data
  `{:clojure.error/line, :clojure.error/column}` from the error span
  (`r-exdata*`, `r-baddelim-exdata`); opts-map `:eof` presence rule
  (present, even nil, → value at EOF; absent → throw); unknown opt
  keys ignored (`r-opts-unknown`).
- TDD plan: oracle probe files first (alpha10 jars, 78 pins; stdin
  behaviors probed through `StringReader`-backed equivalents plus one
  piped-stdin script run, never interactively), stage tests red
  (unresolvable vars), then stage 1 (readers + `read-line`), then
  stage 2 (`read`/`line-seq`/`with-in-str` + `read-string` drive-by),
  then row flips + `coverage_matrix`.
- CI impact: new Rust code under `rust/**`, no new dependencies (std
  only); full gate suite applies (fmt, clippy `-D warnings`, 100%
  line+patch floors, complexity ratchet ≤15). No workflow changes
  needed.

### Constraints

EPL-1.0 headers on new Rust files; MSRV 1.85 (no let-chains); signed
commits (m-hu) with `Ref #36`; green-before-push; macOS work runs from
`rust/`; oracle command + output committed for every new behavior test.
Tests never touch the real stdin root (it would block CI): string
readers everywhere, plus direct constructor tests for the stdin kind.
