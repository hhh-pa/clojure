;; Copyright (c) Rich Hickey All rights reserved. The use and
;; Copyright (c) Hao Hu. All rights reserved.
;; distribution terms for this software are covered by the Eclipse Public
;; License 1.0 (http://opensource.org/licenses/eclipse-1.0.php) which can be found
;; in the file epl-v10.html at the root of this distribution. By using this
;; software in any fashion, you are agreeing to be bound by the terms of
;; this license. You must not remove this notice, or any other, from this
;; software.

;; Originally contributed by Stephen C. Gilardi

;; Port of upstream src/clj/clojure/main.clj (W4 entry/IO): the namespace
;; fns only. This file MUST NOT rewire the binary CLI.
;;
;; The ns metadata lives in the positional docstring/attr slots: upstream
;; carries ^{:doc :author} on the ns symbol, which this loader does not read.
;; Upstream's (:import (java.io StringReader BufferedWriter FileWriter)
;; (java.nio.file Files) (java.nio.file.attribute FileAttribute)
;; (clojure.lang Compiler Compiler$CompilerException
;; LineNumberingPushbackReader RT LispReader$ReaderException)) is dropped:
;; the ported fns use load/load-file/read/read+string/with-in-str plus
;; String members instead. clojure.string is deliberately NOT required
;; (its top-level set! would fail a bare require of this namespace).

(ns clojure.main
  "Top-level main function for Clojure REPL and scripts."
  {:author "Stephen C. Gilardi and Rich Hickey"}
  (:refer-clojure :exclude [with-bindings])
  (:require [clojure.spec.alpha :as spec]))

