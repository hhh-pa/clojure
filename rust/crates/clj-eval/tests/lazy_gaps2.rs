// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S8 coverage round 2: close the `lazy.rs` line gaps left by
//! `lazy_gaps.rs` (`/tmp/eval-subset3.info`, `clj-eval/src/lazy.rs`).
//! Every test below executes at least one currently-open (`DA:0`)
//! line; see `/tmp/lazy-r2-map.md` for the line-to-test map and for
//! the provably-dead arms (restructure proposals, no test can reach
//! them). All expectations are JVM-oracle-verified except the
//! fail-fast `=`-over-infinite UOE pins (the JVM hangs there; the
//! repo deliberately throws, mirroring the established `hash` UOE
//! pins in `control.rs`).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => panic!("expected an uncaught throw for {src}, got {vs:?}"),
    }
}

#[test]
fn lazy2_realized_states() {
    let mut eval = boot();
    // is_realized: pending reads false, forced chunks and forced
    // empties read true (never forcing, like LazySeq.isRealized).
    assert_eq!(run_last(&mut eval, "(realized? (map inc [1 2]))"), "false");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (map inc [1 2])] (doall s) (realized? s))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(let [s (map inc [])] (doall s) (realized? s))"),
        "true"
    );
}

#[test]
fn lazy2_subvector_step() {
    let mut eval = boot();
    // Lazy consumers step the raw subvector view (not its seq).
    assert_eq!(
        run_last(&mut eval, "(doall (map inc (subvec [1 2 3] 1)))"),
        "(3 4)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (filter odd? (subvec [1 2 3 4] 1)))"),
        "(3)"
    );
}

#[test]
fn lazy2_step_errors() {
    let mut eval = boot();
    // Non-seqable extensions fail the ISeq coercion both when
    // stepped through a lazy consumer and when seqed directly.
    assert_eq!(
        uncaught(&mut eval, "(doall (map inc (atom 1)))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Atom"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq (atom 1))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Atom"
    );
}

#[test]
fn lazy2_empty_input_steps() {
    let mut eval = boot();
    // Lazy consumers over empty maps/strings step to empty.
    assert_eq!(run_last(&mut eval, "(doall (map inc {}))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (filter odd? \"\"))"), "()");
}

#[test]
fn lazy2_keys_spent_underlying_rows() {
    let mut eval = boot();
    // Entry views over a pending that forces empty: the view stays
    // non-nil (keys_empty never forces) but every read/step/rest of
    // it observes exhaustion. Each row below forces a different
    // consumer over the same spent shape.
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (keys (concat (seq {:a 1}) (filter odd? [2]))))"
        ),
        "(:a)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(nil? (first (rest (keys (concat (seq {:a 1}) (filter odd? [2]))))))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (rest (rest (keys (concat (seq {:a 1}) (filter odd? [2]))))))"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (partition-by odd? (rest (keys (concat (seq {:a 1}) (filter odd? [2]))))))"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (mapcat (fn [_] (rest (keys (concat (seq {:a 1}) (filter odd? [2]))))) [1]))"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (rest (cycle (rest (keys (concat (seq {:a 1}) (filter odd? [2])))))))"
        ),
        "()"
    );
}

#[test]
fn lazy2_keys_rest_shapes() {
    let mut eval = boot();
    // keys_rest over single-chunk, multi-chunk, list, and
    // nil-tailed underlyings (advancing never casts, so raw
    // ranges/vectors are fine here; counts keep map order out).
    assert_eq!(run_last(&mut eval, "(doall (rest (keys [[:a 1]])))"), "()");
    assert_eq!(
        run_last(&mut eval, "(count (rest (keys (vec (seq {:a 1 :b 2})))))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(count (rest (vals (vec (seq {:a 1 :b 2})))))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(nil? (rest (keys (quote ([:a 1] [:b 2])))))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(doall (rest (keys (range 1))))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (rest (keys (first (partition 1 [5])))))"),
        "()"
    );
}

