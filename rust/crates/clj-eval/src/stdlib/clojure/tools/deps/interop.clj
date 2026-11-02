;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W7 (tools track, issue #204): port of upstream
;; `clojure/tools/deps/interop.clj`. Upstream shells out to the
;; Clojure CLI (`clojure.java.process`, `-T` tool invocation over
;; stdin/stdout envelopes); this runtime has no subprocess CLI, so
;; `invoke-tool` dispatches the `:fn` in-process against the
;; embedded namespaces instead.
;;
;; Divergences from the upstream forms:
;; - The private `build` atom, `cli-build`, and `validate-version`
;;   are dropped (they shell `clojure --version`; there is no CLI
;;   binary to gate on).
;; - The tool fn runs via `requiring-resolve` + one call with the
;;   args map (which still carries `:clojure.exec/invoke :fn`,
;;   exactly what the CLI would hand the fn). A fn that does not
;;   resolve throws ex-info naming it (this is how the excluded
;;   `clojure.tools.deps/find-latest-version` surfaces — gap-124
;;   X1 — instead of a CLI "unknown error" envelope).
;; - `:preserve-envelope true` answers the emulated CLI envelope
;;   `{:tag :ret, :val (pr-str ret)}` on success and
;;   `{:tag :err, :val (pr-str {:cause ... :data ...})}` on tool
;;   failure; without it the return value (or the thrown error
;;   itself, rather than the CLI's ex-info re-wrap) passes
;;   straight through.
;; - `(:require [clojure.java.process :as proc] [clojure.edn :as edn]
;;   [clojure.java.io :as jio] [clojure.string :as str])` is dropped:
;;   no arm of the port uses those namespaces.
;; - `(set! *warn-on-reflection* true)` is dropped (JVM-compiler
;;   concept; see the basis.impl port).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.interop
  "Functions for invoking Java processes and invoking tools via the Clojure CLI.")

(defn ^:dynamic invoke-tool
  "Invoke tool using Clojure CLI. Args (one of :tool-alias or :tool-name, and :fn
  are required):
    :tool-alias - Tool alias to invoke (keyword)
    :tool-name - Name of installed tool to invoke (string or symbol)
    :fn - Function (symbol)
    :args - map of args to pass to function

  Options:
    :preserve-envelope - if true, return the full invocation envelope, default=false"
  {:added "1.12"}
  [{:keys [tool-name tool-alias fn args preserve-envelope]
    :or {preserve-envelope false}
    :as opts}]
  (when-not (or tool-name tool-alias) (throw (ex-info "Either :tool-alias or :tool-name must be provided" (or opts {}))))
  (when-not (symbol? fn) (throw (ex-info (str ":fn should be a symbol " fn) (or opts {}))))
  (let [tool-fn (requiring-resolve fn)]
    (when-not tool-fn
      (throw (ex-info (str "Tool fn is not available in this runtime: " fn) (or opts {}))))
    (let [tool-args (assoc args :clojure.exec/invoke :fn)
          _ (when (:debug opts) (println "args" [fn tool-args]))
          _ (when (:debug opts) (println "Invoking: " fn))]
      (try
        (let [ret (tool-fn tool-args)]
          (if preserve-envelope
            {:tag :ret :val (pr-str ret)}
            ret))
        (catch Throwable t
          (if preserve-envelope
            {:tag :err :val (pr-str {:cause (ex-message t) :data (ex-data t)})}
            (throw t)))))))

(comment
  ;; regular invocation, should return {:hi :there}
  (invoke-tool {:tool-alias :deps, :fn 'clojure.core/identity, :args {:hi :there}})

  ;; invocation throws, should return throwable map data
  (try
    (invoke-tool {:tool-alias :deps, :fn 'clojure.core/+, :args {:fail :here}})
    (catch clojure.lang.ExceptionInfo e (ex-data e)))

  ;; capture stdout in returned envelope
  (let [resp (invoke-tool {:tool-alias :deps,
                           :fn 'list
                           :args {:format :edn
                                  :clojure.exec/out :capture}
                           :preserve-envelope true})]
    (edn/read-string (:out resp)))

  )
