;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 1: shell port of clojure.tools.deps.extensions
;; (org.clojure/tools.deps 0.31.1646, extensions.clj +
;; extensions/local.clj) covering `lib-location` only — the full
;; multimethod SPI (`canonicalize`, `manifest-type`, ...) arrives
;; in later slices. Dispatch is upstream-exact: kind keys
;; `:local/root`, `:mvn/version`, `:git/url`/`:git/sha`/
;; `:git/tag`/`:sha`; zero matches throw `Coord of unknown type`
;; with `{:coord}` (during dispatch, as upstream `coord-type`);
;; two matches throw `Coord type is ambiguous` the same way.
;;
;; Port decisions (see docs/gap-124/slice-1.md R1/R2):
;; - `:mvn` is REFUSED (permanent redesign): Maven coords parse
;;   and validate (config compatibility), but resolving them
;;   throws an ex-info naming the lib with the source-only rule.
;; - `:git` is NOT-YET (temporary): it throws an ex-info
;;   deferring to the git slice.
;; - Non-map coords throw the `throw-bad-coord` non-map shape
;;   exactly; unknown/ambiguous coord maps throw from dispatch
;;   (`Coord of unknown type` / `Coord type is ambiguous` with
;;   `{:coord}` only — upstream `coord-type` parity).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.extensions
  "Defines the SPI for new dep and manifest types - to implement, extend the
  multimethods here, then load those extensions before using the tools.deps API.
  (Slice 1 shell: `lib-location` only.)")

(defn- coord-type
  [coord]
  (when (map? coord)
    (let [matches (cond-> []
                    (contains? coord :local/root) (conj :local)
                    (contains? coord :mvn/version) (conj :mvn)
                    (or (contains? coord :git/url)
                      (contains? coord :git/sha)
                      (contains? coord :git/tag)
                      (contains? coord :sha)) (conj :git))]
      (case (count matches)
        0 (throw (ex-info (str "Coord of unknown type: " (pr-str coord))
                   {:coord coord}))
        1 (first matches)
        (throw (ex-info (str "Coord type is ambiguous: " (pr-str coord))
                 {:coord coord}))))))

(defmulti lib-location
  "Takes a coordinate and returns the location where the lib would be
  installed locally. Location keys:

  :base     local repo base directory path
  :path     path within base dir
  :type     coordinate type"
  (fn [lib coord config] (coord-type coord)))

(defmethod lib-location :local
  [_lib coord _config]
  {:base (:local/root coord)
   :path ""
   :type :local})

(defmethod lib-location :mvn
  [lib coord _config]
  (throw (ex-info (str "Source-only port: cannot resolve Maven coordinate for " lib)
           {:lib lib :coord coord})))

(defmethod lib-location :git
  [lib coord _config]
  (throw (ex-info (str "Cannot resolve git coordinate for " lib ": deferred to the git slice")
           {:lib lib :coord coord})))

(defmethod lib-location :default
  ;; Non-map coords only (unknown/ambiguous maps throw from
  ;; dispatch): upstream `throw-bad-coord` non-map arm, exactly.
  [lib coord _config]
  (throw (ex-info (str "Bad coordinate for library " lib ", expected map: " (pr-str coord))
           {:lib lib :coord coord})))