#[test]
fn lazy2_keys_seq_views() {
    let mut eval = boot();
    // seq over entry views checks the underlying bounds without
    // forcing (nil? never touches the entries, so no cast).
    assert_eq!(
        run_last(&mut eval, "(nil? (seq (keys (range 3))))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(nil? (seq (keys (repeat 2 :a))))"),
        "false"
    );
}

#[test]
fn lazy2_idrop_range() {
    let mut eval = boot();
    // drop/nthrest shift long ranges eagerly; double ranges stay lazy.
    assert_eq!(
        run_last(&mut eval, "(doall (drop 2 (range 10)))"),
        "(2 3 4 5 6 7 8 9)"
    );
    assert_eq!(
        run_last(&mut eval, "(nthrest (range 10) 3)"),
        "(3 4 5 6 7 8 9)"
    );
    assert_eq!(run_last(&mut eval, "(doall (drop 10 (range 10)))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (range 1.5))"), "(0 1)");
    assert_eq!(run_last(&mut eval, "(doall (drop 1 (range 1.5)))"), "(1)");
}

#[test]
fn lazy2_idrop_repeat() {
    let mut eval = boot();
    // drop shifts repeats eagerly: infinite drops return the same
    // repeat, counted drops narrow, past-end drops answer ().
    assert_eq!(
        run_last(&mut eval, "(doall (drop 2 (repeat 5 :a)))"),
        "(:a :a :a)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (take 2 (drop 1 (repeat :a))))"),
        "(:a :a)"
    );
    assert_eq!(run_last(&mut eval, "(doall (drop 9 (repeat 2 :a)))"), "()");
}

#[test]
fn lazy2_idrop_realized() {
    let mut eval = boot();
    // drop/nthrest shift chunked/string realized seqs eagerly.
    assert_eq!(run_last(&mut eval, "(doall (drop 2 (seq [1 2])))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (drop 1 (seq [1 2 3])))"),
        "(2 3)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (drop 1 (seq \"abc\")))"),
        "(\\b \\c)"
    );
    assert_eq!(run_last(&mut eval, "(nthrest (seq [1 2 3]) 1)"), "(2 3)");
}

#[test]
fn lazy2_idrop_map() {
    let mut eval = boot();
    // In-bounds drops over small maps shift eagerly (counts keep
    // map order out of the pins).
    assert_eq!(run_last(&mut eval, "(count (drop 1 {:a 1 :b 2}))"), "1");
    assert_eq!(
        run_last(&mut eval, "(count (drop 2 {:a 1 :b 2 :c 3}))"),
        "1"
    );
}

#[test]
fn lazy2_drop_zero_empty() {
    let mut eval = boot();
    // Non-positive drops seq eagerly; empty colls answer ().
    assert_eq!(run_last(&mut eval, "(doall (drop 0 []))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (drop -1 []))"), "()");
}

#[test]
fn lazy2_nthrest_counts() {
    let mut eval = boot();
    // Double counts: integral doubles drop exactly; fractional
    // doubles walk the (dec n) loop like the JVM.
    assert_eq!(run_last(&mut eval, "(nthrest [1 2 3] 1.0)"), "(2 3)");
    assert_eq!(
        run_last(&mut eval, "(doall (nthrest (map inc [1 2 3]) 1.0))"),
        "(3 4)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (nthrest (map inc [1 2 3]) 1.5))"),
        "(4)"
    );
}

