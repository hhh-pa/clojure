;; Owned calls hand compiled fns their argument array (#271): a param the
;; body consumes is taken, so reduce's accumulator reaches assoc/conj
;; uniquely owned and updates in place — while values the caller still
;; holds (shared inputs, a param read twice, borrowed direct calls) must
;; never change underneath it.
(defn step [acc x] (assoc acc (mod x 5) (conj (nth acc (mod x 5)) x)))
(println (reduce step (vec (repeat 5 [])) (range 30)))
(defn upd [m k] (update m k (fnil inc 0)))
(println (reduce upd {} [:a :b :a :c :a]))
(def shared (vec (range 5)))
(defn keep-orig [acc x] (assoc acc 0 x))
(println (reduce keep-orig shared [9]) shared)
(println (reduce (fn [acc x] (conj acc x)) [] (range 5)) (reduce-kv (fn [m k v] (assoc m v k)) {} {:a 1 :b 2}))
(let [v [1 2 3] f (fn [acc x] (assoc acc 0 x))] (println (reduce f v [7 8]) v))
(println (map (fn [x] (assoc x 0 :z)) [[1] [2]]) (mapv (fn [m] (dissoc m :a)) [{:a 1 :b 2}]))
(defn twice [v] [(assoc v 0 :x) v])
(println (twice [1 2]) (reduce (fn [acc [k v]] (assoc acc k v)) {} [[:x 1] [:y 2]]))
(println (step [[] [] [] [] []] 7) (keep-orig shared 1) shared)
(println (try (reduce (fn [acc] acc) [] [1]) (catch clojure.lang.ArityException e (str "arity: " (.getName (class e))))))
(println (try (reduce (fn [acc x] (quot acc x)) 10 [2 0]) (catch ArithmeticException e (.getMessage e))))
