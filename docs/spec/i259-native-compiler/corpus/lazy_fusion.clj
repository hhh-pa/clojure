;; Fused sole lazy combinators (map, mapcat, filter, remove, take, concat,
;; repeat, range, subvec) and double fast-path in Prepared::call2 (#271).
(defn dot-dbl [a b] (reduce + 0.0 (map * a b)))
(defn dot-int [a b] (reduce + 0 (map * a b)))

;; Side-effect chunking check: chunked vector/range tests full 32-item chunk
;; before early stop, matching forced lazy seqs.
(defn chunk-trace [kind]
  (let [log (atom [])]
    (let [res (case kind
                :map-vec (some #(when (= % 2) %) (map (fn [x] (swap! log conj x) x) (vec (range 40))))
                :map-rng (some #(when (= % 2) %) (map (fn [x] (swap! log conj x) x) (range 40)))
                :flt-vec (some #(when (= % 2) %) (filter (fn [x] (swap! log conj x) (even? x)) (vec (range 40))))
                :flt-rng (some #(when (= % 2) %) (filter (fn [x] (swap! log conj x) (even? x)) (range 40)))
                :rem-list (some #(when (= % 3) %) (remove (fn [x] (swap! log conj x) (even? x)) (list 0 1 2 3 4 5)))
                :take-flt (vec (take 2 (filter (fn [x] (swap! log conj x) (even? x)) (vec (range 40)))))
                :take-map2 (vec (take 2 (map (fn [a b] (swap! log conj [a b]) (+ a b)) [1 2 3 4] [10 20 30 40]))))]
      [res (count @log)])))

(println (dot-dbl [1.5 2.5 3.0] [2.0 4.0 0.5]))
(println (dot-int [2 3 4] [5 6 7]))
(println (reduce + 0.0 [1 2 3 4]) (reduce - 10 [1.5 2.5]) (mapv inc [1.5 2.5]) (mapv dec [3.5 4.5]))
(println (mapv + [1 2 3] (map #(* 10 %) [4 5 6 7])))
(println (mapv + [] (map #(throw (ex-info "boom" {})) [1 2])))
(println (mapv (fn [a b] (- a (* 0.5 b))) [1.0 2.0 3.0] [2.0 4.0 6.0]))
(println (vec (map (fn [a b c] (+ a b c)) [1 2] [10 20] [100 200])))
(println (vec (map {:a 1 :b 2} [:a :c] [9 8])))
(println (vec (concat [1 2] (repeat 3 0.0) (subvec [10 20 30 40] 1 3) (range 3))))
(println (vec (concat (concat [1] [2]) (concat [3] [4]) [5])))
(println (vec (take 5 (concat [1 2] (repeat 9)))))
(println (vec (take 0 (map #(throw (ex-info "boom" {})) [1 2]))))
(println (count (filter #(= % 2) [1 2 3 2 4 2])) (count (remove even? (range 10))))
(println (count (filter odd? (list 1 2 3 4 5))) (count (take 4 (concat [1 2] (repeat 9)))))
(println (some #{3} (concat (take 3 [1 2 3 4]) [5 6])) (some #{9} (concat (take 2 [1 2 3]) [9])))
(println (vec (filter even? [])) (vec (filter even? (range 0))) (vec (map inc [])) (vec (map inc (range 0))))
(println (vec (map + [] [1 2])) (vec (map :k [{:k 1} {:k 2}])))
(println (mapv chunk-trace [:map-vec :map-rng :flt-vec :flt-rng :rem-list :take-flt :take-map2]))
(println (try (vec (take "bad" [1 2])) (catch Exception e :bad-take)))
(println (try (vec (concat [1] 42)) (catch Exception e :bad-concat)))