#[test]
fn lazy2_nthrest_errors() {
    let mut eval = boot();
    // The (pos? n) coercion runs before coll dispatch: nil is the
    // getClass NPE, non-numbers the Number CCE.
    assert_eq!(
        uncaught(&mut eval, "(nthrest [1 2] nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(nthrest [1 2] :a)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn lazy2_nthrest_overrun() {
    let mut eval = boot();
    // Huge counts walk to the end silently, mapping empty to ().
    assert_eq!(
        run_last(&mut eval, "(doall (nthrest (map inc [1 2]) 5))"),
        "()"
    );
    assert_eq!(run_last(&mut eval, "(doall (nthrest nil 2))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (nthrest (quote (1 2)) 5))"),
        "()"
    );
}

#[test]
fn lazy2_mapcat_one_outer() {
    let mut eval = boot();
    // mapcat over one-at-a-time outers splices item by item and
    // skips nil/empty items.
    assert_eq!(
        run_last(&mut eval, "(doall (mapcat list (quote (1 2))))"),
        "(1 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (mapcat (fn [x]) (quote (1 2))))"),
        "()"
    );
}

#[test]
fn lazy2_unit_fn_lists() {
    let mut eval = boot();
    // The whole-unit fns step one-at-a-time inputs item by item.
    assert_eq!(
        run_last(&mut eval, "(doall (dedupe (quote (1 1 2))))"),
        "(1 2)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (keep (fn [x] (if (odd? x) x)) (quote (1 2 3))))"
        ),
        "(1 3)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (map-indexed (fn [i x] [i x]) (quote (7 8))))"
        ),
        "([0 7] [1 8])"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (keep-indexed (fn [i x] (if (odd? i) x)) (quote (10 20 30 40))))"
        ),
        "(20 40)"
    );
}

#[test]
fn lazy2_dedupe_dup_unit() {
    let mut eval = boot();
    // A unit that is entirely a run against the last kept item is
    // skipped without emitting.
    assert_eq!(
        run_last(&mut eval, "(doall (dedupe (concat [1] [1 1])))"),
        "(1)"
    );
}

#[test]
fn lazy2_keep_indexed_nil_unit() {
    let mut eval = boot();
    // A unit whose every result is nil advances with indices kept.
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (keep-indexed (fn [i x] (if (odd? i) x)) (concat [10] [20 30 40])))"
        ),
        "(20 40)"
    );
}

#[test]
fn lazy2_partition_all_empty() {
    let mut eval = boot();
    // partition-all over empty/nil inputs answers ().
    assert_eq!(run_last(&mut eval, "(doall (partition-all 2 []))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (partition-all 2 nil))"), "()");
}

#[test]
fn lazy2_rest_empty_pending() {
    let mut eval = boot();
    // rest of a pending that forces empty answers ().
    // (Needs the R-2341 rest_value fast-path to cover line 2341;
    // without it these cover the already-covered nil arm instead.)
    assert_eq!(run_last(&mut eval, "(doall (rest (map inc [])))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (rest (filter odd? [2])))"),
        "()"
    );
}

#[test]
fn lazy2_next_pending_tail() {
    let mut eval = boot();
    // next nils when the rest is a pending that forces empty.
    assert_eq!(run_last(&mut eval, "(next (cons 1 (map inc [])))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(next (cons 1 (filter odd? [2])))"),
        "nil"
    );
}

#[test]
fn lazy2_rest_cons_tail() {
    let mut eval = boot();
    // rest of a cons over () answers ().
    assert_eq!(
        run_last(&mut eval, "(doall (rest (cons 1 (quote ()))))"),
        "()"
    );
}

#[test]
fn lazy2_drop_overrun_lazy() {
    let mut eval = boot();
    // Lazy drops past the end answer ().
    assert_eq!(run_last(&mut eval, "(doall (drop 9 (quote (1 2))))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (drop 9 (map inc [1 2])))"),
        "()"
    );
}

#[test]
fn lazy2_partition_zero_step() {
    let mut eval = boot();
    // Non-positive steps replay the window start (take-bounded).
    assert_eq!(
        run_last(&mut eval, "(doall (take 2 (partition 2 0 [1 2 3])))"),
        "((1 2) (1 2))"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (take 2 (partition-all 2 0 [1 2 3])))"),
        "((1 2) (1 2))"
    );
}

