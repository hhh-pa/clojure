// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D seq spot pins: `mapcat` splices (found via the `not`-fix
//! ripple — the old shape never concatenated), plus the
//! take/drop-while backlog rows.

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
fn reduced_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(reduced? (reduced 1))"), "true");
    assert_eq!(run_last(&mut eval, "(reduced? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(reduced? nil)"), "false");
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce (fn [a b] (if (> a 1) (reduced :stop) (+ a b))) [1 2 3])"
        ),
        ":stop"
    );
}

#[test]
fn mapcat_rows() {
    let mut eval = boot();
    // Each outer item is seq'd and spliced (≡ `(apply concat
    // (map f xs))`); nil/empty items vanish.
    assert_eq!(run_last(&mut eval, "(mapcat list [1 2 3])"), "(1 2 3)");
    assert_eq!(
        run_last(
            &mut eval,
            "(mapcat (fn [x] (if (odd? x) (list x))) [1 2 3])"
        ),
        "(1 3)"
    );
    assert_eq!(run_last(&mut eval, "(mapcat list nil)"), "()");
    assert_eq!(run_last(&mut eval, "(mapcat (fn [x] nil) [1 2])"), "()");
    assert_eq!(
        run_last(&mut eval, "(mapcat vector [1 2] \"ab\")"),
        "(1 \\a 2 \\b)"
    );
    // Non-seqable items fail like `concat` would.
    assert_eq!(
        uncaught(&mut eval, "(doall (mapcat (fn [x] x) [1]))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // One arg is the (out-of-scope) transducer arity.
    assert_eq!(
        uncaught(&mut eval, "(mapcat list)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/mapcat"
    );
}

#[test]
fn while_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(take-while odd? [1 3 4 5])"), "(1 3)");
    assert_eq!(run_last(&mut eval, "(drop-while odd? [1 3 4 5])"), "(4 5)");
    assert_eq!(
        run_last(&mut eval, "(split-with odd? [1 3 4 5])"),
        "[(1 3) (4 5)]"
    );
}

#[test]
fn xform_divergence_rows() {
    let mut eval = boot();
    // The 1-arg transducer arities are out of scope: arity errors
    // naming the `clojure.core/F` display form.
    for f in ["map", "filter", "remove", "keep", "keep-indexed"] {
        let src = format!("({f} list)");
        assert_eq!(
            uncaught(&mut eval, &src),
            format!(
                "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/{f}"
            ),
            "{src}"
        );
    }
    for f in ["distinct", "dedupe"] {
        let src = format!("({f})");
        assert_eq!(
            uncaught(&mut eval, &src),
            format!(
                "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/{f}"
            ),
            "{src}"
        );
    }
}

#[test]
fn remove_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(remove odd? [1 2 3 4])"), "(2 4)");
    assert_eq!(run_last(&mut eval, "(remove odd? [1 3])"), "()");
}

#[test]
fn interleave_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(interleave)"), "()");
    assert_eq!(run_last(&mut eval, "(interleave [1 2] [3 4])"), "(1 3 2 4)");
}

#[test]
fn interpose_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(interpose :x [1 2 3])"),
        "(1 :x 2 :x 3)"
    );
}

#[test]
fn repeatedly_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(take 3 (repeatedly (fn [] 7)))"),
        "(7 7 7)"
    );
    assert_eq!(run_last(&mut eval, "(repeatedly 3 (fn [] 8))"), "(8 8 8)");
}

#[test]
fn iterate_nil_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(take 3 (iterate inc 0))"), "(0 1 2)");
    // A nil `f` passes construction (failing only on first advance).
    assert_eq!(run_last(&mut eval, "(take 0 (iterate nil 1))"), "()");
}

#[test]
fn range_arity_rows() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(range 1 2 3 4)"),
        "clojure.lang.ArityException | Wrong number of args (4) passed to: clojure.core/range"
    );
}

#[test]
fn keep_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(keep (fn [x] (if (odd? x) (* x 10))) [1 2 3])"),
        "(10 30)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(keep-indexed (fn [i x] (if (odd? i) x)) [:a :b :c :d])"
        ),
        "(:b :d)"
    );
    assert_eq!(
        run_last(&mut eval, "(map-indexed vector [10 20])"),
        "([0 10] [1 20])"
    );
}

