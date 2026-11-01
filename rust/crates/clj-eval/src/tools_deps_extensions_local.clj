;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: port of clojure.tools.deps.extensions.local
;; (org.clojure/tools.deps 0.31.1646, extensions/local.clj) minus
;; jar reading (refused by redesign) — coord-type-keys, dep-id,
;; canonicalize, manifest-type, find-versions, compare-versions,
;; coord-summary, license-info, and the `:jar` manifest rules.
;;
;; Port decisions (see docs/gap-124/slice-2.md R2/B8/B10):
;; - `lib-location :local` stays in the shell (B8 layout rule —
;;   the shell owns all of `lib-location`).
;; - A file `:local/root` is REFUSED in `manifest-type` (R2), as
;;   are `coord-deps`/`coord-paths :jar` (one message, three
;;   sites); `manifest-file`/usage/prep `:jar` stay
;;   upstream-exact nils, license-info falls to `:default` nil.
;; - `dir/canonicalize` already returns the canonical string, so
;;   the upstream `.getCanonicalPath` wrapper is unneeded.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.extensions.local
  (:require [clojure.tools.deps.extensions :as ext]
            [clojure.tools.deps.util.dir :as dir]
            [clojure.tools.deps.util.port :as port]))

(defmethod ext/coord-type-keys :local
  [_type]
  #{:local/root})

(defmethod ext/dep-id :local
  [lib {:keys [local/root] :as _coord} _config]
  {:lib lib
   :root root})

(defn- ensure-file
  [lib root]
  (if (port/file-exists? root)
    root
    (throw (ex-info (format "Local lib %s not found: %s" lib root) {:lib lib :root root}))))

(defn- throw-jar-refusal
  [lib coord]
  (throw (ex-info (str "Source-only port: cannot use jar manifest for " lib " (source directories only)")
           {:lib lib :coord coord})))

(defmethod ext/canonicalize :local
  [lib {:keys [local/root] :as coord} _config]
  (let [canonical-root (dir/canonicalize root)]
    (ensure-file lib canonical-root) ;; throw if missing
    [lib (assoc coord :local/root canonical-root)]))

(defmethod ext/find-versions :local
  [_lib _coord _type _config]
  nil)

(defmethod ext/manifest-type :local
  [lib {:keys [local/root deps/manifest] :as coord} _config]
  (cond
    manifest {:deps/manifest manifest :deps/root root}
    (port/is-file? (ensure-file lib root)) (throw-jar-refusal lib coord)
    :else (ext/detect-manifest root)))

(defmethod ext/coord-summary :local [lib {:keys [local/root]}]
  (str lib " " root))

(defmethod ext/license-info :local
  [lib coord config]
  (let [coord (merge coord (ext/manifest-type lib coord config))]
    (ext/license-info-mf lib coord (:deps/manifest coord) config)))

(defmethod ext/compare-versions [:local :local]
  [lib {x-root :local/root :as x} {y-root :local/root :as y} _config]
  (if (= x-root y-root)
    0
    (throw (ex-info (str "No known ancestor relationship between local versions for " lib ": " x-root " and " y-root)
                    {:lib lib :x x :y y}))))

;; `:jar` rules (R2/B10): deps/paths refuse (empty deps would
;; lie; jars never reach the load path); the manifest file is nil
;; (upstream-exact); usage/prep are nil (upstream TBD-exact);
;; license-info falls to `:default` nil ("unknown").

(defmethod ext/coord-deps :jar
  [lib coord _manifest _config]
  (throw-jar-refusal lib coord))

(defmethod ext/coord-paths :jar
  [lib coord _manifest _config]
  (throw-jar-refusal lib coord))

(defmethod ext/manifest-file :jar
  [_lib {:keys [deps/root] :as _coord} _mf _config]
  nil)

(defmethod ext/coord-usage :jar
  [_lib _coord _manifest-type _config]
  ;; TBD
  nil)

(defmethod ext/prep-command :jar
  [_lib _coord _manifest-type _config]
  ;; TBD
  nil)
