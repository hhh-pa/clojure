;; #271 Slice 2: inline static fast paths (Long/parseLong, Integer/parseInt,
;; Double/parseDouble, Long/remainderUnsigned, Math/*), variadic arithmetic
;; folding, exact integer division, comparison chains, and min/max. Every
;; input a fast path does not take must fall back to run's own result or
;; error (bignums, ratios, decimals, nil, strings, chars).
(defn show [label f]
  (println label (try (let [v (f)] (str (pr-str v) " " (pr-str (class v))))
                      (catch Throwable e (str "THROW " (.getName (class e)) " " (.getMessage e))))))

(def nums [0 1 -1 7 Long/MAX_VALUE Long/MIN_VALUE 0.0 -0.0 1.5 -2.5 ##NaN ##Inf
           10000000000000000000N 1/2 1.5M nil "3" \a])

(doseq [x nums]
  (show [:abs x] #(Math/abs x))
  (show [:sqrt x] #(Math/sqrt x))
  (show [:unary x] #(vector (Math/exp x) (Math/log x) (Math/log10 x) (Math/ceil x) (Math/floor x)
                            (Math/sin x) (Math/cos x) (Math/tan x) (Math/asin x) (Math/acos x)
                            (Math/atan x) (Math/sinh x) (Math/cosh x) (Math/tanh x) (Math/cbrt x)))
  (show [:not-inlined x] #(vector (Math/signum x) (Math/rint x)))
  (doseq [y [0 2 -3 0.5 ##NaN 3/2 nil]]
    (show [:pow x y] #(Math/pow x y))
    (show [:atan2 x y] #(Math/atan2 x y))
    (show [:hypot x y] #(Math/hypot x y))
    (show [:urem x y] #(Long/remainderUnsigned x y))))

(doseq [s ["0" "-1" "+1" "-0" "" " 1" "-" "0x10" "9223372036854775807" "9223372036854775808"
           "-9223372036854775808" "2147483647" "2147483648" "-2147483649" "1_000" "1.0" "NaN"
           "-Infinity" "1e10" ".5" "5." "1d" "1.5F" "0x1p3" " 1.5 " "1e400" "00012" nil 5 \1]]
  (show [:parse s] #(vector (Long/parseLong s)))
  (show [:parseInt s] #(Integer/parseInt s))
  (show [:parseDouble s] #(Double/parseDouble s)))

(show :computed-class #(clojure.core/__static* (str "java.lang." "Math") "abs" -5))

(doseq [a [1 -7 Long/MAX_VALUE 2.5 1/3 nil 10000000000000000000N]
        b [2 0 -1 0.5 Long/MIN_VALUE nil]]
  (show [:fold a b] #(vector (+ a b 1) (+ 1 a b a b) (- a b 1) (* a b 2 1)))
  (show [:unfolded a b] #(vector (+' a b a b) (bit-and a b 3 7) (bit-or a b 1 2) (bit-xor a b 1 2)))
  (show [:div a b] #(vector (/ a b) (/ a b 2)))
  (show [:chain a b] #(vector (< a b 3) (<= a b b) (> a b -10 -20) (>= a a b) (== a b b)
                              (if (< a b 3) :lt :not-lt) (if (> 10 a b) :gt :not-gt)))
  (show [:minmax a b] #(vector (max a b) (min a b) (max a b 0) (min a b 0 -1)))
  (show [:quot a b] #(vector (quot a b) (rem a b) (mod a b))))

(defn typed-loop [n]
  (loop [i 0 hi 0 lo 100 acc 0]
    (if (< i n)
      (recur (inc i) (max hi (rem (* i 37) 101)) (min lo (rem (* i 53) 97))
             (+ acc (Math/abs (- i 50)) (Long/remainderUnsigned i 7)))
      [hi lo acc])))
(show :typed-loop #(typed-loop 200))
(show :loop-ovf #(loop [i 0 acc (- Long/MAX_VALUE 1)] (if (< i 3) (recur (inc i) (+ acc i)) acc)))
