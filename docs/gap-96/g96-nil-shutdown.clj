;; Gap-96 oracle probes: shutdown with a nil pool field (oracle: 1.13.0-alpha9 jars, JDK 27).
;; Run from repo root:
;; CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/gap-96/g96-nil-shutdown.clj > docs/gap-96/g96-nil-shutdown.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(def pooled0 clojure.lang.Agent/pooledExecutor)
(def solo0 clojure.lang.Agent/soloExecutor)
;; ---- nil set returns nil (set! value) ----
(throwof "z-nil-set-ret" '(do (set-agent-send-executor! nil) (let [r (nil? clojure.lang.Agent/pooledExecutor)] (set-agent-send-executor! pooled0) r)))
;; ---- both nil: the solo NPE wins, nothing shuts ----
(throwof "z-both" '(do (set-agent-send-executor! nil) (set-agent-send-off-executor! nil) (try (shutdown-agents) (catch Throwable t [:threw (str (class t)) (str (.getMessage ^Throwable t))]) (finally (set-agent-send-executor! pooled0) (set-agent-send-off-executor! solo0)))))
(throwof "z-both-pools-live" '(do (def a (agent 0)) (def b (agent 0)) (send a inc) (send-off b inc) [(await-for 5000 a) (await-for 5000 b) @a @b]))
;; ---- shutdown-agents with nil pooledExecutor ----
(throwof "z-nil-pooled-shutdown" '(do (set-agent-send-executor! nil) (try (shutdown-agents) (finally (set-agent-send-executor! pooled0)))))
;; ---- the pooled NPE lands AFTER the solo side shut ----
(throwof "z-pooled-npe-shut-solo" '(do (def c (agent 0)) (def d (agent 0)) (send c inc) (send-off d inc) [(await-for 5000 c) (await-for 500 d) @c @d]))
;; ---- shutdown-agents with nil soloExecutor ----
(throwof "z-nil-solo-shutdown" '(do (set-agent-send-off-executor! nil) (try (shutdown-agents) (finally (set-agent-send-off-executor! solo0)))))
(shutdown-agents)
(System/exit 0)
