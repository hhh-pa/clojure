;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/core_print.clj (W4 entry/IO).
;;
;; Upstream this file is NOT a namespace: it is loaded into clojure.core
;; with (in-ns 'clojure.core) as part of the core load. The port keeps
;; that shape — the (ns clojure.core-print) form below exists only so the
;; file has a load root for require; every definition lands in
;; clojure.core via the in-ns that follows it.
;;
;; Almost everything upstream defines here already lives in the native
;; print system, so the port defines nothing twice:
;;   * print-method / print-dup (the multis) and their full method table
;;     — native (printsys.rs); the upstream defmethods dispatch on JVM
;;     Class objects and write via .write/.append interop, neither of
;;     which exists here, so they are NOT redefined in Clojure.
;;   * print-simple / print-ctor, char-escape-string / char-name-string,
;;     primitives-classnames, StackTraceElement->vec, Throwable->map,
;;     PrintWriter-on — native.
;;   * *print-length* / *print-level* / *print-namespace-maps* /
;;     *verbose-defrecords* (*print-meta* / *print-readably* / *print-dup*
;;     come from core.clj upstream) — native.
;;   * Upstream's top-level (set! *warn-on-reflection* true) is dropped:
;;     reflection warnings are a JVM-compiler knob, and a top-level set!
;;     would fail a bare require.
;; What remains is print-initialized, the file's only contribution that
;; the natives do not already provide.

(ns clojure.core-print
  "Port of upstream clojure/core_print.clj: clojure.core print support."
  {:author "Rich Hickey"})

(in-ns 'clojure.core)

(def ^:private print-initialized true)
