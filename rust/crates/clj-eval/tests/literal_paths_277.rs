// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #277: the VM's literal-path `get-in` / `assoc-in` ops and owned
//! sorted-set `conj`/`disj`, against JVM Clojure 1.13.0-alpha10 on JDK 26.
//! Every row quotes a probe of `docs/spec/i277-recipes/probes/literal_paths.clj`
//! (its definitions are this suite's setup) and its JVM answer from
//! `literal_paths.out`.

use clj_eval::{Evaluator, boot};
use clj_value::Value;

const PROBES: &str = include_str!("../../../../docs/spec/i277-recipes/probes/literal_paths.clj");

/// The probes' own shape: `pr-str` of the value, or the throw.
fn outcome(eval: &mut Evaluator, expr: &str) -> String {
    let src = format!(
        "(try (pr-str {expr}) (catch Throwable e (str (.getName (class e)) \": \" (.getMessage e))))"
    );
    let values = eval.eval_source(&src).expect("the probe evaluates");
    match values.last() {
        Some(Value::Str(s)) => s.to_string(),
        other => panic!("{expr}: {other:?}"),
    }
}

/// An evaluator holding the probe file's definitions (every line that is
/// not a `(t ...)` row).
fn probe_eval() -> Evaluator {
    let mut eval = boot();
    let setup: Vec<&str> = PROBES.lines().filter(|l| !l.starts_with("(t ")).collect();
    eval.eval_source(&setup.join("\n"))
        .expect("the probe definitions load");
    eval
}

