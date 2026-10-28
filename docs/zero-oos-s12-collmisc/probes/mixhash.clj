;; S12 oracle probes: hash-combine/mix-collection-hash (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/mixhash.clj > docs/zero-oos-s12-collmisc/probes/mixhash.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- mix-collection-hash ----
(throwof "mh-mix-1-0" '(mix-collection-hash 1 0))
(throwof "mh-mix-0-0" '(mix-collection-hash 0 0))
(throwof "mh-mix-1-1" '(mix-collection-hash 1 1))
(throwof "mh-mix-1-2" '(mix-collection-hash 1 2))
(throwof "mh-mix-neg" '(mix-collection-hash -1 3))
(throwof "mh-mix-big" '(mix-collection-hash 123456789 10))
(throwof "mh-mix-max" '(mix-collection-hash 9223372036854775807 1))
(throwof "mh-mix-vec" '(= (mix-collection-hash 1 0) (hash [])))
(throwof "mh-mix-set" '(= (mix-collection-hash 0 0) (hash #{})))
(throwcause "mh-mix-arity" '(mix-collection-hash 1))
(throwcause "mh-mix-str" '(mix-collection-hash "a" 1))
(throwof "mh-mix-double" '(mix-collection-hash 1.9 1))
;; ---- hash-combine ----
(throwof "mh-comb-formula" '(= (hash-combine 0 :a) (clojure.lang.Util/hashCombine 0 (clojure.lang.Util/hash :a))))
(throwof "mh-comb-0-nil" '(hash-combine 0 nil))
(throwof "mh-comb-0-0" '(hash-combine 0 0))
(throwof "mh-comb-0-1" '(hash-combine 0 1))
(throwof "mh-comb-1-1" '(hash-combine 1 1))
(throwof "mh-comb-0-str" '(hash-combine 0 "a"))
(throwof "mh-comb-0-kw" '(hash-combine 0 :a))
(throwof "mh-comb-5-kw" '(hash-combine 5 :a))
(throwof "mh-comb-0-vec" '(hash-combine 0 [1 2]))
(throwof "mh-comb-0-map" '(hash-combine 0 {:a 1}))
(throwof "mh-comb-0-set" '(hash-combine 0 #{1}))
(throwof "mh-comb-neg" '(hash-combine -1 "ab"))
(throwof "mh-comb-big" '(hash-combine 12345 nil))
(throwof "mh-comb-double" '(hash-combine 0 1.5))
(throwof "mh-comb-ratio" '(hash-combine 0 1/2))
(throwof "mh-comb-char" '(hash-combine 0 \a))
(throwof "mh-comb-bool" '(hash-combine 0 true))
(throwof "mh-comb-negzero" '(hash-combine 0 -0.0))
(throwof "mh-comb-bigint" '(hash-combine 0 9223372036854775807))
(throwof "mh-tower-hash" '[(hash 5N) (hash 1/2) (hash 1.5M)])
(throwof "mh-tower-util" '[(clojure.lang.Util/hash 5N) (clojure.lang.Util/hash 1/2) (clojure.lang.Util/hash 1.5M)])
(throwof "mh-comb-bign" '(hash-combine 0 5N))
(throwof "mh-comb-dec" '(hash-combine 0 1.5M))
(throwof "mh-util-spot" '[(clojure.lang.Util/hash [1 2]) (clojure.lang.Util/hash :a) (clojure.lang.Util/hash {:a 1}) (clojure.lang.Util/hash nil)])
(throwof "mh-comb-list" '(hash-combine 0 (quote (1 2))))
(throwof "mh-tower-agree" '[(= (hash 5) (hash 5N)) (= 5 5N)])
(throwcause "mh-comb-seed-str" '(hash-combine "a" 1))
(throwof "mh-comb-bigseed" '(hash-combine 1099511627776 1))
(throwcause "mh-comb-arity" '(hash-combine 1))
