;; Copyright (c) Rich Hickey. All rights reserved.
;; Copyright (c) Hao Hu. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; W6 port of upstream clojure/core/reducers.clj (oracle: upstream
;; source text, read-only; no JVM at runtime, ever). Public API
;; unchanged: pool, fjtask, reduce, CollFold/coll-fold, fold,
;; reducer, folder, map, mapcat, filter, remove, flatten,
;; take-while, take, drop, cat, append!, foldcat, monoid.
;;
;; Port adaptations (pinned by rust/crates/clj-eval/tests/w6_reducers.rs):
;; - ForkJoin → futures: fjfork spawns a future, fjjoin derefs it,
;;   fjinvoke runs inline (nested forks still parallelize; the future
;;   pool is unbounded so chunks never deadlock). Task failures
;;   surface as ExecutionException (deref) rather than raw rethrow
;;   (join). `pool` stays a delay of an opaque handle for API shape;
;;   nothing consults it.
;; - No clojure.core.protocols on the port (W8 owns it): reduction
;;   dispatch is a private `coll-reduce` fn (maps reduce kv, like
;;   kv-reduce; everything else reduces sequentially with `reduced`
;;   short-circuit). `reducer`/`folder` return reifies recognized via
;;   a private PXf protocol (same wrapping semantics).
;; - The CollFold protocol is kept, with nil/PersistentVector/
;;   IPersistentMap impls. The port dispatches exact classes, so the
;;   upstream Object default lives in `fold`'s fallback (a single
;;   sequential reduce — the docstring allows it: parallelism is
;;   optional, order is not), the vector impl names the concrete
;;   class (subvecs miss and fold sequentially — same results), and
;;   the PersistentHashMap `.fold` has no counterpart (maps fold
;;   sequentially through the IPersistentMap entry).
;; - No clojure.walk yet (W1): do-rfn carries a private postwalk
;;   (lists/vectors/maps only — all do-rfn feeds it).
;; - No ArrayList: `cat`'s identity is `[]`, and Cat trees are lazy
;;   concats (reducible, foldable, seqable, and counted, as
;;   documented). ->Cat/Cat. are omitted (upstream: "do not
;;   construct this directly"). Counts walk, so deep trees cost
;;   O(n log n). (The port's `=` equates sequential collections
;;   across seq/vector, unlike the JVM — cats compare accordingly.)
;; - `(set! *warn-on-reflection* true)` is dropped (a port load
;;   hazard — the flag is unread here).

(ns ^{:doc
      "A library for reduction and parallel folding. Alpha and subject
      to change."
      :author "Rich Hickey"}
  clojure.core.reducers
  (:refer-clojure :exclude [reduce map mapcat filter remove take take-while drop flatten cat]))

(alias 'core 'clojure.core)

;;;;;;;;;;;;;; some future stuff (W6: ForkJoin → futures) ;;;;;;;;;;

(def pool (delay {:w6-pool :reducers}))

(defn fjtask [f]
  f)

(defn- fjinvoke [f]
  (f))

(defn- fjfork [task] (future-call task))

(defn- fjjoin [task] @task)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;; W6: marks reducer/folder reifies so `coll-reduce` can unwrap them
;; (upstream dispatches the CollReduce protocol; exact-class dispatch
;; cannot express its defaults, so this stays a fn). Sequential legs
;; delegate to core/reduce (which unwraps `reduced` itself — the port
;; has no `unreduced`/`deref` for it); maps split entries into kv.
;; W6: public only because the port misresolves private protocol
;; vars from method bodies (port-internal; not upstream API).
(defprotocol PXf
  (xf-fn [x])
  (xf-base [x]))

(defn- coll-reduce
  ([coll f] (coll-reduce coll f (f)))
  ([coll f init]
   (cond
     (nil? coll) init
     (satisfies? PXf coll) (coll-reduce (xf-base coll) ((xf-fn coll) f) init)
     (map? coll) (core/reduce (fn [ret e] (f ret (key e) (val e))) init coll)
     :else (core/reduce f init coll))))

(defn reduce
  "Like core/reduce except:
     When init is not provided, (f) is used.
     Maps are reduced with reduce-kv"
  ([f coll] (reduce f (f) coll))
  ([f init coll]
   (coll-reduce coll f init)))

(defprotocol CollFold
  (coll-fold [coll n combinef reducef]))

(defn fold
  "Reduces a collection using a (potentially parallel) reduce-combine
  strategy. The collection is partitioned into groups of approximately
  n (default 512), each of which is reduced with reducef (with a seed
  value obtained by calling (combinef) with no arguments). The results
  of these reductions are then reduced with combinef (default
  reducef). combinef must be associative, and, when called with no
  arguments, (combinef) must produce its identity element. These
  operations may be performed in parallel, but the results will
  preserve order."
  {:added "1.5"}
  ([reducef coll] (fold reducef reducef coll))
  ([combinef reducef coll] (fold 512 combinef reducef coll))
  ([n combinef reducef coll]
   ;; W6: types without an impl fold sequentially (the upstream
   ;; Object default, as a fallback branch).
   (if (satisfies? CollFold coll)
     (coll-fold coll n combinef reducef)
     (reduce reducef (combinef) coll))))

(defn reducer
  "Given a reducible collection, and a transformation function xf,
  returns a reducible collection, where any supplied reducing
  fn will be transformed by xf. xf is a function of reducing fn to
  reducing fn."
  {:added "1.5"}
  ([coll xf]
     ;; W6: protocols qualify (upstream writes
     ;; clojure.core.protocols/CollReduce the same way) — `reify`
     ;; resolves its quoted specs at call time against the current
     ;; namespace, so bare names break cross-ns calls.
     (reify
      clojure.core.reducers/PXf
      (xf-fn [_] xf)
      (xf-base [_] coll))))

(defn folder
  "Given a foldable collection, and a transformation function xf,
  returns a foldable collection, where any supplied reducing
  fn will be transformed by xf. xf is a function of reducing fn to
  reducing fn."
  {:added "1.5"}
  ([coll xf]
     (reify
      clojure.core.reducers/PXf
      (xf-fn [_] xf)
      (xf-base [_] coll)

      clojure.core.reducers/CollFold
      (coll-fold [_ n combinef reducef]
                 (fold n combinef (xf reducef) coll)))))

(defn- do-curried
  [name doc meta args body]
  ;; W6: built with `list` (syntax-quote yields seqs, but the port's
  ;; `fn` spec wants list clauses — the prelude builds fn forms the
  ;; same way; `gensym` stands in for `x#`).
  (let [cargs (vec (butlast args))
        x (gensym "x")]
    (list 'defn name doc meta
          (list cargs (list 'fn [x] (concat [name] cargs [x])))
          ;; W6: `apply list`, not `list*` (`list*` of one arg hands
          ;; back the cons cell, which is not a list).
          (apply list (cons args body)))))

(defmacro ^:private defcurried
  "Builds another arity of the fn that returns a fn awaiting the last
  param"
  [name doc meta args & body]
  (do-curried name doc meta args body))

;; W6: clojure.walk is another workstream (W1); do-rfn only needs a
;; tiny postwalk, so it carries its own.
(defn- postwalk [f form]
  ;; W6: core/map stays qualified — this ns shadows `map` below.
  (cond
    (map? form) (f (into {} (core/map (fn [[k v]] [(postwalk f k) (postwalk f v)])) form))
    (vector? form) (f (vec (core/map #(postwalk f %) form)))
    (seq? form) (f (apply list (core/map #(postwalk f %) form)))
    :else (f form)))

(defn- do-rfn [f1 k fkv]
  ;; W6: built with `list` (see do-curried); the remover also
  ;; rebuilds lists (`remove` returns lazy seqs, vectors stay
  ;; vectors).
  (list 'fn
        (list [] (list f1))
        (postwalk
         #(if (sequential? %)
            ((if (vector? %) vec (fn [s] (apply list s)))
             (core/remove #{k} %))
            %)
         fkv)
        fkv))

(defmacro ^:private rfn
  "Builds 3-arity reducing fn given names of wrapped fn and key, and k/v impl."
  [[f1 k] fkv]
  (do-rfn f1 k fkv))

(defcurried map
  "Applies f to every value in the reduction of coll. Foldable."
  {:added "1.5"}
  [f coll]
  (folder coll
   (fn [f1]
     (rfn [f1 k]
          ([ret k v]
             (f1 ret (f k v)))))))

(defcurried mapcat
  "Applies f to every value in the reduction of coll, concatenating the result
  colls of (f val). Foldable."
  {:added "1.5"}
  [f coll]
  (folder coll
   (fn [f1]
     (let [f1 (fn
                ([ret v]
                  (let [x (f1 ret v)] (if (reduced? x) (reduced x) x)))
                ([ret k v]
                  (let [x (f1 ret k v)] (if (reduced? x) (reduced x) x))))]
       (rfn [f1 k]
            ([ret k v]
               (reduce f1 ret (f k v))))))))

(defcurried filter
  "Retains values in the reduction of coll for which (pred val)
  returns logical true. Foldable."
  {:added "1.5"}
  [pred coll]
  (folder coll
   (fn [f1]
     (rfn [f1 k]
          ([ret k v]
             (if (pred k v)
               (f1 ret k v)
               ret))))))

(defcurried remove
  "Removes values in the reduction of coll for which (pred val)
  returns logical true. Foldable."
  {:added "1.5"}
  [pred coll]
  (filter (complement pred) coll))

(defcurried flatten
  "Takes any nested combination of sequential things (lists, vectors,
  etc.) and returns their contents as a single, flat foldable
  collection."
  {:added "1.5"}
  [coll]
  (folder coll
   (fn [f1]
     (fn
       ([] (f1))
       ([ret v]
          (if (sequential? v)
            (coll-reduce (flatten v) f1 ret)
            (f1 ret v)))))))

(defcurried take-while
  "Ends the reduction of coll when (pred val) returns logical false."
  {:added "1.5"}
  [pred coll]
  (reducer coll
   (fn [f1]
     (rfn [f1 k]
          ([ret k v]
             (if (pred k v)
               (f1 ret k v)
               (reduced ret)))))))

(defcurried take
  "Ends the reduction of coll after consuming n values."
  {:added "1.5"}
  [n coll]
  (reducer coll
   (fn [f1]
     (let [cnt (atom n)]
       (rfn [f1 k]
         ([ret k v]
            (swap! cnt dec)
            (if (neg? @cnt)
              (reduced ret)
              (f1 ret k v))))))))

(defcurried drop
  "Elides the first n values from the reduction of coll."
  {:added "1.5"}
  [n coll]
  (reducer coll
   (fn [f1]
     (let [cnt (atom n)]
       (rfn [f1 k]
         ([ret k v]
            (swap! cnt dec)
            (if (neg? @cnt)
              (f1 ret k v)
              ret)))))))

;; W6: Cat trees are lazy concats (reducible, foldable, seqable,
;; and counted). ->Cat is omitted (upstream: "do not construct this
;; directly").
(defn cat
  "A high-performance combining fn that yields the catenation of the
  reduced values. The result is reducible, foldable, seqable and
  counted, providing the identity collections are reducible, seqable
  and counted. The single argument version will build a combining fn
  with the supplied identity constructor. Tests for identity
  with (zero? (count x)). See also foldcat."
  {:added "1.5"}
  ([] [])
  ([ctor]
     (fn
       ([] (ctor))
       ([left right] (cat left right))))
  ([left right]
     (cond
      (zero? (count left)) right
      (zero? (count right)) left
      :else
      (concat left right))))

(defn append!
  ".adds x to acc and returns acc"
  {:added "1.5"}
  [acc x]
  (conj acc x))

(defn foldcat
  "Equivalent to (fold cat append! coll)"
  {:added "1.5"}
  [coll]
  (fold cat append! coll))

(defn monoid
  "Builds a combining fn out of the supplied operator and identity
  constructor. op must be associative and ctor called with no args
  must return an identity value for it."
  {:added "1.5"}
  [op ctor]
  (fn m
    ([] (ctor))
    ([a b] (op a b))))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;; fold impls ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
(defn- foldvec
  [v n combinef reducef]
  (cond
   (empty? v) (combinef)
   (<= (count v) n) (reduce reducef (combinef) v)
   :else
   (let [split (quot (count v) 2)
         v1 (subvec v 0 split)
         v2 (subvec v split (count v))
         fc (fn [child] #(foldvec child n combinef reducef))]
     (fjinvoke
      #(let [f1 (fc v1)
             ;; W6: the forked future is the join handle (upstream
             ;; rejoins the mutated task).
             f2 (fjfork (fjtask (fc v2)))]
         (combinef (f1) (fjjoin f2)))))))

(extend-protocol CollFold
 nil
 (coll-fold
  [coll n combinef reducef]
  (combinef))

 ;; W6: the port dispatches exact classes (see the header note).
 clojure.lang.PersistentVector
 (coll-fold
  [v n combinef reducef]
  (foldvec v n combinef reducef))

 clojure.lang.IPersistentMap
 (coll-fold
  [coll n combinef reducef]
  (reduce reducef (combinef) coll)))
