// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S19 STM pins: every oracle row in
//! `docs/zero-oos-s19-stm/probes/{refs,txn,errors}.out` (46 + 70 +
//! 58), transcribed probe-identically except where noted.
//!
//! Row map (the §1i covering tests): `atomic_commit`
//! (t-body-value, t-empty, t-commit, t-atomic-abort, t-deref-local,
//! t-retry-invisible, t-commute-abort-atomic, e-dosync-passthrough),
//! `outside_tx_gate` (e-alter/refset/commute/ensure-outside, the
//! nil/nonref/nonfn check-order rows, the four updater arities),
//! `send_deferral` (t-send-defer, t-sendoff-defer, t-sendvia-defer,
//! t-send-abort, t-send-returns-agent, t-send-badagent-in-txn),
//! `io_gate` (t-io-in/out/multi/empty, t-sync-out/empty/
//! flags-ignored/flags-need-not-be-nil/runs-tx), `ref_ctor_opts`
//! (r-odd-opts, r-bad-opt-key, r-meta-opt/none, r-eq-same/distinct,
//! r-identical, r-validator-construct/nil-opt/reject, the
//! r-min/max-default/opt/set/negative/above/below rows),
//! `history_accessors` (the e-min/max/count ladder, arity, and
//! receiver rows), `history_trim` (r-history-init/after/after2/
//! min-keeps/max-trims/default-many/min3-many), `macro_shapes`
//! (t-shape-dosync, t-shape-sync/io/io-multi, t-sync-arity0),
//! `nesting_join` (t-nested, t-nested-join, t-sync-in),
//! `commute_compose` (t-alter/refset-returns, every t-commute-*
//! except t-commute-abort-atomic, t-ensure-commute,
//! t-commute-ensure, t-ensure-returns), `validator_watch_timing`
//! (t-validator-calls, every t-watch-* and t-addwatch/
//! t-removewatch-* row, e-validator-msg), `invoke_and_predicates`
//! (r-deref/reader/timeout, r-realized/atom/num, r-counted,
//! r-ifn, r-atom-ifn, r-invoke0/1/fnval/variadic, r-atom-invoke,
//! r-with-meta, t-invoke-in-txn), `await_gates`
//! (t-await-noargs-in/out, t-await/awaitfor-in-dosync,
//! t-await/awaitfor-action-tx), `boundfn_conveyance` (t-bound-fn,
//! t-future-no-convey), `print_and_str` (r-str, r-new/nil/coll).
//!
//! Pinned divergences: r-new/r-new-nil/r-new-coll (the port's
//! transient `#object[…]` convention), t-shape-sync/t-shape-io/
//! t-shape-io-multi (`sync`/`io!` are special forms —
//! `macroexpand-1` is the identity), t-watch-order* (insertion
//! order — the port's atom convention; t-watch-strkeys coincides).
//! `identical?` is out of scope in the port (matrix row 574), so the
//! six rows using it substitute `=` (same-cell + equal meta —
//! equivalent here, where no meta is involved) plus one shared-cell
//! behavioral pin. `(Exception. …)` has no port surface, so abort
//! signals throw through the `s19-ex` double (same class, same
//! message). t-sendvia-defer substitutes the `nil` executor
//! (the JVM static field has no port surface; executors are
//! ignored). JVM-only rows (t-key-hashes — the port hashes
//! relationally, never literally; t-setvalidator-in-txn —
//! `set-validator!` is out of scope, matrix row 644) stay as
//! skip comments with cause.
//!
//! `stm_portside_pins` holds port-side pins with no oracle rows
//! (oracle-verified locally during implementation):
//! commit-phase write semantics, recur fences, eager send checks,
//! the alter/commute check-order corners, and trim edges.
//! `stm_retry_under_contention` pins a genuine conflict retry
//! (force-trim strands a reader; the retry commits).
//! `stm_revalidate_retry` pins the revalidation-mismatch retry (a
//! committed write lands between the read and the commit).
//! `seam_suites_green` replays agent/future/atom smoke rows through
//! the §1f seam (outside a transaction the send path is untouched).

use std::sync::Arc;

use clj_eval::{Evaluator, NativeError, SourceError, boot};
use clj_reader::{ReaderOpts, read_str};
use clj_value::{PrintOpts, ThrowClass, Throwable, Value, pr_to_string};

/// A `java.lang.Exception` throw double (`(Exception. …)` has no
/// port surface — construction needs a host descriptor root): builds
/// the oracle's throwable directly, like `eval_semantics`' `t-ex`.
fn install_throw_double(eval: &mut Evaluator) {
    eval.install_native(
        "user",
        "s19-ex",
        Arc::new(|_, args| match args {
            [Value::Str(m)] => Ok(Value::Throwable(Throwable {
                id: 0,
                class: ThrowClass::Exception,
                message: Some(Arc::clone(m)),
                data: None,
                cause: None,
            })),
            _ => Err(NativeError::arity("user/s19-ex", args.len())),
        }),
    );
}

/// A booted evaluator with the throw double installed.
fn boot_stm() -> Evaluator {
    let mut eval = boot();
    install_throw_double(&mut eval);
    eval
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    pr_to_string(vs.last().unwrap(), &PrintOpts::pr())
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// The analyzer error chain for a source that fails analysis (the
/// `t-sync-arity0` shape — mirrors `analyzer_contract`).
fn chain_of(src: &str) -> Vec<String> {
    let mut eval = Evaluator::new("test");
    let form = read_str(src, &ReaderOpts::default()).unwrap();
    let err = eval.analyze(&form).unwrap_err();
    err.chain()
}

/// Assert the transient-style `#object` rendering with matching hex
/// identities (per-process ids, never pinned literally).
fn assert_object_shape(pr: &str, class: &str) {
    let prefix = format!("#object[{class} 0x");
    let quoted = format!("\"{class}@");
    assert!(
        pr.starts_with(&prefix),
        "missing #object prefix for {class}: {pr}"
    );
    assert!(pr.ends_with("\"]"), "missing #object suffix: {pr}");
    let hex = pr[prefix.len()..].split(' ').next().unwrap();
    assert!(
        !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "bad id hex: {pr}"
    );
    assert!(
        pr.contains(&format!("{quoted}{hex}\"")),
        "mismatched toString hex: {pr}"
    );
}

fn assert_ref_object(pr: &str) {
    assert_object_shape(pr, "clojure.lang.Ref");
}

/// A `ClassCastException` with the JDK module verbiage (same-module
/// classes combine, different modules split).
fn cce(from: &str, to: &str) -> String {
    fn module(cls: &str) -> &'static str {
        if cls.starts_with("java.") {
            "module java.base of loader 'bootstrap'"
        } else {
            "unnamed module of loader 'app'"
        }
    }
    let (fm, tm) = (module(from), module(to));
    let message = if fm == tm {
        format!("class {from} cannot be cast to class {to} ({from} and {to} are in {fm})")
    } else {
        format!("class {from} cannot be cast to class {to} ({from} is in {fm}; {to} is in {tm})")
    };
    format!("java.lang.ClassCastException | {message}")
}

