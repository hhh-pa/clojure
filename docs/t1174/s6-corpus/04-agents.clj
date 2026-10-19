;; S6 differential corpus 04: agents (send/await/errors/restart/watches/shutdown).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s6-pins.md P5/P6, concurrency.rs agent_ctor_rows/_errors,
;; agent_error_mode_rows, agent_restart_rows, agent_await_rows,
;; agent_nested_rows, agent_nested_discarded_on_failure, agent_validator_rows,
;; agent_watch_rows, agent_star_rows, agent_send_shapes, agent_panic_rows,
;; agent_value_shapes, agent_stress, agent_shutdown_rows,
;; agent_restart_resumed_after_shutdown, agent_shutdown_joins,
;; agent_restart_after_shutdown, set_error_handler_rows, set_error_mode_rows.
;; Order matters: `rs`/`rs2` fail at load for the post-shutdown restart pins;
;; `:shutdown-join-all` is the first shutdown call; nothing after it needs
;; live pools. `latched` polls agent-error so `await` never races the latch
;; (P6 CAUTION). Never prints raw refs.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn latched [a] (while (nil? (agent-error a)) (Thread/sleep 10)) a)
(defn failed [] (let [a (agent 0)] (send a (fn [s] (throw (ex-info "boom" {:n 1})))) (latched a)))

;; Pre-shutdown failure fixtures (for the post-shutdown restart pins).
(def rs-gate (promise))
(def rs (agent 0))
(send rs (fn [s] @rs-gate (throw (ex-info "x" {}))))
(send rs inc)
(deliver rs-gate :go)
(def rs2-gate (promise))
(def rs2 (agent 0))
(send rs2 (fn [s] @rs2-gate (throw (ex-info "x" {}))))
(send rs2 inc)
(deliver rs2-gate :go)

