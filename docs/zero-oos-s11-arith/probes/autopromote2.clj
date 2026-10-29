;; S11 auto-promote follow-up probes: 1-arity cast edges (issue #56).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s11-arith/probes/autopromote2.clj > docs/zero-oos-s11-arith/probes/autopromote2.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn classof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v) " <" (.getName (class v)) ">")))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(classof "a2-mulnil" '(*' nil))
(classof "a2-addchar" '(+' \a))
(classof "a2-mulchar" '(*' \a))
(throwof "a2-addtrue" '(+' true))
(throwof "a2-multrue" '(*' true))
(classof "a2-addrat" '(+' 1/2))
(classof "a2-muldec" '(*' 5M))
(classof "a2-subchar" '(-' \a))
