;; T1199 M2 error-shape probes (upstream 9e51d6dc, 4610bb46).
;; Oracle: clojure 1.13.0-alpha10 + spec.alpha 0.6.249 + core.specs.alpha 0.6.148-alpha12.
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar clojure.main docs/t1199/probes/m2-errors.clj > docs/t1199/probes/m2-errors.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; malformed :req! (spec rejects: macroexpand Syntax error)
(throwof "req-vec" '(let [{:req! [a]} {:a 1}] a))
(throwof "req-num" '(let [{:req! 5} {:a 1}] a))
(throwof "req-nested-bad" '(let [{:req! {5 :a}} {:a 1}] a))
;; loop goes through expansion first: Unexpected (cast) vs Syntax (iseq)
(throwof "loop-req-bad-full" '(loop [{:req! [a]} {:a 1}] a))
(throwof "loop-req-num" '(loop [{:req! 5} {:a 1}] a))
;; vacuous :req!: empty map ok, empty vec/nil die in the inner-let spec
(throwof "loop-req-empty-vec" '(loop [{:req! []} {:a 1}] 42))
(throwof "loop-req-empty-map" '(loop [{:req! {}} {:a 1}] 42))
(throwof "let-req-empty-map" '(let [{:req! {}} {:a 1}] 42))
(throwof "loop-req-nil" '(loop [{:req! nil} {:a 1}] 42))
;; fn takes :req!; malformed params die in the fn spec
(throwof "fn-req" '((fn [{:req! {a :long-a}}] a) {:long-a 1}))
(throwof "fn-req-nested" '((fn [{:req! {{:keys [c]} :nested}}] c) {:nested {:c 3}}))
(throwof "fn-req-missing" '((fn [{:req! {a :long-a}}] a) {}))
(throwof "fn-req-or" '((fn [{:req! {b :b}, :or {b 42}}] b) {}))
(throwof "fn-req-missingsel" '((fn [{:req! {a :long-a}, :missing miss}] miss) {}))
(throwof "fn-req-bad" '((fn [{:req! [a]}] a) {:a 1}))
;; element-namespace precedence errors: (namespace %) always runs now
(throwof "str-unqual" '(let [{:keys ["s"]} {"s" 1}] s))
(throwof "str-qual" '(let [{:foo/keys ["s"]} {:foo/s 1}] s))
(throwof "num-qual" '(let [{:foo/keys [5]} {:foo/x 1}] x))
;; post-& qualification keeps element namespaces
(throwof "post-qual" '(let [{:foo/keys [a & :other/b], :select s} {:foo/a 1, :other/b 2}] s))
(throwof "post-qual-req" '(let [{:foo/keys! [aa & :other/bb], :select s} {:foo/aa 1, :other/bb 2}] s))
