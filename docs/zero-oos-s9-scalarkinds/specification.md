# S9 scalar kinds (zero-oos slice 9)

Eight upstream vars over three new scalar kinds: `parse-uuid`
`random-uuid` `uuid?` `uri?` `Inst` `inst-ms` `inst-ms*` `inst?`.
UUID is 128 bits, Instant is millis-since-epoch, URI is a validated
string plus parsed parts. All three get `=`/`hash`/`compare` parity
with the JVM and JVM print shapes, pinned by the alpha10 probes in
`probes/` (`uuid.out`, `inst.out`, `uri.out`, `edges.out`,
`uriparts.out`, `uriedges.out`).

Target universe rows come from PR #41's inventory; this slice adds
its 8 rows to the wave-2 matrix's single
`### Previously unlisted — accepted future work` alpha table,
flipped straight to `implemented` (+8 rows; header counts = actual
at land).

## Kinds (`clj-value`, new `scalar.rs`)

- `Value::Uuid(Uuid)` — `{ hi: u64, lo: u64 }` (most/least bits).
  `uuid_from_str` replicates `UUID.fromString` exactly: the
  length-36 fast path plus the lenient slow path (five
  dash-separated all-hex groups, each fitting `i64`, masked to
  32/16/16/16/48 bits — probes `u-parse-longshort`,
  `u-parse-short1`, `u-parse-shortmid`; over-`i64` groups fail —
  `u-parse-long1`). `toString` is lowercase canonical.
  `random` draws two `u64` from the `rand` crate (the `rng.rs`
  source) and stamps version 4 / Leach-Salz variant bits
  (`u-rand-ver`, `u-rand-var`).
- `Value::Instant(Instant)` — `{ millis: i64 }`. Construction from
  an `#inst` payload reuses the S6 grammar validator
  (`validate_inst`: proleptic leap rules, padding, leap-second-60
  gate) and computes millis through a `GregorianCalendar`
  emulation: default lenient, default cutover (1582-10-15,
  `-12219292800000`), the payload zone (missing zone is GMT —
  `construct-calendar`, probes `i-ms-nozone`, `i-ms-dayzone`).
  Consequences, all probed: the Oct 5–14 1582 gap rolls +10 days
  (`e-cut-gap*`); pre-cutover days count Julian
  (`i-ms-year1`, `e-cut-1500`, `e-cut-1000`); `1500-02-29` fails
  validation although Julian-1500 is a leap year
  (`e-cut-1500leap`); `:59:60` rolls one minute (`i-ms-leapsec`);
  fractions truncate to millis (`i-ms-frac-round`).
- `Value::Uri(Uri)` — `{ text, scheme, authority, user_info, host,
  port, path, query, fragment, opaque, id }`: the validated
  original string plus raw (never decoded) parts plus a
  construction id for the `#object` hex (the `DELAY_ID`
  precedent). `uri_parse` ports the JDK single-arg parser:
  RFC 2396 char masks (copied hex longs, RFC-commented like the
  Murmur constants), opaque-vs-hierarchical split, empty
  authority collapsing to null (`c-emptyauth`), server parse with
  registry fallback (`c-negport`, `c-badport`, `c-multi-at`,
  `g-underscore`), strict bracketed IPv6 (grammar ported from
  `parseIPv6Reference`, `%scope` included — `c-scope`), and the
  non-ASCII rule (`c > 0x80`, not space, not control —
  `c-nonascii` vs `c-del`/`c-c1`/`c-nbsp`). Invalid input fails
  with the `URISyntaxException` reason/index/input triple
  (`e-bad-*`, `g-*`).

The three ctors are `pub` (usable across crates): `uuid_from_str`,
`uuid_random`, `instant_from_str`, `uri_parse`. The issue says
"`pub(crate)` ctors for S6"; they must be `pub` because the S6
default readers live in `clj-eval` while the kinds live in
`clj-value` (same visibility as S6's own `validate_inst` /
`validate_uuid`).

## Equality, hashing, ordering

