;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; W6 port of upstream clojure/parallel.clj (oracle: upstream source
;; text, read-only; no JVM at runtime, ever). Public API unchanged:
;; par, pany, pmax, pmin, psummary, preduce, pvec, pdistinct, psort,
;; pfilter-nils, pfilter-dupes.
;;
;; Port adaptations (pinned by rust/crates/clj-eval/tests/w6_parallel.rs):
;; - The jsr166y ForkJoin ParallelArray cannot exist: parallel arrays
;;   are `ParallelArray` records holding a realized vector plus
;;   attached ops, and realization applies bound, then filters, then
;;   maps (the required attachment order, enforced when ops attach —
;;   upstream misorders fail with linkage errors, the port reports
;;   the violation directly).
;; - Aggregates run sequentially except `preduce`, which folds
;;   512-wide chunks on futures (same results; parallelism is an
;;   optimization the docstrings never promise observably).
;; - Comparators are fns of two args (the port `sort` convention),
;;   not java.util.Comparator objects.
;; - `pany` picks uniformly at random (docstring-faithful).
;; - `(par x)` on an already-parallel array returns it as-is (even
;;   without mappings — upstream rebuilds those); `-with` size
;;   mismatches throw (like the JDK ops).
;; - `pcumulate` stays private; upstream notes it is broken (passes
;;   null to the reducer), the port runs the sequential prefix scan.
;; - Misuse throws ex-info (the port has no exception ctors).

(ns ^{:doc "DEPRECATED Wrapper of the ForkJoin library (JSR-166)."
       :author "Rich Hickey"}
    clojure.parallel)
(alias 'parallel 'clojure.parallel)

;; W6: a parallel array is realized data plus attached ops (:stage
;; tracks the latest attachment kind: bound 0, filters 1, maps 2).
(defrecord ParallelArray [tag data stage bound filters maps])

(defn- parray? [x]
  (and (map? x) (= (:tag x) :clojure.parallel/parallel-array)))

(defn- with-size-check [d c2]
  (when (not= (count d) (count c2))
    (throw (ex-info "par :map-with/:filter-with collections must match in size"
                    {:left (count d) :right (count c2)}))))

(defn- apply-bound [data bound]
  (if bound
    (let [[s e] bound]
      (subvec (vec data) s e))
    data))

(defn- apply-filters [data filters]
  (reduce (fn [d [op args]]
            (cond
              (= op :filter) (filterv args d)
              (= op :filter-index)
              (vec (keep-indexed (fn [i x] (when (args x i) x)) d))
              (= op :filter-with)
              (let [[p c2] args]
                (with-size-check d c2)
                (vec (keep (fn [[x y]] (when (p x y) x))
                           (map vector d c2))))))
          data filters))

(defn- apply-maps [data maps]
  (reduce (fn [d [op args]]
            (cond
              (= op :map) (mapv args d)
              (= op :map-index)
              (vec (map-indexed (fn [i x] (args x i)) d))
              (= op :map-with)
              (let [[f c2] args]
                (with-size-check d c2)
                (mapv (fn [[x y]] (f x y)) (map vector d c2)))))
          data maps))

;; W6: index args count positions as the collection enters each op.
(defn- realize [pa]
  (-> (:data pa)
      (apply-bound (:bound pa))
      (apply-filters (:filters pa))
      (apply-maps (:maps pa))))

(defn- realize-coll [coll]
  (if (parray? coll)
    (realize coll)
    (vec coll)))

(def ^:private op-stage
  {:bound 0 :filter 1 :filter-index 1 :filter-with 1
   :map 2 :map-index 2 :map-with 2})

(defn- attach-op [pa [op args]]
  (let [stage (get op-stage op)]
    (when (nil? stage)
      (throw (ex-info (str "Unsupported par op: " op) {:op op})))
    (when (< stage (:stage pa))
      (throw (ex-info (str "par ops must attach in bound/filter/map order: " op)
                      {:op op})))
    (cond
      (= op :bound) (assoc pa :bound args :stage 0)
      (= op :filter-with)
      (let [[p c2] args]
        (-> pa
            (update :filters conj [:filter-with [p (realize-coll c2)]])
            (assoc :stage 1)))
      (= op :map-with)
      (let [[f c2] args]
        (-> pa
            (update :maps conj [:map-with [f (realize-coll c2)]])
            (assoc :stage 2)))
      (<= stage 1) (-> pa
                       (update :filters conj [op args])
                       (assoc :stage 1))
      :else (-> pa
                (update :maps conj [op args])
                (assoc :stage 2)))))

(defn par
  "Creates a parallel array from coll. ops, if supplied, perform
  on-the-fly filtering or transformations during parallel realization
  or calculation. ops form a chain, and bounds must precede filters,
  must precede maps. ops must be a set of keyword value pairs of the
  following forms:

     :bound [start end]

  Only elements from start (inclusive) to end (exclusive) will be
  processed when the array is realized.

     :filter pred

  Filter preds remove elements from processing when the array is realized. pred
  must be a function of one argument whose return will be processed
  via boolean.

     :filter-index pred2

  pred2 must be a function of two arguments, which will be an element
  of the collection and the corresponding index, whose return will be
  processed via boolean.

     :filter-with [pred2 coll2]

  pred2 must be a function of two arguments, which will be
  corresponding elements of the 2 collections.

     :map f

  Map fns will be used to transform elements when the array is
  realized. f must be a function of one argument.

     :map-index f2

  f2 must be a function of two arguments, which will be an element of
  the collection and the corresponding index.

     :map-with [f2 coll2]

  f2 must be a function of two arguments, which will be corresponding
  elements of the 2 collections."

  ([coll]
     (if (parray? coll)
       coll
       (->ParallelArray :clojure.parallel/parallel-array (vec coll) -1 nil [] [])))
  ([coll & ops]
     (reduce attach-op
             (par coll)
             (partition 2 ops))))

