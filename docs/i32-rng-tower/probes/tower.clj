;; S-rng-tower oracle probes: tower (oracle: alpha10 jars, issue #32).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i32-rng-tower/probes/tower.clj > docs/i32-rng-tower/probes/tower.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; bigint coercions
(throwof "bigint-int" '(bigint 5))
(throwof "bigint-dbl" '(bigint 5.5))
(throwof "bigint-negdbl" '(bigint -5.9))
(throwof "bigint-str" '(bigint "42"))
(throwof "bigint-nil" '(bigint nil))
(throwof "bigint-ratio" '(bigint 1/2))
(throwof "bigint-bigdec" '(bigint 5.9M))
(throwof "bigint-id" '(bigint 5N))
(throwof "bigint-bool" '(bigint true))
(throwof "bigint-badstr" '(bigint "x"))
(throwof "bigint-hexstr" '(bigint "0x10"))
(throwof "bigint-nan" '(bigint Double/NaN))
(throwof "bigint-bign" '(bigint 1e100))
(throwof "bigint-bigstr" '(bigint "123456789012345678901234567890"))
;; bigdec coercions
(throwof "bigdec-int" '(bigdec 5))
(throwof "bigdec-dbl" '(bigdec 5.1))
(throwof "bigdec-neg" '(bigdec -5))
(throwof "bigdec-str" '(bigdec "5"))
(throwof "bigdec-ratio" '(bigdec 1/2))
(throwof "bigdec-nil" '(bigdec nil))
(throwof "bigdec-id" '(bigdec 5.0M))
(throwof "bigdec-bool" '(bigdec true))
(throwof "bigdec-badstr" '(bigdec "x"))
(throwof "bigdec-exp" '(bigdec "1e3"))
(throwof "bigdec-exp2" '(bigdec "1.5e-3"))
(throwof "bigdec-forms" '[(bigdec ".5") (bigdec "5.") (bigdec "+5")])
(throwof "bigdec-third" '(bigdec 1/3))
(throwof "bigdec-nan" '(bigdec Double/NaN))
(throwof "bigdec-inf" '(bigdec Double/POSITIVE_INFINITY))
;; num coercions
(throwof "num-int" '(num 5))
(throwof "num-dbl" '(num 5.5))
(throwof "num-str" '(num "5"))
(throwof "num-tower" '[(num 5N) (num 5M) (num 1/2)])
(throwof "num-nil" '(num nil))
(throwof "num-bool" '(num true))
;; rationalize coercions
(throwof "rat-half" '(rationalize 0.5))
(throwof "rat-int" '(rationalize 5))
(throwof "rat-big" '[(rationalize 5N) (rationalize 5M) (rationalize 1/2)])
(throwof "rat-51M" '(rationalize 5.1M))
(throwof "rat-50M" '(rationalize 5.0M))
(throwof "rat-str" '(rationalize "0.5"))
(throwof "rat-25" '(rationalize 2.5))
(throwof "rat-neg" '(rationalize -0.5))
(throwof "rat-neg25" '(rationalize -2.5))
(throwof "rat-00" '(rationalize 0.0))
(throwof "rat-1e100" '(rationalize 1e100))
(throwof "rat-nan" '(rationalize Double/NaN))
(throwof "rat-nil" '(rationalize nil))
(throwof "rat-bool" '(rationalize true))
;; reader literals
(throwof "read-ratio" '(read-string "1/2"))
(throwof "read-42" '(read-string "4/2"))
(throwof "read-64" '(read-string "6/4"))
(throwof "read-21" '(read-string "2/1"))
(throwof "read-05" '(read-string "0/5"))
(throwof "read-10" '(read-string "1/0"))
(throwof "read-0N" '(read-string "0N"))
(throwof "read-hexN" '(read-string "0xFFN"))
(throwof "read-hex" '(read-string "0xFF"))
(throwof "read-bighex" '(read-string "0xFFFFFFFFFFFFFFFFFF"))
(throwof "read-negrat" '(read-string "-1/2"))
(throwof "read-tower" '[(read-string "4/2") (read-string "5M") (read-string "5N")])
;; equality / hashing behavior
(throwof "eq-tower" '[(= 5N 5) (= 5M 5.0) (= 1/2 1/2) (= 5.0M 5.00M) (== 1/2 0.5) (= 5N 5N)])
(throwof "eq-cross" '[(= 5M 5N) (= 5N 5M) (= 1/2 5) (= 1/2 0.5) (= 5M 1/2) (= 5N 5.0) (= 0N 0)])
(throwof "eq-num" '[(== 5N 5) (== 5M 5.0) (== 5.0M 5.00M)])
(throwof "eq-big" '(= (bigint "123456789012345678901234567890") (bigint "123456789012345678901234567890")))
(throwof "map-tower" '[({5.0M :a} 5.00M) ({5N :a} 5) ({1/2 :a} 1/2) (contains? #{5N} 5) (get {5N :a} 5)])
;; predicates
(throwof "pred-tower" '[(integer? 5N) (integer? 5M) (integer? 1/2) (number? 5N) (number? 5M) (number? 1/2) (int? 5N)])
(throwof "pred-sign" '[(zero? 0N) (pos? 5M) (neg? 1/2) (zero? 0M) (neg? -1/2)])
;; compare / sort
(throwof "sort-mixed" '(sort [5M 1/2 5N 1 0.5 2]))
(throwof "cmp-tower" '[(compare 1/2 0.5) (compare 0.5 1/2) (compare 5M 5.0) (compare 0.1 1/10) (compare 5N 5.0) (compare 5M 5N)])
(throwof "minmax-tower" '[(min 1/2 1) (max 5N 3) (max 0.5 1/2) (min 0.5 1/2)])
;; promotion lattice
(throwof "promo-add" '[(+ 1/2 5) (+ 1/2 5N) (+ 1/2 5M) (+ 5N 5M) (+ 5N 5) (+ 5M 5) (+ 5M 0.5) (+ 1/2 0.5)])
(throwof "promo-mul" '[(* 1/2 5N) (- 5M 5) (/ 5N 2) (/ 1 2)])
(throwof "promo-inc" '[(inc 5M) (inc 1/2) (dec 1/2) (dec 5N) (inc 5N) (+ 1N 2N)])
(throwof "promo-types" '(map type [(+ 1/2 5N) (+ 5N 5M) (+ 5M 0.5) (+ 1/2 0.5) (/ 5N 2)]))
(throwof "promo-conv" '[(long 5M) (long 1/2) (long 5.9M) (int 1/2) (double 1/2) (double 5N) (float 1/2) (long 5N) (float 5N)])
(throwof "promo-cmp" '[(== 5N 5) (== 5M 5.0) (== 5.0M 5.00M) (< 1/2 1) (compare 5M 5N) (zero? 0M) (neg? -1/2)])
(throwof "promo-quot" '[(quot 7N 2N) (rem 7N 2N) (mod 7N 2N) (quot 7N 2) (quot 7 2N) (quot 5M 2) (quot 1/2 1)])
(throwof "promo-div0" '[(try (quot 5M 0M) (catch ArithmeticException e "div0")) (try (quot 5 0N) (catch ArithmeticException e "div0"))])
(throwof "promo-remmod" '[(rem 5M 2) (mod 5M 2) (rem 1/2 1) (mod 7N -2N) (mod -7N 2N) (rem 7N -2N)])
(throwof "promo-nonterm" '(try (bigdec 1/3) (catch ArithmeticException e "nonterm")))
;; printing (pr adds N/M, str is plain)
(throwof "str-tower" '[(str 5N) (str 5M) (str 1/2)])
(throwof "str-dec" '[(str 5.0M) (str 5.00M) (str 1E+3M)])
(throwof "pr-dec" '[(pr-str 5.0M) (pr-str 5.00M)])
;; checked (non-wrapping) tower conversions
(throwof "conv-i1" '(try (int (bigint "4294967296")) (catch ArithmeticException e "overflow")))
(throwof "conv-i2" '[(int (bigint "-1")) (int 7/2) (int 5.9M)])
(throwof "conv-l1" '(try (long (bigint "18446744073709551616")) (catch IllegalArgumentException e (.getMessage e))))
(throwof "conv-l3" '[(long (bigint "-1")) (long -7/2) (long 5.9M)])
(throwof "index-tower" '[(drop 2N [1 2 3]) (take 2N [1 2 3]) (drop 1/2 [1 2 3]) (nth [1 2 3] 1N)])
(throwof "index-big" '(try (doall (drop (bigint "99999999999999999999") [1])) (catch ArithmeticException e "overflow")))
(throwof "float-big" '(try (float (bigint "1267650600228229401496703205376")) (catch IllegalArgumentException e (.getMessage e))))
;; counts through seq machinery
(throwof "seq-tower" '[(take 2N [1 2 3]) (repeat 2N "x") (partition 3N [1 2 3]) (partition 3.0 [1 2 3]) (range 0 5N) (range 0 3/2 1/2)])
(throwof "rat-01" '(rationalize 0.1))
;; nth/subvec index semantics over tower
(throwof "nth-tower" '[(nth [1 2 3] 1N) (nth [1 2 3] 1/2) (nth [1 2 3] 5.9M) (nth [1 2 3] 1.9) (nth [1] 2N -1)])
(throwof "nth-big" '(try (nth [1] (bigint "99999999999999999999")) (catch IllegalArgumentException e (.getMessage e))))
(throwof "nth-bigrat" '(try (nth [1] (/ (bigint "99999999999999999999") 1)) (catch IllegalArgumentException e (.getMessage e))))
(throwof "subvec-tower" '[(subvec [1 2 3] 1N) (subvec [1 2 3] 1/2 2N)])
;; nth trunc + int-range (fixed bounds)
(throwof "nth-trunc" '[(nth [1 2 3] 1/2) (nth [1 2 3 4 5 6] 5.9M) (nth [1] 2N -1) (nth [1 2 3] 7/2)])
(throwof "nth-intrange" '(try (nth [1] 4294967296N) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "nth-intrange2" '(try (nth [1] 8589934593/2) (catch Throwable e [(str (type e)) (.getMessage e)])))
;; nth trunc, all in-bounds (7/2 discriminates trunc-vs-ceil)
(throwof "nth-trunc2" '[(nth [1 2 3] 1/2) (nth [1 2 3 4 5 6] 5.9M) (nth [1] 2N -1) (nth [1 2 3 4] 7/2)])
;; float past f32 range: checked or inf?
(throwof "float-huge" '(float (bigint "12676506002282294014967032053761234567890")))
(throwof "float-huged" '(float 1e300))
;; true (non-folding) ratio index rendering
(throwof "nth-truerat" '(try (nth [1] (/ (bigint "99999999999999999999") 3)) (catch IllegalArgumentException e (.getMessage e))))
;; who folds integral ratios to BigInt: add or range?
(throwof "fold-add" '[(+ 1/2 1/2) (+ 3/2 1/2) (* 1/2 2N) (/ 4N 2N) (take 3 (iterate (partial + 1/2) 0))])
(throwof "range-tower2" '[(range 0 2N) (range 0N 3) (range 1/2 3/2 1/2) (range 0 2 1N)])
;; bigdec-meets-ratio stepping + countedness
(throwof "range-mixed" '[(try (doall (take 3 (range 0M 2 1/3))) (catch Throwable e [(str (type e)) (.getMessage e)]))])
(throwof "range-counted" '[(counted? (range 0 5)) (counted? (range 0 5N)) (counted? (range 0 5.0))])
;; take-1 still forces a chunk (advance throws)
(throwof "range-take1" '(try (take 1 (range 0M 2 1/3)) (catch Throwable e [(str (type e)) (.getMessage e)])))
;; overflow step + double-walk tower kinds
(throwof "range-overflow" '(try (rest (range 9223372036854775800 9223372036854775807N 100)) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "range-dwalk" '[(range 0N 2.5) (range 0.5 2N) (range 0M 2.5)])
;; bigdec/bigint-step walks + huge conversions/div-zero
(throwof "range-bigdec" '(range 0M 2M 0.5M))
(throwof "range-bigstep" '(range 0N 3N 1N))
(throwof "conv-hugeint" '(try (int (bigint "170141183460469231731687303715884105728")) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "conv-ratlong" '(try (long (/ (bigint "99999999999999999999") 3)) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "conv-declong" '(try (long 1E+30M) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "conv-ratfloat" '(try (float (/ (bigint "12676506002282294014967032053761234567890") 3)) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "conv-decfloat" '(try (float 1E+30M) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "div-tower0" '[(try (/ 5M 0M) (catch ArithmeticException e "tower-div0")) (try (/ 1 0N) (catch ArithmeticException e "tower-div0"))])
;; int of huge decimal + float of huge decimal
(throwof "conv-hugedecint" '(try (int 1E+30M) (catch Throwable e [(str (type e)) (.getMessage e)])))
(throwof "conv-hugedecfloat" '(try (float 1E+300M) (catch Throwable e [(str (type e)) (.getMessage e)])))
;; ratio step/double end kinds
(throwof "range-ratd" '[(range 1/2 2.5) (range 0 2.5 1/2)])
;; bigdec-start/int-end/bigint-step kinds
(throwof "range-mixstep" '[(range 0M 2 1N) (float 5.5M)])
;; huge/plus exponents
(throwof "bigdec-exp" '[(bigdec "1e+3") (try (bigdec "1e99999999999999999999999") (catch Throwable e [(str (type e)) (.getMessage e)]))])
;; empty exponent tail
(throwof "bigdec-expe" '[(try (bigdec "1e") (catch Throwable e (.getMessage e))) (try (bigdec "1e-") (catch Throwable e (.getMessage e)))])
;; floor-gap pins: bigdec-of-ratio, rationalize edges, bigint zero/empty, BigDec compare orders, signum
(throwof "tower-gap" '[(bigdec 1/2) (bigdec 1/4) (rationalize 0.0) (rationalize 5e-324) (rationalize 1.0E16) (bigint "0") (bigint "000") (try (bigint "") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigint "+") (catch Throwable e [(str (type e)) (.getMessage e)])) (= 5.00M 5.0) (= 1.25M 1.5M) (.signum (java.math.BigDecimal. "0.0")) (.signum (java.math.BigDecimal. "-2.5"))])
;; factor-5 denominators + BigDec compare order
(throwof "tower-gap2" '[(bigdec 1/5) (bigdec 3/20) (= 5.00M 5.0M) (= 1.5M 1.25M)])
;; round-2 review findings: int-scale cap, neg-scale hash/equiv, double-sign bigint, 2^63 counts, short/byte/char tower
(throwof "tower-r2" '[(try (bigdec "1e-3000000000") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e-2147483648") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e2147483647") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e2147483648") (catch Throwable e [(str (type e)) (.getMessage e)])) (= 0E+3M 0M) (= (hash 0E+3M) (hash 0M)) (= 10E+4M 1E+5M) (= (hash 10E+4M) (hash 1E+5M)) (try (bigint "--5") (catch Throwable e [(str (type e)) (.getMessage e)])) (take 2 (repeat 9.223372036854776E18 "x")) (try (nth [1 2 3] 9.223372036854776E18) (catch Throwable e [(str (type e)) (.getMessage e)])) (short 5N) (byte 5M) (char 65N) (try (short 40000) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 128) (catch Throwable e [(str (type e)) (.getMessage e)]))])
;; round-2b: exponent-digit boundary, short/byte/char edges, take/drop huge
(throwof "tower-r2b" '[(try (bigdec "1e9999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e99999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (short 5.5) (short 5.9M) (try (short 1e300) (catch Throwable e [(str (type e)) (.getMessage e)])) (byte -129) (byte 5N) (char 65.0) (char 66M) (try (char 1114112) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char -1) (catch Throwable e [(str (type e)) (.getMessage e)])) (take 9.223372036854776E18 [1 2 3]) (drop 9.223372036854776E18 [1 2 3])])
;; round-2c: re-probe forms lost when tower-r2b aborted (each try-wrapped)
(throwof "tower-r2c" '[(try (bigdec "1e9999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e99999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (short 5.5) (short 5.9M) (try (short 1e300) (catch Throwable e [(str (type e)) (.getMessage e)])) (byte 5N) (char 65.0) (char 66M) (try (char 1114112) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char -1) (catch Throwable e [(str (type e)) (.getMessage e)])) (take 9.223372036854776E18 [1 2 3]) (drop 9.223372036854776E18 [1 2 3])])
;; round-2d: every form try-wrapped (take/drop suspected for the AE)
(throwof "tower-r2d" '[(try (bigdec "1e9999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (bigdec "1e99999999999") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 5.5) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 5.9M) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 1e300) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 5N) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 65.0) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 66M) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 1114112) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char -1) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (take 9.223372036854776E18 [1 2 3]) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (drop 9.223372036854776E18 [1 2 3]) (catch Throwable e [(str (type e)) (.getMessage e)]))])
;; round-2e: nonzero-exponent rule, byte-double, huge-bigint short, ratio short/byte
(throwof "tower-r2e" '[(try (bigdec "1e00000000001") (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 1e300) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 5.5) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 9223372036854775808N) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 9223372036854775807N) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (short 1/2) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 1/2) (catch Throwable e [(str (type e)) (.getMessage e)]))])
;; round-2f: BigDec-huge + ratio narrowings for short/byte/char
(throwof "tower-r2f" '[(try (short 1E300M) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (byte 1E300M) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 1E300M) (catch Throwable e [(str (type e)) (.getMessage e)])) (short 7/2) (byte 7/2) (char 7/2)])
;; round-2g: char intValue model discriminators
(throwof "tower-r2g" '[(try (char 1e300) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 65.5) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 4294967296N) (catch Throwable e [(str (type e)) (.getMessage e)])) (try (char 9223372036854775808N) (catch Throwable e [(str (type e)) (.getMessage e)]))])
