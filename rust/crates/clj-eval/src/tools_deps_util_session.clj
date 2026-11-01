;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slices 2-3: port of clojure.tools.deps.util.session
;; (org.clojure/tools.deps 0.31.1646, util/session.clj) — slice 2
;; brought `session` + `retrieve`; slice 3 adds `with-session` and
;; `retrieve-local`, completing the namespace.
;;
;; Port adaptations (see docs/gap-124/slice-2.md B2 and
;; docs/gap-124/slice-3.md C9/C10):
;; - `session` is an `(atom {})`, not a `ConcurrentHashMap`;
;;   `retrieve` is check-then-swap (a repeated compute under races
;;   is benign — cached values are content-derived).
;; - `with-session` expands through `list` (no syntax-quote):
;;   `let`/`try`/`finally`/`var` stay unqualified (the analyzer
;;   matches those only unqualified), core vars are fully
;;   qualified, and the fresh session is `(atom {})`.
;; - `retrieve-local` scopes on the `port/thread-id` native.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.util.session
  "Maintains session resources during or across runs of the resolver."
  (:require [clojure.tools.deps.util.port :as port]))

(def session (atom {}))

(defn retrieve
  "Read the current value of key from the session. If absent, and if-absent-fn
  is supplied, invoke the fn, set it in the session (if there is one),
  and return the value."
  ([key]
   (get @session key))
  ([key if-absent-fn]
   (if (contains? @session key)
     (get @session key)
     (let [v (if-absent-fn)]
       (swap! session assoc key v)
       v))))

(defn retrieve-local
  "Like retrieve, but scoped to a thread-specific key, so never shared across threads."
  ([key]
   (retrieve {:thread (port/thread-id) :key key}))
  ([key if-absent-fn]
   (retrieve {:thread (port/thread-id) :key key} if-absent-fn)))

(defmacro with-session
  "Create a new empty session and execute the body"
  [& body]
  (let [prior-sym (gensym "prior")
        session-var (list 'var 'clojure.tools.deps.util.session/session)
        prior (list 'clojure.core/deref session-var)
        fresh (list 'clojure.core/alter-var-root
                session-var
                (list 'clojure.core/constantly
                  (list 'clojure.core/atom {})))
        restore (list 'clojure.core/alter-var-root
                  session-var
                  (list 'clojure.core/constantly prior-sym))]
    (apply list 'let
      (vector prior-sym prior)
      (list fresh
        (apply list 'try
          (concat body
            (list (list 'finally restore))))))))
