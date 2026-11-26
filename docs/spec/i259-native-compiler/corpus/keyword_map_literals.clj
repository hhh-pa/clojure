;; Keyword map literals through a shaped template (#271): the literal's
;; keys, order, equality and class match a pairs-built map; values
;; evaluate in order; other literals keep the pairs path.
(def log (atom []))
(defn note [x] (swap! log conj x) x)
(defn point [x y] {:x x :y y})
(println (point 1 2) (= (point 1 2) {:y 2 :x 1}) (keys (point 3 4)) (class (point 5 6)))
(println {:a (note 1) :b (note 2) :c (note 3)} @log)
(println (assoc (point 1 2) :z 3) (dissoc (point 1 2) :x) (merge (point 1 2) {:x 9}))
(println (reduce (fn [acc i] (conj acc {:i i :sq (* i i)})) [] (range 4)))
(println {:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8})
(println {:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9})
(println (let [k :dyn] {k 1 :b 2}) {"s" 1 :k 2} {:ns/q 1 :q 2} ^{:m 1} {:a 1})
(println (meta ^{:m 1} {:a (note 4)}) (get {:x nil} :x :absent) (count {:only 1}))
(println (frequencies (map (fn [i] {:p (mod i 2)}) (range 5))))
