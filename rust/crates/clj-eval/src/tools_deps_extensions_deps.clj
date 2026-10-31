;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: whole-file port of
;; clojure.tools.deps.extensions.deps (org.clojure/tools.deps
;; 0.31.1646, extensions/deps.clj) — the `:deps` manifest methods
;; (coord-deps, coord-paths, manifest-file, coord-usage,
;; prep-command) over string paths.
;;
;; Port adaptations (see docs/gap-124/slice-2.md B3/B4/B5):
;; - The session key and `manifest-file` use canonical paths
;;   (upstream: absolute); the read itself stays cwd-relative for
;;   relative roots, as upstream.
;; - `coord-paths` canonicalizes once (the port's `canonicalize`
;;   returns strings; upstream's File→string final pass is
;;   unneeded).
;; - `child-path` joins dir/file-name with `/` (A7); a nil dir
;;   answers the bare name (upstream `(jio/file nil f)` parity).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.extensions.deps
  (:require [clojure.tools.deps.edn :as depsedn]
            [clojure.tools.deps.extensions :as ext]
            [clojure.tools.deps.util.dir :as dir]
            [clojure.tools.deps.util.io :as io]
            [clojure.tools.deps.util.port :as port]
            [clojure.tools.deps.util.session :as session]))

(def ^:private file-separator "/")

(defn- child-path
  "Join dir and a child file name (upstream `jio/file` 2-arity): a
  nil dir answers the bare name (upstream parity — `(jio/file nil
  f)` is cwd-relative)."
  [dir file-name]
  (if (nil? dir)
    file-name
    (str dir file-separator file-name)))

(defn- deps-map
  [config dir]
  (let [f (child-path dir "deps.edn")]
    (session/retrieve
      {:deps :map :file (port/canonical-path f)} ;; session key
      #(if (port/file-exists? f)
         (depsedn/merge-edns [(depsedn/root-deps) (depsedn/read-deps f)])
         (depsedn/root-deps)))))

(defmethod ext/coord-deps :deps
  [_lib {:keys [deps/root] :as _coord} _mf config]
  (dir/with-dir root
    (seq (:deps (deps-map config root)))))

(defmethod ext/coord-paths :deps
  [_lib {:keys [deps/root] :as _coord} _mf config]
  (dir/with-dir root
    (->> (:paths (deps-map config root))
      (map #(dir/canonicalize %))
      (map #(do
              (when (not (dir/sub-path? %))
                (io/printerrln "WARNING: Deprecated use of path" % "external to project" root))
              %))
      vec)))

(defmethod ext/manifest-file :deps
  [_lib {:keys [deps/root] :as _coord} _mf _config]
  (let [manifest (child-path root "deps.edn")]
    (when (port/file-exists? manifest)
      (port/canonical-path manifest))))

(defmethod ext/coord-usage :deps [lib {:keys [deps/root] :as _coord} manifest-type config]
  (dir/with-dir root
    (:tools/usage (deps-map config root))))

(defmethod ext/prep-command :deps [lib {:keys [deps/root] :as _coord} manifest-type config]
  (dir/with-dir root
    (let [external-deps (deps-map config root)]
      (when-let [prep-info (:deps/prep-lib external-deps)]
        (let [exec-args (-> external-deps :aliases (get (:alias prep-info)) :exec-args)]
          (cond-> prep-info
            exec-args (assoc :exec-args exec-args)))))))
