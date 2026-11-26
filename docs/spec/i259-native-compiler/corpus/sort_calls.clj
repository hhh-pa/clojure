;; sort and sort-by through prepared key functions and comparators (#271):
;; the booted compare orders directly, every other comparator and key
;; function keeps its own answers, call counts and errors.
(def ts (vec (for [i (range 30)] [(mod (* i 7) 11) (- 50 i) (str "s" (mod i 5))])))
(println (sort-by first ts))
(println (sort-by second ts))
(println (sort-by #(nth % 2) ts))
(println (sort-by first > ts))
(println (sort-by (juxt first second) ts))
(println (sort-by :a [{:a 3} {:a 1} {:a 2}]) (sort-by count ["ccc" "a" "bb"]))
(println (sort > [3 1 2]) (sort < [3 1 2]) (sort compare [3 1.5 2]) (sort (comparator <) [3 1 2]))
(println (sort #(compare %2 %1) ["b" "a" "c"]) (sort-by first compare (take 5 ts)))
(println (sort-by identity [nil 2 1]) (sort-by - [1 3 2]) (sort-by first [[1 :a] [1 :b] [0 :c]]))
(println (sort (fn [a b] (< (count a) (count b))) ["ccc" "a" "bb"]) (sort-by identity []) (sort-by identity [7]))
(println (sort #'compare [3 1 2]) (sort-by second #'compare (take 4 ts)) (sort-by first #(compare %2 %1) (take 4 ts)))
(println (try (sort-by identity [1 "a"]) (catch ClassCastException e :cce)))
(println (try (sort-by first [1 2]) (catch Exception e :bad-key)) (try (sort-by 5 [1 2]) (catch Exception e :not-fn)))
(let [calls (atom 0)]
  (doall (sort-by (fn [x] (swap! calls inc) x) (range 20 0 -1)))
  (println :keyfn-calls @calls))
(let [calls (atom 0)]
  (doall (sort (fn [a b] (swap! calls inc) (compare a b)) (range 20 0 -1)))
  (println :comparator-calls @calls))
