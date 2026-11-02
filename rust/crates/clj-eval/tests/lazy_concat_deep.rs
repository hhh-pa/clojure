// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! #205: deep `concat` nests stay iterative (stepping and dropping).
//! `mapcat` tails nest one concat per element, so `for`+`:when` and
//! `mapcat` over tens of thousands of elements used to overflow the
//! Rust stack (the harness died in
//! `count-negative-numbers-in-sorted-matrix-test`, whose brute-force
//! count walks 2M elements through a nested `for`). `step_concat`
//! now flattens nested unforced concats through an explicit resume
//! stack, and the `PendingSeq`/`ConcatFrame` drops unwind concat
//! chains iteratively. Every expectation below is a plain value
//! check (JVM-agreed shapes); the depth is the point — each of
//! these aborted with stack overflow before the fix.

use clj_eval::{Evaluator, boot};
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

/// Single `mapcat` over 50K elements (the minimal overflow repro).
#[test]
fn mapcat_50k_stays_iterative() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(count (mapcat list (range 50000)))"),
        "50000"
    );
}

/// `for` with `:when` expands to `mapcat`: 50K elements.
#[test]
fn for_when_50k_stays_iterative() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(count (for [x (range 50000) :when (odd? x)] x))"
        ),
        "25000"
    );
}

/// Nested `for`+`:when` (the brute-force-count shape): 100x2000.
#[test]
fn for_nested_200k_stays_iterative() {
    let mut eval = boot();
    eval.eval_source("(def r100 (vec (repeat 100 (vec (range 1000 -1000 -1)))))")
        .unwrap();
    assert_eq!(
        run_last(&mut eval, "(count (for [row r100 n row :when (neg? n)] n))"),
        "99900"
    );
}

/// Left-nested concats (`(reduce concat ...)`): 20K deep, stepped
/// iteratively and unwound iteratively at drop.
#[test]
fn concat_left_nest_20k_stays_iterative() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(count (reduce concat (map list (range 20000))))"
        ),
        "20000"
    );
}

/// Right-nested concats (each tail rests on the next): 20K deep.
#[test]
fn concat_right_nest_20k_stays_iterative() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(count (reduce (fn [acc x] (concat [x] acc)) nil (range 20000)))"
        ),
        "20000"
    );
}

/// A partial walk abandons a ~20K resume stack; dropping it stays
/// iterative too.
#[test]
fn concat_deep_stack_drop_stays_iterative() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (take 5 (reduce concat (map list (range 20000)))))"
        ),
        "[0 1 2 3 4]"
    );
}

/// Flattening clones inner content out without disturbing the
/// inner cell: an aliased inner still forces normally later with
/// identical items.
#[test]
fn concat_aliased_inner_forces_normally() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (concat [1 2] [3]) t (concat s [4])] [(vec t) (vec s)])"
        ),
        "[[1 2 3 4] [1 2 3]]"
    );
}

/// A pending concat between live inputs: the outer level resumes
/// past it once it exhausts.
#[test]
fn concat_middle_pending_resumes_outer() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (concat [1] (mapcat identity [[2] [3]]) [4]))"
        ),
        "[1 2 3 4]"
    );
}

/// Empties (eager, self-made, and spliced) vanish between live inputs.
#[test]
fn concat_mixed_empties_vanish() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (concat [] (concat) [1] (mapcat list []) [2]))"
        ),
        "[1 2]"
    );
}

/// A forced (memoized) tail nested as another concat's input
/// steps through the normal path, and stays usable directly
/// afterwards.
#[test]
fn concat_memoized_tail_nests_and_reuses() {
    let mut eval = boot();
    eval.eval_source("(def w (concat [1] (concat [10] [11]) [2]))")
        .unwrap();
    assert_eq!(run_last(&mut eval, "(first w)"), "1");
    // `tw` is `w`'s forced tail past `1` (`rest` memoizes it).
    eval.eval_source("(def tw (rest w))").unwrap();
    assert_eq!(
        run_last(&mut eval, "(vec (concat tw [99]))"),
        "[10 11 2 99]"
    );
    assert_eq!(run_last(&mut eval, "(vec tw)"), "[10 11 2]");
}

/// A nested inner concat spliced mid-walk: forcing the inner
/// returns a fresh tail already carrying a resume stack (the live
/// outer inputs), and the descent splices that stack above the
/// current level.
#[test]
fn concat_nested_inner_splices_stack() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (mapcat identity [(concat (concat [1] [2]) [3] [4])]))"
        ),
        "[1 2 3 4]"
    );
}
