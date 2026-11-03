;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W8 (core parity) port (issue #204) of upstream
;; `src/clj/clojure/core/protocols.clj` (a pure-Rust port: every
;; `.`-interop call is replaced by a native equivalent, and every
;; JVM divergence is noted where it lands).
;;
;; Divergences from upstream:
;; - The `(set! *warn-on-reflection* true)` line is dropped: `load`
;;   does not bind that var, so a top-level `set!` throws (the same
;;   pre-existing loader gap that breaks `(require 'clojure.string)`
;;   on the base branch; the loader lives in `ns.rs`, out of W8
;;   bounds).
;; - Bare `Object`/`Iterable` extension targets are spelled qualified
;;   (`java.lang.Object`, `java.lang.Iterable`): the port has no
;;   default imports.
;; - `Datafiable`/`Navigable` drop `:extend-via-metadata` (the
;;   `defprotocol` macro has no such option): the wrapper fns below
;;   implement the same metadata-first dispatch in its place.
;; - Protocol dispatch here is exact-class-key (there is no hierarchy
;;   walk), so the `Object` defaults do not fire for unlisted types
;;   the way upstream's do. The `coll-reduce`/`internal-reduce`/
;;   `datafy`/`nav` wrappers below restore the call behavior (every
;;   call reduces/returns exactly what upstream returns); only
;;   `satisfies?`/`extends?` still answer only for explicitly
;;   extended types.
;; - `iterator-reduce!` reduces over `iterator-seq` (there is no
;;   `.hasNext`/`.next` surface); `iter-reduce` and
;;   `interface-or-naive-reduce` route through `seq-reduce`/
;;   `naive-seq-reduce` (there is no `.iterator`/`.reduce` surface);
;;   `naive-seq-reduce` unwraps with `unreduced` instead of `@`
;;   (`@` on a `Reduced` misses in the `deref` native).
;;   Interface extension arms (`IReduceInit`, `Iterable`, ...) are
;;   kept for shape and `extends?` parity but can only fire on exact
;;   class matches.

(ns clojure.core.protocols)

(defprotocol CollReduce
  "Protocol for collection types that can implement reduce faster than
  first/next recursion. Called by clojure.core/reduce. Baseline
  implementation defined in terms of Iterable."
  (coll-reduce [coll f] [coll f val]))

(defprotocol InternalReduce
  "Protocol for concrete seq types that can reduce themselves
   faster than first/next recursion. Called by clojure.core/reduce."
  (internal-reduce [seq f start]))

(defn- seq-reduce
  ([coll f]
     (if-let [s (seq coll)]
       (internal-reduce (next s) f (first s))
       (f)))
  ([coll f val]
     (let [s (seq coll)]
       (internal-reduce s f val))))

;; Upstream walks `.hasNext`/`.next` directly; the port reduces over
;; `iterator-seq`, which consumes the same handle and likewise
;; respects `reduced` (via `reduce`).
(defn iterator-reduce!
  ([iter f]
   (let [s (seq (iterator-seq iter))]
     (if s (reduce f s) (f))))
  ([iter f val]
   (reduce f val (iterator-seq iter))))

(defn- iter-reduce
  ;; Upstream reduces over `(.iterator coll)`; without a `.iterator`
  ;; surface the port reduces over the seq (equal results).
  ([coll f]
   (seq-reduce coll f))
  ([coll f val]
   (seq-reduce coll f val)))

(defn- naive-seq-reduce
  "Reduces a seq, ignoring any opportunities to switch to a more
  specialized implementation."
  [s f val]
  ;; Upstream derefs with `@ret`; the port uses `unreduced`
  ;; (`@` on a `Reduced` misses in the `deref` native — a
  ;; pre-existing gap outside W8 bounds — while `unreduced`
  ;; unwraps identically here).
  (loop [s (seq s)
         val val]
    (if s
      (let [ret (f val (first s))]
        (if (reduced? ret)
          (unreduced ret)
          (recur (next s) ret)))
      val)))

(defn- interface-or-naive-reduce
  ;; Upstream calls `.reduce` on `IReduceInit` instances; without a
  ;; `.reduce` surface the port always reduces naively (equal results).
  "Reduces via IReduceInit if possible, else naively."
  [coll f val]
  (naive-seq-reduce coll f val))

(extend-protocol CollReduce
  nil
  (coll-reduce
   ([coll f] (f))
   ([coll f val] val))

  java.lang.Object
  (coll-reduce
   ([coll f] (seq-reduce coll f))
   ([coll f val] (seq-reduce coll f val)))

  clojure.lang.IReduceInit
  (coll-reduce
    ([coll f] (seq-reduce coll f))
    ([coll f val] (seq-reduce coll f val)))

  ;;aseqs are iterable, masking internal-reducers
  clojure.lang.ASeq
  (coll-reduce
   ([coll f] (seq-reduce coll f))
   ([coll f val] (seq-reduce coll f val)))

  ;;for range
  clojure.lang.LazySeq
  (coll-reduce
   ([coll f] (seq-reduce coll f))
   ([coll f val] (seq-reduce coll f val)))

  ;;vector's chunked seq is faster than its iter
  clojure.lang.PersistentVector
  (coll-reduce
   ([coll f] (seq-reduce coll f))
   ([coll f val] (seq-reduce coll f val)))

  java.lang.Iterable
  (coll-reduce
   ([coll f] (iter-reduce coll f))
   ([coll f val] (iter-reduce coll f val)))

  clojure.lang.APersistentMap$KeySeq
  (coll-reduce
    ([coll f] (iter-reduce coll f))
    ([coll f val] (iter-reduce coll f val)))

  clojure.lang.APersistentMap$ValSeq
  (coll-reduce
    ([coll f] (iter-reduce coll f))
    ([coll f val] (iter-reduce coll f val))))

(extend-protocol InternalReduce
  nil
  (internal-reduce
   [s f val]
   val)

  ;; handles vectors and ranges
  clojure.lang.IChunkedSeq
  (internal-reduce
   [s f val]
   ;; Upstream walks chunks and calls `.reduce` on each; without a
   ;; `.reduce` surface the port reduces naively (equal results).
   (naive-seq-reduce s f val))

  clojure.lang.StringSeq
  (internal-reduce
   [str-seq f val]
   ;; Upstream reads the `.s`/`.i` fields directly; the port reduces
   ;; naively (equal results).
   (naive-seq-reduce str-seq f val))

  java.lang.Object
  (internal-reduce
   [s f val]
   ;; `unreduced`, not upstream's `@ret` (same `@`-on-`Reduced`
   ;; gap as `naive-seq-reduce` above).
   (loop [cls (class s)
          s s
          f f
          val val]
     (if-let [s (seq s)]
       (if (identical? (class s) cls)
         (let [ret (f val (first s))]
                (if (reduced? ret)
                  (unreduced ret)
                  (recur cls (next s) f ret)))
         (interface-or-naive-reduce s f val))
       val))))

