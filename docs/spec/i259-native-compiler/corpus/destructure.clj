;; Destructuring in let, fn params, loop, and nested patterns.
(defn pair-sum [[a b & more]] (+ a b (count more)))
(defn opts [{:keys [x y] :or {x 10 y 20} :as m}] [x y (count m)])
(defn strs [{:strs [a] :syms [b]}] [a b])
(defn nested [[a [b c]] {{:keys [d]} :inner}] [a b c d])
(println (pair-sum [1 2 3 4 5]))
(println (pair-sum '(7 8)))
(println (opts {:x 1}) (opts {}) (opts {:y 5 :z 6}))
(println (strs {"a" 1 'b 2}))
(println (nested [1 [2 3]] {:inner {:d 4}}))
(let [[x & xs :as all] (range 5)
      {:keys [k] :or {k :none}} {:j 1}]
  (println x xs all k))
(let [[_ _ third] "abcd"] (println third))
(let [[a b] nil] (println a b))
(loop [[h & t] [1 2 3] acc 0]
  (if h (recur t (+ acc h)) (println acc)))
(println ((fn [& {:keys [a b]}] (list a b)) :a 1 :b 2))
(println ((fn [[a b :as v] & rest] [a b v rest]) [1 2] 3 4))
