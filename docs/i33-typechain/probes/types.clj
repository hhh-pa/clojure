;; S-typechain oracle probes: stage 1 types (oracle: alpha10 jars, issue #33).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i33-typechain/probes/types.clj > docs/i33-typechain/probes/types.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- top-level type definitions (certain) ----
(definterface Adder (add [x y]))
(definterface M2 (m [x]) (m [x y]))
(deftype Pair [a b])
(deftype P2 [a] Adder (add [this x y] (+ a x y)))
(deftype D2 [] M2 (m [this x] [:one x]) (m [this x y] [:two x y]))
(defrecord Point [x y])
;; ---- A: deftype ----
(throwof "dtor-class" '(class (Pair. 10 20)))
(throwof "dtor-field" '(.-a (Pair. 10 20)))
(throwof "dtor-kw" '(:a (Pair. 1 2)))
(throwof "dtor-get" '(get (Pair. 1 2) :a :dflt))
(throwof "dtor-count" '(count (Pair. 1 2)))
(throwof "dtor-eq" '(= (Pair. 1 2) (Pair. 1 2)))
(throwof "dtor-record?" '(record? (Pair. 1 2)))
(throwof "dtor-map?" '(map? (Pair. 1 2)))
(throwof "dtor-mapinst" '(instance? clojure.lang.IPersistentMap (Pair. 1 2)))
(throwcause "dtor-arity1" '(Pair. 1))
(throwcause "dtor-arity0" '(Pair.))
(throwof "dtor-meth" '(.add (P2. 1) 2 3))
(throwof "dtor-m1" '(.m (D2.) 1))
(throwof "dtor-m2" '(.m (D2.) 1 2))
(throwof "dtor-supers" '(sort (map str (supers Pair))))
;; ---- B: defrecord ----
(throwof "rec-pos" '(:x (->Point 1 2)))
(throwof "rec-mapf" '(:y (map->Point {:x 1 :y 2})))
(throwof "rec-mapf-extra" '(:z (map->Point {:x 1 :y 2 :z 3})))
(throwof "rec-ctor" '(:x (Point. 1 2)))
(throwof "rec-record?" '(record? (->Point 1 2)))
(throwof "rec-map?" '(map? (->Point 1 2)))
(throwof "rec-count" '(count (->Point 1 2)))
(throwof "rec-keys" '(keys (->Point 1 2)))
(throwof "rec-vals" '(vals (->Point 1 2)))
(throwof "rec-seq" '(seq (->Point 1 2)))
(throwof "rec-get" '(get (->Point 1 2) :nope :dflt))
(throwof "rec-contains" '(contains? (->Point 1 2) :y))
(throwof "rec-find" '(find (->Point 1 2) :x))
(throwof "rec-kw-dflt" '(:nope (->Point 1 2) :dflt))
(throwof "rec-assoc-type" '(class (assoc (->Point 1 2) :z 3)))
(throwof "rec-assoc-count" '(count (assoc (->Point 1 2) :z 3)))
(throwof "rec-assoc-val" '(:z (assoc (->Point 1 2) :z 3)))
(throwof "rec-dissoc-type" '(class (dissoc (->Point 1 2) :x)))
(throwof "rec-dissoc-count" '(count (dissoc (->Point 1 2) :x)))
(throwof "rec-eq" '(= (->Point 1 2) (->Point 1 2)))
(throwof "rec-eq-map" '(= (->Point 1 2) {:x 1 :y 2}))
(throwof "rec-eq-diff" '(= (->Point 1 2) (->Point 1 3)))
(throwof "rec-merge-type" '(class (merge (->Point 1 2) {:z 3})))
(throwof "rec-into" '(into {} (->Point 1 2)))
(throwof "rec-select" '(select-keys (->Point 1 2) [:y]))
(throwcause "rec-arity" '(->Point 1))
(throwof "rec-supers" '(sort (map str (supers Point))))
(throwof "rec-iface" '(str Adder))
;; ---- C: reify ----
(throwof "reify-str" '(str (reify Object (toString [this] "hi!"))))
(throwof "reify-meth" '(.add (reify Adder (add [this x y] (* x y))) 3 4))
(throwof "reify-self-eq" '(let [r (reify Object)] (= r r)))
(throwof "reify-distinct" '(= (reify Object) (reify Object)))
(throwof "reify-objinst" '(instance? java.lang.Object (reify Object (toString [this] "x"))))
;; ---- D: proxy ----
(throwof "proxy-str" '(str (proxy [Object] [] (toString [] "prox"))))
(throwof "proxy-objinst" '(instance? java.lang.Object (proxy [Object] [] (toString [] "p"))))
(throwof "proxy-iface" '(.add (proxy [Object Adder] [] (add [x y] (- x y))) 10 4))
;; normhex: stabilize identity-hash suffixes (and reify counters).
(defn normhex [s]
  (-> s
      (clojure.string/replace #"eval\d+" "evalN")
      (clojure.string/replace #"reify__\d+" "reify__N")
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@H")))
(defn prof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn strof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str (normhex (str v))))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; ---- G: predicates (record vs deftype) ----
(throwof "g-coll-rec" '(coll? (->Point 1 2)))
(throwof "g-coll-dt" '(coll? (Pair. 1 2)))
(throwof "g-assoc-rec" '(associative? (->Point 1 2)))
(throwof "g-assoc-dt" '(associative? (Pair. 1 2)))
(throwof "g-counted-rec" '(counted? (->Point 1 2)))
(throwof "g-counted-dt" '(counted? (Pair. 1 2)))
(throwof "g-seqable-rec" '(seqable? (->Point 1 2)))
(throwof "g-seqable-dt" '(seqable? (Pair. 1 2)))
(throwof "g-sequential-rec" '(sequential? (->Point 1 2)))
(throwof "g-sequential-dt" '(sequential? (Pair. 1 2)))
(throwof "g-set-rec" '(set? (->Point 1 2)))
(throwof "g-set-dt" '(set? (Pair. 1 2)))
(throwof "g-vector-rec" '(vector? (->Point 1 2)))
(throwof "g-vector-dt" '(vector? (Pair. 1 2)))
(throwof "g-list-rec" '(list? (->Point 1 2)))
(throwof "g-list-dt" '(list? (Pair. 1 2)))
(throwof "g-ifn-rec" '(ifn? (->Point 1 2)))
(throwof "g-ifn-dt" '(ifn? (Pair. 1 2)))
(throwof "g-fn-rec" '(fn? (->Point 1 2)))
(throwof "g-fn-dt" '(fn? (Pair. 1 2)))
;; ---- H: record extras ----
(throwof "h-empty" '(empty (->Point 1 2)))
(throwof "h-empty-type" '(class (empty (->Point 1 2))))
(throwof "h-empty?" '(empty? (->Point 1 2)))
(throwof "h-conj-vec" '(conj (->Point 1 2) [:z 3]))
(throwof "h-conj-vec-type" '(class (conj (->Point 1 2) [:z 3])))
(throwof "h-conj-map-type" '(class (conj (->Point 1 2) {:z 3})))
(throwof "h-dissoc-missing-type" '(class (dissoc (->Point 1 2) :nope)))
(throwof "h-assoc-existing" '(assoc (->Point 1 2) :x 9))
(throwcause "h-nth" '(nth (->Point 1 2) 0))
(throwof "h-contains-missing" '(contains? (->Point 1 2) :nope))
(throwof "h-mapnil" '(map->Point nil))
(throwof "h-mapempty" '(map->Point {}))
(throwcause "h-invoke-hit" '((->Point 1 2) :x))
(throwcause "h-invoke-miss" '((->Point 1 2) :nope :d))
(throwcause "h-invoke-arity" '((->Point 1 2) :x :d :extra))
(throwcause "h-transient-type" '(class (transient (->Point 1 2))))
(throwof "h-update-type" '(class (update (->Point 1 2) :x inc)))
(throwof "h-getin" '(get-in (->Point 1 2) [:x]))
(throwof "h-destructure" '(let [{:keys [x]} (->Point 1 2)] x))
;; ---- I: deftype errors ----
(throwcause "i-assoc" '(assoc (Pair. 1 2) :a 1))
(throwcause "i-dissoc" '(dissoc (Pair. 1 2) :a))
(throwcause "i-conj" '(conj (Pair. 1 2) [:a 1]))
(throwcause "i-keys" '(keys (Pair. 1 2)))
(throwcause "i-seq" '(seq (Pair. 1 2)))
(throwcause "i-contains" '(contains? (Pair. 1 2) :a))
(throwcause "i-find" '(find (Pair. 1 2) :a))
(throwcause "i-nth" '(nth (Pair. 1 2) 0))
(throwcause "i-empty" '(empty (Pair. 1 2)))
(throwcause "i-invoke" '((Pair. 1 2) :a))
(throwcause "i-ctor-extra" '(Pair. 1 2 3))
;; ---- J: print / str forms ----
(strof "j-str-rec" '(str (->Point 1 2)))
(throwof "j-pr-rec" '(pr-str (->Point 1 2)))
(throwof "j-print-rec" '(with-out-str (print (->Point 1 2))))
(strof "j-str-dt" '(Pair. 1 2))
(throwof "j-str-proxy" '(str (proxy [Object] [] (toString [] "prox"))))
(strof "j-tostring-dt" '(.toString (Pair. 1 2)))
(strof "j-str-reifybare" '(reify Object))
;; ---- K: method / field errors ----
(throwcause "k-meth-arity" '(.add (P2. 1) 2))
(throwcause "k-meth-nosuch" '(.nosuch (P2. 1)))
(throwcause "k-meth-wrongtype" '(.add (Pair. 1 2) 1 2))
(throwcause "k-field-nosuch" '(.-nosuch (Pair. 1 2)))
;; ---- L: hashes ----
(throwof "l-hash-rec" '(hash (->Point 1 2)))
(throwof "l-hash-map" '(hash {:x 1 :y 2}))
(throwof "l-rec-hashcode" '(.hashCode (->Point 1 2)))
(throwof "l-map-hashcode" '(.hashCode {:x 1 :y 2}))
;; hash-solver inputs: candidate parts of the record-hash algorithm
(throwof "l-hash-p13" '(hash (->Point 1 3)))
(throwof "l-hash-m13" '(hash {:x 1 :y 3}))
(throwof "l-hash-strname" '(hash "user.Point"))
(throwof "l-hash-symname" '(hash 'user.Point))
(throwof "l-hash-swapped" '(hash (->Point 2 1)))
(throwof "l-hash-ext" '(hash (assoc (->Point 1 2) :z 3)))
(throwof "l-hash-seq" '(hash (seq (->Point 1 2))))
;; ---- M: deftype print forms (default print-method) ----
(prof "m-pr-dt" '(Pair. 1 2))
(prof "m-print-dt" '(with-out-str (print (Pair. 1 2))))
(prof "m-pr-reify" '(reify Object (toString [this] "hi!")))
(prof "m-print-reify" '(with-out-str (print (reify Object (toString [this] "hi!")))))
;; ---- N: dashed type names (munging), compare/rseq/meta edges ----
(defrecord my-rec [x])
(throwof "n-hash-dashed" '(hash (my-rec. 1)))
(throwof "n-map-x1" '(hash {:x 1}))
(throwof "n-sym-underscore" '(hash (symbol "user.my_rec")))
(throwof "n-sym-dash" '(hash (symbol "user.my-rec")))
(throwcause "n-compare" '(compare (->Point 1 2) (->Point 1 2)))
(throwcause "n-rseq" '(rseq (->Point 1 2)))
(throwof "n-pr-dashed" '(pr-str (my-rec. 1)))
(throwof "n-meta" '(meta (with-meta (->Point 1 2) {:a 1})))
(throwof "n-pr-meta" '(binding [*print-meta* true] (pr-str (with-meta (->Point 1 2) {:a 1}))))
;; ---- E: gen-interface / gen-class (uncertain: wrapped, exit 0 either way) ----
(try
  (let [v (eval '(gen-interface :name user.Greetable :methods [[greet [String] String]]))]
    (println (str "geniface " (pr-str v))))
  (catch Throwable t
    (println (str "geniface THROW " (.getName (class t)) "|" (pr-str (.getMessage t))))))
(try
  (let [v (eval '(do (gen-class :name user.Klass :prefix "klass-" :methods [[hello [] String]])
                     (import 'user.Klass)
                     (defn klass-hello [this] "yo")
                     (.hello (Klass.))))]
    (println (str "genclass " (pr-str v))))
  (catch Throwable t
    (println (str "genclass THROW " (.getName (class t)) "|" (pr-str (.getMessage t))))))