fn ise(message: &str) -> String {
    format!("java.lang.IllegalStateException | {message}")
}

fn npe(message: &str) -> String {
    format!("java.lang.NullPointerException | {message}")
}

fn arity_error(name: &str, argc: usize) -> String {
    format!(
        "clojure.lang.ArityException | Wrong number of args ({argc}) passed to: clojure.core/{name}"
    )
}

#[test]
fn ref_ctor_opts() {
    let mut eval = boot();
    // r-odd-opts (shared `atom_options` shape).
    assert_eq!(
        uncaught(&mut eval, "(ref 0 :validator)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :validator"
    );
    // r-bad-opt-key (unknown keys ignored).
    assert_eq!(run_last(&mut eval, "(deref (ref 0 :bogus 1))"), "0");
    // r-meta-opt, r-meta-none.
    assert_eq!(run_last(&mut eval, "(meta (ref 0 :meta {:k 1}))"), "{:k 1}");
    assert_eq!(run_last(&mut eval, "(meta (ref 0))"), "nil");
    // r-eq-same, r-eq-distinct (shared-cell identity).
    assert_eq!(run_last(&mut eval, "(let [r (ref 0)] (= r r))"), "true");
    assert_eq!(run_last(&mut eval, "(= (ref 0) (ref 0))"), "false");
    // r-identical: `identical?` is out of scope in the port, so `=`
    // (same-cell + equal meta — equivalent with no meta involved).
    assert_eq!(run_last(&mut eval, "(let [r (ref 0)] (= r r))"), "true");
    // r-validator-construct (a vetoing ctor throws).
    assert_eq!(
        uncaught(&mut eval, "(ref 3 :validator even?)"),
        ise("Invalid reference state")
    );
    // r-validator-nil-opt (falsy never installs).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :validator nil)] (dosync (alter r (constantly :x))))"
        ),
        ":x"
    );
    // r-validator-reject (commit veto: nothing installs).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :validator even?)] [(try (dosync (alter r (constantly 3))) :committed (catch IllegalStateException e [:thrown (ex-message e)])) @r])"
        ),
        "[[:thrown \"Invalid reference state\"] 0]"
    );
    // r-min-default, r-max-default, r-min-opt, r-max-opt.
    assert_eq!(run_last(&mut eval, "(ref-min-history (ref 0))"), "0");
    assert_eq!(run_last(&mut eval, "(ref-max-history (ref 0))"), "10");
    assert_eq!(
        run_last(&mut eval, "(ref-min-history (ref 0 :min-history 5))"),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(ref-max-history (ref 0 :max-history 7))"),
        "7"
    );
    // r-min-set, r-max-set (`identical?` → `=` — no meta involved).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(= (ref-min-history r 3) r) (ref-min-history r)])"
        ),
        "[true 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(= (ref-max-history r 7) r) (ref-max-history r)])"
        ),
        "[true 7]"
    );
    // The setter answers the same cell (a write through the
    // returned handle reads through the original).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) s (ref-min-history r 3)] (dosync (alter s inc)) @r)"
        ),
        "1"
    );
    // r-max-below-min, r-min-above-max (no cross-constraint).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 5)] (ref-max-history r 3) [(ref-min-history r) (ref-max-history r)])"
        ),
        "[5 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :max-history 7)] (ref-min-history r 9) [(ref-min-history r) (ref-max-history r)])"
        ),
        "[9 7]"
    );
    // r-min-negative, r-max-negative (no lower bound).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r -1) (ref-min-history r))"
        ),
        "-1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-max-history r -1) (ref-max-history r))"
        ),
        "-1"
    );
}

