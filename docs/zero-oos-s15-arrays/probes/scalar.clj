;; S15 array oracle probes: scalar-range edges (oracle: alpha10 jars, issue #59).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s15-arrays/probes/scalar.clj > docs/zero-oos-s15-arrays/probes/scalar.out
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            g (.getMessage ^Throwable c)
            m (if g (-> g
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))
                "nil")]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- A: checked-int paths (direct idx, dims, aset-int val, into, conj) ----
(throwcause "a-aget-huge" '(aget (int-array [1 2]) 3000000000))
(throwcause "a-aget-hugedbl" '(aget (int-array [1 2]) 1e300))
(throwcause "a-tail-hugedbl" '(aget (make-array Integer/TYPE 2 2) 0 1e300))
(throwof "a-tail-nan" '(aget (make-array Integer/TYPE 2 2) 0 Double/NaN))
(throwcause "a-make-huge" '(alength (make-array Integer/TYPE 3000000000)))
(throwcause "a-setint-huge" '(aset-int (int-array [1 2]) 0 3000000000))
(throwcause "a-into-huge" '(into-array Integer/TYPE [3000000000]))
(throwcause "a-vo-huge" '(conj (vector-of :int) 3000000000))
(throwcause "a-setlong-huge" '(aset-long (long-array [1 2]) 0 1e300))
(throwof "a-long-maxdbl" '(long 9.223372036854776E18))
;; ---- B: ctor narrowing (Numbers longValue/intValue — wrap, no check) ----
(throwof "b-long-huge" '(seq (long-array [1e300])))
(throwof "b-int-hugedbl" '(seq (int-array [1e300])))
(throwof "b-short-wrap" '(seq (short-array [40000])))
(throwof "b-int-wrap" '(seq (int-array [3000000000])))
(throwof "b-float-inf" '(seq (float-array [1e300])))
(throwcause "b-bool-nil" '(boolean-array [nil]))
(throwcause "b-double-nil" '(double-array [nil]))
(throwcause "b-int-bool" '(int-array [true]))
(throwcause "b-long-str" '(long-array ["a"]))
(throwof "b-byte-neg" '(seq (byte-array [-1 -129])))
;; ---- D: 2-arity init dispatch (scalar fill vs seq fill) ----
(throwof "d-int-dblinit" '(seq (int-array 2 2.5)))
(throwcause "d-int-strinit" '(int-array 2 "ab"))
(throwof "d-int-nilinit" '(seq (int-array 2 nil)))
(throwof "d-long-dblinit" '(seq (long-array 2 1.5)))
(throwof "d-bool-nilinit" '(seq (boolean-array 2 nil)))
(throwcause "d-int-boolinit" '(int-array 2 true))
;; ---- E: tail-store strictness (port Int reads as JVM Long) ----
(throwcause "e-tail-int-long" '(aset (make-array Integer/TYPE 1 1) 0 0 5))
(throwof "e-tail-long-long" '(let [a (make-array Long/TYPE 1 1)] (aset a 0 0 5) (seq (aget a 0))))
(throwof "e-tail-int-int" '(let [a (make-array Integer/TYPE 1 1)] (aset a 0 0 (int 5)) (seq (aget a 0))))
(throwof "e-direct-int-huge" '(let [a (int-array [1])] [(aset a 0 3000000000) (seq a)]))
(throwof "e-direct-float-huge" '(let [a (float-array [1.0])] [(aset a 0 1e300) (seq a)]))
(throwof "e-to-nil-ident" '(identical? (to-array nil) (to-array nil)))
(throwof "e-short-huge" '(seq (short-array [1e300])))
(throwof "e-byte-huge" '(seq (byte-array [1e300])))
(throwcause "e-char-nil" '(char-array [nil]))
(throwcause "e-aset-nil-multi" '(aset nil 0 0 9))
(throwcause "e-setint-vec" '(aset-int [1 2] 0 5))
(throwcause "e-setint-nilarr" '(aset-int nil 0 5))
;; ---- F: vector-of/array predicate table ----
(throwof "f-vo-ifn" '(ifn? (vector-of :int 1)))
(throwcause "f-vo-invoke" '((vector-of :int 10 20) 1))
(throwof "f-vo-assoc" '(associative? (vector-of :int 1)))
(throwof "f-vo-coll" '(coll? (vector-of :int 1)))
(throwof "f-vo-seq" '(seq? (vector-of :int 1)))
(throwof "f-arr-assoc" '(associative? (int-array [1])))
(throwof "f-arr-ifn" '(ifn? (int-array [1])))
;; ---- G: Vec/vector invoke + runtime-nil (Object-static) ----
(throwcause "g-vo-fkey" '((vector-of :int 10 20) 1.5))
(throwcause "g-vo-skey" '((vector-of :int 10 20) (short 1)))
(throwof "g-vo-bigkey" '((vector-of :int 10 20) 1N))
(throwcause "g-vo-hugekey" '((vector-of :int 10 20) 9999999999999999999999N))
(throwcause "g-vo-strkey" '((vector-of :int 10 20) "a"))
(throwcause "g-vo-nilkey" '((vector-of :int 10 20) nil))
(throwcause "g-vo-oobkey" '((vector-of :int 10 20) 5))
(throwcause "g-vo-negkey" '((vector-of :int 10 20) -1))
(throwcause "g-vo-arity2" '((vector-of :int 10 20) 1 :nf))
(throwcause "g-vec-fkey" '([10 20] 1.5))
(throwcause "g-vec-strkey" '([10 20] "a"))
(throwcause "g-vec-oob" '([10 20] 5))
(throwof "g-vec-arity2" '([10 20] 1 :nf))
(throwcause "g-vec-nilkey" '([10 20] nil))
(throwcause "g-nil-aget" '(let [x (first [])] (aget x 0)))
(throwcause "g-nil-alen" '(let [x (first [])] (alength x)))
(throwcause "g-nil-aset" '(let [x (first [])] (aset x 0 5)))
;; ---- C: vector-of meta printing ----
(throwof "c-vo-meta" '(binding [*print-meta* true] (pr-str (with-meta (vector-of :int 1 2) {:a 1}))))
(throwof "c-vec-meta" '(binding [*print-meta* true] (pr-str (with-meta [1 2] {:a 1}))))
