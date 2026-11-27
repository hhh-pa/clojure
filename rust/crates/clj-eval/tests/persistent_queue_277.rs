// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #277: `clojure.lang.PersistentQueue` (a FIFO window, O(1) `pop`)
//! against JVM Clojure 1.13.0-alpha10 on JDK 26. Every row quotes a
//! probe of `docs/spec/i277-recipes/probes/queue.clj` and its JVM
//! answer from `queue.out`; identity hashes (`0x...`, random on the JVM)
//! are masked on both sides.

use clj_eval::{Evaluator, boot};
use clj_value::Value;

/// `0x<hex>` identity hashes as `0xID`.
fn mask(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("0x") {
        out.push_str(&rest[..i]);
        out.push_str("0xID");
        rest = rest[i + 2..].trim_start_matches(|c: char| c.is_ascii_hexdigit());
    }
    out.push_str(rest);
    out
}

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

#[test]
fn variadic_queue_conj_preserves_the_shared_prefix_and_metadata() {
    let mut eval = boot();
    assert_eq!(
        outcome(
            &mut eval,
            "(let [q (with-meta (conj clojure.lang.PersistentQueue/EMPTY :a) {:tag true})
                   grown (conj q nil :b :c)]
               [(vec q) (vec grown) (peek (pop grown)) (meta grown)])"
        ),
        "[[:a] [:a nil :b :c] nil {:tag true}]"
    );
}

#[test]
fn variadic_conj_still_extends_a_subvector() {
    assert_eq!(
        outcome(&mut boot(), "(conj (subvec [0 1] 1) 2 3)"),
        "[1 2 3]"
    );
}