#[test]
fn invoke_and_predicates() {
    let mut eval = boot();
    // r-deref, r-deref-reader.
    assert_eq!(run_last(&mut eval, "(deref (ref 5))"), "5");
    assert_eq!(run_last(&mut eval, "@(ref 5)"), "5");
    // r-deref-timeout (the existing non-blocking arm — no STM change).
    assert_eq!(
        uncaught(&mut eval, "(deref (ref 5) 100 :t)"),
        cce("clojure.lang.Ref", "java.util.concurrent.Future")
    );
    // r-realized, r-realized-atom, r-realized-num (`realized?` casts
    // to `IPending` — the existing arm; refs miss it like atoms).
    assert_eq!(
        uncaught(&mut eval, "(realized? (ref 0))"),
        cce("clojure.lang.Ref", "clojure.lang.IPending")
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? (atom 0))"),
        cce("clojure.lang.Atom", "clojure.lang.IPending")
    );
    assert_eq!(
        uncaught(&mut eval, "(realized? 5)"),
        cce("java.lang.Long", "clojure.lang.IPending")
    );
    // r-counted, r-ifn, r-atom-ifn.
    assert_eq!(run_last(&mut eval, "(counted? (ref 0))"), "false");
    assert_eq!(run_last(&mut eval, "(ifn? (ref 0))"), "true");
    assert_eq!(run_last(&mut eval, "(ifn? (atom 0))"), "false");
    // r-invoke0, r-invoke1 (deref, then invoke the value — a
    // non-`IFn` value fails with the port's existing short invoke
    // cast, S19 §1d; the JVM rows carry the full CCE verbiage).
    assert_eq!(
        uncaught(&mut eval, "((ref 5))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(
        uncaught(&mut eval, "((ref 5) 1)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    // r-invoke-fnval, r-invoke-variadic (any arity, incl. `apply`).
    assert_eq!(run_last(&mut eval, "((ref (fn [a b] (+ a b))) 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(apply (ref +) (range 1 22))"), "231");
    // r-atom-invoke (control: atoms are not invokable — the same
    // short shape, untouched).
    assert_eq!(
        uncaught(&mut eval, "((atom (fn [a b] (+ a b))) 1 2)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Atom to clojure.lang.IFn"
    );
    // r-with-meta (refs are not `IObj` — the generic cast).
    assert_eq!(
        uncaught(&mut eval, "(with-meta (ref 0) {:a 1})"),
        cce("clojure.lang.Ref", "clojure.lang.IObj")
    );
}

#[test]
fn atomic_commit() {
    let mut eval = boot_stm();
    // t-body-value, t-empty.
    assert_eq!(run_last(&mut eval, "(dosync 1 2)"), "2");
    assert_eq!(run_last(&mut eval, "(dosync)"), "nil");
    // t-commit.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) s (ref 10)] (dosync (alter r inc) (alter s + 5)) [@r @s])"
        ),
        "[1 15]"
    );
    // t-atomic-abort (a body throw discards everything).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) s (ref 10)] [(try (dosync (alter r inc) (throw (s19-ex \"boom\"))) :committed (catch Exception e :caught)) @r @s])"
        ),
        "[:caught 0 10]"
    );
    // t-deref-local (read-your-write).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (alter r (constantly 5)) @r))"
        ),
        "5"
    );
    // t-retry-invisible (a calm transaction runs its body once).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)] [(dosync (swap! c inc) :done) @c])"
        ),
        "[:done 1]"
    );
    // t-commute-abort-atomic (the guard aborts the whole transaction).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) s (ref 0)] [(try (dosync (commute r + 1) (alter r + 10) (alter s + 100)) :c (catch IllegalStateException e :t)) @r @s])"
        ),
        "[:t 0 0]"
    );
    // e-dosync-passthrough (user throws escape unwrapped).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (dosync (throw (ex-info \"boom\" {:k 1}))) :no (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)]))"
        ),
        "[\"boom\" {:k 1}]"
    );
}

#[test]
fn commute_compose() {
    let mut eval = boot();
    // t-alter-returns, t-refset-returns, t-commute-returns.
    assert_eq!(run_last(&mut eval, "(dosync (alter (ref 0) + 1 2))"), "3");
    assert_eq!(run_last(&mut eval, "(dosync (ref-set (ref 0) 9))"), "9");
    assert_eq!(run_last(&mut eval, "(dosync (commute (ref 0) + 5))"), "5");
    // t-commute-reruns (in-txn application + one commit re-run).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) r (ref 0)] (dosync (commute r (fn [x] (swap! c inc) (+ x 1)))) [@r @c])"
        ),
        "[1 2]"
    );
    // t-commute-order (re-runs compose in issue order).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [log (atom []) r (ref 0)] (dosync (commute r (fn [x] (swap! log conj :f) (+ x 1))) (commute r (fn [x] (swap! log conj :g) (* x 10)))) [@r @log])"
        ),
        "[10 [:f :g :f :g]]"
    );
    // t-commute-alter (earlier non-commute writes replay under the re-run).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (alter r + 10) (commute r + 1)) @r)"
        ),
        "11"
    );
    // t-commute-alter-rev (the set-guard fires at the call).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (dosync (commute r + 1) (alter r + 10)) @r)"
        ),
        ise("Can't set after commute")
    );
    // t-commute-refset, t-commute-refset-rev.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (ref-set r 10) (commute r + 1)) @r)"
        ),
        "11"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(try (dosync (commute r + 1) (ref-set r 10)) :c (catch IllegalStateException e (ex-message e))) @r])"
        ),
        "[\"Can't set after commute\" 0]"
    );
    // t-commute-set-immediate (later body never runs).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) c (atom 0)] [(try (dosync (commute r + 1) (alter r + 10) (swap! c inc)) :c (catch IllegalStateException e :t)) @c @r])"
        ),
        "[:t 0 0]"
    );
    // t-commute-ensure, t-ensure-commute, t-ensure-returns.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (ensure r) (commute r + 1)) @r)"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (commute r + 1) (ensure r)))"
        ),
        "1"
    );
    assert_eq!(run_last(&mut eval, "(dosync (ensure (ref 7)))"), "7");
    // t-invoke-in-txn (invocation reads tx-locally).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (ref-set r (fn [x] (* x 10))) (r 5)))"
        ),
        "50"
    );
}

#[test]
fn nesting_join() {
    let mut eval = boot();
    // t-nested (inner joins: one commit).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(dosync (alter r inc) (dosync (alter r inc))) @r])"
        ),
        "[2 2]"
    );
    // t-nested-join (one watch round across the join).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) r (ref 0)] (add-watch r :k (fn [_ _ _ _] (swap! c inc))) (dosync (alter r inc) (dosync (alter r inc))) @c)"
        ),
        "1"
    );
    // t-sync-in (`sync` joins the running transaction).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (sync nil (alter r inc)) @r))"
        ),
        "1"
    );
}

