;; Gap-96 oracle probes: shutdown lifecycle with customs (oracle: 1.13.0-alpha9 jars, JDK 27).
;; Run from repo root:
;; CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/gap-96/g96-shutdown.clj > docs/gap-96/g96-shutdown.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn named-factory [thread-name]
  (reify java.util.concurrent.ThreadFactory
    (newThread [_ r] (doto (Thread. r thread-name) (.setDaemon true)))))
(defn tname [] (.getName (Thread/currentThread)))
;; ---- shutdown returns at once; in-flight custom actions finish ----
(throwof "s-inflight" '(do (def exs (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-life"))) (def als (agent 0)) (set-agent-send-executor! exs) (send als (fn [s] (Thread/sleep 800) 41)) (let [t0 (System/nanoTime) r (shutdown-agents) dt (/ (- (System/nanoTime) t0) 1000000.0)] [r (< dt 700) (do (Thread/sleep 1500) @als)])))
;; ---- post-shutdown send: accepted, handler sees the rejection, never runs ----
(throwof "s-post-send" '(do (def ps (promise)) (def vs (atom nil)) (def as1 (agent 1 :error-handler (fn [ag e] (reset! vs (str (class e))) (deliver ps :saw)))) (let [ret (send as1 inc)] [(identical? ret as1) (deref ps 5000 :TIMEOUT) @vs (await-for 300 as1) @as1])))
;; ---- post-shutdown custom send-off: same rejection ----
(throwof "s-post-off" '(do (def po (promise)) (def vo (atom nil)) (def ao1 (agent 2 :error-handler (fn [ag e] (reset! vo (str (class e))) (deliver po :saw)))) (set-agent-send-off-executor! (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-dead"))) (shutdown-agents) (let [ret (send-off ao1 inc)] [(identical? ret ao1) (deref po 5000 :TIMEOUT) @vo (await-for 300 ao1) @ao1])))
;; ---- post-shutdown future-call throws ----
(throwof "s-post-future" '(future-call (fn [] 1)))
;; ---- fresh installs after shutdown are live again ----
(throwof "s-fresh-send" '(do (def exn (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-fresh"))) (def anf (agent 0)) (def tpf (promise)) (set-agent-send-executor! exn) (send anf (fn [s] (deliver tpf (tname)) 9)) [(deref tpf 5000 :TIMEOUT) (await-for 5000 anf) @anf]))
(throwof "s-fresh-future" '(do (def exq (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-freshfut"))) (set-agent-send-off-executor! exq) (let [f (future-call (fn [] (tname)))] (deref f 5000 :TIMEOUT))))
(System/exit 0)
