;; Variadic `conj` runs as the chain it is defined as: every arg first, in
;; order, then one owned two-arg `conj` per item (#271 Slice 6k).
(defn noisy [x] (println "arg" x) x)
(println (conj [] 1 2 3) (conj [0] (noisy 1) (noisy 2)) (conj '(3) 2 1) (conj nil 1 2) (conj #{} 1 2 1)
         (conj {} [:a 1] [:b 2] {:c 3}) (conj (sorted-set 3) 1 2) (conj (with-meta [1] {:m 1}) 2 3))
(println (meta (conj (with-meta [1] {:m 1}) 2 3)) (conj [] nil nil) (conj (subvec [1 2 3] 1) 4 5) (conj (vec (range 40)) 40 41))
(defn grow [n] (loop [i 0 out []] (if (< i n) (recur (inc i) (if (odd? i) (conj out i (- i)) (conj out i))) out)))
(println (grow 7) (count (grow 1000)))
(defn shared [v] (let [w (conj v 9 8)] [v w]))
(println (shared [1 2]) (shared (vec (range 33))))
(println (try (conj {} [:a 1] [:b]) (catch Exception e (.getMessage e)))
         (try (conj "s" 1 2) (catch Exception e (class e)))
         (try (conj 5 1 2) (catch Exception e (class e))))