#[test]
fn lazy2_partition_double_step() {
    let mut eval = boot();
    // Double steps ceil through the IDrop coercion over vectors.
    assert_eq!(
        run_last(&mut eval, "(doall (partition 2 1.0 [1 2 3]))"),
        "((1 2) (2 3))"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (partition-all 2 1.0 [1 2 3]))"),
        "((1 2) (2 3) (3))"
    );
}

#[test]
fn lazy2_partition_double_width() {
    let mut eval = boot();
    // Double widths never fill: the JVM compares `(= n (count
    // p))` and `Numbers.equal` is category-strict (`(= 3.0 3)` is
    // false — oracled), so every double width drops and the run
    // ends.
    assert_eq!(run_last(&mut eval, "(doall (partition 2.5 [1 2 3]))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (partition 2.5 1 [1 2 3]))"),
        "()"
    );
    assert_eq!(run_last(&mut eval, "(doall (partition 3.0 [1 2 3]))"), "()");
    assert_eq!(
        run_last(&mut eval, "(doall (partition 3.0 1 [1 2 3]))"),
        "()"
    );
}

#[test]
fn lazy2_range_double_end() {
    let mut eval = boot();
    // 1-arity ranges with double ends: positives walk from 0,
    // non-positives answer ().
    assert_eq!(run_last(&mut eval, "(doall (range 2.5))"), "(0 1 2)");
    assert_eq!(run_last(&mut eval, "(doall (range -1.5))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (range 0.0))"), "()");
}

#[test]
fn lazy2_range_double_mixed() {
    let mut eval = boot();
    // Mixed int/double bounds walk the double path with verbatim
    // heads (ints stay ints until a double promotes).
    assert_eq!(run_last(&mut eval, "(doall (range 0 3 1.5))"), "(0 1.5)");
    assert_eq!(
        run_last(&mut eval, "(doall (range 0.5 3))"),
        "(0.5 1.5 2.5)"
    );
}

#[test]
fn lazy2_range_double_empty() {
    let mut eval = boot();
    // Double-path empties: wrong-direction bounds and equal bounds.
    assert_eq!(run_last(&mut eval, "(doall (range 5.0 1.0))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (range 1 1 1.5))"), "()");
}

#[test]
fn lazy2_range_zero_steps() {
    let mut eval = boot();
    // Zero steps repeat the start forever (take-bounded).
    assert_eq!(
        run_last(&mut eval, "(doall (take 3 (range 5 10 0)))"),
        "(5 5 5)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (take 3 (range 0 3 0.0)))"),
        "(0 0 0)"
    );
}

#[test]
fn lazy2_range_huge_count() {
    let mut eval = boot();
    // Counts past i32 fall back to the double path with int bounds;
    // only bounded consumers are pinned (never count).
    assert_eq!(run_last(&mut eval, "(first (range 0 5000000000 1))"), "0");
    assert_eq!(run_last(&mut eval, "(nth (range 0 5000000000 1) 3)"), "3");
    assert_eq!(
        run_last(&mut eval, "(doall (take 3 (range 0 5000000000 1)))"),
        "(0 1 2)"
    );
}

#[test]
fn lazy2_range_overflow_bounds() {
    let mut eval = boot();
    // Overflow counts fall back too; an Int+Int overflow ends the
    // walk like the JVM forceChunk, and rest of it answers ().
    assert_eq!(
        run_last(
            &mut eval,
            "(first (range 5 9223372036854775807 9223372036854775807))"
        ),
        "5"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (rest (range 5 9223372036854775807 9223372036854775807)))"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(doall (take 2 (range 5 9223372036854775807 9223372036854775807)))"
        ),
        "(5)"
    );
}

#[test]
fn lazy2_equiv_seq_matrix() {
    let mut eval = boot();
    // Sequential = walks pairwise with early exit: drained-side
    // shapes (one/chunk) and buffered-shorter sides answer false,
    // and item mismatches answer false.
    assert_eq!(run_last(&mut eval, "(= [2] (quote (2 3)))"), "false");
    assert_eq!(run_last(&mut eval, "(= [2] [2 3])"), "false");
    assert_eq!(run_last(&mut eval, "(= [2 3] [2])"), "false");
    assert_eq!(run_last(&mut eval, "(= [1] [2])"), "false");
    assert_eq!(run_last(&mut eval, "(= [2] (concat [2] [3]))"), "false");
}

