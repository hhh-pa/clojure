;; S16 oracle probes: identical? (oracle: alpha10 jars, issue #60).
;; Run from repo root (/home/hhu/workspace/clojure-s16):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/identical.clj > docs/zero-oos-s16-classemu/probes/identical.out
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
                  (clojure.string/replace #"__\d+" "__N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- A: per-kind identity ----
(throwof "d-nil" '(identical? nil nil))
(throwof "d-bool" '(identical? true true))
(throwof "d-int-small" '(identical? 1 1))
(throwof "d-int-big" '(identical? 10000000000 10000000000))
(throwof "d-int-neg" '(identical? -5 -5))
(throwof "d-double" '(identical? 1.5 1.5))
(throwof "d-char-cached" '(identical? \a \a))
(throwof "d-char-uncached" '(identical? \€ \€))
(throwof "d-bigint" '(identical? (bigint 5) (bigint 5)))
(throwof "d-bigdec" '(identical? 1.5M 1.5M))
(throwof "d-ratio" '(identical? 1/3 1/3))
(throwof "d-str-lit" '(identical? "a" "a"))
(throwof "d-str-built" '(identical? (str \a) (str \a)))
(throwof "d-kw" '(identical? :a :a))
(throwof "d-sym" '(identical? 'a 'a))
(throwof "d-vec-same" '(let [v [1]] (identical? v v)))
(throwof "d-vec-eq" '(identical? [1] [1]))
(throwof "d-map-eq" '(identical? {:a 1} {:a 1}))
(throwof "d-list-eq" '(identical? '(1) '(1)))
(throwof "d-set-eq" '(identical? #{1} #{1}))
(throwof "d-fn-same" '(let [f (fn [x] x)] (identical? f f)))
(throwof "d-fn-distinct" '(identical? (fn [x] x) (fn [x] x)))
(throwof "d-atom-same" '(let [a (atom 1)] (identical? a a)))
(throwof "d-atom-distinct" '(identical? (atom 1) (atom 1)))
(throwof "d-var" '(identical? #'map #'map))
(throwof "d-class" '(identical? String String))
(throwof "d-regex" '(identical? #"x" #"x"))
(throwof "d-lazy-same" '(let [s (map inc [1])] (identical? s s)))
;; ---- B: inline ----
(throwof "d-macroexpand" '(macroexpand '(identical? a b)))
(throwof "d-inline-arities" '(:inline-arities (meta #'identical?)))
(throwof "d-has-inline" '(some? (:inline (meta #'identical?))))
;; ---- C: arity / higher-order ----
(throwcause "d-arity0" '(identical?))
(throwcause "d-arity1" '(identical? 1))
(throwcause "d-arity3" '(identical? 1 2 3))
(throwof "d-apply-t" '(apply identical? [1 1]))
(throwof "d-apply-f" '(apply identical? [[1] [1]]))
(throwcause "d-apply-arity" '(apply identical? [1]))
(throwof "d-ho-str" '(map identical? ["a"] ["a"]))
(throwof "d-ho-vec" '(map identical? [[1]] [[1]]))
(throwof "d-eval-order" '(let [a (atom [])] [(identical? (do (swap! a conj :x) 1) (do (swap! a conj :y) 1)) @a]))
;; ---- D: = vs identical? ----
(throwof "d-eq-vs-ident-vec" '[(= [1] [1]) (identical? [1] [1])])
(throwof "d-eq-vs-ident-big" '[(= 10000000000 10000000000) (identical? 10000000000 10000000000)])
(throwof "d-eq-vs-ident-str" '[(= (str \a) (str \a)) (identical? (str \a) (str \a))])