- `=`: UUID by 128 bits (`u-eq-case`); Instant by millis
  (`i-eq-zone`); URI by the JDK `equals` algorithm (opaque flag,
  scheme/host case-insensitive, everything else case-sensitive
  except `%`-hex normalization — `r-eq-*`, `e-uri-escape`,
  `e-uri-regcase`, `e-uri-ipv6case`). Cross-kind and
  scalar-vs-other comparisons are false (`u-eq-str`, `i-eq-str`,
  `r-eq-str`).
- `hash`: `UUID.hashCode` (`(msb ^ lsb)` folded —
  `u-hash-a/b/c`), `Date.hashCode` (`t ^ (t >>> 32)` —
  `i-hash-a/b/c`), `URI.hashCode` (`hashIgnoringCase(scheme)`,
  fragment, path, query, userInfo, host, `1949 * port`, or the
  registry authority — `r-hash-*`, `e-uri-hash-*`,
  `c-hash-*`). All 20 pinned hash ints replay bit-exact.
- `compare`: same-kind only — UUID by signed `(msb, lsb)`
  (`u-cmp-signed`), Instant by millis, URI by the JDK `compareTo`
  component order (scheme, opaque flag, ssp/authority, path,
  query, fragment — `r-cmp-*`, `e-cmp-*`). Cross-kind pairs fail
  the checkcast of `y` to `x`'s class through the existing
  `jvm_compare` targets (`java.util.UUID` / `java.util.Date` /
  `java.net.URI` — `u-cmp-str`, `i-cmp-str`, `r-cmp-str` pin the
  texts). Like every port ordered pair, same-kind results
  normalize to -1/0/1 (the JVM returns raw char/port diffs such
  as `r-cmp-scheme` -2 and `r-cmp-port-vs-noport` -81; the port
  maps `Ordering`, so the sign is what replays).

## Printing and `str`

- `pr`/`print` (both `readably` modes — the JVM print-methods are
  unconditional): `#uuid "<canon>"` (`u-pr`, `u-print`),
  `#inst "<UTC yyyy-MM-dd'T'HH:mm:ss.SSS-00:00>"` (the
  `SimpleDateFormat` emulation: GMT, cutover-aware fields, year
  ≤ 0 rendered as era-shifted `1 - year` — `i-pr-*`,
  `e-cut-pr-year0`, `e-cut-pr-1500`, `e-cut-proleptic`), and
  `#object[java.net.URI 0x<id-hex> "<text>"]` (quoted under `pr`,
  bare under `print` — `r-pr`, `r-print`; the hex is the
  construction id, a deterministic stand-in for identity hash).
- `str`: UUID/URI render `toString` bare (`u-str`, `r-str`);
  Instant renders `Date.toString` emulated at UTC
  (`EEE MMM dd HH:mm:ss zzz yyyy`, US names, zero-padded day,
  unpadded era-shifted year, no millis, cutover-aware fields —
  `i-str-day`, `i-str-pre`, `e-str-*`). The JVM honors the
  default timezone here; the port is UTC-fixed (as the oracle
  runs), documented below. `str` needs its own scalar arms
  because `print` stays tagged while `str` goes bare.

## The `Inst` protocol and the 8 vars

`core.clj` (after the `defprotocol` machinery) defines the
protocol exactly like upstream core:

```clojure
(defprotocol Inst (inst-ms* [inst]))
(extend-protocol Inst java.util.Date
  (inst-ms* [inst] (clojure.core/__inst_millis* inst)))
(defn inst-ms [inst] (inst-ms* inst))
(defn inst? [x] (satisfies? Inst x))
```

- `Inst` is a real `#33` protocol map; `inst-ms*` is the
  macro-generated variadic dispatch fn. The `java.util.Date`
  extension key rides the dotted-name rule and matches
  `jvm_class_of(Value::Instant)`.
- `__inst_millis*` is a hidden native (the `__protocol*`
  precedent) answering the millis; user types extend `Inst`
  through the ordinary machinery (`i-ext-str`, `i-ext-deftype`
  replay against port-defined types).
