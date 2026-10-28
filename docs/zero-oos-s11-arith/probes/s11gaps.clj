;; S11 final gap probes (oracle: alpha10 jars, issue #56).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s11-arith/probes/s11gaps.clj > docs/zero-oos-s11-arith/probes/s11gaps.out
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
;; ---- A: ratio short/byte paths (wrap vs double-saturate) ----
(classof "g-shortrat" '(unchecked-short 100000000000000000000/3))
(classof "g-shortratneg" '(unchecked-short -100000000000000000000/3))
(classof "g-byterat" '(unchecked-byte 100000000000000000000/3))
;; ---- B: -int coercion past i64 (wrap vs long-range check) ----
(classof "g-intbighuge" '(unchecked-add-int 18446744073709551616N 1))
(classof "g-intbighugneg" '(unchecked-add-int -18446744073709551617N 1))
(classof "g-intrathuge" '(unchecked-add-int 55340232221128654849/3 1))
(classof "g-intdechuge" '(unchecked-add-int 1.0E19M 1))
;; ---- C: autopromote 1-arity nil (throwof, no harness class call) ----
(throwof "g-mulnil" '(*' nil))
;; ---- D: bit-and-not arity + fold direction ----
(throwof "g-andnot0" '(bit-and-not))
(throwof "g-andnot1" '(bit-and-not 6))
(classof "g-andnot4" '(bit-and-not 6 3 1 2))
(throwof "g-or1" '(bit-or 1))
(throwof "g-xor1" '(bit-xor 5))
;; ---- E: bitwise offender order ----
(throwof "g-order" '(bit-and "a" 1.0))
(throwof "g-order2" '(bit-and 1.0 "a"))