#[test]
fn distinct_dedupe_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(distinct [1 1 2 1 3])"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(dedupe [1 1 2 2 1])"), "(1 2 1)");
}

#[test]
fn partition_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(partition 2 [1 2 3])"), "((1 2))");
    assert_eq!(
        run_last(&mut eval, "(partition 2 1 [1 2 3])"),
        "((1 2) (2 3))"
    );
    assert_eq!(
        run_last(&mut eval, "(partition 2 2 [0 0] [1 2 3])"),
        "((1 2) (3 0))"
    );
}

#[test]
fn partition_all_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(partition-all 2 [1 2 3])"),
        "((1 2) (3))"
    );
    assert_eq!(
        run_last(&mut eval, "(partition-all 2 1 [1 2 3])"),
        "((1 2) (2 3) (3))"
    );
}

#[test]
fn partition_by_take_nth_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(partition-by odd? [1 3 2 4 5])"),
        "((1 3) (2 4) (5))"
    );
    assert_eq!(run_last(&mut eval, "(take-nth 2 [1 2 3 4 5])"), "(1 3 5)");
}

#[test]
fn reductions_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(reductions + [1 2 3])"), "(1 3 6)");
    assert_eq!(run_last(&mut eval, "(reductions + 10 [1 2])"), "(10 11 13)");
    assert_eq!(
        run_last(&mut eval, "(reductions + (reduced 9) [1 2])"),
        "(9)"
    );
}

#[test]
fn reduce_empty_rows() {
    let mut eval = boot();
    // No init over an empty coll calls `(f)`.
    assert_eq!(run_last(&mut eval, "(reduce + [])"), "0");
    assert_eq!(run_last(&mut eval, "(reduce + nil)"), "0");
}

#[test]
fn take_last_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(take-last 2 [1 2 3 4])"), "(3 4)");
    assert_eq!(run_last(&mut eval, "(take-last 5 [1 2])"), "(1 2)");
}

#[test]
fn drop_last_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(drop-last [1 2 3])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(drop-last 2 [1 2 3 4])"), "(1 2)");
}

#[test]
fn butlast_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(butlast [1 2 3])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(butlast [1])"), "nil");
    assert_eq!(run_last(&mut eval, "(butlast nil)"), "nil");
}

#[test]
fn sort_bool_rows() {
    let mut eval = boot();
    // Reversed inputs force both comparator argument orders, so the
    // direct-`true` and reversed-call arms each run at least once.
    assert_eq!(run_last(&mut eval, "(sort < [1 2])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(sort < [2 1])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(sort < [1 1])"), "(1 1)");
    assert_eq!(run_last(&mut eval, "(sort > [1 3 2])"), "(3 2 1)");
}

#[test]
fn sort_num_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(sort (fn [a b] (- a b)) [3 1 2])"),
        "(1 2 3)"
    );
    assert_eq!(
        run_last(&mut eval, "(sort (fn [a b] (- a b)) [2.5 1.5])"),
        "(1.5 2.5)"
    );
    assert_eq!(
        run_last(&mut eval, "(sort (fn [a b] (- a b)) [1.5 2.5])"),
        "(1.5 2.5)"
    );
    assert_eq!(
        run_last(&mut eval, "(sort (fn [a b] (- a b)) [1.5 1.5])"),
        "(1.5 1.5)"
    );
}

#[test]
fn sort_bad_comp_rows() {
    let mut eval = boot();
    let want = concat!(
        "java.lang.ClassCastException | ",
        "class clojure.lang.Keyword cannot be cast to class java.lang.Number ",
        "(clojure.lang.Keyword is in unnamed module of loader 'app'; ",
        "java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(uncaught(&mut eval, "(sort (fn [a b] :x) [2 1])"), want);
    // Three elements force a second comparison after the failure.
    assert_eq!(uncaught(&mut eval, "(sort (fn [a b] :x) [3 1 2])"), want);
}

#[test]
fn sort_by_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(sort-by count [[1 2] [1]])"),
        "([1] [1 2])"
    );
    assert_eq!(
        run_last(&mut eval, "(sort-by count > [[1] [1 2 3] [1 2]])"),
        "([1 2 3] [1 2] [1])"
    );
    assert_eq!(run_last(&mut eval, "(sort-by count [])"), "()");
}

