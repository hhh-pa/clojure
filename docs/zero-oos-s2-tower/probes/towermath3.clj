;; S2 tower-math oracle probes, round 3: MathContext(String) grammar,
;; with-precision edges, quot/rem scale pins (oracle: alpha10 jars,
;; issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath3.clj > docs/zero-oos-s2-tower/probes/towermath3.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r3-sp2" '(with-precision "precision=3  roundingMode=HALF_UP" 1))
(throwof "r3-lead" '(with-precision " precision=3 roundingMode=HALF_UP" 1))
(throwof "r3-trail" '(with-precision "precision=3 roundingMode=HALF_UP " 1))
(throwof "r3-lowkey" '(with-precision "precision=3 roundingmode=HALF_UP" 1))
(throwof "r3-plus" '(with-precision "precision=+3 roundingMode=HALF_UP" (/ 1M 3M)))
(throwof "r3-speq" '(with-precision "precision= 3 roundingMode=HALF_UP" 1))
(throwof "r3-junk" '(with-precision "precision=3 roundingMode=HALF_UP junk" 1))
(throwof "r3-lowmode" '(with-precision "precision=3 roundingMode=half_up" 1))
(throwof "r3-nomode" '(with-precision 10 :rounding))
(throwof "r3-noprec" '(with-precision))
(throwof "r3-quotscale" '(quot 1E+3M 3M))
(throwof "r3-quotmcscale" '(with-precision 5 (quot 1E+3M 3M)))
(throwof "r3-rem" '(with-precision 1 (rem 55.678M 1.1M)))
(throwof "r3-eqdiv" '(with-precision 1 (= 0.3M (/ 1M 3M))))