#[test]
fn validator_watch_timing() {
    let mut eval = boot_stm();
    // t-validator-calls (construction over the initial value, then
    // once at commit over the final — intermediates never).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom []) r (ref 0 :validator (fn [x] (swap! calls conj x) true))] (dosync (alter r inc) (alter r inc)) @calls)"
        ),
        "[0 2]"
    );
    // t-watch-commit (one fire: old committed, new committed).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (dosync (alter r inc) (alter r inc)) @seen)"
        ),
        "[[0 2]]"
    );
    // Divergence (t-watch-order, t-watch-order3,
    // t-watch-order-shuffled, t-watch-shuffled2, t-watch-keys4,
    // t-watch-mixedkeys, t-watch-samefn, t-watch-keys8): the JVM
    // fires in key-hash order; the port fires insertion order (its
    // atom convention — S19 §1e).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (dosync (alter r inc)) @seen)"
        ),
        "[:a :b]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (dosync (alter r inc)) @seen)"
        ),
        "[:a :b :c]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (dosync (alter r inc)) @seen)"
        ),
        "[:b :a :c]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (dosync (alter r inc)) @seen)"
        ),
        "[:c :a :b]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (add-watch r :d (fn [_ _ _ _] (swap! seen conj :d))) (dosync (alter r inc)) @seen)"
        ),
        "[:a :b :c :d]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r 1 (fn [_ _ _ _] (swap! seen conj 1))) (add-watch r \"s\" (fn [_ _ _ _] (swap! seen conj \"s\"))) (dosync (alter r inc)) @seen)"
        ),
        "[:a 1 \"s\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0) f (fn [k _ _ _] (swap! seen conj k))] (add-watch r :a f) (add-watch r :b f) (add-watch r :c f) (dosync (alter r inc)) @seen)"
        ),
        "[:a :b :c]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (doseq [k [:k1 :k2 :k3 :k4 :k5 :k6 :k7 :k8]] (add-watch r k (fn [_ _ _ _] (swap! seen conj k)))) (dosync (alter r inc)) @seen)"
        ),
        "[:k1 :k2 :k3 :k4 :k5 :k6 :k7 :k8]"
    );
    // t-watch-strkeys (insertion coincides with the JVM here).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r \"a\" (fn [_ _ _ _] (swap! seen conj \"a\"))) (add-watch r \"b\" (fn [_ _ _ _] (swap! seen conj \"b\"))) (add-watch r \"c\" (fn [_ _ _ _] (swap! seen conj \"c\"))) (dosync (alter r inc)) @seen)"
        ),
        "[\"a\" \"b\" \"c\"]"
    );
    // t-watch-order-atom (control: the port's CURRENT atom order is
    // insertion — the JVM row reads `[:b :a]`, a pre-existing atom
    // divergence S19 mirrors rather than relitigates).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) a (atom 0)] (add-watch a :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch a :b (fn [_ _ _ _] (swap! seen conj :b))) (swap! a inc) @seen)"
        ),
        "[:a :b]"
    );
    // JVM-only: t-key-hashes — the port hashes relationally, never
    // literally (no pin asserts a `hash` value anywhere in the port).
    // JVM-only: t-setvalidator-in-txn — `set-validator!` is
    // out-of-scope in the port (matrix row 644); the row needs it.
    // t-watch-abort (an abort fires nothing).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (try (dosync (alter r inc) (throw (s19-ex \"boom\"))) (catch Exception e :caught)) @seen)"
        ),
        "[]"
    );
    // t-addwatch-in-txn (watches install immediately, even in-txn).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (dosync (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (alter r inc)) @seen)"
        ),
        "[[0 1]]"
    );
    // t-addwatch-returns, t-removewatch-returns (`identical?` → `=`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (= (add-watch r :k (fn [& _])) r))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (add-watch r :k (fn [& _])) [(= (remove-watch r :k) r) (dosync (alter r inc))])"
        ),
        "[true 1]"
    );
    // e-validator-msg (commit veto, terminal).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0 :validator even?)] (dosync (alter r (constantly 3))))"
        ),
        ise("Invalid reference state")
    );
}

#[test]
fn send_deferral() {
    let mut eval = boot_stm();
    // t-send-defer (the action observes the committed value).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send a (fn [_] @r))) (await a) [@r @a])"
        ),
        "[5 5]"
    );
    // t-send-returns-agent (`identical?` → `=`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (dosync (= (send a identity) a)))"
        ),
        "true"
    );
    // t-sendoff-defer.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send-off a (fn [_] @r))) (await a) [@r @a])"
        ),
        "[5 5]"
    );
    // t-send-abort (the deferral dies with the attempt).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) a (agent 0)] [(try (dosync (alter r (constantly 5)) (send a (fn [_] 99)) (throw (s19-ex \"boom\"))) :committed (catch Exception e :caught)) (do (await a) [@r @a])])"
        ),
        "[:caught [0 0]]"
    );
    // t-sendvia-defer (the `nil` executor — the JVM static field has
    // no port surface; executors are ignored on both sides).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send-via nil a (fn [_] @r))) (await a) [@r @a])"
        ),
        "[5 5]"
    );
    // t-send-badagent-in-txn (arity/cast validation stays eager).
    assert_eq!(
        uncaught(&mut eval, "(dosync (send :x (fn [_] 1)))"),
        cce("clojure.lang.Keyword", "clojure.lang.Agent")
    );
    // A send-only transaction still dispatches (the read-only fast
    // path skips the clock bump, not the outbox — JVM-verified), as
    // does an ensure-only transaction with a send.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (dosync (send a (fn [_] 42))) (await a) @a)"
        ),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) r (ref 0)] (dosync (ensure r) (send a (fn [_] 43))) (await a) @a)"
        ),
        "43"
    );
}

#[test]
fn await_gates() {
    let mut eval = boot();
    // t-await-noargs-in, t-await-noargs-out (the gate fires even with
    // zero agents; outside, the zero-agent call is nil).
    assert_eq!(
        uncaught(&mut eval, "(dosync (await))"),
        ise("await in transaction")
    );
    assert_eq!(run_last(&mut eval, "(await)"), "nil");
    // t-await-in-dosync, t-awaitfor-in-dosync.
    assert_eq!(
        uncaught(&mut eval, "(dosync (await (agent 0)) :waited)"),
        ise("await in transaction")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (await-for 100 (agent 0)) :waited)"),
        ise("await-for in transaction")
    );
    // t-await-action-tx, t-awaitfor-action-tx (the tx message wins
    // over the in-action message).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0) a2 (agent 0)] (send a (fn [_] (deliver p (try (dosync (await a2)) :waited (catch IllegalStateException e (ex-message e)))) 0)) (deref p 5000 :timeout))"
        ),
        "\"await in transaction\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [p (promise) a (agent 0) a2 (agent 0)] (send a (fn [_] (deliver p (try (dosync (await-for 100 a2)) :waited (catch IllegalStateException e (ex-message e)))) 0)) (deref p 5000 :timeout))"
        ),
        "\"await-for in transaction\""
    );
}

