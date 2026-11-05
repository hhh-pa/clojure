;; S5D differential corpus 10: sort, sort-by, reverse, compare, max-key, min-key.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (sort), s5d-gaps7e (sort/reverse/maxkey), s5d-gaps7g4
;; (arities), s5d-gaps7g5 (nil args, empty), s5d-gaps7g12 (sort classes),
;; gaps2/gaps3/gaps4 (stability, comparators, ties), s5d-gap/gap2 (compare),
;; probe39 (reverse-sort, cmp-compare, max-min-key).
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn cls [tag thunk]
  (print tag " => ")
  (prn (try (let [v (thunk)] [(class v) (seq? v) (counted? v) (list? v) (sequential? v) (coll? v)])
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- sort values (7e, gaps3, gaps4, probe39, 7g5) ---
(p ":sort-basic" #(sort [3 1 2]))
(p ":sort-gt" #(sort > [3 1 2]))
(p ":sort-lt" #(sort < [3 1 2]))
(p ":sort-boolcmp" #(sort (fn [a b] (< a b)) [3 1 2]))
(p ":sort-numcmp" #(sort (fn [a b] (- a b)) [3 1 2]))
(p ":sort-badcmp" #(sort (fn [a b] :x) [3 1 2]))
(p ":sort-fn-false" #(sort (fn [x y] false) [3 1 2]))
(p ":sort-fn-number" #(sort (fn [x y] (- x y)) [3 1 2]))
(p ":sort-fn-nonnum" #(sort (fn [x y] :k) [2 1]))
(p ":sort-nil" #(sort nil))
(p ":sort-empty" #(sort []))
(p ":sort-empty-str" #(sort ""))
(p ":sort-strs" #(sort ["b" "a"]))
(p ":sort-mixed" #(sort [1 "a"]))
(p ":sort-nonnil" #(sort [2 nil 1]))
(p ":sort-stability" #(sort-by first [[1 :a] [1 :b] [0 :c]]))
(p ":a-sort" #(eval (list 'sort)))

;; --- sort classes / counted / meta (7g12, gaps2, NEW meta-drop) ---
(p ":sort-class" #(str (class (sort [3 1 2]))))
(p ":sort-counted" #(counted? (sort [3 1 2])))
(p ":sort-empty-class" #(str (class (sort []))))
(p ":sort-nil-class" #(str (class (sort nil))))
(p ":sort-meta" #(meta (sort (with-meta [2 1] {:a 1}))))
(p ":sortby-meta" #(meta (sort-by identity (with-meta [2 1] {:a 1}))))

;; --- sort-by (7e, 7g12, gaps4, probe39, 7g5, NEW keyfn-call count) ---
(p ":sortby-key" #(sort-by :a [{:a 2} {:a 1}]))
(p ":sortby-ident" #(sort-by identity [3 1 2]))
(p ":sortby-cmp" #(sort-by :a > [{:a 1} {:a 3} {:a 2}]))
(p ":sortby-nil" #(sort-by :a nil))
(p ":sortby-empty-str" #(sort-by :a ""))
(p ":sortby-count" #(sort-by count ["aaa" "a" "aa"]))
(p ":sortby-compare" #(sort-by :a compare [{:a 2} {:a 1}]))
(p ":sortby-class" #(str (class (sort-by :a [{:a 2} {:a 1}]))))
(p ":sortby-keyfn-calls" #(let [n (atom 0)] (sort-by (fn [x] (swap! n inc) x) [3 1 2]) @n))
(p ":a-sortby" #(eval (list 'sort-by)))

;; --- reverse (7e, probe39, 7g4, 7g5) ---
(cls ":c-reverse" #(reverse [1 2]))
(p ":reverse-str" #(reverse "ab"))
(p ":reverse-nil" #(reverse nil))
(p ":reverse-num" #(reverse 5))
(p ":reverse-vec" #(reverse [1 2 3]))
(p ":reverse-abc" #(reverse "abc"))
(p ":rev-empty" #(reverse ""))
(p ":a-reverse" #(eval (list 'reverse)))

;; --- compare (s5d-gap/gap2, probe39 cmp-compare, 7g5) ---
(p ":cmp-nilnil" #(compare nil nil))
(p ":cmp-nil1" #(compare nil 1))
(p ":cmp-1nil" #(compare 1 nil))
(p ":cmp-strs" #(compare "a" "b"))
(p ":cmp-str-self" #(compare "a" "a"))
(p ":cmp-nums" #(compare 1 2))
(p ":cmp-kws" #(compare :a :b))
(p ":cmp-vecs" #(compare [1] [2]))
(p ":cmp-vec-prefix" #(compare [1 2] [1]))
(p ":cmp-num-str" #(compare 1 "a"))
(p ":cmp-str-num" #(compare "a" 1))
(p ":cmp-list-vec" #(compare '(1) [1]))
(p ":cmp-lists" #(compare '(1) '(2)))
(p ":cmp-vec-num" #(compare [1] 5))
(p ":cmp-num-vec" #(compare 5 [1]))
(p ":cmp-bools" #(compare true false))
(p ":cmp-chars" #(compare \a \b))
(p ":cmp-maps" #(compare {:a 1} {:a 1}))
(p ":cmp-sets" #(compare #{1} #{1}))

;; --- max-key / min-key (7e, 7g4, 7g5, gaps3, gaps4, probe39) ---
(p ":maxkey-basic" #(max-key :a {:a 1} {:a 3} {:a 2}))
(p ":maxkey-1" #(max-key :a {:a 1}))
(p ":maxkey-count" #(max-key count [1] [1 2] [1 2 3]))
(p ":maxkey-tie" #(max-key count [1 2] [3 4]))
(p ":maxkey-single" #(max-key count [1]))
(p ":maxkey-noargs" #(max-key :a))
(p ":maxkey-nonnum" #(max-key identity "a" "b"))
(p ":minkey-basic" #(min-key count "a" "abc" "ab"))
(p ":minkey-count" #(min-key count [1] [1 2]))
(p ":minkey-first" #(min-key first [2 :x] [1 :y]))
(p ":minkey-tie" #(min-key count [1 2] [3 4]))
(p ":minkey-noargs" #(min-key :a))
(p ":a-maxkey" #(eval (list 'max-key)))
(p ":a-minkey" #(eval (list 'min-key)))
(println "S5D-10 DONE")
