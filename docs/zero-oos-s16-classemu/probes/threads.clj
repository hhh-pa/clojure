;; S16 oracle probes: .. and memfn (oracle: alpha10 jars, issue #60).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/threads.clj > docs/zero-oos-s16-classemu/probes/threads.out
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
;; ---- A: .. expansion ----
(throwof "x-doc" '(macroexpand-1 '(.. System (getProperties) (get "os.name"))))
(throwof "x-single" '(macroexpand-1 '(.. "a" (toString))))
(throwof "x-bare" '(macroexpand-1 '(.. "a" toString)))
(throwof "x-three" '(macroexpand-1 '(.. x a b c)))
(throwcause "x-arity1" '(macroexpand-1 '(.. x)))
;; ---- B: .. behavior (instance surface) ----
(throwof "d-tostring" '(.. "a" (toString)))
(throwof "d-long-tostring" '(.. 1 (toString)))
(throwof "d-vec-tostring" '(.. [1 2] (toString)))
(throwof "d-map-tostring" '(.. {:a 1} (toString)))
(throwof "d-keyword-tostring" '(.. :k (toString)))
(throwcause "d-getclass" '(.. "a" (getClass)))
(throwof "d-hashcode-str" '(.. "a" (hashCode)))
(throwof "d-hashcode-long" '(.. 1 (hashCode)))
(throwof "d-hashcode-bool" '(.. true (hashCode)))
(throwof "d-equals-t" '(.. "a" (equals "a")))
(throwof "d-equals-f" '(.. "a" (equals "b")))
(throwof "d-equals-numtower" '(.. 1 (equals 1.0)))
(throwof "d-chain" '(.. "abc" (toString) (toString)))
(throwcause "d-nil" '(.. nil (toString)))
(throwcause "d-missing0" '(.. "a" (noSuchMethod)))
(throwcause "d-missing1" '(.. "a" (noSuchMethod 1 2)))
(throwcause "d-bare-missing" '(.. "a" noSuchField))
(throwof "d-length" '(.. "abc" (length)))
(throwof "d-substring" '(.. "abc" (substring 1 2)))
;; ---- C: memfn expansion ----
(throwcause "m-plain" '(macroexpand-1 '(memfn toString)))
(throwcause "m-args" '(macroexpand-1 '(memfn substring 0 1)))
(throwcause "m-arity0" '(macroexpand-1 '(memfn)))
;; ---- D: memfn behavior ----
(throwof "f-tostring" '((memfn toString) 5))
(throwof "f-map" '(map (memfn toString) [1 2]))
(throwof "f-length" '(map (memfn length) ["a" "bb"]))
(throwof "f-substring" '((memfn substring beg end) "abc" 0 1))
(throwcause "f-missing" '((memfn noSuchMethod) "a"))
