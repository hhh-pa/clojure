;; S6 differential corpus 02: delays (delay/force/realized?/throw-caching/memoize).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s6-pins.md P1, concurrency.rs delay_rows, delay_throw_cached,
;; realized_rows/_errors, force_rows, memoize_rows, deref_rows (delay parts),
;; meta_rows + print_str_rows (delay parts), agent_value_shapes (:realized-agent).
;; Self-force StackOverflowError is NOT pinned (JVM stderr warning is fragile;
;; the port diverges with IllegalStateException anyway). Every entry is
;; self-contained; never prints raw refs.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- deref delay parts (deref_rows; atom/var parts in 01) ---
(p ":deref-delay" #(deref (delay 42)))
(p ":deref-delay-reader" #(do @(delay 42)))

;; --- delay rows (delay_rows) ---
(p ":force-delay" #(force (delay 42)))
(p ":delay-body-once" #(do (def dc (atom 0)) (def dd (delay (swap! dc inc))) (deref dd) (force dd) @dc))
(p ":delay-multi" #(deref (delay 1 2)))
(p ":delay-empty" #(deref (delay)))
(p ":delay-nil-realized" #(let [d (delay nil)] [(deref d) (realized? d)]))
(p ":delay-lexical" #(let [x 40] (force (delay (+ x 2)))))
(p ":delay-dynamic" #(do (def ^:dynamic dfd 1) (let [d (delay dfd)] (binding [dfd 2] (force d)))))
(p ":delay-class" #(str (class (delay 1))))
(p ":delaystar-shadow" #(let [delay* 5] delay*))

;; --- throw caching (delay_throw_cached) ---
(p ":delay-throw-cached" #(do (def dtc (atom 0)) (def dtd (delay (swap! dtc inc) (throw (ex-info "boom" {:n 1})))) [(try (deref dtd) (catch clojure.lang.ExceptionInfo e :e1)) (try (force dtd) (catch clojure.lang.ExceptionInfo e :e2)) @dtc (realized? dtd)]))
(p ":delay-throw-reraise" #(do (def dte (delay (throw (ex-info "boom" {:n 7})))) (try (force dte) (catch clojure.lang.ExceptionInfo e :x)) (try (force dte) (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)]))))
(p ":delay-throw-uncaught" #(force (delay (throw (ex-info "boom" {})))))

;; --- realized? rows, delay/lazy parts (realized_rows; future/promise in 03) ---
(p ":realized-delay-false" #(realized? (delay 1)))
(p ":realized-delay-true" #(let [d (delay 1)] (force d) (realized? d)))
(p ":realized-lazy-false" #(realized? (map inc [1 2 3])))
(p ":realized-lazy-after-first" #(let [s (map inc [1])] (first s) (realized? s)))

;; --- realized? errors (realized_errors) ---
(p ":realized-atom" #(realized? (atom 1)))
(p ":realized-long" #(realized? 1))
(p ":realized-vec" #(realized? [1]))
(p ":realized-nil" #(realized? nil))
(p ":realized-arity-0" #(realized?))
(p ":realized-arity-2" #(realized? (delay 1) 2))
(p ":realized-agent" #(realized? (agent 1)))

;; --- force rows (force_rows) ---
(p ":force-42" #(force 42))
(p ":force-nil" #(force nil))
(p ":force-str" #(force "s"))
(p ":force-atom-passthrough" #(let [a (atom 1)] (reset! (force a) 9) @a))
(p ":force-arity-0" #(force))
(p ":force-arity-2" #(force 1 2))

;; --- memoize (memoize_rows) ---
(p ":memo-basic" #(let [c (atom 0) f (memoize (fn [x] (swap! c inc) (* x 2)))] [(f 21) (f 21) @c (f 1) @c]))
(p ":memo-multi-arity" #(let [f (memoize +)] [(f 1 2) (f 1 2 3)]))
(p ":memo-nullary" #(let [c (atom 0) f (memoize (fn [] (swap! c inc)))] [(f) (f) @c]))
(p ":memo-nil" #(let [c (atom 0) f (memoize (fn [x] (swap! c inc) nil))] [(f 1) (f 1) @c]))
(p ":memo-fn" #(fn? (memoize +)))
(p ":memo-arity" #(try (memoize) (catch clojure.lang.ArityException e :arity)))

;; --- delay meta (meta_rows, delay parts; atom parts in 01) ---
(p ":meta-delay-nil" #(meta (delay 1)))
(p ":withmeta-delay" #(with-meta (delay 1) {:a 1}))
(p ":altermeta-delay" #(alter-meta! (delay 1) assoc :a 1))
(p ":printmeta-delay-bool" #(.startsWith ^String (binding [*print-meta* true] (pr-str (delay 1))) "#object[clojure.lang.Delay 0x"))

;; --- delay print/equality/predicates (print_str_rows, delay parts) ---
(p ":pr-delay-bool" #(.startsWith ^String (pr-str (delay 1)) "#object[clojure.lang.Delay 0x"))
(p ":str-delay-bool" #(.startsWith ^String (str (delay 1)) "clojure.lang.Delay@"))
(p ":pr-noforce" #(do (def pd (atom 0)) (def pdd (delay (swap! pd inc))) (pr-str pdd) @pd))
(p ":delay-eq-self" #(let [d (delay 1)] (= d d)))
(p ":delay-eq-fresh" #(= (delay 1) (delay 1)))
(p ":delay-hash-int" #(integer? (hash (delay 1))))
(p ":seq-delay" #(seq (delay 1)))
(p ":count-delay" #(count (delay 1)))
(p ":counted-delay" #(counted? (delay 1)))
(p ":ifn-delay" #(ifn? (delay 1)))

(println "S6-02 DONE")
