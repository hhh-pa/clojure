;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/edn.clj (W4 entry/IO).
;;
;; The ns metadata lives in the positional docstring/attr slots: upstream
;; carries ^{:doc :author} on the ns symbol, which this loader does not
;; read (see clojure.repl for the same note).

(ns clojure.edn
  "edn reading."
  {:author "Rich Hickey"}
  (:refer-clojure :exclude [read read-string]))

;; Upstream delegates both fns to clojure.lang.EdnReader (JVM). The port
;; drives clojure.core/read + clojure.core/read-string instead, with
;; *read-eval* bound false (so #= eval is rejected, as in edn) and the
;; opts :readers/:default layers bound onto *data-readers* and
;; *default-data-reader-fn* (core/read ignores those opts keys and only
;; consults the vars). :eof passes through; when absent, EOF throws.

(defn read
  "Reads the next object from stream, which must be an instance of
  java.io.PushbackReader or some derivee.  stream defaults to the
  current value of *in*.

  Reads data in the edn format (subset of Clojure data):
  http://edn-format.org

  opts is a map that can include the following keys:
  :eof - value to return on end-of-file. When not supplied, eof throws an exception.
  :readers  - a map of tag symbols to data-reader functions to be considered before default-data-readers.
              When not supplied, only the default-data-readers will be used.
  :default - A function of two args, that will, if present and no reader is found for a tag,
             be called with the tag and the value."

  {:added "1.5"}
  ([]
   (read *in*))
  ([stream]
   (read {} stream))
  ([opts stream]
   (binding [*read-eval* false
             *data-readers* (merge *data-readers* (:readers opts))
             *default-data-reader-fn* (or (:default opts) *default-data-reader-fn*)]
     (clojure.core/read (if (contains? opts :eof) {:eof (:eof opts)} {}) stream))))

(defn read-string
  "Reads one object from the string s. Returns nil when s is nil or empty.

  Reads data in the edn format (subset of Clojure data):
  http://edn-format.org

  opts is a map as per clojure.edn/read"
  {:added "1.5"}
  ([s] (read-string {:eof nil} s))
  ([opts s]
   (when s
     (binding [*read-eval* false
               *data-readers* (merge *data-readers* (:readers opts))
               *default-data-reader-fn* (or (:default opts) *default-data-reader-fn*)]
       (clojure.core/read-string (if (contains? opts :eof) {:eof (:eof opts)} {}) s)))))
