# Future work program — what needs to be done

Owner direction (2026-10-30): there are no non-goals. The deleted
§1.2 items are goals now (N1 by emulation, N5 as epics); binary JVM
compatibility is the one declined item. This spec is the narrative;
issue #42 is the live checkbox tracker — on conflict about status,
#42 and the issues win.

## 1. Goals

- G1. Land the queue; keep master green (merge-when-green).
- G2. Zero divergences: close every follow-up, disposition the
  reader-contract §24 residue, close #42.
- G3. N1 emulation program (#120): port-native host-interop
  solutions. No bridging to a user-provided JVM — no JDK/JVM at
  runtime, ever.
- G4. N5 epics in priority order: JIT (#121) → spec (#122) →
  async (#123) → tools.deps (#124) → `#=` reader (#125).
- G5. Release sync with upstream (#107): when upstream releases,
  we release.

## 2. Scope inventory

### 2.1 Landing queue (in order)

#99 (peer rebase + coverage fix pending) → #100 (approved;
rebase + re-green after #99) → #109 (approved) → #111
(approved-conditional).

### 2.2 Backlog slices in flight

#102 (compare CCE text), #108 (MIN-side typing), #103
(inst/uuid cutover) — agents out, PRs to review + land.
#128 (Windows await-hang test race, master red) — fix-forward,
owner TBD.

### 2.3 Backlog unclaimed

#94 (default data readers), #96 (pool replacement + lifecycle),
#101 (big-number identical?/CAS remainder), #106 (4b edges;
after #100), #110 (analyzer macro bypass; after #100), #112
(destructure edges; after #111), #114 (locking exit matching).

### 2.4 N1 emulation program (#120)

Residue: real host construction, dotted invocation on host
objects, `import` of real JVM classes, reflection over real
host classes, `clojure.java.*`, `clojure.reflect`. Already
done (baseline, not to redo): class emulator, port-native
types/protocols/multimethods (#33), `(new Object)`, `..`
surface, emulated `import`, precise host-form rejections.

### 2.5 N5 epics (priority order)

- #121 LLVM JIT (AOT exists; add just-in-time execution).
- #122 clojure.spec port (exact library scope TBD in-issue).
- #123 core.async port (channels + go blocks).
- #124 tools.deps port (deps.edn resolution; scope TBD).
- #125 `#=` eval reader (honor `*read-eval*`; flip §24 item 2).

### 2.6 Declined

- #126 binary compatibility with JVM artifacts — owner: no
  JVM compatibility (t1192 N2 records the stance).

### 2.7 Open decisions

- N2: var matrix complete (691/691, zero residue); §24 items
  4–11 have no issues — umbrella triage issue proposed, not
  filed. Accept permanently or split?
- N1 sub-slice breakdown (epic #120 needs slicing before work).

## 3. Working agreements (pointer)

Review loop with independent read-only reviewers,
green-before-push, merge-when-green; landing queue order;
coordination via the storyboard + discussion #88 with signed
`[session]` claims. CI: 8-way sharding won the measured trial;
no second CI system; no self-hosted runners (no hardware).

## 4. Acceptance

Master green; queue empty; every follow-up closed or
dispositioned; §24 residue dispositioned; #42 closed; port
releases track upstream releases.
