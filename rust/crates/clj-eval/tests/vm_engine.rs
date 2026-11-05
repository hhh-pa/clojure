// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #226 register VM: every row runs twice — compiled by the VM (a
//! top-level form) and on the tree evaluator (inside `(binding [] …)`,
//! which the VM hands to it whole) — and both must give the expected
//! value: the JVM's (Clojure 1.12) for [`JVM_ROWS`], the port's own
//! pre-existing semantics for [`PORT_ROWS`] (direct calls see a
//! redefined core fn; the JVM compiler inlines those fns instead).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

/// `pr-str` of the form's value, or `Class: message` when it throws.
fn show(eval: &mut Evaluator, form: &str) -> String {
    let src = format!(
        "(try (pr-str {form}) (catch Throwable t (str (.getName (class t)) \": \" (.getMessage t))))"
    );
    let vs = eval
        .eval_source(&src)
        .unwrap_or_else(|e| panic!("{form}: {e}"));
    pr_to_string(vs.last().unwrap(), &PrintOpts::print())
}

const SETUP: &str = r#"(def ^:dynamic myplus +)
(defn g-plus [a b] (myplus a b))
(defn t-lt [a b] (if (< a b) :lt :ge))
(defn t-count [x] (count x))
(defn t-get [m k] (get m k))
(defn t-nth3 [v i nf] (nth v i nf))
(defn t-max4 [a b c d] (max a b c d))
(defn t-zero [x] (if (zero? x) :z :nz))
(defn t-eq [a b] (if (= a b) :same :diff))
(defn t-nth-test [v i] (if (nth v i nil) :hit :miss))
(defn t-max-test [a b c] (if (max a b c) :yes :no))
(defn fact [n acc] (if (zero? n) acc (recur (dec n) (* acc n))))
(defn sum-destr [[a & r] acc] (if a (recur r (+ acc a)) acc))
(defn vari ([] :none) ([a] [:one a]) ([a & more] [:many a more]))
(defn opt-arg [{:keys [x y] :or {x 10 y 20}}] [x y])
(defn t-inc [x] (inc x))
"#;

