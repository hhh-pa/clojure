;; Natives calling compiled closures (#271): one- and two-arg prepared calls
;; run over the caller's argument array, owned calls (update, reduce-kv,
;; transducers) through the owned entry; results, exceptions and arity
;; errors match run either way.
(defn sq [x] (* x x))
(def add (fn [a b] (+ a b)))
(def k 3)
(defn plus-k [x] (+ x k))
(println (map sq [1 2 3]) (filter odd? (map plus-k [1 2 3])) (mapv (fn [x] (str "v" x)) [1 2]))
(println (reduce add 0 [1 2 3]) (reduce (fn [acc x] (conj acc (* 2 x))) [] [1 2]) (sort-by (fn [p] (- (first p))) [[1 :a] [3 :b] [2 :c]]))
(println (update {:k 1} :k sq) (update-in {:a {:b 2}} [:a :b] add 10) (reduce-kv (fn [acc k v] (+ acc v)) 0 {:a 1 :b 2}))
(println (transduce (map sq) + 0 [1 2 3]) (into [] (comp (filter odd?) (map plus-k)) [1 2 3 4]))
(println (try (doall (map (fn [x] (if (= x 2) (throw (ex-info "map-boom" {:x x})) x)) [1 2 3])) (catch Exception e (ex-message e))))
(println (try (reduce (fn [a x] (if (> x 1) (throw (ex-info "reduce-boom" {})) (+ a x))) 0 [1 2]) (catch Exception e (ex-message e))))
(println (try (update {:k 1} :k (fn [x] (throw (ex-info "update-boom" {:x x})))) (catch Exception e (ex-message e))))
(println (try (reduce-kv (fn [acc k v] (if (= k :b) (throw (ex-info "kv-boom" {})) (+ acc v))) 0 {:a 1 :b 2}) (catch Exception e (ex-message e))))
(println (try (doall (map add [1 2])) (catch Exception e :arity)) (try (reduce sq 0 [1 2]) (catch Exception e :arity)))