/// `(label, expression, JVM answer)`.
const ORACLE: &[(&str, &str, &str)] = &[
    ("g1 found", r##"(f1 m)"##, r##"1"##),
    ("g2 missing nf", r##"(f2 m :x)"##, r##":nf"##),
    ("g3 present nil val", r##"(f2 m :n)"##, r##"nil"##),
    ("g4 vector index", r##"(f3 m 1 0)"##, r##"3"##),
    ("g5 out of range", r##"(f3 m 5 0)"##, r##"nil"##),
    ("g6 nil intermediate nf", r##"(f4 m)"##, r##":nf"##),
    ("g7 non-literal path", r##"(f5 m [:a :b])"##, r##"1"##),
    ("g8 five keys", r##"(f6 m)"##, r##"nil"##),
    ("g9 nil coll", r##"(f1 nil)"##, r##"nil"##),
    ("g10 string coll", r##"(get-in "abc" [1])"##, r##"\b"##),
    ("g12 one key", r##"(get-in m [:v])"##, r##"[[1 2] [3 4]]"##),
    (
        "g13 order",
        r##"(let [log (atom [])] (get-in (do (swap! log conj :coll) m) [(do (swap! log conj :k1) :a) (do (swap! log conj :k2) :b)] (do (swap! log conj :nf) 0)) @log)"##,
        r##"[:coll :k1 :k2 :nf]"##,
    ),
    (
        "a1",
        r##"(a1 m 9)"##,
        r##"{:a {:b 9, :n nil}, :v [[1 2] [3 4]]}"##,
    ),
    (
        "a2",
        r##"(a2 m 1 0 :x)"##,
        r##"{:a {:b 1, :n nil}, :v [[1 2] [:x 4]]}"##,
    ),
    (
        "a3",
        r##"(a3 m)"##,
        r##"{:a {:b 1, :n nil}, :v [[1 2] [3 4]], :new {:deep {:k 1}}}"##,
    ),
    (
        "a4 append",
        r##"(a4 m)"##,
        r##"{:a {:b 1, :n nil}, :v [[1 2] [3 4] :end]}"##,
    ),
    (
        "a5 oob",
        r##"(a5 m)"##,
        r##"java.lang.IndexOutOfBoundsException: "##,
    ),
    (
        "a6 reduce",
        r##"(a6 m)"##,
        r##"{:a {:b 1, :n nil}, :v [[4 2] [3 -4]]}"##,
    ),
    (
        "a7 shared",
        r##"(let [x m y (a1 x 2)] [x y])"##,
        r##"[{:a {:b 1, :n nil}, :v [[1 2] [3 4]]} {:a {:b 2, :n nil}, :v [[1 2] [3 4]]}]"##,
    ),
    (
        "a8 nil coll",
        r##"(assoc-in nil [:a :b] 1)"##,
        r##"{:a {:b 1}}"##,
    ),
    ("a9 one key", r##"(assoc-in {} [:k] 1)"##, r##"{:k 1}"##),
    (
        "a11 string key on vector",
        r##"(assoc-in [[1]] [0 :x] 1)"##,
        r##"java.lang.IllegalArgumentException: Key must be integer"##,
    ),
    (
        "a12 order",
        r##"(let [log (atom [])] (assoc-in (do (swap! log conj :coll) m) [(do (swap! log conj :k1) :a) (do (swap! log conj :k2) :b)] (do (swap! log conj :v) 0)) @log)"##,
        r##"[:coll :k1 :k2 :v]"##,
    ),
    (
        "a13 read coll",
        r##"(a13 m)"##,
        r##"[{:a {:b 0, :n nil}, :v [[1 2] [3 4]]} 1]"##,
    ),
    (
        "a14 exec coll",
        r##"(assoc-in (identity m) [:a :b] 5)"##,
        r##"{:a {:b 5, :n nil}, :v [[1 2] [3 4]]}"##,
    ),
    (
        "s1 sorted pq",
        r##"(loop [h (sorted-set [3 :c] [1 :a] [2 :b]) out []] (if (empty? h) out (let [x (first h)] (recur (disj h x) (conj out (second x))))))"##,
        r##"[:a :b :c]"##,
    ),
    (
        "s2 sorted grow",
        r##"(let [s (reduce conj (sorted-set) (range 600 0 -1))] [(count s) (first s) (last s) (take 3 (drop 300 s))])"##,
        r##"[600 1 600 (301 302 303)]"##,
    ),
    (
        "s3 sorted shared",
        r##"(let [s (sorted-set 3 1 2) t (conj s 0) u (disj s 1)] [s t u])"##,
        r##"[#{1 2 3} #{0 1 2 3} #{2 3}]"##,
    ),
    (
        "s4 sorted-by",
        r##"(let [s (sorted-set-by > 1 5 3)] [(conj s 4) (disj s 5)])"##,
        r##"[#{5 4 3 1} #{3 1}]"##,
    ),
];

#[test]
fn literal_paths_and_sorted_sets_answer_like_the_jvm_277() {
    let mut eval = probe_eval();
    let wrong: Vec<String> = ORACLE
        .iter()
        .filter_map(|(label, expr, want)| {
            let got = outcome(&mut eval, expr);
            (got != *want).then(|| format!("{label}: got {got}, JVM {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn literal_path_ops_follow_a_redefined_var_277() {
    // No JVM row: the JVM's compiled call keeps the original fn under
    // `with-redefs`, while this runtime's direct calls re-resolve a
    // redefined var (every direct call, not just these ops), so both ops
    // build the path vector and call the redefinition.
    let mut eval = probe_eval();
    assert_eq!(
        outcome(
            &mut eval,
            "(with-redefs [get-in (fn [c p & nf] [:get p nf])] [(f1 m) (f2 m :x)])"
        ),
        "[[:get [:a :b] nil] [:get [:a :x] (:nf)]]"
    );
    assert_eq!(
        outcome(
            &mut eval,
            "(with-redefs [assoc-in (fn [c p v] [:assoc p v])] (a1 m 7))"
        ),
        "[:assoc [:a :b] 7]"
    );
}

#[test]
fn matrix_path_preserves_present_nil_and_fallback_types() {
    let mut eval = boot();
    assert_eq!(
        outcome(
            &mut eval,
            r#"[(get-in [[nil]] [0 0] :missing)
                (get-in [[1]] [0 1] :missing)
                (get-in ["ab"] [0 1] :missing)
                (get-in [{:key 7}] [0 :key] :missing)
                (get-in [[9]] [4294967296 0] :missing)]"#,
        ),
        r#"[nil :missing \b 7 9]"#
    );
}
