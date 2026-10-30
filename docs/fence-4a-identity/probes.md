# Fence 4a oracle probes (JVM Clojure 1.12.3)

All run with `java -cp /tmp/clojure-1.12.3.jar:/tmp/spec.alpha.jar:/tmp/core.specs.jar clojure.main`.
Fresh boxes come from arithmetic (literals would share constants).
Each result is pinned by a test in `rust/crates/clj-eval/tests/classemu.rs`
(`identical_*`), `tests/concurrency.rs` (`cas_identity`), the
`classemu.rs` unit test `identical_unreachable_kinds`, or a
`clj-value` unit test.

## P1 — boxing caches (`identical_caches`)

Longs (`LongCache`, -128..127):

- `(identical? (+ 100 27) (+ 100 27))` → `true`
- `(identical? (+ 100 27) (+ 120 7))` → `true` (127)
- `(identical? (+ 127 1) (+ 100 28))` → `false` (128)
- `(identical? (+ -100 -28) (+ -120 -8))` → `true` (-128)
- `(identical? (+ -100 -29) (+ -120 -9))` → `false` (-129)
- `(identical? (+ 999 1) (+ 500 500))` → `false` (1000)
- `(let [n (+ 999 1)] (identical? n n))` → `false` (unboxed
  locals rebox per use — outside the cache there is no object
  to share, not even for the same binding)
- `(let [n (+ 100 27)] (identical? n n))` → `true`
- `(identical? (+ 100 27) 127)` → `true` (mixed fresh/literal)
- `(identical? 128 128)` → `false` (literals box fresh per
  occurrence — no constant pooling for numbers)
- `(identical? 1N 1N)` → `false` (deferred: bigs stay `=`)

Chars (`Character` cache, `\u{0}`..`\u{7f}`):

- `(identical? (char 97) (char 97))` → `true`
- `(identical? (char 127) (char 127))` → `true`
- `(identical? (char 128) (char 128))` → `false`
- `(identical? (char 200) (char 200))` → `false`
- `(let [c (char 200)] (identical? c c))` → `false` (rebox rule)
- `(identical? \a \a)` → `true` (literals via `valueOf`)

Doubles (never cached):

- `(identical? (+ 1.0 0.0) (+ 1.0 0.0))` → `false`
- `(identical? 0.0 -0.0)` → `false`
- `(let [d Double/NaN] (identical? d d))` → `false` (reboxes)

Cross-kind: `(identical? 1 1.0)` → `false`,
`(identical? :a "a")` → `false`.

## P2 — strings (`identical_strings`)

- `(identical? "ab" "ab")` → `true` (literals pool — the JVM
  interns class constants globally)
- `(identical? (str "a" "") (str "a" ""))` → `false` (fresh)
- `(identical? (.substring "abc" 0 1) (.substring "abc" 0 1))` →
  `false` (fresh; port pins `(subs ...)` instead — no interop)
- `(let [s (str "a" "")] (identical? s s))` → `true`
- `(identical? (quote "ab") "ab")` → `true` (the quote answers
  the interned constant)

Note: `(identical? (str "a") (str "a"))` → `true` is a test
artifact (`str` of one string returns its argument); the fresh
rows above use two-arg `str`.

## P3 — heap objects (`identical_heap`, `identical_unreachable_kinds`)

Same binding `true`, distinct-but-equal `false`:

- symbols: `(identical? (symbol "a") (symbol "a"))` → `false`
  (even `(identical? 'a 'a)` → `false` — the reader news per
  read); same binding → `true`
- keywords intern: `(identical? (keyword "a") (keyword "a"))` →
  `true`, namespaced likewise
- uuid/uri/inst: fresh → `false`, same binding → `true`
- regex: `(identical? #"a" #"a")` → `false` (fresh `Pattern`
  per occurrence — no pooling); same binding → `true`
- arrays: `(identical? (int-array [1]) (int-array [1]))` →
  `false`; same binding → `true`
- vars: `(identical? #'map #'map)` → `true`
- tagged: `(identical? (tagged-literal 'x 1) (tagged-literal 'x 1))`
  → `false`; same binding → `true`
- fns: `(identical? + +)` → `true`
- `with-meta` news: `(let [s 'a] (identical? s (with-meta s {:x 1})))`
  → `false` (and likewise for vectors)
- `(identical? (first (quote ("ab"))) "ab")` → `true` (nested
  quote payload interns)

## P4 — CAS reference semantics (`cas_identity`)

- `(let [a (atom 5)] (compare-and-set! a (+ 2 3) :x))` → `true`
- `(let [a (atom 1000)] [(compare-and-set! a (+ 999 1) :x) @a])`
  → `[false 1000]` (misses despite `=`)
- `(let [a (atom :x)] (compare-and-set! a (keyword "x") :y))` →
  `true`
- `(let [s (symbol "q")] (let [a (atom s)] (compare-and-set! a (symbol "q") :y)))`
  → `false`
- `(let [a (atom 1.5)] [(compare-and-set! a 1.5 :x) @a])` →
  `[false 1.5]` (doubles miss)
- `(let [a (atom (str "a" ""))] [(compare-and-set! a (str "a" "") :x) @a])`
  → `[false "a"]` (fresh strings miss)
- `(let [a (atom "a")] (compare-and-set! a "a" :x))` → `true`
  (pooled literals hit)
- validator runs even on a miss: construction + attempt = 2 calls
- no watch fires on a miss

## P5 — deferred big-number identity (post-tower record)

`BigDecimal` results canonicalize through `BigDecimal.valueOf`
(`ZERO_THROUGH_TEN`: scale-0 values 0..10 share — probed across
`+`, `-`, `*`, `/`, and raw `.add` interop); bare `1M` literals
stay fresh (`(identical? 1M 1M)` → `false`), negatives and
nonzero scales stay fresh. `BigInteger` never canonicalizes
(`(+ 1N -1N)`, `(* 999N 0N)`, `(bigint 0)` all fresh). Ratios
always fresh. The deferred Arc-box must reproduce exactly this
(canonical-share for valueOf-routed scale-0 0..10, fresh
otherwise); until then bigs stay `=`-compared.

## P6 — unreachables (Rust-level pins)

`Instant`, `Uri`, and `ReaderConditional` have no Clojure
surface in the port (upstream has no URI constructor either);
their fresh-false/same-true rules are pinned by
`identical_unreachable_kinds` calling `identical_values`
directly.
