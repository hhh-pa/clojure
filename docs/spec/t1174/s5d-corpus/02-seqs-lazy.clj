;; S5D differential corpus 02: seqs, lazy semantics, seq lib (map/filter/reduce/...).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Repeat/cycle/iterate/cons, take-last/butlast/drop, sort),
;; s5d-gaps7e (seq fns), s5d-gaps7g/7g2/7g3 (seq-type matrices), s5d-gaps7g9/7g10
;; (seq classes), s5d-gaps7g4 (0-arities), gaps/gaps2/gaps3/gaps4 (chunking,
;; xforms, reduce), probe39 (seq-*, map-*, reduce-*, ...).
;; NOTE: bare transducer fns are never printed raw (identity hash); entries
;; print [(fn? t) (str (class t))] instead.
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

;; --- seq-type matrix (7g2 + 7g (tree) row) ---
(def tpreds [coll? sequential? associative? counted? list? vector? map? set? seq?])
(def tnames ["coll?" "sequential?" "associative?" "counted?" "list?" "vector?" "map?" "set?" "seq?"])
(println (str "val\t" (clojure.string/join "\t" tnames)))
(doseq [[n v] [["(seq map)" (seq {:a 1})] ["(seq set)" (seq #{1})]
               ["(seq list)" (seq '(1))] ["(seq str)" (seq "ab")]
               ["(seq vec)" (seq [1 2])] ["(range)" (range 5)] ["(range0)" (range 0)]
               ["(take)" (take 2 [1 2])] ["(drop)" (drop 1 [1 2])]
               ["(filter)" (filter pos? [1])] ["(cons-nil)" (cons 1 nil)]
               ["(cons-vec)" (cons 1 [2])] ["(rest-vec)" (rest [1 2])]
               ["(next-vec)" (next [1 2])] ["(cycle)" (cycle [1])]
               ["(reverse)" (reverse [1 2])] ["(sort)" (sort [2 1])]
               ["(butlast)" (butlast [1 2])] ["(mapcat)" (mapcat list [1])]
               ["(interleave)" (interleave [1] [2])] ["(partition)" (partition 2 [1 2 3])]
               ["(dedupe)" (dedupe [1])] ["(reductions)" (reductions + [1 2])]
               ["(take-nth)" (take-nth 2 [1 2])] ["(iterate)" (iterate inc 0)]
               ["(repeat-n)" (repeat 2 1)] ["(keep)" (keep pos? [1])]
               ["(mapidx)" (map-indexed vector [1])] ["(distinct)" (distinct [1])]
               ["(partby)" (partition-by identity [1])] ["(interpose)" (interpose 0 [1])]
               ["(cycle-nil)" (cycle nil)] ["(lazycat)" (lazy-cat [1] [2])]
               ["(doall)" (doall (map inc [1]))] ["(flatten)" (flatten [[1]])]
               ["(sortby)" (sort-by identity [2 1])]
               ["(split-take)" (nth (split-at 1 [1 2]) 0)]
               ["(grpb-val)" (get (group-by identity [1]) 1)]
               ["(subvec-seq)" (seq (subvec [1 2 3] 1))]
               ["(entry)" (first {:a 1})] ["(map-seq-first)" (first (seq {:a 1}))]
               ["(liststar)" (list* 1 [2])] ["(lazy-took)" (take 1 (map inc [1 2]))]
               ["(dorun)" (dorun [1])] ["(reductions1)" (reductions + [7])]
               ["(tree)" (tree-seq seq? seq [[1]])]]]
  (print n)
  (doseq [pr tpreds] (print "\t" (pr v)))
  (newline))

;; --- seq-type matrix 2: repeat/cycle/drop/take/transient/reduced (7g3, error-tolerant) ---
(println (str "val\t" (clojure.string/join "\t" tnames)))
(doseq [[n v] [["(rep-n)" (repeat 2 :x)] ["(rep-inf)" (repeat :x)]
               ["(repd-n)" (repeatedly 2 (fn [] 1))] ["(repd-inf)" (repeatedly (fn [] 1))]
               ["(cycle-vec)" (cycle [1 2])] ["(interleave1)" (interleave [1 2])]
               ["(concat0)" (concat)] ["(concat1)" (concat [1 2])]
               ["(keys-map)" (keys {:a 1})] ["(vals-map)" (vals {:a 1})]
               ["(part-empty)" (partition 2 [])] ["(map-nil)" (map inc nil)]
               ["(take-nil)" (take 1 nil)] ["(drop0-vec)" (drop 0 [1 2])]
               ["(drop0-list)" (drop 0 '(1 2))] ["(drop-range)" (drop 1 (range 100))]
               ["(drop-str)" (drop 1 "ab")] ["(take-range)" (take 1 (range 100))]
               ["(drop-list)" (drop 1 '(1 2))] ["(take-list)" (take 1 '(1 2))]
               ["(takelast)" (take-last 1 [1 2])] ["(droplast)" (drop-last 1 [1 2])]
               ["(rest-nil)" (rest nil)] ["(butlast-str)" (butlast "ab")]
               ["(rev-nil)" (reverse nil)] ["(sort-nil)" (sort nil)]
               ["(trans-vec)" (transient [1])] ["(trans-map)" (transient {:a 1})]
               ["(trans-set)" (transient #{1})] ["(reduced)" (reduced 5)]
               ["(exinfo)" (ex-info "m" {})]]]
  (print n)
  (doseq [pr tpreds] (print "\t" (try (pr v) (catch Throwable t (str "THROW:" (.getName (class t)))))))
  (newline))

;; --- seq classes (7g9, 7g10) ---
(cls ":c-map" #(map inc [1]))
(cls ":c-reductions" #(reductions + [1 2]))
(cls ":c-lazyseq" #(lazy-seq (cons 1 nil)))
(cls ":c-nextstr" #(next "ab"))
(cls ":c-nextmap" #(next {:a 1 :b 2}))
(cls ":c-seqstr" #(seq "ab"))
(cls ":c-seqmap" #(seq {:a 1}))
(cls ":c-seqset" #(seq #{1}))
(cls ":c-seqvec" #(seq [1 2]))

;; --- seq / first / rest / next / last (7c, 7g10, probe39, 7g5 empties) ---
(p ":second-nil" #(second nil))
(p ":second-1" #(second [1]))
(p ":last-nil" #(last nil))
(p ":last-12" #(last [1 2]))
(p ":last-str" #(last "ab"))
(p ":last-vec" #(last []))
(p ":nnext-short" #(nnext [1]))
(p ":nnext-123" #(nnext [1 2 3]))
(p ":fnext-nil" #(fnext nil))
(p ":fnext-123" #(fnext [1 2 3]))
(p ":nfirst-nest" #(nfirst [[1 2] 3]))
(p ":next-e" #(next []))
(p ":rest-e" #(rest []))
(p ":seq-nil" #(seq nil))
(p ":seq-e" #(seq []))
(p ":seq-1" #(seq [1]))
(p ":first-nil" #(first nil))
(p ":first-e" #(first []))
(p ":first-str" #(first "abc"))
(p ":rest-nil" #(rest nil))
(p ":rest-e2" #(rest []))
(p ":rest-str" #(rest "abc"))
(p ":next-nil" #(next nil))
(p ":next-1" #(next [1]))
(p ":seq-5" #(seq 5))
(p ":seq-empty" #(seq ""))
(p ":first-empty" #(first ""))
(p ":rest-empty" #(rest ""))
(p ":a-seq" #(eval (list 'seq)))
(p ":a-first" #(eval (list 'first)))
(p ":a-second" #(eval (list 'second)))
(p ":a-last" #(eval (list 'last)))
(p ":a-rest" #(eval (list 'rest)))
(p ":a-next" #(eval (list 'next)))
(p ":a-nnext" #(eval (list 'nnext)))
(p ":a-fnext" #(eval (list 'fnext)))
(p ":a-nfirst" #(eval (list 'nfirst)))

;; --- chunking / realization (gaps, probe39) ---
(p ":map-chunked" #(let [n (atom 0) s (map (fn [x] (swap! n inc) x) (range 100))] (let [f (first s) n1 @n g (second s) n2 @n] [f n1 g n2])))
(p ":map-unchunked-list" #(let [n (atom 0) s (map (fn [x] (swap! n inc) x) (apply list (range 100)))] (let [f (first s)] [f @n])))
(p ":repeat-chunked" #(let [n (atom 0) s (map (fn [x] (swap! n inc) x) (repeat 100 0))] [(first s) @n]))
(p ":iterate-chunked" #(let [n (atom 0) s (map (fn [x] (swap! n inc) x) (iterate inc 0))] [(first s) @n]))
(p ":repeatedly-one" #(let [n (atom 0) s (repeatedly (fn [] (swap! n inc) 7))] [(first s) @n]))
(p ":for-chunked" #(let [n (atom 0)] (let [f (first (for [x (range 100)] (do (swap! n inc) x)))] [f @n])))
(p ":lazy-seq-once" #(let [n (atom 0) s (lazy-seq (swap! n inc) [1 2])] (let [a (first s) b (first s)] [a b @n (vec s)])))
(p ":lazy-cat-basic" #(vector (lazy-cat [1 2] [3] nil [4]) (take 2 (lazy-cat (range) [99]))))
(p ":binding-convey-lazy" #(do (def ^:dynamic lb39dyn :root) (binding [lb39dyn :bound] (def lb39s (map (fn [_] lb39dyn) [1]))) [(first lb39s) (binding [lb39dyn :inner] (first (map (fn [_] lb39dyn) [1])))]))

;; --- map / filter / remove / keep (7e, gaps, probe39, 7g4) ---
(p ":map-0" #(map))
(p ":filter-0" #(filter))
(p ":map-1" #(let [t (map inc)] [(fn? t) (str (class t))]))
(p ":filter-xform" #(let [t (filter odd?)] [(fn? t) (str (class t))]))
(p ":map-multi" #(vector (map + [1 2 3] [10 20]) (map vector [:a :b] [1 2]) (map inc nil) (map list [1 2] [3 4] [5 6])))
(p ":map-4-colls" #(map list [1] [2] [3] [4]))
(p ":map-list12" #(map list [1 2]))
(p ":map-empty" #(map inc ""))
(p ":map-nil" #(map inc nil))
(p ":filter-nil" #(vec (filter nil [1])))
(p ":filter-false-coll" #(vec (filter odd? false)))
(p ":filter-remove-keep" #(vector (filter odd? (range 6)) (filter odd? nil) (remove odd? (range 6)) (keep (fn [x] (when (odd? x) (* x 10))) (range 6)) (keep-indexed (fn [i x] (when (odd? i) x)) [:a :b :c :d]) (map-indexed vector [:a :b]) (map-indexed vector nil)))
(p ":keep-nil-fn" #(vec (keep nil [1 nil 2])))
(p ":keepidx" #(keep-indexed (fn [i x] (when (= 1 (mod i 2)) x)) [:a :b :c]))
(p ":mapidx" #(map-indexed vector [:a :b]))
(p ":map-indexed-str" #(map-indexed vector "ab"))
(p ":keep-false" #(keep identity [nil false 0]))
(p ":a-map" #(eval (list 'map)))
(p ":a-filter" #(eval (list 'filter)))
(p ":a-remove" #(eval (list 'remove)))
(p ":a-keep" #(eval (list 'keep)))
(p ":a-keepidx" #(eval (list 'keep-indexed)))
(p ":a-mapidx" #(eval (list 'map-indexed)))

;; --- transducer arities (probe39 seq-xform-arity) ---
(p ":xform-arity" #(vector (fn? (map inc)) (fn? (filter odd?)) (fn? (remove odd?)) (fn? (keep identity)) (fn? (keep-indexed vector)) (fn? (distinct)) (fn? (dedupe)) (boolean (re-find #"map\$fn" (str (class (map inc)))))))

;; --- distinct / dedupe / mapcat (7e, gaps3, probe39, 7g4) ---
(p ":distinct-basic" #(distinct [1 1 2 1 3 2]))
(p ":distinct-empty" #(distinct nil))
(p ":distinct-order" #(distinct [3 1 3 2 1]))
(p ":dedupe-basic" #(dedupe [1 1 2 1]))
(p ":dedupe-str" #(dedupe "aaabbb"))
(p ":dedupe-two-arg" #(dedupe odd? [1 2]))
(p ":mapcat-basic" #(mapcat reverse [[3 2 1] [6 5 4]]))
(p ":mapcat-empty" #(mapcat reverse []))
(p ":mapcat-nil" #(mapcat reverse nil))
(p ":mapcat-seq" #(mapcat seq [[1] [2 3]]))
(p ":mapcat-kw" #(mapcat :a [{:a [1 2]} {:a [3]}]))
(p ":mapcat-nocolls" #(mapcat vector))
(p ":a-distinct" #(eval (list 'distinct)))
(p ":a-dedupe" #(eval (list 'dedupe)))
(p ":a-mapcat" #(eval (list 'mapcat)))

;; --- reduce / reductions / reduced (7e, gaps, gaps3, probe39) ---
(p ":reduce-nofn" #(reduce + [1 2 3]))
(p ":reduce-init" #(reduce + 10 [1 2]))
(p ":reduce-empty" #(reduce + []))
(p ":reduce-empty-init" #(reduce + 10 []))
(p ":reduce-1" #(reduce + [5]))
(p ":reduce-nil" #(reduce + nil))
(p ":reduce-init-nil" #(reduce + 10 nil))
(p ":reduce-early" #(reduce (fn [a b] (if (> b 2) (reduced a) (+ a b))) 0 [1 2 3 4]))
(p ":reduce-init-reduced" #(vector (reduce + (reduced 10) [1 2]) (reduce + (reduced 10) []) (reduce + (reduced 10) nil)))
(p ":reduce-conj-reduced-init" #(reduce conj (reduced [1]) [2]))
(p ":reduce-empty-str" #(reduce + ""))
(p ":reduced?-t" #(reduced? (reduced 1)))
(p ":reduced?-f" #(reduced? 1))
(p ":reductions-basic" #(reductions + [1 2 3]))
(p ":reductions-init" #(reductions + 10 [1 2]))
(p ":reductions-empty" #(reductions + [1]))
(p ":reductions-e" #(reductions + []))
(p ":reductions-nil" #(reductions + nil))
(p ":reductions-reduced-init" #(reductions + (reduced 10) [1 2]))
(p ":reductions-no-init-single" #(reductions + [5]))
(p ":reductions-str" #(reductions str ["a" "b"]))
(p ":reductions-init-str" #(reductions str "S" ["a" "b"]))
(p ":reductions-empty-str" #(reductions + ""))
(p ":a-reduce" #(eval (list 'reduce)))
(p ":a-reductions" #(eval (list 'reductions)))

;; --- group-by / frequencies (7e, probe39, gaps3, gaps4, 7g5) ---
(p ":groupby-basic" #(group-by (fn [x] (= 1 (mod x 2))) [1 2 3 4]))
(p ":groupby-empty" #(group-by identity nil))
(p ":groupby-odd" #(group-by odd? (range 6)))
(p ":groupby-nilcoll" #(group-by :a nil))
(p ":groupby-empty-str" #(group-by identity ""))
(p ":frequencies-basic" #(frequencies [:a :b :a]))
(p ":frequencies-empty" #(frequencies nil))
(p ":frequencies-vec" #(frequencies [:a :b :a]))
(p ":frequencies-order" #(frequencies [:b :a :b]))
(p ":frequencies-str" #(frequencies "abba"))
(p ":frequencies-empty-str" #(frequencies ""))
(p ":a-groupby" #(eval (list 'group-by)))
(p ":a-frequencies" #(eval (list 'frequencies)))

;; --- every? / some / not-any? / not-every? (7e, gaps3, probe39, 7g4, 7g5) ---
(p ":everyq-basic" #(every? pos? [1 2]))
(p ":everyq-nil" #(every? pos? nil))
(p ":everyq-empty" #(every? pos? []))
(p ":everyq-empty-str" #(every? pos? ""))
(p ":some-basic" #(some #{2} [1 2 3]))
(p ":some-nilp" #(some nil? [1 nil]))
(p ":some-empty" #(some identity nil))
(p ":some-kw" #(some :a [{:b 1} {:a 2}]))
(p ":some-odd-nil" #(some odd? nil))
(p ":some-empty-str" #(some pos? ""))
(p ":notany-basic" #(not-any? nil? [1 2]))
(p ":notany-nil" #(not-any? nil? nil))
(p ":notany-empty" #(not-any? odd? []))
(p ":notevery-basic" #(not-every? pos? [1 -2]))
(p ":notevery-nil" #(not-every? pos? nil))
(p ":notevery-empty" #(not-every? odd? []))
(p ":a-everyq" #(eval (list 'every?)))
(p ":a-some" #(eval (list 'some)))
(p ":a-notany" #(eval (list 'not-any?)))
(p ":a-notevery" #(eval (list 'not-every?)))

;; --- mapv / filterv (7e, probe39, 7g4, 7g5) ---
(p ":mapv-basic" #(mapv inc [1 2]))
(p ":mapv-multi" #(mapv + [1 2] [10 20]))
(p ":mapv-3" #(mapv + [1] [2] [3]))
(p ":mapv-nil" #(mapv inc nil))
(p ":mapv-empty-str" #(mapv inc ""))
(p ":filterv-basic" #(filterv pos? [1 -2 3]))
(p ":filterv-range" #(filterv odd? (range 5)))
(p ":filterv-nil" #(filterv odd? nil))
(p ":a-mapv" #(eval (list 'mapv)))
(p ":a-filterv" #(eval (list 'filterv)))
(println "S5D-02 DONE")
