// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #254 unboxed loop tier (`vm_scalar`): every expression, test,
//! statement, effect and exit shape the tier lowers (with `let`
//! destructuring of an `nth`), and every miss
//! it hands back to the boxed tiers (overflow, non-scalar state or
//! invariants, frozen/map/persistent effect targets, out-of-range
//! and non-integer indexes, a redefined site, unsupported forms) —
//! including misses after an iteration's effects ran, which undo
//! them (appends, replaced and popped items, runs of appends,
//! aliased transients, a full undo log). Every row runs twice —
//! compiled by the VM and on the tree evaluator (inside
//! `(binding [] …)`) — and both must give the JVM's value (Clojure
//! 1.12). The VM runs compile each loop's program at its first entry
//! (`set_eager_tier`); the lazy rows run the default, where a loop
//! compiles once it runs hot.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

/// `pr-str` of the form's value, or `Class: message` when it throws.
fn show(eval: &mut Evaluator, form: &str) -> String {
    let src = format!(
        "(try (pr-str {form}) (catch Throwable t (str \"THROW \" (.getName (class t)) \": \" (.getMessage t))))"
    );
    let vs = eval
        .eval_source(&src)
        .unwrap_or_else(|e| panic!("{form}: {e}"));
    pr_to_string(vs.last().unwrap(), &PrintOpts::print())
}

/// An evaluator whose loops compile their programs at first entry.
fn eager() -> Evaluator {
    let mut eval = boot();
    eval.set_eager_tier(true);
    eval
}

fn check(rows: &[(&str, &str)]) {
    for (form, want) in rows {
        let mut eval = eager();
        assert_eq!(show(&mut eval, form), *want, "VM: {form}");
        let mut eval = boot();
        let tree = format!("(binding [] {form})");
        assert_eq!(show(&mut eval, &tree), *want, "tree: {form}");
    }
}

