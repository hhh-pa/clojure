;; S2 tower-math oracle probes, round 9: abs of BigInteger,
;; numerator of BigInteger, BigInteger huge-conversion texts
;; (oracle: alpha10 jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath9.clj > docs/zero-oos-s2-tower/probes/towermath9.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "r9-absbi" '(abs (biginteger -5)))
(throwof "r9-numbi" '(numerator (biginteger 5)))
(throwof "r9-inthuge" '(int (biginteger 9223372036854775808)))
(throwof "r9-longhuge" '(long (biginteger 18446744073709551616)))
(throwof "r9-shorthuge" '(short (biginteger 100000)))
(throwof "r9-bytehuge" '(byte (biginteger 1000)))
(throwof "r9-charhuge" '(char (biginteger 1114112)))
(throwof "r9-char" '(char (biginteger 65)))
(throwof "r9-floathuge" '(float (biginteger 10000000000000000000000000000000000000000)))
