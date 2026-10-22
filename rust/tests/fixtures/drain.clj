;; T1192 FX1: exit-join fixture — no awaits AND no
;; shutdown-agents; the driver latches + joins at exit (like
;; clojure.main), so both lines print deterministically (JVM
;; non-daemon parity).
(def drain-a (agent 0))
(send drain-a (fn [s] (println "agent-drained") 1))
(future (println "future-drained"))
