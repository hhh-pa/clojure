;; Special forms and control flow lowered natively.
(def ^:dynamic *depth* 0)
(def ^{:doc "a counter"} counter (atom 0))
(defn ^:private bump! [] (swap! counter inc))
(declare later)
(defn calls-later [x] (later x))
(defn later [x] (* x 3))
(println (calls-later 5) (:doc (meta #'counter)) (:private (meta #'bump!)))
(println #{1 2 3} (count #{:a :b}) ^:m [1 2] {:a 1 :b [2 #{3}]})
(println (letfn [(ev? [n] (if (zero? n) true (od? (dec n))))
                 (od? [n] (if (zero? n) false (ev? (dec n))))]
           [(ev? 10) (od? 7)]))
(defn classify [x]
  (case x
    1 :one
    (2 3) :two-or-three
    "s" :string
    :k :keyword
    [1 2] :vec
    nil :nil
    :default))
(println (map classify [1 2 3 "s" :k [1 2] nil 99]))
(defn sym-case [s] (case s a 1 b 2 (c d) 3 0))
(println (map sym-case '[a b c d e]))
(defn char-case [c] (case c \a 1 \b 2 0))
(println (map char-case "abz"))
(binding [*depth* 5]
  (println *depth*)
  (set! *depth* 6)
  (println *depth*))
(println *depth*)
(def d (delay (println "realized") 42))
(println (realized? d) @d @d)
(def bf (binding [*depth* 9] (bound-fn [] *depth*)))
(println (bf))
(def named-bf (bound-fn self [n] (if (pos? n) (self (dec n)) :done)))
(println (named-bf 3))
(println (try (assert (= 1 2) "math is broken") (catch AssertionError e (.getMessage e))))
(println (try (assert false) (catch AssertionError e (.getMessage e))))
(println (do (assert true) :ok))
(println (if-let [x nil] x :else) (when-not false :w) (cond-> 1 true inc false dec))
(println (let [x 1] (letfn [(f [] x)] (f))))
(println Long/MAX_VALUE Math/PI (class "s") (str #"a+b") (re-find #"\d+" "ab123"))
(println 'sym 'ns/sym :kw :ns/kw \c 1.5 -3 2/3 1N 1.5M "s\n")
(println (quote (1 2 x)) '{:a [b]} (type 'x))
(println (instance? String "x") String)
(println ((fn [])) (let [a 1]) (do) (loop [] nil) (binding [] :empty-binding) (with-redefs [] :empty-redefs))
(println (try (inc 1 2) (catch clojure.lang.ArityException _ :arity)) (try (dec) (catch clojure.lang.ArityException _ :arity0)))
(defn ^{:v [(inc 1)]} meta-vec [] 1)
(defn ^{:s #{(inc 2)}} meta-set [] 2)
(defn ^{:m {:a (inc 3)}} meta-map [] 3)
(println (:v (meta #'meta-vec)) (:s (meta #'meta-set)) (:m (meta #'meta-map)) (meta-vec) (meta-set) (meta-map))