const JVM_ROWS: &[(&str, &str)] = &[
    (
        r#"(mapv (fn [x] (case x 1 :one 2 :two :other)) [1 2 3 1.0 nil :k])"#,
        r#"[:one :two :other :other :other :other]"#,
    ),
    (
        r#"(mapv (fn [x] (case x "Aa" 1 "BB" 2 :k 3 [1 2] 4 :none)) ["Aa" "BB" :k [1 2] "x" nil])"#,
        r#"[1 2 3 4 :none :none]"#,
    ),
    (r#"(case (map inc [0 1]) (1 2) :yes :no)"#, r#":no"#),
    (r#"(case 5 (1 2 3) :small (4 5 6) :mid :big)"#, r#":mid"#),
    (
        r#"(let [a (atom 0)] [(try (/ 1 0) (catch ArithmeticException e :caught) (finally (swap! a inc))) @a])"#,
        r#"[:caught 1]"#,
    ),
    (
        r#"(let [a (atom 0)] [(try 7 (finally (swap! a inc))) @a])"#,
        r#"[7 1]"#,
    ),
    (
        r#"(let [a (atom 0)] (try (try (/ 1 0) (catch ArithmeticException e (throw (ex-info "h" {}))) (finally (swap! a inc))) (catch Exception e [(ex-message e) @a])))"#,
        r#"["h" 1]"#,
    ),
    (
        r#"(try (try (/ 1 0) (finally (throw (ex-info "fin" {})))) (catch Exception e (ex-message e)))"#,
        r#""fin""#,
    ),
    (
        r#"(try (try (/ 1 0) (catch IllegalStateException e :no)) (catch ArithmeticException e :outer))"#,
        r#":outer"#,
    ),
    (
        r#"(try (try (throw (ex-info "x" {})) (catch Exception e (throw (ex-info "y" {}))) (finally (throw (ex-info "z" {})))) (catch Exception e (ex-message e)))"#,
        r#""z""#,
    ),
    (
        r#"(try (throw (Exception. "boom")) (catch Exception e (.getMessage e)))"#,
        r#""boom""#,
    ),
    (
        r#"(try (throw nil) (catch NullPointerException e :npe))"#,
        r#":npe"#,
    ),
    (
        r#"(try (throw 5) (catch ClassCastException e :cce))"#,
        r#":cce"#,
    ),
    (
        r#"(let [x Long/MAX_VALUE] (try (inc x) (catch ArithmeticException e (ex-message e))))"#,
        r#""long overflow""#,
    ),
    (
        r#"(let [x Long/MIN_VALUE] (try (dec x) (catch ArithmeticException e (ex-message e))))"#,
        r#""long overflow""#,
    ),
    (
        r#"(let [x 1.5 y 2] [(inc x) (dec x) (+ x y) (- x y) (* x y) (< x y) (> x y) (<= x y) (>= x y)])"#,
        r#"[2.5 0.5 3.5 -0.5 3.0 true false true false]"#,
    ),
    (
        r#"(let [x 3000000000 y 4000000000] (try (* x y) (catch ArithmeticException e (ex-message e))))"#,
        r#""long overflow""#,
    ),
    (
        r#"(let [x 1N] [(inc x) (dec x) (+ x 1) (< x 2)])"#,
        r#"[2N 0N 2N true]"#,
    ),
    (r#"(let [x 1.5] (if (< x 2) :y :n))"#, r#":y"#),
    (
        r#"(let [x 3 y 3] [(if (< x y) 1 2) (if (<= x y) 1 2) (if (> x y) 1 2) (if (>= x y) 1 2)])"#,
        r#"[2 1 2 1]"#,
    ),
    (r#"(let [a (atom 1)] (if (< @a 2.5) :y :n))"#, r#":y"#),
    (r#"(binding [myplus -] (g-plus 5 2))"#, r#"3"#),
    (r#"(g-plus 5 2)"#, r#"7"#),
    (
        r#"[(t-nth-test [1 2] 1) (t-nth-test [1 2] 5) (t-max-test 1 2 3) (t-max4 4 9 2 7) (t-max4 1 2.5 3 0)]"#,
        r#"[:hit :miss :yes 9 3]"#,
    ),
    (
        r#"(let [v [10 20 30] i 1 j 5 k -1] [(nth v i) (nth v j :nf) (nth v k :nf) (try (nth v j) (catch IndexOutOfBoundsException e :oob))])"#,
        r#"[20 :nf :nf :oob]"#,
    ),
    (
        r#"(let [t (transient [1 2 3]) i 1 j 9] [(nth t i) (nth t j :nf) (nth t -1 :nf)])"#,
        r#"[2 :nf :nf]"#,
    ),
    (
        r#"(let [s "abc" i 1] [(nth s i) (nth s 9 :nf)])"#,
        r#"[\b :nf]"#,
    ),
    (
        r#"(let [v [1 2] i 5000000000] (try (nth v i) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.ArithmeticException""#,
    ),
    (
        r#"(let [v [1 2] i 5000000000] (try (nth v i :nf) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.ArithmeticException""#,
    ),
    (
        r#"(let [m (transient {:a 1})] (try (nth m 0) (catch Exception e (.getName (class e)))))"#,
        r#""java.lang.UnsupportedOperationException""#,
    ),
    (r#"(let [l (list 1 2 3) i 2] (nth l i))"#, r#"3"#),
    (r#"(let [v [1 2] i 1.0] (nth v i))"#, r#"2"#),
    (
        r#"(let [m {:a 1 "s" 2 \c 3 1 4 nil 5 true 6 (quote sym) 7} k :a] [(get m k) (get m "s") (get m \c) (get m 1) (get m nil) (get m true) (get m (quote sym)) (get m :zz) (get m :zz :d)])"#,
        r#"[1 2 3 4 5 6 7 nil :d]"#,
    ),
    (r#"(let [m {[1] :v} k [1]] (get m k))"#, r#":v"#),
    (
        r#"(let [m (sorted-map 1 :a 2 :b) k 2] [(get m k) (get m 3 :d)])"#,
        r#"[:b :d]"#,
    ),
    (
        r#"(let [v [5 6] i 1 j 7] [(get v i) (get v j) (get v j :d) (get v :x) (get v -1 :neg)])"#,
        r#"[6 nil :d nil :neg]"#,
    ),
    (
        r#"(let [s #{1 2} k 2] [(get s k) (get s 9)])"#,
        r#"[2 nil]"#,
    ),
    (
        r#"(let [c nil k :a] [(get c k) (get c k :d)])"#,
        r#"[nil :d]"#,
    ),
    (
        r#"(let [m {:a 1} f (fn [] (keys m))] (get m (do (f) :a)))"#,
        r#"1"#,
    ),
    (
        r#"(let [a nil b [1 2] c {:a 1} d #{1 2 3} e "hello" f (list 1 2) g (range 3)] [(count a) (count b) (count c) (count d) (count e) (count f) (count g)])"#,
        r#"[0 2 1 3 5 2 3]"#,
    ),
    (
        r#"(let [m {:a 1} s #{:x} ss (sorted-set 1 2) v [1 2] k :a] [(contains? m k) (contains? m :b) (contains? s :x) (contains? s :y) (contains? ss 2) (contains? v 1) (contains? v 5)])"#,
        r#"[true false true false true true false]"#,
    ),
    (
        r#"(let [a 1 b 1 c 2 d \x e "s" f :k g true] [(= a b) (= a c) (= d \x) (= e "s") (= f :k) (= g true) (= nil nil) (= a 1.0) (= [1] [1]) (= a nil) (not= a c) (not= a b) (not= [1] [1])])"#,
        r#"[true false true true true true true false true false true false false]"#,
    ),
    (
        r#"(let [a 3 b 7 c -2] [(min a b) (max a b) (min a b c) (max a b c) (min a 2.5) (max a 7.5) (max 1 2 3 4 5)])"#,
        r#"[3 7 -2 7 2.5 7.5 5]"#,
    ),
    (
        r#"(let [a 17 b 5 c -17 z 0] [(quot a b) (rem a b) (mod a b) (quot c b) (rem c b) (mod c b) (mod a -5) (mod c -5) (mod 10 5) (quot Long/MIN_VALUE -1) (rem Long/MIN_VALUE -1) (mod Long/MIN_VALUE -1)])"#,
        r#"[3 2 2 -3 -2 3 -3 -2 0 -9223372036854775808 0 0]"#,
    ),
    (
        r#"(let [a 17 z 0] [(try (quot a z) (catch ArithmeticException e (ex-message e))) (try (rem a z) (catch ArithmeticException e (ex-message e))) (try (mod a z) (catch ArithmeticException e (ex-message e)))])"#,
        r#"["/ by zero" "/ by zero" "Divide by zero"]"#,
    ),
    (
        r#"(let [a 17.5 b 5] [(quot a b) (rem a b) (mod a b)])"#,
        r#"[3.0 2.5 2.5]"#,
    ),
    (
        r#"[(apply rem [7 2]) (apply mod [-7 2]) (apply quot [7 2])]"#,
        r#"[1 1 3]"#,
    ),
    (
        r#"(let [a 0 b 5 c -3 d 2.0] [(zero? a) (zero? b) (pos? b) (pos? c) (neg? c) (neg? b) (even? b) (odd? b) (even? a) (zero? d) (pos? d)])"#,
        r#"[true false true false true false false true true false true]"#,
    ),
    (
        r#"(let [s "x"] (try (zero? s) (catch ClassCastException e :cce)))"#,
        r#":cce"#,
    ),
    (
        r#"(let [d 2.5] (try (even? d) (catch IllegalArgumentException e (ex-message e))))"#,
        r#""Argument must be an integer: 2.5""#,
    ),
    (
        r#"(let [a nil b false c 0 d []] [(not a) (not b) (not c) (nil? a) (nil? c) (some? a) (some? d)])"#,
        r#"[true true false true false false true]"#,
    ),
    (
        r#"(let [t (transient [1 2]) i 0 j 2] (persistent! (assoc! (assoc! t i :a) j :b)))"#,
        r#"[:a 2 :b]"#,
    ),
    (
        r#"(let [t (transient [1 2]) j 5] (try (assoc! t j :x) (catch IndexOutOfBoundsException e :oob)))"#,
        r#":oob"#,
    ),
    (
        r#"(let [t (transient {:a 1})] (persistent! (assoc! t :b 2)))"#,
        r#"{:a 1, :b 2}"#,
    ),
    (
        r#"(let [t (transient [1 2])] (try (assoc! t :k 2) (catch IllegalArgumentException e (ex-message e))))"#,
        r#""Key must be integer""#,
    ),
    (
        r#"(let [t (transient [1])] (persistent! t) (try (assoc! t 0 2) (catch Throwable e (ex-message e))))"#,
        r#""Transient used after persistent! call""#,
    ),
    (
        r#"[(apply nth [(transient [1]) -1 :nf]) (apply count [[1 2]])]"#,
        r#"[:nf 2]"#,
    ),
    (
        r#"(loop [i 0 v []] (if (< i 5) (recur (inc i) (conj v i)) v))"#,
        r#"[0 1 2 3 4]"#,
    ),
    (r#"(let [v [1 2]] [(conj v 3) v])"#, r#"[[1 2 3] [1 2]]"#),
    (
        r#"(let [v [1 2] w (conj v 3)] [v w])"#,
        r#"[[1 2] [1 2 3]]"#,
    ),
    (
        r#"(let [m {:a 1}] (let [m2 (assoc m :b 2)] [m m2]))"#,
        r#"[{:a 1} {:a 1, :b 2}]"#,
    ),
    (
        r#"(reduce (fn [acc x] (conj acc x)) [] (range 5))"#,
        r#"[0 1 2 3 4]"#,
    ),
    (
        r#"(reduce (fn [acc x] (assoc acc x (* x x))) {} (range 4))"#,
        r#"{0 0, 1 1, 2 4, 3 9}"#,
    ),
    (
        r#"(let [v (vec (range 40))] (loop [i 0 v v] (if (< i 40) (recur (inc i) (assoc v i (- i))) (reduce + v))))"#,
        r#"-780"#,
    ),
    (
        r#"(let [x [1] y (if (seq x) (conj x 2) x)] [x y])"#,
        r#"[[1] [1 2]]"#,
    ),
    (
        r#"(mapv #(%) (loop [i 0 acc []] (if (< i 3) (recur (inc i) (conj acc (fn [] i))) acc)))"#,
        r#"[0 1 2]"#,
    ),
    (
        r#"(letfn [(ev? [n] (if (zero? n) true (od? (dec n)))) (od? [n] (if (zero? n) false (ev? (dec n))))] [(ev? 10) (od? 7) (ev? 3)])"#,
        r#"[true true false]"#,
    ),
    (
        r#"((fn f [n] (if (< n 2) n (+ (f (- n 1)) (f (- n 2))))) 10)"#,
        r#"55"#,
    ),
    (
        r#"(let [f (fn self [] self)] (identical? (f) (f)))"#,
        r#"true"#,
    ),
    (
        r#"(let [base 5 add (fn [x] (+ x base))] (map add [1 2 3]))"#,
        r#"(6 7 8)"#,
    ),
    (
        r#"(let [x 1] (let [f (fn [] (let [g (fn [] x)] (g)))] (f)))"#,
        r#"1"#,
    ),
    (
        r#"(take 5 ((fn step [n] (lazy-seq (cons n (step (inc n))))) 0))"#,
        r#"(0 1 2 3 4)"#,
    ),
    (
        r#"(lazy-seq (loop [i 0] (if (< i 3) (recur (inc i)) (list i))))"#,
        r#"(3)"#,
    ),
    (
        r#"(let [d (delay (+ 1 2))] [(realized? d) @d (realized? d)])"#,
        r#"[false 3 true]"#,
    ),
    (
        r#"(let [n 3] (doall (for [i (range n) j (range i) :when (odd? (+ i j))] [i j])))"#,
        r#"([1 0] [2 1])"#,
    ),
    (
        r#"[(fact 10 1) (sum-destr [1 2 3 4] 0) (vari) (vari 1) (vari 1 2 3) (opt-arg {}) (opt-arg {:x 1})]"#,
        r#"[3628800 10 :none [:one 1] [:many 1 (2 3)] [10 20] [1 20]]"#,
    ),
    (
        r#"(let [{:keys [a b] :or {a 1 b 2}} {:b 5} [x & ys :as all] [1 2 3]] [a b x ys all])"#,
        r#"[1 5 1 (2 3) [1 2 3]]"#,
    ),
    (
        r#"(loop [[a b] [1 2] n 0] (if (< n 3) (recur [b (+ a b)] (inc n)) [a b]))"#,
        r#"[5 8]"#,
    ),
    (
        r#"(loop [{:keys [x] :or {x 0}} {} n 0] (if (< n 2) (recur {:x (+ x 10)} (inc n)) x))"#,
        r#"20"#,
    ),
    (
        r#"((fn [a & {:keys [k] :or {k :dflt}}] [a k]) 1)"#,
        r#"[1 :dflt]"#,
    ),
    (
        r#"(try (vari 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21) (catch Exception e :ok))"#,
        r#"[:many 1 (2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21)]"#,
    ),
    (
        r#"(try ((fn [a b] a) 1) (catch clojure.lang.ArityException e :arity))"#,
        r#":arity"#,
    ),
    (
        r#"(try (assert (= 1 1)) :passed (catch AssertionError e :failed))"#,
        r#":passed"#,
    ),
    (
        r#"(try (assert false) (catch AssertionError e (ex-message e)))"#,
        r#""Assert failed: false""#,
    ),
    (
        r#"(try (assert false "custom msg") (catch AssertionError e (ex-message e)))"#,
        r#""Assert failed: custom msg\nfalse""#,
    ),
    (
        r#"(try (assert false (map #(/ 1 %) [0])) (catch ArithmeticException e :str-threw))"#,
        r#":str-threw"#,
    ),
    (
        r#"(let [a 1 b 2] [[a b] {a b :k (+ a b)} #{a b} {:nested [a {b a}]}])"#,
        r#"[[1 2] {1 2, :k 3} #{1 2} {:nested [1 {2 1}]}]"#,
    ),
    (
        r#"(do (def ^{:tag String :k (inc 1)} dm-x "v") [(:k (meta (var dm-x))) (.getName (:tag (meta (var dm-x))))])"#,
        r#"[2 "java.lang.String"]"#,
    ),
    (
        r#"(let [m {(list 1 2) :lazy}] (get m (map identity [1 2])))"#,
        r#":lazy"#,
    ),
    (
        r#"(try ((fn [x] {:pre [(pos? x)]} x) -1) (catch AssertionError e (ex-message e)))"#,
        r#""Assert failed: (pos? x)""#,
    ),
    (
        r#"((fn [x] {:pre [(pos? x)] :post [(> % 1)]} (inc x)) 3)"#,
        r#"4"#,
    ),
    (
        r#"(let [a 3 b 9 c 1 d 4] [(max a b (inc c) d) (min (dec a) b c d 0)])"#,
        r#"[9 0]"#,
    ),
    (
        r#"(mapv (fn [x] (case x "Aa" 1 :k 2 :none)) ["BB" "Aa" :k])"#,
        r#"[:none 1 2]"#,
    ),
    (
        r#"(try (loop [i 0] (if (< i 2) (recur (inc i)) (throw (ex-info "L" {})))) (catch Exception e (ex-message e)))"#,
        r#""L""#,
    ),
    (
        r#"((fn [n acc] (if (zero? n) acc (recur (dec n) (* acc n)))) 5 1)"#,
        r#"120"#,
    ),
    (
        r#"(mapv #(%) ((fn [n acc] (if (zero? n) acc (recur (dec n) (conj acc (fn [] n))))) 3 []))"#,
        r#"[3 2 1]"#,
    ),
    (
        r#"(let [a (atom 0)] (doall (lazy-seq (if (< (swap! a inc) 3) (recur) (list @a)))))"#,
        r#"(3)"#,
    ),
    (
        r#"(let [t (transient {})] (persistent! (assoc! t 0 :x)))"#,
        r#"{0 :x}"#,
    ),
    (
        r#"(binding [myplus -] ((fn [a b] (myplus a b)) 5 2))"#,
        r#"3"#,
    ),
    (
        r#"(loop [[a b] [1 2]] (if (< a 10) (recur [b (+ a b)]) [a b]))"#,
        r#"[13 21]"#,
    ),
    (
        r#"(try (doall (lazy-seq (throw (ex-info "lz" {})))) (catch Exception e (ex-message e)))"#,
        r#""lz""#,
    ),
];

const PORT_ROWS: &[(&str, &str)] = &[
    (
        r#"(try (case (map identity [(range)]) :x :a :b) (catch UnsupportedOperationException e :unsupported))"#,
        r#":unsupported"#,
    ),
    (
        r#"(try (inc 1 2) (catch clojure.lang.ArityException e :arity))"#,
        r#":arity"#,
    ),
    (r#"(with-redefs [inc dec] (t-inc 5))"#, r#"4"#),
    (
        r#"(try (case (range) :x :a :b) (catch UnsupportedOperationException e :unsupported))"#,
        r#":unsupported"#,
    ),
    (
        r#"(try (case (list (range)) :x :a :b) (catch UnsupportedOperationException e :unsupported))"#,
        r#":unsupported"#,
    ),
    (
        r#"(with-redefs [< >] ((fn [a b] (if (< a b) :lt :ge)) 1 2))"#,
        r#":ge"#,
    ),
    (r#"(with-redefs [< >] (t-lt 1 2))"#, r#":ge"#),
    (
        r#"(with-redefs [count (fn [_] 42)] (t-count [1 2]))"#,
        r#"42"#,
    ),
    (
        r#"(with-redefs [get (fn [_ _] :redef)] (t-get {:a 1} :a))"#,
        r#":redef"#,
    ),
    (
        r#"(with-redefs [nth (fn [_ _ _] :redef)] (t-nth3 [1] 0 :nf))"#,
        r#":redef"#,
    ),
    (
        r#"(with-redefs [max (fn [& _] :redef)] (t-max4 1 2 3 4))"#,
        r#":redef"#,
    ),
    (r#"(with-redefs [zero? (fn [_] true)] (t-zero 5))"#, r#":z"#),
    (
        r#"(with-redefs [= (fn [_ _] true)] (t-eq 1 2))"#,
        r#":same"#,
    ),
    (
        r#"(with-redefs [nth (fn [& _] nil)] (t-nth-test [1] 0))"#,
        r#":miss"#,
    ),
    (
        r#"(with-redefs [max (fn [& _] nil)] (t-max-test 1 2 3))"#,
        r#":no"#,
    ),
];

fn booted() -> Evaluator {
    let mut eval = boot();
    eval.eval_source(SETUP)
        .unwrap_or_else(|e| panic!("setup: {e}"));
    eval
}

fn check_both(rows: &[(&str, &str)]) {
    let mut eval = booted();
    for (form, want) in rows {
        assert_eq!(&show(&mut eval, form), want, "vm: {form}");
        let tree = format!("(binding [] {form})");
        assert_eq!(&show(&mut eval, &tree), want, "tree: {form}");
    }
}

#[test]
fn vm_and_tree_match_the_jvm() {
    check_both(JVM_ROWS);
}

#[test]
fn vm_and_tree_share_port_semantics() {
    check_both(PORT_ROWS);
}

/// A deterministic conflict retry (the `stm_retry_under_contention`
/// shape) whose stranded deref sits inside a VM-compiled `try`.
const RETRY: &str = "(let [r (ref 0 :min-history 1 :max-history 0) c (atom 0) fin (atom 0) p1 (promise) p2 (promise)] (future (deref p1) (dosync (alter r inc)) (dosync (alter r inc)) (deliver p2 true)) [(try (dosync (guarded c p1 p2 r fin)) (catch Exception e (ex-message e))) @c @fin @r])";

#[test]
fn stm_retry_unwinds_through_a_vm_try() {
    // The retry's unwind runs `finally` and resumes; the fresh
    // attempt runs the body (and `finally`) again.
    let mut eval = booted();
    eval.eval_source(
        "(defn guarded [c p1 p2 r fin] (try (swap! c inc) (deliver p1 true) (deref p2) (deref r) (finally (swap! fin inc))))",
    )
    .unwrap();
    assert_eq!(show(&mut eval, RETRY), "[2 2 2 2]");
}

#[test]
fn a_throwing_finally_masks_the_retry_unwind() {
    // `finally` throwing during the retry's unwind replaces it (like
    // the JVM's `RetryEx`): the transaction fails with that throw.
    let mut eval = booted();
    eval.eval_source(
        "(defn guarded [c p1 p2 r fin] (try (swap! c inc) (deliver p1 true) (deref p2) (deref r) (finally (when (= 1 (swap! fin inc)) (throw (ex-info \"masked\" {}))))))",
    )
    .unwrap();
    assert_eq!(show(&mut eval, RETRY), "[\"masked\" 1 1 2]");
}
