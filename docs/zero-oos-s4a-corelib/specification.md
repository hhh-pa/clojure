# S4a corelib small vars (zero-oos slice 4a)

Twelve upstream vars over existing machinery, no new value
kinds: `some->` `some->>` `when-first` `while`
`parse-boolean` `parse-double` `parse-long` `printf`
`println-str` `prn-str` `rand-nth` `test`.

Target universe rows come from the landed 692-row inventory
(`rust/core-coverage.md`, via PR #41); this slice flips its 12
rows to `implemented` in place (header counts = actual).

## Semantics

Macros (`core.clj`, beside the implemented `cond->`/`when`;
`~'if` bare per the file's specials rule, `let`/`loop`/
macro heads qualified-by-syntax-quote as usual):

- `some->` — upstream shape: one `let` over a gensym, one
  step per form, each step `(if (nil? g) nil (-> g step))`;
  nil short-circuits the rest. Zero forms answers the expr.
- `some->>` — same with `->>`.
- `when-first` — upstream shape: `assert-args` (vector
  binding, exactly 2 forms) then `(when-let [xs# (seq xs)]
  (let [x (first xs#)] body...))`; `xs` evaluates once.
- `while` — upstream shape: `(loop [] (when test body...
  (recur)))`; answers nil.

Natives (`core.rs` + `CORE_NATIVES` rows; 1-arity with the
house arity guard; arity misses throw the house text):

- `parse-boolean` — `"true"` → true, `"false"` → false,
  any other string → nil. Non-strings throw IAE with the
  upstream `parsing-err` text (`Expected string, got nil`
  for nil, else `jvm_class_of` — the port's class-name
  table already mirrors the JVM).
- `parse-long` — `Long/valueOf` or nil on `NumberFormat`
  (optional single sign, Unicode Nd digits, overflow → nil).
  Oracle settles non-ASCII digits, blank/empty shapes, and
  the non-string IAE text.
- `parse-double` — `Double/valueOf` or nil on `NumberFormat`
  (trim, `NaN`/`Infinity`, decimal + hex floats, `d`/`f`
  suffixes, overflow → `##Inf`). Oracle settles spelling
  case, suffix-on-`NaN`/hex edges, and trim width.

Fns (`core.clj` one-liners over implemented vars):

- `printf` — `(print (apply format fmt args))`.
- `println-str` — `(with-out-str (apply println xs))`.
- `prn-str` — `(with-out-str (apply prn xs))`.
- `rand-nth` — `(nth coll (rand-int (count coll)))`;
  oracle settles the nil/empty/string shapes.
- `test` — `(let [f (:test (meta v))] (if f (do (f) :ok)
  :no-test))`; the metadata fn's value is discarded, its
  throw propagates.

## BDD scenarios

- Given `(some-> {:a {:b 42}} :a :b)` / `(some-> nil :a
  :b)` / `(some-> {:a nil} :a :b)`, threading answers
  `42` / nil / nil (later steps never run on nil).
- Given `(some->> [1 2] (map inc) (filter odd?))`-style
  last-position steps, threading matches `->>` per step.
- Given `(when-first [x [1 2]] x)` / `(when-first [x
  nil] :body)` / a non-vector or 1/3-form binding, the
  answers are `1` / nil / the stock assert text, and `xs`
  evaluates exactly once.
- Given `(while (pos? @a) (swap! dec))`-style loops, the
  body runs until the test fails and the form answers nil.
- Given each parse battery string (digits, signs,
  overflow, blanks, hex, `NaN`/`Infinity` spellings,
  suffixes, junk) and non-string (nil, int, bool, kw),
  each parse answers the oracle's value/throw verbatim.
- Given `(printf "%s=%d" "a" 1)` / `(println-str 1 nil
  "x")` / `(prn-str 1 nil "x")`, output matches the
  oracle's exact strings (newline policy included).
- Given `(rand-nth [1 2 3])` ×N, every draw is a member;
  nil/empty/string shapes replay the oracle.
- Given a var with/without a `:test` metadata fn, `test`
  answers `:ok` / `:no-test` (fn value discarded).
- Given 0 or 2+ args, each native throws the house arity
  text (pinned).

## Tests

`tests/zero_oos_s4a_corelib.rs`: one test per var replaying
its probe labels verbatim, plus a shared arity-miss test.
RNG pins are shape/range only. Matrix `notes` name the
tests.

## Non-goals

`parse-uuid` (S9 scalar kinds), `find-var`/
`requiring-resolve` (S5), `even?`-style vocabulary gaps
(none expected — every head used above resolves today),
print-dup behavior (S8), `with-redefs`-style test
fixtures (use plain metadata fns).
