;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W7 (tools track, issue #204): port of upstream
;; `clojure/java/basis/impl.clj` over the embedded tools.deps
;; `port` substrate (no `clojure.edn`, no `clojure.java.io`
;; readers, no `PushbackReader`).
;;
;; Divergences from the upstream forms:
;; - Upstream `read-edn` coerces `f` via `jio/reader`, wraps it in
;;   a `PushbackReader`, and reads twice (`{:default tagged-literal
;;   :eof EOF}`) to enforce the single-value shape. The port reads
;;   `(slurp f)` and delegates to `port/read-edn-string`, which
;;   enforces the same single-value shape and reports the same
;;   `:path` ex-data. Unknown tagged elements throw instead of
;;   answering `tagged-literal` (basis files are plain data).
;; - Upstream `read-basis` builds a `jio/file` and tests `.exists`;
;;   the port works in path strings and tests `port/file-exists?`.
;; - `(set! *warn-on-reflection* true)` is dropped: reflection
;;   warnings are a JVM-compiler concept with no interpreter
;;   counterpart.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.java.basis.impl
  (:require
    [clojure.tools.deps.util.port :as port]))

(defn- read-edn
  "Read one edn value from the file at path f.
  Empty input returns nil; a second value throws ex-info."
  [f]
  (port/read-edn-string (slurp f) f))

(defn- read-basis
  "Read basis edn from basis file or throw"
  [basis-file]
  (when basis-file
    (when (port/file-exists? basis-file)
      (read-edn basis-file))))

;; delay construction until needed, access via initial-basis
(def init-basis
  (delay (read-basis (System/getProperty "clojure.basis"))))

;; delay construction until needed, access via current-basis
(def the-basis
  (delay (atom @init-basis)))

(defn update-basis!
  "Update the runtime basis by applying f with args"
  [f & args]
  (apply swap! @the-basis f args))
