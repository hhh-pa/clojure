;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: port of clojure.tools.deps.util.session
;; (org.clojure/tools.deps 0.31.1646, util/session.clj) — `session`
;; + `retrieve` only. `with-session`/`retrieve-local` arrive with
;; the API ns in slice 3 (nothing here needs scoping yet).
;;
;; Port adaptations (see docs/gap-124/slice-2.md B2):
;; - `session` is an `(atom {})`, not a `ConcurrentHashMap`;
;;   `retrieve` is check-then-swap (a repeated compute under races
;;   is benign — cached values are content-derived).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.util.session
  "Maintains session resources during or across runs of the resolver.
  (Slice 2 port: `session` + `retrieve` only.)")

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
