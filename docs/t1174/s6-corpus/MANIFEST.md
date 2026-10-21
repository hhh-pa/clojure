# S6 differential corpus MANIFEST

6 files, `NN-topic.clj` + `expected/NN-topic.out` (golden stdout).
498 entries total (128 + 51 + 105 + 188 + 19 + 7); every golden is
byte-identical across re-runs (stability-verified).
Truth source: `s6-pins.md` (P1–P6); 06 sources `s9-plan.md` S9B (the
s6-plan "follow-up if cheap" item). Probe inputs live in
`/tmp/s6-probe*.clj` (not committed); corpus files are the durable,
runnable record. The row tests in
`rust/crates/clj-eval/tests/concurrency.rs` ARE the transcription —
no new tests were written; the map below ties every tag to its test.

## Oracle

From the repo root (`/Users/hhu/workspace/clojure`):

```
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:$HOME/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:$HOME/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar:$HOME/.m2/repository/org/clojure/test.check/1.1.3/test.check-1.1.3.jar"
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp "$CP" clojure.main docs/t1174/s6-corpus/NN-topic.clj \
  > docs/t1174/s6-corpus/expected/NN-topic.out 2>&1
```

Flags are load-bearing (same drift as s5d: bare machine locale/zone
differ). Regenerating one file must diff to zero unless its `.clj`
changed (staleness check: `.clj -nt expected/*.out`, plus the tag-set
check: entry tags in each `.clj` match `^:tag` lines in its `.out`).

## Entry format (all files)

- `(p ":tag" #(form))` prints `:tag  => <prn of value>`; a throw prints
  `:tag  => "THROW <class>: <message>"` (outermost only).
- Every file ends with `(println "S6-NN DONE")`; every file is one line
  per entry + DONE (no multi-line tables).
- Helpers print nothing: `latched` (04) polls `agent-error` until a
  failure lands; `failed` (04) builds a pre-failed agent; `rs`/`rs2`
  (04) and `ff`/`fc` (03) are top-level fixtures read by ordered
  entries documented in-file.

## Determinism rules

- NEVER print raw agents/futures/promises/atoms/delays (identity
  hashes). Only values, booleans, classes, messages, counts.
- Identity shapes pin as booleans: `(.startsWith (pr-str x)
  "#object[clojure.lang.Atom 0x")`, `(.startsWith (str x)
  "clojure.lang.Atom@")`, same for Delay/Agent and for the
  `reify__` prefixes of futures/promises.
- Full CCE text is stable on one JDK and pinned raw — including the
  `reify__NNNN` counters (`future_call$reify__8745`,
  `promise$reify__8792`), which are fixed at clojure.core load for a
  given jar (verified identical across reruns; a jar rebuild may
  renumber them and requires regen).
- `await` never races a failure latch: every failed-agent entry polls
  `agent-error` first (`latched`), per the P6 CAUTION (an unlatched
  `await` on a failing agent can hang the JVM).
- Every blocked worker is released before the file ends (promise
  delivers inside entries or bare top-level, never as extra tags), and
  03/04 end with `shutdown-agents`, so the JVM exits promptly
  (pool threads are non-daemon and linger forever otherwise —
  04's final call sits after its DONE trailer because the
  post-shutdown restart pins resurrect the pools).
- Never unbounded-`await` an agent fed by nil-executor `send-via`
  (the queue wedges — pinned via `await-for` in `:sendvia-nil-wedges`).
- Countdown/timeout entries use latch rendezvous (promise gates), never
  sleeps for correctness; the two 200ms sleeps in `:rs-state`/`:rs2-state`
  only settle already-dropped queues post-shutdown.

## Files

- `01-atoms.clj` (128): atom ctor/options, deref (+var kin), swap!
  (+retry accounting), reset!, compare-and-set!, validators (incl.
  ctor veto, throw shapes, checked wrap), watches (incl.
  same-value fire, nested publish, throw-keeps), atom meta/print/
  equality/predicate/cast shapes.
  Sources: pins P2, s6-probe-a/e1. Tests: atom_ctor_rows/_errors,
  validator_throw_shapes, validator_checked_wraps, deref_rows/_errors,
  swap_rows, swap_cas_accounting, swap_errors, reset_rows/_errors,
  cas_rows/_errors, watch_rows/_errors, meta_rows, print_str_rows.
