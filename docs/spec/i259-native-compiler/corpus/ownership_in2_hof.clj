;; #271 Slice 5g: per-arg ownership transfer, 2-key assoc-in/update-in & 3-arg
;; update fast paths, atom swap! in-place fast paths,let-slot release on recur,
;; and HofLoop filterv / (vec|into []|count (filter ...)) / some fusion.

(defn step-state [st i]
  (let [m1 (assoc-in st [:grid i] (* i 2))
        m2 (update-in m1 [:grid i] inc)
        m3 (update m2 :ticks inc)
        m4 (update m3 :budget dec)]
    m4))

(defn step-tail [st n]
  (if (zero? n)
    st
    (let [tmp [n (inc n)]
          st2 (step-state st (first tmp))]
      (recur st2 (dec n)))))

;; General fallback path for assoc-in2 / update-in2 (repack_own_args into 3-arg native)
(defn fallback-in2 [st]
  [(assoc-in st [:missing :k] 99)
   (update-in st [:a :b] + 10)
   (update-in {:a {:b nil}} [:a :b] (fnil inc 0))])

;; HofLoop FilterCollect, CountFilter, Some on both direct (vector/range) and
;; general (list/lazy-seq) collections, plus 1-arg owned count slow path.
(defn run-hofs [coll]
  [(filterv (fn [x] (pos? (mod x 2))) coll)
   (vec (filter (fn [x] (< x 4)) coll))
   (into [] (filter (fn [x] (>= x 2)) coll))
   (count (filter (fn [x] (= (mod x 2) 1)) coll))
   (count (filterv (fn [x] (= (mod x 2) 0)) coll))
   (some (fn [x] (when (= x 3) [:hit x])) coll)
   (some (fn [x] (when (= x 99) :nope)) coll)
   (count (filter odd? coll))])

(defn run-atoms []
  (let [a (atom {:a {:b 1} :x 0})
        c (atom 10)]
    (swap! a assoc :x 5)
    (swap! a assoc-in [:a :b] 7)
    (swap! c + 3)
    [@a @c]))

(let [final-st (step-tail {:grid [0 0 0 0] :ticks 0 :budget 10} 3)]
  (println (:grid final-st) (:ticks final-st) (:budget final-st))
  (println (fallback-in2 {:a {:b 5}}))
  (println (run-hofs [0 1 2 3 4 5]))
  (println (run-hofs (range 6)))
  (println (run-hofs '(0 1 2 3 4 5)))
  (println (run-atoms)))

(defn atom-lookups [row matrix i j]
  [(nth @row i) (nth (nth @matrix i) j)])

(println (atom-lookups (atom [7 8]) (atom [[10 11] [12 13]]) 1 0))
(println (atom-lookups (atom (subvec [7 8 9] 1))
                       (atom [(subvec [10 11 12] 1)]) 0 1))
(println (atom-lookups (atom '(8)) (atom '([12 13])) 0 1))
(println (atom-lookups (volatile! '(8)) (atom ['(12 13)]) 0 1))
(println (atom-lookups (atom [8 9]) (atom [[12 13]]) 0.0 1.0))
(println (try (nth @nil 0) (catch Exception _ :deref-failed))
         (try (nth (nth @nil 0) 0) (catch Exception _ :deref-failed))
         (try (nth (nth @(atom []) 0) 0) (catch Exception _ :out-of-bounds)))

(defn scaled-long [a b d]
  (long (quot (* a b) d)))

(println (scaled-long 7 -5 3)
         (scaled-long 2305843009213693952N 4 2)
         (scaled-long 2 5 18446744073709551615N)
         (scaled-long -9223372036854775808 1 -1)
         (scaled-long 5.5 4 2)
         (scaled-long 6 5 2.0)
         (long (quot 20 3)))
(println (try (scaled-long 9223372036854775807 2 2)
              (catch ArithmeticException _ :overflow))
         (try (scaled-long 2 5 0) (catch ArithmeticException _ :zero-divisor)))
(let [read-divisor? (atom false)]
  (try (long (quot (* 9223372036854775807 2)
                   (do (reset! read-divisor? true) 2)))
       (catch ArithmeticException _ nil))
  (println @read-divisor?))

(let [seen (atom [])
      input (map (fn [x] (swap! seen conj x) x) [1 2 3])]
  (println (apply str (interpose ", " (map str input))) @seen))
(println (apply str (interpose ", " (map str [])))
         (apply str (interpose " · " (map str [nil \é -2]))))
(println (apply str (interpose " · " (map str [-9223372036854775808 0 9223372036854775807]))))
(println (try (apply str (interpose ", " (map str 22)))
              (catch Exception _ :not-seqable)))
(println (apply str (interpose ", " [1 2]))
         (try (long) (catch Exception _ :arity)))
(let [s (str "fresh-" 7)]
  (println (identical? s (apply str (interpose ", " (map str (list s)))))
           (apply str (interpose ", " (map str [s "other"])))))
(let [seen (atom [])
      input (map (fn [x]
                   (swap! seen conj [:map x])
                   (reify Object
                     (toString [_] (swap! seen conj [:str x]) (str x))))
                 [1 2 3])]
  (println (apply str (interpose ", " (map str input))) @seen))

(let [cs (vec "éé.éa")
      tail (subvec cs 1)]
  (prn (count (filter #(= % \é) cs))
       (vec (filter #(= \é %) tail))
       (vec (remove #(= % \.) tail))
       (vec (drop-while #(= % \é) cs))
       (vec (drop-while #(= % \é) '(\é \a)))))
