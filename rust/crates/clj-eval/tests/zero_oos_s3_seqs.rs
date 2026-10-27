// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S3 seq/coll walks: probe replay for `bounded-count` /
//! `flatten` / `merge-deep` / `merge-deep-with` / `partitionv` /
//! `partitionv-all` / `reduce-kv` / `run!` / `splitv-at` /
//! `tree-seq` / `update-keys` / `update-vals` (see
//! `docs/zero-oos-s3-seqs/probes/seqwalks.{clj,out}`). Labels name
//! the oracle rows they replay.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| pr(&realized(eval, v)))
        .collect()
}

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
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
            panic!("expected an uncaught throw for {src}, got {vs:?}")
        }
    }
}

#[test]
fn bounded_count_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(bounded-count 5 [1 2 3])"), "3");
    assert_eq!(run_last(&mut eval, "(bounded-count 2 [1 2 3])"), "3");
    assert_eq!(
        run_last(&mut eval, "(bounded-count 3 (map inc (range 10)))"),
        "3"
    );
    assert_eq!(
        run_last(&mut eval, "(bounded-count 20 (map inc (range 10)))"),
        "10"
    );
    // Early exit: terminates over the infinite range.
    assert_eq!(run_last(&mut eval, "(bounded-count 3 (range))"), "3");
    assert_eq!(run_last(&mut eval, "(bounded-count 0 (range))"), "0");
    assert_eq!(run_last(&mut eval, "(bounded-count 5 nil)"), "0");
    assert_eq!(run_last(&mut eval, "(bounded-count 5 [])"), "0");
    assert_eq!(run_last(&mut eval, "(bounded-count 2 \"abc\")"), "2");
    assert_eq!(run_last(&mut eval, "(bounded-count 1 {:a 1 :b 2})"), "2");
}

#[test]
fn tree_seq_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(tree-seq sequential? seq [[1] [2 [3]]])"),
        "([[1] [2 [3]]] [1] 1 [2 [3]] 2 [3] 3)"
    );
    assert_eq!(run_last(&mut eval, "(tree-seq sequential? seq 1)"), "(1)");
    assert_eq!(
        run_last(&mut eval, "(tree-seq sequential? seq nil)"),
        "(nil)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(take 4 (tree-seq (fn [_] true) (fn [n] [(inc n)]) 0))"
        ),
        "(0 1 2 3)"
    );
    // Children never called when branch? is false.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom 0)] [(doall (tree-seq (fn [_] false) (fn [n] (swap! calls inc) [n]) :r)) @calls])"
        ),
        "[(:r) 0]"
    );
}

#[test]
fn flatten_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(flatten [1 [2 [3]]])"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(flatten nil)"), "()");
    assert_eq!(run_last(&mut eval, "(flatten [1 2 3])"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(flatten [\"ab\" [1]])"), "(\"ab\" 1)");
    assert_eq!(run_last(&mut eval, "(flatten [{:a 1} [2]])"), "({:a 1} 2)");
    assert_eq!(
        run_last(&mut eval, "(take 3 (flatten (iterate (fn [x] [x]) 1)))"),
        "(1 1 1)"
    );
}

#[test]
fn merge_deep_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(merge-deep {:a {:b 1 :c 1}} {:a {:b 2}})"),
        "{:a {:b 2, :c 1}}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(merge-deep {:a {:x 1}} {:a {:y 2}} {:a {:x 9 :z 3}})"
        ),
        "{:a {:x 9, :y 2, :z 3}}"
    );
    assert_eq!(run_last(&mut eval, "(merge-deep {:a 1} {:a 2})"), "{:a 2}");
    assert_eq!(
        run_last(&mut eval, "(merge-deep {:a 1} {:a {:b 2}})"),
        "{:a {:b 2}}"
    );
    assert_eq!(
        run_last(&mut eval, "(merge-deep {:a {:b 2}} {:a 1})"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(merge-deep)"), "nil");
    assert_eq!(run_last(&mut eval, "(merge-deep {:a 1})"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(merge-deep {:a 1} nil)"), "{:a 1}");
    assert_eq!(
        run_last(&mut eval, "(:added (meta #'merge-deep))"),
        "\"1.13\""
    );
}

#[test]
fn merge_deep_with_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(merge-deep-with + {:a {:n 1} :t 1} {:a {:n 2} :t 2})"
        ),
        "{:a {:n 3}, :t 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(merge-deep-with + {:n 1} {:n 2} {:n 4})"),
        "{:n 7}"
    );
    assert_eq!(
        run_last(&mut eval, "(merge-deep-with str {:a 1} {:a 2})"),
        "{:a \"12\"}"
    );
    assert_eq!(run_last(&mut eval, "(merge-deep-with +)"), "nil");
    assert_eq!(run_last(&mut eval, "(merge-deep-with + {:a 1})"), "{:a 1}");
    assert_eq!(
        run_last(&mut eval, "(:added (meta #'merge-deep-with))"),
        "\"1.13\""
    );
}