- `02-delay.clj` (51): delay/force/realized? (incl. lazy-seq
  IPending), throw caching + realized-after-throw, memoize, delay
  meta/print/equality shapes.
  Sources: pins P1, s6-probe-a. Tests: deref_rows, delay_rows,
  delay_throw_cached, realized_rows/_errors, force_rows, memoize_rows,
  meta_rows, print_str_rows, agent_value_shapes (`:realized-agent`).
- `03-futures.clj` (105): future/future-call/conveyance, future?/
  future-done?, failure caching (incl. wrapper freshness, ex-info
  embedding, EE-not-Runtime), 3-arity deref casting + ms conversion,
  timeout outcomes, cancel lifecycle (+running-cancel,
  Cancelled-deref), ns sharing, promise first-wins/invocation/meta,
  stress.
  Sources: pins P3/P4, s6-probe-a/b/e1. Tests: future_rows,
  future_pred_rows, future_throw_cached, deref_timeout_rows,
  future_timeout_rows, future_cancel_rows,
  future_cancel_running_discards, future_ns_sharing, promise_rows,
  promise_threaded_deliver, promise_meta_rows, future_stress_swap,
  realized_rows, agent_shutdown_rows (`:shutdown` only).
- `04-agents.clj` (188): agent ctor/options, send family (+extra
  args, bad-fn latch), await/await-for (incl. in-action refusal),
  error modes (continue/fail/handler/bogus), restart (+clear-actions,
  veto), nested order (+discard-on-failure), validator-in-action,
  agent watches (+throw-after-publish, barrier log), `*agent*`,
  setters, value shapes, stress, shutdown lifecycle (+joins,
  post-shutdown, restart-after-shutdown) LAST.
  Sources: pins P5/P6, s6-probe-a/b/c2/d/e1. Tests: all `agent_*`
  and `set_*` tests (see map).
- `06-pvalues.clj` (7): `pcalls`/`pvalues` composites (vec/empty/
  seq?, side-effect fan-out, ExecutionException propagation).
  Sources: s9-plan S9B. Tests: `pcalls_rows`, `pvalues_rows`,
  `pvalues_error_wraps_execution_exception` (see map; the same fns
  also carry the 05 `:pcalls-basic`-family tags).

## Tag → test transcription map

Grouped by file; each tag appears once (except `:shutdown-join-all`,
shared by two tests). Test fns are in
`rust/crates/clj-eval/tests/concurrency.rs`.

### 01-atoms.clj

- `atom_ctor_rows` (13): `:atom-deref-1` `:atom-deref-nil`
  `:atom-deref-false` `:atom-meta-kv` `:atom-meta-none`
  `:atom-meta-nilopt` `:atom-validator-ok` `:atom-validator-nil`
  `:atom-validator-false` `:atom-unknown-opt` `:atom-dup-meta-last`
  `:atom-dup-mixed` `:atom-class`
- `atom_ctor_errors` (7): `:atom-ctor-veto` `:atom-ctor-noval`
  `:atom-ctor-stray` `:atom-ctor-nonkv` `:atom-ctor-badmeta`
  `:atom-ctor-badvalidator` `:atom-arity-0`
- `validator_throw_shapes` (2): `:validator-throw-propagates`
  `:validator-throw-ctor`
- `validator_checked_wraps` (1): `:validator-checked-wrap`
  (source-level `(throw (Exception. ...))` replaces the test native;
  same wrap shape verified)
- `deref_rows` (5): `:deref-atom` `:deref-atom-reader` `:deref-var`
  `:deref-var-dynamic` `:deref-var-unbound-class`
- `deref_errors` (5): `:deref-long` `:deref-str` `:deref-nil`
  `:deref-arity-0` `:deref-arity-2`