#[test]
fn io_gate() {
    let mut eval = boot();
    // t-io-in, t-io-out, t-io-multi, t-io-empty.
    assert_eq!(
        uncaught(&mut eval, "(dosync (io! :x))"),
        ise("I/O in transaction")
    );
    assert_eq!(run_last(&mut eval, "(io! :x)"), ":x");
    assert_eq!(run_last(&mut eval, "(io! 1 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "(io!)"), "nil");
    // t-sync-out, t-sync-empty, t-sync-flags-ignored,
    // t-sync-flags-need-not-be-nil, t-sync-runs-tx.
    assert_eq!(run_last(&mut eval, "(sync nil :x)"), ":x");
    assert_eq!(run_last(&mut eval, "(sync nil)"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)] [(sync (swap! c inc) :body) @c])"
        ),
        "[:body 0]"
    );
    assert_eq!(run_last(&mut eval, "(sync :whatever :x)"), ":x");
    assert_eq!(
        uncaught(&mut eval, "(sync nil (io! :x))"),
        ise("I/O in transaction")
    );
}

#[test]
fn macro_shapes() {
    let mut eval = boot();
    // t-shape-dosync (byte-identical to the JVM expansion).
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 (quote (dosync (alter r inc))))"),
        "(clojure.core/sync nil (alter r inc))"
    );
    // Divergence (t-shape-sync, t-shape-io, t-shape-io-multi): the
    // JVM expands through `LockingTransaction`/`if`; `sync`/`io!`
    // are special forms in the port, so `macroexpand-1` is the
    // identity (S19 §1b).
    assert_eq!(
        run_last(
            &mut eval,
            "(macroexpand-1 (quote (sync nil (alter r inc))))"
        ),
        "(sync nil (alter r inc))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 (quote (io! :x)))"),
        "(io! :x)"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 (quote (io! 1 2 3)))"),
        "(io! 1 2 3)"
    );
    // t-sync-arity0 (the bare analyzer error renders `Class |
    // message` with no wrapper — specials analyze directly, unlike
    // builtin macros; the cause text is the oracle text).
    assert_eq!(
        chain_of("(sync)"),
        vec![
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/sync"
                .to_owned(),
        ]
    );
}

#[test]
fn boundfn_conveyance() {
    let mut eval = boot();
    // t-bound-fn (same-thread calls observe the transaction).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync ((bound-fn [] (alter r inc)))) @r)"
        ),
        "1"
    );
    // t-future-no-convey (cross-thread conveyance drops it).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (let [p (promise)] (future (deliver p (try (alter r inc) :ok (catch IllegalStateException e :no-tx)))) (deref p 5000 :timeout))))"
        ),
        ":no-tx"
    );
}

#[test]
fn outside_tx_gate() {
    let mut eval = boot();
    // e-alter-outside, e-refset-outside, e-commute-outside,
    // e-ensure-outside.
    assert_eq!(
        uncaught(&mut eval, "(alter (ref 0) inc)"),
        ise("No transaction running")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-set (ref 0) 9)"),
        ise("No transaction running")
    );
    assert_eq!(
        uncaught(&mut eval, "(commute (ref 0) + 1)"),
        ise("No transaction running")
    );
    assert_eq!(
        uncaught(&mut eval, "(ensure (ref 7))"),
        ise("No transaction running")
    );
    // e-alter-nil-outside, e-alter-nonref-outside (the ref cast
    // precedes the gate).
    assert_eq!(
        uncaught(&mut eval, "(alter nil inc)"),
        npe(
            "Cannot invoke \"clojure.lang.Ref.alter(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null"
        )
    );
    assert_eq!(
        uncaught(&mut eval, "(alter 1 inc)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    // e-alter-nil-in, e-alter-nonref-in, e-refset-nil-in,
    // e-refset-nonref-in, e-commute-nil-in, e-commute-nonref-in,
    // e-ensure-nil-in, e-ensure-nonref-in.
    assert_eq!(
        uncaught(&mut eval, "(dosync (alter nil inc))"),
        npe(
            "Cannot invoke \"clojure.lang.Ref.alter(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null"
        )
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (alter 1 inc))"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ref-set nil 1))"),
        npe("Cannot invoke \"clojure.lang.Ref.set(Object)\" because \"ref\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ref-set 1 9))"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (commute nil + 1))"),
        npe(
            "Cannot invoke \"clojure.lang.Ref.commute(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null"
        )
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (commute 1 + 1))"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ensure nil))"),
        npe("Cannot invoke \"clojure.lang.Ref.touch()\" because \"ref\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ensure 1))"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    // e-alter-nonfn, e-commute-nonfn, e-alter-nonfn-outside,
    // e-commute-nonfn-outside, e-alter-nil-nonfn (the fn cast
    // precedes the gate AND nil-dispatch).
    assert_eq!(
        uncaught(&mut eval, "(dosync (alter (ref 0) 5))"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (commute (ref 0) 5))"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(alter (ref 0) 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(commute (ref 0) 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    assert_eq!(
        uncaught(&mut eval, "(alter nil 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    // e-alter-arity, e-commute-arity, e-refset-arity,
    // e-ensure-arity0, e-ensure-arity2, e-count-arity0,
    // e-count-arity2, e-min-arity0, e-min-arity3, e-max-arity0,
    // e-max-arity3.
    assert_eq!(
        uncaught(&mut eval, "(dosync (alter (ref 0)))"),
        arity_error("alter", 1)
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (commute (ref 0)))"),
        arity_error("commute", 1)
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ref-set (ref 0)))"),
        arity_error("ref-set", 1)
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ensure))"),
        arity_error("ensure", 0)
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (ensure (ref 0) (ref 1)))"),
        arity_error("ensure", 2)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-history-count)"),
        arity_error("ref-history-count", 0)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-history-count (ref 0) 1)"),
        arity_error("ref-history-count", 2)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history)"),
        arity_error("ref-min-history", 0)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history (ref 0) 1 2)"),
        arity_error("ref-min-history", 3)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history)"),
        arity_error("ref-max-history", 0)
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history (ref 0) 1 2)"),
        arity_error("ref-max-history", 3)
    );
}

