;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

(ns 
  ^{:author "Stuart Halloway",
    :doc "Non-core data functions."}
  clojure.data
  (:require [clojure.set :as set]))

;; W1 port: upstream sets *warn-on-reflection* true here; the port
;; rejects set! on an unbound root and has no reflection warnings,
;; so the set! is dropped.

(declare diff)

(defn- atom-diff
  "Internal helper for diff."
  [a b]
  (if (= a b) [nil nil a] [a b nil]))

;; for big things a sparse vector class would be better
(defn- vectorize
  "Convert an associative-by-numeric-index collection into
   an equivalent vector, with nil for any missing keys"
  [m]
  (when (seq m)
    (reduce
     (fn [result [k v]] (assoc result k v))
     (vec (repeat (apply max (keys m))  nil))
     m)))

(defn- diff-associative-key
  "Diff associative things a and b, comparing only the key k."
  [a b k]
  (let [va (get a k)
        vb (get b k)
        [a* b* ab] (diff va vb)
        in-a (contains? a k)
        in-b (contains? b k)
        same (and in-a in-b
                  (or (not (nil? ab))
                      (and (nil? va) (nil? vb))))]
    [(when (and in-a (or (not (nil? a*)) (not same))) {k a*})
     (when (and in-b (or (not (nil? b*)) (not same))) {k b*})
     (when same {k ab})
     ]))

(defn- diff-associative
  "Diff associative things a and b, comparing only keys in ks."
  [a b ks]
  (reduce
   (fn [diff1 diff2]
     (doall (map merge diff1 diff2)))
   [nil nil nil]
   (map
    (partial diff-associative-key a b)
    ks)))

(defn- diff-sequential
  [a b]
  (vec (map vectorize (diff-associative
                       (if (vector? a) a (vec a))
                       (if (vector? b) b (vec b))
                       (range (max (count a) (count b)))))))

(defprotocol ^{:added "1.3"} EqualityPartition
  "Implementation detail. Subject to change."
  (^{:added "1.3"} equality-partition [x] "Implementation detail. Subject to change."))

(defprotocol ^{:added "1.3"} Diff
  "Implementation detail. Subject to change."
  (^{:added "1.3"} diff-similar [a b] "Implementation detail. Subject to change."))

;; W1 port: upstream extends Diff and EqualityPartition over nil,
;; Object, and java.util.{Set,List,Map}, testing (.. x getClass
;; isArray) for JVM arrays. The port dispatches protocols by exact
;; class only (no hierarchy walk), has no java.util interfaces, and
;; reports isArray false, so both dispatches below are
;; predicate-based. The EqualityPartition/Diff protocol vars above
;; are kept for API shape, but diff routes through these helpers;
;; JVM arrays partition as :atom here (upstream subdiffs them
;; sequentially).
(defn- equality-partition-of
  [x]
  (cond (map? x) :map
        (set? x) :set
        (sequential? x) :sequential
        :else :atom))

(defn- as-set-value
  [s]
  (if (set? s) s (into #{} s)))

(defn- diff-similar-of
  [partition a b]
  (cond (= partition :set)
        (let [aval (as-set-value a)
              bval (as-set-value b)]
          [(not-empty (set/difference aval bval))
           (not-empty (set/difference bval aval))
           (not-empty (set/intersection aval bval))])

        (= partition :sequential)
        (diff-sequential a b)

        (= partition :map)
        (diff-associative a b (set/union (keys a) (keys b)))

        ;; Both :atom (upstream reaches atom-diff through the
        ;; nil/Object protocol impls).
        :else
        (atom-diff a b)))

(defn diff
  "Recursively compares a and b, returning a tuple of
  [things-only-in-a things-only-in-b things-in-both].
  Comparison rules:

  * For equal a and b, return [nil nil a].
  * Maps are subdiffed where keys match and values differ.
  * Sets are never subdiffed.
  * All sequential things are treated as associative collections
    by their indexes, with results returned as vectors.
  * Everything else (including strings!) is treated as
    an atom and compared for equality."
  {:added "1.3"}
  [a b]
  (if (= a b)
    [nil nil a]
    ;; W1 port: upstream calls the equality-partition/diff-similar
    ;; protocol fns; see the dispatch note above.
    (let [pa (equality-partition-of a)]
      (if (= pa (equality-partition-of b))
        (diff-similar-of pa a b)
        (atom-diff a b)))))
  