- `swap_rows` (6): `:swap-basic` `:swap-returns-new`
  `:swap-extra-3` `:swap-extra-5` `:swap-keyword-fn`
  `:swap-nil-result`
- `swap_cas_accounting` (2): `:swap-retry-race`
  `:swap-uncontended`
- `swap_errors` (16): `:swap-long-rcv` `:swap-long-both`
  `:swap-nil-0` `:swap-nil-1` `:swap-nil-2` `:swap-nil-3`
  `:swap-arity-1` `:swap-badfn` `:swap-nilfn-0` `:swap-nilfn-1`
  `:swap-nilfn-2` `:swap-nilfn-3` `:swap-veto` `:swap-veto-keeps`
  `:swap-throwf-keeps` `:swap-f-arity-bool` (startsWith: fn name
  carries eval counters)
- `reset_rows` (3): `:reset-basic` `:reset-nil`
  `:reset-same-fires`
- `reset_errors` (6): `:reset-long` `:reset-nil-rcv`
  `:reset-arity-1` `:reset-arity-3` `:reset-veto`
  `:reset-veto-keeps`
- `cas_rows` (7): `:cas-win` `:cas-lose` `:cas-kw` `:cas-ident`
  `:cas-to-nil` `:cas-lose-no-watch` `:cas-win-fires`
- `cas_errors` (5): `:cas-long` `:cas-nil` `:cas-arity-2`
  `:cas-validates-on-match` `:cas-validates-on-mismatch`
- `watch_rows` (11): `:watch-add-returns-ref`
  `:watch-remove-returns-ref` `:watch-args`
  `:watch-same-value-fires` `:watch-remove-stops`
  `:watch-remove-missing` `:watch-nonkw-key` `:watch-replace-fn`
  `:watch-nil-fn-silent` `:watch-nested-publish`
  `:watch-throw-keeps`
- `watch_errors` (7): `:watch-add-long` `:watch-remove-long`
  `:watch-add-nil` `:watch-remove-nil` `:watch-add-badfn`
  `:watch-add-arity` `:watch-remove-arity`
- `meta_rows` (7): `:alter-meta` `:reset-meta`
  `:alter-meta-read` `:reset-meta-nil` `:withmeta-atom`
  `:varymeta-atom` `:printmeta-atom-bool`
- `print_str_rows` (25): `:pr-atom-bool` `:str-atom-bool`
  `:print-atom-bool` `:format-atom-bool` `:atom-eq-self`
  `:atom-eq-fresh` `:atom-delay-neq` `:atom-not-eq`
  `:atom-hash-int` `:seq-atom` `:first-atom` `:count-atom`
  `:nth-atom` `:coll-atom` `:seq?-atom` `:counted-atom`
  `:fn-atom` `:map-atom` `:vector-atom` `:var-atom` `:get-atom`
  `:find-atom` `:conj-atom` `:compare-atom` `:invoke-atom`

### 02-delay.clj

- `deref_rows` (2): `:deref-delay` `:deref-delay-reader`
- `delay_rows` (9): `:force-delay` `:delay-body-once`
  `:delay-multi` `:delay-empty` `:delay-nil-realized`
  `:delay-lexical` `:delay-dynamic` `:delay-class`
  `:delaystar-shadow`
- `delay_throw_cached` (3): `:delay-throw-cached`
  `:delay-throw-reraise` `:delay-throw-uncaught` (self-force SOE
  dropped — see below)
- `realized_rows` (4): `:realized-delay-false`
  `:realized-delay-true` `:realized-lazy-false`
  `:realized-lazy-after-first`
- `realized_errors` (6): `:realized-atom` `:realized-long`
  `:realized-vec` `:realized-nil` `:realized-arity-0`
  `:realized-arity-2`
- `agent_value_shapes` (1): `:realized-agent`
- `force_rows` (6): `:force-42` `:force-nil` `:force-str`
  `:force-atom-passthrough` `:force-arity-0` `:force-arity-2`
