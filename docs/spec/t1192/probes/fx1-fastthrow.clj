;; T1192 FX1 fast-throw probe (oracle: stock Clojure 1.13.0-alpha9).
;; Single-shot CCE messages vs hot-loop (OmitStackTraceInFastThrow).
(loop [i 0 full 0 empty 0]
  (if (>= i 50000)
    (println "update-tmap: full=" full "empty=" empty)
    (let [m (try (update (transient {:a 1}) :a inc) :no-throw
                 (catch ClassCastException e (.getMessage e)))]
      (recur (inc i) (if (some? m) (inc full) full) (if (nil? m) (inc empty) empty)))))
(loop [i 0 full 0 empty 0]
  (if (>= i 50000)
    (println "keys-next-first: full=" full "empty=" empty)
    (let [m (try (first (next (keys [1 2]))) :no-throw
                 (catch ClassCastException e (.getMessage e)))]
      (recur (inc i) (if (some? m) (inc full) full) (if (nil? m) (inc empty) empty)))))
