;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/java/browse.clj (clojure.java.browse)
;; for clojure-rs: same public API (*open-url-script*, browse-url).
;;
;; Port notes:
;; - The (:import (java.io File) (java.net URI) (java.lang ProcessBuilder
;;   ProcessBuilder$Redirect)) clause is dropped (none of those classes is in
;;   the emulated lattice).
;; - clojure.string is NOT required (it cannot load at top level while the
;;   loader leaves *warn-on-reflection* unbound): which(1) output is trimmed
;;   with .trim instead of str/trim-newline (identical for that output).
;; - open-url-in-browser (java.awt.Desktop) is dropped with its private
;;   helper open-url-in-browser: there is no AWT without a JVM, and its
;;   Reflector probe cannot exist here. The open command (the script path)
;;   is the Desktop equivalent.
;; - browse-url spawns the open command in a future running clojure.java.shell
;;   (upstream: a detached ProcessBuilder with output discarded, never waited
;;   on). The call never blocks; output is captured instead of discarded, and
;;   a spawn failure stays silent inside the future (so the Swing fallback is
;;   only reached when no open command was found, unlike upstream where a
;;   failed start also falls through).
;; - open-url-in-swing is unchanged (it requires clojure.java.browse-ui on
;;   demand, which lands with its own workstream).

(ns
  ^{:author "Christophe Grand",
    :doc "Start a web browser from Clojure"}
  clojure.java.browse
  (:require [clojure.java.shell :as sh]))

(defn- macosx? []
  (-> "os.name" System/getProperty .toLowerCase
    (.startsWith "mac os x")))

(defn- xdg-open-loc []
  ;; try/catch needed to mask exception on Windows without Cygwin
  (let [which-out (try (:out (sh/sh "which" "xdg-open"))
                       (catch Exception e ""))]
    (if (= which-out "")
      nil
      ;; Upstream: (str/trim-newline which-out).
      (.trim which-out))))

(defn- open-url-script-val []
  (if (macosx?)
    "/usr/bin/open"
    (xdg-open-loc)))

;; We could assign (open-url-script-val) to *open-url-script* right
;; away in the def below, but clojure.java.shell/sh creates a future
;; that causes a long wait for the JVM to exit during Clojure compiles
;; (unless we can somehow here make it call (shutdown-agents) later).
;; Better to initialize it when we first need it, in browse-url.

(def ^:dynamic *open-url-script* (atom :uninitialized))

(defn- open-url-in-swing
 "Opens url (a string) in a Swing window."
 [url]
  ; the implementation of this function resides in another namespace to be loaded "on demand"
  ; this fixes a bug on mac os x where the process turns into a GUI app
  ; see http://code.google.com/p/clojure-contrib/issues/detail?id=32
  (require 'clojure.java.browse-ui)
  ((find-var 'clojure.java.browse-ui/open-url-in-swing) url))

(defn browse-url
  "Open url in a browser"
  {:added "1.2"}
  [url]
  (let [script @*open-url-script*
        script (if (= :uninitialized script)
                 (reset! *open-url-script* (open-url-script-val))
                 script)]
    (or (when script
          (try
            ;; Upstream: detached (.start pb) with stdout/stderr discarded.
            (future (sh/sh script (str url)))
            true
            (catch Throwable _ false)))
        (open-url-in-swing url))))
