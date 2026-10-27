;; S10 oracle probes: agent executor vars (oracle: alpha10 jars, issue #53).
;; Run from repo root (/home/hhu/workspace/clojure-s10):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s10-cells/probes/executors.clj > docs/zero-oos-s10-cells/probes/executors.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; Save the JVM defaults; every probe below restores them.
(def pooled0 clojure.lang.Agent/pooledExecutor)
(def solo0 clojure.lang.Agent/soloExecutor)
(defn named-factory [thread-name]
  (reify java.util.concurrent.ThreadFactory
    (newThread [_ r] (doto (Thread. r thread-name) (.setDaemon true)))))
;; ---- set-agent-send-executor!: returns the executor (set! value) ----
(throwof "e-set-ret" '(do (def ex1 (java.util.concurrent.Executors/newSingleThreadExecutor)) (let [ok (identical? (set-agent-send-executor! ex1) ex1)] (set-agent-send-executor! pooled0) (.shutdownNow ex1) ok)))
;; ---- replacement routes subsequent `send` actions to the new pool ----
(throwof "e-routes" '(do (def ex2 (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "s10-probe-pool"))) (def a1 (agent 0)) (def tp (promise)) (set-agent-send-executor! ex2) (send a1 (fn [s] (deliver tp (.getName (Thread/currentThread))) (inc s))) (let [t (deref tp 5000 :TIMEOUT)] (await a1) (set-agent-send-executor! pooled0) (.shutdownNow ex2) [@a1 t])))
;; ---- nil is accepted at set time; the send then NPEs ----
(throwof "e-nil-set" '(do (set-agent-send-executor! nil) (set-agent-send-executor! pooled0) :restored))
(throwof "e-nil-send" '(do (set-agent-send-executor! nil) (try (let [a (send (agent 0) inc)] [(.getName (class a)) @a (nil? (agent-error a))]) (finally (set-agent-send-executor! pooled0)))))
;; ---- non-executor rejected at set time; pool untouched ----
(throwcause "e-non-exec" '(set-agent-send-executor! :kw))
(throwof "e-non-exec-pool-ok" '(do (let [r (try (do (set-agent-send-executor! :kw) :set-ok) (catch Throwable t :caught) (finally (set-agent-send-executor! pooled0))) a (agent 0)] (send a inc) (await a) [r @a])))
;; ---- send-off executor mirrors ----
(throwof "e-off-ret" '(do (def ex3 (java.util.concurrent.Executors/newSingleThreadExecutor)) (let [ok (identical? (set-agent-send-off-executor! ex3) ex3)] (set-agent-send-off-executor! solo0) (.shutdownNow ex3) ok)))
(throwof "e-off-routes" '(do (def ex4 (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "s10-probe-off"))) (def a2 (agent 0)) (def tp2 (promise)) (set-agent-send-off-executor! ex4) (send-off a2 (fn [s] (deliver tp2 (.getName (Thread/currentThread))) (inc s))) (let [t (deref tp2 5000 :TIMEOUT)] (await a2) (set-agent-send-off-executor! solo0) (.shutdownNow ex4) [@a2 t])))
(throwof "e-off-nil-set" '(do (set-agent-send-off-executor! nil) (set-agent-send-off-executor! solo0) :restored))
(throwcause "e-off-non-exec" '(set-agent-send-off-executor! 42))
;; ---- pools are independent: solo swap does not move `send` ----
(throwof "e-cross" '(do (def ex5 (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "s10-probe-cross"))) (def a3 (agent 0)) (def tp3 (promise)) (set-agent-send-off-executor! ex5) (send a3 (fn [s] (deliver tp3 (.getName (Thread/currentThread))) (inc s))) (let [t (deref tp3 5000 :TIMEOUT)] (await a3) (set-agent-send-off-executor! solo0) (.shutdownNow ex5) (not= t "s10-probe-cross"))))
;; ---- still the JVM defaults afterwards ----
(throwof "e-restored" '(and (identical? clojure.lang.Agent/pooledExecutor pooled0) (identical? clojure.lang.Agent/soloExecutor solo0)))
;; Agent pools are non-daemon: shut them down so the oracle JVM exits.
(shutdown-agents)