#[test]
fn dorun_doall_n_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(dorun 2 [1 2 3])"), "nil");
    assert_eq!(run_last(&mut eval, "(dorun 3 [1 2])"), "nil");
    assert_eq!(run_last(&mut eval, "(dorun 0 [1 2])"), "nil");
    assert_eq!(run_last(&mut eval, "(doall 2 [1 2 3])"), "[1 2 3]");
    assert_eq!(run_last(&mut eval, "(doall 3 [1])"), "[1]");
}

#[test]
fn mapv_filterv_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(mapv inc [1 2 3])"), "[2 3 4]");
    assert_eq!(run_last(&mut eval, "(mapv + [1 2] [10 20 30])"), "[11 22]");
    assert_eq!(run_last(&mut eval, "(filterv odd? [1 2 3 4])"), "[1 3]");
}

#[test]
fn group_by_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(get (group-by odd? [1 2 3 4]) true)"),
        "[1 3]"
    );
    assert_eq!(
        run_last(&mut eval, "(get (group-by odd? [1 2 3 4]) false)"),
        "[2 4]"
    );
    assert_eq!(run_last(&mut eval, "(group-by odd? [])"), "{}");
}

#[test]
fn frequencies_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(get (frequencies [:a :b :a]) :a)"),
        "2"
    );
    assert_eq!(
        run_last(&mut eval, "(get (frequencies [:a :b :a]) :b)"),
        "1"
    );
    assert_eq!(run_last(&mut eval, "(frequencies [])"), "{}");
}

#[test]
fn every_pred_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((every-pred odd?))"), "true");
    assert_eq!(
        run_last(&mut eval, "((every-pred odd? (fn [x] true)) 1 3)"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "((every-pred odd? (fn [x] false)) 1)"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "((every-pred odd?) 1 3 5 7)"), "true");
    assert_eq!(run_last(&mut eval, "((every-pred odd?) 1 3 5 8)"), "false");
    assert_eq!(
        run_last(
            &mut eval,
            "((every-pred odd? (fn [x] true) (fn [x] (odd? x)) (fn [x] true)) 1 3 5 7)"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((every-pred odd? (fn [x] true) (fn [x] (odd? x)) (fn [x] true)) 1 3 5 8)"
        ),
        "false"
    );
}

#[test]
fn some_fn_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((some-fn odd?))"), "nil");
    assert_eq!(run_last(&mut eval, "((some-fn odd?) 2 4)"), "nil");
    assert_eq!(run_last(&mut eval, "((some-fn odd?) 2 3)"), "true");
    assert_eq!(run_last(&mut eval, "((some-fn odd?) 2 4 6 7)"), "true");
    assert_eq!(run_last(&mut eval, "((some-fn odd?) 2 4 6 8)"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "((some-fn (fn [x] false) (fn [x] false) (fn [x] false) odd?) 2 4 6 7)"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((some-fn (fn [x] false) (fn [x] false) (fn [x] false) odd?) 2 4 6 8)"
        ),
        "nil"
    );
}

#[test]
fn max_min_key_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(max-key count [1] [1 2] [1 2 3])"),
        "[1 2 3]"
    );
    assert_eq!(
        run_last(&mut eval, "(max-key count [1 2 3] [1] [1 2])"),
        "[1 2 3]"
    );
    assert_eq!(run_last(&mut eval, "(min-key count [1 2 3] [1])"), "[1]");
    assert_eq!(run_last(&mut eval, "(min-key count [1] [1 2])"), "[1]");
}

#[test]
fn comp_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((comp) 5)"), "5");
    assert_eq!(run_last(&mut eval, "((comp inc) 5)"), "6");
    assert_eq!(run_last(&mut eval, "((comp inc inc) 5)"), "7");
    assert_eq!(run_last(&mut eval, "((comp str +) 1 2)"), "\"3\"");
}

#[test]
fn partial_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((partial +) 1 2)"), "3");
    assert_eq!(run_last(&mut eval, "((partial + 10) 1 2)"), "13");
    assert_eq!(
        run_last(&mut eval, "((partial str \"a\" \"b\") \"c\")"),
        "\"abc\""
    );
}

#[test]
fn complement_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((complement odd?) 2)"), "true");
    assert_eq!(run_last(&mut eval, "((complement odd?) 1)"), "false");
}

