// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #226 direct-threaded native calls: analysis snapshots
//! `Global`-headed native calls (skipping the callee clone, the
//! downcasts, and the `Arc` bumps), with inline integer ops for
//! the genuine arith/compare natives. Pins results, error
//! parity, and invalidation: redefs, `binding`, `with-redefs`,
//! `alter-var-root`, and `ns-unmap` all take the slow road with
//! behavior identical to the ordinary `Invoke` road.

use clj_eval::{Evaluator, Expr, SourceError, boot};
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

fn analyze_one(eval: &mut Evaluator, src: &str) -> Expr {
    let forms = eval.read_forms(src, &mut clj_reader::NoHooks).unwrap();
    assert_eq!(forms.len(), 1, "{src}");
    eval.analyze(&forms[0]).unwrap()
}

#[test]
fn fast_arith_int_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(- 5 3)"), "2");
    assert_eq!(run_last(&mut eval, "(* 4 -3)"), "-12");
    assert_eq!(run_last(&mut eval, "(+ -5 -7)"), "-12");
    assert_eq!(run_last(&mut eval, "(- 0 0)"), "0");
    assert_eq!(run_last(&mut eval, "(< 1 2)"), "true");
    assert_eq!(run_last(&mut eval, "(< 2 2)"), "false");
    assert_eq!(run_last(&mut eval, "(<= 2 2)"), "true");
    assert_eq!(run_last(&mut eval, "(> 3 2)"), "true");
    assert_eq!(run_last(&mut eval, "(> 2 2)"), "false");
    assert_eq!(run_last(&mut eval, "(>= 1 2)"), "false");
    assert_eq!(run_last(&mut eval, "(>= 2 2)"), "true");
    assert_eq!(run_last(&mut eval, "(inc 1)"), "2");
    assert_eq!(run_last(&mut eval, "(inc -1)"), "0");
    assert_eq!(run_last(&mut eval, "(dec 1)"), "0");
    assert_eq!(run_last(&mut eval, "(dec 0)"), "-1");
    // The loop core stays on the fast road across iterations.
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [i 0 s 0] (if (< i 100) (recur (inc i) (+ s i)) s))"
        ),
        "4950"
    );
}

#[test]
fn fast_arith_overflow_falls_back() {
    let mut eval = boot();
    let overflow = "java.lang.ArithmeticException | long overflow";
    // Each op primes validated first (two steady calls), so the
    // overflow lands on the fast road and delegates to the native.
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(uncaught(&mut eval, "(+ 9223372036854775807 1)"), overflow);
    assert_eq!(run_last(&mut eval, "(- 5 3)"), "2");
    assert_eq!(run_last(&mut eval, "(- 5 3)"), "2");
    assert_eq!(uncaught(&mut eval, "(- -9223372036854775808 1)"), overflow);
    assert_eq!(run_last(&mut eval, "(* 2 3)"), "6");
    assert_eq!(run_last(&mut eval, "(* 2 3)"), "6");
    assert_eq!(uncaught(&mut eval, "(* 9223372036854775807 2)"), overflow);
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(uncaught(&mut eval, "(inc 9223372036854775807)"), overflow);
    assert_eq!(run_last(&mut eval, "(dec 0)"), "-1");
    assert_eq!(run_last(&mut eval, "(dec 0)"), "-1");
    assert_eq!(uncaught(&mut eval, "(dec -9223372036854775808)"), overflow);
}