/// `(label, expression, JVM answer)`.
const ORACLE: &[(&str, &str, &str)] = &[
    (
        "q1 pr",
        r##"q"##,
        r##"#object[clojure.lang.PersistentQueue 0x3a7704c "clojure.lang.PersistentQueue@7861"]"##,
    ),
    (
        "q2 str",
        r##"(str q)"##,
        r##""clojure.lang.PersistentQueue@7861""##,
    ),
    (
        "q3 print",
        r##"(with-out-str (print q))"##,
        r##""#object[clojure.lang.PersistentQueue 0x3a7704c clojure.lang.PersistentQueue@7861]""##,
    ),
    ("q4 seq", r##"(seq q)"##, r##"(1 2 3)"##),
    ("q5 empty-seq", r##"(seq E)"##, r##"nil"##),
    ("q6 count", r##"(count q)"##, r##"3"##),
    ("q7 peek", r##"(peek q)"##, r##"1"##),
    (
        "q8 pop",
        r##"(pop q)"##,
        r##"#object[clojure.lang.PersistentQueue 0x5f577419 "clojure.lang.PersistentQueue@402"]"##,
    ),
    (
        "q9 pop-empty",
        r##"(pop E)"##,
        r##"#object[clojure.lang.PersistentQueue 0x11963225 "clojure.lang.PersistentQueue@1"]"##,
    ),
    ("q10 peek-empty", r##"(peek E)"##, r##"nil"##),
    (
        "q11 empty?",
        r##"[(empty? E) (empty? q)]"##,
        r##"[true false]"##,
    ),
    (
        "q12 = vec",
        r##"[(= q [1 2 3]) (= [1 2 3] q) (= q '(1 2 3))]"##,
        r##"[true true true]"##,
    ),
    ("q13 = queue", r##"(= q (conj E 1 2 3))"##, r##"true"##),
    (
        "q14 class",
        r##"(class q)"##,
        r##"clojure.lang.PersistentQueue"##,
    ),
    (
        "q15 vector?",
        r##"[(vector? q) (seq? q) (sequential? q) (coll? q) (counted? q)]"##,
        r##"[false false true true true]"##,
    ),
    (
        "q16 into",
        r##"(into E [4 5])"##,
        r##"#object[clojure.lang.PersistentQueue 0x6e5bfdfc "clojure.lang.PersistentQueue@442"]"##,
    ),
    ("q17 into-vec", r##"(into [] q)"##, r##"[1 2 3]"##),
    ("q18 nth", r##"(nth q 1)"##, r##"2"##),
    (
        "q19 first/rest",
        r##"[(first q) (rest q) (next q)]"##,
        r##"[1 (2 3) (2 3)]"##,
    ),
    (
        "q20 conj order",
        r##"(seq (conj (pop q) 4))"##,
        r##"(2 3 4)"##,
    ),
    ("q21 reduce", r##"(reduce + q)"##, r##"6"##),
    ("q22 map", r##"(map inc q)"##, r##"(2 3 4)"##),
    (
        "q23 empty",
        r##"(empty q)"##,
        r##"#object[clojure.lang.PersistentQueue 0x11963225 "clojure.lang.PersistentQueue@1"]"##,
    ),
    (
        "q24 meta",
        r##"(meta (with-meta q {:a 1}))"##,
        r##"{:a 1}"##,
    ),
    ("q25 hash=", r##"(= (hash q) (hash [1 2 3]))"##, r##"true"##),
    ("q26 get", r##"(get q 0)"##, r##"nil"##),
    (
        "q27 contains?",
        r##"(contains? q 0)"##,
        r##"java.lang.IllegalArgumentException: contains? not supported on type: clojure.lang.PersistentQueue"##,
    ),
    ("q28 apply", r##"(apply + q)"##, r##"6"##),
    ("q29 vec", r##"(vec q)"##, r##"[1 2 3]"##),
    ("q30 sort", r##"(sort (conj E 3 1 2))"##, r##"(1 2 3)"##),
    (
        "q31 bfs",
        r##"(loop [q (conj E 0) seen []] (if (empty? q) seen (let [x (peek q)] (recur (if (< x 5) (conj (pop q) (inc x)) (pop q)) (conj seen x)))))"##,
        r##"[0 1 2 3 4 5]"##,
    ),
    (
        "q32 pr-nested",
        r##"[q {:q q}]"##,
        r##"[#object[clojure.lang.PersistentQueue 0x3a7704c "clojure.lang.PersistentQueue@7861"] {:q #object[clojure.lang.PersistentQueue 0x3a7704c "clojure.lang.PersistentQueue@7861"]}]"##,
    ),
    (
        "q33 empty-pr",
        r##"E"##,
        r##"#object[clojure.lang.PersistentQueue 0x11963225 "clojure.lang.PersistentQueue@1"]"##,
    ),
    ("q34 last", r##"(last q)"##, r##"3"##),
    (
        "q35 stack fns",
        r##"[(peek (conj E)) (count (pop (pop (pop q))))]"##,
        r##"[nil 0]"##,
    ),
    (
        "q36 shared pop",
        r##"(let [q (conj E 1 2) r q] [(seq (pop q)) (seq r)])"##,
        r##"[(2) (1 2)]"##,
    ),
    (
        "q37 shared conj",
        r##"(let [q (conj E 1) r q] [(seq (conj q 2)) (seq r)])"##,
        r##"[(1 2) (1)]"##,
    ),
    (
        "q38 long bfs",
        r##"(loop [q (conj E 0) n 0] (if (empty? q) n (let [x (peek q)] (recur (if (< x 2000) (conj (pop q) (inc x) (+ x 2000)) (pop q)) (inc n)))))"##,
        r##"4001"##,
    ),
    ("q39 hashCode", r##"(.hashCode q)"##, r##"30817"##),
    (
        "q40 set member",
        r##"(contains? #{[1 2 3]} q)"##,
        r##"true"##,
    ),
];

#[test]
fn persistent_queue_answers_like_the_jvm_277() {
    let mut eval = boot();
    eval.eval_source("(def E clojure.lang.PersistentQueue/EMPTY) (def q (conj E 1 2 3))")
        .expect("setup");
    let wrong: Vec<String> = ORACLE
        .iter()
        .filter_map(|(label, expr, want)| {
            let got = mask(&outcome(&mut eval, expr));
            (got != mask(want)).then(|| format!("{label}: got {got}, JVM {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
