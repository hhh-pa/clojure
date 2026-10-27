;; S10 oracle probes: comparator + hash helpers (oracle: alpha10 jars, issue #53).
;; Run from repo root (/home/hhu/workspace/clojure-s10):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s10-cells/probes/helpers.clj > docs/zero-oos-s10-cells/probes/helpers.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
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
;; ---- comparator call convention: (cmp x y) -> -1/1/0 ----
(throwof "k-lt" '((comparator <) 1 2))
(throwof "k-gt" '((comparator <) 2 1))
(throwof "k-eq" '((comparator <) 1 1))
(throwof "k-flip" '((comparator >) 1 2))
(throwof "k-flip2" '((comparator >) 2 1))
(throwof "k-sort" '(sort (comparator >) [1 2 3]))
(throwof "k-sorted-map" '(keys (into (sorted-map-by (comparator >)) {1 :a 2 :b 3 :c})))
(throwof "k-truthy-lt" '((comparator (fn [x y] (when (< x y) :lt))) 1 2))
(throwof "k-truthy-eq" '((comparator (fn [x y] (when (< x y) :lt))) 2 2))
(throwof "k-str" '(sort (comparator (fn [x y] (< (count x) (count y)))) ["ccc" "a" "bb"]))
(throwcause "k-arity" '((comparator <) 1))
(throwcause "k-nonfn-pred" '((comparator 5) 1 2))
;; ---- hash-ordered-coll integer vectors ----
(throwof "h-ord-123" '(hash-ordered-coll [1 2 3]))
(throwof "h-ord-empty" '(hash-ordered-coll []))
(throwof "h-ord-single" '(hash-ordered-coll [42]))
(throwof "h-ord-list" '(hash-ordered-coll '(1 2)))
(throwof "h-ord-eq-vec-list" '(= (hash-ordered-coll [1 2]) (hash-ordered-coll '(1 2))))
(throwof "h-ord-eq-hash" '(= (hash [1 2 3]) (hash-ordered-coll [1 2 3])))
(throwof "h-ord-str-els" '(hash-ordered-coll ["a" "b"]))
(throwof "h-ord-nested" '(hash-ordered-coll [[1] #{2} {:a 1}]))
(throwof "h-ord-map" '(hash-ordered-coll {:a 1}))
(throwof "h-ord-neg" '(hash-ordered-coll [-1 0]))
(throwcause "h-ord-nil" '(hash-ordered-coll nil))
(throwcause "h-ord-string" '(hash-ordered-coll "ab"))
(throwcause "h-ord-num" '(hash-ordered-coll 5))
;; ---- hash-unordered-coll integer vectors ----
(throwof "h-unord-set" '(hash-unordered-coll #{1 2 3}))
(throwof "h-unord-empty" '(hash-unordered-coll #{}))
(throwof "h-unord-single" '(hash-unordered-coll #{42}))
(throwof "h-unord-eq-hash" '(= (hash #{1 2 3}) (hash-unordered-coll #{1 2 3})))
(throwof "h-unord-map" '(hash-unordered-coll {:a 1 :b 2}))
(throwof "h-unord-map-eq-hash" '(= (hash {:a 1 :b 2}) (hash-unordered-coll {:a 1 :b 2})))
(throwof "h-unord-vec-order" '(= (hash-unordered-coll [1 2]) (hash-unordered-coll [2 1])))
(throwof "h-unord-vec-val" '(hash-unordered-coll [1 2]))
(throwof "h-entry" '(let [e (first {:a 1})] [(hash e) (hash-ordered-coll [(.getKey ^java.util.Map$Entry e) (.getValue ^java.util.Map$Entry e)]) (= (hash e) (hash-ordered-coll [(.getKey ^java.util.Map$Entry e) (.getValue ^java.util.Map$Entry e)]))]))
(throwcause "h-unord-nil" '(hash-unordered-coll nil))
