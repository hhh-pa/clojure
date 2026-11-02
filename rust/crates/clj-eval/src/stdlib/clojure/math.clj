;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/math.clj (clojure.math) for clojure-rs:
;; same public API (E, PI + 44 fns). The 19 members the class-emulation
;; layer provides (sin cos tan asin acos atan exp log log10 sqrt cbrt ceil
;; floor atan2 pow round sinh cosh tanh hypot) are called verbatim; the rest
;; are pure implementations over them (Rust std doubles underneath).
;;
;; Port notes (behavioral deltas vs java.lang.Math):
;; - LONG COERCION (jdk-long): matches the JDK (long) cast for every double
;;   (NaN -> 0, infinities clamp, fractions truncate). Out-of-range BigInt /
;;   BigDecimal clamp instead of wrapping (the JVM wraps). (long x) itself
;;   throws out-of-range here, hence the explicit clamp.
;; - Integer-returning fns (get-exponent) answer longs (the port has no int).
;; - The private min-value stands in for Double/MIN_VALUE (2^-1074), which
;;   the emulation layer answers wrongly as MIN_NORMAL (pre-existing bug).
;; - IEEE-remainder is the single-rounding formula x - rint(x/y)*y: exact for
;;   small quotients, a few ulp off adversarially, garbage past |quotient| ~ Inf.
;; - expm1 / log1p use Taylor series (|x| <= 0.5, ~1e-15 relative) and the
;;   direct formula outside it (~1 ulp). Not bit-identical to the JDK.
;; - rint / ulp / signum / copy-sign / get-exponent / next-after / next-up /
;;   next-down / scalb / to-radians / to-degrees / floor-div / floor-mod /
;;   the *-exact family are exact (integer ops throw ArithmeticException
;;   "long overflow" exactly like the JDK *Exact family).
;; - :inline / :inline-arities metadata is kept verbatim but inert (our
;;   compiler does no inlining).
;; - (set! *warn-on-reflection* true) is guarded by thread-bound?: the loader
;;   does not bind the var during load, so a bare set! would fail the require.

