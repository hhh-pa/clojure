;; S6 differential corpus 03: futures, promises, timeout deref, cancel.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s6-pins.md P3/P4, concurrency.rs future_rows, future_pred_rows,
;; future_throw_cached, deref_timeout_rows, future_timeout_rows,
;; future_cancel_rows, future_cancel_running_discards, future_ns_sharing,
;; promise_rows, promise_threaded_deliver, promise_meta_rows,
;; future_stress_swap, realized_rows (future/promise parts).
;; Ordered fixtures: `ff` (failed future, first deref rendezvouses) and `fc`
;; (cancelled future) are top-level defs read by the entries below them.
;; Every worker is released (deliver) before the final `:shutdown` so the
;; JVM exits; reify `__NNNN` counters are build-fixed (see MANIFEST).
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- future rows (future_rows) ---
(p ":future-deref-1" #(do @(future 1)))
(p ":future-multi-body" #(deref (future 1 2 3)))
(p ":future-call-basic" #(deref (future-call (fn [] 41))))
(p ":future-conveys" #(do (def ^:dynamic s6b-dx 1) (binding [s6b-dx 2] @(future s6b-dx))))
(p ":future-bind-local" #(do (def ^:dynamic s6b-dy 1) @(future (binding [s6b-dy 9] s6b-dy)) s6b-dy))
(p ":future-class" #(str (class (future 1))))
(p ":pr-future-bool" #(.startsWith ^String (pr-str (future 1)) "#object[clojure.core$future_call$reify__"))
(p ":str-future-bool" #(.startsWith ^String (str (future 1)) "clojure.core$future_call$reify__"))
(p ":futurecall-nonfn-spawns" #(future? (future-call 5)))
(p ":force-future-passthrough" #(future? (force (future 1))))
(p ":futurecall-arity-0" #(future-call))
(p ":futurecall-arity-2" #(future-call (fn []) (fn [])))

;; --- future predicates (future_pred_rows) ---
(p ":future?-vec" #(vec (map future? [(future 1) (promise) 1 nil (delay 1) "s"])))
(p ":futuredone-settled" #(let [f (future 1)] [@f (future-done? f) (realized? f)]))
(p ":futuredone-pending" #(let [g (promise) f (future @g :never)] [(future-done? f) (do (deliver g :go) :released)]))
(p ":futuredone-promise" #(future-done? (promise)))
(p ":futuredone-delay" #(future-done? (delay 1)))
(p ":futuredone-long" #(future-done? 5))
(p ":futuredone-nil" #(future-done? nil))
(p ":futuredone-arity-0" #(future-done?))

;; --- failed futures (future_throw_cached; ordered on `ff`) ---
(def ff (future (throw (ex-info "b" {}))))
(p ":future-fail-deref1" #(@ff))
(p ":future-fail-realized" #(realized? ff))
(p ":future-fail-done" #(future-done? ff))
(p ":future-fail-deref2" #(@ff))
(p ":future-fail-timeout-deref" #(deref ff 5000 :t))
(p ":future-wrap-fresh" #(let [f (future (throw (ex-info "b" {})))] (try @f (catch java.util.concurrent.ExecutionException e1 (try @f (catch java.util.concurrent.ExecutionException e2 [(= e1 e2) (= (ex-cause e1) (ex-cause e2))]))))))
(p ":future-nonfn-deref" #(@(future-call 5)))
(p ":future-exinfo-cause" #(try @(future (throw (ex-info "boom" {:n 1}))) (catch java.util.concurrent.ExecutionException e [(ex-message e) (ex-message (ex-cause e)) (ex-data (ex-cause e))])))
(p ":future-exinfo-nilmsg" #(@(future (throw (ex-info nil {:n 1})))))
(p ":future-exinfo-nildata" #(@(future (throw (ex-info "x" nil)))))
(p ":future-ee-not-runtime" #(try @(future (throw (ex-info "b" {}))) (catch RuntimeException e :r) (catch Exception e :e)))
(p ":future-checked-src" #(deref (future (throw (Exception. "boom")))))

;; --- timeout deref casting/conversion (deref_timeout_rows) ---
(p ":deref-timeout-atom" #(deref (atom 1) 50 :t))
(p ":deref-timeout-delay" #(deref (delay 1) 50 :t))
(p ":deref-timeout-var" #(do (def s6b-tv 1) (deref #'s6b-tv 50 :t)))
(p ":deref-timeout-long" #(deref 5 50 :t))
(p ":deref-timeout-nil" #(deref nil 50 :t))
(p ":deref-timeout-arity2" #(deref (future 1) 50))
(p ":deref-ms-str" #(deref (future 1) "50" :t))
(p ":deref-ms-kw" #(deref (future 1) :x :t))
(p ":deref-ms-bool" #(deref (future 1) true :t))
(p ":deref-ms-char" #(deref (future 1) \a :t))
(p ":deref-ms-nil" #(deref (future 1) nil :t))
(p ":deref-promise-09" #(deref (promise) 0.9 :t))
(p ":deref-promise-neg" #(deref (promise) -5 :t))
(p ":deref-promise-0" #(deref (promise) 0 :t))
(p ":deref-future-double-ms" #(deref (future 1) 5000.9 :t))
(p ":deref-settled-badms" #(let [f (future :v)] @f (deref f "50" :t)))
(p ":deref-settled-ok" #(let [f (future :v)] @f (deref f 50 :t)))

;; --- timeout outcomes (future_timeout_rows) ---
(p ":future-timeout-pending" #(let [p (promise) f (future @p :never)] (let [r (deref f 50 :t)] (deliver p :go) r)))
(p ":future-timeout-delivered" #(let [p (promise) f (future (deref p 5000 :slow))] (deliver p :fast) @f))
(p ":deref-promise-timeout" #(deref (promise) 50 :t))
(p ":deref-promise-delivered" #(let [p (promise)] (deliver p :v) (deref p 50 :t)))

;; --- cancel (future_cancel_rows; ordered on `fc`) ---
(def fc-gate (promise))
(def fc (future @fc-gate :never))
(p ":cancel-pending" #(future-cancel fc))
(p ":cancel-done" #(future-done? fc))
(p ":cancel-realized" #(realized? fc))
(p ":cancel-deref" #(@fc))
(p ":cancel-deref-timeout" #(deref fc 5000 :t))
(p ":cancel-msg-empty" #(try @fc (catch java.util.concurrent.CancellationException e (ex-message e))))
(p ":cancel-is-ise" #(try @fc (catch IllegalStateException e :ise) (catch Throwable t :other)))
(p ":cancel-again" #(future-cancel fc))
(p ":cancel-completed" #(let [f (future 1)] @f (future-cancel f)))
(p ":cancel-badms" #(deref fc "50" :t))
(p ":cancel-promise" #(future-cancel (promise)))
(p ":cancel-long" #(future-cancel 5))
(p ":cancel-nil" #(future-cancel nil))
(p ":cancel-arity-0" #(future-cancel))

;; --- cancel of a running body (future_cancel_running_discards) ---
;; JVM truth: the body is interrupted; deref throws Cancelled. (The port
;; runs the body to completion and discards the result instead.)
(p ":cancel-running" #(let [p (promise) started (promise) f (future (deliver started :s) @p :done)] @started [(future-cancel f) (future-done? f) (realized? f) (try @f (catch java.util.concurrent.CancellationException e :cancelled)) (do (deliver p :go) :released)]))

;; --- future/shared state (future_ns_sharing) ---
(p ":future-def-visible" #(do @(future (def s6b-shared 42)) s6b-shared))
(p ":future-late-def" #(let [p (promise) f (future @p (deref (resolve 's6b-late)))] (def s6b-late :late) (deliver p :go) @f))
(p ":future-convey-add" #(do (def ^:dynamic s6b-bv 1) (binding [s6b-bv 7] @(future (+ s6b-bv 1)))))

;; --- promises (promise_rows) ---
(p ":deliver-first-wins-eq" #(let [p (promise)] (= p (deliver p 1))))
(p ":deliver-second-nil" #(let [p (promise)] (deliver p 1) [(deliver p 2) @p (realized? p)]))
(p ":deliver-nil-first" #(let [p (promise)] [(= p (deliver p nil)) @p (realized? p) (deliver p 2)]))
(p ":promise-unrealized" #(realized? (promise)))
(p ":pr-promise-bool" #(.startsWith ^String (pr-str (promise)) "#object[clojure.core$promise$reify__"))
(p ":promise-class" #(str (class (promise))))
(p ":str-promise-bool" #(.startsWith ^String (str (promise)) "clojure.core$promise$reify__"))
(p ":promise-blocks-till-deliver" #(let [p (promise) f (future @p)] (deliver p :v) @f))
(p ":promise-invoke" #(let [p (promise)] [(= p (p 5)) @p (p 6)]))
(p ":deliver-plus" #(deliver + 5))
(p ":ifn-promise" #(ifn? (promise)))
(p ":ifn-future" #(ifn? (future 1)))
(p ":deliver-long" #(deliver 5 6))
(p ":deliver-nil-fn" #(deliver nil 1))
(p ":promise-call-0" #((promise)))
(p ":promise-call-2" #((promise) 1 2))
(p ":promise-arity-1" #(promise 1))
(p ":deliver-arity-1" #(deliver (promise)))
(p ":deliver-arity-3" #(deliver (promise) 1 2))

;; --- worker-thread delivery (promise_threaded_deliver analogue) ---
(p ":promise-worker-delivers" #(let [p (promise) f (future (deliver p 7))] @p))

;; --- future/promise meta (promise_meta_rows) ---
(p ":meta-promise-nil" #(meta (promise)))
(p ":meta-future-nil" #(meta (future 1)))
(p ":meta-withmeta-promise" #(meta (with-meta (promise) {:a 1})))
(p ":meta-withmeta-future" #(meta (with-meta (future 1) {:a 1})))
(p ":promise-meta-shared" #(let [p (promise) q (with-meta p {:a 1})] [(meta p) (meta q) (= p q) (do (deliver p :v) @q)]))
(p ":meta-vary-promise" #(meta (vary-meta (promise) assoc :a 1)))
(p ":altermeta-promise" #(alter-meta! (promise) assoc :a 1))
(p ":resetmeta-future" #(reset-meta! (future 1) {:a 1}))

;; --- stress (future_stress_swap) ---
(p ":stress-swap-2000" #(do (def s6b-n 8) (def s6b-m 250) (def s6b-a (atom 0)) (def s6b-futs (vec (map (fn [_] (future (dotimes [_ s6b-m] (swap! s6b-a inc)))) (range s6b-n)))) (doseq [f s6b-futs] @f) @s6b-a))

;; --- realized? future/promise parts (realized_rows; delay/lazy in 02) ---
(p ":realized-future-pending" #(let [g (promise) f (future @g :never)] [(realized? f) (do (deliver g :go) :released)]))
(p ":realized-future-done" #(let [f (future 1)] @f (realized? f)))
(p ":realized-promise-delivered" #(let [p (promise)] (deliver p 1) (realized? p)))
(p ":realized-future-failed" #(let [f (future (throw (ex-info "b" {})))] (try @f (catch Exception e :x)) (realized? f)))

;; --- shutdown so the JVM exits (agent_shutdown_rows; main pins in 04) ---
(deliver fc-gate :go)
(p ":shutdown" #(shutdown-agents))

(println "S6-03 DONE")
