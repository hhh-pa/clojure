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