;; The `Object`-default emulation (see the header note): protocol
;; dispatch here is exact-class-key, so values whose class has no arm
;; would miss. Upstream's `Object` arm routes those through
;; `seq-reduce`/`naive-seq-reduce`; the wrappers do the same whenever
;; no explicit (or user-extended) impl applies. `extend-protocol`
;; keeps working: extensions land in the protocol map, which the
;; captured protocol fn reads through its var on every call.
(let [proto-coll-reduce coll-reduce]
  (defn coll-reduce
    ([coll f]
     (if (satisfies? CollReduce coll)
       (proto-coll-reduce coll f)
       (seq-reduce coll f)))
    ([coll f val]
     (if (satisfies? CollReduce coll)
       (proto-coll-reduce coll f val)
       (seq-reduce coll f val)))))

(let [proto-internal-reduce internal-reduce]
  (defn internal-reduce
    [s f val]
    (if (satisfies? InternalReduce s)
      (proto-internal-reduce s f val)
      (naive-seq-reduce s f val))))

(defprotocol IKVReduce
  "Protocol for concrete associative types that can reduce themselves
   via a function of key and val faster than first/next recursion over map
   entries. Called by clojure.core/reduce-kv, and has same
   semantics (just different arg order)."
  (kv-reduce [amap f init]))

(defprotocol Datafiable
  (datafy [o] "return a representation of o as data (default identity)"))

(extend-protocol Datafiable
  nil
  (datafy [_] nil)

  java.lang.Object
  (datafy [x] x))

;; Metadata-first dispatch plus the `Object`-default emulation (see
;; the header note): upstream checks `(get (meta x)
;; 'clojure.core.protocols/datafy)` before the impl-map lookup
;; (`emit-method-builder` with `:extend-via-metadata`), and the
;; `Object` arm defaults to identity. Corner divergence: a value
;; whose type has an inline (deftype) impl AND a metadata fn takes
;; the metadata fn here (the port has no direct-interface fast path
;; to prefer the inline impl the way upstream does).
(let [proto-datafy datafy]
  (defn datafy
    [o]
    (if-let [f (get (meta o) 'clojure.core.protocols/datafy)]
      (f o)
      (if (satisfies? Datafiable o)
        (proto-datafy o)
        o))))

(defprotocol Navigable
  (nav [coll k v] "return (possibly transformed) v in the context of coll and k (a key/index or nil),
defaults to returning v."))

(extend-protocol Navigable
  java.lang.Object
  (nav [_ _ x] x))

(let [proto-nav nav]
  (defn nav
    [coll k v]
    (if-let [f (get (meta coll) 'clojure.core.protocols/nav)]
      (f coll k v)
      (if (satisfies? Navigable coll)
        (proto-nav coll k v)
        v))))
