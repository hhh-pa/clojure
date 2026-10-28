;; S2 tower-math oracle probes, round 4: MathContext(String) int-parse
;; edges + boolean precision (oracle: alpha10 jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath4.clj > docs/zero-oos-s2-tower/probes/towermath4.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r4-bigstr" '(with-precision "precision=99999999999999999999 roundingMode=HALF_UP" 1))
(throwof "r4-emptystr" '(with-precision "precision= roundingMode=HALF_UP" 1))
(throwof "r4-boolprec" '(with-precision true 1))
