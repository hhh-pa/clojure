;;; utilities.clj -- part of the pretty printer for Clojure

;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Author: Tom Faulhaber
;; April 3, 2009

;; This module implements some utility function used in formatting and pretty
;; printing. The functions here could go in a more general purpose library,
;; perhaps.

(in-ns 'clojure.pprint)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Helper functions for digesting formats in the various
;;; phases of their lives.
;;; These functions are actually pretty general.
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defn- map-passing-context [func initial-context lis]
  (loop [context initial-context
         lis lis
         acc []]
    (if (empty? lis)
      [acc context]
    (let [this (first lis)
          remainder (next lis)
          [result new-context] (apply func [this context])]
      (recur new-context remainder (conj acc result))))))

(defn- consume [func initial-context]
  (loop [context initial-context
         acc []]
    (let [[result new-context] (apply func [context])]
      (if (not result)
        [acc new-context]
      (recur new-context (conj acc result))))))

(defn- consume-while [func initial-context]
  (loop [context initial-context
         acc []]
    (let [[result continue new-context] (apply func [context])]
      (if (not continue)
        [acc context]
      (recur new-context (conj acc result))))))

(defn- unzip-map
  "Take a map that has pairs in the value slots and produce a pair of
  maps, the first having all the first elements of the pairs and the
  second all the second elements of the pairs"
  [m]
  [(into {} (for [[k [v1 v2]] m] [k v1]))
   (into {} (for [[k [v1 v2]] m] [k v2]))])

(defn- tuple-map
  "For all the values, v, in the map, replace them with [v v1]"
  [m v1]
  (into {} (for [[k v] m] [k [v v1]])))

(defn- rtrim
  "Trim all instances of c from the end of sequence s"
  [s c]
  (let [len (count s)]
    (if (and (pos? len) (= (nth s (dec (count s))) c))
      (loop [n (dec len)]
        (cond 
         (neg? n) ""
         (not (= (nth s n) c)) (subs s 0 (inc n))
         true (recur (dec n))))
      s)))

(defn- ltrim
  "Trim all instances of c from the beginning of sequence s"
  [s c]
  (let [len (count s)]
    (if (and (pos? len) (= (nth s 0) c))
      (loop [n 0]
        (if (or (= n len) (not (= (nth s n) c)))
          (subs s n)
          (recur (inc n))))
      s)))

(defn- prefix-count
  "Return the number of times that val occurs at the start of sequence aseq,
  if val is a seq itself, count the number of times any element of val
  occurs at the beginning of aseq"
  [aseq val]
  (let [test (if (coll? val) (set val) #{val})]
    (loop [pos 0]
     (if (or (= pos (count aseq)) (not (test (nth aseq pos))))
       pos
       (recur (inc pos))))))

(defn- prerr
  "Println to *err*"
  [& args]
  (binding [*out* *err*]
    (apply println args)))

(defmacro ^{:private true} prlabel
  "Print args to *err* in name = value format"
  [prefix arg & more-args]
  `(prerr ~@(cons (list 'quote prefix) (mapcat #(list (list 'quote %) "=" %)
                                                  (cons arg (seq more-args))))))

;; W3 divergence: upstream declares (definterface PrettyFlush (^void ppflush []))
;; for its java.io.Writer proxies. This port represents writers as atoms (see
;; column_writer.clj), so the flush entry point is the pp-flush fn in
;; pretty_writer.clj and no interface is needed.

(defn- pp-ex
  "Build the throwable for error paths. Upstream throws host ctors
  ((RuntimeException. msg), (IllegalArgumentException. msg), etc.); this port
  has no host exception ctors, so errors throw ex-infos carrying the upstream
  class keyword in :pp/error. Catch with (catch Exception e ...) and read the
  message with ex-message."
  [class-kw msg]
  (ex-info msg {:pp/error class-kw}))

(defn- walk
  "Local stand-in for clojure.walk/walk (upstream :use, see pprint.clj note).
  Same shape as upstream: inner over children, outer over the rebuilt form.
  Map entries are walked as [k v] pairs (no MapEntry/create here) and records
  rebuild as plain maps; pll-mod-body only walks macroexpanded code."
  [inner outer form]
  (cond
    (list? form) (outer (apply list (map inner form)))
    (map? form) (outer (into (empty form)
                             (map (fn [[k v]] [(inner k) (inner v)]) form)))
    (seq? form) (outer (doall (map inner form)))
    (coll? form) (outer (into (empty form) (map inner form)))
    :else (outer form)))
