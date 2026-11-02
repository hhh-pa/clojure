;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/uuid.clj (clojure.uuid) for clojure-rs:
;; same public API (none: the only defn is private, as upstream).
;;
;; Port notes:
;; - default-uuid-reader parses via clojure.core/parse-uuid (upstream calls
;;   java.util.UUID/fromString, which has no emulation). Bad strings throw
;;   ex-info (upstream: IllegalArgumentException "Invalid UUID string: ...",
;;   whose message is kept verbatim); non-strings throw ex-info (upstream:
;;   IllegalArgumentException "#uuid data reader expected string", likewise).
;;   There is no (IllegalArgumentException. ...) ctor without a JVM.
;; - The print-method / print-dup defmethods for java.util.UUID are dropped:
;;   the engine already prints values as #uuid "...", and print-method here
;;   dispatches on token strings, so class-dispatched methods could never fire.

(ns clojure.uuid)

(defn- default-uuid-reader [form]
  (if (string? form)
    (or (parse-uuid form)
        (throw (ex-info (str "Invalid UUID string: " form) nil)))
    (throw (ex-info "#uuid data reader expected string" nil))))
