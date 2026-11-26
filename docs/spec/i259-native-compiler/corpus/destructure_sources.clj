;; A map pattern without `:as` reads its keys from the source itself unless
;; the source is a list, which converts to its map first (#271 Slice 6j).
(defn f1 [m] (let [{:keys [a b]} m] [a b]))
(defn f2 [m] (let [{a :a b :b :or {b 9}} m] [a b]))
(defn f3 [m] (let [{:strs [x] :syms [y] :keys [z]} m] [x y z]))
(defn f4 [m] (let [{:keys [a] :as all} m] [a (count all)]))
(defn f5 [p] (let [{:keys [a] {:keys [c]} :b} p] [a c]))
(println (f1 {:a 1 :b 2}) (f1 '(:a 1 :b 2)) (f1 nil) (f1 [1 2]) (f1 (sorted-map :b 5)) (f1 "str"))
(println (f2 {:a 1}) (f2 {:a 1 :b nil}) (f2 '(:a 3)) (f2 {}))
(println (f3 {"x" 1 'y 2 :z 3}) (f3 (list "x" 4 'y 5)) (f4 {:a 1 :b 2}) (f4 '(:a 7)))
(println (f5 {:a 1 :b {:c 2}}) (f5 (list :a 1 :b (list :c 3))) (map f1 [{:a 1} nil '(:b 2)]))
(println (try (f1 '(:a)) (catch Exception e (.getMessage e))))
(defn loop-sum [ms] (loop [ms ms acc 0] (if-let [[m & more] (seq ms)] (let [{:keys [a b]} m] (recur more (+ acc a b))) acc)))
(println (loop-sum [{:a 1 :b 2} {:a 3 :b 4} (list :a 5 :b 6)]))
