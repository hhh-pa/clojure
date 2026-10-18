;; S4 oracle probes: :or lookup precedence and required-key defaults.
;; Exit 0 (all errors caught; cause chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "or-local-entry" #(eval '(let [{x261 :x261 :or {x261 5}} {}] x261)))
(tx "or-key-entry" #(eval '(let [{x261 :x261 :or {:x261 7}} {}] x261)))
(tx "or-both-entry" #(eval '(let [{x261 :x261 :or {x261 5 :x261 7}} {:x261 9}] x261)))
(tx "req-missing-req" #(eval '(let [{:keys [a261] :req [a261]} {}] a261)))
(tx "req-present-req" #(eval '(let [{:keys [a261] :req [a261]} {:a261 1}] a261)))
(tx "req-or-default" #(eval '(let [{:keys [a261] :req [a261] :or {a261 1}} {}] a261)))
(tx "req-key-default" #(eval '(let [{:keys [a261] :req [a261] :or {:a261 1}} {}] a261)))
(tx "or-unknown-noselect" #(eval '(let [{:keys [a261] :or {zzz261 1}} {:a261 1}] a261)))
(tx "or-keys-select" #(eval '(let [{:keys [a261] :select s261 :or {:a261 1}} {:b261 2}] s261)))
(tx "or-missing-key" #(eval '(let [{:keys! [a261] :missing m261 :or {:a261 1}} {}] [a261 m261])))
(tx "rest-on-number" #(eval '(let [[& r261] 5] r261)))
(tx "rest-on-nil" #(eval '(let [[& r261] nil] r261)))
(tx "rest-on-str" #(eval '(let [[& r261] "xy"] r261)))
(tx "rest-on-empty" #(eval '(let [[& r261] []] r261)))
