;; S2 tower-math oracle probes, round 5: minus self-subtraction,
;; zero results under ctx, garbage bindings (oracle: alpha10 jars,
;; issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath5.clj > docs/zero-oos-s2-tower/probes/towermath5.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r5-selfsub" '(with-precision 3 (- 5.678M 5.678M)))
(throwof "r5-mixsub" '(with-precision 3 (- 1M 1/3)))
(throwof "r5-zdiv" '(with-precision 3 (/ 0M 3M)))
(throwof "r5-zdiv2" '(with-precision 3 (/ 0.0M 3M)))
(throwof "r5-zadd" '(with-precision 1 (+ 0.0M 0.0M)))
(throwof "r5-garbage" '(binding [*math-context* 5] (+ 1M 1M)))
(throwof "r5-garbage-int" '(binding [*math-context* 5] (+ 1 2)))
(throwof "r5-garbage-str" '(binding [*math-context* "x"] (/ 1M 3M)))
