;; S10 oracle probes: taps (oracle: alpha10 jars, issue #53).
;; Run from repo root (/home/hhu/workspace/clojure-s10):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s10-cells/probes/taps.clj > docs/zero-oos-s10-cells/probes/taps.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; ---- add/remove return nil; tap> true with room (even tap-less) ----
(throwof "t-no-taps" '(tap> :lonely))
(throwof "t-add-ret" '(do (def t1 (fn [_] nil)) (let [r (add-tap t1)] (remove-tap t1) r)))
(throwof "t-add-nonfn-ret" '(do (add-tap 42) (remove-tap 42)))
(throwof "t-remove-ret" '(do (def fr (fn [_] nil)) (add-tap fr) [(remove-tap fr) (remove-tap fr)]))
(throwof "t-remove-never" '(remove-tap (fn [_] nil)))
;; ---- delivery roundtrip ----
(throwof "t-deliver" '(do (def p1 (promise)) (def f1 (fn [x] (deliver p1 x))) (add-tap f1) (tap> :x) (let [v (deref p1 5000 :TIMEOUT)] (remove-tap f1) v)))
(throwof "t-tap-ret-with-tap" '(do (def fw (fn [_] nil)) (add-tap fw) (let [r (tap> :w)] (remove-tap fw) r)))
(throwof "t-thread" '(do (def pn (promise)) (def fn1 (fn [_] (deliver pn (.getName (Thread/currentThread))))) (add-tap fn1) (tap> :t) (let [v (deref pn 5000 :TIMEOUT)] (remove-tap fn1) v)))
(throwof "t-nil" '(do (def pn2 (promise)) (def fn2 (fn [x] (deliver pn2 [:got x]))) (add-tap fn2) (tap> nil) (let [v (deref pn2 5000 :TIMEOUT)] (remove-tap fn2) v)))
(throwof "t-add-twice" '(do (def c1 (atom 0)) (def fc (fn [_] (swap! c1 inc))) (add-tap fc) (add-tap fc) (tap> :once) (Thread/sleep 500) (remove-tap fc) @c1))
(throwof "t-removed-misses" '(do (def p3 (promise)) (def f3 (fn [x] (deliver p3 x))) (add-tap f3) (remove-tap f3) (tap> :missed) (deref p3 300 :TIMEOUT)))
(throwof "t-throw-safe" '(do (def p4 (promise)) (def boom (fn [_] (throw (ex-info "boom" {})))) (def f4 (fn [x] (deliver p4 x))) (add-tap boom) (add-tap f4) (tap> :after-boom) (let [v (deref p4 5000 :TIMEOUT)] (remove-tap boom) (remove-tap f4) v)))
;; ---- tap-> (1.13): returns x, still delivers ----
(throwof "t-tap->-ret" '(tap-> 5))
(throwof "t-tap->-delivers" '(do (def p5 (promise)) (def f5 (fn [x] (deliver p5 x))) (add-tap f5) (let [r (tap-> :threaded) v (deref p5 5000 :TIMEOUT)] (remove-tap f5) [r v])))
(throwof "t-tap->-nil" '(do (def p6 (promise)) (def f6 (fn [x] (deliver p6 [:got x]))) (add-tap f6) (let [r (tap-> nil) v (deref p6 5000 :TIMEOUT)] (remove-tap f6) [r v])))
(throwof "t-tap->-false" '(do (def p7 (promise)) (def f7 (fn [x] (deliver p7 [:got x]))) (add-tap f7) (let [r (tap-> false) v (deref p7 5000 :TIMEOUT)] (remove-tap f7) [r v])))
;; ---- full queue: block the consumer, flood, count (time-boxed ~1s; LAST) ----
(throwof "t-full" '(do (def gate (promise)) (def slow (fn [_] @gate)) (add-tap slow) (tap> :blocker) (Thread/sleep 500) (let [rs (doall (repeatedly 1500 #(tap> :flood))) trues (count (filter true? rs)) falses (count (filter false? rs))] (deliver gate :go) (Thread/sleep 500) (remove-tap slow) [trues falses])))
