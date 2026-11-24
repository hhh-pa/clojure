;; In-place owned vector `assoc`/`conj` (#271): only a uniquely held
;; vector updates in place; every alias keeps its own value.
(defn fill [n]
  (loop [i 0 v [0 0 0]]
    (if (< i n) (recur (inc i) (assoc v (rem i 3) i)) v)))
(defn grow [n]
  (loop [i 0 v [0]]
    (if (< i n) (recur (inc i) (conj v i)) v)))
(defn grow-d [n]
  (loop [i 0 v [0.5]]
    (if (< i n) (recur (inc i) (conj v (* 0.5 i))) v)))
(defn grow-g [n]
  (loop [i 0 v [:a]]
    (if (< i n) (recur (inc i) (conj v (if (even? i) :e i))) v)))
(println (fill 10) (count (grow 3000)) (reduce + (grow 3000)) (peek (grow-d 50)) (take 5 (grow-g 7)))

;; Aliases: the old vector stays observable, so it must not change.
(defn keep-history [n]
  (loop [i 0 v [0 0] hist []]
    (if (< i n) (recur (inc i) (assoc v 0 i) (conj hist v)) [v hist])))
(println (keep-history 4))
(let [a [1 2 3] b (assoc a 0 9) c (conj a 4)] (println a b c (identical? a b)))
(def shared-v [1 2])
(defn bump [] (loop [i 0 v shared-v] (if (< i 3) (recur (inc i) (conj v i)) v)))
(println (bump) shared-v (bump))
(defn alias-loop []
  (loop [i 0 v [1 2] w nil]
    (if (< i 3) (recur (inc i) (assoc v 1 i) (if (= i 1) v w)) [v w])))
(println (alias-loop))

;; Retyping and edge shapes fall back to the general path.
(defn mix []
  (loop [i 0 v [1 2]]
    (case i
      0 (recur 1 (assoc v 0 2.5))
      1 (recur 2 (conj v :k))
      2 (recur 3 (assoc v 3 :end))
      v)))
(println (mix) (assoc [] 0 :x) (conj [] 1) (assoc [:a nil] 1 :b))
(println (try (assoc [1] 5 2) (catch Exception e :oob)) (try (assoc [1] -1 2) (catch Exception e :oob)))
(println (loop [i 0 v [[1] [2]]] (if (< i 2) (recur (inc i) (assoc v i i)) v)))
(println (loop [i 0 v (vec (range 1024))] (if (< i 3) (recur (inc i) (conj v i)) (count v))))
(println (meta (loop [i 0 v (with-meta [1 2] {:m 1})] (if (< i 2) (recur (inc i) (assoc v 0 i)) v))))
