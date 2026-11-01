;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-120 slice 3: port of clojure.java.shell (clojure 1.12.6,
;; java/shell.clj) — `sh` over a spawn native, `with-sh-dir` /
;; `with-sh-env` over `binding`.
;;
;; Port narrowings (all pinned, see tests/java_shell.rs):
;; - `:in` takes strings and byte arrays only (streams/Readers/Files
;;   need the `clojure.java.io` slice, which has not landed).
;; - `:in-enc` takes UTF-8 only (the port has charset decoders but
;;   no encoders); anything else is `UnsupportedEncodingException`.
;; - `:out-enc` takes `:bytes` or any name the shared charset
;;   tables know (UTF-8/16/32, ASCII, Latin-1 + aliases).
;; - `:dir` takes strings only (same jio reason).
;; - `:env` takes maps only (upstream also admits a raw String[]).
;; - `:err` decodes lossy UTF-8 (upstream uses the platform
;;   default — identical on UTF-8 platforms).
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.java.shell
  "Conveniently launch a sub-process providing its stdin and
  collecting its stdout. Port: clojure.java.io sources for :in
  and :dir arrive with the jio slice; see the header."
  (:require [clojure.java.shell.impl :as impl]))

(def ^:dynamic *sh-dir* nil)
(def ^:dynamic *sh-env* nil)

(defmacro with-sh-dir
  "Sets the directory for use with sh, see sh for details."
  {:added "1.2"}
  [dir & forms]
  (apply list 'clojure.core/binding
    (vector (symbol "clojure.java.shell" "*sh-dir*") dir)
    forms))

(defmacro with-sh-env
  "Sets the environment for use with sh, see sh for details."
  {:added "1.2"}
  [env & forms]
  (apply list 'clojure.core/binding
    (vector (symbol "clojure.java.shell" "*sh-env*") env)
    forms))

(defn- parse-args
  [args]
  (let [default-encoding "UTF-8"
        default-opts {:out-enc default-encoding :in-enc default-encoding :dir *sh-dir* :env *sh-env*}
        [cmd opts] (split-with string? args)]
    [cmd (merge default-opts (apply hash-map opts))]))

(defn sh
  "Passes the given strings to launch a sub-process.

  Options are

  :in      may be given followed by a String or byte array to be
           fed to the sub-process's stdin. (Port: streams/Readers/
           Files need clojure.java.io.)
  :in-enc  option may be given followed by \"UTF-8\" (the only
           supported name — the port has no charset encoders).
           Defaults to UTF-8. If :in provides a byte array, the
           bytes pass unencoded and this option is ignored.
  :out-enc option may be given followed by :bytes or a String. If a
           String is given, it names a character encoding
           (UTF-8/16/32, ASCII, Latin-1 + aliases). If :bytes is
           given, stdout is stored in a byte array and returned.
           Defaults to UTF-8.
  :env     override the process env with a map. (Port: maps only.)
  :dir     override the process dir with a String. (Port: strings
           only.)

  You can bind :env or :dir for multiple operations using with-sh-env
  and with-sh-dir.

  sh returns a map of
    :exit => sub-process's exit code
    :out  => sub-process's stdout (as byte[] or String)
    :err  => sub-process's stderr (String, lossy UTF-8)"
  {:added "1.2"}
  [& args]
  (let [[cmd opts] (parse-args args)]
    (impl/proc (vec cmd) (:in opts) (:in-enc opts) (:out-enc opts) (:env opts) (:dir opts))))
