;; S5D differential corpus 16: range ctors/coercions, NaN/overflow edges, repeat edges.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (7i range ctors/coercions), s5d-gaps7i (i-range-*),
;; s5d-gaps7i4 (i4-range-nanstep), s5d-gaps7i6 (r-range-str0 recovery).
;; NOTE: 1-arg nil/str, (range), step-0 take/cls, (range 0 NaN), repeat
;; nil/str/bool/2.5 live in 03; this file pins only the 7i-new cells.
;; Rule: every infinite/NaN range is take-bounded.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn cls [tag thunk]
  (print tag " => ")
  (prn (try (let [v (thunk)] [(class v) (seq? v) (counted? v) (list? v) (sequential? v) (coll? v)])
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- range 1-arg news (7i i-range-self, i-range-err) ---
(p ":range-int5" #(range (int 5)))
(cls ":c-range-dbl5" #(range 5.0))
(p ":range-dbl5-elems" #(vec (map (fn [x] (str (class x))) (range 5.0))))
(p ":range-dbl3-take" #(take 2 (range 0 5.0 1)))
(p ":range-dbl3-firstcls" #(str (class (first (range 0 5.0 1)))))

;; --- range 3-arg bad types: any non-Number in any slot throws (7i, 7i6) ---
(p ":range-12s" #(range 1 2 "a"))
(p ":range-1s1" #(range 1 "a" 1))
(p ":range-s21" #(range "a" 2 1))
(p ":range-ss0" #(range "a" "b" 0))
(p ":range-1s0" #(take 2 (range 1 "a" 0)))
(p ":range-1s0-cls" #(str (class (range 1 "a" 0))))
(p ":range-s10" #(take 2 (range "a" 1 0)))
(p ":range-ss1" #(take 1 (range "a" "b" 1)))
(p ":range-nilnil" #(range nil nil))
(p ":range-1nil1" #(range 1 nil 1))
(p ":range-ss1b" #(range "a" "a" 1))
(p ":range-nil3" #(range nil nil nil))

;; --- range zero-step news (7i i-range-zero) ---
(p ":range-550-val" #(range 5 5 0))
(p ":range-540-take" #(take 3 (range 5 4 0)))
(cls ":c-range-negzero" #(range 0 5 -0.0))
(p ":range-step0d-firstcls" #(str (class (first (range 0.0 5 0.0)))))

;; --- range count overflow -> double path (7i i-range-big) ---
(p ":range-max1-take" #(take 2 (range 9223372036854775806 9223372036854775807 5)))
(cls ":c-range-0max" #(range 0 9223372036854775807 1))
(cls ":c-range-neg5max" #(range -5 9223372036854775807 2))
(p ":range-minmax-neg1" #(range Long/MIN_VALUE Long/MAX_VALUE -1))

;; --- range NaN matrix (7i i-range-nan) ---
(cls ":c-range-nannan" #(range ##NaN ##NaN))
(p ":range-nannan-take1" #(take 1 (range ##NaN ##NaN)))
(p ":range-11d" #(range 1 1.0))
(cls ":c-range-11d" #(range 1 1.0))
(p ":range-1d1" #(range 1.0 1))

;; --- range NaN step (7i4 i4-range-nanstep) ---
(p ":range-05nan-take" #(take 3 (range 0 5 ##NaN)))
(cls ":c-range-05nan" #(range 0 5 ##NaN))
(p ":range-0d5nan-take" #(take 3 (range 0.0 5 ##NaN)))

;; --- repeat news (7i i-repeat-err, 7i4 i4-repeat-huge) ---
(p ":rep0-val" #(repeat 0 :x))
(cls ":c-take1rep" #(take 1 (repeat :x)))
(p ":rephuge-take1" #(take 1 (repeat 9223372036854775807 :x)))
(cls ":c-rephuge" #(repeat 9223372036854775807 :x))
(println "S5D-16 DONE")
