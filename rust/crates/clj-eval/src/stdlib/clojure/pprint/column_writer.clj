;;; column_writer.clj -- part of the pretty printer for Clojure


;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Author: Tom Faulhaber
;; April 3, 2009
;; Revised to use proxy instead of gen-class April 2010

;; This module implements a column-aware wrapper around an instance of java.io.Writer

(in-ns 'clojure.pprint)

;; W3 divergence: upstream wraps the base writer in a (proxy [Writer IDeref])
;; whose deref is a ref of fields, with dosync/alter updates. Proxies over
;; Writer are unsupported here, so a column writer is an atom holding the same
;; fields map ({:max :cur :line :base} plus the :column-writer tag), updated
;; with swap! (single-threaded use, so STM atomicity is unneeded). The base is
;; a real writer; writes reach it via pp-base-write below.

(def ^:dynamic ^{:private true} *default-page-width* 72)

(defn- pp-base-write
  "Write x (a string or a char code) to a real (non-port) writer by
  rebounding *out* momentarily. Upstream: (.write ^Writer base x)."
  [base x]
  (binding [*out* base]
    (print (if (string? x) x (char x)))))

(defn- get-field [this sym]
  (sym @this))

(defn- set-field [this sym new-val]
  (swap! this assoc sym new-val))

(defn- get-column [this]
  (get-field this :cur))

(defn- get-line [this]
  (get-field this :line))

(defn- get-max-column [this]
  (get-field this :max))

(defn- set-max-column [this new-max]
  (set-field this :max new-max)
  nil)

(defn- get-writer [this]
  (get-field this :base))

(defn- pp-last-index
  "Upstream (.lastIndexOf s (int \\newline)); no lastIndexOf here, so scan."
  [^String s]
  (loop [i (dec (count s))]
    (cond (neg? i) -1
          (= (nth s i) \newline) i
          :else (recur (dec i)))))

(defn- c-write-char [this c]
  (let [c (int c)]
    ;; Upstream wraps the counter updates in dosync; two swap!s are equivalent
    ;; under single-threaded use.
    (if (= c (int \newline))
      (do
        (set-field this :cur 0)
        (set-field this :line (inc (get-field this :line))))
      (set-field this :cur (inc (get-field this :cur)))))
  (pp-base-write (get-field this :base) c))

(defn- col-write-string [this ^String s]
  (let [nl (pp-last-index s)]
    (if (neg? nl)
      (set-field this :cur (+ (get-field this :cur) (count s)))
      (do
        (set-field this :cur (- (count s) nl 1))
        (set-field this :line (+ (get-field this :line)
                                 (count (filter #(= % \newline) s)))))))
  (pp-base-write (get-field this :base) s))

(defn- col-write
  "Write x (a string or a char code) through a column-writer atom. Upstream:
  the proxy's (write [x]) condp on (class x) over String/Integer/Long (plus a
  cbuf/off/len arm that no caller uses)."
  [this x]
  (if (string? x)
    (col-write-string this x)
    (c-write-char this x)))

(defn- column-writer
  ([writer] (column-writer writer *default-page-width*))
  ([writer max-columns]
   (atom {:column-writer true
          :max max-columns, :cur 0, :line 0 :base writer})))