#[test]
fn partitionv_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(partitionv 2 [1 2 3 4])"),
        "([1 2] [3 4])"
    );
    assert_eq!(run_last(&mut eval, "(partitionv 2 [1 2 3])"), "([1 2])");
    assert_eq!(
        run_last(&mut eval, "(partitionv 2 1 [1 2 3])"),
        "([1 2] [2 3])"
    );
    assert_eq!(
        run_last(&mut eval, "(partitionv 2 3 [1 2 3 4 5])"),
        "([1 2] [4 5])"
    );
    assert_eq!(
        run_last(&mut eval, "(partitionv 2 2 [:p :p] [1 2 3])"),
        "([1 2] [3 :p])"
    );
    assert_eq!(
        run_last(&mut eval, "(partitionv 3 3 [:p] [1 2 3 4])"),
        "([1 2 3] [4 :p])"
    );
    assert_eq!(run_last(&mut eval, "(partitionv 2 [])"), "()");
    assert_eq!(run_last(&mut eval, "(partitionv 2 nil)"), "()");
    assert_eq!(
        run_last(&mut eval, "(take 3 (partitionv 0 [1 2]))"),
        "([] [] [])"
    );
    assert_eq!(
        run_last(&mut eval, "(take 2 (partitionv 2 (range)))"),
        "([0 1] [2 3])"
    );
    assert_eq!(
        run_last(&mut eval, "(:added (meta #'partitionv))"),
        "\"1.12\""
    );
}

#[test]
fn partitionv_all_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(partitionv-all 2 [1 2 3])"),
        "([1 2] [3])"
    );
    assert_eq!(
        run_last(&mut eval, "(partitionv-all 2 1 [1 2 3])"),
        "([1 2] [2 3] [3])"
    );
    assert_eq!(run_last(&mut eval, "(partitionv-all 2 [])"), "()");
    assert_eq!(run_last(&mut eval, "(partitionv-all 2 nil)"), "()");
    // The 1-arity stateful transducer.
    assert_eq!(
        run_last(&mut eval, "(into [] (partitionv-all 2) [1 2 3])"),
        "[[1 2] [3]]"
    );
    assert_eq!(run_last(&mut eval, "(into [] (partitionv-all 2) [])"), "[]");
    assert_eq!(
        run_last(&mut eval, "(into [] (partitionv-all 2) [1 2])"),
        "[[1 2]]"
    );
    assert_eq!(
        run_last(&mut eval, "(into [] (partitionv-all 3) [1 2 3 4])"),
        "[[1 2 3] [4]]"
    );
    assert_eq!(
        run_last(&mut eval, "(take 2 (partitionv-all 2 (range)))"),
        "([0 1] [2 3])"
    );
    assert_eq!(
        run_last(&mut eval, "(:added (meta #'partitionv-all))"),
        "\"1.12\""
    );
}

#[test]
fn reduce_kv_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(reduce-kv (fn [a k v] (+ a v)) 0 {:a 1 :b 2})"),
        "3"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce-kv (fn [a k v] (assoc a k v)) {} [:x :y])"
        ),
        "{0 :x, 1 :y}"
    );
    assert_eq!(
        run_last(&mut eval, "(reduce-kv (fn [a k v] (conj a k)) [] [:x :y])"),
        "[0 1]"
    );
    assert_eq!(
        run_last(&mut eval, "(reduce-kv (fn [a k v] a) :init nil)"),
        ":init"
    );
    assert_eq!(
        run_last(&mut eval, "(reduce-kv (fn [a k v] a) :init {})"),
        ":init"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce-kv (fn [a k v] (if (= k :b) (reduced :stop) (conj a k))) [] {:a 1 :b 2 :c 3})"
        ),
        ":stop"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce-kv (fn [a k v] (if (= k 1) (reduced :stop) (conj a v))) [] [:x :y :z])"
        ),
        ":stop"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce-kv (fn [a k v] (conj a k)) [] (sorted-map :b 1 :a 2))"
        ),
        "[:a :b]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce-kv (fn [a k v] (+ a v)) 0 (seq {:a 1 :b 2}))"
        ),
        "3"
    );
    // Records ride the slow path (same order as kvreduce).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defrecord S3K [a b]) (reduce-kv (fn [acc k v] (conj acc [k v])) [] (->S3K 1 2)))"
        ),
        "[[:a 1] [:b 2]]"
    );
    // Non-entries fail the Map$Entry cast with byte-identical
    // text to the JVM (oracle rk-list/rk-str/rk-entry).
    assert_eq!(
        uncaught(&mut eval, "(reduce-kv (fn [a k v] a) 0 '(1 2))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(reduce-kv (fn [a k v] a) 0 \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.util.Map$Entry (java.lang.Character and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(reduce-kv (fn [a k v] a) 0 (find {:a 1} :a))"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn run_bang_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom [])] [(run! (fn [x] (swap! seen conj x)) [1 2 3]) @seen])"
        ),
        "[nil [1 2 3]]"
    );
    assert_eq!(run_last(&mut eval, "(run! println nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(run! println [])"), "nil");
    // Reduced short-circuits the walk; run! still answers nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [seen (atom [])] [(run! (fn [x] (swap! seen conj x) (when (= x 2) (reduced :s))) [1 2 3]) @seen])"
        ),
        "[nil [1 2]]"
    );
}

