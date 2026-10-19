;; S5D differential corpus 19: keys/vals tails, seq metas, take-last news, str/tostring, hash, = early-exit.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (7i keys/vals tails, take-last/butlast/drop-last,
;; str/tostring + hash, = early-exit), s5d-gaps7i2 (i2-drop-kind LazySeq@),
;; s5d-gaps7i3 (i3-keys, i3-meta-seq, i3-takelast, i3-butlast, i3-hashuoe,
;; i3-seqself), s5d-gaps7i4 (i4-tostring), s5d-gaps7i6 (r-keys-tail, r-tl-extra,
;; r-str-lazy, r-str-realize, r-hash-finite, r-eq, r-eq-lazy).
;; NOTE: keys/vals basics live in 09, take-last/butlast/drop values in 05
;; (all 7i3 butlast cells duplicate 05 -- see there), str/print in 11,
;; %s-lazy in 12; this file pins only the 7i-new cells.
;; Rule: LazySeq@ hashes are deterministic content hashes (no normalization
;; needed); (hash <infinite>) throws UOE immediately (no hang).
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

;; --- keys/vals news: nils, classes, unvalidated tails (7i3 i3-keys) ---
(p ":keys-empty" #(keys {}))
(p ":vals-empty" #(vals {}))
(p ":keys-empty-nilq" #(nil? (keys {})))
(p ":vals-nil-nilq" #(nil? (vals nil)))
(cls ":c-keys-good" #(keys {:a 1}))
(p ":keys-first-a" #(first (keys {:a 1})))
(cls ":c-keys-vec10" #(keys [10 20]))
(cls ":c-keys-rest-vecvec" #(rest (keys [[1 2]])))
(p ":keys-rest-vecvec-first" #(first (rest (keys [[1 2]]))))
(p ":keys-next-1-nilq" #(nil? (next (keys [[1]]))))
(cls ":c-keys-next-good" #(next (keys {:a 1 :b 2})))
(p ":keys-next-meta" #(meta (next (with-meta (keys {:a 1 :b 2}) {:m 1}))))

;; --- keys/vals tails: next/rest/drop never validate entries (7i6) ---
(cls ":c-keys-rest-1" #(rest (keys {:a 1})))
(p ":keys-rest-1-str" #(str (rest (keys {:a 1}))))
(p ":keys-rest-good-first" #(first (rest (keys {:a 1 :b 2}))))
(cls ":c-vals-next-vec" #(next (vals [1 2])))
(cls ":c-vals-rest-vec" #(rest (vals [1 2])))
(p ":vals-next-first" #(first (next (vals [1 2]))))
(p ":keys-drop1-str" #(str (drop 1 (keys {:a 1 :b 2}))))
(cls ":c-keys-drop1" #(drop 1 (keys {:a 1 :b 2})))
(p ":keys-rest-counted" #(counted? (rest (keys {:a 1 :b 2}))))

;; --- with-meta/next: only StringSeq propagates meta (7i3 i3-meta-seq) ---
(cls ":c-withmeta-seq" #(with-meta (seq [1 2]) {:a 1}))
(p ":meta-next-vec" #(meta (next (with-meta (seq [1 2]) {:a 1}))))
(p ":meta-next-strseq" #(meta (next (with-meta (seq "ab") {:a 1}))))
(p ":meta-rest-strseq" #(meta (rest (with-meta (seq "ab") {:a 1}))))
(p ":meta-next-range" #(meta (next (with-meta (range 5) {:a 1}))))
(p ":meta-next-repeat" #(meta (next (with-meta (repeat 2 :x) {:a 1}))))
(p ":meta-next-cycle" #(meta (next (with-meta (cycle [1]) {:a 1}))))
(p ":meta-next-iterate" #(meta (next (with-meta (iterate inc 0) {:a 1}))))
(p ":meta-rest-list" #(meta (rest (with-meta '(1 2) {:a 1}))))
(p ":meta-drop1-strseq" #(meta (drop 1 (with-meta (seq "abc") {:a 1}))))
(p ":meta-next-sort" #(meta (next (with-meta (sort [2 1]) {:a 1}))))

;; --- take-last/drop-last news (7i3, 7i6; rest duplicates 05) ---
(p ":tl-nil-lazy" #(take-last nil (map inc [1])))
(p ":tl-1e100" #(take-last 1e100 [1]))
(p ":tl-repeat-val" #(take-last 2 (repeat 3 :x)))
(cls ":c-droplast" #(drop-last 1 [1 2]))
(p ":droplast-str" #(str (drop-last 1 [1 2])))

;; --- str/tostring table (7i4 i4-tostring) ---
(p ":tostr-keys" #(str (keys {:a 1})))
(p ":tostr-seqvec" #(str (seq [1 2])))
(p ":tostr-seqstr" #(str (seq "ab")))
(p ":tostr-vec" #(str [1 2]))
(p ":tostr-list" #(str '(1 2)))
(p ":tostr-map" #(str {:a 1}))
(p ":tostr-set" #(str #{1}))
(p ":tostr-range" #(str (range 3)))
(p ":tostr-repeat" #(str (repeat 2 :x)))
(p ":tostr-cons" #(str (cons 1 [2])))
(p ":tostr-sort" #(str (sort [2 1])))
(p ":tostr-map-starts" #(.startsWith (str (map inc [1])) "clojure.lang.LazySeq@"))
(p ":tostr-mixvec" #(str ["a" :b]))

;; --- str of lazy: class@content-hash, realizes but never prints (7i6) ---
(p ":strlz-map" #(.startsWith (str (map inc [1])) "clojure.lang.LazySeq@"))
(p ":strlz-filter" #(.startsWith (str (filter odd? [1])) "clojure.lang.LazySeq@"))
(p ":strlz-realized" #(let [s (map inc [1])] (doall s) (.startsWith (str s) "clojure.lang.LazySeq@")))
(p ":strlz-printstr" #(print-str (map inc [1])))
(p ":strlz-take2cycle" #(str (take 2 (cycle [1]))))
(cls ":c-take2cycle" #(take 2 (cycle [1])))
(p ":strlz-realize" #(let [c (atom 0) s (map (fn [x] (swap! c inc) x) '(1 2))] [(str s) @c]))

;; --- seq-self news (7i3 i3-seqself; repeat/cycle/cons in 03/04) ---
(cls ":c-seq-range" #(seq (range 3)))
(cls ":c-seq-iterate" #(seq (iterate inc 0)))
(cls ":c-seq-keys" #(seq (keys {:a 1})))

;; --- hash: infinite seqs UOE, finite hash fine (7i3, 7i6) ---
(p ":hash-cycle" #(hash (cycle [1])))
(p ":hash-iterate" #(hash (iterate inc 0)))
(p ":hash-repeat" #(hash (repeat :x)))
(p ":hash-take1rep" #(hash (take 1 (repeat :x))))
(p ":hash-range0" #(hash (range 0 5 0)))
(p ":hash-mapinc" #(hash (map inc [1 2])))
(p ":hash-range10" #(hash (range 10)))
(p ":hash-filter" #(hash (filter odd? [1 2 3])))
(p ":hash-drop1" #(hash (drop 1 [1 2 3])))
(p ":hash-take2cycle" #(hash (take 2 (cycle [1]))))

;; --- = / equiv: cross-type rules + early exit (7i6 r-eq, r-eq-lazy) ---
(p ":eq-int-dbl" #(= 1 1.0))
(p ":eq-nan" #(= ##NaN ##NaN))
(p ":eq-bigint" #(= 1N 1))
(p ":eq-nilnil" #(= nil nil))
(p ":eq-nilfalse" #(= nil false))
(p ":eq-range-vec" #(= (range 3) [0 1 2]))
(p ":eq-vec-list" #(= [1 2] '(1 2)))
(p ":eq-vec-same" #(= [1 2] [1 2]))
(p ":eq-vec-diff" #(= [1] [2]))
(p ":eq-map-diffv" #(= {:a 1} {:a 2}))
(p ":eq-map-diffk" #(= {:a 1} {:b 1}))
(p ":eq-str" #(= "a" "a"))
(p ":eq-numstr" #(= 1 "a"))
(p ":eq-set" #(= #{1} #{1}))
(p ":eq-char" #(= \a \a))
(p ":eq-dbl" #(= 1.5 1.5))
(p ":eq-vec-count" #(= [1 2] [1 2 3]))
(p ":eq-repeat-term" #(= (repeat :x) [:y]))
(p ":eq-range-short" #(= (range) [0 1]))
(p ":eq-short-range" #(= [0 1] (range)))
(p ":eq-lazy-pair" #(let [a (atom 0) b (atom 0)]
                      [(= (map (fn [x] (swap! a inc) x) '(1 2))
                          (map (fn [x] (swap! b inc) x) '(1 3))) @a @b]))
(p ":eq-lazy-count" #(let [a (atom 0) b (atom 0)]
                       [(= (map (fn [x] (swap! a inc) x) '(1 2))
                           (map (fn [x] (swap! b inc) x) '(1))) @a @b]))
(p ":eq-lazy-vec" #(let [a (atom 0)]
                     [(= (map (fn [x] (swap! a inc) x) '(1 2)) [1 3]) @a]))
(println "S5D-19 DONE")