- `memoize_rows` (6): `:memo-basic` `:memo-multi-arity`
  `:memo-nullary` `:memo-nil` `:memo-fn` `:memo-arity`
- `meta_rows` (4): `:meta-delay-nil` `:withmeta-delay`
  `:altermeta-delay` `:printmeta-delay-bool`
- `print_str_rows` (10): `:pr-delay-bool` `:str-delay-bool`
  `:pr-noforce` `:delay-eq-self` `:delay-eq-fresh`
  `:delay-hash-int` `:seq-delay` `:count-delay`
  `:counted-delay` `:ifn-delay`

### 03-futures.clj

- `future_rows` (12): `:future-deref-1` `:future-multi-body`
  `:future-call-basic` `:future-conveys` `:future-bind-local`
  `:future-class` `:pr-future-bool` `:str-future-bool`
  `:futurecall-nonfn-spawns` `:force-future-passthrough`
  `:futurecall-arity-0` `:futurecall-arity-2`
- `future_pred_rows` (8): `:future?-vec` `:futuredone-settled`
  `:futuredone-pending` (reads `[false :released]`: releases its
  worker) `:futuredone-promise` `:futuredone-delay`
  `:futuredone-long` `:futuredone-nil` `:futuredone-arity-0`
- `future_throw_cached` (12): `:future-fail-deref1`
  `:future-fail-realized` `:future-fail-done`
  `:future-fail-deref2` `:future-fail-timeout-deref`
  `:future-wrap-fresh` `:future-nonfn-deref`
  `:future-exinfo-cause` `:future-exinfo-nilmsg`
  `:future-exinfo-nildata` `:future-ee-not-runtime`
  `:future-checked-src` (source-level checked throw replaces the
  test native)
- `deref_timeout_rows` (17): `:deref-timeout-atom`
  `:deref-timeout-delay` `:deref-timeout-var`
  `:deref-timeout-long` `:deref-timeout-nil`
  `:deref-timeout-arity2` `:deref-ms-str` `:deref-ms-kw`
  `:deref-ms-bool` `:deref-ms-char` `:deref-ms-nil`
  `:deref-promise-09` `:deref-promise-neg` `:deref-promise-0`
  `:deref-future-double-ms` `:deref-settled-badms`
  `:deref-settled-ok`
- `future_timeout_rows` (4): `:future-timeout-pending`
  (releases its worker after the timeout read)
  `:future-timeout-delivered` `:deref-promise-timeout`
  `:deref-promise-delivered`
- `future_cancel_rows` (14): `:cancel-pending` `:cancel-done`
  `:cancel-realized` `:cancel-deref` `:cancel-deref-timeout`
  `:cancel-msg-empty` `:cancel-is-ise` `:cancel-again`
  `:cancel-completed` `:cancel-badms` `:cancel-promise`
  `:cancel-long` `:cancel-nil` `:cancel-arity-0`
- `future_cancel_running_discards` (1): `:cancel-running`
  (JVM shape: interrupt + Cancelled deref — port diverges)
- `future_ns_sharing` (3): `:future-def-visible`
  `:future-late-def` `:future-convey-add`
- `promise_rows` (19): `:deliver-first-wins-eq`
  `:deliver-second-nil` `:deliver-nil-first`
  `:promise-unrealized` `:pr-promise-bool` `:promise-class`
  `:str-promise-bool` `:promise-blocks-till-deliver`
  `:promise-invoke` `:deliver-plus` `:ifn-promise`
  `:ifn-future` `:deliver-long` `:deliver-nil-fn`
  `:promise-call-0` `:promise-call-2` `:promise-arity-1`
  `:deliver-arity-1` `:deliver-arity-3`
- `promise_threaded_deliver` (1): `:promise-worker-delivers`
  (future worker delivers to main-blocked deref; same
  Clojure-level behavior as the Rust-thread test)
- `promise_meta_rows` (8): `:meta-promise-nil`
  `:meta-future-nil` `:meta-withmeta-promise`
  `:meta-withmeta-future` `:promise-meta-shared`
  `:meta-vary-promise` `:altermeta-promise`
  `:resetmeta-future`
