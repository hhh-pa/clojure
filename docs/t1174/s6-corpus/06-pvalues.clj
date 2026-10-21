;; S6 differential corpus 06: pvalues/pcalls composites, error propagation.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s9-plan.md S9B (s6-plan "follow-up if cheap" item),
;; concurrency.rs pcalls_rows, pvalues_rows,
;; pvalues_error_wraps_execution_exception.
;; Only future-backed composites run here (daemon pools, so the JVM
;; exits); no agent shutdown entry is needed.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- pcalls rows (pcalls_rows) ---
(p ":pcalls-vec" #(vec (pcalls (fn [] 1) (fn [] 2) (fn [] 3))))
(p ":pcalls-vec-empty" #(vec (pcalls)))
(p ":pcalls-seq" #(seq? (pcalls (fn [] 1))))

;; --- pvalues rows (pvalues_rows) ---
(p ":pvalues-vec" #(vec (pvalues 1 (+ 1 1) :a)))
(p ":pvalues-vec-empty" #(vec (pvalues)))
(p ":pvalues-inc3" #(let [n (atom 0)] (doall (pvalues (swap! n inc) (swap! n inc) (swap! n inc))) @n))

;; --- pvalues error (pvalues_error_wraps_execution_exception) ---
(p ":pvalues-throw" #(doall (pvalues 1 (/ 1 0) 3)))
