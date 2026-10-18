;; S4 oracle probes, round 1: nested :or fire-once under outlet forcing.
;; Exit 0 (all errors caught; cause chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "or-nested-select" #(let [log (atom [])] [(let [{{:keys [b] :or {b (do (swap! log conj 7) 42)}} :m :select s} {:m {}}] [s]) @log]))
(tx "or-nested-all" #(let [log (atom [])] [(let [{{:keys [b] :or {b (do (swap! log conj 7) 42)}} :m :all s} {:m {}}] [s]) @log]))
(tx "or-nested-excess" #(let [log (atom [])] [(let [{{:keys [b] :or {b (do (swap! log conj 7) 42)}} :m :excess s} {:m {} :z 9}] [s]) @log]))
(tx "or-nested-req-outlet" #(let [log (atom [])] [(let [{{:keys [b] :select ns :or {b (do (swap! log conj 7) 42)}} :m :select s} {:m {}}] [ns s]) @log]))
(tx "or-nested-present" #(let [log (atom [])] [(let [{{:keys [b] :or {b (do (swap! log conj 7) 42)}} :m :select s} {:m {:b 1}}] [s]) @log]))