- `future_stress_swap` (1): `:stress-swap-2000`
- `realized_rows` (4): `:realized-future-pending` (reads
  `[false :released]`) `:realized-future-done`
  `:realized-promise-delivered` `:realized-future-failed`
- `agent_shutdown_rows` (1): `:shutdown` (main pins in 04)

### 04-agents.clj

- `agent_ctor_rows` (22): `:agent-deref-1` `:agent-deref-nil`
  `:agent-meta-kv` `:agent-meta-none` `:agent-meta-nilopt`
  `:agent-validator-ok` `:agent-validator-nil`
  `:agent-handler-nil` `:agent-unknown-opt` `:agent-dup-meta`
  `:agent-bogus-mode-ok` `:agent-class` `:pr-agent-bool`
  `:str-agent-bool` `:agent-eq-self` `:agent-eq-fresh`
  `:agent-atom-neq` `:agent-hash-int` `:send-returns-agent`
  `:sendoff-returns-agent` `:sendvia-returns-agent`
  `:sendvia-nil-wedges` (nil executor wedges the queue — port
  diverges by running the action)
- `agent_ctor_errors` (9): `:agent-ctor-veto` `:agent-ctor-noval`
  `:agent-ctor-badmeta` `:agent-ctor-badvalidator`
  `:agent-ctor-badhandler` `:agent-ctor-badmode`
  `:agent-ctor-mode-nil` `:agent-ctor-mode-false`
  `:agent-arity-0`
- `agent_error_mode_rows` (16): `:continue-clears`
  `:fail-await-caught` `:fail-state-parked` `:fail-error-detail`
  `:fail-error-class` `:fail-send-throws`
  `:fail-sendoff-throws` `:fail-sendvia-throws`
  `:fail-await-throws` `:fail-awaitfor-throws`
  `:fail-cause-detail` `:handler-default-continues`
  `:explicit-fail-wins` `:explicit-fail-parked`
  `:bogus-mode-fails` `:bogus-mode-parked`
- `agent_restart_rows` (18): `:restart-returns-new`
  `:restart-proceeds` `:restart-no-watch-fire`
  `:restart-clear-drops` `:restart-keep-queue`
  `:restart-veto-stays-failed` `:restart-veto-state`
  `:restart-veto-then-ok` `:restart-unfailed`
  `:restart-unfailed-novalidate` `:restart-badclear-opt`
  `:restart-arity-1` `:restart-nil` `:restart-long`
  `:agent-error-arity-0` `:agent-error-nil` `:agent-error-long`
  `:agent-error-healthy`
- `agent_await_rows` (18): `:await-multi` `:await-none`
  `:awaitfor-none` `:awaitfor-ok` `:awaitfor-gated`
  `:awaitfor-neg` (releases its worker) `:awaitfor-neg-idle`
  `:awaitfor-double` `:await-in-action-plain`
  `:await-none-in-action-msg` `:awaitfor-in-action-msg`
  `:await-long` `:await-nil` `:awaitfor-str-long`
  `:awaitfor-str-agent` `:awaitfor-nil-ms` `:awaitfor-arity-0`
  `:await-second-failed`
- `agent_nested_rows` (3): `:nest-single` `:nest-double`
  `:nest-two`
- `agent_nested_discarded_on_failure` (2): `:nest-dropped-fail`
  `:nest-dropped-continue`
- `agent_validator_rows` (5): `:validator-continue`
  `:validator-fail-latch` `:validator-fail-detail`
  `:validator-fail-class` `:validator-throw-continue`
- `agent_watch_rows` (9 + 1 shared): `:watch-agent-fire`
  `:watch-agent-same` `:watch-agent-removed`
  `:watch-agent-add-eq` `:watch-agent-rem-eq`
  `:watch-agent-nil-silent` `:watch-throw-continue`
  `:watch-throw-fail` `:watch-throw-fail-state`, plus
  `:shutdown-join-all` (shared with `agent_shutdown_joins`:
  barrier-full-log `[[5 6] [6 6]]`)
