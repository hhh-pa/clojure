;; Slice 5o: primed FastArith, pre-effect primed scalar loops, inline
;; double/int/long/char/bit-not/unchecked-* ops, Long/remainderUnsigned
;; in scalar loops, and inline Character/digit.

(defn primed-ops [a b c]
  [(+' a b) (-' a b) (*' a b) (inc' a) (dec' a) (+' a b c) (*' a 1.5)])

(println (primed-ops 10 3 2))
(println (primed-ops 9223372036854775807 1 2))
(println (primed-ops -9223372036854775808 1 2))
(println (primed-ops 2.5 1.5 0.5))

;; Pre-effect primed scalar loop (stays native on i64, deopts cleanly on BigInt promotion)
(defn pre-eff-dp [n seed]
  (let [row (transient (vec (repeat (inc n) 0)))]
    (assoc! row 0 seed)
    (loop [j 1]
      (when (<= j n)
        (assoc! row j (+' (nth row (dec j)) (nth row (dec j))))
        (recur (inc j))))
    (nth (persistent! row) n)))

(println (pre-eff-dp 10 1))
(println (pre-eff-dp 65 1))

;; Post-effect primed loop (must not replay effect on deopt)
(defn post-eff-primed [n]
  (let [t (transient [0])]
    (loop [i 0 acc 9223372036854775806]
      (if (= i n)
        [(nth (persistent! t) 0) acc]
        (do
          (assoc! t 0 (inc (nth t 0)))
          (recur (inc i) (+' acc 1)))))))

(println (post-eff-primed 4))

;; Inner-loop effect + primed inside outer loop (must not replay outer effect)
(defn nested-eff-primed [n]
  (let [t (transient [0])]
    (loop [outer 0 acc 9223372036854775806]
      (if (= outer n)
        [(nth (persistent! t) 0) acc]
        (let [a2 (loop [k 0 a acc]
                   (if (= k 2)
                     a
                     (do
                       (assoc! t 0 (inc (nth t 0)))
                       (recur (inc k) (+' a 1)))))]
          (recur (inc outer) a2))))))

(println (nested-eff-primed 2))

;; Scalar casts: double, int, long, char
(defn cast-all [x]
  [(double x) (long x) (int x)])

(println (cast-all 42))
(println (cast-all -17))
(println (cast-all 3.75))
(println (cast-all -3.75))
(println (double 10N) (long \A) (int \z) (long (bigint 7)) (int (/ 0.0 0.0)) (long (/ 0.0 0.0)))
(println (try (int 3000000000) (catch Exception _ :int-ov)))
(println (try (int 1.0e20) (catch Exception _ :dbl-int-ov)))
(println (try (long 1.0e30) (catch Exception _ :dbl-long-ov)))
(println (try (double \a) (catch Exception _ :dbl-ch)))

(defn char-casts [x] (char x))
(println (char-casts \Z) (char-casts 65) (char-casts 0) (char-casts 0xE000))
(println (try (char-casts -1) (catch Exception _ :ch-neg)))

;; Unchecked ops & bit-not outside and inside scalar loops
(defn unchecked-direct [a b]
  [(bit-not a)
   (unchecked-inc a)
   (unchecked-dec a)
   (unchecked-negate a)
   (unchecked-add a b)
   (unchecked-subtract a b)
   (unchecked-multiply a b)])

(println (unchecked-direct 7 3))
(println (unchecked-direct 9223372036854775807 1))
(println (unchecked-direct -9223372036854775808 1))
(println (unchecked-add 1.5 2.5) (unchecked-inc 1.5) (unchecked-dec 1.5) (unchecked-negate 1.5))

(defn lcg-loop [seed n m]
  (loop [i 0 s seed acc 0]
    (if (= i n)
      [s acc]
      (let [ns (unchecked-add (unchecked-multiply s 6364136223846793005) 1442695040888963407)
            r (Long/remainderUnsigned ns m)
            t (unchecked-subtract (unchecked-inc (unchecked-dec (unchecked-negate r))) (bit-not 0))]
        (recur (inc i) ns (unchecked-add acc t))))))

(println (lcg-loop 12345 10 7))

(defn ufp-loop [n]
  (loop [i 0 x 1.5]
    (if (= i n)
      x
      (recur (inc i)
             (unchecked-negate
              (unchecked-dec
               (unchecked-inc
                (unchecked-multiply
                 (unchecked-subtract (unchecked-add x 2.0) 1.0)
                 0.5))))))))

(println (ufp-loop 5))

;; Character/digit fast path & fallbacks
(defn cdig [c r] (Character/digit c r))
(println (cdig \0 10) (cdig \9 10) (cdig \a 16) (cdig \F 16) (cdig \z 36) (cdig \Z 36))
(println (cdig \a 10) (cdig \/ 10) (cdig \: 10) (cdig \a 1) (cdig \a 37) (cdig 65 16))
(println (try (cdig "a" 10) (catch Exception _ :cdig-err)))
