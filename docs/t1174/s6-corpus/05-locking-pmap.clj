;; S6 differential corpus 05: locking, pmap/pcalls/pvalues.
;; Sources: s6-pins.md P6; concurrency.rs locking_rows,
;; locking_mutual_exclusion, pmap_rows, pcalls_rows, pvalues_rows.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- locking (locking_rows, locking_mutual_exclusion) ---
(p ":locking-val" #(locking "x" 42))
(p ":locking-empty" #(locking "x"))
(p ":locking-multi" #(locking "x" 1 2 3))
(p ":locking-reentrant" #(locking "x" (locking "x" :inner)))
(p ":locking-nil" #(locking nil 1))
(p ":locking-arity" #(eval '(locking)))
(p ":locking-mutex" #(let [c (atom 0)] (dorun (map deref (doall (for [_ (range 4)] (future (dotimes [_ 50] (locking "g" (swap! c inc)))))))) @c))
(p ":locking-unlock-on-throw" #(let [c (atom 0)] (try (locking "g" (swap! c inc) (throw (ex-info "lx" {}))) (catch clojure.lang.ExceptionInfo _ nil)) (locking "g" (swap! c inc)) @c))

;; --- pmap (pmap_rows) ---
(p ":pmap-basic" #(pmap inc [1 2 3]))
(p ":pmap-empty" #(pmap inc []))
(p ":pmap-chunk" #(pmap (fn [x] (* x x)) (range 40)))
(p ":pmap-multi" #(pmap + [1 2 3] [10 20 30]))
(p ":pmap-multi-short" #(pmap + [1 2 3] [10 20]))
(p ":pmap-lazy" #(take 2 (pmap inc (range 100))))
(p ":pmap-str" #(pmap str [:a :b]))

;; --- pcalls/pvalues (pcalls_rows, pvalues_rows) ---
(p ":pcalls-basic" #(pcalls (fn [] 1) (fn [] 2)))
(p ":pcalls-empty" #(pcalls))
(p ":pvalues-basic" #(pvalues 1 (+ 1 1) :three))
(p ":pvalues-sidefx" #(let [c (atom 0)] [(sort (doall (pvalues (swap! c inc) (swap! c inc)))) @c]))

(println "S6-05 DONE")
