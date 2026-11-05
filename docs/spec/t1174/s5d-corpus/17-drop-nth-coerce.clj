;; S5D differential corpus 17: drop/nthrest IDrop+coercions, nth big-index, subvec/subs huge, transient access.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (7i drop/nthrest coercions, 7i transients), s5d-gaps7i
;; (i-drop-wrap), s5d-gaps7i2/7i3 (i2/i3-drop-err, i2-drop-kind, i3-nth-big,
;; i3-nthrest), s5d-gaps7i4 (i4-drop-coerce, i4-nth-coerce, i4-nthrest,
;; i4-subvec-subs), s5d-gaps7i6 (r-drop-*, r-nth, r-transient, r-drop-round).
;; NOTE: the drop nil-matrix + take-last/butlast live in 05, nth/get in 09
;; (09 points transient-involved access HERE); this file pins only 7i-new cells.
;; Rule: ceil-family (drop/take/nthrest) rounds fractional n UP; truncate-family
;; (nth/subvec/subs) truncates. LazySeq@ hashes are deterministic content hashes.
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

;; --- drop huge n on IDrop: eager AE/IAE (7i, 7i4, 7i6) ---
(p ":drop-max-vec" #(drop 9223372036854775807 [1 2 3]))
(p ":drop-2p32p1-cls" #(str (class (drop 4294967297 [1 2 3]))))
(p ":drop-2p32p1-str" #(str (drop 4294967297 [1 2 3])))
(p ":drop-2p32-str" #(str (drop 4294967296 [1 2 3])))
(p ":drop-1e100-ctor" #(drop 1e100 [1 2 3]))
(p ":drop-1e100-list" #(str (drop 1e100 '(1 2 3))))
(p ":drop-1e100-str" #(str (drop 1e100 "abc")))
(p ":drop-5e9" #(drop 5e9 [1 2 3]))
(p ":drop-dblmax" #(drop (double Long/MAX_VALUE) [1 2 3]))
(p ":drop-neg1e100" #(drop -1e100 [1 2 3]))
(p ":drop-nan" #(drop ##NaN [1 2 3]))
(p ":drop-min" #(drop Long/MIN_VALUE [1 2 3]))
(p ":drop-max-str" #(str (drop Long/MAX_VALUE "abc")))

;; --- drop fractional/negative n: ceil, as-is (7i6) ---
(p ":drop-25-str" #(str (drop 2.5 [1 2 3 4])))
(cls ":c-drop-25" #(drop 2.5 [1 2 3 4]))
(p ":drop-24-str" #(str (drop 2.4 [1 2 3 4])))
(p ":drop-26-str" #(str (drop 2.6 [1 2 3 4])))
(p ":drop-05-str" #(str (drop 0.5 [1 2 3 4])))
(p ":drop-04-str" #(str (drop 0.4 [1 2 3 4])))
(p ":drop-neg05-str" #(str (drop -0.5 [1 2 3 4])))
(p ":drop-neg1-str" #(str (drop -1 [1 2 3])))
(cls ":c-drop-neg1" #(drop -1 [1 2 3]))
(p ":drop-25-strlazy" #(str (drop 2.5 "abcde")))

;; --- drop bad n: eager on IDrop, lazy+IAE on force otherwise (7i6) ---
(p ":drop-nil-vec" #(drop nil [1]))
(p ":drop-str-vec" #(drop "a" [1]))
(cls ":c-drop-nil5" #(drop nil 5))
(p ":drop-nil5-str" #(str (drop nil 5)))
(p ":drop-transient" #(vec (drop 1 (transient [1 2]))))
(cls ":c-drop-transient" #(drop 1 (transient [1 2])))

;; --- drop kind table (7i2 i2-drop-kind) ---
(cls ":c-drop1-map" #(drop 1 {:a 1 :b 2}))
(p ":drop1-map-vec" #(vec (drop 1 {:a 1 :b 2})))
(p ":drop5-map-str" #(str (drop 5 {:a 1})))
(cls ":c-drop0-vec" #(drop 0 [1 2]))
(cls ":c-drop1-str" #(drop 1 "abc"))
(p ":drop1-str-str" #(str (drop 1 "abc")))
(cls ":c-drop1-strseq" #(drop 1 (seq "abc")))
(cls ":c-drop0-emptylist" #(drop 0 '()))
(p ":drop0-emptylist-str" #(str (drop 0 '())))
(cls ":c-drop1-vec" #(drop 1 [1 2 3]))
(cls ":c-drop3-vec" #(drop 3 [1 2 3]))
(p ":drop3-vec-str" #(str (drop 3 [1 2 3])))
(cls ":c-drop0-map" #(drop 0 {:a 1}))
(p ":drop0-emptymap-str" #(str (drop 0 {})))
(cls ":c-drop1-emptymap" #(drop 1 {}))
(cls ":c-drop2-range" #(drop 2 (range 10)))
(p ":drop2-range-str" #(str (drop 2 (range 10))))
(cls ":c-drop2-repeat" #(drop 2 (repeat 5 :x)))

;; --- take ceil contrast (7i6 r-drop-round) ---
(p ":take-25-str" #(str (take 2.5 [1 2 3 4])))
(p ":take-24-str" #(str (take 2.4 [1 2 3 4])))
(p ":take-05-str" #(str (take 0.5 [1 2 3 4])))

;; --- nth big/coerce index: IAE vs AE vs truncate (7i3, 7i4, 7i6) ---
(p ":nth-max" #(nth [1 2 3] 9223372036854775807))
(p ":nth-25" #(nth [1 2 3] 2.5))
(p ":nth-nilidx2" #(nth [1 2 3] nil))
(p ":nth-stridx2" #(nth [1 2 3] "a"))
(p ":nth-min" #(nth [1 2 3] Long/MIN_VALUE))
(p ":nth-1e100" #(nth [1 2] 1e100))
(p ":nth-5e9" #(nth [1 2] 5e9))
(p ":nth-neg5e9" #(nth [1 2] -5e9))
(p ":nth-inf" #(nth [1 2] ##Inf))
(p ":nth-neginf" #(nth [1 2] ##-Inf))
(p ":nth-19" #(nth [1 2] 1.9))
(p ":nth-hugebigint" #(nth [1 2] 99999999999999999999999N))
(p ":nth-nilcoll-1e100" #(nth nil 1e100))
(p ":nth-strcoll-1e100" #(nth "ab" 1e100))
(p ":nth-ratio32" #(nth [0 1 2] 3/2))
(p ":nth-abc-1" #(nth "abc" 1))

;; --- nthrest (7i4 i4-nthrest, 7i6) ---
(p ":nthrest-over-str" #(str (nthrest [1 2 3] 5)))
(p ":nthrest-0-val" #(nthrest [1 2 3] 0))
(p ":nthrest-0-cls" #(str (class (nthrest [1 2 3] 0))))
(p ":nthrest-neg-val" #(nthrest [1 2 3] -1))
(p ":nthrest-frac15" #(nthrest [1 2 3 4] 1.5))
(p ":nthrest-max-vec" #(nthrest [1 2 3] 9223372036854775807))
(p ":nthrest-max-list" #(str (nthrest '(1 2 3) 9223372036854775807)))
(p ":nthrest-str-vec" #(nthrest [1 2] "a"))
(p ":nthrest-str-list" #(nthrest '(1 2) "a"))

;; --- subvec/subs huge index (7i4 i4-subvec-subs) ---
(p ":subvec-max" #(subvec [1 2 3] 9223372036854775807))
(p ":subvec-max2" #(subvec [1 2 3] 0 9223372036854775807))
(p ":subs-max" #(subs "abc" 0 9223372036854775807))
(p ":subvec-15" #(subvec [1 2 3] 1.5))
(p ":subs-15" #(subs "abc" 1.5))
(p ":subvec-stra" #(subvec [1 2 3] "a"))
(p ":subs-stra" #(subs "abc" "a" 1))
(p ":subvec-nil" #(subvec [1 2 3] nil))
(p ":subs-nil" #(subs "abc" nil 1))

;; --- transient-involved access (7i6 r-transient; pointed here by 09) ---
(p ":tget-vec" #(get (transient [1 2]) 0))
(p ":tget-map" #(get (transient {:a 1}) :a))
(p ":tget-miss-nf" #(get (transient [1 2]) 5 :nf))
(p ":tnth-oob" #(nth (transient [1 2]) 5))
(p ":tcount" #(count (transient [1 2])))
(p ":tcounted" #(counted? (transient [1 2])))
(p ":tseq" #(seq (transient [1 2])))
(p ":tfirst" #(first (transient [1 2])))
(p ":tcontains" #(contains? (transient {:a 1}) :a))
(p ":tfind" #(find (transient [10 20]) 1))
(println "S5D-17 DONE")
