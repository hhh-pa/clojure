;; F-read-opts oracle probes (oracle: alpha10 jars, issue #40).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i40-readopts/probes/read-opts.clj > docs/i40-readopts/probes/read-opts.out
(require 'clojure.string)
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; ---- 1-arity control (incl. the OUTER error shape: wrapper or bare?) ----
(throwof "rs-opts-none" '(read-string "1 2"))
(throwof "rs-opts-none-eof" '(read-string ""))
(throwof "rs-opts-none-trunc" '(read-string "(a"))
(throwof "rs-opts-none-cond" '(read-string "#?(:clj 1 :default 2)"))
;; ---- empty opts ----
(throwof "rs-opts-empty" '(read-string {} "1 2"))
(throwof "rs-opts-empty-eof" '(read-string {} ""))
;; ---- :read-cond ----
(throwof "rs-opts-cond-allow" '(read-string {:read-cond :allow} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-deny" '(read-string {:read-cond false} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-absent" '(read-string {:eof :x} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-preserve" '(read-string {:read-cond :preserve} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-true" '(read-string {:read-cond true} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-splice-top" '(read-string {:read-cond :allow} "#?@(:clj [1 2])"))
(throwof "rs-opts-cond-splice-nested" '(read-string {:read-cond :allow} "(1 #?@(:clj [2 3]))"))
;; ---- :features ----
(throwof "rs-opts-features" '(read-string {:read-cond :allow :features #{:foo}} "#?(:foo 1 :bar 2 :default 3)"))
(throwof "rs-opts-features-unknown" '(read-string {:read-cond :allow :features #{:nope}} "#?(:foo 1 :default 2)"))
(throwof "rs-opts-features-nil" '(read-string {:read-cond :allow :features nil} "#?(:foo 1 :default 2)"))
(throwof "rs-opts-features-vec" '(read-string {:read-cond :allow :features [:foo]} "1"))
(throwof "rs-opts-features-set-nonkw" '(read-string {:read-cond :allow :features #{1}} "1"))
;; ---- :eof ----
(throwof "rs-opts-eof" '(read-string {:eof :x} ""))
(throwof "rs-opts-eof-blank" '(read-string {:eof :x} "  "))
(throwof "rs-opts-eof-comment" '(read-string {:eof :x} "; c"))
(throwof "rs-opts-eof-nil" '(read-string {:eof nil} ""))
(throwof "rs-opts-eof-false" '(read-string {:eof false} ""))
(throwof "rs-opts-eof-trunc" '(read-string {:eof :x} "(a"))
(throwof "rs-opts-eof-trunc-str" '(read-string {:eof :x} "\"abc"))
(throwof "rs-opts-eof-value" '(read-string {:eof :x} "1"))
(throwof "rs-opts-oneform" '(read-string {:eof :x} "1 2"))
;; ---- unknown key ----
(throwof "rs-opts-unknown-key" '(read-string {:bogus 1} "1"))
(throwof "rs-opts-unknown-key-eof" '(read-string {:bogus 1} ""))
;; ---- nil / non-map opts ----
(throwof "rs-opts-nil" '(read-string nil "1"))
(throwof "rs-opts-nil-eof" '(read-string nil ""))
(throwof "rs-opts-nonmap" '(read-string :foo "1"))
(throwof "rs-opts-nonmap-vec" '(read-string [1] "1"))
(throwof "rs-opts-mapentry" '(read-string (find {:a 1} :a) "1"))
;; ---- bad :read-cond ----
(throwof "rs-opts-badcond" '(read-string {:read-cond :bogus} "#?(:clj 1 :default 2)"))
(throwof "rs-opts-cond-nil" '(read-string {:read-cond nil} "#?(:clj 1 :default 2)"))
;; ---- string operand (+ validation order vs opts) ----
(throwof "rs-opts-nilstr" '(read-string {} nil))
(throwof "rs-opts-numstr" '(read-string {} 1))
(throwof "rs-opts-order" '(read-string :foo 1))
(throwof "rs-opts-order-nil" '(read-string :foo nil))
;; ---- arity ----
(throwof "rs-opts-arity0" '(read-string))
(throwof "rs-opts-arity3" '(read-string {} "1" :x))
