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
                  (clojure.string/replace #"--\d+" "--N"))]
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
