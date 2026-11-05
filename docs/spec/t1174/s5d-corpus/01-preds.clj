;; S5D differential corpus 01: predicates, scalar edges, class names.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Predicates/classes), s5d-gaps7b, s5d-gaps7d (distinct?),
;; s5d-gaps7g4 (:a-distinctq), probe39 pred-*.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- collection predicate matrix (7b) ---
(def preds
  [coll? sequential? associative? counted? list? vector? map? set? seq? string? seqable? ifn? fn?])
(def pnames
  ["coll?" "sequential?" "associative?" "counted?" "list?" "vector?" "map?" "set?" "seq?" "string?" "seqable?" "ifn?" "fn?"])
(println (str "val\t" (clojure.string/join "\t" pnames)))
(doseq [[n v] [["nil" nil] ["true" true] ["0" 0] ["1.5" 1.5] ["1N" 1N] ["2/3" 2/3]
               ["\\a" \a] ["s" "s"] [":k" :k] ["'s" 's]
               ["[]" []] ["[1]" [1]] ["{}" {}] ["#{}" #{}]
               ["'(1)" '(1)] ["(list)" (list)] ["(seq[1])" (seq [1])]
               ["(lazy-seq)" (lazy-seq [1])] ["(range)" (range 3)]
               ["(map)" (map inc [1])] ["(concat)" (concat [1] [2])]
               ["(repeat)" (repeat 2 :x)] ["(str-seq)" (seq "ab")]
               ["#'+ " #'+] ["(fn)" (fn [x] x)] ["(atom)" (atom 1)]
               ["(delay)" (delay 1)] ["(var)" #'map]]]
  (print n)
  (doseq [pr preds] (print "\t" (pr v)))
  (newline))

;; --- scalar predicate edges (7b) ---
(doseq [[n v] [["int?" [1 1N 1.5 2/3 "1" nil]] ["integer?" [1 1N 1.5 2/3 1.0M]]
               ["float?" [1.0 (float 1.0) 1]] ["double?" [1.0 (float 1.0) 1]]
               ["number?" [1 1N 2/3 1.5 1.5M "1" nil]]
               ["char?" [\a "a" 97]] ["boolean?" [true false nil 1]]
               ["record?" [{:a 1} nil]] ["var?" [#'map 1]] ["fn?" [#'+ :k [1]]]
               ["ifn?" [#'+ :k 's [1] {:a 1} #{1} #'map 1 nil "(lazy)" ]]
               ["ident?" [:a 'a "a" nil]] ["qualified-ident?" [:a/b 'a/b :a 'a]]
               ["simple-ident?" [:a/b :a 'a/b 'a]]
               ["qualified-keyword?" [:a/b :a 'a/b]] ["simple-keyword?" [:a/b :a]]
               ["qualified-symbol?" ['a/b 'a :a/b]] ["simple-symbol?" ['a/b 'a]]]]
  (let [pr (resolve (symbol n))]
    (print n)
    (doseq [v (if (= n "ifn?") [#'+ :k 's [1] {:a 1} #{1} #'map 1 nil (map inc [1])] v)]
      (print "\t" (pr v)))
    (newline)))

;; --- truth / misc preds (probe39 pred-truth, pred-kinds extras) ---
(p ":nil?-t" #(nil? nil))
(p ":nil?-f" #(nil? false))
(p ":not-nil" #(not nil))
(p ":not-0" #(not 0))
(p ":not-str" #(not ""))
(p ":some?-nil" #(some? nil))
(p ":some?-false" #(some? false))
(p ":true?-1" #(true? 1))
(p ":false?-nil" #(false? nil))
(p ":seqable?-1" #(seqable? 1))
(p ":fn?-native" #(fn? inc))
(p ":fn?-str" #(fn? str))
(p ":ifn?-lazy" #(ifn? (map inc [1])))

;; --- 32-bit ints (pins: port keeps (int x) as Long -> DIVERGENT) ---
(p ":int-class" #(str (class (int 5))))
(p ":int?-int" #(int? (int 5)))
(p ":int?-Integer" #(int? (Integer/valueOf 5)))
(p ":integer?-int" #(integer? (int 5)))
(p ":int?-1N" #(int? 1N))
(p ":int-eq-long" #(= (int 5) 5))

;; --- record? (pins: port always false -> DIVERGENT for real records) ---
(defrecord R39 [a])
(p ":record?-inst" #(record? (->R39 1)))
(p ":record?-map" #(record? {:a 1}))
(p ":record?-nil" #(record? nil))

;; --- class-name rule: strip package only (pins simple_class_of) ---
(p ":class-lazyseq" #(str (class (lazy-seq [1]))))
(p ":class-mapfn" #(str (class (map inc [1]))))
(p ":class-filterfn" #(str (class (filter odd? [1]))))
(p ":class-mapxform" #(str (class (map inc))))
(p ":class-filterxform" #(str (class (filter odd?))))
(p ":class-range" #(str (class (range 5))))
(p ":class-repeat" #(str (class (repeat 2 :x))))
(p ":class-cycle" #(str (class (cycle [1]))))
(p ":class-iterate" #(str (class (iterate inc 0))))
(p ":class-cons" #(str (class (cons 1 [2]))))
(p ":class-mapSeq" #(str (class (seq {:a 1}))))
(p ":class-strSeq" #(str (class (seq "ab"))))
(p ":class-emptylist" #(str (class '())))
(p ":class-sort" #(str (class (sort [2 1]))))
(p ":class-keyword" #(str (class :a)))
(p ":class-symbol" #(str (class 'a)))
(p ":class-var" #(str (class #'map)))
(p ":class-atom" #(str (class (atom 1))))

;; --- distinct? (7d, 7g4, probe39) ---
(p ":distinct?-0" #(distinct?))
(p ":a-distinctq" #(eval (list 'distinct?)))
(p ":distinct?-1" #(distinct? 1))
(p ":distinct?-eq" #(distinct? 1 1))
(p ":distinct?-late" #(distinct? 1 2 1))
(p ":distinct?-ok" #(distinct? 1 2 3))
(p ":distinct?-mixed" #(distinct? 1 1.0))
(p ":distinct?-nil" #(distinct? nil nil))
(p ":distinct?-strs" #(distinct? "a" "a"))
(println "S5D-01 DONE")
