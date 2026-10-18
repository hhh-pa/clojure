;; S4 oracle probes, round 1: binding order (M1).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "binding-init-throw" #(eval '(do (def ^:dynamic d291 0) (def n291 0) (binding [d291 1 n291 (throw (ex-info "init" {}))] :body))))
(tx "binding-init-throw-first" #(eval '(do (def ^:dynamic d292 0) (def n292 0) (binding [n292 (throw (ex-info "init2" {})) d292 1] :body))))
(tx "binding-nondyn-then-throw" #(eval '(do (def ^:dynamic d293 0) (def n293 0) (binding [n293 1 d293 (throw (ex-info "init3" {}))] :body))))
