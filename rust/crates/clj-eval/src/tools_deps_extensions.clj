;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: full port of clojure.tools.deps.extensions
;; (org.clojure/tools.deps 0.31.1646, extensions.clj) — the whole
;; SPI (manifest detection, coord-type registry, every coord and
;; manifest multimethod with upstream-exact defaults) plus the
;; redesign markers (`:mvn` refusal, `:git` not-yet, `:pom`
;; refusals). Slice 1 covered `lib-location` only.
;;
;; Port decisions (see docs/gap-124/slice-2.md B1/B5/B6/B8/B9):
;; - `coord-type` keeps slice-1's hardcoded kind detection: the
;;   port's defmulti has no `getMethodTable`, so `procurer-types`
;;   is the fixed embedded set and the registry cannot drive
;;   dispatch. Verdicts match upstream exactly (anti-drift test).
;; - Layout rule (B8): the shell owns ALL of `lib-location` (it
;;   must host the `:mvn` refusal, which has no namespace);
;;   extension namespaces carry the remaining SPI methods.
;; - The `:mvn` `coord-type-keys` stub lives here (same reason).
;; - Vector dispatch (`compare-versions`) resolves by exact table
;;   match; cross-type pairs fall to `:default` (B6).
;; - `child-path` joins dir/file-name with `/` (A7); a nil dir
;;   answers the bare name (upstream `(jio/file nil f)` parity).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.extensions
  "Defines the SPI for new dep and manifest types - to implement, extend the
  multimethods here, then load those extensions before using the tools.deps API.
  (Slice 2: full SPI; `lib-location` methods live here by the B8 layout rule.)"
  (:require [clojure.tools.deps.util.port :as port]))

(def ^:private file-separator "/")

(defn- child-path
  "Join dir and a child file name (upstream `jio/file` 2-arity): a
  nil dir answers the bare name (upstream parity — `(jio/file nil
  f)` is cwd-relative)."
  [dir file-name]
  (if (nil? dir)
    file-name
    (str dir file-separator file-name)))

;; Helper for autodetect of manifest type

;; vector to control ordering
(def manifest-types
  ["deps.edn" :deps,
   "pom.xml" :pom
   ;; "project.clj" :lein
   ])

(defn detect-manifest
  "Given a directory, detect the manifest type and return the manifest info."
  [dir]
  (loop [[file-name manifest-type & others] manifest-types]
    (when file-name
      (let [f (child-path dir file-name)]
        (if (and (port/file-exists? f) (port/is-file? f))
          {:deps/manifest manifest-type, :deps/root dir}
          (recur others))))))

;; Methods switching on coordinate type

(defmulti coord-type-keys
  "Takes a coordinate type and returns valid set of keys indicating that coord type"
  (fn [type] type))