const JVM_ROWS: &[(&str, &str)] = &[
    (
        r#"(loop [i 0 x 1 y 2] (if (= i 3) (+ (* 10 x) y) (recur (inc i) y x)))"#,
        r#"21"#,
    ),
    (
        r#"(let [s "hello world"] (loop [i 0 n 0] (if (= i (count s)) n (recur (inc i) (if (= (nth s i) \o) (inc n) n)))))"#,
        r#"2"#,
    ),
    (
        r#"(let [s "héllo wörld"] (loop [i 0 n 0] (if (= i (count s)) [n i] (recur (inc i) (if (= (nth s i) \o) (inc n) n)))))"#,
        r#"[1 11]"#,
    ),
    (
        r#"(let [s (apply str (repeat 100 "ab"))] [(loop [i 0 n 0] (if (= i (count s)) n (recur (inc i) (if (= (nth s i) \a) (inc n) n)))) (loop [i 0 n 0] (if (= i (count s)) n (recur (inc i) (if (= (nth s i) \b) (inc n) n))))])"#,
        r#"[100 100]"#,
    ),
    (
        r#"(let [v [1 2 3 4]] (loop [i 0 acc 0] (if (= i (count v)) acc (recur (inc i) (+ acc (nth v i))))))"#,
        r#"10"#,
    ),
    (
        r#"(let [v [1 :a 2]] (loop [i 0 acc 0] (if (= i 3) acc (recur (inc i) (if (= (nth v i) 2) (inc acc) acc)))))"#,
        r#"1"#,
    ),
    (
        r#"(let [t (transient [0 0 0])] (loop [i 0] (if (= i (count t)) (persistent! t) (do (assoc! t i (* i i)) (recur (inc i))))))"#,
        r#"[0 1 4]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 5) (persistent! t) (do (conj! t i) (recur (inc i))))))"#,
        r#"[0 1 2 3 4]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (conj! t (= i 1)) (recur (inc i))))))"#,
        r#"[false true false]"#,
    ),
    (
        r#"(let [s "abc" t (transient [0 0 0])] (loop [i 0] (if (= i 3) (persistent! t) (do (assoc! t i (nth s i)) (recur (inc i))))))"#,
        r#"[\a \b \c]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [0 0])] (loop [i 0] (if (= i 2) [(persistent! t) (persistent! u)] (do (conj! t i) (assoc! u i (+ i 10)) (recur (inc i))))))"#,
        r#"[[0 1] [10 11]]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [0])] (try (loop [i 0] (if (= i 2) :done (do (conj! t i) (assoc! u (+ i 5) 1) (recur (inc i))))) (catch Exception e [(count t) (.getName (class e))])))"#,
        r#"[1 "java.lang.IndexOutOfBoundsException"]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (assoc! t i i) (recur (inc i))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient [0])] (try (loop [i 0] (if (= i 1) :x (do (assoc! t (- i 1) 5) (recur (inc i))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.IndexOutOfBoundsException""#,
    ),
    (
        r#"(let [t (transient [0]) s "a"] (try (loop [i 0] (if (= i 1) :x (do (assoc! t (nth s i) 5) (recur (inc i))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.IllegalArgumentException""#,
    ),
    (
        r#"(let [t (transient {})] (loop [i 0] (if (= i 2) (persistent! t) (do (assoc! t i i) (recur (inc i))))))"#,
        r#"{0 0, 1 1}"#,
    ),
    (
        r#"(let [t (transient {}) u (transient [])] (loop [i 0] (if (= i 2) [(persistent! t) (persistent! u)] (do (assoc! t i i) (conj! u i) (recur (inc i))))))"#,
        r#"[{0 0, 1 1} [0 1]]"#,
    ),
    (
        r#"(let [t [0 0]] (try (loop [i 0] (if (= i 2) :x (do (assoc! t i 1) (recur (inc i))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.ClassCastException""#,
    ),
    (
        r#"(let [t (transient [0 0])] (persistent! t) (try (loop [i 0] (if (= i 2) :x (do (assoc! t i 1) (recur (inc i))))) (catch Throwable e (.getName (class e)))))"#,
        r#""java.lang.IllegalAccessError""#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (persistent! u) (try (loop [i 0] (if (= i 1) :x (do (conj! t i) (conj! u i) (recur (inc i))))) (catch Throwable e [(count t) (.getName (class e))])))"#,
        r#"[1 "java.lang.IllegalAccessError"]"#,
    ),
    (
        r#"(loop [i 0 acc 0] (if (= i 3) acc (recur (inc i) (+ acc (int (nth "abc" i))))))"#,
        r#"294"#,
    ),
    (
        r#"(loop [i 0 acc 0] (if (= i 3) acc (recur (inc i) (+ acc (long (nth "abc" i)) (long i) (int i)))))"#,
        r#"300"#,
    ),
    (
        r#"(try (loop [i 0 acc 0] (if (= i 2) acc (recur (inc i) (+ acc (int (+ 3000000000 i)))))) (catch Exception e (.getName (class e))))"#,
        r#""java.lang.ArithmeticException""#,
    ),
    (
        r#"(try (loop [i 0 acc 0] (if (= i 2) acc (recur (inc i) (+ acc (int (= i 0)))))) (catch Exception e (.getName (class e))))"#,
        r#""java.lang.ClassCastException""#,
    ),
    (
        r#"(try (loop [i 0 acc 0] (if (= i 2) acc (recur (inc i) (+ acc (long (= i 0)))))) (catch Exception e (.getName (class e))))"#,
        r#""java.lang.ClassCastException""#,
    ),
    (
        r#"(loop [i -3 a []] (if (= i 3) a (recur (inc i) (conj a [(zero? i) (pos? i) (neg? i) (< i 1) (<= i 1) (> i 1) (>= i 1) (== i 1)]))))"#,
        r#"[[false false true true true false false false] [false false true true true false false false] [false false true true true false false false] [true false false true true false false false] [false true false false true false true true] [false true false false false true true false]]"#,
    ),
    (
        r#"(loop [i -3 n 0] (if (= i 4) n (let [z (zero? i) p (pos? i) g (neg? i) l (< i 1) le (<= i 1) gt (> i 1) ge (>= i 1) e (== i 1)] (recur (inc i) (+ n (if z 1 0) (if p 10 0) (if g 100 0) (if l 1000 0) (if le 10000 0) (if gt 100000 0) (if ge 1000000 0) (if e 10000000 0))))))"#,
        r#"13254331"#,
    ),
    (
        r#"(let [s "aAa"] (loop [i 0 n 0] (if (= i 3) n (recur (inc i) (cond (= (nth s i) \a) (+ n 1) (= (nth s i) 65) (+ n 10) (not= (nth s i) \A) (+ n 100) :else (+ n 1000))))))"#,
        r#"1002"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 6) n (recur (inc i) (if (or (= i 0) (= i 2)) (inc n) (if (and (pos? i) (< i 4) (not (= i 3))) (+ n 10) (if (not (< i 5)) (+ n 100) n))))))"#,
        r#"112"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 4) n (let [b (< i 2)] (recur (inc i) (if b (inc n) n)))))"#,
        r#"2"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 4) n (recur (inc i) (if i (inc n) n))))"#,
        r#"4"#,
    ),
    (
        r#"(let [s "aabaab"] (loop [i 0 hits 0] (if (= i 6) hits (let [j (loop [j i] (if (and (pos? j) (not= (nth s j) \a)) (recur (dec j)) j))] (recur (inc i) (+ hits j))))))"#,
        r#"13"#,
    ),
    (
        r#"(let [t "xxabcxx" p "abc"] (loop [i 0] (if (> i 4) false (let [j (loop [j 2] (cond (neg? j) -1 (= (nth p j) (nth t (+ i j))) (recur (dec j)) :else j))] (if (neg? j) i (recur (inc i)))))))"#,
        r#"2"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (recur (inc i) (+ n (loop [a i b (inc a)] (if (> a 5) b (recur (inc a) (+ b a))))))))"#,
        r#"50"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (recur (inc i) (+ n (let [a (* i 2) b (inc a)] (+ a b))))))"#,
        r#"15"#,
    ),
    (
        r#"(loop [i 0] (if (= i 3) i (let [j (inc i)] (recur j))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [j 0 m {}] (if (= j 3) m (let [k (inc j)] (recur k m))))"#,
        r#"{}"#,
    ),
    (
        r#"(let [s "x"] (loop [i 0] (if (= i 3) s (let [j (inc i)] (recur j)))))"#,
        r#""x""#,
    ),
    (
        r#"(loop [i 0] (when (< i 3) (let [j (inc i)] (recur j))))"#,
        r#"nil"#,
    ),
    (
        r#"(let [t (transient [])] [(loop [i 0] (if (= i 3) (do (conj! t 99) i) (do (conj! t i) (recur (inc i))))) (persistent! t)])"#,
        r#"[3 [0 1 2 99]]"#,
    ),
    (
        r#"(try (loop [i (- Long/MAX_VALUE 2) n 0] (if (= n 5) i (let [m (inc n)] (recur (inc i) m)))) (catch ArithmeticException e :ovf))"#,
        r#":ovf"#,
    ),
    (
        r#"(loop [i (- Long/MAX_VALUE 2) n 0] (if (= n 5) i (let [m (inc n)] (recur (inc' i) m))))"#,
        r#"9223372036854775810N"#,
    ),
    (
        r#"(loop [i 0.5 n 0] (if (> i 3) [i n] (let [m (inc n)] (recur (+ i 1) m))))"#,
        r#"[3.5 3]"#,
    ),
    (
        r#"(let [n 3.5] (loop [i 0] (if (> i n) i (let [j (inc i)] (recur j)))))"#,
        r#"4"#,
    ),
    (
        r#"(let [s "a€" ] (loop [i 0 acc 0] (if (= i 2) acc (let [c (int (nth s i))] (recur (inc i) (+ acc (if (< c 256) c (get {} c 0))))))))"#,
        r#"97"#,
    ),
    (
        r#"(let [x nil] (loop [i 0] (if (= i (count x)) i (let [j (inc i)] (recur j)))))"#,
        r#"0"#,
    ),
    (
        r#"(let [x 5] (try (loop [i 0] (if (= i 1) i (let [j (+ i (nth x i))] (recur j)))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(let [t (transient {:a 1})] (loop [i 0] (if (= i (count t)) i (let [j (inc i)] (recur j)))))"#,
        r#"1"#,
    ),
    (
        r#"(let [t (transient {0 1})] (try (loop [i 0] (if (= i 1) i (let [j (+ i (nth t i))] (recur j)))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(with-redefs [not (fn [x] true)] (loop [i 0 n 0] (if (= i 3) n (let [j (inc i)] (recur j (if (not (= i 1)) (inc n) n))))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0] (if (= i 3) i (let [[a b] [i i]] (recur (inc a)))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0] (if (= i 3) i (recur (loop [[a] [i]] (inc a)))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 2) n (recur (inc i) (+ n (let [a 1] :x a)))))"#,
        r#"2"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 2) n (recur (inc i) (if (= i n 0) (inc n) n))))"#,
        r#"1"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (recur (inc i) (if (let [g (pos? i)] (if g (not g) g)) (inc n) (+ n 10)))))"#,
        r#"30"#,
    ),
    (
        r#"(try (loop [v 0 i 0] (if (= i 2) [v i] (recur (inc v) (+ i (count v))))) (catch Exception e (.getName (class e))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(let [t (transient [0 0 0])] (loop [i 0 acc 0] (if (= i 3) acc (do (assoc! t i 5) (recur (inc i) (+ acc (nth t i)))))))"#,
        r#"15"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (recur (inc i) (+ n (count (str i))))))"#,
        r#"3"#,
    ),
    (
        r#"(let [s "ab" k 1] (loop [i 0 n 0] (if (= i 2) n (recur (inc i) (+ n (count s) (count s) k k)))))"#,
        r#"12"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 2) (persistent! t) (do (conj! t i) (recur (loop [k i] (if (< k 0) (do (conj! t k) k) (inc k))))))))"#,
        r#"[0 1]"#,
    ),
    (
        r#"(loop [i 0 c \a] (if (= i 3) c (let [j (inc i)] (recur j (nth "xyz" i)))))"#,
        r#"\z"#,
    ),
    (
        r#"(loop [i 0 b false] (if (= i 3) b (let [j (inc i)] (recur j (< i 1)))))"#,
        r#"false"#,
    ),
    (
        r#"(let [s "abc"] (loop [i 0 n 0] (if (= i 3) n (let [c (nth s i)] (recur (inc i) (if (= c \b) (inc n) n))))))"#,
        r#"1"#,
    ),
    (
        r#"(let [t (transient [1 2 3])] (loop [i 0 acc 0] (if (= i 3) acc (recur (inc i) (+ acc (nth t i))))))"#,
        r#"6"#,
    ),
    (
        r#"(let [t [] u (transient [])] (try (loop [i 0] (if (= i 1) :x (do (conj! u i) (conj! t i) (recur (inc i))))) (catch Exception e [(count u) (.getName (class e))])))"#,
        r#"[1 "java.lang.ClassCastException"]"#,
    ),
    (
        r#"(let [s "abc" t (transient [0 0 0])] (loop [i 0] (if (= i 3) (persistent! t) (let [c (nth s i)] (assoc! t i c) (recur (inc i))))))"#,
        r#"[\a \b \c]"#,
    ),
    (
        r#"(let [k 2] (loop [i 0 n 0] (if (= i 2) n (let [j (inc i)] (recur j (+ (+ n k) k))))))"#,
        r#"8"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (if (< i 10) (let [j (inc i)] (recur j (inc n))) (let [[a] [i]] (recur a n)))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0 x 1 y 2] (if (= i 3) (+ (* 10 x) y) (let [j (inc i)] (recur j y x))))"#,
        r#"21"#,
    ),
    (
        r#"(try (loop [i 0 n 0] (if (= i 2) n (let [x i] (recur (inc i) (+ n (count x)))))) (catch Exception e (.getName (class e))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 2) n (let [j (inc i)] (recur j (+ n (do 1))))))"#,
        r#"2"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 2) (do (conj! t 9) (count t)) (do (conj! t i) (recur (inc i))))))"#,
        r#"3"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 n 0] (if (= i 2) [n (persistent! t)] (do (conj! t i) (recur (inc i) (if (< i 1) 5 6))))))"#,
        r#"[6 [0 1]]"#,
    ),
    (
        r#"(let [f (fn [k] (loop [e k] (if (< e 0) -1 (let [x (dec e)] (if (= x 3) e (recur x))))))] (loop [i 0 a 0] (if (= i 40) a (recur (inc i) (+ a (f (mod i 3)))))))"#,
        r#"-40"#,
    ),
    (
        r#"(let [v [5 6 7 8 9 10] f (fn [k] (loop [e 0 n 0] (if (= e (count v)) n (let [x (nth v e)] (recur (inc e) (if (> x k) (inc n) n))))))] (loop [i 0 a 0] (if (= i 40) a (recur (inc i) (+ a (f i))))))"#,
        r#"45"#,
    ),
    (
        r#"(let [t (transient [0 0 0]) g (fn [i] (let [j (+ i 7)] (loop [k 0] (if (< k 1) (do (assoc! t i j) (recur (inc k))) k))))] (dotimes [i 40] (g (mod i 3))) (persistent! t))"#,
        r#"[7 8 9]"#,
    ),
    (
        r#"(with-redefs [not= (fn [a b] false)] (loop [i 0 n 0] (if (= i 3) n (let [j (inc i)] (recur j (if (not= i 1) (inc n) n))))))"#,
        r#"0"#,
    ),
    (
        r#"(with-redefs [mod (fn [a b] 7)] (loop [i 0 n 0] (if (= i 3) n (let [j (inc i)] (recur j (+ n (mod i 2)))))))"#,
        r#"21"#,
    ),
    (
        r#"(let [t (transient [])] (with-redefs [conj! (fn [t x] t)] (loop [i 0] (if (= i 3) (count t) (do (conj! t i) (recur (inc i)))))))"#,
        r#"0"#,
    ),
    (
        r#"(let [f (fn [k] (loop [e k] (if (>= e 0) e (let [x (inc e)] (recur x)))))] (loop [i 0 a 0] (if (= i 40) a (recur (inc i) (+ a (f i))))))"#,
        r#"780"#,
    ),
    (
        r#"(let [t (transient [0 0 0])] [(loop [i 0] (if (= i 2) (persistent! (assoc! t i 9)) (recur (inc i)))) (let [u (transient [])] (count (loop [i 0] (if (= i 2) (conj! u i) (let [k (inc i)] (recur k))))))])"#,
        r#"[[0 0 9] 1]"#,
    ),
    (
        r#"(let [t (transient [0 0 0])] (loop [i 0] (if (= i 2) (assoc! t i 9) (let [k (inc i)] (recur k)))) (persistent! t))"#,
        r#"[0 0 9]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 n 0] (if (= i 4) [n (persistent! t)] (do (conj! t i) (recur (inc i) (let [z (* i 2)] (+ z n)))))))"#,
        r#"[12 [0 1 2 3]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 b false] (if (= i 3) [b (persistent! t)] (do (conj! t i) (recur (inc i) (= i 1))))))"#,
        r#"[false [0 1 2]]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (loop [i 0 n 0] (if (= i 4) [n (persistent! t) (persistent! u)] (do (conj! t i) (conj! u n) (recur (inc i) (if (or (zero? i) (not (pos? n)) (and (< i 9) (> i 2))) (inc n) n))))))"#,
        r#"[2 [0 1 2 3] [0 1 1 1]]"#,
    ),
    (
        r#"(let [t (transient []) v [1 2 3 4]] (loop [i 0 n 0] (if (= i 4) [n (persistent! t)] (do (conj! t i) (recur (inc i) (+ n (loop [k 0] (if (< k 1) (recur (inc k)) (nth v i)))))))))"#,
        r#"[10 [0 1 2 3]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 n 0] (if (= i 3) [n (persistent! t)] (do (conj! t i) (recur (inc i) (let [b (= i 1)] (if b 10 n)))))))"#,
        r#"[10 [0 1 2]]"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (recur (inc i) (+ n i i i))))"#,
        r#"9"#,
    ),
    (
        r#"(loop [i 0 n 0] (if (= i 3) n (let [j (inc i)] (recur j (+ n (max i 1 2) (bit-or i 1 2) (- 10 i 1))))))"#,
        r#"39"#,
    ),
    (
        r#"(let [g (fn [t] (loop [i 0] (if (= i 3) (count t) (do (conj! t i) (recur (inc i))))))] [(g (transient [])) (with-redefs [conj! (fn [t x] t)] (g (transient [])))])"#,
        r#"[3 0]"#,
    ),
    (
        r#"(let [t (transient [0 0 0 0 0])] (loop [i 0 l -1] (if (= i 5) (persistent! t) (do (when (not= l -1) (assoc! t l i)) (recur (inc i) (if (zero? (rem i 2)) i -1))))))"#,
        r#"[1 0 3 0 0]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (loop [i 0] (if (= i 6) [(persistent! t) (persistent! u)] (do (when-not (zero? (rem i 3)) (conj! t i) (conj! u (- i))) (when (> i 1) (when (< i 4) (conj! u (* 10 i)))) (recur (inc i))))))"#,
        r#"[[1 2 4 5] [-1 -2 20 30 -4 -5]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 6) (persistent! t) (do (when (< (count t) 3) (conj! t i)) (recur (inc i))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (loop [i 0] (if (= i 5) [(persistent! t) (persistent! u)] (do (conj! t i) (when (< (count t) 3) (conj! u i)) (recur (inc i))))))"#,
        r#"[[0 1 2 3 4] [0 1]]"#,
    ),
    (
        r#"(let [v [10 20 30] t (transient [0 0 0])] (loop [i 0] (if (= i 5) (persistent! t) (do (when (< i 3) (assoc! t i (nth v i))) (recur (inc i))))))"#,
        r#"[10 20 30]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (if (= i 1) nil nil) (conj! t i) (recur (inc i))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (when (= i 2)) (conj! t i) (recur (inc i))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (when (= i 1) (str i)) (conj! t i) (recur (inc i))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [0 0 0])] (loop [i 0] (if (= i 3) [(persistent! t) (persistent! u)] (do (when (pos? i) (conj! t i) (assoc! u i (- i))) (recur (inc i))))))"#,
        r#"[[1 2] [0 -1 -2]]"#,
    ),
    (
        r#"(let [t (transient [0 0 0 0 0 0]) u (transient [])] (loop [i 0 a 0] (if (= i 6) [a (persistent! t) (persistent! u)] (do (conj! u i) (assoc! t i (* i i)) (let [j (inc i)] (if (= 1 (rem i 2)) (recur j (+ a 1)) (recur j (nth t i))))))))"#,
        r#"[17 [0 1 4 9 16 25] [0 1 2 3 4 5]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 a 0] (if (= i 5) [a (persistent! t)] (do (conj! t i) (if (< (count t) 3) (recur (inc i) (inc a)) (recur (inc i) a))))))"#,
        r#"[2 [0 1 2 3 4]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 a 0] (if (= i 4) [a (persistent! t)] (do (conj! t i) (let [n (count t)] (recur (inc i) (+ a n)))))))"#,
        r#"[10 [0 1 2 3]]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (loop [i 0] (if (= i 3) [(persistent! t) (persistent! u)] (do (conj! t i) (let [x (* 2 i)] (conj! u x) (do (conj! u (inc x)) (recur (inc i))))))))"#,
        r#"[[0 1 2] [0 1 2 3 4 5]]"#,
    ),
    (
        r#"(let [t (transient [])] [(loop [i 0] (do (conj! t i) (if (< i 3) (recur (inc i)) (count t)))) (loop [i 0] (do (conj! t i) (if (< i 3) (recur (inc i)) (* i 10)))) (count t)])"#,
        r#"[4 30 8]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (conj! t i) (let [[a b] [i i]] (recur (+ i (- a b) 1)))))))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(let [t (transient [0 1 2])] (loop [i 0 a 0 b 0] (if (= i 3) [a b (persistent! t)] (do (assoc! t i (* 10 i)) (recur (inc i) (nth t i) (+ b a))))))"#,
        r#"[20 10 [0 10 20]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 n 0] (if (= i 4) [n (persistent! t)] (do (conj! t i) (conj! t i) (recur (inc i) (count t))))))"#,
        r#"[8 [0 0 1 1 2 2 3 3]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 a 0] (if (= i 4) [a (persistent! t)] (do (conj! t (* i 3)) (recur (inc i) (nth t i))))))"#,
        r#"[9 [0 3 6 9]]"#,
    ),
    (
        r#"(let [t (transient [0])] (try (loop [i 0 a 0] (if (= i 4) a (do (conj! t i) (recur (inc i) (nth t 7))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.IndexOutOfBoundsException""#,
    ),
    (
        r#"(let [t (transient [:a 1])] [(loop [i 0 a 0] (if (= i 2) a (do (conj! t i) (recur (inc i) (nth t 1))))) (loop [i 0 a 0] (if (= i 2) a (do (conj! t i) (recur (inc i) (nth t 0)))))])"#,
        r#"[1 :a]"#,
    ),
    (
        r#"(let [t (transient [0 0]) u t] (loop [i 0 a 0] (if (= i 2) [a (persistent! t)] (do (assoc! u 0 (+ i 5)) (recur (inc i) (nth t 0))))))"#,
        r#"[6 [6 0]]"#,
    ),
    (
        r#"(let [t (transient [0 1]) v [1 0]] (loop [i 0 a 0] (if (= i 2) a (do (conj! t i) (recur (inc i) (nth t (nth v i)))))))"#,
        r#"0"#,
    ),
    (
        r#"(let [t (transient [0 1]) s "ab"] (loop [i 0 a 0 c 0] (if (= i 2) [a c] (do (conj! t i) (recur (inc i) (nth s i) (inc (nth t i)))))))"#,
        r#"[\b 2]"#,
    ),
    (
        r#"(let [t (transient [5 6]) p (persistent! (transient [7 8]))] (loop [i 0 a 0 b 0] (if (= i 2) [a b] (do (conj! t i) (recur (inc i) (nth p i) (count p))))))"#,
        r#"[8 2]"#,
    ),
    (
        r#"(let [u (transient []) t (transient [])] (loop [i 0] (if (= i 6) [(persistent! u) (persistent! t)] (do (when (< i 3) (when (pos? i) (conj! u i))) (when (< i 4) (when (> i 1) (conj! t i) (conj! t (- i)))) (recur (inc i))))))"#,
        r#"[[1 2] [2 -2 3 -3]]"#,
    ),
    (
        r#"(let [t (transient [1 2 3 4 5])] (loop [i 0] (if (= i 3) (persistent! t) (do (pop! t) (recur (inc i))))))"#,
        r#"[1 2]"#,
    ),
    (
        r#"(let [t (transient [1 2 3])] (count (persistent! (loop [i 0] (if (= i 2) (pop! t) (recur (inc i)))))))"#,
        r#"2"#,
    ),
    (
        r#"(let [t (transient [])] (try (loop [i 0] (if (= i 2) :done (do (pop! t) (recur (inc i))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.IllegalStateException""#,
    ),
    (
        r#"(let [t (transient [1 2])] (persistent! t) (try (loop [i 0] (if (= i 2) :done (do (pop! t) (recur (inc i))))) (catch Throwable e (.getName (class e)))))"#,
        r#""java.lang.IllegalAccessError""#,
    ),
    (
        r#"(let [h (transient []) v [3 1 4 1 5 9 2 6]] (loop [i 0] (if (= i (count v)) (persistent! h) (let [x (nth v i)] (loop [] (when (and (pos? (count h)) (> (nth h (dec (count h))) x)) (pop! h) (recur))) (conj! h x) (recur (inc i))))))"#,
        r#"[1 1 2 6]"#,
    ),
    (
        r#"(let [t (transient [])] (try (loop [i 0 a 1] (if (= i 70) :done (do (conj! t i) (recur (inc i) (* a 2))))) (catch ArithmeticException e [(count t) (.getName (class e))])))"#,
        r#"[63 "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [])] (try (loop [i 0 a 1] (if (= i 70) :done (do (conj! t a) (recur (inc i) (* (nth t i) 2))))) (catch ArithmeticException e [(count t) (nth t 62) (.getName (class e))])))"#,
        r#"[63 4611686018427387904 "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [0 0 0])] (try (loop [i 0 a 1] (if (= i 70) :done (do (assoc! t 0 a) (recur (inc i) (* (nth t 0) 2))))) (catch ArithmeticException e [(persistent! t) (.getName (class e))])))"#,
        r#"[[4611686018427387904 0 0] "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [])] (try (loop [i 0 a 1] (if (= i 70) :done (do (assoc! t i a) (recur (inc i) (* (nth t i) 2))))) (catch ArithmeticException e [(count t) (.getName (class e))])))"#,
        r#"[63 "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [5 6 7 8])] (try (loop [i 0 a 1] (if (= i 70) :done (do (pop! t) (conj! t a) (recur (inc i) (* (nth t 3) 2))))) (catch ArithmeticException e [(persistent! t) (.getName (class e))])))"#,
        r#"[[5 6 7 4611686018427387904] "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [])] (try (loop [i 0 a 1] (if (= i 70) :done (do (conj! t 1) (conj! t 2) (conj! t a) (recur (inc i) (* (nth t (dec (count t))) 2))))) (catch ArithmeticException e [(count t) (.getName (class e))])))"#,
        r#"[189 "java.lang.ArithmeticException"]"#,
    ),
    (
        r#"(let [t (transient [:a :b])] (try (loop [i 0] (if (= i 3) :done (do (assoc! t 0 i) (recur (+ i (nth t 1)))))) (catch Exception e [(persistent! t) (.getName (class e))])))"#,
        r#"[[0 :b] "java.lang.ClassCastException"]"#,
    ),
    (
        r#"(let [t (transient [1 2]) u t] (try (loop [i 0] (if (= i 2) :done (do (conj! t 7) (pop! u) (assoc! t 0 9) (recur (+ i (nth u 1) (nth t 5)))))) (catch Exception e [(persistent! t) (.getName (class e))])))"#,
        r#"[[9 2] "java.lang.IndexOutOfBoundsException"]"#,
    ),
    (
        r#"(let [t (transient [0])] (loop [i 0] (if (= i 2) (persistent! t) (do (loop [j 0] (when (< j 5000) (assoc! t 0 (+ j i)) (recur (inc j)))) (recur (inc i))))))"#,
        r#"[5000]"#,
    ),
    (
        r#"(let [t (transient []) u (transient [])] (loop [i 0] (if (= i 6) [(persistent! t) (persistent! u)] (do (if (zero? (rem i 2)) (conj! t i) (conj! u i)) (let [x (* i i)] (conj! t x)) (inc i) nil (recur (inc i))))))"#,
        r#"[[0 0 1 2 4 9 4 16 25] [1 3 5]]"#,
    ),
    (
        r#"(try (loop [i 0] (if (= i 3) :done (do (+ i 9223372036854775807) (recur (inc i))))) (catch ArithmeticException e (.getName (class e))))"#,
        r#""java.lang.ArithmeticException""#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (loop [j 0] (if (< j i) (do (conj! t j) (recur (inc j))) j)) (loop [j 0] (when (< j 1) (conj! t -1) (recur (inc j)))) (recur (inc i))))))"#,
        r#"[-1 0 -1 0 1 -1]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 a 0] (if (= i 4) [a (persistent! t)] (recur (inc i) (+ a (do (conj! t i) (* i 10)) (let [x (inc i)] (conj! t x) (* x 100)))))))"#,
        r#"[1060 [0 1 1 2 2 3 3 4]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0 n 0] (if (= i 3) [n (persistent! t)] (do (conj! t i) (recur (inc i) (count t))))))"#,
        r#"[3 [0 1 2]]"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (= i 2) (persistent! t) (do (let [[a] [i]] (conj! t a)) (loop [[b] [i]] (conj! t b)) (recur (inc i))))))"#,
        r#"[0 0 1 1]"#,
    ),
    (
        r#"(loop [i 0 a 0] (if (= i 2) a (recur (inc i) (+ a (if (do) 1 2)))))"#,
        r#"4"#,
    ),
    (
        r#"(let [t (transient [:a :b])] (try (loop [i 0] (if (= i 2) :done (do (pop! t) (recur (+ i (nth t 5)))))) (catch Exception e [(persistent! t) (.getName (class e))])))"#,
        r#"[[:a] "java.lang.IndexOutOfBoundsException"]"#,
    ),
    (
        r#"(let [t (transient [0 0]) s "ab"] (try (loop [i 0] (if (= i 2) :done (do (assoc! t i (nth s i)) (recur (+ i (nth t 5)))))) (catch Exception e [(persistent! t) (.getName (class e))])))"#,
        r#"[[\a 0] "java.lang.IndexOutOfBoundsException"]"#,
    ),
    (
        r#"(let [v [[1 2 3] [4 5 6]]] (loop [i 0 acc 0] (if (= i 2) acc (let [[a b c] (nth v i)] (recur (inc i) (+ acc a b c))))))"#,
        r#"21"#,
    ),
    (
        r#"(let [v [[1 2] [3]]] (loop [i 0 acc 0] (if (= i 2) acc (let [[a b] (nth v i)] (recur (inc i) (+ acc a (if (nil? b) 100 b)))))))"#,
        r#"106"#,
    ),
    (
        r#"(let [v [[1 2] 5]] (try (loop [i 0 acc 0] (if (= i 2) acc (let [[a b] (nth v i)] (recur (inc i) (+ acc a b))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(let [s "ab"] (try (loop [i 0 acc 0] (if (= i 2) acc (let [[a] (nth s i)] (recur (inc i) (+ acc a))))) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (
        r#"(let [t (transient [[1 2] [3 4]])] (loop [i 0 acc 0] (if (= i 2) acc (let [[a b] (nth t i)] (recur (inc i) (+ acc (* a b)))))))"#,
        r#"14"#,
    ),
    (
        r#"(let [v [[:x 1] [2 3]]] (loop [i 0 acc []] (if (= i 2) acc (let [[a b] (nth v i)] (recur (inc i) (conj acc a b))))))"#,
        r#"[:x 1 2 3]"#,
    ),
    (
        r#"(let [v [[1 2 3]]] (loop [i 0 acc 0] (if (= i 1) acc (let [[a & r :as all] (nth v i)] (recur (inc i) (+ acc a (count r) (count all)))))))"#,
        r#"6"#,
    ),
    (
        r#"(let [v [[[1] 2]]] (loop [i 0 acc 0] (if (= i 1) acc (let [[[a] b] (nth v i)] (recur (inc i) (+ acc a b))))))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0 acc 0] (if (= i 2) acc (let [[a b] [i (* 2 i)]] (recur (inc i) (+ acc a b)))))"#,
        r#"3"#,
    ),
    (
        r#"(let [v [[1 2]]] (loop [i 0 acc 0] (if (= i 2) acc (let [[a b] (nth v i [7 8])] (recur (inc i) (+ acc a b))))))"#,
        r#"18"#,
    ),
    (
        r#"(let [v [[1 2] [3 4]] t (transient [])] (loop [i 0] (if (= i 2) (persistent! t) (let [[a b] (nth v i)] (conj! t (+ a b)) (recur (inc i))))))"#,
        r#"[3 7]"#,
    ),
    (
        r#"(let [v [[1 2] [3 4]]] (loop [i 0 acc 0] (if (= i 2) acc (recur (inc i) (+ acc (let [[a b] (nth v i)] (* a b)))))))"#,
        r#"14"#,
    ),
    (
        r#"(vec (sort [[3 1 2] [1 2 3] [1 2 2] [3 0 0] [1 2 2]]))"#,
        r#"[[1 2 2] [1 2 2] [1 2 3] [3 0 0] [3 1 2]]"#,
    ),
    (
        r#"(loop [i 0 a 0] (if (< i 3) (recur (inc i) (+ a (let [{:keys [k]} {:k i}] k))) a))"#,
        r#"3"#,
    ),
    (
        r#"(loop [i 0] (if (< i 3) (recur (inc i)) (let [x i])))"#,
        r#"nil"#,
    ),
    (r#"(loop [i 0])"#, r#"nil"#),
    (
        r#"(loop [i 0] (if (< i 3) (recur (inc i)) (do)))"#,
        r#"nil"#,
    ),
];

#[test]
fn scalar_loops_match_the_jvm() {
    check(JVM_ROWS);
}

/// Loops calling top-level fns (the setup defines them first, so
/// the loop compiles while their vars hold them): inlined callees,
/// and every callee the tier leaves to a real call (captured
/// locals, the fn's own name, variadic or destructuring params,
/// multimethods, maps, natives, conveyed bindings, tree-evaluator
/// closures, recursion, nesting past the inline depth, calls past
/// the per-loop budget, a redefined or rebound var), and loops run
/// many times (a program that logs heavily leaves the tier).
/// `(setup, form, JVM value)`.
const JVM_CALL_ROWS: &[(&str, &str, &str)] = &[
    (
        r#"(defn i1-get [v i] (nth v i))"#,
        r#"(let [v [1 2 3 4]] (loop [i 0 a 0] (if (= i 4) a (recur (inc i) (+ a (i1-get v i))))))"#,
        r#"10"#,
    ),
    (
        r#"(defn i2-find [head edges u c] (loop [e (nth head u)] (if (= e -1) -1 (let [ev (nth edges e)] (if (= (bit-shift-right ev 8) c) e (recur (dec (bit-and ev 255))))))))"#,
        r#"(let [head (transient [1 -1]) edges (transient [(bit-shift-left 97 8) (bit-or (bit-shift-left 98 8) 1)])] (loop [i 0 a 0] (if (= i 4) a (recur (inc i) (+ (* a 10) (inc (i2-find head edges 0 (+ 96 i))))))))"#,
        r#"120"#,
    ),
    (
        r#"(defn i3-sq [x] (* x x)) (defn i3-f [x y] (+ (i3-sq x) y))"#,
        r#"(loop [i 0 a 0] (if (= i 5) a (let [j (* i 2)] (recur (inc i) (i3-f j a)))))"#,
        r#"120"#,
    ),
    (
        r#"(defn i4-r [x] (if (zero? x) 0 (+ 2 (i4-r (dec x)))))"#,
        r#"(loop [i 0 a 0] (if (= i 8) a (recur (inc i) (+ a (i4-r i)))))"#,
        r#"56"#,
    ),
    (
        r#"(let [k 5] (defn i5-cap [x] (+ x k)))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (i5-cap i)))))"#,
        r#"18"#,
    ),
    (
        r#"(def i6-cnt (fn self [x] (if (pos? x) (inc (self (dec x))) 0)))"#,
        r#"(loop [i 0 a 0] (if (= i 4) a (recur (inc i) (+ a (i6-cnt i)))))"#,
        r#"6"#,
    ),
    (
        r#"(defn i7-va [& xs] (count xs)) (defn i7-ds [[a b]] (+ a b)) (defmulti i7-mm (fn [x] (odd? x))) (defmethod i7-mm true [x] 1) (defmethod i7-mm false [x] 10) (def i7-m {0 1 1 2 2 3}) (def ^:dynamic i7-dn inc) (def i7-bf (bound-fn [x] (* x 3))) (binding [] (def i7-tf (fn [x] (- x)))) (defn i7-two [x y] (+ x y))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (i7-va i i) (i7-ds [i i]) (i7-mm i) (i7-m i) (i7-dn i) (i7-bf i) (i7-tf i) (i7-two i i)))))"#,
        r#"57"#,
    ),
    (
        r#"(defn i8-g [x] (inc x)) (defn i8-run [] (loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (i8-g i))))))"#,
        r#"[(i8-run) (do (defn i8-g [x] (dec x)) (i8-run)) (with-redefs [i8-g (fn [x] 100)] (i8-run)) (i8-run)]"#,
        r#"[6 0 300 0]"#,
    ),
    (
        r#"(defn ^:dynamic i9-g [x] (inc x)) (defn i9-run [] (loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (i9-g i))))))"#,
        r#"[(i9-run) (binding [i9-g (fn [x] 7)] (i9-run)) (i9-run)]"#,
        r#"[6 21 6]"#,
    ),
    (
        r#"(defn st-find-edge [head-v edges u c] (loop [e (nth head-v u)] (if (= e -1) -1 (let [ev (nth edges e)] (if (= (bit-shift-right ev 48) c) e (recur (dec (bit-and (bit-shift-right ev 24) 16777215)))))))) (defn st-value [s] (let [n (count s)] (if (zero? n) 0 (let [t (str s "$") nt (inc n) link-v (transient [0]) start-v (transient [0]) end-v (transient [-2]) head-v (transient [-1]) edges (transient [])] (loop [pos 0 active 0 edge 0 alen 0 rem 1 last-new -1] (when (< pos nt) (if (zero? rem) (recur (inc pos) active edge alen 1 -1) (let [pos-ch (int (nth t pos)) edge (if (zero? alen) pos edge) cur-char (int (nth t edge)) e (st-find-edge head-v edges active cur-char)] (if (= e -1) (let [leaf (count link-v) ne (count edges) h0 (nth head-v active)] (conj! link-v 0) (conj! start-v pos) (conj! end-v -1) (conj! head-v -1) (conj! edges (bit-or leaf (bit-shift-left (inc h0) 24) (bit-shift-left cur-char 48))) (assoc! head-v active ne) (when (not= last-new -1) (assoc! link-v last-new active)) (let [rem-next (dec rem)] (if (zero? active) (if (pos? alen) (recur pos 0 (inc (- pos rem-next)) (dec alen) rem-next -1) (recur pos 0 edge alen rem-next -1)) (recur pos (nth link-v active) edge alen rem-next -1)))) (let [ev (nth edges e) nxt (bit-and ev 16777215) en (nth end-v nxt) nxt-start (nth start-v nxt) elen (inc (- (if (= en -1) pos en) nxt-start))] (if (>= alen elen) (recur pos nxt (+ edge elen) (- alen elen) rem last-new) (let [epos (+ nxt-start alen) epos-ch (int (nth t epos))] (if (= epos-ch pos-ch) (do (when (and (not= last-new -1) (not (zero? active))) (assoc! link-v last-new active)) (recur (inc pos) active edge (inc alen) (inc rem) -1)) (let [split-end (+ nxt-start alen -1) split (count link-v) leaf (inc split) ne (count edges)] (conj! link-v 0) (conj! start-v nxt-start) (conj! end-v split-end) (conj! head-v (inc ne)) (conj! link-v 0) (conj! start-v pos) (conj! end-v -1) (conj! head-v -1) (assoc! edges e (bit-or (bit-and ev -16777216) split)) (conj! edges (bit-or leaf (bit-shift-left pos-ch 48))) (conj! edges (bit-or nxt (bit-shift-left (inc ne) 24) (bit-shift-left epos-ch 48))) (assoc! start-v nxt epos) (when (not= last-new -1) (assoc! link-v last-new split)) (let [rem-next (dec rem)] (if (zero? active) (if (pos? alen) (recur pos 0 (inc (- pos rem-next)) (dec alen) rem-next split) (recur pos 0 edge alen rem-next split)) (recur pos (nth link-v active) edge alen rem-next split))))))))))))) (let [nst (count link-v) order (transient [0]) cnt (transient (vec (repeat nst 0)))] (assoc! link-v 0 0) (loop [qi 0] (when (< qi (count order)) (let [v (nth order qi) dv (nth link-v v) leaves (loop [e (nth head-v v) lc 0] (if (= e -1) lc (let [ev (nth edges e) w (bit-and ev 16777215) en (nth end-v w) ne (dec (bit-and (bit-shift-right ev 24) 16777215))] (if (= en -1) (recur ne (inc lc)) (let [w-len (inc (- en (nth start-v w)))] (assoc! start-v w v) (assoc! link-v w (+ dv w-len)) (conj! order w) (recur ne lc))))))] (assoc! cnt v leaves) (recur (inc qi))))) (loop [qi (dec (count order)) ans n] (if (<= qi 0) ans (let [v (nth order qi) c (nth cnt v) p (nth start-v v) _ (assoc! cnt p (+ (nth cnt p) c)) prod (* (nth link-v v) c)] (recur (dec qi) (if (> prod ans) prod ans))))))))))"#,
        r#"(mapv st-value ["a" "ab" "aaa" "abab" "banana" "mississippi" "abcabxabcd" "zzzzzzzzzz" "abcdefghijklmnopqrstuvwxyzabcdefghij" "abracadabraabracadabra"])"#,
        r#"[1 2 4 4 6 11 10 30 36 22]"#,
    ),
    (
        r#"(defn i10-k [x] (loop [j x] (if (> j 2) (- j 2) (recur (+ j 3)))))"#,
        r#"(let [t (transient [])] (loop [i 0] (if (= i 4) (persistent! t) (do (conj! t (i10-k i)) (recur (inc i))))))"#,
        r#"[1 2 3 1]"#,
    ),
    (
        r#"(defn i11-nil [x] nil) (defn i11-two [x] (inc x) (dec x))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (if (i11-nil i) a (+ a (i11-two i))))))"#,
        r#"0"#,
    ),
    (
        r#"(defn i12-b [y x] (+ y x)) (let [k 3] (defn i12-a [x] (i12-b k x)))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (i12-a i)))))"#,
        r#"12"#,
    ),
    (
        r#"(defn c1 [x] (inc x)) (defn c2 [x] (c1 (inc x))) (defn c3 [x] (c2 (inc x))) (defn c4 [x] (c3 (inc x))) (defn c5 [x] (c4 (inc x)))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (c5 i)))))"#,
        r#"18"#,
    ),
    (
        r#"(defn b1 [x] (* x 2))"#,
        r#"(loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i) (b1 i)))))"#,
        r#"102"#,
    ),
    (
        r#"(defn s1-push! [h x] (conj! h x) (conj! h (- x)))"#,
        r#"(let [t (transient [])] (loop [i 0] (if (= i 3) (persistent! t) (do (s1-push! t i) (recur (inc i))))))"#,
        r#"[0 0 1 -1 2 -2]"#,
    ),
    (
        r#"(defn s2-add! [head nxt u x] (let [k (count nxt)] (conj! nxt (nth head u)) (assoc! head u k)))"#,
        r#"(let [head (transient [-1 -1]) nxt (transient [])] (loop [i 0] (if (= i 5) [(persistent! head) (persistent! nxt)] (do (s2-add! head nxt (rem i 2) i) (recur (inc i))))))"#,
        r#"[[4 3] [-1 -1 0 1 2]]"#,
    ),
    (
        r#"(defn s3-f [t x] (conj! t x) (* x 3)) (defn s3-none [x])"#,
        r#"(let [t (transient [])] (loop [i 0 a 0] (if (= i 4) [a (persistent! t)] (do (s3-none i) (recur (inc i) (+ a (s3-f t i)))))))"#,
        r#"[18 [0 1 2 3]]"#,
    ),
    (
        r#"(defn ch-pack [x y] (bit-xor (bit-or (bit-shift-left (+ x 2147483648) 32) (bit-and (+ y 2147483648) 4294967295)) -9223372036854775808)) (defn ch-chain [pts] (let [m (count pts) h (transient [])] (loop [i 0] (if (= i m) (do (when (pos? (count h)) (pop! h)) (persistent! h)) (let [p (nth pts i) px (- (unsigned-bit-shift-right (bit-xor p -9223372036854775808) 32) 2147483648) py (- (bit-and (bit-xor p -9223372036854775808) 4294967295) 2147483648)] (loop [] (when (>= (count h) 2) (let [hn (count h) b (nth h (dec hn)) a (nth h (- hn 2)) ax (- (unsigned-bit-shift-right (bit-xor a -9223372036854775808) 32) 2147483648) ay (- (bit-and (bit-xor a -9223372036854775808) 4294967295) 2147483648) bx (- (unsigned-bit-shift-right (bit-xor b -9223372036854775808) 32) 2147483648) by (- (bit-and (bit-xor b -9223372036854775808) 4294967295) 2147483648) cr (-' (*' (-' bx ax) (-' py ay)) (*' (-' by ay) (-' px ax)))] (when (<= cr 0) (pop! h) (recur))))) (conj! h p) (recur (inc i)))))))"#,
        r#"(let [pts (vec (sort (map (fn [[x y]] (ch-pack x y)) [[0 0] [1 1] [2 0] [2 2] [0 2] [1 0] [1 2] [3 1] [-1 1] [1 -1]])))] (mapv count [(ch-chain pts) (ch-chain (vec (rseq pts)))]))"#,
        r#"[2 3]"#,
    ),
    (
        r#"(defn hv-row [t k i] (loop [r 0] (when (< r 40) (loop [j 0] (when (< j k) (assoc! t j (+ i r j)) (recur (inc j)))) (recur (inc r)))))"#,
        r#"(let [t (transient (vec (repeat 40 0)))] (dotimes [i 30] (hv-row t 40 i)) (persistent! t))"#,
        r#"[68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95 96 97 98 99 100 101 102 103 104 105 106 107]"#,
    ),
    (
        r#"(defn lt-row [t i] (loop [r 0] (when (< r 3) (loop [j 0] (when (< j 2) (conj! t (+ i r j)) (recur (inc j)))) (recur (inc r)))))"#,
        r#"(let [t (transient [])] (dotimes [i 40] (lt-row t i)) (reduce + (persistent! t)))"#,
        r#"5040"#,
    ),
    (
        r#"(defn kr-root! [parent x] (let [root (loop [curr x] (let [p (nth parent curr)] (if (= p curr) curr (recur p))))] (loop [curr x] (if (= curr root) root (let [nxt (nth parent curr)] (assoc! parent curr root) (recur nxt))))))"#,
        r#"(let [edges [[4 1 2] [1 2 3] [3 1 3] [2 3 4] [5 1 4] [1 4 5]] sorted (vec (sort edges)) m (count sorted) parent (transient (vec (range 6))) sizes (transient (vec (repeat 6 1)))] (loop [i 0 total 0] (if (= i m) total (let [[w u v] (nth sorted i) ru (kr-root! parent u) rv (kr-root! parent v)] (if (= ru rv) (recur (inc i) total) (let [su (nth sizes ru) sv (nth sizes rv)] (if (>= su sv) (do (assoc! parent rv ru) (assoc! sizes ru (+ su sv)) (recur (inc i) (+ total w))) (do (assoc! parent ru rv) (assoc! sizes rv (+ su sv)) (recur (inc i) (+ total w))))))))))"#,
        r#"7"#,
    ),
    (
        r#"(defn lz-in [x] (if (and (> x 2) (< x 7)) 1 0))"#,
        r#"(loop [i 0 a 0] (if (< i 10) (recur (inc i) (+ a (lz-in i))) a))"#,
        r#"4"#,
    ),
];

#[test]
fn inlined_calls_match_the_jvm() {
    for (setup, form, want) in JVM_CALL_ROWS {
        let mut eval = eager();
        eval.eval_source(setup)
            .unwrap_or_else(|e| panic!("{setup}: {e}"));
        assert_eq!(show(&mut eval, form), *want, "VM: {form}");
        let mut eval = boot();
        eval.eval_source(setup)
            .unwrap_or_else(|e| panic!("{setup}: {e}"));
        let tree = format!("(binding [] {form})");
        assert_eq!(show(&mut eval, &tree), *want, "tree: {form}");
    }
}

/// An inlined call skips the call-depth check a real call makes, so
/// a loop with inlined calls runs on scalars only while every call
/// it stands for fits: one frame short of the limit, the boxed road
/// makes the call and overflows, exactly where a loop making the
/// real call does.
#[test]
fn inlined_calls_keep_the_call_depth_limit() {
    const SO: &str = "THROW java.lang.StackOverflowError: ";
    let mut eval = eager();
    eval.eval_source(
        "(defn d-g [x] (inc x))
         (let [k 1] (defn d-h [x] (+ x k)))
         (defn d-run [] (loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (d-g i))))))
         (defn d-real [] (loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (d-h i))))))
         (defn d-none [] (loop [i 0 a 0] (if (= i 3) a (recur (inc i) (+ a (inc i))))))
         (defn d-deep [f n] (if (zero? n) (f) (d-deep f (dec n))))",
    )
    .unwrap();
    // The deepest a loop making no call still runs at.
    let deepest = (49_980..50_000)
        .take_while(|n| show(&mut eval, &format!("(d-deep d-none {n})")) == "6")
        .last()
        .expect("a depth that runs");
    for run in ["d-run", "d-real"] {
        let at = |n: i32| format!("(d-deep {run} {n})");
        assert_eq!(show(&mut eval, &at(deepest - 1)), "6", "{run}");
        assert_eq!(show(&mut eval, &at(deepest)), SO, "{run}");
    }
}

