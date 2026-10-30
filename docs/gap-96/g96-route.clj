;; Gap-96 oracle probes: routing + nil + validation (oracle: 1.13.0-alpha9 jars, JDK 27).
;; Run from repo root:
;; CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/gap-96/g96-route.clj > docs/gap-96/g96-route.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(def pooled0 clojure.lang.Agent/pooledExecutor)
(def solo0 clojure.lang.Agent/soloExecutor)
(defn named-factory [thread-name]
  (reify java.util.concurrent.ThreadFactory
    (newThread [_ r] (doto (Thread. r thread-name) (.setDaemon true)))))
(defn tname [] (.getName (Thread/currentThread)))
;; ---- send routes onto the installed pool ----
(throwof "r-send" '(do (def exr (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-send"))) (def ar (agent 0)) (def tpr (promise)) (set-agent-send-executor! exr) (send ar (fn [s] (deliver tpr (tname)) (inc s))) (let [t (deref tpr 5000 :TIMEOUT)] (await ar) (set-agent-send-executor! pooled0) (.shutdownNow exr) [@ar t])))
;; ---- send-off routes onto the installed pool ----
(throwof "r-off" '(do (def exo (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-off"))) (def ao (agent 0)) (def tpo (promise)) (set-agent-send-off-executor! exo) (send-off ao (fn [s] (deliver tpo (tname)) (inc s))) (let [t (deref tpo 5000 :TIMEOUT)] (await ao) (set-agent-send-off-executor! solo0) (.shutdownNow exo) [@ao t])))
;; ---- future-call runs on the installed solo pool ----
(throwof "r-future" '(do (def exf (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-fut"))) (set-agent-send-off-executor! exf) (let [f (future-call (fn [] (tname))) t (deref f 5000 :TIMEOUT)] (set-agent-send-off-executor! solo0) (.shutdownNow exf) t)))
;; ---- per-send read: swapping pools between sends moves each action ----
(throwof "r-persend" '(do (def ex1 (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-one"))) (def ex2 (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-two"))) (def ap (agent [])) (set-agent-send-executor! ex1) (send ap (fn [s] (conj s (tname)))) (set-agent-send-executor! ex2) (send ap (fn [s] (conj s (tname)))) (await ap) (let [v @ap] (set-agent-send-executor! pooled0) (.shutdownNow ex1) (.shutdownNow ex2) v)))
;; ---- await barriers run on the installed send pool (await completes) ----
(throwof "r-await" '(do (def exa (java.util.concurrent.Executors/newSingleThreadExecutor (named-factory "g96-await"))) (def aa (agent 0)) (set-agent-send-executor! exa) (send aa inc) (let [r (await aa)] (set-agent-send-executor! pooled0) (.shutdownNow exa) [r @aa])))
;; ---- nil send: handler notification + wedge (send slot) ----
(throwof "n-handler-send" '(do (def nps (promise)) (def nvs (atom nil)) (def nas (agent 0 :error-handler (fn [ag e] (reset! nvs [(str (class e)) (str (.getMessage ^Throwable e))]) (deliver nps :saw)))) (set-agent-send-executor! nil) (send nas (fn [s] :never)) (let [saw (deref nps 5000 :TIMEOUT)] (send nas (fn [s] :second)) (let [w (await-for 500 nas) v @nas e (nil? (agent-error nas))] (set-agent-send-executor! pooled0) [saw @nvs w v e]))))
;; ---- nil send-off: handler notification + wedge (send-off slot) ----
(throwof "n-handler-off" '(do (def npo (promise)) (def nvo (atom nil)) (def nao (agent 0 :error-handler (fn [ag e] (reset! nvo [(str (class e)) (str (.getMessage ^Throwable e))]) (deliver npo :saw)))) (set-agent-send-off-executor! nil) (send-off nao (fn [s] :never)) (let [saw (deref npo 5000 :TIMEOUT)] (send-off nao (fn [s] :second)) (let [w (await-for 500 nao) v @nao e (nil? (agent-error nao))] (set-agent-send-off-executor! solo0) [saw @nvo w v e]))))
;; ---- nil solo: future-call throws to the caller ----
(throwof "n-future" '(do (set-agent-send-off-executor! nil) (try (let [f (future-call (fn [] 1))] (deref f 2000 :settled)) (finally (set-agent-send-off-executor! solo0)))))
;; ---- validation: String + Boolean CCE shapes ----
(throwof "n-cce-str" '(set-agent-send-executor! "x"))
(throwof "n-cce-bool" '(set-agent-send-off-executor! true))
;; ---- defaults restored ----
(throwof "r-restored" '(and (identical? clojure.lang.Agent/pooledExecutor pooled0) (identical? clojure.lang.Agent/soloExecutor solo0)))
(shutdown-agents)
(System/exit 0)
