;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; PORT (stdlib W2, issue #204): same public API as upstream
;; `src/clj/clojure/test.clj`. JVM divergences are marked DIVERGENCE below:
;; no thread stacks (assertion file/line is captured at macroexpansion and
;; bound to *location* for do-report), no exception ctors (throw via the
;; private throw-illegal-arg! native), clojure.template (W1) is inlined as
;; the private apply-template, clojure.stacktrace (W4) is replaced by the
;; minimal private print-cause-trace, and macro expansions use fully
;; qualified clojure.test refs (this runtime analyzes expansions at the
;; use site; upstream relies on home-ns resolution).

;;; test.clj: test framework for Clojure

;; by Stuart Sierra
;; March 28, 2009

;; Thanks to Chas Emerick, Allen Rohner, and Stuart Halloway for
;; contributions and suggestions.

(ns 
  ^{:author "Stuart Sierra, with contributions and suggestions by 
  Chas Emerick, Allen Rohner, and Stuart Halloway",
     :doc "A unit testing framework.

   ASSERTIONS

   The core of the library is the \"is\" macro, which lets you make
   assertions of any arbitrary expression:

   (is (= 4 (+ 2 2)))
   (is (instance? Integer 256))
   (is (.startsWith \"abcde\" \"ab\"))

   You can type an \"is\" expression directly at the REPL, which will
   print a message if it fails.

       user> (is (= 5 (+ 2 2)))

       FAIL in  (:1)
       expected: (= 5 (+ 2 2))
         actual: (not (= 5 4))
       false

   The \"expected:\" line shows you the original expression, and the
   \"actual:\" shows you what actually happened.  In this case, it
   shows that (+ 2 2) returned 4, which is not = to 5.  Finally, the
   \"false\" on the last line is the value returned from the
   expression.  The \"is\" macro always returns the result of the
   inner expression.

   There are two special assertions for testing exceptions.  The
   \"(is (thrown? c ...))\" form tests if an exception of class c is
   thrown:

   (is (thrown? ArithmeticException (/ 1 0))) 

   \"(is (thrown-with-msg? c re ...))\" does the same thing and also
   tests that the message on the exception matches the regular
   expression re:

   (is (thrown-with-msg? ArithmeticException #\"Divide by zero\"
                         (/ 1 0)))

   DOCUMENTING TESTS

   \"is\" takes an optional second argument, a string describing the
   assertion.  This message will be included in the error report.

   (is (= 5 (+ 2 2)) \"Crazy arithmetic\")

   In addition, you can document groups of assertions with the
   \"testing\" macro, which takes a string followed by any number of
   assertions.  The string will be included in failure reports.
   Calls to \"testing\" may be nested, and all of the strings will be
   joined together with spaces in the final report, in a style
   similar to RSpec <http://rspec.info/>

   (testing \"Arithmetic\"
     (testing \"with positive integers\"
       (is (= 4 (+ 2 2)))
       (is (= 7 (+ 3 4))))
     (testing \"with negative integers\"
       (is (= -4 (+ -2 -2)))
       (is (= -1 (+ 3 -4)))))

   Note that, unlike RSpec, the \"testing\" macro may only be used
   INSIDE a \"deftest\" or \"with-test\" form (see below).


   DEFINING TESTS

   There are two ways to define tests.  The \"with-test\" macro takes
   a defn or def form as its first argument, followed by any number
   of assertions.  The tests will be stored as metadata on the
   definition.

   (with-test
       (defn my-function [x y]
         (+ x y))
     (is (= 4 (my-function 2 2)))
     (is (= 7 (my-function 3 4))))

   As of Clojure SVN rev. 1221, this does not work with defmacro.
   See http://code.google.com/p/clojure/issues/detail?id=51

   The other way lets you define tests separately from the rest of
   your code, even in a different namespace:

   (deftest addition
     (is (= 4 (+ 2 2)))
     (is (= 7 (+ 3 4))))

   (deftest subtraction
     (is (= 1 (- 4 3)))
     (is (= 3 (- 7 4))))

   This creates functions named \"addition\" and \"subtraction\", which
   can be called like any other function.  Therefore, tests can be
   grouped and composed, in a style similar to the test framework in
   Peter Seibel's \"Practical Common Lisp\"
   <http://www.gigamonkeys.com/book/practical-building-a-unit-test-framework.html>

   (deftest arithmetic
     (addition)
     (subtraction))

   The names of the nested tests will be joined in a list, like
   \"(arithmetic addition)\", in failure reports.  You can use nested
   tests to set up a context shared by several tests.


   RUNNING TESTS

   Run tests with the function \"(run-tests namespaces...)\":

   (run-tests 'your.namespace 'some.other.namespace)

   If you don't specify any namespaces, the current namespace is
   used.  To run all tests in all namespaces, use \"(run-all-tests)\".

   By default, these functions will search for all tests defined in
   a namespace and run them in an undefined order.  However, if you
   are composing tests, as in the \"arithmetic\" example above, you
   probably do not want the \"addition\" and \"subtraction\" tests run
   separately.  In that case, you must define a special function
   named \"test-ns-hook\" that runs your tests in the correct order:

   (defn test-ns-hook []
     (arithmetic))

   Note: test-ns-hook prevents execution of fixtures (see below).


   OMITTING TESTS FROM PRODUCTION CODE

   You can bind the variable \"*load-tests*\" to false when loading or
   compiling code in production.  This will prevent any tests from
   being created by \"with-test\" or \"deftest\".


   FIXTURES

   Fixtures allow you to run code before and after tests, to set up
   the context in which tests should be run.

   A fixture is just a function that calls another function passed as
   an argument.  It looks like this:

   (defn my-fixture [f]
      Perform setup, establish bindings, whatever.
     (f)  Then call the function we were passed.
      Tear-down / clean-up code here.
    )

   Fixtures are attached to namespaces in one of two ways.  \"each\"
   fixtures are run repeatedly, once for each test function created
   with \"deftest\" or \"with-test\".  \"each\" fixtures are useful for
   establishing a consistent before/after state for each test, like
   clearing out database tables.

   \"each\" fixtures can be attached to the current namespace like this:
   (use-fixtures :each fixture1 fixture2 ...)
   The fixture1, fixture2 are just functions like the example above.
   They can also be anonymous functions, like this:
   (use-fixtures :each (fn [f] setup... (f) cleanup...))

   The other kind of fixture, a \"once\" fixture, is only run once,
   around ALL the tests in the namespace.  \"once\" fixtures are useful
   for tasks that only need to be performed once, like establishing
   database connections, or for time-consuming tasks.

   Attach \"once\" fixtures to the current namespace like this:
   (use-fixtures :once fixture1 fixture2 ...)

   Note: Fixtures and test-ns-hook are mutually incompatible.  If you
   are using test-ns-hook, fixture functions will *never* be run.


   SAVING TEST OUTPUT TO A FILE

   All the test reporting functions write to the var *test-out*.  By
   default, this is the same as *out*, but you can rebind it to any
   PrintWriter.  For example, it could be a file opened with
   clojure.java.io/writer.


   EXTENDING TEST-IS (ADVANCED)

   You can extend the behavior of the \"is\" macro by defining new
   methods for the \"assert-expr\" multimethod.  These methods are
   called during expansion of the \"is\" macro, so they should return
   quoted forms to be evaluated.

   You can plug in your own test-reporting framework by rebinding
   the \"report\" function: (report event)

   The 'event' argument is a map.  It will always have a :type key,
   whose value will be a keyword signaling the type of event being
   reported.  Standard events with :type value of :pass, :fail, and
   :error are called when an assertion passes, fails, and throws an
   exception, respectively.  In that case, the event will also have
   the following keys:

     :expected   The form that was expected to be true
     :actual     A form representing what actually occurred
     :message    The string message given as an argument to 'is'

   The \"testing\" strings will be a list in \"*testing-contexts*\", and
   the vars being tested will be a list in \"*testing-vars*\".

   Your \"report\" function should wrap any printing calls in the
   \"with-test-out\" macro, which rebinds *out* to the current value
   of *test-out*.

   The event types are:
   :default :pass :fail :error :summary :begin-test-ns :end-test-ns
   :begin-test-var :end-test-var
"}
  clojure.test)
;; DIVERGENCE (requires): upstream also requires clojure.template (W1 owns
;; it; `are` uses the private apply-template below instead), clojure.stacktrace
;; (W4 owns it; :error reporting uses the private print-cause-trace below),
;; and clojure.string (only used by the JVM stack sniffing in do-report).

;; Nothing is marked "private" here, so you can rebind things to plug
;; in your own testing or reporting frameworks.


;;; USER-MODIFIABLE GLOBALS

(defonce ^:dynamic
  ^{:doc "True by default.  If set to false, no test functions will
   be created by deftest, set-test, or with-test.  Use this to omit
   tests when compiling or loading production code."
    :added "1.1"}
  *load-tests* true)

(def ^:dynamic
 ^{:doc "The maximum depth of stack traces to print when an Exception
  is thrown during a test.  Defaults to nil, which means print the 
  complete stack trace."
   :added "1.1"}
 *stack-trace-depth* nil)


;;; GLOBALS USED BY THE REPORTING FUNCTIONS

(def ^:dynamic *report-counters* nil)	  ; bound to a ref of a map in test-ns

(def ^:dynamic *initial-report-counters*  ; used to initialize *report-counters*
     {:test 0, :pass 0, :fail 0, :error 0})

(def ^:dynamic *testing-vars* (list))  ; bound to hierarchy of vars being tested

(def ^:dynamic *testing-contexts* (list)) ; bound to hierarchy of "testing" strings

(def ^:dynamic *test-out* *out*)         ; PrintWriter for test reporting output

(defmacro with-test-out
  "Runs body with *out* bound to the value of *test-out*."
  {:added "1.1"}
  [& body]
  ;; DIVERGENCE: vars qualified; expansions analyze at the use site.
  `(binding [clojure.core/*out* clojure.test/*test-out*]
     ~@body))

;;; UTILITIES FOR REPORTING FUNCTIONS

(defn file-position
  "Returns a vector [filename line-number] for the nth call up the
  stack.

  Deprecated in 1.2: The information needed for test reporting is
  now on :file and :line keys in the result map."
  {:added "1.1"
   :deprecated "1.2"}
  [n]
  ;; DIVERGENCE: upstream walks (new Throwable) stack frames; without thread
  ;; stacks this returns the current *file* and an unknown line. n is ignored.
  [*file* nil])

(defn testing-vars-str
  "Returns a string representation of the current test.  Renders names
  in *testing-vars* as a list, then the source file and line of
  current assertion."
  {:added "1.1"}
  [m]
  (let [{:keys [file line]} m]
    (str
     ;; Uncomment to include namespace in failure report:
     ;;(ns-name (:ns (meta (first *testing-vars*)))) "/ "
     (reverse (map #(:name (meta %)) *testing-vars*))
     " (" file ":" line ")")))

(defn testing-contexts-str
  "Returns a string representation of the current test context. Joins
  strings in *testing-contexts* with spaces."
  {:added "1.1"}
  []
  (apply str (interpose " " (reverse *testing-contexts*))))

(defn inc-report-counter
  "Increments the named counter in *report-counters*, a ref to a map.
  Does nothing if *report-counters* is nil."
  {:added "1.1"}
  [name]
  (when *report-counters*
    (dosync (commute *report-counters* update-in [name] (fnil inc 0)))))

;;; TEST RESULT REPORTING

(defmulti
  ^{:doc "Generic reporting function, may be overridden to plug in
   different report formats (e.g., TAP, JUnit).  Assertions such as
   'is' call 'report' to indicate results. The argument given to
   'report' will be a map with a :type key. See (doc clojure.test)
   docs under 'EXTENDING TEST-IS' for types of arguments for
   'report'."
     :dynamic true
     :added "1.1"}
  report :type)

(def ^:dynamic ^:private *location*
  ;; DIVERGENCE: no thread stacks exist, so `is` binds the assertion's
  ;; expansion-site {:file :line} (file from *file*, line from (:line (meta
  ;; &form))) here; do-report merges it into :fail/:error events. Custom
  ;; assert-expr methods inherit it at no cost.
  nil)

(defn- error-file-and-line
  ;; DIVERGENCE: upstream reads (.getStackTrace actual) frames; the port has
  ;; one emulated frame per throwable (StackTraceElement->vec [class method
  ;; file line]), whose line is the reader/ex-data line or -1 when unknown.
  ;; Returns {:line ...} only when a real line is known, else nil (merges
  ;; cleanly).
  [actual]
  (when (instance? Throwable actual)
    (let [line (nth (StackTraceElement->vec actual) 3)]
      (when-not (= -1 line) {:line line}))))

(defn- file-and-line
  {:deprecated "1.8"}
  ;; DIVERGENCE: upstream reads JVM frame `depth` via .getStackTrace; the
  ;; port has one emulated frame per throwable (see error-file-and-line):
  ;; depth is ignored and unknown lines become nil.
  [exception depth]
  {:file *file* :line (:line (error-file-and-line exception))})

;; NOTE: upstream stacktrace-file-and-line is dropped: it consumed JVM
;; StackTraceElement seqs, which cannot exist here; error-file-and-line
;; covers the port's single emulated frame.

(defn do-report
  "Add file and line information to a test result and call report.
   If you are writing a custom assert-expr method, call this function
   to pass test results to report."
  {:added "1.2"}
  [m]
  ;; DIVERGENCE: upstream recovers file/line from JVM thread stacks per
  ;; event; here `is` binds the expansion-site *location* (above) and
  ;; do-report merges it, so custom assert-expr methods get locations too.
  ;; Explicit :file/:line keys on m always win.
  (report
   (case
    (:type m)
    :fail (merge {:file *file* :line nil}
                 (error-file-and-line (:actual m))
                 *location*
                 m)
    :error (merge {:file *file* :line nil}
                  (error-file-and-line (:actual m))
                  *location*
                  m)
    m)))

(defn- print-cause-trace
  ;; DIVERGENCE: minimal local stand-in for clojure.stacktrace/print-cause-trace
  ;; (W4 owns that ns; upstream test.clj requires it). Renders the
  ;; Throwable->map chain instead of JVM frames: one "type: message data?"
  ;; line per link plus its emulated frame. depth caps the links (nil = all).
  [tr depth]
  (let [via (:via (Throwable->map tr))
        links (if (nil? depth) via (take depth via))]
    (loop [links links first? true]
      (when (seq links)
        (let [{:keys [type message data at]} (first links)]
          (when-not first? (println "Caused by:"))
          (print (str type))
          (when message (print (str ": " message)))
          (when data (print (str " " (pr-str data))))
          (newline)
          (println (str " at " (pr-str at)))
          (recur (next links) false))))))

(defmethod report :default [m]
  (with-test-out (prn m)))

(defmethod report :pass [m]
  (with-test-out (inc-report-counter :pass)))

(defmethod report :fail [m]
  (with-test-out
    (inc-report-counter :fail)
    (println "\nFAIL in" (testing-vars-str m))
    (when (seq *testing-contexts*) (println (testing-contexts-str)))
    (when-let [message (:message m)] (println message))
    (println "expected:" (pr-str (:expected m)))
    (println "  actual:" (pr-str (:actual m)))))

(defmethod report :error [m]
  (with-test-out
   (inc-report-counter :error)
   (println "\nERROR in" (testing-vars-str m))
   (when (seq *testing-contexts*) (println (testing-contexts-str)))
   (when-let [message (:message m)] (println message))
   (println "expected:" (pr-str (:expected m)))
   (print "  actual: ")
   (let [actual (:actual m)]
     (if (instance? Throwable actual)
       (print-cause-trace actual *stack-trace-depth*)
       (prn actual)))))

(defmethod report :summary [m]
  (with-test-out
   (println "\nRan" (:test m) "tests containing"
            (+ (:pass m) (:fail m) (:error m)) "assertions.")
   (println (:fail m) "failures," (:error m) "errors.")))

(defmethod report :begin-test-ns [m]
  (with-test-out
   (println "\nTesting" (ns-name (:ns m)))))

;; Ignore these message types:
(defmethod report :end-test-ns [m])
(defmethod report :begin-test-var [m])
(defmethod report :end-test-var [m])



;;; UTILITIES FOR ASSERTIONS

(defn get-possibly-unbound-var
  "Like var-get but returns nil if the var is unbound."
  {:added "1.1"}
  [v]
  ;; DIVERGENCE: this runtime's var-get returns the Unbound sentinel instead
  ;; of throwing, so check bound? first (the catch is kept for shape).
  (try (when (bound? v) (var-get v))
       (catch IllegalStateException e
         nil)))

(defn function?
  "Returns true if argument is a function or a symbol that resolves to
  a function (not a macro)."
  {:added "1.1"}
  [x]
  (if (symbol? x)
    (when-let [v (resolve x)]
      (when-let [value (get-possibly-unbound-var v)]
        (and (fn? value)
             (not (:macro (meta v))))))
    (fn? x)))

(defn assert-predicate
  "Returns generic assertion code for any functional predicate.  The
  'expected' argument to 'report' will contains the original form, the
  'actual' argument will contain the form with all its sub-forms
  evaluated.  If the predicate returns false, the 'actual' form will
  be wrapped in (not...)."
  {:added "1.1"}
  [msg form]
  ;; DIVERGENCE: do-report qualified; expansions analyze at the use site.
  (let [args (rest form)
        pred (first form)]
    `(let [values# (list ~@args)
           result# (apply ~pred values#)]
       (if result#
         (clojure.test/do-report {:type :pass, :message ~msg,
                  :expected '~form, :actual (cons '~pred values#)})
         (clojure.test/do-report {:type :fail, :message ~msg,
                  :expected '~form, :actual (list '~'not (cons '~pred values#))}))
       result#)))

(defn assert-any
  "Returns generic assertion code for any test, including macros, Java
  method calls, or isolated symbols."
  {:added "1.1"}
  [msg form]
  ;; DIVERGENCE: do-report qualified; expansions analyze at the use site.
  `(let [value# ~form]
     (if value#
       (clojure.test/do-report {:type :pass, :message ~msg,
                :expected '~form, :actual value#})
       (clojure.test/do-report {:type :fail, :message ~msg,
                :expected '~form, :actual value#}))
     value#))



;;; ASSERTION METHODS

;; You don't call these, but you can add methods to extend the 'is'
;; macro.  These define different kinds of tests, based on the first
;; symbol in the test expression.

(defmulti assert-expr 
  (fn [msg form]
    (cond
      (nil? form) :always-fail
      (seq? form) (first form)
      :else :default)))

(defmethod assert-expr :always-fail [msg form]
  ;; nil test: always fail
  ;; DIVERGENCE: do-report qualified; expansions analyze at the use site.
  `(clojure.test/do-report {:type :fail, :message ~msg}))

(defmethod assert-expr :default [msg form]
  (if (and (sequential? form) (function? (first form)))
    (assert-predicate msg form)
    (assert-any msg form)))

(defmethod assert-expr 'instance? [msg form]
  ;; Test if x is an instance of y.
  ;; DIVERGENCE: do-report qualified; expansions analyze at the use site.
  `(let [klass# ~(nth form 1)
         object# ~(nth form 2)]
     (let [result# (instance? klass# object#)]
       (if result#
         (clojure.test/do-report {:type :pass, :message ~msg,
                  :expected '~form, :actual (class object#)})
         (clojure.test/do-report {:type :fail, :message ~msg,
                  :expected '~form, :actual (class object#)}))
       result#)))

(defmethod assert-expr 'thrown? [msg form]
  ;; (is (thrown? c expr))
  ;; Asserts that evaluating expr throws an exception of class c.
  ;; Returns the exception thrown.
  ;; DIVERGENCE: do-report qualified (use-site analysis); the try/catch is
  ;; list-built because this runtime only recognizes concrete-list catch
  ;; clauses while syntax-quote yields lazy seqs.
  (let [klass (second form)
        body (nthnext form 2)
        e (gensym "e")]
    (apply list 'try
           (concat body
                   [(list 'clojure.test/do-report
                          {:type :fail :message msg
                           :expected (list 'quote form) :actual nil})
                    (list 'catch klass e
                          (list 'clojure.test/do-report
                                {:type :pass :message msg
                                 :expected (list 'quote form) :actual e})
                          e)]))))

(defmethod assert-expr 'thrown-with-msg? [msg form]
  ;; (is (thrown-with-msg? c re expr))
  ;; Asserts that evaluating expr throws an exception of class c.
  ;; Also asserts that the message string of the exception matches
  ;; (with re-find) the regular expression re.
  ;; DIVERGENCE: do-report qualified (use-site analysis); the try/catch is
  ;; list-built because this runtime only recognizes concrete-list catch
  ;; clauses while syntax-quote yields lazy seqs.
  (let [klass (nth form 1)
        re (nth form 2)
        body (nthnext form 3)
        e (gensym "e")
        m (gensym "m")]
    (apply list 'try
           (concat body
                   [(list 'clojure.test/do-report
                          {:type :fail :message msg
                           :expected (list 'quote form) :actual nil})
                    (list 'catch klass e
                          (list 'let [m (list '.getMessage e)]
                                (list 'if (list 're-find re m)
                                      (list 'clojure.test/do-report
                                            {:type :pass :message msg
                                             :expected (list 'quote form)
                                             :actual e})
                                      (list 'clojure.test/do-report
                                            {:type :fail :message msg
                                             :expected (list 'quote form)
                                             :actual e})))
                          e)]))))


(defmacro try-expr
  "Used by the 'is' macro to catch unexpected exceptions.
  You don't call this."
  {:added "1.1"}
  [msg form]
  ;; DIVERGENCE: do-report qualified (use-site analysis); the try/catch is
  ;; list-built because this runtime only recognizes concrete-list catch
  ;; clauses while syntax-quote yields lazy seqs. :error events inherit the
  ;; enclosing `is` *location* via do-report.
  (let [t (gensym "t")]
    (list 'try (assert-expr msg form)
          (list 'catch 'Throwable t
                (list 'clojure.test/do-report
                      {:type :error :message msg
                       :expected (list 'quote form) :actual t})))))



;;; ASSERTION MACROS

;; You use these in your tests.

(defmacro is
  "Generic assertion macro.  'form' is any predicate test.
  'msg' is an optional message to attach to the assertion.
  
  Example: (is (= 4 (+ 2 2)) \"Two plus two should be 4\")

  Special forms:

  (is (thrown? c body)) checks that an instance of c is thrown from
  body, fails if not; then returns the thing thrown.

  (is (thrown-with-msg? c re body)) checks that an instance of c is
  thrown AND that the message on the exception matches (with
  re-find) the regular expression re."
  {:added "1.1"}
  ([form]
   ;; DIVERGENCE: the rebuilt call is list-built (concrete) with &form's
   ;; meta propagated: syntax-quote would drop :line/:column and yield a
   ;; lazy seq, which cannot carry meta here.
   (with-meta (list 'clojure.test/is form nil) (meta &form)))
  ([form msg]
   ;; DIVERGENCE: refs qualified (use-site analysis); binds the
   ;; expansion-site *location* (no thread stacks to sniff instead).
   `(binding [clojure.test/*location*
              {:file ~*file* :line ~(:line (meta &form))}]
      (clojure.test/try-expr ~msg ~form))))

(defn- apply-template
  ;; DIVERGENCE: minimal inline of clojure.template/apply-template (W1 owns
  ;; that ns; upstream test.clj requires it). Same contract: argv must be a
  ;; vector of symbols; values substitute for argv symbols throughout expr
  ;; (code-shaped data: lists, vectors, maps, sets).
  [argv expr values]
  (assert (vector? argv))
  (assert (every? symbol? argv))
  (let [smap (zipmap argv values)]
    (letfn [(subst [form]
              (cond
                (map? form) (into {} (map (fn [[k v]] [(subst k) (subst v)])) form)
                (vector? form) (vec (map subst form))
                (set? form) (set (map subst form))
                (seq? form) (apply list (map subst form))
                :else (get smap form form)))]
      (subst expr))))

(defmacro are
  "Checks multiple assertions with a template expression.
  See clojure.template/do-template for an explanation of
  templates.

  Example: (are [x y] (= x y)  
                2 (+ 1 1)
                4 (* 2 2))
  Expands to: 
           (do (is (= 2 (+ 1 1)))
               (is (= 4 (* 2 2))))

  Note: This breaks some reporting features, such as line numbers."
  {:added "1.1"}
  [argv expr & args]
  (if (or
       ;; (are [] true) is meaningless but ok
       (and (empty? argv) (empty? args))
       ;; Catch wrong number of args
       (and (pos? (count argv))
            (pos? (count args))
            (zero? (mod (count args) (count argv)))))
    ;; DIVERGENCE: clojure.template is W1 (unavailable here), so `are`
    ;; substitutes via the private apply-template above (same output shape
    ;; as upstream temp/do-template) instead of emitting a temp/do-template
    ;; call; each `is` is line-stamped with the `are` form's line, and the
    ;; argv-mismatch throw goes through the private throw-illegal-arg!
    ;; native (no exception ctors exist here).
    (let [line (:line (meta &form))
          template (list 'clojure.test/is expr)
          groups (partition (count argv) args)]
      (apply list 'do
             (map (fn [values]
                    (with-meta (apply-template argv template values)
                      {:line line}))
                  groups)))
    ((var clojure.core/throw-illegal-arg!)
     "The number of args doesn't match are's argv.")))

(defmacro testing
  "Adds a new string to the list of testing contexts.  May be nested,
  but must occur inside a test function (deftest)."
  {:added "1.1"}
  [string & body]
  ;; DIVERGENCE: vars qualified; expansions analyze at the use site.
  `(binding [clojure.test/*testing-contexts*
             (conj clojure.test/*testing-contexts* ~string)]
     ~@body))



;;; DEFINING TESTS

(defmacro with-test
  "Takes any definition form (that returns a Var) as the first argument.
  Remaining body goes in the :test metadata function for that Var.

  When *load-tests* is false, only evaluates the definition, ignoring
  the tests."
  {:added "1.1"}
  [definition & body]
  (if *load-tests*
    `(doto ~definition (alter-meta! assoc :test (fn [] ~@body)))
    definition))


(defmacro deftest
  "Defines a test function with no arguments.  Test functions may call
  other tests, so tests may be composed.  If you compose tests, you
  should also define a function named test-ns-hook; run-tests will
  call test-ns-hook instead of testing all vars.

  Note: Actually, the test body goes in the :test metadata on the var,
  and the real function (the value of the var) calls test-var on
  itself.

  When *load-tests* is false, deftest is ignored."
  {:added "1.1"}
  [name & body]
  (when *load-tests*
    ;; DIVERGENCE: test-var qualified (use-site analysis); the :test fn is
    ;; installed via alter-meta! in code position because this runtime's def
    ;; stores metadata as data (upstream evaluates the :test fn form).
    `(do (def ~name (fn [] (clojure.test/test-var (var ~name))))
         (alter-meta! (var ~name) assoc :test (fn [] ~@body))
         (var ~name))))

(defmacro deftest-
  "Like deftest but creates a private var."
  {:added "1.1"}
  [name & body]
  (when *load-tests*
    ;; DIVERGENCE: test-var qualified (use-site analysis); the :test fn is
    ;; installed via alter-meta! in code position because this runtime's def
    ;; stores metadata as data (upstream evaluates the :test fn form).
    `(do (def ~(vary-meta name assoc :private true)
            (fn [] (clojure.test/test-var (var ~name))))
         (alter-meta! (var ~name) assoc :test (fn [] ~@body))
         (var ~name))))


(defmacro set-test
  "Experimental.
  Sets :test metadata of the named var to a fn with the given body.
  The var must already exist.  Does not modify the value of the var.

  When *load-tests* is false, set-test is ignored."
  {:added "1.1"}
  [name & body]
  (when *load-tests*
    `(alter-meta! (var ~name) assoc :test (fn [] ~@body))))



;;; DEFINING FIXTURES

(defn- add-ns-meta
  "Adds elements in coll to the current namespace metadata as the
  value of key."
  {:added "1.1"}
  [key coll]
  (alter-meta! *ns* assoc key coll))

(defmulti use-fixtures
  "Wrap test runs in a fixture function to perform setup and
  teardown. Using a fixture-type of :each wraps every test
  individually, while :once wraps the whole run in a single function."
  {:added "1.1"}
  (fn [fixture-type & args] fixture-type))

(defmethod use-fixtures :each [fixture-type & args]
  (add-ns-meta ::each-fixtures args))

(defmethod use-fixtures :once [fixture-type & args]
  (add-ns-meta ::once-fixtures args))

(defn- default-fixture
  "The default, empty, fixture function.  Just calls its argument."
  {:added "1.1"}
  [f]
  (f))

(defn compose-fixtures
  "Composes two fixture functions, creating a new fixture function
  that combines their behavior."
  {:added "1.1"}
  [f1 f2]
  (fn [g] (f1 (fn [] (f2 g)))))

(defn join-fixtures
  "Composes a collection of fixtures, in order.  Always returns a valid
  fixture function, even if the collection is empty."
  {:added "1.1"}
  [fixtures]
  (reduce compose-fixtures default-fixture fixtures))




;;; RUNNING TESTS: LOW-LEVEL FUNCTIONS

(defn test-var
  "If v has a function in its :test metadata, calls that function,
  with *testing-vars* bound to (conj *testing-vars* v)."
  {:dynamic true, :added "1.1"}
  [v]
  (when-let [t (:test (meta v))]
    (binding [*testing-vars* (conj *testing-vars* v)]
      (do-report {:type :begin-test-var, :var v})
      (inc-report-counter :test)
      (try (t)
           (catch Throwable e
             (do-report {:type :error, :message "Uncaught exception, not in assertion."
                      :expected nil, :actual e})))
      (do-report {:type :end-test-var, :var v}))))

(defn test-vars
  "Groups vars by their namespace and runs test-var on them with
  appropriate fixtures applied."
  {:added "1.6"}
  [vars]
  (doseq [[ns vars] (group-by (comp :ns meta) vars)]
    (let [once-fixture-fn (join-fixtures (::once-fixtures (meta ns)))
          each-fixture-fn (join-fixtures (::each-fixtures (meta ns)))]
      (once-fixture-fn
       (fn []
         (doseq [v vars]
           (when (:test (meta v))
             (each-fixture-fn (fn [] (test-var v))))))))))

(defn test-all-vars
  "Calls test-vars on every var interned in the namespace, with fixtures."
  {:added "1.1"}
  [ns]
  (test-vars (vals (ns-interns ns))))

(defn test-ns
  "If the namespace defines a function named test-ns-hook, calls that.
  Otherwise, calls test-all-vars on the namespace.  'ns' is a
  namespace object or a symbol.

  Internally binds *report-counters* to a ref initialized to
  *initial-report-counters*.  Returns the final, dereferenced state of
  *report-counters*."
  {:added "1.1"}
  [ns]
  (binding [*report-counters* (ref *initial-report-counters*)]
    (let [ns-obj (the-ns ns)]
      (do-report {:type :begin-test-ns, :ns ns-obj})
      ;; If the namespace has a test-ns-hook function, call that:
      (if-let [v (find-var (symbol (str (ns-name ns-obj)) "test-ns-hook"))]
	((var-get v))
        ;; Otherwise, just test every var in the namespace.
        (test-all-vars ns-obj))
      (do-report {:type :end-test-ns, :ns ns-obj}))
    @*report-counters*))



;;; RUNNING TESTS: HIGH-LEVEL FUNCTIONS

(defn run-tests
  "Runs all tests in the given namespaces; prints results.
  Defaults to current namespace if none given.  Returns a map
  summarizing test results."
  {:added "1.1"}
  ([] (run-tests *ns*))
  ([& namespaces]
     (let [summary (assoc (apply merge-with + (map test-ns namespaces))
                     :type :summary)]
       (do-report summary)
       summary)))

(defn run-all-tests
  "Runs all tests in all namespaces; prints results.
  Optional argument is a regular expression; only namespaces with
  names matching the regular expression (with re-matches) will be
  tested."
  {:added "1.1"}
  ([] (apply run-tests (all-ns)))
  ([re] (apply run-tests (filter #(re-matches re (name (ns-name %))) (all-ns)))))

(defn successful?
  "Returns true if the given test summary indicates all tests
  were successful, false otherwise."
  {:added "1.1"}
  [summary]
  (and (zero? (:fail summary 0))
       (zero? (:error summary 0))))

(defn run-test-var
  "Runs the tests for a single Var, with fixtures executed around the test, and summary output after."
  {:added "1.11"}
  [v]
  (binding [*report-counters* (ref *initial-report-counters*)]
    (let [ns-obj (-> v meta :ns)
          summary (do
                    (do-report {:type :begin-test-ns
                                :ns   ns-obj})
                    (test-vars [v])
                    (do-report {:type :end-test-ns
                                :ns   ns-obj})
                    (assoc @*report-counters* :type :summary))]
      (do-report summary)
      summary)))

(defmacro run-test
  "Runs a single test.

  Because the intent is to run a single test, there is no check for the namespace test-ns-hook."
  {:added "1.11"}
  [test-symbol]
  (let [test-var (resolve test-symbol)]
    (cond
      (nil? test-var)
      (binding [*out* *err*]
        (println "Unable to resolve" test-symbol "to a test function."))

      (not (-> test-var meta :test))
      (binding [*out* *err*]
        (println test-symbol "is not a test."))

      :else
      ;; DIVERGENCE: run-test-var qualified; expansions analyze at the use site.
      `(clojure.test/run-test-var ~test-var))))
