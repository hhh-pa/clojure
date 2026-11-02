;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

(ns ^{:doc "Functions to turn objects into data. Alpha, subject to change"}
  clojure.datafy)

;; W1 port: upstream requires clojure.core.protocols and defers to its
;; Datafiable/Navigable protocols (identity defaults; user-extendable,
;; including via metadata). That namespace is owned by W8 and is not
;; embedded here, so this port dispatches on predicates with the same
;; defaults: unknown values datafy to themselves, nav returns v. Gaps:
;; user extensions to clojure.core.protocols do not take effect here,
;; and :extend-via-metadata is not honored.
;; (Upstream also sets *warn-on-reflection* true; the port rejects
;; set! on an unbound root and has no reflection warnings.)

(defn- sortmap [m]
  (into (sorted-map) m))

(defn- datafy-value
  [x]
  (cond (instance? Throwable x)
        (Throwable->map x)

        (instance? clojure.lang.IRef x)
        (with-meta [(deref x)] (meta x))

        (instance? clojure.lang.Namespace x)
        ;; W1 port: upstream reads (.getName n); the port's namespace
        ;; handles answer ns-name instead (same name, as a string).
        (with-meta {:name (str (ns-name x))
                    :publics (-> x ns-publics sortmap)
                    :imports (-> x ns-imports sortmap)
                    :interns (-> x ns-interns sortmap)}
          (meta x))

        (class? x)
        (let [{:keys [members] :as ret} ((requiring-resolve 'clojure.reflect/reflect) x)]
          (assoc ret :name (-> x .getName symbol) :members (->> members (group-by :name) sortmap)))

        :else x))

(defn datafy
  "Attempts to return x as data.
  datafy will return the value of clojure.core.protocols/datafy. If
  the value has been transformed and the result supports
  metadata, :clojure.datafy/obj will be set on the metadata to the
  original value of x, and :clojure.datafy/class to the name of the
  class of x, as a symbol."
  [x]
  (let [v (datafy-value x)]
    (if (identical? v x)
      v
      (if (instance? clojure.lang.IObj v)
        ;; W1 port: upstream writes ::obj/::class; load reads the whole
        ;; file before the ns form evaluates, so auto-resolve lands in
        ;; the caller's ns — write the upstream keys out (spec_alpha
        ;; precedent: no ::auto-resolve in embedded files).
        (vary-meta v assoc :clojure.datafy/obj x :clojure.datafy/class (-> x class .getName symbol))
        v))))

(defn nav
  "Returns (possibly transformed) v in the context of coll and k (a
  key/index or nil). Callers should attempt to provide the key/index
  context k for Indexed/Associative/ILookup colls if possible, but not
  to fabricate one e.g. for sequences (pass nil). nav returns the
  value of clojure.core.protocols/nav."
  [coll k v]
  ;; W1 port: upstream delegates to clojure.core.protocols/nav, whose
  ;; only (Object) implementation returns v; see the dispatch note above.
  v)
