# I214 JIT-subset corpus MANIFEST

TDD-first corpus for the #214 subset expansion (spec
`../spec.md`, pins `../pins.md`). Lives outside `docs/t1174`
(the T1174 contract set is frozen; its `02-loop`/`10-when`
fixtures are superseded but byte-kept there by the
t1174-empty-diff rule — the live loop/when corpus is here).

## Layout

- `11-loop`, `12-when`: supported programs.
  `expected/NN.out` = stdout goldens (seeded from `run`,
  exit 0, stderr empty); `expected/jvm/NN.jvm.out` = JVM
  script-mode goldens, byte-identical at commission
  (triple-grounded: interpreter == JVM; AOT must match
  both — `11-loop.jvm.out` via Clojure 1.11.3,
  `12-when.jvm.out` via Clojure 1.13.0-alpha10).
- `unsupported/02-loop`: `loop` with destructuring (still
  out — names `loop`); plain-symbol loops compile (see
  `11-loop`). `.runout` = `run` stdout (exit 0).

## Oracle recipes

From the repo root (`B=rust/target/debug/clojure-rs`):

```
# Seed interpreter goldens:
$B run docs/i214-jit-subset/corpus/NN-topic.clj > expected/NN-topic.out
# JVM cross-check (script mode — `-i/-e` loses *out* on System/exit):
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:..."
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp "$CP" clojure.main NN-topic.clj > expected/jvm/NN-topic.jvm.out
# AOT check:
$B build NN-topic.clj -o /tmp/i214/NN && /tmp/i214/NN | diff - expected/NN-topic.out
```

## Differential rules

- Supported: AOT binary stdout byte-identical to
  `expected/NN.out`, exit 0, stderr empty.
- Unsupported `02-loop`: `build` exit 1, stderr exactly
  `{path}:1:8: unsupported form in AOT subset: loop
  (destructuring is outside the subset; bind plain
  symbols)`; `run` exit 0 with `.runout` stdout.

## Programs

- `11-loop`: `loop`/`recur` (countdown, accumulation,
  rebinding order, nesting, `defn` bodies).
- `12-when`: `when`/`when-not` (branch taken/skipped,
  multi-body, nil results).
- `unsupported/02-loop`: `loop` with destructuring (still
  out — names `loop`); plain-symbol loops compile (see `11-loop`).

## Regression test

`rust/tests/build_corpus.rs` (`i214_*` tests) replays every
file: supported via build+execute and `jit` vs `run`
goldens; unsupported via the exact `build` diagnostic +
`run` exit-0 golden (runs on CI with LLVM; no JVM needed).