- `agent_star_rows` (7): `:star-nil-outside`
  `:star-bound-in-action` `:star-nested-own` `:star-conveys`
  `:star-watcher-bound` (renamed: the golden `false` pins
  JVM-bound; the old `-bare` name was wrong)
  `:star-validator-bare` (ctor-time validator runs outside
  any action → `true`) `:star-handler-bound` (error-handler
  under `:continue` sees bound → `false`; deliver-then-read,
  modeled on `:handler-default-continues`)
- `agent_send_shapes` (13): `:send-long` `:sendoff-long`
  `:sendvia-long` `:send-nil` `:sendoff-nil` `:sendvia-nil`
  `:send-arity-1` `:sendoff-arity-1` `:sendvia-arity-2`
  `:send-extra` `:sendoff-extra` `:send-badfn-latch`
  `:send-nilfn-latch`
- `agent_panic_rows` (2): `:error-action-latches`
  `:error-continue` (JVM `(Error.)` analogue of the Rust-panic
  test: Errors latch under `:fail`, clear under `:continue`)
- `agent_value_shapes` (29): `:deref-agent`
  `:deref-agent-reader` `:deref-agent-timeout` `:force-agent-eq`
  `:futureq-agent` `:alter-meta-agent` `:reset-meta-agent`
  `:alter-meta-agent-read` `:withmeta-agent` `:varymeta-agent`
  `:printmeta-agent-bool` `:seq-agent` `:first-agent`
  `:count-agent` `:nth-agent` `:coll-agent` `:seq?-agent`
  `:counted-agent` `:fn-agent` `:ifn-agent` `:map-agent`
  `:vector-agent` `:var-agent` `:get-agent` `:find-agent`
  `:conj-agent` `:compare-agent` `:invoke-agent`
  `:deliver-agent` (`:realized-agent` lives in 02)
