(let [seen (atom [])
      f (fn [v] (swap! seen conj v) (inc v))]
  (println (first (map f [1 2 3])) @seen)
  (reset! seen [])
  (println (first (map f '(1 2 3))) @seen))

(let [seen (atom [])
      pred (fn [v] (swap! seen conj v) (odd? v))]
  (println (first (filter pred [2 3 4 5])) @seen)
  (reset! seen [])
  (println (first (remove pred [3 4 5])) @seen))

(let [seen (atom [])
      f (fn [v] (swap! seen conj v) (if (> v 2) [v (- v)] []))]
  (println (first (mapcat f [1 2 3 4])) @seen)
  (reset! seen [])
  (println (first (mapcat f '(1 2 3 4))) @seen))

(let [f (fn [v] (if (even? v) (throw (ex-info "failed" {:at :first})) v))]
  (println (try (first (map f [1 2 3]))
                (catch Exception e (ex-data e)))))

(println (first (map inc [])) (first (filter number? [:x])))
(println (try (first [1] [2])
              (catch clojure.lang.ArityException _ :arity)))
