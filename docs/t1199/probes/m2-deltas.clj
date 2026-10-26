;; T1199 M2 delta probes (upstream 9e51d6dc :req!, 4610bb46 ns-dominance).
;; Oracle: clojure 1.13.0-alpha10 + spec.alpha 0.6.249 + core.specs.alpha 0.6.148-alpha12
;; (these WIP features postdate alpha9, so alpha9 jars reject them).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar clojure.main docs/t1199/probes/m2-deltas.clj > docs/t1199/probes/m2-deltas.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; :req! directive (upstream req!-directive tests, adapted)
(throwof "req-kw" '(let [{:req! {a :long-a}} {:long-a 1, :b 2}] a))
(throwof "req-missing" '(let [{:req! {a :long-a}} {}] a))
(throwof "req-mix" '(let [{:keys! [b], :keys [d], :req! {a :long-a}} {:long-a 1, :b 2}] [a b d]))
(throwof "req-or" '(let [{:keys! [b], :keys [d], :req! {a :long-a}, :or {d 42}} {:long-a 1, :b 2}] [a b d]))
(throwof "req-or-same" '(eval (quote (let [{:req! {b :b}, :or {b 42}} {}] b))))
(throwof "req-nested" '(let [{:req! {{:keys! [c]} :nested}} {:nested {:c 3}}] c))
(throwof "req-missingsel" '(let [{:req! {a :long-a}, :missing miss} {}] miss))
;; req! as a public fn (upstream :added 1.13)
(throwof "req-present" '(req! {:a 1} :a))
(throwof "req-absent" '(req! {} :a))
(throwof "req-nilval" '(req! {:a nil} :a))
(throwof "req-vec" '(req! [1 2] 1))
(throwof "req-vec-miss" '(req! [1 2] 5))
(throwof "req-str" '(req! "ab" 1))
(throwof "req-nil" '(req! nil :a))
(throwof "req-arity3" '(req! {:a 1} :a :nf))
(throwof "req-arity1" '(req! {:a 1}))
;; ns-dominance (upstream shapes; post-& keys observed via :select)
(throwof "ns-postamp" '(let [{:foo/keys! [aa & :bb], :select sel} {:foo/aa 1, :bb 2, :foo/bb 3}] sel))
(throwof "ns-postamp-plain" '(let [{:keys! [aa & :bb], :select sel} {:aa 1, :bb 2}] sel))
(throwof "ns-elem-kw" '(let [{:foo/keys [::a]} {:user/a 1, :foo/a 2}] a))
(throwof "ns-dir-only" '(let [{:foo/keys [x]} {:foo/x 42}] x))
