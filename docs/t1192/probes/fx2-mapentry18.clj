;; T1192 FX2 MapEntry probe 18 (PR #20 review round 1 oracle rows).
;; Oracle: clojure 1.13.0-alpha9 + spec.alpha 0.6.249 + core.specs.alpha 0.6.133-alpha10
;;   (artifact is core.specs.alpha, not core.specs).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha9/clojure-1.13.0-alpha9.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar clojure.main docs/t1192/probes/fx2-mapentry18.clj > docs/t1192/probes/fx2-mapentry18.out
(def e (first {:a 1}))
(defn throwof [label form]
  ;; pr-str INSIDE the try: lazy values (e.g. vals) can throw on realization,
  ;; and probe12's bare-eval throwof would abort the file while printing.
  (println label (try (pr-str (eval form))
                      (catch Throwable t (str "THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; (1) declare with non-symbol
(throwof "declare-5" '(declare 5))
;; (2) seqable?
(throwof "seqable-e" '(seqable? e))
(throwof "seqable-vec" '(seqable? [1 2]))
(throwof "seqable-num" '(seqable? 5))
;; (3) nthnext edges
(throwof "nthnext-e0" '(nthnext e 0))
(throwof "nthnext-e1" '(nthnext e 1))
(throwof "nthnext-e2" '(nthnext e 2))
(throwof "nthnext-vec0" '(nthnext [1 2 3] 0))
(throwof "nthnext-vec5" '(nthnext [1 2 3] 5))
(throwof "nthnext-neg" '(nthnext [1 2 3] -1))
(throwof "nthnext-nonn" '(nthnext [1 2 3] 1.5))
(throwof "nthnext-unseq" '(nthnext 5 1))
;; (4) merge-with
(throwof "merge-with-empty" '(merge-with + {:a 1} (quote ())))
(throwof "merge-with-seq" '(merge-with + {:a 1} (seq {:b 2 :c 3})))
;; (5) find
(throwof "find-e0" '(find e 0))
(throwof "find-e1" '(find e 1))
(throwof "find-ekw" '(find e :kw))
;; (6) invoke
(throwof "invoke-2" '(e 0 1))
(throwof "invoke-neg" '(e -1))
;; (7) vals over vector of vector
(throwof "vals-vec" '(vals [[1 2]]))
;; (8) sorted?
(throwof "sorted-vec" '(sorted? [1]))
(throwof "sorted-map" '(sorted? (sorted-map :a 1)))
;; (9) eval-spliced binding positions (MapEntry where the analyzer wants a vector)
(throwof "let-entry-kw" '(eval (list (quote let) (first {:x 1}) 1)))
;; control: plain vector [:x 1] fails identically -> kw failure is about :x, not entryness
(throwof "let-kw-vec" '(eval (list (quote let) [(quote :x) 1] 1)))
(throwof "fn-entry-invoke" '(let [f (eval (list (quote fn) (first {(quote a) (quote b)}) (quote a)))] (f 10 20)))
(throwof "fn-entry-fnq" '(fn? (eval (list (quote fn) (first {(quote a) (quote b)}) (quote a)))))
(throwof "let-entry-pattern" '(eval (list (quote let) [(first {(quote a) (quote b)}) [10 20]] (quote a))))
(throwof "let-entry-keys" '(eval (list (quote let) [{:keys (first {(quote a) (quote b)})} {:a 10 :b 20}] (quote a))))
(throwof "let-entry-bindings" '(eval (list (quote let) (first {(quote x) 1}) (quote x))))
;; (10) arity errors
(throwof "rseq-0" '(rseq))
(throwof "rseq-2" '(rseq [1] [2]))
(throwof "nthnext-1" '(nthnext [1]))
(throwof "nthnext-0" '(nthnext))
(throwof "sorted-0" '(sorted?))
(throwof "sorted-2" '(sorted? [1] [2]))
;; (11) map-destructure int keys
(throwof "mapdest-0" '(let [{x 0} (first {:a 1})] x))
(throwof "mapdest-1" '(let [{x 1} (first {:a 1})] x))
;; (12) sequential-destructure missing index
(throwof "seqdest-missing" '(let [[a b c] (first {:x 1})] c))
;; (13) dead rows re-run: probe11 merge-with-entry-class + probe7 rseq-str-class (original forms)
(throwof "merge-with-entry-class:" '(class (merge-with + {:a 1} e)))
(throwof "rseq-str-class" '(class (rseq "ab")))
