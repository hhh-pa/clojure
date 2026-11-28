# Issue #273: `clojure.string/replace` and the `Matcher` emulation follow the JDK

## Problem

`clojure-rs` (`run`, `jit`, `build`) diverged from JVM Clojure in pattern replacement:

- `str/replace` handed string replacements to the regex crate, which reads its own template syntax (`$name`, `${n}`) instead of the JDK's. `\$` stayed `\$`, `$10` with one group read group 10 (empty), and bad references were silently dropped.
- Every match iteration (`replace`, the fn form of `replace`, `re-seq`, `split`) used the crate's iterators. These skip an empty match that starts right where a non-empty one ended; `Matcher.find` reports it. So `(str/replace "aaa" #"a*" "-")` gave `"-"`, not `"--"`.
- `string.clj`'s `replace-first-by` (fn replacements in `replace-first`), and `replace-by` once the native fast path is left, need `StringBuffer(int)`, `Matcher.appendReplacement`, `appendTail`, `replaceFirst` and `quoteReplacement`. None were emulated, so those calls threw.

## Behavior (the JDK's, oracle: JVM Clojure 1.13.0-alpha10 on JDK 26)

**`find` order.** After an empty match, the next search starts one character later. After a non-empty match, it starts right at its end, and an empty match there counts.

**Replacement grammar** (`Matcher.appendReplacement`):

| Form | Meaning | Errors |
| --- | --- | --- |
| `\c` | the character `c` | a trailing `\` is `IllegalArgumentException: character to be escaped is missing` |
| `$n` | the longest group number that stays legal | a missing index or a non-digit is an `IllegalArgumentException`; a number past the last group is `IndexOutOfBoundsException: No group N` |
| `${name}` | the named group | the JDK's four `IllegalArgumentException` texts: empty name, missing `}`, leading digit, unknown name |

An unmatched group appends nothing. A template is only read once something matches, so a bad template over an input with no match returns the input unchanged.

**Fn replacements.** The fn's answer must be a `String`, because it goes through `Matcher/quoteReplacement`. Anything else is a `ClassCastException`; nil is the quote's NPE.

**`Matcher` members:**
- `replaceAll` and `replaceFirst` reset the matcher, then run the JDK's own append loop, so the handle ends in the JDK's state.
- `appendReplacement(sb, rep)` needs a current match (`IllegalStateException: No match found`), then a buffer (nil is the JDK's NPE). It then appends the text from the append position to the match, followed by the expansion:
  - A template `IllegalArgumentException` rolls the buffer back.
  - Any other failure leaves what was appended, as the JDK's `setLength` rollback covers `IllegalArgumentException` only.
  - Calling it twice for one match is the JDK's range `IndexOutOfBoundsException`, with UTF-16 indices.
- `appendTail(sb)` appends the rest and returns `sb`.
- `Matcher/quoteReplacement` quotes `\` and `$`.

**`StringBuffer`.** `StringBuffer`, `StringBuffer(int)` and `StringBuffer(CharSequence)` build a buffer with `StringBuilder`'s members and the class `java.lang.StringBuffer`. Nil is the `(CharSequence)` overload's NPE.

## Design

`clj-eval/src/jdkmatch.rs` holds the shared pieces:
- the `find` iterators (`finds`, `capture_finds`);
- the `splice` loop;
- the `Template` parser and expander;
- `replace`, `quote_replacement` and `replacement_string`.

It is used by `re.rs` (`replace_all_fast`, `split`, `re-seq`), by `stdlib_natives.rs` (the fn form of `replace`), and by the `Matcher` handle (`collmisc.rs`), which also gains an append position.

Patterns that can never match the empty string keep the crate's iterators: both orders agree there, and the crate's are the fastest path. Whether a pattern can match empty is read once per compiled pattern from `regex-syntax`'s `minimum_len`; that crate was already in the tree through `regex`. Only patterns that can match empty step with `find_at`.

## Acceptance

- **Oracle test.** `crates/clj-eval/tests/string_replace_273.rs` replays 93 JVM-oracle rows from `probes/{replace,interop}.clj` against their JVM answers in `probes/*.out`. These cover every form above, `re-seq`/`split` with empty-capable patterns, and the interop edge cases.
- **Compiled path.** The i259 corpus program `clojure_string_replace.clj` gained the same shapes, so `rust/tests/native_corpus.rs` checks `jit` against `run`.
- **Gates.** Full quality gates green: 100% coverage, complexity 0, geiger at baseline.
- **Sweep.** The snippet sweep matches 1754/1754.

## Out of scope (pre-existing, not replacement semantics)

- **`e21`:** `(str/re-quote-replacement nil)` gives the JVM's NPE text for `string.clj`'s `^CharSequence`-hinted `.toString`. This runtime words NPEs on hinted interop calls generically.
- **`StringBuilder`:** `(StringBuilder. nil)` reports an overload ambiguity, while JDK 26 reaches the `(CharSequence)` NPE.
