;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 3: port of clojure.tools.deps.util.concurrent
;; (org.clojure/tools.deps 0.31.1646, util/concurrent.clj) over the
;; port's own futures — `new-executor`, `processors`,
;; `submit-task`, `shutdown-on-error`. The JVM `thread-factory` has
;; no counterpart (`future-call` owns its threads) and is not
;; ported.
;;
;; Port adaptations (see docs/gap-124/slice-3.md C4):
;; - `new-executor` returns an opaque `{:threads n}` token (no
;;   `ExecutorService`); `submit-task` is `future-call`, whose
;;   binding conveyance matches upstream's `bound-fn*` + submit.
;; - `shutdown-on-error` is a nil no-op: nothing to shut down —
;;   in-flight local reads complete harmlessly, and every caller
;;   throws right after.
;; - `processors` reads the `port/available-processors` native.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.util.concurrent
  "Concurrency utilities.
  IMPL namespace, subject to change without warning."
  (:require [clojure.tools.deps.util.port :as port]))

(def processors (port/available-processors))

(defn new-executor
  "Create an executor token for n worker threads (port: an opaque
  map — upstream returns a fixed JVM thread pool)."
  [n]
  {:threads n})

(defn submit-task
  "Submit f to the executor and return a future (port:
  `future-call`, which conveys bindings like upstream's
  `bound-fn*`)."
  [executor f]
  (future-call f))

(defn shutdown-on-error
  "Shut the executor down after an error (port: no-op — there is no
  pool to shut down; in-flight tasks complete harmlessly)."
  [executor]
  nil)
