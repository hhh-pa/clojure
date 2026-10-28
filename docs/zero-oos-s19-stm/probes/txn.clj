;; S19 oracle probes: dosync, alter/commute/ref-set/ensure, sync/io!, send deferral (oracle: alpha10 jars, issue #63).
;; Run from repo root (/home/hhu/workspace/clojure-s19):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s19-stm/probes/txn.clj > docs/zero-oos-s19-stm/probes/txn.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(defn normsym [s]
  (-> s
      (clojure.string/replace #"G__\d+" "G__N")
      (clojure.string/replace #"__\d+__auto__" "__N__auto__")))
(defn showof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn shapeof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normsym (normhex (pr-str v))))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; ---- dosync basics ----
(throwof "t-body-value" '(dosync 1 2))
(throwof "t-empty" '(dosync))
(throwof "t-commit" '(let [r (ref 0) s (ref 10)] (dosync (alter r inc) (alter s + 5)) [@r @s]))
(throwof "t-atomic-abort" '(let [r (ref 0) s (ref 10)] [(try (dosync (alter r inc) (throw (Exception. "boom"))) :committed (catch Exception e :caught)) @r @s]))
(throwof "t-deref-local" '(let [r (ref 0)] (dosync (alter r (constantly 5)) @r)))
(throwof "t-retry-invisible" '(let [c (atom 0)] [(dosync (swap! c inc) :done) @c]))
;; ---- alter / ref-set / commute / ensure returns ----
(throwof "t-alter-returns" '(dosync (alter (ref 0) + 1 2)))
(throwof "t-refset-returns" '(dosync (ref-set (ref 0) 9)))
(throwof "t-commute-returns" '(dosync (commute (ref 0) + 5)))
(throwof "t-commute-reruns" '(let [c (atom 0) r (ref 0)] (dosync (commute r (fn [x] (swap! c inc) (+ x 1)))) [@r @c]))
(throwof "t-commute-order" '(let [log (atom []) r (ref 0)] (dosync (commute r (fn [x] (swap! log conj :f) (+ x 1))) (commute r (fn [x] (swap! log conj :g) (* x 10)))) [@r @log]))
(throwof "t-commute-alter" '(let [r (ref 0)] (dosync (alter r + 10) (commute r + 1)) @r))
(throwof "t-commute-alter-rev" '(let [r (ref 0)] (dosync (commute r + 1) (alter r + 10)) @r))
(throwof "t-commute-refset" '(let [r (ref 0)] (dosync (ref-set r 10) (commute r + 1)) @r))
(throwof "t-commute-refset-rev" '(let [r (ref 0)] [(try (dosync (commute r + 1) (ref-set r 10)) :c (catch IllegalStateException e (.getMessage e))) @r]))
(throwof "t-commute-set-immediate" '(let [r (ref 0) c (atom 0)] [(try (dosync (commute r + 1) (alter r + 10) (swap! c inc)) :c (catch IllegalStateException e :t)) @c @r]))
(throwof "t-commute-abort-atomic" '(let [r (ref 0) s (ref 0)] [(try (dosync (commute r + 1) (alter r + 10) (alter s + 100)) :c (catch IllegalStateException e :t)) @r @s]))
(throwof "t-commute-ensure" '(let [r (ref 0)] (dosync (ensure r) (commute r + 1)) @r))
(throwof "t-ensure-commute" '(let [r (ref 0)] (dosync (commute r + 1) (ensure r))))
(throwof "t-ensure-returns" '(dosync (ensure (ref 7))))
;; ---- nesting ----
(throwof "t-nested" '(let [r (ref 0)] [(dosync (alter r inc) (dosync (alter r inc))) @r]))
(throwof "t-nested-join" '(let [c (atom 0) r (ref 0)] (add-watch r :k (fn [_ _ _ _] (swap! c inc))) (dosync (alter r inc) (dosync (alter r inc))) @c))
;; ---- validator / watch timing ----
(throwof "t-validator-calls" '(let [calls (atom []) r (ref 0 :validator (fn [x] (swap! calls conj x) true))] (dosync (alter r inc) (alter r inc)) @calls))
(throwof "t-watch-commit" '(let [seen (atom []) r (ref 0)] (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (dosync (alter r inc) (alter r inc)) @seen))
(throwof "t-watch-order" '(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (dosync (alter r inc)) @seen))
(throwof "t-watch-order-atom" '(let [seen (atom []) a (atom 0)] (add-watch a :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch a :b (fn [_ _ _ _] (swap! seen conj :b))) (swap! a inc) @seen))
(throwof "t-watch-abort" '(let [seen (atom []) r (ref 0)] (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (try (dosync (alter r inc) (throw (Exception. "boom"))) (catch Exception e :caught)) @seen))
(throwof "t-addwatch-in-txn" '(let [seen (atom []) r (ref 0)] (dosync (add-watch r :k (fn [_ _ o n] (swap! seen conj [o n]))) (alter r inc)) @seen))
(throwof "t-setvalidator-in-txn" '(let [r (ref 0)] [(try (dosync (set-validator! r even?) (alter r (constantly 3))) :committed (catch Throwable t [:thrown (.getName (class t)) (.getMessage t)])) @r]))
;; ---- send deferral ----
(throwof "t-send-defer" '(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send a (fn [_] @r))) (await a) [@r @a]))
(throwof "t-send-returns-agent" '(let [a (agent 0)] (dosync (identical? (send a identity) a))))
(throwof "t-sendoff-defer" '(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send-off a (fn [_] @r))) (await a) [@r @a]))
(throwof "t-send-abort" '(let [r (ref 0) a (agent 0)] [(try (dosync (alter r (constantly 5)) (send a (fn [_] 99)) (throw (Exception. "boom"))) :committed (catch Exception e :caught)) (do (await a) [@r @a])]))
(throwof "t-sendvia-defer" '(let [r (ref 0) a (agent 0)] (dosync (alter r (constantly 5)) (send-via clojure.lang.Agent/pooledExecutor a (fn [_] @r))) (await a) [@r @a]))
(throwof "t-send-badagent-in-txn" '(dosync (send :x (fn [_] 1))))
(throwof "t-await-noargs-in" '(dosync (await)))
(throwof "t-await-noargs-out" '(await))
;; ---- io! / sync ----
(throwof "t-io-in" '(dosync (io! :x)))
(throwof "t-io-out" '(io! :x))
(throwof "t-io-multi" '(io! 1 2 3))
(throwof "t-io-empty" '(io!))
(throwof "t-sync-in" '(let [r (ref 0)] (dosync (sync nil (alter r inc)) @r)))
(throwof "t-sync-out" '(sync nil :x))
(throwof "t-sync-empty" '(sync nil))
(throwof "t-sync-arity0" '(sync))
(throwof "t-sync-flags-ignored" '(let [c (atom 0)] [(sync (swap! c inc) :body) @c]))
(throwof "t-sync-flags-need-not-be-nil" '(sync :whatever :x))
(throwof "t-sync-runs-tx" '(sync nil (io! :x)))
(throwof "t-watch-order3" '(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (dosync (alter r inc)) @seen))
(throwof "t-watch-order-shuffled" '(let [seen (atom []) r (ref 0)] (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (dosync (alter r inc)) @seen))
(throwof "t-watch-shuffled2" '(let [seen (atom []) r (ref 0)] (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (dosync (alter r inc)) @seen))
(throwof "t-watch-keys4" '(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r :b (fn [_ _ _ _] (swap! seen conj :b))) (add-watch r :c (fn [_ _ _ _] (swap! seen conj :c))) (add-watch r :d (fn [_ _ _ _] (swap! seen conj :d))) (dosync (alter r inc)) @seen))
(throwof "t-watch-strkeys" '(let [seen (atom []) r (ref 0)] (add-watch r "a" (fn [_ _ _ _] (swap! seen conj "a"))) (add-watch r "b" (fn [_ _ _ _] (swap! seen conj "b"))) (add-watch r "c" (fn [_ _ _ _] (swap! seen conj "c"))) (dosync (alter r inc)) @seen))
(throwof "t-watch-mixedkeys" '(let [seen (atom []) r (ref 0)] (add-watch r :a (fn [_ _ _ _] (swap! seen conj :a))) (add-watch r 1 (fn [_ _ _ _] (swap! seen conj 1))) (add-watch r "s" (fn [_ _ _ _] (swap! seen conj "s"))) (dosync (alter r inc)) @seen))
(throwof "t-watch-samefn" '(let [seen (atom []) r (ref 0) f (fn [k _ _ _] (swap! seen conj k))] (add-watch r :a f) (add-watch r :b f) (add-watch r :c f) (dosync (alter r inc)) @seen))
(throwof "t-watch-keys8" '(let [seen (atom []) r (ref 0)] (doseq [k [:k1 :k2 :k3 :k4 :k5 :k6 :k7 :k8]] (add-watch r k (fn [_ _ _ _] (swap! seen conj k)))) (dosync (alter r inc)) @seen))
(throwof "t-key-hashes" '[(hash :a) (hash :b) (hash :c) (hash :d) (hash :k1) (hash :k2) (hash :k3) (hash :k4) (hash :k5) (hash :k6) (hash :k7) (hash :k8)])
;; ---- conveyance ----
(throwof "t-bound-fn" '(let [r (ref 0)] (dosync ((bound-fn [] (alter r inc)))) @r))
(throwof "t-future-no-convey" '(let [r (ref 0)] (dosync (let [p (promise)] (future (deliver p (try (alter r inc) :ok (catch IllegalStateException e :no-tx)))) (deref p 5000 :timeout)))))
;; ---- misc ----
(throwof "t-await-in-dosync" '(dosync (await (agent 0)) :waited))
(throwof "t-awaitfor-in-dosync" '(dosync (await-for 100 (agent 0)) :waited))
(throwof "t-await-action-tx" '(let [p (promise) a (agent 0) a2 (agent 0)] (send a (fn [_] (deliver p (try (dosync (await a2)) :waited (catch IllegalStateException e (.getMessage e)))) 0)) (deref p 5000 :timeout)))
(throwof "t-awaitfor-action-tx" '(let [p (promise) a (agent 0) a2 (agent 0)] (send a (fn [_] (deliver p (try (dosync (await-for 100 a2)) :waited (catch IllegalStateException e (.getMessage e)))) 0)) (deref p 5000 :timeout)))
(throwof "t-invoke-in-txn" '(let [r (ref 0)] (dosync (ref-set r (fn [x] (* x 10))) (r 5))))
(throwof "t-addwatch-returns" '(let [r (ref 0)] (identical? (add-watch r :k (fn [& _])) r)))
(throwof "t-removewatch-returns" '(let [r (ref 0)] (add-watch r :k (fn [& _])) [(identical? (remove-watch r :k) r) (dosync (alter r inc))]))
;; ---- macro shapes ----
(shapeof "t-shape-dosync" '(macroexpand-1 (quote (dosync (alter r inc)))))
(shapeof "t-shape-sync" '(macroexpand-1 (quote (sync nil (alter r inc)))))
(shapeof "t-shape-io" '(macroexpand-1 (quote (io! :x))))
(shapeof "t-shape-io-multi" '(macroexpand-1 (quote (io! 1 2 3))))
(shutdown-agents)
