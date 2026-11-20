;; Collections print their scalar children from the parent's options in one
;; buffer (#271): every shape, binding and print-method override prints as
;; the per-child dispatch did.
(defrecord R [a b])
(prn [1 "a" \b :k nil true 1.5] '(1 2 "x") #{:s} {:a 1 :b "two" :c [3 4]})
(println [1 "a" \b :k nil true 1.5] '(1 2 "x") {:a "q\"uote" :n nil})
(prn [[1 2] [3 [4 5]]] {[1 2] {:x #{3}}} (list [1] (list 2)) [(->R 1 "r")] (map inc [1 2 3]))
(prn (first {:k "v"}) (subvec [1 2 3 4] 1 3) (sorted-map :b 2 :a 1) (sorted-set 3 1 2))
(binding [*print-length* 2] (prn [1 2 3 4] '(1 2 3) {:a 1 :b 2 :c 3} #{1 2 3}) (println [[1 2 3] [4 5 6]]))
(binding [*print-level* 1] (prn [1 [2 [3]]] {:a {:b 1}}) (println [1 "s" [2]]))
(binding [*print-meta* true] (prn (with-meta [1 2] {:tag :v}) [(with-meta [3] {:m 1}) 4]))
(binding [*print-dup* true] (prn [1 "s" :k] {:a 1}))
(binding [*print-readably* false] (prn ["s" \c] {:k "v"}))
(binding [*print-namespace-maps* true] (prn {:a/x 1 :a/y 2} {:a/x 1 :b 2}))
(println (pr-str [1 "a"] {:k \c}) (print-str [1 "a"]) (with-out-str (prn [1 2]) (print #{:x})))
(println (count (with-out-str (dotimes [i 20] (prn [i (str "s" i) (keyword (str "k" i))])))))
(prn [] () {} #{} [nil] [""])
(defmethod print-method clojure.lang.Keyword [k w] (.write w (str "KW<" (name k) ">")))
(prn [:a 1 {:b 2}] #{:c})
