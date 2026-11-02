;   Copyright (c) Chris Houser, Dec 2008. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Common Public License 1.0 (http://opensource.org/licenses/cpl.php)
;   which can be found in the file CPL.TXT at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

; Utilities meant to be used interactively at the REPL

;; Port of upstream src/clj/clojure/repl.clj (W4 entry/IO).
;;
;; The ns metadata lives in the positional docstring/attr slots: upstream
;; carries ^{:author :doc} on the ns symbol, which this loader does not read.
;; Upstream's (:import (java.io LineNumberReader InputStreamReader
;; PushbackReader) (clojure.lang RT Reflector)) is dropped: the ported fns
;; below use no JVM reader/resource interop. clojure.string is deliberately
;; NOT required (its top-level set! would fail a bare require of this
;; namespace); the port uses String members (.indexOf/.split/.endsWith)
;; plus core fns instead.

(ns
  clojure.repl
  "Utilities meant to be used interactively at the REPL"
  {:author "Chris Houser, Christophe Grand, Stephen Gilardi, Michel Salim"}
  (:require [clojure.spec.alpha :as spec]))

(import 'java.lang.Object)

(def ^:private special-doc-map
  '{. {:url "java_interop#dot"
       :forms [(.instanceMember instance args*)
               (.instanceMember Classname args*)
               (Classname/staticMethod args*)
               Classname/staticField]
       :doc "The instance member form works for both fields and methods.
  They all expand into calls to the dot operator at macroexpansion time."}
    def {:forms [(def symbol doc-string? init?)]
         :doc "Creates and interns a global var with the name
  of symbol in the current namespace (*ns*) or locates such a var if
  it already exists.  If init is supplied, it is evaluated, and the
  root binding of the var is set to the resulting value.  If init is
  not supplied, the root binding of the var is unaffected."}
    do {:forms [(do exprs*)]
        :doc "Evaluates the expressions in order and returns the value of
  the last. If no expressions are supplied, returns nil."}
    if {:forms [(if test then else?)]
        :doc "Evaluates test. If not the singular values nil or false,
  evaluates and yields then, otherwise, evaluates and yields else. If
  else is not supplied it defaults to nil."}
    monitor-enter {:forms [(monitor-enter x)]
                   :doc "Synchronization primitive that should be avoided
  in user code. Use the 'locking' macro."}
    monitor-exit {:forms [(monitor-exit x)]
                  :doc "Synchronization primitive that should be avoided
  in user code. Use the 'locking' macro."}
    new {:forms [(Classname. args*) (new Classname args*)]
         :url "java_interop#new"
         :doc "The args, if any, are evaluated from left to right, and
  passed to the constructor of the class named by Classname. The
  constructed object is returned."}
    quote {:forms [(quote form)]
           :doc "Yields the unevaluated form."}
    recur {:forms [(recur exprs*)]
           :doc "Evaluates the exprs in order, then, in parallel, rebinds
  the bindings of the recursion point to the values of the exprs.
  Execution then jumps back to the recursion point, a loop or fn method."}
    set! {:forms[(set! var-symbol expr)
                 (set! (. instance-expr instanceFieldName-symbol) expr)
                 (set! (. Classname-symbol staticFieldName-symbol) expr)]
          :url "vars#set"
          :doc "Used to set thread-local-bound vars, Java object instance
fields, and Java class static fields."}
    throw {:forms [(throw expr)]
           :doc "The expr is evaluated and thrown, therefore it should
  yield an instance of some derivee of Throwable."}
    try {:forms [(try expr* catch-clause* finally-clause?)]
         :doc "catch-clause => (catch classname name expr*)
  finally-clause => (finally expr*)

  Catches and handles Java exceptions."}
    var {:forms [(var symbol)]
         :doc "The symbol must resolve to a var, and the Var object
itself (not its value) is returned. The reader macro #'x expands to (var x)."}})

(defn- special-doc [name-symbol]
  (assoc (or (special-doc-map name-symbol) (meta (resolve name-symbol)))
         :name name-symbol
         :special-form true))

(defn- namespace-doc [nspace]
  (assoc (meta nspace) :name (ns-name nspace)))

(defn- print-doc [{n :ns
                   nm :name
                   :keys [forms arglists special-form doc url macro spec]
                   :as m}]
  (println "-------------------------")
  (println (or spec (str (when n (str (ns-name n) "/")) nm)))
  (when forms
    (doseq [f forms]
      (print "  ")
      (prn f)))
  (when arglists
    (prn arglists))
  (cond
    special-form
    (println "Special Form")
    macro
    (println "Macro")
    spec
    (println "Spec"))
  (when doc (println " " doc))
  (when special-form
    (if (contains? m :url)
      (when url
        (println (str "\n  Please see http://clojure.org/" url)))
      (println (str "\n  Please see http://clojure.org/special_forms#" nm))))
  (when n
    (when-let [fnspec (spec/get-spec (symbol (str (ns-name n)) (name nm)))]
      (println "Spec")
      (doseq [role [:args :ret :fn]]
        (when-let [spec (get fnspec role)]
          (println " " (str (name role) ":") (spec/describe spec)))))))

(defn find-doc
  "Prints documentation for any var whose documentation or name
contains a match for re-string-or-pattern"
  {:added "1.0"}
  [re-string-or-pattern]
    (let [re (re-pattern re-string-or-pattern)
          ms (concat (mapcat #(sort-by :name (map meta (vals (ns-interns %))))
                             (all-ns))
                     (map namespace-doc (all-ns))
                     (map special-doc (keys special-doc-map)))]
      (doseq [m ms
              :when (and (:doc m)
                         (or (re-find (re-matcher re (:doc m)))
                             (re-find (re-matcher re (str (:name m))))))]
               (print-doc m))))

(defmacro doc
  "Prints documentation for a var or special form given its name,
   or for a spec if given a keyword"
  {:added "1.0"}
  ;; Divergence: upstream calls the private helpers as #'print-doc /
  ;; #'special-doc / #'namespace-doc inside syntax-quote. That breaks
  ;; twice here: syntax-quote resolves helper symbols in the CALLER's
  ;; namespace (user/print-doc), and the analyzer rejects
  ;; cross-namespace private calls outright. The port names this
  ;; namespace explicitly and reaches the privates via @(var ...),
  ;; the var-quote escape hatch.
  [name]
  (if-let [special-name ('{& fn catch try finally try} name)]
    `((deref (var ~(symbol "clojure.repl" "print-doc")))
      ((deref (var ~(symbol "clojure.repl" "special-doc"))) '~special-name))
    (cond
      (special-doc-map name) `((deref (var ~(symbol "clojure.repl" "print-doc")))
                               ((deref (var ~(symbol "clojure.repl" "special-doc"))) '~name))
      ;; Divergence: this spec port's describe throws on unresolvable
      ;; keywords (upstream answers ::unknown), so the keyword branch
      ;; describes only registered specs and splices the unknown marker
      ;; itself otherwise.
      (keyword? name) `((deref (var ~(symbol "clojure.repl" "print-doc")))
                        {:spec '~name
                         :doc '~(if (spec/get-spec name)
                                  (spec/describe name)
                                  :clojure.spec.alpha/unknown)})
      (find-ns name) `((deref (var ~(symbol "clojure.repl" "print-doc")))
                       ((deref (var ~(symbol "clojure.repl" "namespace-doc"))) (find-ns '~name)))
      (resolve name) `((deref (var ~(symbol "clojure.repl" "print-doc"))) (meta (var ~name))))))

;; ----------------------------------------------------------------------
;; Examine Clojure functions (Vars, really)

(defn- defines-form?
  "True when form is a top-level (def/defn/defn-/defmacro/defonce ...)
  defining the var named target."
  [form target]
  (and (seq? form)
       (let [head (first form)]
         (and (symbol? head)
              (contains? '#{def defn defn- defmacro defonce} head)
              (let [nm (second form)]
                (and (symbol? nm) (= (symbol (name nm)) target)))))))

(defn- capture-form-text
  "Drop leading blank/comment lines from a read+string capture, keeping
  the form text from its first line on. (Leading #_ discards, if any,
  are kept: only full-line gaps can be stripped without parsing.)"
  [s]
  (let [lines (.split s "\r\n|[\n\r]")]
    (loop [ls (seq lines)]
      (if (nil? ls)
        ""
        (let [trimmed (.trim (first ls))]
          (if (or (empty? trimmed) (.startsWith trimmed ";"))
            (recur (rest ls))
            (apply str (interpose "\n" ls))))))))

(defn source-fn
  "Returns a string of the source code for the given symbol, if it can
  find it.  This requires that the symbol resolve to a Var defined in
  a namespace for which the .clj is in the classpath.  Returns nil if
  it can't find the source.  For most REPL usage, 'source' is more
  convenient.

  Example: (source-fn 'filter)"
  [x]
  ;; Divergence: upstream streams the .clj off the classpath through a
  ;; LineNumberReader (RT/baseLoader + proxying PushbackReader), seeks
  ;; to (:line (meta v)), and captures one read, throwing
  ;; IllegalStateException (a JVM ctor this runtime lacks) when
  ;; *read-eval* is :unknown. The port slurps (:file (meta v)) — nil
  ;; unless it names a readable file, so vars from embedded sources
  ;; (whose :file is a resource path, not a file) report "not found" —
  ;; then loops read+string over the whole text and returns the capture
  ;; of the first form defining (:name (meta v)). The by-name search
  ;; replaces the line seek because the loader stamps :line 1 on every
  ;; top-level form, which would mis-seek every var past the first; the
  ;; capture itself is Java-trimmed (upstream keeps leading whitespace).
  (when-let [v (resolve x)]
    (when-let [filepath (:file (meta v))]
      (when-let [target (:name (meta v))]
        (when-let [text (try (slurp filepath) (catch Throwable _ nil))]
          (if (= :unknown *read-eval*)
            (throw (ex-info "Unable to read source while *read-eval* is :unknown." {}))
            (let [eof (Object.)
                  read-opts (assoc (if (.endsWith ^String filepath "cljc") {:read-cond :allow} {})
                                   :eof eof)]
              (with-in-str text
                (loop []
                  (let [[form s] (read+string read-opts *in*)]
                    (cond (= form eof) nil
                          (defines-form? form target) (capture-form-text s)
                          :else (recur))))))))))))

(defmacro source
  "Prints the source code for the given symbol, if it can find it.
  This requires that the symbol resolve to a Var defined in a
  namespace for which the .clj is in the classpath.

  Example: (source filter)"
  ;; Divergence: see doc — syntax-quote would qualify source-fn to the
  ;; caller's namespace, so this namespace is named explicitly.
  [n]
  `(println (or (~(symbol "clojure.repl" "source-fn") '~n) (str "Source not found"))))

(defn apropos
  "Given a regular expression or stringable thing, return a seq of all
public definitions in all currently-loaded namespaces that match the
str-or-pattern."
  [str-or-pattern]
  ;; Divergence: String.contains has no counterpart here; the
  ;; stringable arm compares via the 1-arg .indexOf member instead.
  ;; (str ns) is the #object[...] form here rather than the ns name, so
  ;; the qualifier comes from (str (ns-name ns)) instead.
  (let [matches? (if (instance? java.util.regex.Pattern str-or-pattern)
                   #(re-find str-or-pattern (str %))
                   #(>= (.indexOf (str %) (str str-or-pattern)) 0))]
    (sort (mapcat (fn [ns]
                    (let [ns-name (str (ns-name ns))]
                      (map #(symbol ns-name (str %))
                           (filter matches? (keys (ns-publics ns))))))
                  (all-ns)))))

(defn dir-fn
  "Returns a sorted seq of symbols naming public vars in
  a namespace or namespace alias. Looks for aliases in *ns*"
  [ns]
  (sort (map first (ns-publics (the-ns (get (ns-aliases *ns*) ns ns))))))

(defmacro dir
  "Prints a sorted directory of public vars in a namespace"
  ;; Divergence: see doc — syntax-quote would qualify dir-fn to the
  ;; caller's namespace, so this namespace is named explicitly.
  [nsname]
  `(doseq [v# (~(symbol "clojure.repl" "dir-fn") '~nsname)]
     (println v#)))

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

(defn pst
  "Prints a stack trace of the exception, to the depth requested. If none supplied, uses the root cause of the
  most recent repl exception (*e), and a depth of 12."
  {:added "1.3"}
  ([] (pst 12))
  ([e-or-depth]
     (if (instance? Throwable e-or-depth)
       (pst e-or-depth 12)
       (when-let [e *e]
         (pst (root-cause e) e-or-depth))))
  ([^Throwable e depth]
     ;; Divergence: a native throwable carries one [class method file
     ;; line] frame (its link's :at in Throwable->map), so each level
     ;; prints just that frame and each cause under its own Caused by:.
     ;; The depth take and the RestFn/AFn filter are kept though native
     ;; frames never match the filter.
     (binding [*out* *err*]
       (when (#{:read-source :macro-syntax-check :macroexpansion :compile-syntax-check :compilation}
               (-> e ex-data :clojure.error/phase))
         (println "Note: The following stack trace applies to the reader or compiler, your code was not executed."))
       (println (str (-> e class (.getSimpleName)) " "
                     (.getMessage e)
                     (when-let [info (ex-data e)] (str " " (pr-str info)))))
       (let [st [(-> e Throwable->map :via first :at)]
             cause (ex-cause e)]
         (doseq [el (take depth
                          (remove #(#{"clojure.lang.RestFn" "clojure.lang.AFn"} (str (first %)))
                                  st))]
           (println (str \tab (stack-element-str el))))
         (when cause
           (println "Caused by:")
           (pst cause (min depth
                           (+ 2 (- (count (:trace (Throwable->map cause)))
                                   (count st))))))))))

;; ----------------------------------------------------------------------
;; Handle Ctrl-C keystrokes

(defn thread-stopper
  "Returns a function that takes one arg and uses that as an exception message
  to stop the given thread.  Defaults to the current thread"
  ;; Divergence: Thread/currentThread and Thread.stop have no counterpart
  ;; here; both arities throw.
  ([] (thread-stopper nil))
  ([thread] (throw (ex-info "thread-stopper is not supported on this runtime (no threads to stop)." {}))))

(defn set-break-handler!
  "Register INT signal handler.  After calling this, Ctrl-C will cause
  the given function f to be called with a single argument, the signal.
  Uses thread-stopper if no function given."
  ;; Divergence: sun.misc.Signal has no counterpart here; both arities throw.
  ([] (set-break-handler! nil))
  ([f]
   (throw (ex-info "set-break-handler! is not supported on this runtime (no signal handlers)." {}))))
