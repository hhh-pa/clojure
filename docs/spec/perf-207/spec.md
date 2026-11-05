# Perf-207: close the 3–60x interpreter gap on the recipes suite (spec)

Parent: [#207](https://github.com/hhh-pa/clojure/issues/207) (profiled hotspots
+ 5 ordered fixes — this spec implements that order, extended with my own
measurements). Pure performance: zero behavior change. No JDK/JVM anywhere.

## 1. Baseline (reproduced before any change)

Release binary (`cargo build --release --no-default-features -p clojure-rs`),
local macOS/arm64, recipes suite read-only at
`/Users/hhu/workspace/recipes/clojure` (scripts copied to run; inputs copied
from workstation `~/w-resources/resource/shared`). Host was loaded, so
absolutes are noisy; profile shares are the evidence. All runs exit 0 and
match snip-expected outputs (per-line trailing trim).

Exact case paths:

- small sort: `src/sort/bubble_sort/solution.clj` × `sort/bubble_sort/solution`
  (4 cases): 2.0–2.4 s each.
- suffix-array: `src/string_related/suffix_array/string_function_calculation.clj`
  × same-named resources (11 cases): 2.1 s (case 0) → 18.3 s (case 8).
- disjoint-set: `src/data_structure/disjoint_set/connected_component.clj` ×
  same-named resources (10 cases): 2.6–3.6 s (cases 0–5), 10.8 s (6),
  33.8 s (7), 30.1 s (8), 44.9 s (9).
- dijkstra: `src/graph/shortest_path/dijkstrashortreach.clj` ×
  `graph/shortest_path/dijkstrashortreach` (9 cases): small 2.9–4.5 s,
  425 KB→11.0 s, 364 KB→10.2 s, 779 KB→22.8 s, 818 KB→29.3 s;
  37 MB case 7: 671.1 s wall (387.6 s user, maxRSS 2.9 GB), exit 0,
  output matches (one earlier run was SIGTERMed at 221 s by an outside
  killer; the rerun completed clean).

There is a ~2 s per-run floor (embedded `core.clj` load through the same
interpreter); the fixes below speed that up too.

My own 25 s `sample` flat profile of the 37 MB dijkstra run agrees with #207:
`pthread_mutex_lock/unlock` and the allocator (`malloc`/`free`/zone) top the
profile, then `SipHasher::write` + `hash_one<String>`, `Value::clone` + drop
glue, `Env::get`, `NamespaceRegistry::state`, `SharedRegistry::root_or_unbound`,
`Env::{extend,set,len}`, `bind_pattern`, `destructure`, `pattern_names`,
`Arc<Mutex<Vec<Value>>>` drop, and `pthread_mutex_init/destroy` (mutexes are
built and torn down on the hot path). No scan-cache frames are hot (the F10
string cache holds), but `str_scan` takes a global mutex per `nth` past
index 64 — the parse loop hammers one string, so it joins the mutex crowd.

Why dijkstra-parse is the worst case: `parse-ints` is a `loop`/`recur` over
37 M chars; each char does ~50 global derefs (`+`, `>=`, `nth`, `long`, …)
at one global-mutex + SipHash-String-lookup + double-`Value`-clone each, one
`(nth s i)` at one string-cache-mutex each, and one 7-binding `recur` at
seven full-frame `Env` clones (each cloning every slot `Value` and
`pthread_mutex_init`-ing a fresh mutex) plus per-slot `set` locks.

## 2. Fixes (in #207 order)

### F1. Borrow/`Cow`/`Arc` value flow (largest single item)

Scope to what the profile proves, not a value-model rewrite (`Value` is
already `Arc`-heavy; the measured clone costs are *call-site* overhead):

- F1a. `NamespaceRegistry::root_or_unbound`/`SharedRegistry::root_or_unbound`:
  today clones the whole `VarState` (root + meta map) under the global lock,
  then clones the root `Value` out of the clone. Add a single-clone
  root-only read. Callers that only need `.dynamic` (`eval_binding`) or only
  the root (`with-redefs` save) get narrow accessors instead of `state()`.
