# F-read-opts (issue #40): `read-string` opts arity + coverage-universe repair

## Part 1 — Contract

### 1a. `(read-string opts s)` (code)

JVM `read-string` has arities `([s] [opts s])`; the port has
only `[s]` (`io.rs: native_read_string`, arity `[1]` — a
2-arity call fails arity today). Add the opts arity:

- The accepted keys, key validation, and error behavior are
  **identical to the port's `(read opts stream)` 2-arity**
  (`reader.rs: parse_read_opts`) wherever the oracle agrees;
  where JVM `read-string` differs from JVM `read`, the
  oracle wins and the difference is pinned below.
- New probes in `docs/i40-readopts/probes/read-opts.clj`
  (+ `.out`, oracle-generated) pin every key and error; the
  port matches the oracle row for row:

| probe id | pins |
|---|---|
| `rs-opts-none` | 1-arity unchanged (control) |
| `rs-opts-empty` | empty opts map behaves as 1-arity |
| `rs-opts-cond-allow` | `:read-cond :allow` reads `#?` with `:features` |
| `rs-opts-cond-deny` | `:read-cond false`/absent denies `#?` (default) |
| `rs-opts-cond-preserve` | `:read-cond :preserve` yields reader-conditionals |
| `rs-opts-features` | `:features` selects `#?` branches; unknown feature skipped |
| `rs-opts-eof` | `:eof` on empty/blank input (oracle decides: value or throw) |
| `rs-opts-unknown-key` | unknown key ignored or rejected per oracle |
| `rs-opts-nil` | nil opts (oracle decides) |
| `rs-opts-nonmap` | non-map opts error (mirror `read` unless oracle differs) |
| `rs-opts-badcond` | bad `:read-cond` value error (mirror `read` unless oracle differs) |

- `read-string` keeps its existing traits: no line numbers,
  bare EOF errors, one form only. `(read ...)` is untouched.
- The `read-string` row in `rust/core-coverage.md` gains the
  opts note (`2-arity opts; probes rs-opts-*`).

### 1b. Coverage-universe repair (rows; no behavior change)

The audit (`audit/report.md`, inputs `nspublics.txt`,
`universe.txt`, `missing.txt`, `stale.txt`, `meta.tsv`,
`resolves.tsv`, per-var `triage.tsv`) proves the 422-row
universe omits 270 JVM publics. Add **269 rows** (all of
`triage.tsv` except `defprotocol`, owned by #33 — §2d):

| triage label (count) | file status | note convention |
|---|---|---|
| `implemented` (5) | `implemented` | mechanism + test citation (grep-verified) |
| `planned` = ready (53) | `out-of-scope` | `future: <one-line plan>` |
| `out-of-scope` = never (100) | `out-of-scope` | hard reason (`N1`, `N4`, never, …) |
| `needs-design` (111 of 112) | `out-of-scope` | `pending design: <seam>; see #40` |

- Ready and pending-design rows are `out-of-scope`, **not**
  `planned`: the matrix test fails on any unresolved
  `planned` row (TDD red), so merged `planned` rows must
  resolve — none of these do. The note carries the bucket.
- Placement: three new sections at the end of the file
  (`Previously unlisted — resolves today` /
  `— accepted future work` / `— no slice yet`), rows
  alphabetical within each section, exact shape
  `| var | status | notes |` with the var backticked.
- Header: counts become **691 vars: 390 implemented,
  2 divergent, 299 out of scope, 0 planned**; the intro gains
  one line explaining the 7 extra-universe rows (4
  port-visible JVM privates, 1 port-only var, 2 guard rows)
  and one line stating merged `planned` rows must resolve
  (the matrix fails otherwise — add none unresolved).
- The 7 stale rows are all deliberate (§2c): keep untouched.
- Verification: `coverage_matrix` stays green (it auto-covers
  new rows); row↔triage parity is a one-off reviewer script
  (quoted in the PR), not a new test.

## Part 2 — Notes

### 2a. Derivation story

The 422-row universe is hand-curated: created at the
squash-import root (`63b2aa54`, 380/2/40), then #36 flipped 5
rows (`72992dc`, 385/2/35). No generator exists anywhere
(coverage gates are LCOV-only; nothing references
`ns-publics`); no rationale was ever written down. The gap is
not version skew: the port reports `1.13.0-alpha9`, the audit
oracle is alpha10, but <5% of the missing vars are
1.12/1.13-new — 1.0-era vars (`while`, `run!`, `bit-and`,
`update-vals`, …) were simply never listed.

### 2b. Audit method

`ns-publics` on the alpha10 jars → 685 names (corroborated by
`count` = 685, 685 distinct lines, 0 dupes; `ns-interns` =
808); file parse → 422 rows (header counts re-verified
exact); diff → 270 missing / 7 stale; booted-core `resolve`
probes (same logic as the matrix) → 5 silent resolvers with
passing controls. All inputs are committed under `audit/`.

### 2c. Stale rows (keep all 7)

`*loaded-libs*`, `*loading-verbosely*`, `*pending-paths*`,
`preserving-reduced` are JVM-private but port-visible;
`*load-path*` is port-only (dirs vector); `clone-thread-
bindings` and `pprint` are documented guard rows.

### 2d. `defprotocol` carve-out + shared-file note

`defprotocol` is missing from the 422 but implemented on
`origin/33-typechain` (their branch adds its row — verified
their only added row). #40 skips it; #33 owns it. Both
branches edit `core-coverage.md`: whoever lands second
resolves the (textual, disjoint-line) conflicts.

### 2e. Follow-ups (not this slice)

Ready-53 implementations (future slices, one per area);
pending-design seams (`*read-eval*`, `read+string`,
`load-reader`, tag table, hierarchy, print dispatch, …);
nothing here changes any `planned`/`divergent` semantics.