/// Lazy compile (the default): a loop runs boxed until it has
/// `recur`ed 64 times (then hands the rest of the run to its program:
/// flat and fused bodies, a program that finishes, one that misses,
/// a body with no program) or has been entered 32 times (then its
/// boxed history decides: too short a trip compiles nothing); a
/// program whose first runs all miss early leaves the tier. Outer
/// locals reach the late compile through registers, captures and the
/// fn's own name.
const LAZY_ROWS: &[(&str, &str)] = &[
    (
        r#"(loop [i 0 a 0] (if (< i 200) (let [b (* i 2)] (recur (inc i) (+ a b))) a))"#,
        r#"39800"#,
    ),
    (
        r#"(let [v (vec (range 300))] (loop [i 0 a 0] (if (< i 300) (recur (inc i) (+ a (nth v i))) a)))"#,
        r#"44850"#,
    ),
    (
        r#"(let [v (vec (concat (range 100) [10N] (range 10)))] (loop [i 0 a 0] (if (< i (count v)) (let [x (nth v i)] (recur (inc i) (+ a x))) a)))"#,
        r#"5005N"#,
    ),
    (
        r#"(let [v (vec (concat (range 100) [10N] (range 10)))] (loop [i 0 a 0] (if (< i (count v)) (recur (inc i) (+ a (nth v i))) a)))"#,
        r#"5005N"#,
    ),
    (
        r#"(loop [i 0 m {}] (if (< i 100) (recur (inc i) (assoc m (rem i 7) i)) (count m)))"#,
        r#"7"#,
    ),
    (
        r#"(loop [i 0 m {}] (if (< i 100) (let [k (rem i 7)] (recur (inc i) (assoc m k i))) (count m)))"#,
        r#"7"#,
    ),
    (
        r#"(do (defn lz-one [x] (loop [j x] (if (> j 0) j (recur (inc j))))) (reduce + (map lz-one (range 1 41))))"#,
        r#"820"#,
    ),
    (
        r#"(do (defn lz-two [v x] (loop [j 0 a x] (if (< j 1) (recur (inc j) (+ a (nth v j))) a))) (reduce + (map #(lz-two [5] %) (range 40))))"#,
        r#"980"#,
    ),
    (
        r#"(do (defn lz-miss [v] (loop [i 0 a 0] (if (< i (count v)) (recur (inc i) (+ a (nth v i))) a))) (reduce + (repeatedly 100 #(lz-miss [1 2N 3]))))"#,
        r#"600N"#,
    ),
    (
        r#"(let [k 5] (defn lz-cap [] (loop [i 0 a 0] (if (< i 100) (let [b (+ a k)] (recur (inc i) b)) a))) (lz-cap))"#,
        r#"500"#,
    ),
    (
        r#"(do (def lz-self (fn self [n] (loop [i 0 a 0] (if (< i 100) (let [b (+ a i)] (recur (inc i) b)) (if (pos? n) (+ a (self (dec n))) a))))) (lz-self 2))"#,
        r#"14850"#,
    ),
    (
        r#"(let [t (transient [])] (loop [i 0] (if (< i 150) (do (conj! t (* i i)) (recur (inc i))) (reduce + (persistent! t)))))"#,
        r#"1113775"#,
    ),
];

#[test]
fn lazy_tier_matches_the_jvm() {
    for (form, want) in LAZY_ROWS {
        let mut eval = boot();
        assert_eq!(show(&mut eval, form), *want, "lazy VM: {form}");
    }
}