(defmethod coord-type-keys :default [_type] #{})

(defmethod coord-type-keys :mvn
  ;; Refusal stub (B1/B8): no maven namespace will ever exist, so
  ;; the `:mvn` kind keys live here next to the `:mvn`
  ;; `lib-location` refusal.
  [_type]
  #{:mvn/version})

(defn procurer-types
  "Returns the set of registered procurer types. (Port: the fixed
  embedded set — upstream reads the `coord-type-keys` method table,
  which the port's defmulti does not expose. See B1.)"
  []
  #{:local :git :mvn})

(defn- coord-type
  "Determine the coordinate type of the coordinate. (Port: fixed
  kind detection with the same verdicts as upstream's
  procurer-key version — zero matches throw `Coord of unknown
  type`, two or more throw `Coord type is ambiguous`, both with
  `{:coord}` only. See B1.)"
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
  (throw (ex-info (str "Cannot resolve git coordinate for " lib ": deferred to the git-network slice")
           {:lib lib :coord coord})))

(defmethod lib-location :default
  ;; Non-map coords only (unknown/ambiguous maps throw from
  ;; dispatch): upstream `throw-bad-coord` non-map arm, exactly.
  [lib coord _config]
  (throw (ex-info (str "Bad coordinate for library " lib ", expected map: " (pr-str coord))
           {:lib lib :coord coord})))

(defmulti canonicalize
  "Takes a lib and coordinate and returns a canonical form.
  For example, a Maven coordinate might resolve LATEST to a version or a Git
  coordinate might resolve a partial sha to a full sha. Returns [lib coord]."
  (fn [lib coord config] (coord-type coord)))

(defmethod canonicalize :default [lib coord config]
  [lib coord])

(defmulti dep-id
  "Returns an identifier value that can be used to detect a lib/coord cycle while
   expanding deps. This will only be called after canonicalization so it can rely
   on the canonical form."
  (fn [lib coord config] (coord-type coord)))

(defn- throw-bad-coord
  [lib coord]
  (if (map? coord)
    (let [type (coord-type coord)]
      (if (nil? type)
        (throw (ex-info (str "No coordinate type found for library " lib " in coordinate " (pr-str coord)) {:lib lib :coord coord}))
        (throw (ex-info (str "Coordinate type " type " not loaded for library " lib " in coordinate " (pr-str coord))
                 {:lib lib :coord coord}))))
    (throw (ex-info (str "Bad coordinate for library " lib ", expected map: " (pr-str coord)) {:lib lib :coord coord}))))

(defmethod dep-id :default [lib coord config]
  (throw-bad-coord lib coord))

(defmulti manifest-type
  "Takes a lib, a coord, and the root config. Dispatch based on the
   coordinate type. Detect and return the manifest type and location
   for this coordinate."
  (fn [lib coord config] (coord-type coord)))

(defmethod manifest-type :default [lib coord config]
  (throw-bad-coord lib coord))

(defmulti coord-summary
  "Takes a coord, and returns a concise description, used when printing tree"
  (fn [lib coord] (coord-type coord)))

(defmethod coord-summary :default [lib coord]
  (str lib " " (coord-type coord)))

(defmulti license-info
  "Return map of license info (:name and :url) or nil if unknown"
  (fn [lib coord config] (coord-type coord)))

(defmethod license-info :default [lib coord config] nil)

;; Version comparison, either within or across coordinate types

(defmulti compare-versions
  "Given two coordinates, use this as a comparator returning a negative number, zero,
  or positive number when coord-x is logically 'less than', 'equal to', or 'greater than'
  coord-y. The dispatch occurs on the type of x and y."
  (fn [lib coord-x coord-y config] [(coord-type coord-x) (coord-type coord-y)]))

(defmethod compare-versions :default
  [lib coord-x coord-y config]
  (throw (ex-info (str "Unable to compare versions for " lib ": " (pr-str coord-x) " and " (pr-str coord-y))
           {:lib lib :coord-x coord-x :coord-y coord-y})))

;; Find coords

(defmulti find-versions
  "Return a coll of coords based on a lib and a partial coord"
  (fn [lib coord coord-type config] coord-type))

(defmethod find-versions :default [lib coord coord-type config]
  (throw-bad-coord lib coord))

(defn find-all-versions
  "Find versions across all registered procurer types.
  Returns coll of coordinates for this lib (based on lib and partial coordinate).
  For each procurer type, coordinates are returned in chronological order."
  [lib coord config]
  (mapcat #(find-versions lib coord % config) (procurer-types)))

;; Methods switching on manifest type

(defn- throw-bad-manifest
  [lib coord manifest-type]
  (if manifest-type
    (throw (ex-info (str "Manifest type " manifest-type " not loaded for " lib " in coordinate " (pr-str coord))
             {:lib lib :coord coord}))
    (throw (ex-info (str "Manifest file not found for " lib " in coordinate " (pr-str coord))
             {:lib lib :coord coord}))))

(defmulti coord-deps
  "Return coll of immediate [lib coord] external deps for this library."
  (fn [lib coord manifest-type config] manifest-type))

(defmethod coord-deps :default [lib coord manifest-type config]
  (throw-bad-manifest lib coord manifest-type))

(defmulti coord-paths
  "Return coll of classpath roots for this library on disk."
  (fn [lib coord manifest-type config] manifest-type))

(defmethod coord-paths :default [lib coord manifest-type config]
  (throw-bad-manifest lib coord manifest-type))

(defmulti manifest-file
  "Return path to manifest file (if any). If this file is updated,
  causes the cache to be recomputed."
  (fn [lib coord manifest-type config] manifest-type))

(defmethod manifest-file :default [lib coord manifest-type config]
  (throw-bad-manifest lib coord manifest-type))

(defmulti license-info-mf
  "Return map of license info (:name and :url) or nil if unknown
  based on the manifest."
  (fn [lib coord manifest-type config] manifest-type))

(defmethod license-info-mf :default [lib coord manifest-type config] nil)

(defmulti coord-usage
  "Return usage info map for this library with the following optional keys:
    :ns-default - default namespace symbol
    :ns-aliases - map of alias to namespace symbol"
  (fn [lib coord manifest-type config] manifest-type))

(defmethod coord-usage :default [lib coord manifest-type config]
  (throw-bad-manifest lib coord manifest-type))

(defmulti prep-command
  "Return prep command for this library with the following keys:
    :alias - alias to use when invoking (keyword)
    :fn - function to invoke in alias (symbol)
    :ensure - relative path in repo to ensure exists after prep"
  (fn [lib coord manifest-type config] manifest-type))

(defmethod prep-command :default [lib coord manifest-type config]
  (throw-bad-manifest lib coord manifest-type))

;; `:pom` markers (R1 corollary, B9/B10): pom manifests are
;; detected but never read — deps/paths refuse source-only (empty
;; deps would lie; the port reads no XML), the manifest file stays
;; a faithful path (no XML involved), usage/prep stay
;; upstream-exact nils, and license-info falls to `:default` nil.

(defmethod coord-deps :pom
  [lib coord _manifest _config]
  (throw (ex-info (str "Source-only port: cannot read pom manifest for " lib " (no XML in the source-only port)")
           {:lib lib :coord coord})))

(defmethod coord-paths :pom
  [lib coord _manifest _config]
  (throw (ex-info (str "Source-only port: cannot read pom manifest for " lib " (no XML in the source-only port)")
           {:lib lib :coord coord})))

(defmethod manifest-file :pom
  [_lib {:keys [deps/root] :as _coord} _mf _config]
  (port/canonical-path (child-path root "pom.xml")))

(defmethod coord-usage :pom
  [_lib _coord _manifest-type _config]
  ;; TBD
  nil)

(defmethod prep-command :pom
  [_lib _coord _manifest-type _config]
  ;; TBD
  nil)
