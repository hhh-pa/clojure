;; S2 tower-math oracle probes, round 8: BigInteger membership +
;; arith absorption, quot/rem scales, direct str+mode ctor
;; (oracle: alpha10 jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath8.clj > docs/zero-oos-s2-tower/probes/towermath8.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r8-intp" '(int? (biginteger 5)))
(throwof "r8-integerp" '(integer? (biginteger 5)))
(throwof "r8-numberp" '(number? (biginteger 5)))
(throwof "r8-rationalp" '(rational? (biginteger 5)))
(throwof "r8-ratiop" '(ratio? (biginteger 5)))
(throwof "r8-decp" '(decimal? (biginteger 5)))
(throwof "r8-floatp" '(float? (biginteger 5)))
(throwof "r8-hash" '(= (hash 5N) (hash (biginteger 5))))
(throwof "r8-hash2" '(= (hash 5) (hash (biginteger 5))))
(throwof "r8-bigrd" '(bigint (biginteger 5)))
(throwof "r8-bigdrd" '(bigdec (biginteger 5)))
(throwof "r8-numrd" '(num (biginteger 5)))
(throwof "r8-ratrd" '(rationalize (biginteger 5)))
(throwof "r8-add" '(+ (biginteger 5) 1))
(throwof "r8-add2" '(+ (biginteger 5) 2N))
(throwof "r8-mul" '(* (biginteger 5) 2))
(throwof "r8-div" '(/ (biginteger 6) 2))
(throwof "r8-div2" '(/ (biginteger 1) 2))
(throwof "r8-inc" '(inc (biginteger 5)))
(throwof "r8-quot" '(quot (biginteger 7) 2))
(throwof "r8-lt" '(< (biginteger 5) 6))
(throwof "r8-typeop" '(type (+ (biginteger 5) 1)))
(throwof "r8-typemin" '(type (min (biginteger 5) 6)))
(throwof "r8-int" '(int (biginteger 5)))
(throwof "r8-long" '(long (biginteger 5)))
(throwof "r8-double" '(double (biginteger 5)))
(throwof "r8-strmodedirect" '(java.math.MathContext. "precision=3 roundingMode=HALF_UP" java.math.RoundingMode/FLOOR))
(throwof "r8-quotscale" '(with-precision 5 (quot 5.678M 2.5M)))
(throwof "r8-quotscale0" '(quot 5.678M 2.5M))
(throwof "r8-quotz" '(quot 0.333M 1M))
(throwof "r8-remz" '(rem 0.333M 1M))