- `agent_stress` (2): `:stress-send` `:stress-sendoff`
- `set_error_handler_rows` (8): `:seth-install` `:seth-clear`
  `:seth-fires` `:seth-silent` (fresh agent, so state is 1 not
  the test's threaded 2) `:seth-nil` `:seth-long`
  `:seth-badfn` `:seth-arity-1`
- `set_error_mode_rows` (11): `:setm-continue`
  `:setm-continue-clears` `:setm-fail-latches`
  `:setm-fail-parked` `:setm-bogus-fails` `:setm-nil-fails`
  `:setm-nil-ok` `:setm-nil-agn` `:setm-long` `:setm-str`
  `:setm-arity-1`
- `agent_shutdown_joins` (1 shared): `:shutdown-join-all`
  (`[[[5 6] [6 6]] 1 2 :done true]`; bare future-done? after
  shutdown is racy on the JVM, so the entry derefs — see below)
- `agent_shutdown_rows` (7): `:shutdown-again`
  `:shutdown-arity-1` `:shutdown-send-kept`
  `:shutdown-sendoff-kept` `:shutdown-sendvia-kept`
  `:shutdown-awaitfor` `:shutdown-future-submit-class`
  (class-only pin: the RejectedExecutionException message
  carries identity hashes)
- `agent_restart_after_shutdown` (3): `:rs-failed`
  `:rs-restart` `:rs-state`
- `agent_restart_resumed_after_shutdown` (3): `:rs2-failed`
  `:rs2-restart` `:rs2-state`

### 05-locking-pmap.clj

- `locking_rows` (6): `:locking-val` `:locking-empty`
  `:locking-multi` `:locking-reentrant` `:locking-nil`
  `:locking-arity` (via `eval`: the arity error is a macro
  compile error, uncatchable inline)
- `locking_mutual_exclusion` (2): `:locking-mutex` (4 futures x
  50 increments, exact 200) `:locking-unlock-on-throw`
- `pmap_rows` (7): `:pmap-basic` `:pmap-empty` `:pmap-chunk`
  `:pmap-multi` `:pmap-multi-short` `:pmap-lazy` `:pmap-str`
- `pcalls_rows` (2): `:pcalls-basic` `:pcalls-empty`
- `pvalues_rows` (2): `:pvalues-basic` `:pvalues-sidefx`
  (`doall` + `sort`: the raw shape `[(2 1) 0]` is a
  laziness/race artifact — unrealized at counter read, order
  racy; normalized to `[(1 2) 2]`)
### 06-pvalues.clj

- `pcalls_rows` (3): `:pcalls-vec` `:pcalls-vec-empty`
  `:pcalls-seq`
- `pvalues_rows` (3): `:pvalues-vec` `:pvalues-vec-empty`
  `:pvalues-inc3`
- `pvalues_error_wraps_execution_exception` (1):
  `:pvalues-throw` (golden prints `THROW <class>: <message>`;
  the test asserts the `uncaught` `|`-joined same chain)

## Tests without corpus tags

- `evaluator_is_send_sync`: Rust-only (no Clojure surface).
- Every other test fn in `concurrency.rs` has at least one tag
  above; every tag maps to exactly one test (except
  `:shutdown-join-all`, shared by two).

## Dropped / reshaped entries

- Delay self-force (`delay_throw_cached`): DROPPED. The JVM throws
  `StackOverflowError` but also prints a HotSpot warning to stderr
  (`Potentially dangerous stack overflow in ReservedStackAccess
  ...`), which lands in the golden via `2>&1` and is too fragile
  to pin. The port diverges here anyway (`IllegalStateException
  | Delay is already being forced`).
- Bare `future-done?` after `shutdown-agents`
  (`agent_shutdown_joins`): RESHAPED. Racy on the JVM (`true`
  then `false` across identical runs — shutdown does not join
  future threads). `:shutdown-join-all` derefs after shutdown
  instead (`:done true`, deterministic).
- `send-via` extras with nil executor (`agent_send_shapes`):
  RESHAPED. The port runs the action; the JVM wedges the agent
  queue (unbounded `await` hangs — this killed probe C). Pinned
  as `:sendvia-returns-agent` + `:sendvia-nil-wedges`
  (`[false 0]` via `await-for`).
- `[false :released]` shapes (`:futuredone-pending`,
  `:realized-future-pending`): extended so each entry releases
  its worker; the first element is the test's pin.

## Known divergences (golden = JVM truth, port differs)

- Reify classes carry build counters on the JVM
  (`...$future_call$reify__8745`, `...$promise$reify__8792`);
  the port reports counter-free names. Visible in
  `:future-class`, `:promise-class`, and the CCE/AME messages
  naming them.
- `with-meta`/`vary-meta` (atom/delay/agent),
  `alter-meta!` (delay/promise/future), invoking an atom/agent,
  `deliver` onto a non-fn/agent, and the `:send-badfn-latch`
  cause all throw full module-verbiage CCEs on the JVM; the
  port uses short `Cannot cast X to Y` shapes.
- `:deliver-nil-fn` is NPE `... because "promise" is null` on
  the JVM (`"this.f" is null` for `:send-nilfn-latch`); the
  port reports `Cannot invoke nil as a function`.
- `:promise-call-0`/`:promise-call-2` are
  `AbstractMethodError` on the JVM; the port throws
  `ArityException`.
- `:cancel-running`: the JVM interrupts the body (Cancelled
  deref); the port runs it to completion and discards the
  result.
- `:sendvia-nil-wedges`: the JVM wedges the queue; the port
  runs the action.
- Delay self-force: JVM `StackOverflowError`; port
  `IllegalStateException` (dropped entry, see above).
- Post-shutdown `future-done?`: racy on the JVM; the port
  joins deterministically (reshaped entry, see above).

## Open items

- A jar rebuild may renumber the `reify__NNNN` counters; regen
  all six goldens and expect diffs confined to the
  counter-bearing lines.
- The locking-nil NPE carries a gensym
  (`locklocal__...__auto__...`); pinned raw on this JDK, shape
  only across JDKs.
- `defn/def/let/loop/fn/recur` shapes remain out of scope (s5d).