#[test]
fn print_and_str() {
    let mut eval = boot();
    // Divergence (r-new, r-new-nil, r-new-coll): the JVM renders
    // `{:status :ready, :val …}`; the port prints every transient
    // cell with the `#object[…]` convention (S19 §2f).
    assert_ref_object(&run_last(&mut eval, "(ref 0)"));
    assert_ref_object(&run_last(&mut eval, "(ref nil)"));
    assert_ref_object(&run_last(&mut eval, "(ref {:a [1 2]})"));
    // r-str (the `toString`, never the `#object` form).
    let pr = run_last(&mut eval, "(str (ref 0))");
    assert!(
        pr.starts_with("\"clojure.lang.Ref@"),
        "missing toString prefix: {pr}"
    );
    assert!(pr.ends_with('"'), "missing string quote: {pr}");
    let hex = pr["\"clojure.lang.Ref@".len()..pr.len() - 1].to_owned();
    assert!(
        !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "bad toString hex: {pr}"
    );
    // `print-dup` has no `Ref` method (the atom shape — dispatch
    // through the `Ref` token, then the stock `No method` throw).
    assert_eq!(
        uncaught(&mut eval, "(binding [*print-dup* true] (pr-str (ref 0)))"),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.lang.Ref\""
    );
    // `print-simple` renders a ref's meta prefix (the atom
    // `s2-atom` shape: `^meta` plus the `toString`).
    assert!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (ref 1 :meta {:a 1}) *out*)))"
        )
        .starts_with("\"^{:a 1} clojure.lang.Ref@")
    );
}

#[test]
fn history_trim() {
    let mut eval = boot();
    // r-history-init, r-history-after, r-history-after2 (defaults
    // trim to zero).
    assert_eq!(run_last(&mut eval, "(ref-history-count (ref 0))"), "0");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (alter r inc)) (ref-history-count r))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "0"
    );
    // r-history-min-keeps, r-history-max-trims, r-history-default-many,
    // r-history-min3-many.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 5)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 0 :max-history 1)] (dosync (alter r inc)) (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (dotimes [_ 15] (dosync (alter r inc))) (ref-history-count r))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 3)] (dotimes [_ 8] (dosync (alter r inc))) (ref-history-count r))"
        ),
        "3"
    );
}

#[test]
fn history_accessors() {
    let mut eval = boot();
    // e-min-nonref, e-max-nonref, e-count-nonref, e-count-nil,
    // e-min-get-nil, e-max-get-nil.
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history 5)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history 5)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-history-count 5)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-history-count nil)"),
        npe("Cannot invoke \"clojure.lang.Ref.getHistoryCount()\" because \"ref\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history nil)"),
        npe("Cannot invoke \"clojure.lang.Ref.getMinHistory()\" because \"ref\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history nil)"),
        npe("Cannot invoke \"clojure.lang.Ref.getMaxHistory()\" because \"ref\" is null")
    );
    // e-min-nonnum, e-max-nonnum, e-min-double, e-max-double,
    // e-min-nil, e-max-nil (the §1a ladder).
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history (ref 0) :x)"),
        cce("clojure.lang.Keyword", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history (ref 0) :x)"),
        cce("clojure.lang.Keyword", "java.lang.Number")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 3.9) (ref-min-history r))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-max-history r 3.9) (ref-max-history r))"
        ),
        "3"
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history (ref 0) nil)"),
        npe("Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref-max-history (ref 0) nil)"),
        npe("Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null")
    );
    // e-min-huge, e-min-negbig, e-min-nan, e-min-longbig,
    // e-min-char, e-min-bool, e-min-bigint, e-min-str, e-min-ratio,
    // e-min-bigdec, e-min-negratio, e-min-bigint-big,
    // e-min-negdouble, e-min-doublebig.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 1e300) (ref-min-history r))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r -1e300) (ref-min-history r))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: -1.0E300"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r ##NaN) (ref-min-history r))"
        ),
        "0"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 3000000000) (ref-min-history r))"
        ),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r \\a) (ref-min-history r))"
        ),
        cce("java.lang.Character", "java.lang.Number")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r true) (ref-min-history r))"
        ),
        cce("java.lang.Boolean", "java.lang.Number")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 5N) (ref-min-history r))"
        ),
        "5"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r \"3\") (ref-min-history r))"
        ),
        cce("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 7/2) (ref-min-history r))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 7.9M) (ref-min-history r))"
        ),
        "7"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r -7/2) (ref-min-history r))"
        ),
        "-3"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 9999999999999999999999N) (ref-min-history r))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 9999999999999999999999"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r -3.9) (ref-min-history r))"
        ),
        "-3"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 3e9) (ref-min-history r))"
        ),
        "java.lang.ArithmeticException | integer overflow"
    );
}

