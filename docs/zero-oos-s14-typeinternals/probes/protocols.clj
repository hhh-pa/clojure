;; S14 type-internals oracle probes: protocol introspection (oracle: alpha10 jars, issue #58).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s14-typeinternals/probes/protocols.clj > docs/zero-oos-s14-typeinternals/probes/protocols.out
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
(defn show [label form]
  (println (str label " " (normhex (pr-str (eval form))))))
;; ---- setup ----
(defprotocol RP (rm [x]) (rn [x y]))
(extend-protocol RP String (rm [x] :str-m) (rn [x y] :str-n))
(extend-protocol RP Long (rm [x] :long-m))
(extend-protocol RP Number (rm [x] :number-m) (rn [x y] :number-n))
(extend-protocol RP clojure.lang.IPersistentMap (rm [x] :map-m))
(extend-protocol RP Object (rm [x] :obj-m))
(deftype RI [x] RP (rm [this] :inline-m))
(defprotocol RQ (qm [x]))
;; ---- A: find-protocol-impl ----
(show "a-exact-keys" '(sort (keys (find-protocol-impl RP "hi"))))
(show "a-exact-call" '((find-protocol-method RP :rm "hi") "hi"))
(show "a-long-call" '((find-protocol-method RP :rm 5) 5))
(show "a-long-missing" '(find-protocol-method RP :rn 5))
(show "a-super-call" '((find-protocol-method RP :rm (short 5)) (short 5)))
(show "a-iface-call" '((find-protocol-method RP :rm {:a 1}) {:a 1}))
(show "a-object-call" '((find-protocol-method RP :rm :kw) :kw))
(show "a-inline" '(let [i (RI. 1)] (identical? i (find-protocol-impl RP i))))
(show "a-miss" '(find-protocol-impl RQ "hi"))
(show "a-nil" '(find-protocol-impl RP nil))
(throwcause "a-nonproto" '(find-protocol-impl 5 "hi"))
(throwcause "a-arity" '((var find-protocol-impl) RP))
;; ---- B: find-protocol-method ----
(show "b-inline-nil" '(find-protocol-method RP :rm (RI. 1)))
(show "b-str-key" '(find-protocol-method RP "rm" "hi"))
(show "b-missing-k" '(find-protocol-method RP :nope "hi"))
(throwcause "b-nonproto" '(find-protocol-method 5 :rm "hi"))
(throwcause "b-arity" '((var find-protocol-method) RP :rm))
;; ---- C: extenders ----
(show "c-list" '(sort (map str (extenders RP))))
(show "c-empty" '(extenders RQ))
(show "c-nonproto" '(extenders 5))
(show "c-nil" '(extenders nil))
(throwcause "c-arity" '((var extenders)))
;; ---- D: -cache-protocol-fn ----
(defprotocol RC (rcm [x]))
(extend-protocol RC String (rcm [x] :cached))
(deftype RD [] RC (rcm [this] :d))
(show "d-hit" '((-cache-protocol-fn rcm "hi" (:on-interface RC) :INTERF) "hi"))
(show "d-interf" '(-cache-protocol-fn rcm (RD.) (:on-interface RC) :INTERF))
(throwcause "d-miss" '(-cache-protocol-fn rcm 5 (:on-interface RC) :INTERF))
(throwcause "d-nil" '(-cache-protocol-fn rcm nil (:on-interface RC) :INTERF))
(throwcause "d-badpf" '(-cache-protocol-fn (fn [x] x) "hi" (:on-interface RC) :INTERF))
(throwcause "d-arity" '((var -cache-protocol-fn) rcm "hi"))
;; ---- E: -reset-methods ----
(show "e-ok" '(do (-reset-methods RC) [(rcm "hi") (-reset-methods RC)]))
(show "e-nonproto" '(-reset-methods 5))
(show "e-nil" '(-reset-methods nil))
(throwcause "e-arity" '((var -reset-methods)))
;; ---- F: nil extension ----
(extend-protocol RP nil (rm [x] :nil-m))
(show "f-nil-call" '((find-protocol-method RP :rm nil) nil))
;; ---- G: hint-check order ----
(throwcause "g-pf-c" '(-cache-protocol-fn 5 "hi" 6 :INTERF))
(throwcause "g-c-interf" '(-cache-protocol-fn rcm "hi" 6 7))
