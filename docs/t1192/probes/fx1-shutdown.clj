;; T1192 FX1 shutdown probes (oracle: stock Clojure 1.13.0-alpha9).
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.13.0-alpha9 jars> clojure.main fx1-shutdown.clj
(println (shutdown-agents))
(try (future 1)
  (catch java.util.concurrent.RejectedExecutionException e
    (println (str (class e)))))
(try (future 1)
  (catch java.util.concurrent.RejectedExecutionException e
    ;; Volatile across runs (pool identity hashes) — the port pins
    ;; a fixed message instead (frozen stable-text wedge).
    (println (str "MSG: " (ex-message e)))))
