;; Core calls over borrowed locals (no rebinding forms, so core calls take
;; the borrowed-argument path): plain locals, captured locals, letfn cells,
;; computed args, and the native fallback for uncovered cases.
(defn sum-captured [xs]
  (let [f (fn [i] (+ (nth xs i) (count xs)))]
    (reduce + (map f (range (count xs))))))
(println (sum-captured [1 2 3]))
(defn cell-arg []
  (letfn [(g [x] (inc x))]
    [(fn? g) (some? g) (g 1)]))
(println (cell-arg))
(defn transient-loop [n]
  (let [t (transient [])]
    (loop [i 0]
      (when (< i n)
        (conj! t (* i i))
        (recur (inc i))))
    (let [v (persistent! t)]
      [(count v) (nth v 2) (get v 9 :nf) (contains? v 1) (= (nth v 1) 1) (not= (peek v) 0)])))
(println (transient-loop 5))
(defn chars-and-strings [s]
  [(nth s 1) (= (nth s 0) \a) (int (nth s 2)) (count s) (get {:a s} :a) (first s) (seq "") (empty? s)])
(println (chars-and-strings "abc"))
(defn fallbacks [m]
  [(get m :k) (nth (list 1 2 3) 1) (count (map inc [1 2])) (+ 1.5 2) (= [1] [1]) (max 1 2 3)])
(println (fallbacks {:k 1}))
(println (try (nth [1] 5) (catch IndexOutOfBoundsException _ :oob)) (try (assoc! (transient #{}) 1 1) (catch ClassCastException e (.getMessage e))))
