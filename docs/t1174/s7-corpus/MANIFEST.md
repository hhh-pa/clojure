# S7 transcript corpus MANIFEST

4 scripted REPL sessions (`.in`) with both sides' transcripts:
`expected/NN-topic.ours.out` (streaming `repl`, byte-exact) and
`expected/NN-topic.jvm.out` (piped `clojure.main`, raw).
Truth source: `s7-pins.md` (P1–P7).

## Oracle

From the repo root:

```
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:$HOME/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:$HOME/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar:$HOME/.m2/repository/org/clojure/test.check/1.1.3/test.check-1.1.3.jar"
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp "$CP" clojure.main < docs/t1174/s7-corpus/NN-topic.in \
  > docs/t1174/s7-corpus/expected/NN-topic.jvm.out 2>&1
```

Ours: `clojure-rs repl --no-history < NN-topic.in`.

## Differential rule

`normalize.py` strips the ambient JVM differences pinned in P6
(banner, `<ns>=> ` prompts) and non-deterministic `Full report
at:` lines. After normalization:

- `01-basics`, `02-multiline`, `04-ns`: byte-identical to
  `expected/*.ours.out` (verified at commission, both sides
  stability re-run).
- `03-errors`: ours-pinned ONLY. Error text is a documented
  format divergence (one-line `Class: message` vs the JVM's
  `Execution error ... at ...` + report file); the session
  still proves session-survival, `*e` binding, and `*1`
  preservation on both sides (same shapes, different prose).

## Sessions

- `01-basics` (7 forms): values, `def`, `*1` shifting incl.
  nil, strings. Differential.
- `02-multiline` (2 units): paren continuation across lines,
  `do` + inner print. Differential.
- `03-errors` (6 forms): runtime throw, `*1` preserved
  (→ `3` differential, read before `:e?` shifts the window),
  `:e?`, unmatched `)`, unresolvable symbol.
  Ours-pinned (see above).
- `04-ns` (3 forms): `ns-name`, `in-ns` + switch marker,
  qualified `ns-name` in the bare ns. Differential.
  (Raw `(in-ns 'foo)` is unpinnable — the Namespace print
  carries an identity hash — so the script swallows it in
  a `do`. An earlier script version held bare-ns forms whose
  `Unable to resolve` matched the JVM — observed during
  commissioning, before the qualified rewrite; the current
  script holds none.)

## Regression test

`rust/tests/repl_transcripts.rs` replays every `.in` through the
built binary and asserts `expected/*.ours.out` byte-exact (runs
on CI, no JVM needed). Re-verify the JVM side on demand with
`normalize.py` per the recipe above.