(import 'java.lang.Object)

(declare main)

;;;;;;;;;;;;;;;;;;; redundantly copied from clojure.repl to avoid dep ;;;;;;;;;;;;;;

;; Divergence: upstream demunge calls clojure.lang.Compiler/demunge (JVM).
;; The port replays its table — CHAR_MAP reversed, plus $ -> / — replacing
;; longest keys first (upstream sorts DEMUNGE_PATTERN the same way, since
;; "_" is a prefix of every _WORD_ escape). String.replace has no
;; counterpart, so replacement loops on the 1-arg .indexOf member.
(def ^:private demunge-map
  {"$" "/"
   "_" "-"
   "_COLON_" ":"
   "_PLUS_" "+"
   "_GT_" ">"
   "_LT_" "<"
   "_EQ_" "="
   "_TILDE_" "~"
   "_BANG_" "!"
   "_CIRCA_" "@"
   "_SHARP_" "#"
   "_SINGLEQUOTE_" "'"
   "_DOUBLEQUOTE_" "\""
   "_PERCENT_" "%"
   "_CARET_" "^"
   "_AMPERSAND_" "&"
   "_STAR_" "*"
   "_BAR_" "|"
   "_LBRACE_" "{"
   "_RBRACE_" "}"
   "_LBRACK_" "["
   "_RBRACK_" "]"
   "_SLASH_" "/"
   "_BSLASH_" "\\"
   "_QMARK_" "?"})

(defn- replace-literal
  [s match replacement]
  (loop [s s]
    (let [i (.indexOf s match)]
      (if (neg? i)
        s
        (recur (str (subs s 0 i) replacement (subs s (+ i (count match)))))))))

(defn demunge
  "Given a string representation of a fn class,
  as in a stack trace element, returns a readable version."
  {:added "1.3"}
  [fn-name]
  (reduce (fn [acc entry]
            (replace-literal acc (key entry) (val entry)))
          (str fn-name)
          (sort-by #(- (count (key %))) demunge-map)))

(defn root-cause
  "Returns the initial cause of an exception or error by peeling off all of
  its wrappers"
  {:added "1.3"}
  [^Throwable t]
  ;; Divergence: upstream stops peeling at CompilerExceptions with a real
  ;; source (the .source field); there is no such field here, so the port
  ;; peels to the last cause.
  (loop [cause t]
    (if-let [next (ex-cause cause)]
      (recur next)
      cause)))

;;;;;;;;;;;;;;;;;;; end of redundantly copied from clojure.repl to avoid dep ;;;;;;;;;;;;;;

(def ^:private core-namespaces
  #{"clojure.core" "clojure.core.reducers" "clojure.core.protocols" "clojure.data" "clojure.datafy"
    "clojure.edn" "clojure.instant" "clojure.java.io" "clojure.main" "clojure.pprint" "clojure.reflect"
    "clojure.repl" "clojure.set" "clojure.spec.alpha" "clojure.spec.gen.alpha" "clojure.spec.test.alpha"
    "clojure.string" "clojure.template" "clojure.uuid" "clojure.walk" "clojure.xml" "clojure.zip"})

(defn- core-class?
  [^String class-name]
  (and (not (nil? class-name))
       (or (.startsWith class-name "clojure.lang.")
           (contains? core-namespaces (second (re-find #"^([^$]+)\$" class-name))))))

(defn stack-element-str
  "Returns a (possibly unmunged) string representation of a StackTraceElement"
  {:added "1.3"}
  [el]
  ;; Divergence: upstream takes a StackTraceElement; here el is a
  ;; StackTraceElement->vec [class method file line] vector.
  (let [[cls method file line] el
        clojure-fn? (and file (or (.endsWith file ".clj")
                                  (.endsWith file ".cljc")
                                  (= file "NO_SOURCE_FILE")))]
    (str (if clojure-fn?
           (demunge (str cls))
           (str (str cls) "." (str method)))
         " (" (str file) ":" line ")")))
;;;;;;;;;;;;;;;;;;; end of redundantly copied from clojure.repl to avoid dep ;;;;;;;;;;;;;;

(defmacro with-bindings
  "Executes body in the context of thread-local bindings for several vars
  that often need to be set!: *ns* *warn-on-reflection* *math-context*
  *print-meta* *print-length* *print-level* *compile-path*
  *command-line-args* *1 *2 *3 *e"
  [& body]
  `(binding [*ns* *ns*
             *warn-on-reflection* *warn-on-reflection*
             *math-context* *math-context*
             *print-meta* *print-meta*
             *print-length* *print-length*
             *print-level* *print-level*
             *print-namespace-maps* true
             *data-readers* *data-readers*
             *default-data-reader-fn* *default-data-reader-fn*
             *compile-path* (System/getProperty "clojure.compile.path" "classes")
             *command-line-args* *command-line-args*
             *unchecked-math* *unchecked-math*
             *assert* *assert*
             clojure.spec.alpha/*explain-out* clojure.spec.alpha/*explain-out*
             *1 nil
             *2 nil
             *3 nil
             *e nil]
     ~@body))

(defn repl-prompt
  "Default :prompt hook for repl"
  []
  (printf "%s=> " (ns-name *ns*)))

(defn skip-if-eol
  "If the next character on stream s is a newline, skips it, otherwise
  leaves the stream untouched. Returns :line-start, :stream-end, or :body
  to indicate the relative location of the next character on s. The stream
  must either be an instance of LineNumberingPushbackReader or duplicate
  its behavior of both supporting .unread and collapsing all of CR, LF, and
  CRLF to a single \\newline."
  ;; Divergence: native readers expose no single-char .read/.unread, so
  ;; this hook cannot be implemented; it throws.
  [s]
  (throw (ex-info "skip-if-eol is not supported on this runtime (readers expose no single-char read/unread)." {})))

(defn skip-whitespace
  "Skips whitespace characters on stream s. Returns :line-start, :stream-end,
  or :body to indicate the relative location of the next character on s.
  Interprets comma as whitespace and semicolon as comment to end of line.
  Does not interpret #! as comment to end of line because only one
  character of lookahead is available. The stream must either be an
  instance of LineNumberingPushbackReader or duplicate its behavior of both
  supporting .unread and collapsing all of CR, LF, and CRLF to a single
  \\newline."
  ;; Divergence: native readers expose no single-char .read/.unread (nor
  ;; .readLine), so this hook cannot be implemented; it throws.
  [s]
  (throw (ex-info "skip-whitespace is not supported on this runtime (readers expose no single-char read/unread)." {})))

(defn renumbering-read
  "Reads from reader, which must be a LineNumberingPushbackReader, while capturing
  the read string. If the read is successful, reset the line number and re-read.
  The line number on re-read is the passed line-number unless :line or
  :clojure.core/eval-file meta are explicitly set on the read value."
  {:added "1.10"}
  ([opts ^LineNumberingPushbackReader reader line-number]
   ;; Divergence: there is no LineNumberReader/setLineNumber here, so the
   ;; line-number reset is skipped; the port captures [form text] via
   ;; read+string and re-reads the text (so a bad read still throws on
   ;; the second read, as upstream).
   (let [[_ s] (read+string opts reader)]
     (with-in-str s
       (read opts *in*)))))

(defn repl-read
  "Default :read hook for repl. Reads from *in* which must either be an
  instance of LineNumberingPushbackReader or duplicate its behavior of both
  supporting .unread and collapsing all of CR, LF, and CRLF into a single
  \\newline. repl-read:
    - skips whitespace, then
      - returns request-prompt on start of line, or
      - returns request-exit on end of stream, or
      - reads an object from the input stream, then
        - skips the next input character if it's end of line, then
        - returns the object."
  [request-prompt request-exit]
  ;; Divergence: skip-whitespace/skip-if-eol have no counterpart (no
  ;; peek/unread); core/read already skips whitespace/comments, so the
  ;; port reads one form and maps EOF to request-exit. request-prompt is
  ;; never returned: blank lines block for more input instead.
  (read {:read-cond :allow :eof request-exit} *in*))

(defn repl-exception
  "Returns the root cause of throwables"
  [throwable]
  (root-cause throwable))

(defn- file-name
  "Helper to get just the file name part of a path or nil"
  ;; Divergence: java.io.File has no counterpart; the port takes the last
  ;; /- or \\-separated segment instead.
  [^String full-path]
  (when full-path
    (try
      (last (.split full-path "[\\\\/]"))
      (catch Throwable t))))

(defn- file-path
  "Helper to get the relative path to the source file or nil"
  ;; Divergence: java.io.File and the user.dir property have no
  ;; counterpart, so no absolutization/relativization can happen; the
  ;; degenerate remainder is the identity.
  [^String full-path]
  full-path)

(defn- java-loc->source
  "Convert Java class name and method symbol to source symbol, either a
  Clojure function or Java class and method."
  ;; Divergence: String.split/replaceAll/join have no String-member
  ;; counterparts; the port splits on $ via the 1-arg .split member,
  ;; strips --suffixes via the 1-arg Matcher .replaceAll member, and
  ;; joins via interpose.
  [clazz method]
  (if (#{'invoke 'invokeStatic} method)
    (let [degen #(.replaceAll (re-matcher #"--.*$" %) "")
          [ns-name fn-name & nested] (->> (str clazz) (#(.split % "\\$")) (map demunge) (map degen))]
      (symbol ns-name (apply str (interpose "$" (cons fn-name nested)))))
    (symbol (name clazz) (name method))))

(defn ex-triage
  "Returns an analysis of the phase, error, cause, and location of an error that occurred
  based on Throwable data, as returned by Throwable->map. All attributes other than phase
  are optional:
    :clojure.error/phase - keyword phase indicator, one of:
      :read-source :compile-syntax-check :compilation :macro-syntax-check :macroexpansion
      :execution :read-eval-result :print-eval-result
    :clojure.error/source - file name (no path)
    :clojure.error/path - source path
    :clojure.error/line - integer line number
    :clojure.error/column - integer column number
    :clojure.error/symbol - symbol being expanded/compiled/invoked
    :clojure.error/class - cause exception class symbol
    :clojure.error/cause - cause exception message
    :clojure.error/spec - explain-data for spec error"
  {:added "1.10"}
  [datafied-throwable]
  ;; Divergence: the native Throwable->map carries no top-level :phase
  ;; (upstream threads (-> o ex-data :clojure.error/phase) through), so
  ;; the port recovers it from the outermost link's ex-data. Note there
  ;; is deliberately no :or default on phase: a default here would win
  ;; over the recovery.
  (let [{:keys [via trace phase]} datafied-throwable
        phase (or phase (-> via first :data :clojure.error/phase) :execution)
        {:keys [type message data]} (last via)
        {:clojure.spec.alpha/keys [problems fn], :clojure.spec.test.alpha/keys [caller]} data
        {:clojure.error/keys [source] :as top-data} (:data (first via))]
    (assoc
      (case phase
        :read-source
        (let [{:clojure.error/keys [line column]} data]
          (cond-> (merge (-> via second :data) top-data)
            source (assoc :clojure.error/source (file-name source)
                          :clojure.error/path (file-path source))
            (#{"NO_SOURCE_FILE" "NO_SOURCE_PATH"} source) (dissoc :clojure.error/source :clojure.error/path)
            message (assoc :clojure.error/cause message)))

        (:compile-syntax-check :compilation :macro-syntax-check :macroexpansion)
        (cond-> top-data
          source (assoc :clojure.error/source (file-name source)
                        :clojure.error/path (file-path source))
          (#{"NO_SOURCE_FILE" "NO_SOURCE_PATH"} source) (dissoc :clojure.error/source :clojure.error/path)
          type (assoc :clojure.error/class type)
          message (assoc :clojure.error/cause message)
          problems (assoc :clojure.error/spec data))

        (:read-eval-result :print-eval-result)
        (let [[source method file line] (-> trace first)]
          (cond-> top-data
            line (assoc :clojure.error/line line)
            file (assoc :clojure.error/source file)
            (and source method) (assoc :clojure.error/symbol (java-loc->source source method))
            type (assoc :clojure.error/class type)
            message (assoc :clojure.error/cause message)))

        :execution
        (let [[source method file line] (->> trace (drop-while #(core-class? (name (first %)))) first)
              file (first (remove #(or (nil? %) (#{"NO_SOURCE_FILE" "NO_SOURCE_PATH"} %)) [(:file caller) file]))
              err-line (or (:line caller) line)]
          (cond-> {:clojure.error/class type}
            err-line (assoc :clojure.error/line err-line)
            message (assoc :clojure.error/cause message)
            (or fn (and source method)) (assoc :clojure.error/symbol (or fn (java-loc->source source method)))
            file (assoc :clojure.error/source file)
            problems (assoc :clojure.error/spec data))))
      :clojure.error/phase phase)))

(defn ex-str
  "Returns a string from exception data, as produced by ex-triage.
  The first line summarizes the exception phase and location.
  The subsequent lines describe the cause."
  {:added "1.10"}
  [{:clojure.error/keys [phase source path line column symbol class cause spec]
    :as triage-data}]
  ;; Divergence: upstream extracts the simple class name with the
  ;; possessive regex #"([^.])++$"; the regex engine here rejects
  ;; possessive quantifiers, so the port uses #"([^.]*)$", which agrees
  ;; on every input (both match the trailing non-dot run, or ""/nil
  ;; for the empty name, which the or-default then replaces).
  (let [loc (str (or path source "REPL") ":" (or line 1) (if column (str ":" column) ""))
        class-name (name (or class ""))
        simple-class (if class (or (first (re-find #"([^.]*)$" class-name)) class-name))
        cause-type (if (contains? #{"Exception" "RuntimeException"} simple-class)
                     "" ;; omit, not useful
                     (str " (" simple-class ")"))]
    (case phase
      :read-source
      (format "Syntax error reading source at (%s).%n%s%n" loc cause)

      :macro-syntax-check
      (format "Syntax error macroexpanding %sat (%s).%n%s"
              (if symbol (str symbol " ") "")
              loc
              (if spec
                (with-out-str
                  (spec/explain-out
                    (if (= spec/*explain-out* spec/explain-printer)
                      (update spec :clojure.spec.alpha/problems
                              (fn [probs] (map #(dissoc % :in) probs)))
                      spec)))
                (format "%s%n" cause)))

      :macroexpansion
      (format "Unexpected error%s macroexpanding %sat (%s).%n%s%n"
              cause-type
              (if symbol (str symbol " ") "")
              loc
              cause)

      :compile-syntax-check
      (format "Syntax error%s compiling %sat (%s).%n%s%n"
              cause-type
              (if symbol (str symbol " ") "")
              loc
              cause)

      :compilation
      (format "Unexpected error%s compiling %sat (%s).%n%s%n"
              cause-type
              (if symbol (str symbol " ") "")
              loc
              cause)

      :read-eval-result
      (format "Error reading eval result%s at %s (%s).%n%s%n" cause-type symbol loc cause)

      :print-eval-result
      (format "Error printing return value%s at %s (%s).%n%s%n" cause-type symbol loc cause)

      :execution
      (if spec
        (format "Execution error - invalid arguments to %s at (%s).%n%s"
                symbol
                loc
                (with-out-str
                  (spec/explain-out
                    (if (= spec/*explain-out* spec/explain-printer)
                      (update spec :clojure.spec.alpha/problems
                              (fn [probs] (map #(dissoc % :in) probs)))
                      spec))))
        (format "Execution error%s at %s(%s).%n%s%n"
                cause-type
                (if symbol (str symbol " ") "")
                loc
                cause)))))

(defn err->msg
  "Helper to return an error message string from an exception."
  [^Throwable e]
  (-> e Throwable->map ex-triage ex-str))

(defn repl-caught
  "Default :caught hook for repl"
  [e]
  (binding [*out* *err*]
    (print (err->msg e))
    (flush)))

(def ^{:doc "A sequence of lib specs that are applied to `require`
by default when a new command-line REPL is started."} repl-requires
  '[[clojure.repl :refer (source apropos dir pst doc find-doc)]
    [clojure.java.javadoc :refer (javadoc)]
    [clojure.pprint :refer (pp pprint)]
    [clojure.repl.deps :refer (add-libs add-lib sync-deps)]])

(defmacro with-read-known
  "Evaluates body with *read-eval* set to a \"known\" value,
   i.e. substituting true for :unknown if necessary."
  [& body]
  `(binding [*read-eval* (if (= :unknown *read-eval*) true *read-eval*)]
     ~@body))

(defn repl
  "Generic, reusable, read-eval-print loop. By default, reads from *in*,
  writes to *out*, and prints exception summaries to *err*. If you use the
  default :read hook, *in* must either be an instance of
  LineNumberingPushbackReader or duplicate its behavior of both supporting
  .unread and collapsing CR, LF, and CRLF into a single \\newline. Options
  are sequential keyword-value pairs. Available options and their defaults:

     - :init, function of no arguments, initialization hook called with
       bindings for set!-able vars in place.
       default: #()

     - :need-prompt, function of no arguments, called before each
       read-eval-print except the first, the user will be prompted if it
       returns true.
       default: (if (instance? LineNumberingPushbackReader *in*)
                  #(.atLineStart *in*)
                  #(identity true))

     - :prompt, function of no arguments, prompts for more input.
       default: repl-prompt

     - :flush, function of no arguments, flushes output
       default: flush

     - :read, function of two arguments, reads from *in*:
         - returns its first argument to request a fresh prompt
           - depending on need-prompt, this may cause the repl to prompt
             before reading again
         - returns its second argument to request an exit from the repl
         - else returns the next object read from the input stream
       default: repl-read

     - :eval, function of one argument, returns the evaluation of its
       argument
       default: eval

     - :print, function of one argument, prints its argument to the output
       default: prn

     - :caught, function of one argument, a throwable, called when
       read, eval, or print throws an exception or error
       default: repl-caught"
  ;; Divergence: upstream swaps the thread context classloader for a
  ;; DynamicClassLoader here; classloaders do not exist on this runtime,
  ;; so that step is skipped.
  [& options]
  (let [{:keys [init need-prompt prompt flush read eval print caught]
         ;; Divergence: .atLineStart has no counterpart, so :need-prompt
         ;; defaults to always-true instead of testing *in*.
         :or {init        #()
              need-prompt #(identity true)
              prompt      repl-prompt
              flush       flush
              read        repl-read
              eval        eval
              print       prn
              caught      repl-caught}}
        (apply hash-map options)
        request-prompt (Object.)
        request-exit (Object.)
        read-eval-print
        (fn []
          (try
            (let [read-eval *read-eval*
                  input (try
                          (with-read-known (read request-prompt request-exit))
                          ;; Divergence: the reader exception is named with
                          ;; its qualified class (the :import clause was
                          ;; dropped); the class itself resolves and the
                          ;; catch matches what core/read throws.
                          (catch clojure.lang.LispReader$ReaderException e
                            (throw (ex-info nil {:clojure.error/phase :read-source} e))))]
             (or (#{request-prompt request-exit} input)
                 (let [value (binding [*read-eval* read-eval] (eval input))]
                   (set! *3 *2)
                   (set! *2 *1)
                   (set! *1 value)
                   (try
                     (print value)
                     (catch Throwable e
                       (throw (ex-info nil {:clojure.error/phase :print-eval-result} e)))))))
           (catch Throwable e
             (caught e)
             (set! *e e))))]
    (with-bindings
     (binding [*repl* true]
       (try
        (init)
        (catch Throwable e
          (caught e)
          (set! *e e)))
       (prompt)
       (flush)
       (loop []
         (when-not
           (try (identical? (read-eval-print) request-exit)
            (catch Throwable e
             (caught e)
             (set! *e e)
             nil))
           (when (need-prompt)
             (prompt)
             (flush))
           (recur)))))))

(defn load-script
  "Loads Clojure source from a file or resource given its path. Paths
  beginning with @ or @/ are considered relative to classpath."
  [^String path]
  ;; Divergence: RT/loadResourceScript becomes load (which takes
  ;; extensionless roots and appends .clj/.cljc itself, so a trailing
  ;; .clj/.cljc is stripped first); Compiler/loadFile becomes load-file.
  (if (.startsWith path "@")
    (load (let [root (.substring path (if (.startsWith path "@/") 2 1))]
            (cond (.endsWith root ".cljc") (subs root 0 (- (count root) 5))
                  (.endsWith root ".clj") (subs root 0 (- (count root) 4))
                  :else root)))
    (load-file path)))

(defn- init-opt
  "Load a script"
  [path]
  (load-script path))

(defn- eval-opt
  "Evals expressions in str, prints each non-nil result using prn"
  [str]
  ;; Divergence: the StringReader/LineNumberingPushbackReader pair becomes
  ;; with-in-str over the string; the eof sentinel stays a fresh Object.
  (let [eof (Object.)]
    (with-in-str str
      (loop [input (with-read-known (read *in* false eof))]
        (when-not (= input eof)
          (let [value (eval input)]
            (when-not (nil? value)
              (prn value))
            (recur (with-read-known (read *in* false eof)))))))))

(defn- init-dispatch
  "Returns the handler associated with an init opt"
  [opt]
  ({"-i"     init-opt
    "--init" init-opt
    "-e"     eval-opt
    "--eval" eval-opt} opt))

(defn- initialize
  "Common initialize routine for repl, script, and null opts"
  [args inits]
  (in-ns 'user)
  (set! *command-line-args* args)
  (doseq [[opt arg] inits]
    ((init-dispatch opt) arg)))

(defn- main-opt
  "Call the -main function from a namespace with string arguments from
  the command line."
  [[_ main-ns & args] inits]
  (with-bindings
    (initialize args inits)
    (apply (ns-resolve (doto (symbol main-ns) require) '-main) args)))

(defn- repl-opt
  "Start a repl with args and inits. Print greeting if no eval options were
  present"
  ;; Divergence: the trailing (System/exit 0) is dropped — System/exit has
  ;; no counterpart. repl-opt returns nil after the loop ends.
  [[_ & args] inits]
  (when-not (some #(= eval-opt (init-dispatch (first %))) inits)
    (println "Clojure" (clojure-version)))
  (repl :init (fn []
                (initialize args inits)
                (apply require repl-requires)))
  (prn))

(defn- script-opt
  "Run a script from a file, resource, or standard in with args and inits"
  [[path & args] inits]
  (with-bindings
    (initialize args inits)
    (if (= path "-")
      (load-reader *in*)
      (load-script path))))

(defn- null-opt
  "No repl or script opt present, just bind args and run inits"
  [args inits]
  (with-bindings
    (initialize args inits)))

(defn- help-opt
  "Print help text for main"
  [_ _]
  (println (:doc (meta (var main)))))

(defn- main-dispatch
  "Returns the handler associated with a main option"
  [opt]
  (or
   ({"-r"     repl-opt
     "--repl" repl-opt
     "-m"     main-opt
     "--main" main-opt
     nil      null-opt
     "-h"     help-opt
     "--help" help-opt
     "-?"     help-opt} opt)
   script-opt))

(defn- legacy-repl
  "Called by the clojure.lang.Repl.main stub to run a repl with args
  specified the old way"
  [args]
  (println "WARNING: clojure.lang.Repl is deprecated.
Instead, use clojure.main like this:
java -cp clojure.jar clojure.main -i init.clj -r args...")
  (let [[inits [sep & args]] (split-with (complement #{"--"}) args)]
    (repl-opt (concat ["-r"] args) (map vector (repeat "-i") inits))))

(defn- legacy-script
  "Called by the clojure.lang.Script.main stub to run a script with args
  specified the old way"
  [args]
  (println "WARNING: clojure.lang.Script is deprecated.
Instead, use clojure.main like this:
java -cp clojure.jar clojure.main -i init.clj script.clj args...")
  (let [[inits [sep & args]] (split-with (complement #{"--"}) args)]
    (null-opt args (map vector (repeat "-i") inits))))

(defn report-error
  "Create and output an exception report for a Throwable to target.

  Options:
    :target - \"file\" (default), \"stderr\", \"none\"

  If file is specified but cannot be written, falls back to stderr."
  [^Throwable t & {:keys [target]
                   :or {target "file"} :as opts}]
  (when-not (= target "none")
    (let [trace (Throwable->map t)
          triage (ex-triage trace)
          message (ex-str triage)
          report (array-map
                   :clojure.main/message message
                   :clojure.main/triage triage
                   :clojure.main/trace trace)
          report-str (with-out-str
                       (binding [*print-namespace-maps* false]
                         ;; Divergence: clojure.pprint may be absent (it
                         ;; ships in another slice); fall back to pr when
                         ;; requiring-resolve fails.
                         (try
                           ((requiring-resolve 'clojure.pprint/pprint) report)
                           (catch Throwable _ (pr report)))))
          ;; Divergence: no temp-dir property or Files/FileWriter API
          ;; exists here; the port attempts a session-unique file in the
          ;; working directory instead, still falling back to stderr.
          err-path (when (= target "file")
                     (try
                       (let [f (str (gensym "clojure-") ".edn")]
                         (spit f report-str)
                         f)
                       (catch Throwable _)))] ;; ignore, fallback to stderr
      (binding [*out* *err*]
        (if err-path
          (println (str message (System/lineSeparator) "Full report at:" (System/lineSeparator) err-path))
          (println (str report-str (System/lineSeparator) message)))))))

(defn main
  "Usage: java -cp clojure.jar clojure.main [init-opt*] [main-opt] [arg*]

  With no options or args, runs an interactive Read-Eval-Print Loop

  init options:
    -i, --init path     Load a file or resource
    -e, --eval string   Evaluate expressions in string; print non-nil values
    --report target     Report uncaught exception to \"file\" (default), \"stderr\",
                        or \"none\", overrides System property clojure.main.report

  main options:
    -m, --main ns-name  Call the -main function from a namespace with args
    -r, --repl          Run a repl
    path                Run a script from a file or resource
    -                   Run a script from standard input
    -h, -?, --help      Print this help message and exit

  operation:

    - Establishes thread-local bindings for commonly set!-able vars
    - Enters the user namespace
    - Binds *command-line-args* to a seq of strings containing command line
      args that appear after any main option
    - Runs all init options in order
    - Calls a -main function or runs a repl or script if requested

  The init options may be repeated and mixed freely, but must appear before
  any main option. The appearance of any eval option before running a repl
  suppresses the usual repl greeting message: \"Clojure ~(clojure-version)\".

  Paths may be absolute or relative in the filesystem or relative to
  classpath. Classpath-relative paths have prefix of @ or @/"
  ;; Divergence: both (System/exit 1) calls are dropped — System/exit has
  ;; no counterpart. main reports the error and returns nil instead.
  [& args]
  (try
   (if args
     (loop [[opt arg & more :as args] args, inits [], flags nil]
       (cond
         ;; flag
         (contains? #{"--report"} opt)
         (recur more inits (merge flags {(subs opt 2) arg}))

         ;; init opt
         (init-dispatch opt)
         (recur more (conj inits [opt arg]) flags)

         :main-opt
         (try
           ((main-dispatch opt) args inits)
           (catch Throwable t
             (report-error t :target (get flags "report" (System/getProperty "clojure.main.report" "file")))
             nil))))
     (try
       (repl-opt nil nil)
       (catch Throwable t
         (report-error t :target "file")
         nil)))
   (finally
     (flush))))
