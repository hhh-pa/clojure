;; S2 tower-math oracle probes, round 10: division edges under
;; ctx (zero divisor text, negative-scale long division,
;; exact-short paths, division carry, sticky-half tie)
;; (oracle: alpha10 jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath10.clj > docs/zero-oos-s2-tower/probes/towermath10.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r10-divzero" '(with-precision 3 (/ 1M 3M 0M)))
(throwof "r10-divzero2" '(with-precision 3 (/ 1M 0M)))
(throwof "r10-negscale" '(with-precision 3 (/ 12345M 1M)))
(throwof "r10-zerobigdec" '(with-precision 0 (bigdec 1/2)))
(throwof "r10-exactbigdec" '(with-precision 3 (bigdec 1/2)))
(throwof "r10-divcarry" '(with-precision 1 (/ 99M 10M)))
(throwof "r10-stickytie" '(with-precision 2 :rounding HALF_EVEN (/ 35001M 200000M)))
