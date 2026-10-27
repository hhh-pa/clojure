;; S2 tower-math oracle probes, round 7: quot/rem context reads +
;; ratio conversion under ctx, string+mode combo (oracle: alpha10
;; jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath7.clj > docs/zero-oos-s2-tower/probes/towermath7.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r7-strmode" '(with-precision "precision=3 roundingMode=HALF_UP" :rounding FLOOR 1))
(throwof "r7-quotmix" '(with-precision 3 (quot 1/3 1M)))
(throwof "r7-quotmix0" '(quot 1/3 1M))
(throwof "r7-quotread" '(binding [*math-context* 5] (quot 1M 1M)))
(throwof "r7-remread" '(binding [*math-context* 5] (rem 1M 1M)))
(throwof "r7-remmix" '(with-precision 3 (rem 1/3 1M)))
(throwof "r7-remmix0" '(rem 1/3 1M))
