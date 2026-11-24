(let [x [[1 2] (subvec [8 9 10] 1)] i 1 j 0]
  (println (nth (nth x i) j)
           (nth (peek x) j)
           (nth (nth x 0) 1)))
(println (nth (peek (with-meta [[1 2] [3 4]] {:tag true})) 1))
(println (nth (nth ["aé"] 0) 1))
(println (nth (peek '([7 8] [9 10])) 1))
(println (nth (peek []) 0))
(println (try (nth (nth [] -1) 0) (catch Exception e (class e))))
(println (try (nth (nth [[1]] 0) 2147483648) (catch Exception e (class e))))
(let [seen (atom [])
      source (map (fn [x] (swap! seen conj x) [x]) '(3 5))]
  (println (nth (nth source 0) 0) @seen))
(let [seen (atom [])]
  (println (try
             (nth (nth [] -1) (do (swap! seen conj :column) 0))
             (catch Exception e (class e)))
           @seen))
(defn present [x] (if (seq x) :present :empty))
(println (mapv present [nil [] [1] {} {:a 1} #{} #{1} "" "é"
                        '() '(1) (into-array []) (into-array [1])
                        (vector-of :long) (vector-of :long 1) (first {:a 1})]))
(let [seen (atom [])
      s (lazy-seq (swap! seen conj :forced) (list 9))]
  (println (present s) (present s) @seen))
(println (try (present (transient [])) (catch Exception e (class e))))

(println (try (nth (peek 1) 0) (catch Exception e (class e))))