#[test]
fn stm_portside_pins() {
    let mut eval = boot_stm();
    // A send to a failed agent throws EAGERLY in-txn (the body
    // aborts before later forms run — oracle `u-send-failed-pos`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0) c (atom 0)] (send a (fn [_] (throw (s19-ex \"boom\")))) (try (await-for 2000 a) (catch Throwable _ :failed)) [(try (dosync (send a inc) (swap! c inc) :ok) (catch RuntimeException e (ex-message e))) @c])"
        ),
        "[\"Agent is failed, needs restart\" 0]"
    );
    // `alter-meta!`/`reset-meta!` work on refs; `vary-meta` is the
    // `IObj` cast (oracle `u-altermeta`, `u-resetmeta`, `u-varymeta`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :meta {:a 1})] [(alter-meta! r assoc :b 2) (meta r)])"
        ),
        "[{:a 1, :b 2} {:a 1, :b 2}]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :meta {:a 1})] [(reset-meta! r {:c 3}) (meta r)])"
        ),
        "[{:c 3} {:c 3}]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0 :meta {:a 1})] (vary-meta r assoc :b 2))"
        ),
        cce("clojure.lang.Ref", "clojure.lang.IObj")
    );
    // `recur` cannot cross `sync` (the JVM expansion runs the body
    // in a fresh `fn`), but an inner loop recurs freely and `io!`
    // never fences (oracle `u-recur-across`, `u-recur-inside`,
    // `u-recur-io`).
    let mut fenced = boot();
    let form = read_str(
        "(loop [i 0] (if (< i 3) (dosync (recur (inc i))) i))",
        &ReaderOpts::default(),
    )
    .unwrap();
    assert_eq!(
        fenced.analyze(&form).unwrap_err().chain(),
        vec![
            "Syntax error compiling recur at (user:1:33).".to_owned(),
            "java.lang.UnsupportedOperationException | Cannot recur across try".to_owned(),
        ]
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(dosync (loop [i 0] (if (< i 3) (recur (inc i)) i)))"
        ),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0] (if (< i 3) (io! (recur (inc i))) i))"
        ),
        "3"
    );
    // `await-for` checks arity before the tx gate; `await` gates
    // before agent validation (oracle `u-awaitfor-arity-in`,
    // `u-await-badarg-in`).
    assert_eq!(
        uncaught(&mut eval, "(dosync (await-for))"),
        arity_error("await-for", 0)
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (await 5))"),
        ise("await in transaction")
    );
    // A throwing validator vetoes with its cause attached; a
    // throwing watch propagates after installing (oracle
    // `u-validator-throw`, `u-watch-throw`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :validator (fn [x] (if (zero? x) true (throw (s19-ex \"vboom\")))))] [(try (dosync (alter r inc)) :committed (catch IllegalStateException e [:thrown (ex-message e) (some? (ex-cause e))])) @r])"
        ),
        "[[:thrown \"Invalid reference state\" true] 0]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (add-watch r :k (fn [& _] (throw (s19-ex \"wboom\")))) [(try (dosync (alter r inc)) :committed (catch Exception e [:thrown (ex-message e)])) @r])"
        ),
        "[[:thrown \"wboom\"] 1]"
    );
    // A huge ratio follows the `long` ladder (oracle `u-min-hugeratio`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (ref-min-history r 9999999999999999999999/1) (ref-min-history r))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 9999999999999999999999"
    );
    // Accessor arity precedes the receiver check (oracle
    // `u-min-arity-vs-recv`).
    assert_eq!(
        uncaught(&mut eval, "(ref-min-history 5 1 2)"),
        arity_error("ref-min-history", 3)
    );
    // Flags-only `sync`, ensure-only transactions, throwing commute
    // fns, and read-after-`ref-set` (oracle `u-sync-flags-only`,
    // `u-ensure-only`, `u-commute-throw`, `u-refset-deref`).
    assert_eq!(run_last(&mut eval, "(sync :whatever)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(let [r (ref 5)] [(dosync (ensure r)) @r])"),
        "[5 5]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(try (dosync (commute r (fn [_] (throw (s19-ex \"cboom\"))))) :c (catch Exception e [:thrown (ex-message e)])) @r])"
        ),
        "[[:thrown \"cboom\"] 0]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [r (ref 0)] (dosync (ref-set r 42) @r))"),
        "42"
    );
    // The alter fn runs BEFORE the set-guard (its effects and throws
    // surface — oracle `u-setaftercommute-throwfn`,
    // `u-setaftercommute-nilfn`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) c (atom 0)] [(try (dosync (commute r + 1) (alter r (fn [x] (swap! c inc) (throw (s19-ex \"fboom\"))))) :ok (catch Exception e [:thrown (ex-message e)])) @c @r])"
        ),
        "[[:thrown \"fboom\"] 1 0]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (ref 0)] (dosync (commute r + 1) (alter r nil)))"
        ),
        npe("Cannot invoke \"clojure.lang.IFn.applyTo(clojure.lang.ISeq)\" because \"fn\" is null")
    );
    // Nil fns pass the `IFn` checkcast, then fail at the call (oracle
    // `u-alter-nil-nilfn`, `u-alter-ref-nilfn`,
    // `u-alter-ref-nilfn-in`, `u-commute-nilfn-in`).
    assert_eq!(
        uncaught(&mut eval, "(alter nil nil)"),
        npe(
            "Cannot invoke \"clojure.lang.Ref.alter(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null"
        )
    );
    assert_eq!(
        uncaught(&mut eval, "(alter (ref 0) nil)"),
        ise("No transaction running")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (alter (ref 0) nil))"),
        npe("Cannot invoke \"clojure.lang.IFn.applyTo(clojure.lang.ISeq)\" because \"fn\" is null")
    );
    assert_eq!(
        uncaught(&mut eval, "(dosync (commute (ref 0) nil))"),
        npe("Cannot invoke \"clojure.lang.IFn.applyTo(clojure.lang.ISeq)\" because \"fn\" is null")
    );
    // A non-nil non-ref beats a non-fn fun (oracle
    // `u-alter-nonref-nonfn`, `u-commute-nonref-nonfn`).
    assert_eq!(
        uncaught(&mut eval, "(alter 1 5)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    assert_eq!(
        uncaught(&mut eval, "(commute 1 5)"),
        cce("java.lang.Long", "clojure.lang.Ref")
    );
    // A rerun-only throw aborts with nothing installed (the fn
    // throws on its second call — the commit re-run).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) r (ref 0)] [(try (dosync (commute r (fn [x] (swap! c inc) (when (= @c 2) (throw (s19-ex \"rboom\"))) (inc x)))) :ok (catch Exception e [:thrown (ex-message e)])) @c @r])"
        ),
        "[[:thrown \"rboom\"] 2 0]"
    );
    // A read-only ref rides the same commit (unlocked revalidation).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0) s (ref 10)] (dosync (alter r inc) @s))"
        ),
        "10"
    );
    // Nil watch fns install but never fire (the atom convention).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) r (ref 0)] (add-watch r :nil nil) (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (dosync (alter r inc)) @seen)"
        ),
        "[[0 1]]"
    );
    // Negative bounds trim to zero; `min > max` trims to `min`
    // (oracle `u-ref-min-neg1-count`, `u-ref-max-neg1-count`,
    // `u-ref-min2max1-count`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history -1)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :max-history -1)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 2 :max-history 1)] (dosync (alter r inc)) (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r))"
        ),
        "2"
    );
    // Falsy `:meta` never installs; a non-map `:meta` and a non-fn
    // `:validator` fail their casts (oracle `u-ref-meta-nonmap`,
    // `u-ref-validator-nonfn`).
    assert_eq!(run_last(&mut eval, "(meta (ref 0 :meta nil))"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(ref 0 :meta 5)"),
        cce("java.lang.Long", "clojure.lang.IPersistentMap")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref 0 :validator 5)"),
        cce("java.lang.Long", "clojure.lang.IFn")
    );
    // The validator runs before the bounds install, whatever the
    // option order (oracle `u-ref-val-first`, `u-ref-val-first2`).
    assert_eq!(
        uncaught(&mut eval, "(ref 3 :validator even? :min-history :x)"),
        ise("Invalid reference state")
    );
    assert_eq!(
        uncaught(&mut eval, "(ref 3 :min-history :x :validator even?)"),
        ise("Invalid reference state")
    );
    // Rerun-phase writes validate, install, and notify (oracle
    // `u-rerun-s-watch`, `u-rerun-s-veto2` — the veto shape uses a
    // custom validator since `even?` would veto construction).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) s (ref 100) r (ref 0)] (add-watch s :k (fn [_ _ o n] (swap! seen conj [o n]))) [(dosync (commute r (fn [x] (alter s + 1) (inc x)))) @seen])"
        ),
        "[1 [[100 102]]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (ref 100 :validator (fn [x] (not= x 102))) r (ref 0)] [(try (dosync (commute r (fn [x] (alter s + 1) (inc x)))) :ok (catch IllegalStateException e [:thrown (ex-message e)])) @r @s])"
        ),
        "[[:thrown \"Invalid reference state\"] 0 100]"
    );
    // Validate-phase writes install and notify but skip validation
    // (oracle `u-valphase-s-veto`, `u-valphase-watch`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (ref 100 :validator even?) r (ref 0 :validator (fn [x] (when-not (zero? x) (alter s (constantly 7))) true))] [(try (dosync (alter r inc)) :ok (catch Throwable t [:thrown (ex-message t)])) @r @s])"
        ),
        "[:ok 1 7]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom []) s (ref 100) r (ref 0 :validator (fn [x] (when-not (zero? x) (alter s + 1)) true))] (add-watch s :k (fn [_ _ o n] (swap! seen conj [o n]))) [(dosync (alter r inc)) @seen @s])"
        ),
        "[1 [[100 101]] 101]"
    );
    // Divergence: a commute recorded during the re-run applies
    // tx-locally but never reruns (the JVM fail-fasts with
    // `ConcurrentModificationException` — the port snapshots).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) r (ref 0)] [(dosync (commute r (fn [x] (swap! c inc) (when (= @c 2) (commute r (fn [y] (+ y 100)))) (inc x)))) @r @c])"
        ),
        "[1 1 2]"
    );
    // A rerun-fn write to a ref untouched in-txn validates through
    // the unlocked arm (it joined after the lock phase) and still
    // installs.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) s (ref 100) r (ref 0)] [(dosync (commute r (fn [x] (swap! c inc) (when (= @c 2) (alter s + 1)) (inc x)))) @r @s @c])"
        ),
        "[1 1 101 2]"
    );
    // Refs hash by identity down the `hasheq` ladder (id-derived,
    // so pinned relationally, never literally); `(format "%h" r)`
    // renders the `.hashCode` ladder as hex (likewise relational).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] [(integer? (hash r)) (= (hash r) (hash r))])"
        ),
        "[true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (boolean (re-matches #\"[0-9a-f]+\" (format \"%h\" r))))"
        ),
        "true"
    );
    // Refs dedup by identity through the hashed `distinct` path.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0)] (count (doall (distinct [r r]))))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (ref 0) b (ref 0)] (count (doall (distinct [a b]))))"
        ),
        "2"
    );
}

