;; Owned `disj`/`dissoc` (#271): loop accumulators moved into the call
;; update in place; every other shape keeps the general semantics.
(defn churn-set [s n]
  (loop [i 0 s s]
    (if (< i n)
      (recur (inc i) (conj (disj s (rem i 40)) (+ 100 i)))
      s)))
(println (count (churn-set (set (range 40)) 200)) (sort (churn-set #{1 2 3} 5)))

(defn churn-map [m n]
  (loop [i 0 m m]
    (if (< i n)
      (recur (inc i) (assoc (dissoc m (rem i 40)) (+ 100 i) i))
      m)))
(println (count (churn-map (zipmap (range 40) (range 40)) 200))
         (let [m (churn-map {:a 1 0 2} 3)] [(count m) (get m :a) (sort (keys (dissoc m :a)))]))

;; Fallbacks: nil, sorted collections, non-scalar keys, absent keys,
;; records, wrong types, and the 1-arg and many-arg arities.
(defrecord P [x y])
(defn shapes [coll k]
  (loop [i 0 out []]
    (if (< i 1)
      (recur (inc i) (conj out (try (disj coll k) (catch Exception e :cce))
                           (try (dissoc coll k) (catch Exception e :cce))))
      out)))
(println (shapes nil 1))
(println (shapes (sorted-set 3 1 2) 2) (shapes (sorted-map 1 :a 2 :b) 2))
(println (shapes #{[1 2] [3 4]} [1 2]) (shapes {[1 2] :a [3 4] :b} [1 2]))
(println (shapes #{1 2} 9) (shapes {1 2} 9) (shapes [1 2] 0))
(println (dissoc (->P 1 2) :x) (dissoc (->P 1 2) :z) (record? (dissoc (->P 1 2) :z)))
(println (disj #{1 2 3}) (dissoc {:a 1}) (disj #{1 2 3} 1 2) (dissoc {:a 1 :b 2 :c 3} :a :b))
(println (disj (sorted-set 1 2 3) 1 3) (dissoc (sorted-map 1 2 3 4) 1))
(println (try (disj) (catch Exception e :arity)) (try (dissoc) (catch Exception e :arity)))
(println (apply disj #{1 2 3} [1 2]) (apply dissoc {:a 1 :b 2} [:a]))
(let [s #{1 2 3} m {:a 1 :b 2}]
  ;; Shared (not moved) operands stay intact.
  (println (disj s 1) s (dissoc m :a) m))

;; Moved accumulators of every shape: the owned fast path declines
;; (restoring the operand) and the general native answers.
(defn thread-disj [coll ks]
  (loop [ks ks c coll]
    (if (seq ks) (recur (rest ks) (disj c (first ks))) c)))
(defn thread-dissoc [coll ks]
  (loop [ks ks c coll]
    (if (seq ks) (recur (rest ks) (dissoc c (first ks))) c)))
(println (thread-disj nil [1]) (thread-disj (sorted-set 1 2 3) [2 3])
         (thread-disj #{[1] [2] 3} [[1] 3]) (thread-disj #{1 2} [1 1]))
(println (thread-dissoc nil [1]) (thread-dissoc (sorted-map 1 2 3 4) [3])
         (thread-dissoc {[1] 2 3 4} [[1]]) (thread-dissoc {1 2 3 4} [1 9]))
(println (try (thread-disj [1 2] [1]) (catch Exception e :cce))
         (try (thread-dissoc #{1} [1]) (catch Exception e :cce)))
