;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W7 (tools track, issue #204): port of upstream
;; `clojure/repl/deps.clj`. Resolution runs in-process through the
;; embedded tools.deps namespaces (via
;; `clojure.tools.deps.interop/invoke-tool`); added libs become
;; `require`-able through the loader's `*load-path*` instead of a
;; `DynamicClassLoader`.
;;
;; Divergences from the upstream forms:
;; - Upstream `add-loader-url` walks to the topmost
;;   `DynamicClassLoader` and `.addURL`s each new jar/dir URL (or
;;   throws `IllegalAccessError`). There is no classloader here, so
;;   `add-load-path!` appends each new `:paths` entry (a source-dir
;;   string; jars cannot exist in the source-only tools.deps
;;   redesign) to the `clojure.core/*load-path*` root, deduplicated
;;   and order-preserving, so later `require`s resolve the new libs.
;; - Upstream `add-libs` reloads root `*data-readers*` via
;;   `clojure.core/load-data-readers` (which scans the classpath).
;;   The port has no such var: every read entry re-scans when the
;;   `*load-path*` value moved, so the readers refresh
;;   automatically once the paths land — the `set!` line is dropped.
;; - `(throw (RuntimeException. ...))` is `(throw (ex-info ... {}))`
;;   with the identical message (ctors cannot construct — the
;;   spec_alpha.clj precedent).
;; - `(:require [clojure.java.io :as jio])` and
;;   `(:import [clojure.lang DynamicClassLoader RT] [java.io File])`
;;   are dropped (no `File`/`URL`/classloader arms remain), as is
;;   `(set! *warn-on-reflection* true)` (see the basis.impl port).
;; - The single-arity `add-lib` resolves `find-latest-version`
;;   through the tool fn, which the embedded (source-only, no
;;   network `:mvn`) tools.deps excludes (gap-124 X1): it always
;;   throws the interop "not available" ex-info. Pass an explicit
;;   coord (2-arity) instead.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.repl.deps
  "clojure.repl.deps provides facilities for dynamically modifying the available
  libraries in the runtime when running at the REPL, without restarting"
  (:require
   [clojure.java.basis :as basis]
   [clojure.java.basis.impl :as basis-impl]
   [clojure.tools.deps.interop :as tool]))

(defn- add-load-path!
  "Append path string to the *load-path* root (deduped, order kept),
  so the loader resolves namespaces from newly added libs."
  [path]
  (alter-var-root #'clojure.core/*load-path*
    (fn [paths] (vec (distinct (conj (vec paths) path))))))

(defn add-libs
  "Given lib-coords, a map of lib to coord, will resolve all transitive deps for the libs
  together and add them to the repl classpath, unlike separate calls to add-lib."
  {:added "1.12"}
  [lib-coords]
  (when-not *repl* (throw (ex-info "add-libs is only available at the REPL" {})))
  (let [{:keys [libs] :as basis} (basis/current-basis)
        current-libs (set (keys libs))
        lib-coords (reduce-kv #(if (contains? current-libs %2) %1 (assoc %1 %2 %3))
                     {} lib-coords)]
    (when-not (empty? lib-coords)
      (let [procurer (reduce-kv (fn [m k v] (if (contains? #{"mvn" "git" "local"} (namespace k)) (assoc m k v) m)) {} basis)
            tool-args {:existing libs, :add lib-coords, :procurer procurer}
            {:keys [added] :as _res} (tool/invoke-tool {:tool-alias :deps, :fn 'clojure.tools.deps/resolve-added-libs, :args tool-args})
            paths (mapcat :paths (vals added))]
        (run! add-load-path! paths)
        (basis-impl/update-basis! update :libs merge added)
        (let [ret (-> added keys sort vec)]
          (when (seq ret) ret))))))

(defn add-lib
  "Given a lib that is not yet on the repl classpath, make it available by
  downloading the library if necessary and adding it to the classloader.
  Libs already on the classpath are not updated. Requires a valid parent
  DynamicClassLoader.

   lib - symbol identifying a library, for Maven: groupId/artifactId
   coord - optional map of location information specific to the procurer,
           or latest if not supplied

  Returns coll of libs loaded, including transitive (or nil if none).

  For info on libs, coords, and versions, see:
   https://clojure.org/reference/deps_and_cli"
  {:added "1.12"}
  ([lib coord]
   (add-libs {lib coord}))
  ([lib]
   (let [procurer (select-keys (basis/current-basis) [:mvn/repos :mvn/local-repo])
         coord (tool/invoke-tool {:tool-alias :deps
                                  :fn 'clojure.tools.deps/find-latest-version
                                  :args {:lib lib, :procurer procurer}})]
     (if coord
       (add-libs {lib coord})
       (throw (ex-info (str "No version found for lib " lib) {}))))))

(defn sync-deps
  "Calls add-libs with any libs present in deps.edn but not yet present on the classpath.

    :aliases - coll of alias keywords to use during the sync"
  {:added "1.12"}
  [& {:as opts}]
  (let [{:keys [aliases]} opts
        basis-config (:basis-config (basis/current-basis))
        new-basis-config (update basis-config :aliases (fnil into []) aliases)
        new-basis (tool/invoke-tool {:tool-alias :deps, :fn 'clojure.tools.deps/create-basis, :args new-basis-config})
        new-libs (:libs new-basis)]
    (add-libs new-libs)))