#[test]
fn stm_retry_under_contention() {
    // A genuine conflict retry, deterministically: the reader opens
    // (fixing its start point), a writer commits twice past
    // `max-history 0` (force-trimming the reader's version), the
    // reader's deref strands into the internal `Retry`, and the
    // fresh attempt commits. The body runs exactly twice.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 1 :max-history 0) c (atom 0) p1 (promise) p2 (promise)] (future (deref p1) (dosync (alter r inc)) (dosync (alter r inc)) (deliver p2 true)) (dosync (swap! c inc) (deliver p1 true) (deref p2) (deref r)) [@c @r])"
        ),
        "[2 2]"
    );
}

#[test]
fn stm_revalidate_retry() {
    // A genuine revalidation retry, deterministically: the reader
    // records version 0, a writer commits past it (`min-history 1`
    // keeps the version resolvable, so the attempt reaches commit
    // instead of stranding), the commit's revalidation fails, and
    // the fresh attempt commits. The body runs exactly twice.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (ref 0 :min-history 1) c (atom 0) p1 (promise) p2 (promise)] (future (deref p1) (dosync (alter r inc)) (deliver p2 true)) (dosync (swap! c inc) @r (deliver p1 true) (deref p2) (alter r inc)) [@c @r])"
        ),
        "[2 2]"
    );
}

#[test]
fn seam_suites_green() {
    // Agent/future/atom smoke through the §1f seam: outside a
    // transaction every send path is byte-identical (these rows
    // mirror the `concurrency.rs` suite's core shapes).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [a (agent 0)] (send a inc) (await a) @a)"),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (agent 0)] (send-off a (fn [_] 7)) (await a) @a)"
        ),
        "7"
    );
    assert_eq!(run_last(&mut eval, "@(future (+ 1 2))"), "3");
    assert_eq!(
        run_last(&mut eval, "(let [a (atom 0)] (swap! a + 5) @a)"),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(let [p (promise)] (deliver p 9) @p)"),
        "9"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (agent 0)] (await a) :ok)"),
        ":ok"
    );
}
