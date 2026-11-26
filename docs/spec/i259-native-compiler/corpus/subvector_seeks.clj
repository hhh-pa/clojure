;; Windows deep into a large vector (Slice 5m): joining, printing,
;; seq-walking, reducing, and `into`/`drop` over a subvector whose
;; start sits far into the base vector must read only the window's own
;; items (the cursor seeks), with the same values, laziness, and side
;; effects as `run`.
(def big (vec (mapv #(- % 500) (range 4096))))
(def win (subvec big 3000 3100))

;; join and print over the deep window (sudoku's print shape)
(println (clojure.string/join "," (subvec big 4000 4004)))
(println (clojure.string/join " " win))
(println (str (clojure.string/join "-" (subvec big 100 103)) "!"))
;; seq walks: first/next, map/filter/reduce over the window
(println (first win) (last win) (nth win 5) (count win))
(println (reduce + 0 win) (mapv (fn [x] (* x 2)) (subvec big 1024 1032)))
(println (vec (filter odd? (subvec big 2047 2060))))
(println (some #{5} win) (every? int? win))
;; window over a window, and into over a tail window
(println (clojure.string/join "." (subvec (subvec big 100 4000) 2900 2903)))
(println (count (into [] (subvec big 3900 4096))))
;; drop on a vector seeks too
(println (vec (drop 4094 big)) (vec (drop 2 (subvec big 10 14))))
;; doseq side-effect order over a deep window
(def log (atom []))
(doseq [x (subvec big 3071 3075)] (swap! log conj x))
(println @log)
;; laziness unchanged: a lazy map over the window realizes on demand
(def lz (map (fn [x] (do (swap! log conj :r) x)) (subvec big 1 4)))
(println (realized? lz) (first lz) (count @log) (count lz) (count @log))
;; error shapes match: out-of-range windows and nth
(println (try (subvec big 4096 4097) (catch Exception e :oob)))
(println (try (nth win 100) (catch Exception e :nth-oob)) (nth win 100 nil))
