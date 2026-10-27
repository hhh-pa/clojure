;; S11 negate-context follow-up probes (oracle: alpha10 jars, issue #56).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s11-arith/probes/s11negmc.clj > docs/zero-oos-s11-arith/probes/s11negmc.out
(defn classof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v) " <" (.getName (class v)) ">")))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(classof "n-negmc" '(binding [*math-context* (java.math.MathContext. 2)] (-' 0.125M)))
(classof "n-unegmc" '(binding [*math-context* (java.math.MathContext. 2)] (unchecked-negate 0.125M)))
(classof "n-negmc10" '(binding [*math-context* (java.math.MathContext. 10)] (-' 5M)))
(classof "n-unegmc10" '(binding [*math-context* (java.math.MathContext. 10)] (unchecked-negate 5M)))
