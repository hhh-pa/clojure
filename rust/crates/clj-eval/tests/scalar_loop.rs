// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #254 unboxed loop tier (`vm_scalar`): every expression, test,
//! effect and exit shape the tier lowers, and every miss it hands
//! back to the boxed tiers (overflow, non-scalar state or
//! invariants, frozen/map/persistent effect targets, out-of-range
//! and non-integer indexes, a redefined site, effect ordering,
//! unsupported forms). Every row runs twice — compiled by the VM
//! and on the tree evaluator (inside `(binding [] …)`) — and both
//! must give the JVM's value (Clojure 1.12).

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

fn check(rows: &[(&str, &str)]) {
    for (form, want) in rows {
        let mut eval = boot();
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
];

#[test]
fn scalar_loops_match_the_jvm() {
    check(JVM_ROWS);
}