(ns
  ^{:author "Alex Miller",
    :doc "Clojure wrapper functions for java.lang.Math static methods.

  Function calls are inlined for performance, and type hinted for primitive
  long or double parameters where appropriate. In general, Math methods are
  optimized for performance and have bounds for error tolerance. If
  greater precision is needed, use java.lang.StrictMath directly instead.

  For more complete information, see:
  https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html

  Port: members missing from the emulated java.lang.Math surface are pure
  implementations (see the file header for the three approximate ones:
  IEEE-remainder, expm1, log1p)."}
  clojure.math)

(when (thread-bound? #'*warn-on-reflection*)
  (set! *warn-on-reflection* true))

(def
  ^{:doc "Constant for e, the base for natural logarithms.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#E"
    :added "1.11"
    :const true
    :tag 'double}
  E
  Math/E)

(def
  ^{:doc "Constant for pi, the ratio of the circumference of a circle to its diameter.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#PI"
    :added "1.11"
    :const true
    :tag 'double}
  PI
  Math/PI)

;; ----- private port helpers (no upstream counterparts) -----

;; Forward references (helpers and earlier fns call later ones).
(declare rint ulp get-exponent next-up next-down)

(defn- nan?
  [d]
  (not (== d d)))

(defn- infinite?
  [d]
  (or (== d ##Inf) (== d ##-Inf)))

(defn- neg-zero?
  [v]
  (and (zero? v) (neg? (/ 1.0 v))))

(defn- jdk-long
  "The JDK (long) cast: NaN -> 0, infinities and out-of-range magnitudes
  clamp, fractions truncate. (long x) throws out-of-range here instead."
  [x]
  (cond (> x Long/MAX_VALUE) Long/MAX_VALUE
        (< x Long/MIN_VALUE) Long/MIN_VALUE
        :else (long x)))

(defn- jdk-int
  "The JDK (int) cast, same shape as jdk-long."
  [x]
  (cond (> x Integer/MAX_VALUE) Integer/MAX_VALUE
        (< x Integer/MIN_VALUE) Integer/MIN_VALUE
        :else (int x)))

(def ^:private deg-to-rad (/ Math/PI 180))
(def ^:private rad-to-deg (/ 180 Math/PI))

(defn- pow2
  "Exactly 2^k as a double (correctly rounded at the edges: >= 2^1024 is
  ##Inf, <= 2^-1075 is 0.0). Repeated squaring over the bits of k: every
  intermediate is an exact power of two (~11 iterations), so the only
  inexact steps are the correctly-rounded edge over/underflows."
  [k]
  (cond (> k 1100) ##Inf
        (< k -1100) 0.0
        (zero? k) 1.0
        (pos? k) (loop [r 1.0 b 2.0 n k]
                   (if (zero? n)
                     r
                     (let [r (if (odd? n) (* r b) r)
                           n (bit-shift-right n 1)]
                       (if (zero? n) r (recur r (* b b) n)))))
        :else (loop [r 1.0 b 0.5 n (- k)]
                (if (zero? n)
                  r
                  (let [r (if (odd? n) (* r b) r)
                        n (bit-shift-right n 1)]
                    (if (zero? n) r (recur r (* b b) n)))))))

;; The true Double/MIN_VALUE (2^-1074 = 4.9E-324): the emulated
;; Double/MIN_VALUE answers MIN_NORMAL instead (pre-existing port bug, out of
;; scope here), so every JDK MIN_VALUE below spells this instead.
(def ^:private min-value (pow2 -1074))

(defn- raw-exponent
  "The unbiased binary exponent e of d (d > 0, finite): 2^e <= d < 2^(e+1).
  A log approximation seeds e, then exact power-of-two comparisons adjust;
  all powers stay exact (exact doubling/halving, clamped seed)."
  [d]
  (let [e0 (max -1074 (min 1023 (int (Math/floor (/ (Math/log d) (Math/log 2.0))))))]
    (loop [e e0 p (pow2 e0)]
      (let [up (* p 2.0)]
        (cond (>= d up) (recur (inc e) up)
              (< d p) (recur (dec e) (/ p 2.0))
              :else e)))))

(defn- pow2?
  "Whether x (positive) is an exact power of two."
  [x]
  (== x (pow2 (raw-exponent x))))

(defn- next-down-pos
  "nextDown of x (x > 0, finite): one spacing down. Powers of two step by
  half their ulp, except at/below Double/MIN_NORMAL where the spacing below
  stays an ulp (the halved ulp underflows to zero there)."
  [x]
  (let [u (ulp x)]
    (if (pow2? x)
      (let [half (/ u 2.0)]
        (- x (if (zero? half) u half)))
      (- x u))))

(defn- taylor-expm1
  "expm1 via Taylor series, accurate for |x| <= 0.5."
  [x]
  (loop [sum x term x k 1 n 0]
    (if (>= n 30)
      sum
      (let [term' (/ (* term x) (+ k 1))]
        (recur (+ sum term') term' (+ k 1) (+ n 1))))))

(defn- taylor-log1p
  "log1p via Taylor series, accurate for |x| <= 0.5."
  [x]
  (loop [sum x term x k 1 n 0]
    (if (>= n 70)
      sum
      (let [term' (/ (* (- term) x k) (+ k 1))]
        (recur (+ sum term') term' (+ k 1) (+ n 1))))))

(defn sin
  {:doc "Returns the sine of an angle.
  If a is ##NaN, ##-Inf, ##Inf => ##NaN
  If a is zero => zero with the same sign as a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#sin-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/sin (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/sin a))

(defn cos
  {:doc "Returns the cosine of an angle.
  If a is ##NaN, ##-Inf, ##Inf => ##NaN
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#cos-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/cos (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/cos a))

(defn tan
  {:doc "Returns the tangent of an angle.
  If a is ##NaN, ##-Inf, ##Inf => ##NaN
  If a is zero => zero with the same sign as a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#tan-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/tan (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/tan a))

(defn asin
  {:doc "Returns the arc sine of an angle, in the range -pi/2 to pi/2.
  If a is ##NaN or |a|>1 => ##NaN
  If a is zero => zero with the same sign as a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#asin-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/asin (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/asin a))

(defn acos
  {:doc "Returns the arc cosine of a, in the range 0.0 to pi.
  If a is ##NaN or |a|>1 => ##NaN
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#acos-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/acos (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/acos a))

(defn atan
  {:doc "Returns the arc tangent of a, in the range of -pi/2 to pi/2.
  If a is ##NaN => ##NaN
  If a is zero => zero with the same sign as a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#atan-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/atan (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/atan a))

(defn to-radians
  {:doc "Converts an angle in degrees to an approximate equivalent angle in radians.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#toRadians-double-"
   :inline-arities #{1}
   :inline (fn [deg] `(Math/toRadians (double ~deg)))
   :added "1.11"}
  ^double [^double deg]
  ;; Upstream: (Math/toRadians deg), i.e. deg * (PI/180). Same two roundings.
  (* (double deg) deg-to-rad))

(defn to-degrees
  {:doc "Converts an angle in radians to an approximate equivalent angle in degrees.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#toDegrees-double-"
   :inline-arities #{1}
   :inline (fn [r] `(Math/toDegrees (double ~r)))
   :added "1.11"}
  ^double [^double r]
  ;; Upstream: (Math/toDegrees r), i.e. r * (180/PI). Same two roundings.
  (* (double r) rad-to-deg))

(defn exp
  {:doc "Returns Euler's number e raised to the power of a.
  If a is ##NaN => ##NaN
  If a is ##Inf => ##Inf
  If a is ##-Inf => +0.0
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#exp-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/exp (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/exp a))

(defn log
  {:doc "Returns the natural logarithm (base e) of a.
  If a is ##NaN or negative => ##NaN
  If a is ##Inf => ##Inf
  If a is zero => ##-Inf
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#log-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/log (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/log a))

(defn log10
  {:doc "Returns the logarithm (base 10) of a.
  If a is ##NaN or negative => ##NaN
  If a is ##Inf => ##Inf
  If a is zero => ##-Inf
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#log10-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/log10 (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/log10 a))

(defn sqrt
  {:doc "Returns the positive square root of a.
  If a is ##NaN or negative => ##NaN
  If a is ##Inf => ##Inf
  If a is zero => a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#sqrt-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/sqrt (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/sqrt a))

(defn cbrt
  {:doc "Returns the cube root of a.
  If a is ##NaN => ##NaN
  If a is ##Inf or ##-Inf => a
  If a is zero => zero with sign matching a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#cbrt-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/cbrt (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/cbrt a))

(defn IEEE-remainder
  {:doc "Returns the remainder per IEEE 754 such that
    remainder = dividend - divisor * n
  where n is the integer closest to the exact value of dividend / divisor.
  If two integers are equally close, then n is the even one.
  If the remainder is zero, sign will match dividend.
  If dividend or divisor is ##NaN, or dividend is ##Inf or ##-Inf, or divisor is zero => ##NaN
  If dividend is finite and divisor is infinite => dividend
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#IEEEremainder-double-double-

  Port: the single-rounding formula x - rint(x/y)*y. Exact for small
  quotients; may differ from the JDK in the last ulp(s), and degrades once
  |dividend/divisor| overflows to ##Inf."
   :inline-arities #{2}
   :inline (fn [dividend divisor] `(Math/IEEEremainder (double ~dividend) (double ~divisor)))
   :added "1.11"}
  ^double [^double dividend ^double divisor]
  ;; Upstream: (Math/IEEEremainder dividend divisor).
  (let [x (double dividend) y (double divisor)]
    (cond (or (nan? x) (nan? y)) ##NaN
          (or (infinite? x) (zero? y)) ##NaN
          (infinite? y) x
          :else (let [n (rint (/ x y))
                      n (if (zero? n) 0.0 n)]
                  (- x (* n y))))))

(defn ceil
  {:doc "Returns the smallest double greater than or equal to a, and equal to a
  mathematical integer.
  If a is ##NaN or ##Inf or ##-Inf or already equal to an integer => a
  If a is less than zero but greater than -1.0 => -0.0
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#ceil-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/ceil (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/ceil a))

(defn floor
  {:doc "Returns the largest double less than or equal to a, and equal to a
  mathematical integer.
  If a is ##NaN or ##Inf or ##-Inf or already equal to an integer => a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#floor-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/floor (double ~a)))
   :added "1.11"}
  ^double [^double a]
  (Math/floor a))

(defn rint
  {:doc "Returns the double closest to a and equal to a mathematical integer.
  If two values are equally close, return the even one.
  If a is ##NaN or ##Inf or ##-Inf or zero => a
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#rint-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/rint (double ~a)))
   :added "1.11"}
  ^double [^double a]
  ;; Upstream: (Math/rint a). Values >= 2^52 are already integral; below
  ;; that d - floor(d) == 0.5 iff d is an exact tie, so parity via long is
  ;; exact. Zero results keep the argument's sign.
  (let [d (double a)]
    (cond (or (nan? d) (infinite? d) (zero? d)) d
          (>= (abs d) 4503599627370496.0) d
          :else (let [f (Math/floor d)
                      diff (- d f)
                      r (cond (< diff 0.5) f
                              (> diff 0.5) (+ f 1.0)
                              (even? (long f)) f
                              :else (+ f 1.0))]
                  (if (zero? r) (if (neg? d) -0.0 0.0) r)))))

(defn atan2
  {:doc "Returns the angle theta from the conversion of rectangular coordinates (x, y) to polar coordinates (r, theta).
  Computes the phase theta by computing an arc tangent of y/x in the range of -pi to pi.
  For more details on special cases, see:
  https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#atan2-double-double-"
   :inline-arities #{2}
   :inline (fn [y x] `(Math/atan2 (double ~y) (double ~x)))
   :added "1.11"}
  ^double [^double y ^double x]
  (Math/atan2 y x))

(defn pow
  {:doc "Returns the value of a raised to the power of b.
  For more details on special cases, see:
  https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#pow-double-double-"
   :inline-arities #{2}
   :inline (fn [a b] `(Math/pow (double ~a) (double ~b)))
   :added "1.11"}
  ^double [^double a ^double b]
  (Math/pow a b))

(defn round
  {:doc "Returns the closest long to a. If equally close to two values, return the one
  closer to ##Inf.
  If a is ##NaN => 0
  If a is ##-Inf or < Long/MIN_VALUE => Long/MIN_VALUE
  If a is ##Inf or > Long/MAX_VALUE => Long/MAX_VALUE
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#round-double-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/round (double ~a)))
   :added "1.11"}
  ^long [^double a]
  (Math/round a))

(defn random
  {:doc "Returns a positive double between 0.0 and 1.0, chosen pseudorandomly with
  approximately random distribution.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#random--"
   :inline-arities #{0}
   :inline (fn [] `(Math/random))
   :added "1.11"}
  ^double []
  ;; Upstream: (Math/random). Same distribution shape over our RNG.
  (rand))

(defn add-exact
  {:doc "Returns the sum of x and y, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#addExact-long-long-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/addExact (long ~x) (long ~y)))
   :added "1.11"}
  ^long [^long x ^long y]
  ;; Upstream: (Math/addExact x y). Checked + throws "long overflow" alike.
  (+ (jdk-long x) (jdk-long y)))

(defn subtract-exact
  {:doc "Returns the difference of x and y, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#subtractExact-long-long-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/subtractExact (long ~x) (long ~y)))
   :added "1.11"}
  ^long [^long x ^long y]
  ;; Upstream: (Math/subtractExact x y).
  (- (jdk-long x) (jdk-long y)))

(defn multiply-exact
  {:doc "Returns the product of x and y, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#multiplyExact-long-long-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/multiplyExact (long ~x) (long ~y)))
   :added "1.11"}
  ^long [^long x ^long y]
  ;; Upstream: (Math/multiplyExact x y).
  (* (jdk-long x) (jdk-long y)))

(defn increment-exact
  {:doc "Returns a incremented by 1, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#incrementExact-long-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/incrementExact (long ~a)))
   :added "1.11"}
  ^long [^long a]
  ;; Upstream: (Math/incrementExact a).
  (inc (jdk-long a)))

(defn decrement-exact
  {:doc "Returns a decremented by 1, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#decrementExact-long-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/decrementExact (long ~a)))
   :added "1.11"}
  ^long [^long a]
  ;; Upstream: (Math/decrementExact a).
  (dec (jdk-long a)))

(defn negate-exact
  {:doc "Returns the negation of a, throws ArithmeticException on overflow.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#negateExact-long-"
   :inline-arities #{1}
   :inline (fn [a] `(Math/negateExact (long ~a)))
   :added "1.11"}
  ^long [^long a]
  ;; Upstream: (Math/negateExact a).
  (- (jdk-long a)))

(defn floor-div
  {:doc "Integer division that rounds to negative infinity (as opposed to zero).
  The special case (floorDiv Long/MIN_VALUE -1) overflows and returns Long/MIN_VALUE.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#floorDiv-long-long-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/floorDiv (long ~x) (long ~y)))
   :added "1.11"}
  ^long [^long x ^long y]
  ;; Upstream: (Math/floorDiv x y). The JDK algorithm verbatim (quot is
  ;; truncating and wraps MIN/-1 like Java /; unchecked ops wrap like Java).
  (let [x (jdk-long x) y (jdk-long y)
        r (quot x y)]
    (if (and (neg? (bit-xor x y)) (not= (unchecked-multiply r y) x))
      (unchecked-dec r)
      r)))

(defn floor-mod
  {:doc "Integer modulus x - (floorDiv(x, y) * y). Sign matches y and is in the
  range -|y| < r < |y|.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#floorMod-long-long-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/floorMod (long ~x) (long ~y)))
   :added "1.11"}
  ^long [^long x ^long y]
  ;; Upstream: (Math/floorMod x y). The JDK formula verbatim.
  (let [x (jdk-long x) y (jdk-long y)]
    (unchecked-subtract x (unchecked-multiply (floor-div x y) y))))

(defn ulp
  {:doc "Returns the size of an ulp (unit in last place) for d.
  If d is ##NaN => ##NaN
  If d is ##Inf or ##-Inf => ##Inf
  If d is zero => Double/MIN_VALUE
  If d is +/- Double/MAX_VALUE => 2^971
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#ulp-double-"
   :inline-arities #{1}
   :inline (fn [d] `(Math/ulp (double ~d)))
   :added "1.11"}
  ^double [^double d]
  ;; Upstream: (Math/ulp d). Normal d has ulp 2^(e-52); subnormals (exponent
  ;; below -1022) have ulp Double/MIN_VALUE.
  (let [d (double d)]
    (cond (nan? d) ##NaN
          (infinite? d) ##Inf
          (zero? d) min-value
          :else (let [e (raw-exponent (abs d))]
                  (if (< e -1022)
                    min-value
                    (pow2 (- e 52)))))))

(defn signum
  {:doc "Returns the signum function of d - zero for zero, 1.0 if >0, -1.0 if <0.
  If d is ##NaN => ##NaN
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#signum-double-"
   :inline-arities #{1}
   :inline (fn [d] `(Math/signum (double ~d)))
   :added "1.11"}
  ^double [^double d]
  ;; Upstream: (Math/signum d). Zeros fall through with their sign.
  (let [d (double d)]
    (cond (nan? d) ##NaN
          (pos? d) 1.0
          (neg? d) -1.0
          :else d)))

(defn sinh
  {:doc "Returns the hyperbolic sine of x, (e^x - e^-x)/2.
  If x is ##NaN => ##NaN
  If x is ##Inf or ##-Inf or zero => x
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#sinh-double-"
   :inline-arities #{1}
   :inline (fn [x] `(Math/sinh (double ~x)))
   :added "1.11"}
  ^double [^double x]
  (Math/sinh x))

(defn cosh
  {:doc "Returns the hyperbolic cosine of x, (e^x + e^-x)/2.
  If x is ##NaN => ##NaN
  If x is ##Inf or ##-Inf => ##Inf
  If x is zero => 1.0
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#cosh-double-"
   :inline-arities #{1}
   :inline (fn [x] `(Math/cosh (double ~x)))
   :added "1.11"}
  ^double [^double x]
  (Math/cosh x))

(defn tanh
  {:doc "Returns the hyperbolic tangent of x, sinh(x)/cosh(x).
  If x is ##NaN => ##NaN
  If x is zero => zero, with same sign
  If x is ##Inf => +1.0
  If x is ##-Inf => -1.0
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#tanh-double-"
   :inline-arities #{1}
   :inline (fn [x] `(Math/tanh (double ~x)))
   :added "1.11"}
  ^double [^double x]
  (Math/tanh x))

(defn hypot
  {:doc "Returns sqrt(x^2 + y^2) without intermediate underflow or overflow.
  If x or y is ##Inf or ##-Inf => ##Inf
  If x or y is ##NaN and neither is ##Inf or ##-Inf => ##NaN
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#hypot-double-double-"
   :inline-arities #{2}
   :inline (fn [x y] `(Math/hypot (double ~x) (double ~y)))
   :added "1.11"}
  ^double [^double x ^double y]
  (Math/hypot x y))

(defn expm1
  {:doc "Returns e^x - 1. Near 0, expm1(x)+1 is more accurate to e^x than exp(x).
  If x is ##NaN => ##NaN
  If x is ##Inf => #Inf
  If x is ##-Inf => -1.0
  If x is zero => x
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#expm1-double-

  Port: Taylor series for |x| <= 0.5 (~1e-15 relative), exp(x)-1 outside it
  (~1 ulp). Not bit-identical to the JDK."
   :inline-arities #{1}
   :inline (fn [x] `(Math/expm1 (double ~x)))
   :added "1.11"}
  ^double [^double x]
  ;; Upstream: (Math/expm1 x).
  (let [x (double x)]
    (cond (zero? x) x
          (<= (abs x) 0.5) (taylor-expm1 x)
          :else (dec (Math/exp x)))))

(defn log1p
  {:doc "Returns ln(1+x). For small values of x, log1p(x) is more accurate than
  log(1.0+x).
  If x is ##NaN or < -1 => ##NaN
  If x is ##Inf => ##Inf
  If x is -1 => ##-Inf
  If x is 0 => 0 with sign matching x
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#log1p-double-

  Port: Taylor series for |x| <= 0.5 (~1e-15 relative), log(1+x) outside it
  (~1 ulp). Not bit-identical to the JDK."
   :inline-arities #{1}
   :inline (fn [x] `(Math/log1p (double ~x)))
   :added "1.11"}
  ^double [^double x]
  ;; Upstream: (Math/log1p x).
  (let [x (double x)]
    (cond (zero? x) x
          (<= (abs x) 0.5) (taylor-log1p x)
          :else (Math/log (+ 1.0 x)))))

(defn copy-sign
  {:doc "Returns a double with the magnitude of the first argument and the sign of
  the second.
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#copySign-double-double-"
   :inline-arities #{2}
   :inline (fn [magnitude sign] `(Math/copySign (double ~magnitude) (double ~sign)))
   :added "1.11"}
  ^double [^double magnitude ^double sign]
  ;; Upstream: (Math/copySign magnitude sign). Negative zero is detected via
  ;; 1.0/x, so all zero/nonzero sign combinations are exact. (A NaN magnitude
  ;; keeps its own sign bit, unobservable without bit access.)
  (let [m (double magnitude) s (double sign)
        m (if (or (neg? m) (neg-zero? m)) (- m) m)]
    (if (or (neg? s) (neg-zero? s)) (- m) m)))

(defn get-exponent
  {:doc "Returns the exponent of d.
  If d is ##NaN, ##Inf, ##-Inf => Double/MAX_EXPONENT + 1
  If d is zero or subnormal => Double/MIN_EXPONENT - 1
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#getExponent-double-"
   :inline-arities #{1}
   :inline (fn [d] `(Math/getExponent (double ~d)))
   :added "1.11"}
  [^double d]
  ;; Upstream: (Math/getExponent d). Answers a long (no int kind here).
  (let [d (double d)]
    (cond (or (nan? d) (infinite? d)) 1024
          (zero? d) -1075
          :else (let [e (raw-exponent (abs d))]
                  (if (< e -1022) -1075 e)))))

(defn next-after
  {:doc "Returns the adjacent floating point number to start in the direction of
  the second argument. If the arguments are equal, the second is returned.
  If either arg is #NaN => #NaN
  If both arguments are signed zeros => direction
  If start is +-Double/MIN_VALUE and direction would cause a smaller magnitude
    => zero with sign matching start
  If start is ##Inf or ##-Inf and direction would cause a smaller magnitude
    => Double/MAX_VALUE with same sign as start
  If start is equal to +=Double/MAX_VALUE and direction would cause a larger magnitude
    => ##Inf or ##-Inf with sign matching start
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#nextAfter-double-double-"
   :inline-arities #{2}
   :inline (fn [start direction] `(Math/nextAfter (double ~start) (double ~direction)))
   :added "1.11"}
  ^double [^double start ^double direction]
  ;; Upstream: (Math/nextAfter start direction). == treats -0.0/0.0 as equal,
  ;; matching the "equal => direction" and "signed zeros => direction" rules.
  (let [s (double start) d (double direction)]
    (cond (or (nan? s) (nan? d)) ##NaN
          (== s d) d
          (> d s) (next-up s)
          :else (next-down s))))

(defn next-up
  {:doc "Returns the adjacent double of d in the direction of ##Inf.
  If d is ##NaN => ##NaN
  If d is ##Inf => ##Inf
  If d is zero => Double/MIN_VALUE
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#nextUp-double-"
   :inline-arities #{1}
   :inline (fn [d] `(Math/nextUp (double ~d)))
   :added "1.11"}
  ^double [^double d]
  ;; Upstream: (Math/nextUp d). Positive d steps by its exact ulp (always
  ;; exact, overflowing to ##Inf at MAX_VALUE); negative d mirrors nextDown.
  (let [d (double d)]
    (cond (nan? d) ##NaN
          (== d ##Inf) ##Inf
          (== d ##-Inf) (- Double/MAX_VALUE)
          (zero? d) min-value
          (neg? d) (- (next-down-pos (- d)))
          :else (+ d (ulp d)))))

(defn next-down
  {:doc "Returns the adjacent double of d in the direction of ##-Inf.
  If d is ##NaN => ##NaN
  If d is ##-Inf => ##-Inf
  If d is zero => -Double/MIN_VALUE
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#nextDown-double-"
   :inline-arities #{1}
   :inline (fn [d] `(Math/nextDown (double ~d)))
   :added "1.11"}
  ^double [^double d]
  ;; Upstream: (Math/nextDown d). Positive d steps by next-down-pos; negative
  ;; d mirrors next-up.
  (let [d (double d)]
    (cond (nan? d) ##NaN
          (== d ##-Inf) ##-Inf
          (== d ##Inf) Double/MAX_VALUE
          (zero? d) (- min-value)
          (neg? d) (- (+ (- d) (ulp (- d))))
          :else (next-down-pos d))))

(defn scalb
  {:doc "Returns d * 2^scaleFactor, scaling by a factor of 2. If the exponent
  is between Double/MIN_EXPONENT and Double/MAX_EXPONENT, the answer is exact.
  If d is ##NaN => ##NaN
  If d is ##Inf or ##-Inf => ##Inf or ##-Inf respectively
  If d is zero => zero of same sign as d
  See: https://docs.oracle.com/javase/8/docs/api/java/lang/Math.html#scalb-double-int-"
   :inline-arities #{2}
   :inline (fn [d scaleFactor] `(Math/scalb (double ~d) (int ~scaleFactor)))
   :added "1.11"}
  ^double [^double d scaleFactor]
  ;; Upstream: (Math/scalb d (int scaleFactor)). The power of two is exact,
  ;; so the single multiply rounds exactly like the JDK (including gradual
  ;; underflow and signed-zero results).
  (let [d (double d)]
    (cond (nan? d) ##NaN
          (infinite? d) d
          (zero? d) d
          :else (* d (pow2 (jdk-int scaleFactor))))))
