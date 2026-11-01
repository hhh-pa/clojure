;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: port of clojure.tools.deps.util.io
;; (org.clojure/tools.deps 0.31.1646, util/io.clj) — `printerrln`
;; only. `read-edn`/`slurp-edn`/`write-file` are deferred: nothing
;; in this slice needs them (the `edn` ns owns reading).
;;
;; Port adaptations (see docs/gap-124/slice-2.md B7/B12):
;; - `join-space` stands in for `clojure.string/join`
;;   (`clojure.string` is unported — the slice-1 `edn` ns carries
;;   its own copy too, mirroring upstream, whose `edn.clj` cannot
;;   depend on this ns).
;; - `nl` pins `\n` (A7 — upstream reads `line.separator`).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.util.io
  "IO utilities. Consider using clojure.tools.deps.edn for reading deps.edn instead.
  IMPL namespace, subject to change without warning.
  (Slice 2 port: `printerrln` only.)")

(def ^:private nl "\n")

(defn- join-space
  "`clojure.string/join` over `\" \"` (clojure.string is unported)."
  [msgs]
  (apply str (interpose " " msgs)))

(defn printerrln
  "println to *err*"
  [& msgs]
  (binding [*out* *err*
            *print-readably* nil]
    (pr (str (join-space msgs) nl))
    (flush)))
