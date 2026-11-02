;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; W6 port of upstream clojure/core_proxy.clj (oracle: upstream source
;; text, read-only; no JVM at runtime, ever).
;;
;; Upstream this file loads INTO clojure.core (via core.clj's `(load
;; ...)` — there is no clojure.core-proxy namespace to alias, and a
;; require with :as/:use still fails here for the same reason).
;; Every public it defines already lives in this port's clojure.core,
;; so this file intentionally defines nothing: requiring
;; `clojure.core-proxy` only marks the lib loaded, and asserts the
;; API is present so drift fails loudly.
;;
;; Upstream def → port home:
;; - method-sig, proxy-name, get-proxy-class, construct-proxy,
;;   init-proxy, update-proxy, proxy-mappings, proxy-call-with-super
;;   → clojure.core natives (typeinternals.rs, zero-oos S14).
;; - proxy, proxy-super → clojure.core macros (core.clj prelude).
;; - bean → clojure.core native (classemu.rs).
;; - most-specific, group-by-sig, generate-proxy,
;;   get-super-and-interfaces → N/A (JVM bytecode emission; the
;;   port's __proxy* native builds proxies over the deftype/reify
;;   machinery instead).

(in-ns 'clojure.core)

(doseq [s '[proxy proxy-super proxy-mappings get-proxy-class
           construct-proxy init-proxy update-proxy
           proxy-call-with-super proxy-name method-sig bean]]
  (when-not (resolve s)
    (throw (ex-info (str "clojure.core-proxy: missing clojure.core/" s) {}))))
