;; S19 oracle probes: portside pins + the late-commit commute corner (oracle: alpha10 jars, issue #63).
;; Run from repo root (/home/hhu/workspace/clojure-s19):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s19-stm/probes/portside.clj > docs/zero-oos-s19-stm/probes/portside.out
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
;; ---- failed-agent send position ----
(throwof "u-send-failed-pos" '(let [a (agent 0) c (atom 0)] (send a (fn [_] (throw (Exception. "boom")))) (try (await-for 2000 a) (catch Throwable _ :failed)) [(try (dosync (send a inc) (swap! c inc) :ok) (catch RuntimeException e (ex-message e))) @c]))
;; ---- meta fns on refs ----
(throwof "u-altermeta" '(let [r (ref 0 :meta {:a 1})] [(alter-meta! r assoc :b 2) (meta r)]))
(throwof "u-resetmeta" '(let [r (ref 0 :meta {:a 1})] [(reset-meta! r {:c 3}) (meta r)]))
(throwof "u-varymeta" '(let [r (ref 0 :meta {:a 1})] (vary-meta r assoc :b 2)))
;; ---- recur fences ----
(throwof "u-recur-across" '(try (eval '(loop [i 0] (if (< i 3) (dosync (recur (inc i))) i))) :compiled (catch Throwable t [:thrown (.getName (class t)) (.getName (class (.getCause t))) (.getMessage (.getCause t))])))
(throwof "u-recur-sync-zero" '(try (eval '(fn [] (loop [] (dosync (recur))))) :compiled (catch Throwable t [:thrown (.getName (class t)) (.getMessage t)])))
(throwof "u-recur-inside" '(dosync (loop [i 0] (if (< i 3) (recur (inc i)) i))))
(throwof "u-recur-io" '(loop [i 0] (if (< i 3) (io! (recur (inc i))) i)))
;; ---- await order ----
(throwof "u-awaitfor-arity-in" '(dosync (await-for)))
(throwof "u-await-badarg-in" '(dosync (await 5)))
;; ---- throwing validators / watches ----
(throwof "u-validator-throw" '(let [r (ref 0 :validator (fn [x] (if (zero? x) true (throw (Exception. "vboom")))))] [(try (dosync (alter r inc)) :committed (catch IllegalStateException e [:thrown (ex-message e) (some? (ex-cause e))])) @r]))
(throwof "u-watch-throw" '(let [r (ref 0)] (add-watch r :k (fn [& _] (throw (Exception. "wboom")))) [(try (dosync (alter r inc)) :committed (catch Exception e [:thrown (ex-message e)])) @r]))
;; ---- history conversion / accessor arity ----
(throwof "u-min-hugeratio" '(let [r (ref 0)] (ref-min-history r 9999999999999999999999/1) (ref-min-history r)))
(throwof "u-min-arity-vs-recv" '(ref-min-history 5 1 2))
;; ---- sync flags / ensure / commute throw / refset deref ----
(throwof "u-sync-flags-only" '(sync :whatever))
(throwof "u-ensure-only" '(let [r (ref 5)] [(dosync (ensure r)) @r]))
(throwof "u-commute-throw" '(let [r (ref 0)] [(try (dosync (commute r (fn [_] (throw (Exception. "cboom"))))) :c (catch Exception e [:thrown (ex-message e)])) @r]))
(throwof "u-refset-deref" '(let [r (ref 0)] (dosync (ref-set r 42) @r)))
;; ---- set-guard vs alter fn ----
(throwof "u-setaftercommute-throwfn" '(let [r (ref 0) c (atom 0)] [(try (dosync (commute r + 1) (alter r (fn [x] (swap! c inc) (throw (Exception. "fboom"))))) :ok (catch Exception e [:thrown (ex-message e)])) @c @r]))
(throwof "u-setaftercommute-nilfn" '(let [r (ref 0)] (dosync (commute r + 1) (alter r nil))))
;; ---- nil-fn checkcast order ----
(throwof "u-alter-nil-nilfn" '(alter nil nil))
(throwof "u-alter-ref-nilfn" '(alter (ref 0) nil))
(throwof "u-alter-ref-nilfn-in" '(dosync (alter (ref 0) nil)))
(throwof "u-commute-nilfn-in" '(dosync (commute (ref 0) nil)))
;; ---- ref cast beats fn cast ----
(throwof "u-alter-nonref-nonfn" '(alter 1 5))
(throwof "u-commute-nonref-nonfn" '(commute 1 5))
;; ---- negative / crossed bounds ----
(throwof "u-ref-min-neg1-count" '(let [r (ref 0 :min-history -1)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
(throwof "u-ref-max-neg1-count" '(let [r (ref 0 :max-history -1)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
(throwof "u-ref-min2max1-count" '(let [r (ref 0 :min-history 2 :max-history 1)] (dosync (alter r inc)) (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
;; ---- ctor option casts / validator-first ----
(throwof "u-ref-meta-nonmap" '(ref 0 :meta 5))
(throwof "u-ref-validator-nonfn" '(ref 0 :validator 5))
(throwof "u-ref-val-first" '(ref 3 :validator even? :min-history :x))
(throwof "u-ref-val-first2" '(ref 3 :min-history :x :validator even?))
;; ---- rerun-phase writes ----
(throwof "u-rerun-s-watch" '(let [seen (atom []) s (ref 100) r (ref 0)] (add-watch s :k (fn [_ _ o n] (swap! seen conj [o n]))) [(dosync (commute r (fn [x] (alter s + 1) (inc x)))) @seen]))
(throwof "u-rerun-s-veto2" '(let [s (ref 100 :validator (fn [x] (not= x 102))) r (ref 0)] [(try (dosync (commute r (fn [x] (alter s + 1) (inc x)))) :ok (catch IllegalStateException e [:thrown (ex-message e)])) @r @s]))
;; ---- validate-phase writes ----
(throwof "u-valphase-s-veto" '(let [s (ref 100 :validator even?) r (ref 0 :validator (fn [x] (when-not (zero? x) (alter s (constantly 7))) true))] [(try (dosync (alter r inc)) :ok (catch Throwable t [:thrown (ex-message t)])) @r @s]))
(throwof "u-valphase-watch" '(let [seen (atom []) s (ref 100) r (ref 0 :validator (fn [x] (when-not (zero? x) (alter s + 1)) true))] (add-watch s :k (fn [_ _ o n] (swap! seen conj [o n]))) [(dosync (alter r inc)) @seen @s]))
;; ---- late-commit commute (flag b: the JVM fail-fast corner) ----
(throwof "u-latecommute-cme" '(let [c (atom 0) r (ref 0)] [(try (dosync (commute r (fn [x] (swap! c inc) (when (= @c 2) (commute r (fn [y] (+ y 100)))) (inc x)))) :ok (catch Throwable t [:thrown (.getName (class t)) (ex-message t)])) @r @c]))
(shutdown-agents)
