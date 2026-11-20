(ns user
  (:require [clojure.string :as str]))

;; 1. SplitVecValue: filterv seq (no leading empty, leading empty, interior empty),
;;    walk_direct (reduce/mapv) and walk_all_other (concat/take).
(defn test-splitvec []
  (let [s1 (str/split "10 20 30" #"\s+")
        s2 (str/split "  10 20 30" #"\s+")
        s3 (str/split "10,,20,,30" #",")
        f1 (filterv seq s1)
        f2 (filterv seq s2)
        f3 (filterv seq s3)]
    [(count f1) f1 f2 f3
     (filterv #(> (count %) 1) s3)
     (reduce (fn [acc x] (+ acc (count x))) 0 f2)
     (vec (concat f1 (take 2 f3)))
     (into [] (take 2 s2))]))

;; 2. Direct string-collection numeric parsing: SplitVec, Vector, SubVector,
;;    PackedStrings (>= 256 items), plus malformed token and i32 overflow fallbacks.
(defn parse-longs [ss] (mapv #(Long/parseLong %) ss))
(defn parse-ints [ss] (mapv #(Integer/parseInt %) ss))
(defn parse-doubles [ss] (mapv #(Double/parseDouble %) ss))

(defn test-parse-num []
  (let [sp (filterv seq (str/split "  -5 12 99" #"\s+"))
        gv ["1" "2" "3"]
        sv (subvec ["x" "7" "8" "y"] 1 3)
        pv (loop [i 0 acc (transient [])]
             (if (< i 260)
               (recur (inc i) (conj! acc (str i)))
               (persistent! acc)))
        bad (try (parse-longs ["1" "bad"]) (catch Exception _ :caught-long))
        ov (try (parse-ints ["3000000000"]) (catch Exception _ :caught-int))
        bad-d (try (parse-doubles ["1.5" "nah"]) (catch Exception _ :caught-dbl))]
    [(parse-longs sp)
     (parse-ints sp)
     (parse-doubles (str/split "1.5 -2.25 0.5" #"\s+"))
     (parse-longs gv)
     (parse-ints sv)
     (count (parse-longs pv))
     (nth (parse-longs pv) 259)
     (parse-longs (list "4" "5"))
     bad ov bad-d]))

;; 3. Dot product `(reduce + init (map * c1 c2))`: flat ints, mixed/double,
;;    unequal lengths, non-vector list fallback, and i64 overflow fallback.
(defn dot-int [a b] (reduce + 0 (map * a b)))
(defn dot-dbl [a b] (reduce + 0.0 (map * a b)))

(defn test-dot []
  (let [a [1 2 3]
        b [4 5 6 7]
        da [1.5 2.0 3.5]
        db [2.0 4.0 2.0]
        ov (try (dot-int [9223372036854775807] [2])
                (catch ArithmeticException _ :ov-int))
        ov-d (try (dot-dbl [9223372036854775807] [2])
                  (catch ArithmeticException _ :ov-dbl))]
    [(dot-int a b)
     (dot-dbl da db)
     (dot-dbl da a)
     (dot-dbl a da)
     (dot-dbl a b)
     (dot-int (list 2 3) (list 4 5))
     (dot-dbl [] ["ignored"])
     ov ov-d]))

;; 4. 2-collection HofLoop, every?, and fused reduce-+ map (with & without init,
;;    direct vector/range and general list paths, 0/1/N items, BigInt fallback).
(defn test-hof2 []
  (let [v1 [1 2 3 4]
        v2 [10 20 30]
        l1 (list 1 2 3)
        l2 (list 10 20 30)
        side (atom 0)]
    (dorun (map (fn [x y] (swap! side + (* x y))) v1 v2))
    [(mapv (fn [x y] (+ x y)) v1 v2)
     (mapv (fn [x y] (+ x y)) (range 3) v2)
     (mapv (fn [x y] (+ x y)) l1 l2)
     (vec (map (fn [x y] (* x y)) v1 v2))
     (into [] (map (fn [x y] (- y x)) v1 v2))
     @side
     (every? (fn [x] (> x 0)) v1)
     (every? (fn [x] (< x 3)) v1)
     (every? (fn [x] (> x 0)) [])
     (every? (fn [x] (> x 0)) l1)
     (every? (fn [x] (< x 2)) l1)
     (reduce + 0.0 (map (fn [x] (* x 1.5)) v1))
     (reduce + 0.0 (map (fn [x y] (* x y 0.5)) v1 v2))
     (reduce + 0 (map (fn [x y] (* x y)) l1 l2))
     (reduce + (map (fn [x] (* x 2)) v1))
     (reduce + (map (fn [x] x) []))
     (reduce + (map (fn [x] :solo) [1]))
     (reduce + (map (fn [x] (* x 2)) l1))
     (reduce + 0N (map (fn [x] x) v1))]))

(println (test-splitvec))
(println (test-parse-num))
(println (test-dot))
(println (test-hof2))