#[test]
fn lazy2_equiv_map_rows() {
    let mut eval = boot();
    // Map = compares size plus per-key recursion; finite repeat
    // keys hash and compare structurally.
    assert_eq!(run_last(&mut eval, "(= {:a 1} {:a 1 :b 2})"), "false");
    assert_eq!(run_last(&mut eval, "(= {:a 1} {:b 1})"), "false");
    assert_eq!(run_last(&mut eval, "(= {:a 1} {:a 2})"), "false");
    assert_eq!(
        run_last(&mut eval, "(= {(repeat 2 :a) 1} {(repeat 2 :a) 1})"),
        "true"
    );
}

#[test]
fn lazy2_equiv_map_uoe() {
    let mut eval = boot();
    // Infinite cycle/iterate keys throw the bare UOE (fail-fast;
    // the JVM hangs hashing them instead — untestable, mirroring
    // the established hash-UOE pins). Repeat keys cannot be built
    // (construction hashes and hangs first), so cycles pin this.
    assert_eq!(
        uncaught(&mut eval, "(= {(cycle [1]) 1} {(cycle [1]) 1})"),
        "java.lang.UnsupportedOperationException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(= {(iterate inc 0) 1} {(iterate inc 0) 1})"),
        "java.lang.UnsupportedOperationException | "
    );
}

#[test]
fn lazy2_equiv_set_rows() {
    let mut eval = boot();
    // Set = compares size plus per-member search.
    assert_eq!(run_last(&mut eval, "(= #{1 2} #{2 1})"), "true");
    assert_eq!(run_last(&mut eval, "(= #{1} #{1 2})"), "false");
    assert_eq!(run_last(&mut eval, "(= #{1} #{2})"), "false");
}

#[test]
fn lazy2_equiv_set_uoe() {
    let mut eval = boot();
    // Infinite cycle/iterate members throw the bare UOE (same
    // fail-fast rationale as the map-key pins above).
    assert_eq!(
        uncaught(&mut eval, "(= #{(cycle [1])} #{(cycle [1])})"),
        "java.lang.UnsupportedOperationException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(= #{(iterate inc 0)} #{(iterate inc 0)})"),
        "java.lang.UnsupportedOperationException | "
    );
}

#[test]
fn lazy2_counted_seqs() {
    let mut eval = boot();
    // count reads the Counted flag off realized seqs.
    assert_eq!(run_last(&mut eval, "(count (seq [1 2 3]))"), "3");
    assert_eq!(run_last(&mut eval, "(count (rest [1 2 3]))"), "2");
}

#[test]
fn lazy2_thunk0_recur() {
    let mut eval = boot();
    // An empty recur re-invokes the lazy-seq body (the zero-arg fn
    // boundary, like the JVM closure); the atom gates termination.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 0)] (doall (lazy-seq (if (< (swap! a inc) 2) (recur) [(deref a)]))))"
        ),
        "(2)"
    );
}

#[test]
fn lazy2_iterate_nil_fn() {
    let mut eval = boot();
    // The seed emits free without calling f; the first advance
    // through a nil f is the field NPE.
    assert_eq!(run_last(&mut eval, "(first (iterate nil 5))"), "5");
    assert_eq!(
        uncaught(&mut eval, "(second (iterate nil 5))"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"this.f\" is null"
    );
}

#[test]
fn lazy2_rest_tail_nils() {
    let mut eval = boot();
    // Stepped tails that are nil/empty answer `()`: cons over an
    // empty coll seqs its tail to nil, and a single-item subvector
    // remainder is an empty realized seq.
    assert_eq!(run_last(&mut eval, "(doall (rest (cons 1 [])))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (rest (subvec [1] 0)))"), "()");
}