#[test]
fn fast_arith_tower_and_arity_fall_back() {
    let mut eval = boot();
    // `+` primes validated first; the tower and arities then
    // fall back off the fast road.
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+ 1.5 2)"), "3.5");
    assert_eq!(run_last(&mut eval, "(+ 1 2 3)"), "6");
    assert_eq!(run_last(&mut eval, "(+ 1)"), "1");
    assert_eq!(run_last(&mut eval, "(< 1 2)"), "true");
    assert_eq!(run_last(&mut eval, "(< 1 2)"), "true");
    assert_eq!(run_last(&mut eval, "(< \"s\")"), "true");
    assert_eq!(run_last(&mut eval, "(< 1 2 3)"), "true");
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(run_last(&mut eval, "(inc 1.5)"), "2.5");
    assert_eq!(
        uncaught(&mut eval, "(+ \"a\" 1)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(inc 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/inc"
    );
    assert_eq!(run_last(&mut eval, "(dec 0)"), "-1");
    assert_eq!(run_last(&mut eval, "(dec 0)"), "-1");
    assert_eq!(
        uncaught(&mut eval, "(dec 1 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/dec"
    );
}

#[test]
fn direct_general_and_owned() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(str :a \"b\" 1)"), "\":ab1\"");
    assert_eq!(run_last(&mut eval, "(str)"), "\"\"");
    assert_eq!(run_last(&mut eval, "(conj)"), "[]");
    assert_eq!(run_last(&mut eval, "(conj [1] 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(nth [1 2] 1)"), "2");
    // Owned-args loopers run their snapshot entry on the fast road.
    assert_eq!(run_last(&mut eval, "(reduce + 0 [1 2 3])"), "6");
    assert_eq!(run_last(&mut eval, "(mapv inc [1 2])"), "[2 3]");
}

#[test]
fn direct_general_native_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(nth [] 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(inc)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/inc"
    );
    assert_eq!(
        uncaught(&mut eval, "(dec)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/dec"
    );
}

#[test]
fn zero_arg_shapes() {
    let mut eval = boot();
    // Each op primes validated first, so the 0-arity shape runs
    // the general road off the fast path.
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+)"), "0");
    assert_eq!(run_last(&mut eval, "(- 5 3)"), "2");
    assert_eq!(run_last(&mut eval, "(- 5 3)"), "2");
    assert_eq!(
        uncaught(&mut eval, "(-)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/-"
    );
    assert_eq!(run_last(&mut eval, "(< 1 2)"), "true");
    assert_eq!(run_last(&mut eval, "(< 1 2)"), "true");
    assert_eq!(
        uncaught(&mut eval, "(<)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/<"
    );
}

#[test]
fn slow_road_stale_closure_after_redef() {
    let mut eval = boot();
    // The closure snapshots the genuine `+` at analysis; the
    // redef bumps the epoch, so both the stale site and fresh
    // sites take the slow road and see `-`.
    run_last(
        &mut eval,
        "(def adder (eval (list 'fn '[x] (list '+ 'x 1))))",
    );
    assert_eq!(run_last(&mut eval, "(adder 10)"), "11");
    run_last(&mut eval, "(intern 'clojure.core '+ -)");
    assert_eq!(run_last(&mut eval, "(adder 10)"), "9");
    assert_eq!(run_last(&mut eval, "(+ 10 3)"), "7");
}

#[test]
fn slow_road_with_redefs_ptr_mismatch() {
    let mut eval = boot();
    // The bare `+` deref caches the swapped root under the live
    // epoch, so the direct site validates epoch-but-not-identity
    // and runs `-`.
    assert_eq!(
        run_last(&mut eval, "(with-redefs [+ -] (do + (+ 10 3)))"),
        "7"
    );
    assert_eq!(run_last(&mut eval, "(+ 10 3)"), "13");
}

#[test]
fn slow_road_with_redefs_non_fn() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(with-redefs [+ 5] (do + (+ 1 2)))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
}

#[test]
fn slow_road_binding() {
    let mut eval = boot();
    // A dynamic alias over the genuine native: steady calls run
    // the fast road, the `binding` shadows it down the slow road.
    run_last(&mut eval, "(def ^:dynamic f +)");
    assert_eq!(run_last(&mut eval, "(f 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(binding [f -] (f 10 3))"), "7");
    assert_eq!(run_last(&mut eval, "(f 1 2)"), "3");
}