#[test]
fn juxt_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((juxt inc dec) 5)"), "[6 4]");
    assert_eq!(run_last(&mut eval, "((juxt + *) 2 3)"), "[5 6]");
}

#[test]
fn constantly_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((constantly 42) 1 2 3)"), "42");
    assert_eq!(run_last(&mut eval, "((constantly nil))"), "nil");
}

#[test]
fn fnil_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "((fnil vector :a :b) nil 2)"), "[:a 2]");
    assert_eq!(run_last(&mut eval, "((fnil vector :a :b) 1 2)"), "[1 2]");
    assert_eq!(
        uncaught(&mut eval, "((fnil vector :a :b))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/fnil"
    );
}

#[test]
fn some_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(some odd? [2 4 5 6])"), "true");
    assert_eq!(run_last(&mut eval, "(some odd? [2 4])"), "nil");
}

#[test]
fn not_any_every_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(not-any? odd? [2 4])"), "true");
    assert_eq!(run_last(&mut eval, "(not-any? odd? [2 3])"), "false");
    assert_eq!(run_last(&mut eval, "(not-every? odd? [1 3])"), "false");
    assert_eq!(run_last(&mut eval, "(not-every? odd? [1 2])"), "true");
}

#[test]
fn seq_names_reader_value_classes() {
    // Oracle O2: the `ISeq` error names the JVM class.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(seq #\"x\")"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.util.regex.Pattern"
    );
}

#[test]
fn core_r4_take_counts() {
    let mut eval = boot();
    // `NaN` takes nothing; infinities take everything-or-nothing.
    assert_eq!(run_last(&mut eval, "(doall (take ##NaN [1]))"), "()");
    assert_eq!(run_last(&mut eval, "(doall (take ##Inf [1 2]))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(doall (take ##-Inf [1]))"), "()");
    // nil is the numbers NPE; non-numbers fail to `Number`.
    assert_eq!(
        uncaught(&mut eval, "(doall (take nil [1]))"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(doall (take \"s\" [1]))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn core_r4_repeat_counts() {
    let mut eval = boot();
    // Doubles truncate (`intValue`); `NaN` is zero items.
    assert_eq!(run_last(&mut eval, "(doall (repeat 2.5 :a))"), "(:a :a)");
    assert_eq!(run_last(&mut eval, "(doall (repeat ##NaN :a))"), "()");
    // nil trips the `Repeat.create` `doubleValue` NPE; the rest
    // fails to `Number`.
    assert_eq!(
        uncaught(&mut eval, "(doall (repeat nil :a))"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(doall (repeat \"s\" :a))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // Out-of-`long` doubles fail the range check (E9; oracle: IAE).
    for (src, msg) in [
        (
            "(take 3 (repeat ##Inf :a))",
            "Value out of range for long: Infinity",
        ),
        (
            "(take 3 (repeat ##-Inf :a))",
            "Value out of range for long: -Infinity",
        ),
        ("(repeat 1e300 :a)", "Value out of range for long: 1.0E300"),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!("java.lang.IllegalArgumentException | {msg}"),
            "{src}"
        );
    }
}

#[test]
fn core_r4_reduced_rows() {
    let mut eval = boot();
    // Print shape (id is nondeterministic; prefix+suffix pinned).
    let s = run_last(&mut eval, "(pr-str (reduced 1))");
    assert!(s.starts_with("\"#object[clojure.lang.Reduced 0x"), "{s}");
    assert!(s.contains("clojure.lang.Reduced@"), "{s}");
    // Identity equality through `=`.
    assert_eq!(run_last(&mut eval, "(let [r (reduced 1)] (= r r))"), "true");
    assert_eq!(run_last(&mut eval, "(= (reduced 1) (reduced 1))"), "false");
    // Identity hashing through 3-arg `distinct?` (2-arg never hashes).
    assert_eq!(
        run_last(&mut eval, "(distinct? (reduced 1) (reduced 2) :x)"),
        "true"
    );
}

#[test]
fn invoke_non_fn_extension_is_cast_error() {
    let mut eval = boot();
    // Ranges are not `IFn` (short cast format).
    assert_eq!(
        uncaught(&mut eval, "((range 3) 0)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.LongRange to clojure.lang.IFn"
    );
}
