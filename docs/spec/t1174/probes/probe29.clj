;; S4 oracle probes, round 1: binding order (M1), qualified keys (M2),
;; throw-nil (M3), finally masking (L1). Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "binding-init-throw" #(eval '(do (def ^:dynamic d291 0) (def n291 0) (binding [d291 1 n291 (throw (ex-info "init" {}))] :body))))
(tx "binding-init-throw-first" #(eval '(do (def ^:dynamic d292 0) (def n292 0) (binding [n292 (throw (ex-info "init2" {})) d292 1] :body))))
(tx "binding-nondyn-then-throw" #(eval '(do (def ^:dynamic d293 0) (def n293 0) (binding [n293 1 d293 (throw (ex-info "init3" {}))] :body))))
(tx "qual-keys-let" #(eval '(let [{:foo/keys [a294]} {:foo/a294 1}] a294)))
(tx "qual-syms-let" #(eval '(let [{:foo/syms [a294]} {'foo/a294 1}] a294)))
(tx "qual-keys-bang-let" #(eval '(let [{:foo/keys! [a294]} {:foo/a294 1}] a294)))
(tx "qual-strs-let" #(eval '(let [{:foo/strs [a294]} {"foo/a294" 1}] a294)))
(tx "qual-keys-fn" #(eval '((fn [{:foo/keys [a294]}] a294) {:foo/a294 1})))
(tx "qual-value-let" #(eval '(let [{:keys [b294/c294]} {}] 1)))
(tx "qual-syms-bang-let" #(eval '(let [{:foo/syms! [a294]} {'foo/a294 1}] a294)))
(tx "qual-strs-bang-let" #(eval '(let [{:foo/strs! [a294]} {"foo/a294" 1}] a294)))
(tx "qual-value-qual-dir" #(eval '(let [{:foo/keys [b294/c294]} {}] 1)))
(tx "qual-value-strs" #(eval '(let [{:strs [b294/c294]} {}] 1)))
(tx "qual-strs-loop" #(eval '(loop [{:foo/strs [a294]} {}] 1)))
(tx "qual-strs-fn" #(eval '(fn [{:foo/strs [a294]}] 1)))
(tx "throw-nil" #(eval '(throw nil)))
(tx "finally-masks-body" #(eval '(try (throw (ex-info "A" {})) (finally (throw (ex-info "F" {}))))))
(tx "finally-masks-handler" #(eval '(try (throw (ex-info "A" {})) (catch Exception e (throw (ex-info "H" {}))) (finally (throw (ex-info "F" {}))))))
