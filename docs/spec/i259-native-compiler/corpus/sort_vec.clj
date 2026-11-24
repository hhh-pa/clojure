(println (vec (sort [9 2 4 2 -1])))
(println (vec (sort (subvec [8 3 7 2 5] 1 4))))
(println (vec (sort ["z" "a" "é" "a"])))
(println (vec (sort nil)))
(println (try (vec (sort 1 2 3)) (catch Exception e (class e))))
(println (try (vec [] []) (catch Exception e (class e))))

(let [seen (atom [])
      cmp (fn [a b] (swap! seen conj [a b]) (compare b a))]
  (println (vec (sort cmp '(3 2 4 1))) @seen))
(println
  (try
    (vec (sort (fn [_ _] (throw (ex-info "comparison failed" {:ok false}))) [1 2]))
    (catch Exception e (ex-data e))))

(let [a (with-meta [1 [2]] {:id :a})
      b (with-meta [1 [2]] {:id :b})
      pairs (vec (sort [[1 [0 0]] b [1 [1]] [0 [4 5]] a]))]
  (println pairs (mapv #(-> % meta :id) pairs)))
(println (vec (sort [[1 [3]] [1 [2.0]]])))
