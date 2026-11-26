(let [seen (atom [])
      f (fn [x]
          (swap! seen conj [:outer x])
          (map (fn [y] (swap! seen conj [:inner y]) y) [x (inc x)]))]
  (println (into [] (mapcat f) (do (swap! seen conj :source) [4 7 9])) @seen))
(println (into [] (mapcat (fn [x] (when (odd? x) (list :odd x)))) '(1 2 7)))
(println (into [] (mapcat identity) [[1 2] nil [3 :mixed]]))
(println (into [] (mapcat 1) []))
(println (try (into [] (mapcat 1) [2])
              (catch Exception e (class e))))
(let [seen (atom [])
      f (fn [x] (swap! seen conj x) (if (odd? x) [x] (throw (ex-info "bad" {:x x}))))]
  (println (try (into [] (mapcat f) [1 2 3]) (catch Exception e (ex-data e))) @seen))
(println (into [:kept] (mapcat identity) [[8]]))
(println (into [] (map inc) [1 2]))