- F1b. `Env::get` (F3a's redesign) returns the slot clone without a lock;
  `Env::extend` stops cloning every old slot.
- F1c. `invoke_keyed_name` builds `kw.to_string()` and `invoke_map` builds
  `jvm_class_of(callee)` (a `String`) on *every* keyword/map call — both are
  only used on the arity-error path. Move both into the error arms.
- Explicitly NOT doing: a `Cow<Value>`-through-natives rewrite (invasive,
  unmeasured payoff — every native takes `&[Value]` today and the clones
  that matter are the structural ones above, which this spec kills
  directly). Revisit only if post-fix profiles still show `Value::clone`
  hot.

### F2. `fxhash` + interned symbol IDs for env/registry lookups

- F2a. `rustc-hash` is already a direct dependency of `clj-value`, and
  `clj_value::FxBuildHasher` is already exported and used for the persistent
  collections — so this fix adds **zero new dependencies**. Switch every
  `HashMap`/`HashSet` in `clj-eval/src/env.rs` (`Namespace::{vars,ids,
  aliases,refers,imports}`, `NamespaceRegistry::{namespaces,by_id,removed,
  anon}`, `SharedInner::{alter_locks,protocol_fn_methods}`) and the
  `KEYWORD_TABLE` in `clj-value/src/keyword.rs` from `RandomState`/SipHash
  to `FxBuildHasher`. The measured `SipHasher::write` + `hash_one<String>`
  share drops out directly.
- F2b. Symbol interning: **deliberately left out** (see §4).

### F3. Resolve-and-cache vars; registry fast path off the hot path

Analysis already resolves globals to var ids once (#79); the remaining cost
is per-`deref_var`: thread-bindings scan + global mutex + 3 hashed lookups
+ double clone.

- F3a. `Env` redesign (the `Env::{get,extend,set,len}` + `pthread_mutex_init`
  + `Arc<Mutex<Vec<Value>>>` drop share): replace `Arc<Mutex<Vec<Value>>>`
  (full slot-clone + fresh mutex per `extend`, lock per `get`/`set`/`len`)
  with linked immutable frames —
  `Env { frame: Arc<Frame> }`, `Frame { parent: Option<Env>, base: usize,
  len: usize, slots: Slots }`, `Slots = Shared(Arc<[Value]>) |
  Patchable(Arc<Mutex<Vec<Value>>>)`. `extend` allocates only the new slots
  (no clone of old ones, no mutex); `get` indexes the top frame with no
  lock; `len` is O(1) without a lock. `Patchable` frames exist only for
  `letfn`/named-`fn` self-patching (closures must observe the patch through
  the captured `Arc`); `bind_pattern` is restructured to
  compute-then-extend so it never `set`s. Absolute slot indexing (`Local`)
  is preserved via `base`; missing slots still read `nil`; `Send+Sync`
  preserved (frames cross threads in closures/futures).
- F3b. `bind_pattern` fast path: the overwhelmingly common
  `Pattern::Symbol` single-name case extends with the value directly (new
  `Env::extend_one`, no `Vec` at all) instead of extend-with-nil +
  `pattern_names` Vec + destructure Vec + per-slot `set`. Destructuring
  patterns keep compute-then-extend semantics identical to today
  (`:or` defaults still evaluate against the pre-extension frame — #171).
- F3c. Per-evaluator var-root cache + lineage epoch: `SharedRegistry` gains
  `epoch: Arc<AtomicU64>` (lock-free read); `Evaluator` gains
  `root_cache: HashMap<u64, Value, FxBuildHasher>` + `root_epoch: u64`.
  New `deref_var_cached(&mut self, …)` used by the `Expr::Global` arm (and
  `invoke_var`): thread-bindings check first (unchanged — bindings always
  win), then epoch-guarded cache hit (FxHash u64 lookup + one `Value`
  clone, no lock, no SipHash), else the slow path + insert. Every
  `VarState.root` write in the tree flows through one of the four
  `SharedRegistry::with_*` methods (audited — all `.root =` writes live in
  `with_state_mut` closures; `with_ns_mut`/`with_live_or_removed_mut`/
  `with_handle_mut` hand out `&mut Namespace` so they bump conservatively),
  and each of the four bumps the epoch — so any root change invalidates
  every evaluator's cache on its next deref. `fork_for_thread` starts with
  an empty cache at the current epoch. Insert protocol re-checks the epoch
  after the slow read and skips the insert on a race (a concurrent write
  during the read is returned but never cached). The only theoretical
  divergence from today is a cross-thread root write landing between the
  epoch check and the return of a single `deref_var_cached` call (a stale
  root for that one call); single-threaded behavior is bit-identical, and
  any test asserting cross-thread *eventual* visibility still passes
  (the next deref misses). `deref_var(&self)` stays as the slow path for
  all other callers (zero signature ripple).
- F3d. `str_scan` thread-local memo: the global `STR_SCAN_CACHE` mutex is
  taken per string-`nth` past index 64; parse loops hammer one `Arc<str>`.
  Memoize the last `(Weak<str>, StrScan)` per thread (`Weak` keeps the
  no-pin/no-ABA properties); global cache stays the source of truth on
  miss. Lock-free hit path, identical results.

### F4. `Arc<Expr>`, never clone the AST

`make_closure` clones the whole `Vec<Arity>` AST per closure creation, and
`Expr::LazySeqStar` clones its body `Vec<Expr>` per thunk (`~1%` in the
profile, but issue-listed and low-risk):

- F4a. `FunExpr.arities: Vec<Arity>` → `Arc<[Arity]>` (built once at
  analysis; `make_closure` becomes an `Arc` bump + the existing `Env`
  clone). Mechanical analyzer ripple (`vec![…]` → `vec![…].into()` at
  construction; audit that nothing mutates `arities` post-construction).
- F4b. `Closure.arities: Vec<Arity>` → `Arc<[Arity]>` to match.
- F4c. `Expr::LazySeqStar { body }`: share the body (`Arc<Vec<Expr>>`,
  matching `thunk0_value`'s existing `Arc` parameter) instead of cloning
  per thunk — contained to analyzer construction + the eval arm.
- Explicitly NOT doing: a `Box<Expr>` → `Arc<Expr>` rewrite of the whole
  AST (invasive; the measured cost is exactly the two sites above).

### F5. `smallvec`/buffer reuse for short-lived `Vec`s

`smallvec 1.16.3` is already in `Cargo.lock` via `rpds` (which `clj-value`
depends on), so adding it as a direct dependency of `clj-eval` compiles
**zero new crates** — the dependency is already in the build. (This is the
"tiny dependency allowed only if no in-tree alternative exists" case, and
the in-tree facts are even better: the crate is already built.)

- F5a. `Env::extend_one` (F3b) — no `Vec` at all for the hottest bind.
- F5b. `SmallVec<[Value; N]>` for per-call arg buffers: `Expr::Invoke`
  arg `values`, `Expr::Recur` values (+ `Signal::Recur` payload if clean),
  `apply_arity` values. Size the inline capacity from call-site arity
  distribution (parse loops call 1–3 arg fns; `4` is the starting point,
  confirmed by measurement).
- F5c. Thread-local buffer reuse: left out (§4).

## 3. Acceptance

- Wall-clock (release, `--no-default-features`), before → after on the §1
  cases, local + workstation snip confirmation:
  - disjoint-set cases 7/8/9, suffix-array case 8, dijkstra cases 5/6:
    each **≥ 2x faster** (target 3x+; the fixes compound multiplicatively
    on deref+recur-bound workloads).
  - dijkstra 37 MB case 7: finishes (no timeout) on the workstation;
    target ≤ 3x JVM Clojure wall-clock (8 s JVM nominal).
  - sort cases + all small cases: no slower than baseline (within noise;
    the ~2 s floor must not regress — it should shrink via the same fixes).
  - Zero output diffs on every case (snip byte-compare after per-line trim).
- Gates, all green on the branch: `cargo nextest run -p clj-eval` (full
  suite), `cargo fmt --check`, `cargo clippy --workspace --all-targets`
  with `-D warnings`, the complexity gate (`scripts/ci/go/cmd/complexity`
  vs `rust/complexity-baseline.json`), and 100% llvm-cov coverage on every
  new/changed Rust line (colocated `#[cfg(test)]` cover tests per repo
  convention; benchmark harness itself lives outside the covered crates).
- Benchmarks live in `scripts/bench/perf-207/` (runner + case list; test
  data stays in the workstation resources dir, passed via `--resources`).

## 4. Deliberately left out (with reasons)

- Global symbol interning (F2b): symbols are constructed with fresh
  `Arc<str>`s today; a global intern table would put a mutex on every
  `Symbol::new` (hotter than the lookups it would speed) unless sharded or
  lock-free, and `identical?` needs per-construction ids regardless. The
  measured cost is SipHash-per-lookup, which F2a removes directly.
- `Cow<Value>`-through-natives (F1 remainder): every native takes
  `&[Value]`; threading borrows through would touch hundreds of signatures
  for unmeasured payoff — the profile's clone costs are the structural
  sites F1a–F1c/F3 kill directly.
- Whole-AST `Arc<Expr>` (F4 remainder): measured AST-clone cost is exactly
  `make_closure` + `LazySeqStar`; the rewrite would churn every analyzer
  match arm for no further measured gain.
- Thread-local `Vec` buffer reuse (F5c): aliasing/lifetime hazards across
  reentrant eval for gains `smallvec` + `extend_one` already capture.
- `Transient` lock removal: `conj!`/`assoc!` lock per op by design (thread
  safety); the scripts' transient use is per-number/per-vertex, not
  per-char — not in the measured top share.
- `next_id` (global lock per closure/throwable id): kept — ids must be
  lineage-unique across forked evaluators; closure creation is not a
  per-iteration cost in the measured workloads.

## 5. Work plan (spec-first, issue order, largest first)

1. This spec, committed alone. 2. `scripts/bench/perf-207/` harness.
3. F3a+F3b (`Env` + `bind_pattern`), F3c (root cache), F1a (narrow root
   reads) — the deref+recur core; micro-bench after each. 4. F2a (FxHash),
   F3d (scan memo), F1c (lazy error names). 5. F4 (arities/body sharing),
   F5 (smallvec). 6. Full gates + coverage after each commit. 7. Workstation
   before/after snip confirmation. Sign every commit; push `perf/207-hotspots`
   to `origin`; report numbers + behavior-risk notes.
