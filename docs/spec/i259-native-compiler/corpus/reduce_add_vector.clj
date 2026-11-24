;; `(reduce + …)` over vectors (#271): longs and doubles fold inline;
;; everything else keeps the general step semantics.
(defn sum [v] (reduce + v))
(defn sum0 [init v] (reduce + init v))
(println (sum [1 2 3]) (sum0 0.0 [1 2]) (sum [1 2.5]) (sum [0.1 0.2 0.3]) (sum0 1 [0.5 0.25]))
(println (sum []) (sum0 5 []) (sum [7]) (sum [2.5]) (sum [:a]) (sum0 :x []))
(println (sum (vec (range 2000))) (sum (vec (map double (range 1500)))) (sum (into [1] [2.0 3])))
(println (sum [1 2N]) (sum0 1/2 [1]) (sum [1/2 1/3]) (reduce +' [Long/MAX_VALUE 1]))
(println (try (sum [Long/MAX_VALUE 1]) (catch ArithmeticException e (.getMessage e))))
(println (try (sum [1 :a]) (catch Exception e :cce)) (try (sum0 nil [1]) (catch Exception e :npe)))
(println (reduce - [10 1 2]) (reduce + (subvec [1 2 3 4] 1)) (reduce + '(1 2 3)) (reduce + #{1 2}))
(let [plus +] (println (reduce plus [1 2]) (reduce (fn [a b] (+ a b)) [1.5 2])))
(println (sum [##NaN 1.0]) (sum [-0.0]) (sum0 -0.0 [-0.0]) (sum [1e308 1e308]))