;;;;;;;;;;;;;;;;;;;;; aggregate operations ;;;;;;;;;;;;;;;;;;;;;;
(defn pany
  "Returns some (random) element of the coll if it satisfies the bound/filter/map"
  [coll]
  (let [r (realize-coll coll)]
    (when (seq r)
      (nth r (rand-int (count r))))))

(defn pmax
  "Returns the maximum element, presuming Comparable elements, unless
  a Comparator comp is supplied"
  ([coll]
   (let [r (realize-coll coll)]
     (when (seq r)
       (reduce max r))))
  ([coll comp]
   (let [r (realize-coll coll)]
     (when (seq r)
       (reduce (fn [a b] (if (neg? (comp a b)) b a)) r)))))

(defn pmin
  "Returns the minimum element, presuming Comparable elements, unless
  a Comparator comp is supplied"
  ([coll]
   (let [r (realize-coll coll)]
     (when (seq r)
       (reduce min r))))
  ([coll comp]
   (let [r (realize-coll coll)]
     (when (seq r)
       (reduce (fn [a b] (if (pos? (comp a b)) b a)) r)))))

(defn psummary
  "Returns a map of summary statistics (min. max, size, min-index, max-index,
  presuming Comparable elements, unless a Comparator comp is supplied"
  ([coll] (psummary coll compare))
  ([coll comp]
   (let [r (realize-coll coll)]
     (when (seq r)
       (let [n (count r)]
         (loop [i 1 mini 0 minv (nth r 0) maxi 0 maxv (nth r 0)]
           (if (>= i n)
             {:min minv :max maxv :size n :min-index mini :max-index maxi}
             (let [v (nth r i)
                   cmin (comp v minv)
                   cmax (comp v maxv)]
               (recur (inc i)
                      (if (neg? cmin) i mini) (if (neg? cmin) v minv)
                      (if (pos? cmax) i maxi) (if (pos? cmax) v maxv))))))))))

(defn preduce
  "Returns the reduction of the realized elements of coll
  using function f. Note f will not necessarily be called
  consecutively, and so must be commutative. Also note that
  (f base an-element) might be performed many times, i.e. base is not
  an initial value as with sequential reduce."
  [f base coll]
  (let [chunks (partition-all 512 (realize-coll coll))]
    (if (seq chunks)
      ;; All chunk futures spawn before the first deref (ordered
      ;; results, parallel work).
      (let [futs (mapv (fn [chunk] (future (reduce f base chunk))) chunks)]
        (reduce f (mapv deref futs)))
      base)))

;;;;;;;;;;;;;;;;;;;;; collection-producing operations ;;;;;;;;;;;;;;;;;;;;;;

(defn- pa-to-vec [pa]
  (vec pa))

(defn- pall
  "Realizes a copy of the coll as a parallel array, with any bounds/filters/maps applied"
  [coll]
  (realize-coll coll))

(defn pvec
  "Returns the realized contents of the parallel array pa as a Clojure vector"
  [pa] (pa-to-vec (pall pa)))

(defn pdistinct
  "Returns a parallel array of the distinct elements of coll"
  [coll]
  (pa-to-vec (distinct (pall coll))))

(defn- pcumulate [coll f init]
  (vec (rest (reductions f init (pall coll)))))

(defn psort
  "Returns a new vector consisting of the realized items in coll, sorted,
  presuming Comparable elements, unless a Comparator comp is supplied"
  ([coll] (pa-to-vec (sort (pall coll))))
  ([coll comp] (pa-to-vec (sort comp (pall coll)))))

(defn pfilter-nils
  "Returns a vector containing the non-nil (realized) elements of coll"
  [coll]
  (pa-to-vec (remove nil? (pall coll))))

(defn pfilter-dupes
  "Returns a vector containing the (realized) elements of coll,
  without any consecutive duplicates"
  [coll]
  (pa-to-vec (map first (partition-by identity (pall coll)))))


(comment
(load-file "src/parallel.clj")
(refer 'parallel)
(pdistinct [1 2 3 2 1])
;(pcumulate [1 2 3 2 1] + 0) ;broken, not exposed
(def a (make-array Object 1000000))
(dotimes i (count a)
  (aset a i (rand-int i)))
(time (reduce + 0 a))
(time (preduce + 0 a))
(time (count (distinct a)))
(time (count (pdistinct a)))

(preduce + 0 [1 2 3 2 1])
(preduce + 0 (psort a))
(pvec (par [11 2 3 2] :filter-index (fn [x i] (> i x))))
(pvec (par [11 2 3 2] :filter-with [(fn [x y] (> y x)) [110 2 33 2]]))

(psummary ;or pvec/pmax etc
 (par [11 2 3 2]
      :filter-with [(fn [x y] (> y x))
                    [110 2 33 2]]
      :map #(* % 2)))

(preduce + 0
  (par [11 2 3 2]
       :filter-with [< [110 2 33 2]]))

(time (reduce + 0 (map #(* % %) (range 1000000))))
(time (preduce + 0 (par (range 1000000) :map-index *)))
(def v (range 1000000))
(time (preduce + 0 (par v :map-index *)))
(time (preduce + 0 (par v :map  #(* % %))))
(time (reduce + 0 (map #(* % %) v)))
)
