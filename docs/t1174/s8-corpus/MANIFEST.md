# S8 subset corpus MANIFEST

TDD-first corpus for the LLVM AOT pure subset (spec §2.4,
pins `s8-pins.md`). Truth source: `s8-pins.md` (P1–P5).

## Layout

- `00-empty`–`05-print`: supported programs. `expected/NN.out`
  = stdout goldens (seeded from `run`, exit 0, stderr
  empty); `expected/jvm/NN.jvm.out` = JVM script-mode
  goldens, byte-identical at commission (triple-grounded:
  interpreter == JVM; AOT must match both).
- `06-err-*`–`09-err-*`: runtime-error programs (in-subset
  forms that fail at runtime). `expected/NN.out` = partial
  stdout (`before ` — flushed on both paths); `.runerr` =
  the `run` diagnostic (`{path}:1:1: ...`, synthesized
  span). AOT pins TRUE call-site spans instead (ours-pinned
  stderr, exit 1).
- `unsupported/01`–`16`: out-of-subset forms. `.runout` =
  `run` stdout (exit 0 — the BDD "still work under `run`"
  clause; absent `.runerr` = empty stderr). `build` must
  exit 1 naming the first unsupported form (exact-format
  asserts in `rust/tests/build_corpus.rs`, formats below).
- `unsupported/17-analyzer`, `18-reader`: malformed input.
  17: `build` reports the analyzer diagnostic with direct
  file spans (byte-identical to `run` here). 18: `build`
  reports the raw reader message (`{path}:1:2: Unmatched
  delimiter: )`) WITHOUT `run`'s load-file wrapper
  (`Syntax error reading source: ...`) — pinned divergence.

## Oracle recipes

From the repo root (`B=rust/target/debug/clojure-rs`):

```
# Seed interpreter goldens:
$B run docs/t1174/s8-corpus/NN-topic.clj > expected/NN-topic.out
# JVM cross-check (script mode — `-i/-e` loses *out* on System/exit):
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:..."
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp "$CP" clojure.main NN-topic.clj > expected/jvm/NN-topic.jvm.out
# AOT check (post-S8C):
$B build NN-topic.clj -o /tmp/s8/NN && /tmp/s8/NN | diff - expected/NN-topic.out
```

## Differential rules

- Supported: AOT binary stdout byte-identical to
  `expected/NN.out`, exit 0, stderr empty.
- Error programs: AOT stdout byte-identical to
  `expected/NN.out` (the `(print "before ")` prefix pins
  the abort-path flush); AOT stderr ours-pinned
  (`{path}:{line}:{col}: {Class}: {message}`, true spans),
  exit 1.
- Unsupported 01–16: `build` exit 1, stderr exactly
  `{path}:{line}:{col}: unsupported form in AOT subset:
  {form} ({hint})` (the `(line, col, form, hint)` table
  lives in the replay test); `run` exit 0 with
  `.runout` stdout.
- 17/18: exit 1 both paths; 17 byte-identical to `run`,
  18 modulo the load wrapper (above).

## Programs

- `00-empty`: comment-only file (no tops, silent exit 0).
- `01-arith`: `defn`, `+ - * quot`, `if`/`do`/`let`.
- `02-recursion`: `fact`/`fib` via direct self-calls.
- `03-types`: Double/Bool/Str/Vector flow, chained
  comparisons, same-category `=`, `count` (incl. astral
  UTF-16), `nth`/`get` (+miss/nil), `conj`, `if`-no-else
  nil, nil literal.
- `04-let-do-if`: nesting, shadowing, truthiness
  (`nil`/`false` false; `0`/`""`/`[]`/`0.0` true).
- `05-print`: multi-arg space-join, nil, bare `(print)`,
  nested-vector human printing.
- `06-err-quot0`, `07-err-nth`, `08-err-count`,
  `09-err-arity`: abort-path pins (each error class +
  the stdout flush).

## Regression test

`rust/tests/build_corpus.rs` replays every file: supported
via build+execute vs `run` goldens; unsupported via exact
`build` diagnostics + `run` exit-0 goldens; errors via
exact AOT diagnostics + exit codes (runs on CI with LLVM;
no JVM needed).
