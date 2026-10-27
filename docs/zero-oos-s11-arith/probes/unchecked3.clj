;; S11 follow-up probes: -int coercion edges, ratio wrap paths, char edges (issue #56).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s11-arith/probes/unchecked3.clj > docs/zero-oos-s11-arith/probes/unchecked3.out
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
;; ---- A: -int coercion edges ----
(classof "xi-char" '(unchecked-add-int \a 1))
(classof "xi-nan" '(unchecked-add-int ##NaN 1))
(classof "xi-inf" '(unchecked-add-int ##Inf 1))
(classof "xi-neginf" '(unchecked-add-int ##-Inf 1))
(classof "xi-hugedbl" '(unchecked-add-int 1.0E20 1))
(classof "xi-bigwrap" '(unchecked-add-int 4294967296N 1))
(classof "xi-rat" '(unchecked-add-int 7/2 1))
(classof "xi-hugerat" '(unchecked-add-int 8589934595/2 1))
(classof "xi-dec" '(unchecked-add-int 7.5M 1))
(classof "xi-hugedec" '(unchecked-add-int 1.0E20M 1))
(throwof "xi-ar1" '(unchecked-add-int 1))
(classof "xd-char" '(unchecked-divide-int \a 2))
(classof "xd-nan" '(unchecked-divide-int ##NaN 2))
;; ---- B: ratio wrap paths for truncating coercions ----
(classof "xr-intwrap" '(unchecked-int 8589934595/2))
(classof "xr-intneg" '(unchecked-int -8589934595/2))
(classof "xr-inthuge" '(unchecked-int 100000000000000000000/1))
(classof "xr-longwrap" '(unchecked-long 36893488147419103233/2))
(classof "xr-longneg" '(unchecked-long -36893488147419103233/2))
(classof "xr-short" '(unchecked-short 100000000000000000000/1))
(classof "xr-byte" '(unchecked-byte 100000000000000000000/1))
(classof "xr-char" '(unchecked-char 4294967297/2))
;; ---- C: char/bool edges ----
(classof "xc-fchar" '(unchecked-float \a))
(classof "xc-dchar" '(unchecked-double \a))
(classof "xc-sinf" '(unchecked-short ##Inf))
(classof "xc-binf" '(unchecked-byte ##Inf))
(classof "xc-sneginf" '(unchecked-short ##-Inf))
(classof "xc-charhuge" '(unchecked-char 1.0E20))
(classof "xc-charneg" '(unchecked-char -5))
(classof "xc-bi" '(unchecked-int (java.math.BigInteger. "4294967296")))
(classof "xc-addbi" '(unchecked-add (java.math.BigInteger. "1") 2))
(throwof "xc-intbool" '(unchecked-int true))
;; ---- D: math-context for unchecked long ops ----
(classof "xm-addu" '(binding [*math-context* (java.math.MathContext. 2)] (unchecked-add 1/3 0M)))
(classof "xm-incu" '(binding [*math-context* (java.math.MathContext. 2)] (unchecked-inc 1/3)))
(classof "xm-mulu" '(binding [*math-context* (java.math.MathContext. 2)] (unchecked-multiply 1/3 2M)))