#[test]
fn splitv_at_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(splitv-at 2 [1 2 3 4])"),
        "[[1 2] (3 4)]"
    );
    assert_eq!(run_last(&mut eval, "(splitv-at 0 [1 2])"), "[[] (1 2)]");
    assert_eq!(run_last(&mut eval, "(splitv-at 9 [1 2])"), "[[1 2] ()]");
    assert_eq!(run_last(&mut eval, "(splitv-at -1 [1 2])"), "[[] (1 2)]");
    assert_eq!(run_last(&mut eval, "(splitv-at 2 nil)"), "[[] ()]");
    assert_eq!(
        run_last(&mut eval, "(splitv-at 2 (map inc (range 4)))"),
        "[[1 2] (3 4)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(mapv (fn [x] [(vector? x) (seq? x)]) (splitv-at 1 [1 2]))"
        ),
        "[[true false] [false true]]"
    );
}

#[test]
fn update_vals_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(update-vals {:a 1 :b 2} inc)"),
        "{:a 2, :b 3}"
    );
    assert_eq!(run_last(&mut eval, "(update-vals {} inc)"), "{}");
    assert_eq!(run_last(&mut eval, "(update-vals nil inc)"), "{}");
    assert_eq!(run_last(&mut eval, "(update-vals [1 2 3] inc)"), "[2 3 4]");
    // Sorted maps take the {} route: plain map out.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (update-vals (sorted-map :b 1 :a 2) inc)] [(sorted? m) (seq m)])"
        ),
        "[false ([:a 3] [:b 2])]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (update-vals (with-meta {:a 1} {:k :v}) inc))"
        ),
        "{:k :v}"
    );
    // Records drop record-ness through the round-trip (oracle
    // uv-record).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defrecord S3R [a b]) (let [m (update-vals (->S3R 1 2) inc)] [(record? m) (into {} m)]))"
        ),
        "[false {:a 2, :b 3}]"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-vals (find {:a 1} :a) inc)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-vals '(1 2) inc)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-vals #{1 2} inc)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn update_keys_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(update-keys {:a 1 :b 2} name)"),
        "{\"a\" 1, \"b\" 2}"
    );
    assert_eq!(run_last(&mut eval, "(update-keys {} name)"), "{}");
    assert_eq!(run_last(&mut eval, "(update-keys nil name)"), "{}");
    // Vectors become maps keyed by the stringified ordinals.
    assert_eq!(
        run_last(&mut eval, "(update-keys [:a :b] str)"),
        "{\"0\" :a, \"1\" :b}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (update-keys (sorted-map :b 1 :a 2) name)] [(sorted? m) (seq m)])"
        ),
        "[false ([\"a\" 2] [\"b\" 1])]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (update-keys (with-meta {:a 1} {:k :v}) name))"
        ),
        "{:k :v}"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-keys '(1 2) name)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-keys #{1 2} name)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn arity_miss_rows() {
    let mut eval = boot();
    // The house `defn` emits anonymous fns, so arity misses name the
    // volatile `user/evalN/fn--M` display (the JVM names
    // `clojure.core/NAME` — oracle `*-arity*` rows); pin the class,
    // the count, and the anonymous-fn shape only.
    for (src, count) in [
        ("(bounded-count 1)", 1),
        ("(bounded-count 1 2 3)", 3),
        ("(tree-seq sequential? seq)", 2),
        ("(flatten)", 0),
        ("(flatten 1 2)", 2),
        ("(merge-deep-with)", 0),
        ("(partitionv 2)", 1),
        ("(partitionv-all)", 0),
        ("(reduce-kv (fn [a k v] a) 0)", 2),
        ("(run! println)", 1),
        ("(splitv-at 1)", 1),
        ("(update-vals {:a 1})", 1),
        ("(update-keys {:a 1})", 1),
    ] {
        let got = uncaught(&mut eval, src);
        let prefix = format!(
            "clojure.lang.ArityException | Wrong number of args ({count}) passed to: user/"
        );
        assert!(
            got.starts_with(&prefix) && got.contains("/fn--"),
            "for {src}: {got}"
        );
    }
}
