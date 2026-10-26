;; F-sorted oracle probes (oracle: alpha10 jars, issue #31).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i31-sorted/probes/sorted.clj > docs/i31-sorted/probes/sorted.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; constructors order by key/comparator
(throwof "sorted-map" '(sorted-map :b 2 :a 1))
(throwof "sorted-set" '(sorted-set 3 1 2))
(throwof "sorted-set-by" '(sorted-set-by > 3 1 2))
(throwof "sorted-map-by-bad" '(sorted-map-by > :b 2 :a 1))
;; subseq/rsubseq bounds (start test/end test each)
(throwof "subseq-ge" '(subseq (sorted-map :a 1 :b 2 :c 3) >= :b))
(throwof "subseq-gt" '(subseq (sorted-map :a 1 :b 2 :c 3) > :b))
(throwof "rsubseq-le" '(rsubseq (sorted-map :a 1 :b 2 :c 3) <= :b))
(throwof "rsubseq-set-ge" '(rsubseq (sorted-set 1 2 3) >= 2))
(throwof "subseq-set-lt" '(subseq (sorted-set 1 2 3) < 3))
(throwof "subseq-past-end" '(subseq (sorted-map :a 1) >= :z))
(throwof "subseq-unsorted" '(subseq [] >= 1))
;; sortedness survives assoc/dissoc/conj/empty
(throwof "sorted-assoc" '(assoc (sorted-map :a 1) :b 2))
(throwof "sorted-dissoc" '(dissoc (sorted-map :a 1 :b 2) :a))
(throwof "sorted-conj" '(conj (sorted-set 1) 2 1))
(throwof "sorted-pred-t" '(sorted? (sorted-map :a 1)))
(throwof "sorted-pred-f" '(sorted? {:a 1}))
(throwof "sorted-empty" '(empty (sorted-map-by > :a 1)))
(throwof "sorted-count" '(count (sorted-map :a 1 :b 2)))
(throwof "sorted-comparator" '(comparator (sorted-map-by > :a 1)))
;; round two: equality/reversibility/collapse/bounds/nth/errors
(throwof "sorted-empty-p" '(sorted? (empty (sorted-map :a 1))))
(throwof "sorted-eq" '(= (sorted-map :a 1) {:a 1}))
(throwof "sorted-rseq" '(rseq (sorted-map :a 1 :b 2)))
(throwof "sorted-dup" '(sorted-map :a 1 :a 2))
(throwof "subseq-5" '(subseq (sorted-map :a 1 :b 2 :c 3) > :a < :c))
(throwof "rsubseq-5" '(rsubseq (sorted-map :a 1 :b 2 :c 3) > :a < :c))
(throwof "sorted-nth" '(nth (sorted-map :a 1 :b 2) 0))
(throwof "sorted-disj" '(disj (sorted-set 1 2) 1))
(throwof "sorted-kw" '(sorted-set :b :a))
(throwof "sorted-odd" '(sorted-map :a))
(throwof "subseq-eq" '(subseq (sorted-map :a 1 :b 2) = :b))
(throwof "sorted-keys" '(keys (sorted-map :b 2 :a 1)))
(throwof "sorted-vals" '(vals (sorted-map :b 2 :a 1)))
(throwof "sorted-set-nth" '(nth (sorted-set 1 2) 0))
(throwof "sorted-set-rseq" '(rseq (sorted-set 1 2)))
(throwof "sorted-set-eq" '(= (sorted-set 1 2) #{1 2}))
(throwof "sorted-meta" '(meta (with-meta (sorted-map :a 1) {:x 1})))
(throwof "trans-sorted" '(persistent! (assoc! (transient (sorted-map :a 1)) :b 2)))
(throwof "sorted-keyobj" '(let [m (sorted-map-by (fn [a b] (compare (:id a) (:id b))) {:id 1} :a)] (keys (assoc m {:id 1 :x 0} :b))))