;; --- ctor rows (agent_ctor_rows) ---
(p ":agent-deref-1" #(deref (agent 1)))
(p ":agent-deref-nil" #(deref (agent nil)))
(p ":agent-meta-kv" #(meta (agent 1 :meta {:k :v})))
(p ":agent-meta-none" #(meta (agent 1)))
(p ":agent-meta-nilopt" #(meta (agent 1 :meta nil)))
(p ":agent-validator-ok" #(do (agent 2 :validator even?) :ok))
(p ":agent-validator-nil" #(do (agent 1 :validator nil) :ok))
(p ":agent-handler-nil" #(do (agent 1 :error-handler nil) :ok))
(p ":agent-unknown-opt" #(do (agent 1 :foo 2) :ok))
(p ":agent-dup-meta" #(meta (agent 1 :meta {:a 1} :meta {:b 2})))
(p ":agent-bogus-mode-ok" #(do (agent 1 :error-mode :bogus) :ok))
(p ":agent-class" #(str (class (agent 1))))
(p ":pr-agent-bool" #(.startsWith ^String (pr-str (agent 1)) "#object[clojure.lang.Agent 0x"))
(p ":str-agent-bool" #(.startsWith ^String (str (agent 1)) "clojure.lang.Agent@"))
(p ":agent-eq-self" #(let [a (agent 1)] (= a a)))
(p ":agent-eq-fresh" #(= (agent 1) (agent 1)))
(p ":agent-atom-neq" #(= (agent 1) (atom 1)))
(p ":agent-hash-int" #(integer? (hash (agent 1))))
(p ":send-returns-agent" #(let [a (agent 0)] [(= a (send a inc)) (await a) @a]))
(p ":sendoff-returns-agent" #(let [a (agent 0)] [(= a (send-off a inc)) (await a) @a]))
(p ":sendvia-returns-agent" #(let [a (agent 0)] (= a (send-via nil a inc))))
(p ":sendvia-nil-wedges" #(let [a (agent 0)] (send-via nil a inc) [(await-for 2000 a) @a]))

;; --- ctor errors (agent_ctor_errors) ---
(p ":agent-ctor-veto" #(agent 2 :validator odd?))
(p ":agent-ctor-noval" #(agent 1 :validator))
(p ":agent-ctor-badmeta" #(agent 1 :meta 5))
(p ":agent-ctor-badvalidator" #(agent 1 :validator 5))
(p ":agent-ctor-badhandler" #(agent 1 :error-handler 5))
(p ":agent-ctor-badmode" #(agent 1 :error-mode 5))
(p ":agent-ctor-mode-nil" #(do (agent 1 :error-mode nil) :ok))
(p ":agent-ctor-mode-false" #(do (agent 1 :error-mode false) :ok))
(p ":agent-arity-0" #(agent))

;; --- error modes (agent_error_mode_rows) ---
(p ":continue-clears" #(let [a (agent 0 :error-mode :continue)] (send a (fn [s] (throw (ex-info "x" {})))) (send a inc) (await a) [@a (agent-error a)]))
(p ":fail-await-caught" #(let [a (failed)] (try (await a) (catch RuntimeException e :failed))))
(p ":fail-state-parked" #(let [a (failed)] [(nil? (agent-error a)) @a]))
(p ":fail-error-detail" #(let [a (failed)] [(ex-message (agent-error a)) (ex-data (agent-error a))]))
(p ":fail-error-class" #(str (class (agent-error (failed)))))
(p ":fail-send-throws" #(send (failed) inc))
(p ":fail-sendoff-throws" #(send-off (failed) inc))
(p ":fail-sendvia-throws" #(send-via nil (failed) inc))
(p ":fail-await-throws" #(await (failed)))
(p ":fail-awaitfor-throws" #(await-for 5000 (failed)))
(p ":fail-cause-detail" #(try (send (failed) inc) (catch RuntimeException e [(ex-message (ex-cause e)) (ex-data (ex-cause e))])))
(p ":handler-default-continues" #(do (def s6c-hp (promise)) (def s6c-ha (agent 0 :error-handler (fn [ag ex] (deliver s6c-hp [(= ag s6c-ha) (ex-message ex)])))) (send s6c-ha (fn [s] (throw (ex-info "kb" {})))) (send s6c-ha inc) [@s6c-hp (await s6c-ha) @s6c-ha (agent-error s6c-ha)]))
(p ":explicit-fail-wins" #(let [a (agent 0 :error-mode :fail :error-handler (fn [ag ex] :seen))] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":explicit-fail-parked" #(let [a (agent 0 :error-mode :fail :error-handler (fn [ag ex] :seen))] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) [(nil? (agent-error a)) @a]))
(p ":bogus-mode-fails" #(let [a (agent 0 :error-mode :bogus)] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":bogus-mode-parked" #(let [a (agent 0 :error-mode :bogus)] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) [(nil? (agent-error a)) @a]))
;; --- restart (agent_restart_rows) ---
(p ":restart-returns-new" #(let [g (promise) a (agent 0)] (send a (fn [s] @g (throw (ex-info "x" {})))) (send a inc) (deliver g :go) (latched a) (restart-agent a 10)))
(p ":restart-proceeds" #(let [g (promise) a (agent 0)] (send a (fn [s] @g (throw (ex-info "x" {})))) (send a inc) (deliver g :go) (latched a) (restart-agent a 10) [(await a) @a (agent-error a)]))
(p ":restart-no-watch-fire" #(let [g (promise) a (agent 0) log (atom [])] (send a (fn [s] @g (throw (ex-info "x" {})))) (send a inc) (deliver g :go) (latched a) (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (restart-agent a 10) (await a) (first @log)))
(p ":restart-clear-drops" #(let [g (promise) a (agent 0) log (atom [])] (send a (fn [s] @g (throw (ex-info "x" {})))) (send a (fn [s] (swap! log conj :held) 99)) (deliver g :go) (latched a) (restart-agent a 10 :clear-actions true) (await a) [@a @log (agent-error a)]))
(p ":restart-keep-queue" #(let [g (promise) a (agent 0)] (send a (fn [s] @g (throw (ex-info "x" {})))) (send a inc) (deliver g :go) (latched a) (restart-agent a 10 :clear-actions false) (await a) @a))
(p ":restart-veto-stays-failed" #(let [a (agent 0 :validator even?)] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) (restart-agent a 3)))
(p ":restart-veto-state" #(let [a (agent 0 :validator even?)] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) (try (restart-agent a 3) (catch IllegalStateException e :vetoed)) [@a (nil? (agent-error a))]))
(p ":restart-veto-then-ok" #(let [a (agent 0 :validator even?)] (send a (fn [s] (throw (ex-info "x" {})))) (latched a) (try (restart-agent a 3) (catch IllegalStateException e :vetoed)) [(restart-agent a 4) @a (agent-error a)]))
(p ":restart-unfailed" #(restart-agent (agent 1) 2))
(p ":restart-unfailed-novalidate" #(restart-agent (agent 0 :validator even?) 3))
(p ":restart-badclear-opt" #(restart-agent (agent 1) 2 :clear-actions))
(p ":restart-arity-1" #(restart-agent (agent 1)))
(p ":restart-nil" #(restart-agent nil 1))
(p ":restart-long" #(restart-agent 5 1))
(p ":agent-error-arity-0" #(agent-error))
(p ":agent-error-nil" #(agent-error nil))
(p ":agent-error-long" #(agent-error 5))
(p ":agent-error-healthy" #(agent-error (agent 1)))

;; --- await (agent_await_rows) ---
(p ":await-multi" #(let [a (agent 0) b (agent 10)] (send a inc) (send b inc) (await a b) [@a @b]))
(p ":await-none" #(await))
(p ":awaitfor-none" #(await-for 50))
(p ":awaitfor-ok" #(let [a (agent 0)] (send a inc) [(await-for 5000 a) @a]))
(p ":awaitfor-gated" #(let [p (promise) a (agent 0)] (send a (fn [s] @p :never)) [(await-for 50 a) (do (deliver p :go) (await a) @a)]))
(p ":awaitfor-neg" #(let [p (promise) a (agent 0)] (send a (fn [s] @p :never)) (let [r (await-for -5 a)] (deliver p :go) r)))
(p ":awaitfor-neg-idle" #(await-for -5 (agent 0)))
(p ":awaitfor-double" #(await-for 5000.9 (agent 0)))
(p ":await-in-action-plain" #(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await a) (catch RuntimeException e :runtime) (catch Exception e :plain))) s)) @p))
(p ":await-none-in-action-msg" #(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await) (catch Exception e (ex-message e)))) s)) @p))
(p ":awaitfor-in-action-msg" #(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (try (await-for 50 a) (catch Exception e (ex-message e)))) s)) @p))
(p ":await-long" #(await 1))
(p ":await-nil" #(await nil))
(p ":awaitfor-str-long" #(await-for "50" 1))
(p ":awaitfor-str-agent" #(await-for "50" (agent 0)))
(p ":awaitfor-nil-ms" #(await-for nil (agent 0)))
(p ":awaitfor-arity-0" #(await-for))
(p ":await-second-failed" #(let [a (agent 0)] (await a (failed))))

;; --- nested sends (agent_nested_rows, agent_nested_discarded_on_failure) ---
(p ":nest-single" #(let [log (atom []) p (promise) a (agent 0)] (send a (fn [s] (send a (fn [s2] (swap! log conj :inner) s2)) (swap! log conj :outer) (deliver p :sent) s)) @p (await a) @log))
(p ":nest-double" #(let [log (atom []) p (promise) a (agent 0)] (send a (fn [s] (send a (fn [s2] (send a (fn [s3] (swap! log conj :in3) (deliver p :d) s3)) (swap! log conj :in2) s2)) (swap! log conj :out) s)) @p (await a) @log))
(p ":nest-two" #(let [log (atom []) p (promise) a (agent 0)] (send a (fn [s] (send a (fn [x] (swap! log conj :n1) x)) (send a (fn [x] (swap! log conj :n2) x)) (swap! log conj :o) (deliver p :d) s)) @p (await a) @log))
(p ":nest-dropped-fail" #(let [log (atom []) a (agent 0)] (send a (fn [s] (send a (fn [s2] (swap! log conj :nested) s2)) (throw (ex-info "boom" {})))) (latched a) (try (await a) (catch RuntimeException _ nil)) (restart-agent a 0) (await a) [@a @log]))
(p ":nest-dropped-continue" #(let [log (atom []) a (agent 0 :error-mode :continue)] (send a (fn [s] (send a (fn [s2] (swap! log conj :nested) s2)) (throw (ex-info "boom" {})))) (await a) [@a @log]))

;; --- validator in action (agent_validator_rows) ---
(p ":validator-continue" #(let [a (agent 0 :validator even? :error-mode :continue)] (send a (fn [s] 3)) (await a) [@a (agent-error a)]))
(p ":validator-fail-latch" #(let [a (agent 0 :validator even?)] (send a (fn [s] 3)) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":validator-fail-detail" #(let [a (agent 0 :validator even?)] (send a (fn [s] 3)) (latched a) [@a (ex-message (agent-error a))]))
(p ":validator-fail-class" #(let [a (agent 0 :validator even?)] (send a (fn [s] 3)) (latched a) (str (class (agent-error a)))))
(p ":validator-throw-continue" #(let [a (agent 0 :validator (fn [x] (if (zero? x) true (throw (ex-info "vv" {})))) :error-mode :continue)] (send a inc) (await a) [@a (agent-error a)]))

;; --- agent watches (agent_watch_rows; barrier-full-log in :shutdown-join-all) ---
(p ":watch-agent-fire" #(let [log (atom []) a (agent 0)] (add-watch a :k (fn [k r o n] (swap! log conj [k (= r a) o n]))) (send a inc) (await a) (first @log)))
(p ":watch-agent-same" #(let [log (atom []) a (agent 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (send a identity) (await a) (first @log)))
(p ":watch-agent-removed" #(let [log (atom []) a (agent 0)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (remove-watch a :k) (send a inc) (await a) @log))
(p ":watch-agent-add-eq" #(let [a (agent 1)] (= a (add-watch a :k (fn [k r o n])))))
(p ":watch-agent-rem-eq" #(let [a (agent 1)] (add-watch a :k (fn [k r o n])) (= a (remove-watch a :k))))
(p ":watch-agent-nil-silent" #(let [log (atom []) a (agent 0)] (add-watch a :k nil) (add-watch a :j (fn [k r o n] (swap! log conj :j))) (send a inc) (await a) (first @log)))
(p ":watch-throw-continue" #(let [a (agent 1 :error-mode :continue)] (add-watch a :k (fn [k r o n] (throw (ex-info "w" {})))) (send a inc) (await a) [@a (agent-error a)]))
(p ":watch-throw-fail" #(let [a (agent 1)] (add-watch a :k (fn [k r o n] (throw (ex-info "w" {})))) (send a inc) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":watch-throw-fail-state" #(let [a (agent 1)] (add-watch a :k (fn [k r o n] (throw (ex-info "w" {})))) (send a inc) (latched a) [@a (nil? (agent-error a))]))

;; --- *agent* (agent_star_rows) ---
(p ":star-nil-outside" #(nil? *agent*))
(p ":star-bound-in-action" #(let [p (promise) a (agent 0)] (send a (fn [s] (deliver p (= *agent* a)) s)) @p))
(p ":star-nested-own" #(let [p (promise) a (agent 0) b (agent 0)] (send a (fn [s] (send b (fn [t] (deliver p [(= *agent* b) (= *agent* a)]) t)) s)) @p))
(p ":star-conveys" #(do (def ^:dynamic s6c-dx 1) (let [p (promise) a (agent 0)] (binding [s6c-dx 2] (send a (fn [s] (deliver p s6c-dx) s))) @p)))
(p ":star-watcher-bare" #(let [p (promise) a (agent 0)] (add-watch a :k (fn [k r o n] (deliver p (nil? *agent*)))) (send a inc) @p))
;; --- send shapes (agent_send_shapes) ---
(p ":send-long" #(send 1 inc))
(p ":sendoff-long" #(send-off 1 inc))
(p ":sendvia-long" #(send-via nil 1 inc))
(p ":send-nil" #(send nil inc))
(p ":sendoff-nil" #(send-off nil inc))
(p ":sendvia-nil" #(send-via nil nil inc))
(p ":send-arity-1" #(send (agent 1)))
(p ":sendoff-arity-1" #(send-off (agent 1)))
(p ":sendvia-arity-2" #(send-via nil (agent 1)))
(p ":send-extra" #(let [a (agent 0)] (send a + 1 2 3) (await a) @a))
(p ":sendoff-extra" #(let [a (agent 0)] (send-off a + 1 2) (await a) @a))
(p ":send-badfn-latch" #(let [a (agent 0)] (send a 5) (latched a) [(str (class (agent-error a))) (ex-message (agent-error a)) @a]))
(p ":send-nilfn-latch" #(let [a (agent 0)] (send a nil) (latched a) [(str (class (agent-error a))) (ex-message (agent-error a)) @a]))

;; --- Error in action, JVM analogue of a panicking worker (agent_panic_rows) ---
(p ":error-action-latches" #(let [a (agent 0)] (send a (fn [s] (throw (Error. "kablam")))) (latched a) [(str (class (agent-error a))) (ex-message (agent-error a)) @a]))
(p ":error-continue" #(let [a (agent 0 :error-mode :continue)] (send a (fn [s] (throw (Error. "k2")))) (await a) [@a (agent-error a)]))

;; --- agent value shapes (agent_value_shapes) ---
(p ":deref-agent" #(deref (agent 5)))
(p ":deref-agent-reader" #(@(agent 5)))
(p ":deref-agent-timeout" #(deref (agent 1) 50 :t))
(p ":force-agent-eq" #(let [a (agent 1)] (= a (force a))))
(p ":futureq-agent" #(future? (agent 1)))
(p ":alter-meta-agent" #(alter-meta! (agent 1) assoc :a 1))
(p ":reset-meta-agent" #(reset-meta! (agent 1) {:a 1}))
(p ":alter-meta-agent-read" #(let [a (agent 1)] (alter-meta! a assoc :a 1) (meta a)))
(p ":withmeta-agent" #(with-meta (agent 1) {:a 1}))
(p ":varymeta-agent" #(vary-meta (agent 1) assoc :a 1))
(p ":printmeta-agent-bool" #(.startsWith ^String (binding [*print-meta* true] (pr-str (agent 1 :meta {:a 1}))) "^{:a 1} #object[clojure.lang.Agent 0x"))
(p ":seq-agent" #(seq (agent 1)))
(p ":first-agent" #(first (agent 1)))
(p ":count-agent" #(count (agent 1)))
(p ":nth-agent" #(nth (agent 1) 0))
(p ":coll-agent" #(coll? (agent 1)))
(p ":seq?-agent" #(seq? (agent 1)))
(p ":counted-agent" #(counted? (agent 1)))
(p ":fn-agent" #(fn? (agent 1)))
(p ":ifn-agent" #(ifn? (agent 1)))
(p ":map-agent" #(map? (agent 1)))
(p ":vector-agent" #(vector? (agent 1)))
(p ":var-agent" #(var? (agent 1)))
(p ":get-agent" #(get (agent {:a 1}) :a))
(p ":find-agent" #(find (agent {}) :a))
(p ":conj-agent" #(conj (agent 1) 2))
(p ":compare-agent" #(compare (agent 1) 1))
(p ":invoke-agent" #((agent 1)))
(p ":deliver-agent" #(deliver (agent 1) 2))

;; --- stress (agent_stress) ---
(p ":stress-send" #(do (def s6c-n 8) (def s6c-m 250) (def s6c-as (vec (map (fn [_] (agent 0)) (range s6c-n)))) (doseq [a s6c-as] (dotimes [_ s6c-m] (send a inc))) (doseq [a s6c-as] (await a)) (vec (map deref s6c-as))))
(p ":stress-sendoff" #(do (def s6c-bs (vec (map (fn [_] (agent 0)) (range s6c-n)))) (doseq [a s6c-bs] (dotimes [_ s6c-m] (send-off a inc))) (doseq [a s6c-bs] (await a)) (vec (map deref s6c-bs))))

;; --- error-handler setter (set_error_handler_rows) ---
(p ":seth-install" #(set-error-handler! (agent 0 :error-mode :continue) (fn [a e] :h)))
(p ":seth-clear" #(set-error-handler! (agent 0 :error-mode :continue) nil))
(p ":seth-fires" #(let [hp (promise) a (agent 0 :error-mode :continue)] (set-error-handler! a (fn [ag ex] (deliver hp [(= ag a) (ex-message ex)]))) (send a (fn [s] (throw (ex-info "sh" {})))) (send a inc) [@hp (await a) @a (agent-error a)]))
(p ":seth-silent" #(let [hp2 (promise) a (agent 0 :error-mode :continue)] (set-error-handler! a (fn [ag ex] (deliver hp2 :fired))) (set-error-handler! a nil) (send a (fn [s] (throw (ex-info "sh2" {})))) (send a inc) [(await a) @a (agent-error a) (realized? hp2)]))
(p ":seth-nil" #(set-error-handler! nil (fn [a e])))
(p ":seth-long" #(set-error-handler! 42 (fn [a e])))
(p ":seth-badfn" #(set-error-handler! (agent 0) 42))
(p ":seth-arity-1" #(set-error-handler! (agent 0)))

;; --- error-mode setter (set_error_mode_rows) ---
(p ":setm-continue" #(set-error-mode! (agent 0) :continue))
(p ":setm-continue-clears" #(let [a (agent 0)] (set-error-mode! a :continue) (send a (fn [s] (throw (ex-info "x" {})))) (send a inc) [(await a) @a (agent-error a)]))
(p ":setm-fail-latches" #(let [a (agent 0)] (set-error-mode! a :continue) (send a (fn [s] (throw (ex-info "x" {})))) (send a inc) (await a) (set-error-mode! a :fail) (send a (fn [s] (throw (ex-info "y" {})))) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":setm-fail-parked" #(let [a (agent 0)] (set-error-mode! a :fail) (send a (fn [s] (throw (ex-info "y" {})))) (latched a) [(nil? (agent-error a)) @a]))
(p ":setm-bogus-fails" #(let [a (agent 0)] (set-error-mode! a :bogus) (send a (fn [s] (throw (ex-info "z" {})))) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":setm-nil-fails" #(let [a (agent 0)] (set-error-mode! a nil) (send a (fn [s] (throw (ex-info "w" {})))) (latched a) (try (await a) (catch RuntimeException e :failed))))
(p ":setm-nil-ok" #(set-error-mode! (agent 0) nil))
(p ":setm-nil-agn" #(set-error-mode! nil :continue))
(p ":setm-long" #(set-error-mode! 42 :continue))
(p ":setm-str" #(set-error-mode! (agent 0) "continue"))
(p ":setm-arity-1" #(set-error-mode! (agent 0)))

;; --- pre-shutdown latch proofs (agents failed at load) ---
(p ":rs-failed" #(do (latched rs) (try (await rs) (catch RuntimeException e :failed))))
(p ":rs2-failed" #(do (latched rs2) (try (await rs2) (catch RuntimeException e :failed))))

;; --- shutdown joins everything (agent_shutdown_joins + barrier log) ---
;; NOTE: bare `(future-done? f)` after shutdown is RACY on the JVM (true,
;; then false across identical runs — shutdown does not join future threads,
;; unlike the port). The entry derefs after shutdown instead (deterministic).
(p ":shutdown-join-all" #(let [log (atom []) a (agent 5) p (promise) q (promise) b (agent 0) c (agent 0) r (promise) f (future @r :done)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (send a inc) (await a) (send b (fn [s] @p 1)) (send-off c (fn [s] @q 2)) (deliver p :go) (deliver q :go) (deliver r :go) (shutdown-agents) [@log @b @c @f (future-done? f)]))

;; --- post-shutdown (agent_shutdown_rows) ---
(p ":shutdown-again" #(shutdown-agents))
(p ":shutdown-arity-1" #(shutdown-agents 1))
(p ":shutdown-send-kept" #(let [a (agent 9)] [(= a (send a inc)) @a]))
(p ":shutdown-sendoff-kept" #(let [a (agent 9)] [(= a (send-off a inc)) @a]))
(p ":shutdown-sendvia-kept" #(let [a (agent 9)] [(= a (send-via nil a inc)) @a]))
(p ":shutdown-awaitfor" #(await-for 100 (agent 9)))
(p ":shutdown-future-submit-class" #(try (future 1) :submitted (catch java.util.concurrent.RejectedExecutionException e (str (class e)))))

;; --- restarts after shutdown (agent_restart_after_shutdown, _resumed_) ---
(p ":rs-restart" #(restart-agent rs 10))
(p ":rs-state" #(do (Thread/sleep 200) [@rs (agent-error rs)]))
(p ":rs2-restart" #(restart-agent rs2 5))
(p ":rs2-state" #(do (Thread/sleep 200) [@rs2 (agent-error rs2)]))

(println "S6-04 DONE")
;; Pools resurrected by post-shutdown pins must die or the oracle JVM lingers.
(shutdown-agents)