- `parse-uuid`: `[^String s]` semantics — nil is the verbatim
  NPE (`Cannot invoke "String.length()" because "name" is
  null`), non-strings fail the `String` cast, `fromString`
  failures (IAE/NFE) answer nil (`u-parse-*`).
- `random-uuid`: 0-arity, v4 (`e-uuid-ver4`).
- `uuid?` / `uri?`: `pred1!` kind predicates (`u-p-*`, `r-p-*`).

## The S6 boundary (read path NOT flipped)

`#inst` / `#uuid` reads keep answering `TaggedLiteral` on this
slice: S6 owns tag routing, its tests pin the echoing literal
(`inst_default_reads`, `uuid_default_reads`,
`default_readers_direct_calls`), and the issue scopes S9 to
"kinds + printing only" with ctors exposed for S6. Flipping
`default_inst_reader` / `default_uuid_reader` to the new ctors is
a later integration step; the recipe, when it lands (with S6
test updates in the same commit):

1. `default_inst_reader`: after `validate_inst`, return
   `instant_from_str` (infallible post-validation).
2. `default_uuid_reader`: after `validate_uuid`, return
   `uuid_from_str`, mapping the residual `i64`-overflow failure
   (S6-valid, `fromString`-invalid) to the NFE
   `For input string: "<group>" under radix 16`.
3. The read-side `dispatch.rs` valid-payload arms stay
   `TaggedLiteral` (the reader cannot construct values); the
   eval walk resolves them through the flipped defaults.

Until then the issue's `(inst-ms #inst "...")` BDD item cannot
run from Clojure: no core var constructs `Instant` (or `Uri` —
upstream has no URI constructor either). `inst-ms` over real
instants is covered by Rust tests calling the booted `inst-ms`
var with constructed values, plus Clojure tests over user types
extending `Inst`; `uri?` replays the false battery from Clojure
and the kind arms from Rust.

## Divergences from the JVM (all deliberate, all pinned)

- `(inst-ms nil)` ends `found for class: null`; the oracle says
  `nil` (`i-ms-nil`). The text comes from shared `#33`
  machinery (`jvm_class_of`), out of this slice's fence.
- `compare` normalizes to -1/0/1 (above); the `.out` files keep
  the raw JVM ints.
- Instant `str`/`print` render UTC always; the JVM uses the
  default zone. `pr` is zone-free on both.
- `#object` URI hex is the construction id, not identity hash.
- Bad-arity `inst-ms*` calls fail at runtime through
  `__protocol_invoke*`; the JVM fails at compile time
  (`i-msm-arity0`, `i-msm-arity2` — paths in the pins are the
  recording checkout's).
- Upstream alpha10 also extends `Inst` to `java.time.Instant`
  (`i-p-jtime`, `i-sat-jtime`); those values cannot exist in the
  port (no host classes), so within the port's universe only
  `Instant` (+ user extensions) satisfies `Inst`.
- IPv6 validation ports the JDK grammar; Rust-level tests pin
  the common shapes (`::1`, full, mapped, scope, port) and the
  recorded failures — truly exotic literals are best-effort.
- `uri?` over `java.net.URL` is false on both (`r-p-url`); URLs
  likewise cannot exist in the port.

## Tests

- `rust/crates/clj-eval/tests/scalarkinds.rs`: the Clojure-side
  replay — parse/random/predicate batteries, print shapes,
  `=`/`hash`/`compare` over UUIDs, `inst-ms`/`inst?` failure
  modes + user-extension dispatch, `Inst` protocol shape.
- Rust unit tests: millis vectors + cutover + print/`str`
  rendering in `clj-value`; the URI parser/equals/hash/compare
  against every pinned int; `inst-ms` over constructed instants
  (boot + `call`) in `clj-eval`.
- Matrix: 8 rows in `### Previously unlisted — accepted future
  work`, ASCII-sorted by var cell among the table's rows, header
  recounted to actuals.