#[test]
fn slow_road_cache_miss_then_fast() {
    let mut eval = boot();
    // Two dynamic aliases over the genuine native: the first
    // call slow-roads on the epoch the defs bumped, the second
    // alias slow-roads on a cache miss under the live epoch, and
    // both then run validated.
    run_last(&mut eval, "(def ^:dynamic g +)");
    run_last(&mut eval, "(def ^:dynamic h +)");
    assert_eq!(run_last(&mut eval, "(g 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(h 3 4)"), "7");
    assert_eq!(run_last(&mut eval, "(h 3 4)"), "7");
    assert_eq!(run_last(&mut eval, "(g 1 2)"), "3");
}

#[test]
fn slow_road_throwing_arg() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-redefs [+ -] (+ (throw (ex-info \"boom\" {})) 1))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
}

#[test]
fn fast_road_throwing_args() {
    let mut eval = boot();
    // Each road primes validated first (two steady calls), so the
    // throwing arg propagates off the fast road it names.
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "(+ 1 2)"), "3");
    assert_eq!(
        uncaught(&mut eval, "(+ (throw (ex-info \"kaboom\" {})) 1)"),
        "clojure.lang.ExceptionInfo | kaboom"
    );
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(run_last(&mut eval, "(inc 0)"), "1");
    assert_eq!(
        uncaught(&mut eval, "(inc (throw (ex-info \"k2\" {})))"),
        "clojure.lang.ExceptionInfo | k2"
    );
    assert_eq!(run_last(&mut eval, "(str \"a\")"), "\"a\"");
    assert_eq!(run_last(&mut eval, "(str \"a\")"), "\"a\"");
    assert_eq!(
        uncaught(&mut eval, "(str (throw (ex-info \"k3\" {})))"),
        "clojure.lang.ExceptionInfo | k3"
    );
}

#[test]
fn alter_var_root_pins() {
    let mut eval = boot();
    // Nested `do` analyzes upfront (root `+`), so the swap lands
    // down the slow road.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x 3] (do (alter-var-root #'+ (fn [_] *)) (+ x 4)))"
        ),
        "12"
    );
    assert_eq!(run_last(&mut eval, "(+ 3 4)"), "12");
}

#[test]
fn unmap_frozen_mapping() {
    let mut eval = boot();
    // Same for `ns-unmap`: the direct site was analyzed before
    // the unmap, and the frozen mapping answers the root.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x 1] (do (ns-unmap 'clojure.core '+) (+ x 3)))"
        ),
        "4"
    );
}

#[test]
fn unchecked_math_parity() {
    let mut eval = boot();
    // The port reports `long overflow` under bound
    // `*unchecked-math*` (no wrapping); the fast road delegates
    // the overflow, so it reports exactly that.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*unchecked-math* true] (+ 9223372036854775807 1))"
        ),
        "java.lang.ArithmeticException | long overflow"
    );
}

#[test]
fn analyze_shapes() {
    let mut eval = boot();
    assert!(matches!(
        analyze_one(&mut eval, "(+ 1 2)"),
        Expr::DirectNative(_)
    ));
    let debug = format!("{:?}", analyze_one(&mut eval, "(+ 1 2)"));
    assert!(debug.contains("DirectCall"), "{debug}");
    // Non-global heads stay ordinary invokes.
    assert!(matches!(
        analyze_one(&mut eval, "((fn [x] x) 1)"),
        Expr::Invoke { .. }
    ));
    // So do globals whose roots are not natives.
    run_last(&mut eval, "(def x 5)");
    assert!(matches!(analyze_one(&mut eval, "(x)"), Expr::Invoke { .. }));
    run_last(&mut eval, "(defn f [a] a)");
    assert!(matches!(
        analyze_one(&mut eval, "(f 1)"),
        Expr::Invoke { .. }
    ));
}
