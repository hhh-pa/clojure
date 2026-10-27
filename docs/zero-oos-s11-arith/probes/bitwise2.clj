;; S11 bitwise follow-up probes: arities, JVM integral types, arg order (issue #56).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s11-arith/probes/bitwise2.clj > docs/zero-oos-s11-arith/probes/bitwise2.out
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
;; ---- A: variadic folds ----
(throwof "va-and0" '(bit-and))
(classof "va-and4" '(bit-and 7 3 1 7))
(throwof "va-andbad3" '(bit-and 7 3 "a"))
(throwof "va-or0" '(bit-or))
(classof "va-or3" '(bit-or 1 2 4))
(classof "va-xor3" '(bit-xor 7 3 1))
(throwof "va-xor0" '(bit-xor))
;; ---- B: fixed 2-arity checks ----
(throwof "fx-andnot3" '(bit-and-not 6 3 1))
(throwof "fx-set3" '(bit-set 0 1 2))
(throwof "fx-clear3" '(bit-clear 7 1 0))
(throwof "fx-flip3" '(bit-flip 7 1 0))
(throwof "fx-shl3" '(bit-shift-left 1 2 3))
(throwof "fx-shr3" '(bit-shift-right 8 1 1))
(throwof "fx-ubs3" '(unsigned-bit-shift-right 8 1 1))
(throwof "fx-test3" '(bit-test 4 2 1))
(throwof "fx-test0" '(bit-test))
;; ---- C: JVM integral types (Integer/Short/Byte/Character) ----
(classof "jt-andint" '(bit-and (int 6) 3))
(classof "jt-andshort" '(bit-and (short 6) 3))
(classof "jt-andbyte" '(bit-and (byte 6) 3))
(classof "jt-andchar" '(bit-and \a 3))
(classof "jt-notint" '(bit-not (int 6)))
(classof "jt-shlint" '(bit-shift-left 1 (int 2)))
(classof "jt-orint" '(bit-or (Integer. 6) (Integer. 3)))
;; ---- D: arg order + class names ----
(throwof "ao-bothbad" '(bit-and "a" "b"))
(throwof "ao-secondbad" '(bit-and 1 "a"))
(throwof "ao-kw" '(bit-and :a 1))
(throwof "ao-vec" '(bit-and [1] 1))
(throwof "ao-bool" '(bit-and true 1))
(throwof "ao-nil2" '(bit-and 1 nil))
(throwof "ao-shnil2" '(bit-shift-left 1 nil))
(throwof "ao-testnil2" '(bit-test 4 nil))
;; ---- E: extra shift values ----
(classof "ev-hugesh" '(bit-shift-left 1 9223372036854775807))
(classof "ev-ubs" '(unsigned-bit-shift-right -8 2))
(classof "ev-shr0" '(bit-shift-right 5 0))
(classof "ev-shl0" '(bit-shift-left 5 0))
