;; S6 differential corpus 01: atoms (deref/swap!/reset!/CAS/validators/watches/meta).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s6-pins.md P2, concurrency.rs atom_ctor_rows/_errors, validator_*,
;; deref_rows/_errors, swap_*, reset_*, cas_*, watch_*, meta_rows, print_str_rows (atom parts).
;; Every entry is self-contained (no cross-entry fixtures); never prints raw refs.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- ctor rows (atom_ctor_rows) ---
(p ":atom-deref-1" #(deref (atom 1)))
(p ":atom-deref-nil" #(deref (atom nil)))
(p ":atom-deref-false" #(deref (atom false)))
(p ":atom-meta-kv" #(meta (atom 1 :meta {:k :v})))
(p ":atom-meta-none" #(meta (atom 1)))
(p ":atom-meta-nilopt" #(meta (atom 1 :meta nil)))
(p ":atom-validator-ok" #(do (atom 2 :validator even?) :ok))
(p ":atom-validator-nil" #(do (atom 1 :validator nil) :ok))
(p ":atom-validator-false" #(do (atom 1 :validator false) :ok))
(p ":atom-unknown-opt" #(do (atom 1 :foo 2) :ok))
(p ":atom-dup-meta-last" #(meta (atom 1 :meta {:a 1} :meta {:b 2})))
(p ":atom-dup-mixed" #(meta (atom 1 :validator (fn [x] true) :meta {:a 1})))
(p ":atom-class" #(str (class (atom 1))))

;; --- ctor errors (atom_ctor_errors) ---
(p ":atom-ctor-veto" #(atom 2 :validator odd?))
(p ":atom-ctor-noval" #(atom 1 :validator))
(p ":atom-ctor-stray" #(atom 1 :a 1 :stray))
(p ":atom-ctor-nonkv" #(atom 1 2))
(p ":atom-ctor-badmeta" #(atom 1 :meta 5))
(p ":atom-ctor-badvalidator" #(atom 1 :validator 5))
(p ":atom-arity-0" #(atom))

;; --- validator throw shapes (validator_throw_shapes, validator_checked_wraps) ---
(p ":validator-throw-propagates" #(do (def vt (atom 0 :validator (fn [x] (if (zero? x) true (throw (ex-info "vx" {:n 1})))))) (try (reset! vt 5) (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)]))))
(p ":validator-throw-ctor" #(atom 0 :validator (fn [x] (throw (ex-info "vx" {:n 1})))))
(p ":validator-checked-wrap" #(try (let [a (atom 0 :validator (fn [x] (if (zero? x) true (throw (Exception. "checked-boom")))))] (reset! a 1)) (catch IllegalStateException e [(ex-message e) (ex-message (ex-cause e)) (str (class (ex-cause e)))])))

;; --- deref rows, atom/var parts (deref_rows; delay parts in 02) ---
(p ":deref-atom" #(deref (atom 5)))
(p ":deref-atom-reader" #(do @(atom 5)))
(p ":deref-var" #(do (def dx 7) (deref #'dx)))
(p ":deref-var-dynamic" #(do (def ^:dynamic ddy 1) (binding [ddy 2] (deref #'ddy))))
(p ":deref-var-unbound-class" #(str (class (do (declare dux) (deref #'dux)))))

;; --- deref errors (deref_errors) ---
(p ":deref-long" #(deref 1))
(p ":deref-str" #(deref "s"))
(p ":deref-nil" #(deref nil))
(p ":deref-arity-0" #(deref))
(p ":deref-arity-2" #(deref (atom 1) 50))

;; --- swap! rows (swap_rows, swap_cas_accounting) ---
(p ":swap-basic" #(swap! (atom 1) inc))
(p ":swap-returns-new" #(let [a (atom 1)] [(swap! a inc) @a]))
(p ":swap-extra-3" #(swap! (atom 0) + 1 2 3))
(p ":swap-extra-5" #(swap! (atom 0) + 1 2 3 4 5))
(p ":swap-keyword-fn" #(swap! (atom {:a 1}) :a))
(p ":swap-nil-result" #(let [a (atom 1)] (swap! a (fn [x])) @a))
(p ":swap-retry-race" #(let [calls (atom 0) a (atom 1)] (swap! a (fn [x] (swap! calls inc) (reset! a 10) (+ x 100))) [@a @calls]))
(p ":swap-uncontended" #(let [calls (atom 0) a (atom 1)] (swap! a (fn [x] (swap! calls inc) (inc x))) [@a @calls]))

;; --- swap! errors (swap_errors) ---
(p ":swap-long-rcv" #(swap! 1 inc))
(p ":swap-long-both" #(swap! 1 2))
(p ":swap-nil-0" #(swap! nil inc))
(p ":swap-nil-1" #(swap! nil + 1))
(p ":swap-nil-2" #(swap! nil + 1 2))
(p ":swap-nil-3" #(swap! nil + 1 2 3))
(p ":swap-arity-1" #(swap! (atom 1)))
(p ":swap-badfn" #(swap! (atom 1) 5))
(p ":swap-nilfn-0" #(swap! (atom 1) nil))
(p ":swap-nilfn-1" #(swap! (atom 1) nil :x))
(p ":swap-nilfn-2" #(swap! (atom 1) nil :x :y))
(p ":swap-nilfn-3" #(swap! (atom 1) nil :x :y :z))
(p ":swap-veto" #(let [a (atom 0 :validator even?)] (swap! a inc)))
(p ":swap-veto-keeps" #(do (def sv (atom 0 :validator even?)) (def svlog (atom [])) (add-watch sv :k (fn [k r o n] (swap! svlog conj n))) (try (swap! sv inc) (catch IllegalStateException e :vetoed)) [@sv @svlog]))
(p ":swap-throwf-keeps" #(do (def sf (atom 1)) (def sflog (atom [])) (add-watch sf :k (fn [k r o n] (swap! sflog conj n))) [(try (swap! sf (fn [x] (throw (ex-info "fx" {})))) (catch clojure.lang.ExceptionInfo e :thrown)) @sf @sflog]))
(p ":swap-f-arity-bool" #(try (swap! (atom 1) (fn [a b c] a)) :no-throw (catch clojure.lang.ArityException e (.startsWith ^String (ex-message e) "Wrong number of args (1) passed to: "))))

;; --- reset! rows/errors (reset_rows, reset_errors) ---
(p ":reset-basic" #(reset! (atom 1) 7))
(p ":reset-nil" #(let [a (atom 1)] (reset! a nil) @a))
(p ":reset-same-fires" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (reset! a 1) @log))
(p ":reset-long" #(reset! 1 2))
(p ":reset-nil-rcv" #(reset! nil 1))
(p ":reset-arity-1" #(reset! (atom 1)))
(p ":reset-arity-3" #(reset! (atom 1) 2 3))
(p ":reset-veto" #(let [a (atom 0 :validator even?)] (reset! a 3)))
(p ":reset-veto-keeps" #(do (def rs (atom 0 :validator even?)) (try (reset! rs 3) (catch IllegalStateException e :vetoed)) @rs))

;; --- compare-and-set! rows/errors (cas_rows, cas_errors) ---
(p ":cas-win" #(let [a (atom 1)] [(compare-and-set! a 1 2) @a]))
(p ":cas-lose" #(let [a (atom 1)] [(compare-and-set! a 99 2) @a]))
(p ":cas-kw" #(let [a (atom :a)] (compare-and-set! a :a :b)))
(p ":cas-ident" #(let [v [1] a (atom v)] (compare-and-set! a v :new)))
(p ":cas-to-nil" #(let [a (atom 1)] [(compare-and-set! a 1 nil) @a]))
(p ":cas-lose-no-watch" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (compare-and-set! a 99 2) @log))
(p ":cas-win-fires" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (compare-and-set! a 1 2) @log))
(p ":cas-long" #(compare-and-set! 1 1 2))
(p ":cas-nil" #(compare-and-set! nil 1 2))
(p ":cas-arity-2" #(compare-and-set! (atom 1) 1))
(p ":cas-validates-on-match" #(let [a (atom 0 :validator even?)] (compare-and-set! a 0 3)))
(p ":cas-validates-on-mismatch" #(let [a (atom 0 :validator even?)] (compare-and-set! a 99 3)))

;; --- watches (watch_rows, watch_errors) ---
(p ":watch-add-returns-ref" #(let [a (atom 1)] (reset! (add-watch a :k (fn [k r o n])) 5) @a))
(p ":watch-remove-returns-ref" #(let [a (atom 1)] (add-watch a :k (fn [k r o n])) (reset! (remove-watch a :k) 6) @a))
(p ":watch-args" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [k (deref r) o n]))) (reset! a 2) @log))
(p ":watch-same-value-fires" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj [o n]))) (swap! a identity) @log))
(p ":watch-remove-stops" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (swap! log conj 1))) (remove-watch a :k) (reset! a 2) @log))
(p ":watch-remove-missing" #(let [a (atom 1)] (remove-watch a :nope) :ok))
(p ":watch-nonkw-key" #(let [a (atom 1)] (add-watch a [1] (fn [k r o n])) :ok))
(p ":watch-replace-fn" #(let [log (atom []) a (atom 0)] (add-watch a :k (fn [k r o n] (swap! log conj :old))) (add-watch a :k (fn [k r o n] (swap! log conj :new))) (reset! a 1) @log))
(p ":watch-nil-fn-silent" #(let [log (atom []) a (atom 1)] (add-watch a :k nil) (add-watch a :j (fn [k r o n] (swap! log conj :j))) (reset! a 2) @log))
(p ":watch-nested-publish" #(let [log (atom []) a (atom 1)] (add-watch a :k (fn [k r o n] (when (< n 3) (swap! a inc)) (swap! log conj n))) (reset! a 2) [@a @log]))
(p ":watch-throw-keeps" #(do (def wa (atom 1)) (add-watch wa :k (fn [k r o n] (throw (ex-info "wx" {})))) [(try (reset! wa 2) (catch clojure.lang.ExceptionInfo e :caught)) @wa]))
(p ":watch-add-long" #(add-watch 1 :k (fn [k r o n])))
(p ":watch-remove-long" #(remove-watch 1 :k))
(p ":watch-add-nil" #(add-watch nil :k (fn [k r o n])))
(p ":watch-remove-nil" #(remove-watch nil :k))
(p ":watch-add-badfn" #(add-watch (atom 1) :k 5))
(p ":watch-add-arity" #(add-watch (atom 1) :k))
(p ":watch-remove-arity" #(remove-watch (atom 1)))

;; --- atom meta (meta_rows, atom parts; delay parts in 02) ---
(p ":alter-meta" #(alter-meta! (atom 1) assoc :a 1))
(p ":reset-meta" #(reset-meta! (atom 1) {:a 1}))
(p ":alter-meta-read" #(let [a (atom 1)] (alter-meta! a assoc :a 1) (meta a)))
(p ":reset-meta-nil" #(let [a (atom 1 :meta {:a 1})] (reset-meta! a nil) (meta a)))
(p ":withmeta-atom" #(with-meta (atom 1) {:a 1}))
(p ":varymeta-atom" #(vary-meta (atom 1) assoc :a 1))
(p ":printmeta-atom-bool" #(.startsWith ^String (binding [*print-meta* true] (pr-str (atom 1 :meta {:a 1}))) "^{:a 1} #object[clojure.lang.Atom 0x"))

;; --- atom print/equality/predicates/casts (print_str_rows, atom parts) ---
(p ":pr-atom-bool" #(.startsWith ^String (pr-str (atom 1)) "#object[clojure.lang.Atom 0x"))
(p ":str-atom-bool" #(.startsWith ^String (str (atom 1)) "clojure.lang.Atom@"))
(p ":print-atom-bool" #(.startsWith ^String (with-out-str (print (atom 1))) "#object[clojure.lang.Atom 0x"))
(p ":format-atom-bool" #(.startsWith ^String (format "%s" (atom 1)) "clojure.lang.Atom@"))
(p ":atom-eq-self" #(let [a (atom 1)] (= a a)))
(p ":atom-eq-fresh" #(= (atom 1) (atom 1)))
(p ":atom-delay-neq" #(= (atom 1) (delay 1)))
(p ":atom-not-eq" #(not= (atom 1) (atom 1)))
(p ":atom-hash-int" #(integer? (hash (atom 1))))
(p ":seq-atom" #(seq (atom 1)))
(p ":first-atom" #(first (atom 1)))
(p ":count-atom" #(count (atom 1)))
(p ":nth-atom" #(nth (atom 1) 0))
(p ":coll-atom" #(coll? (atom 1)))
(p ":seq?-atom" #(seq? (atom 1)))
(p ":counted-atom" #(counted? (atom 1)))
(p ":fn-atom" #(fn? (atom 1)))
(p ":map-atom" #(map? (atom 1)))
(p ":vector-atom" #(vector? (atom 1)))
(p ":var-atom" #(var? (atom 1)))
(p ":get-atom" #(get (atom {:a 1}) :a))
(p ":find-atom" #(find (atom {}) :a))
(p ":conj-atom" #(conj (atom 1) 2))
(p ":compare-atom" #(compare (atom 1) 1))
(p ":invoke-atom" #((atom 1)))

(println "S6-01 DONE")
