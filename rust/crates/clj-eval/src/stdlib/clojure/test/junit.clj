;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; PORT (stdlib W2, issue #204): same public API as upstream
;; `src/clj/clojure/test/junit.clj`. JVM divergences are marked DIVERGENCE
;; below: clojure.stacktrace (W4) is replaced by clojure.test's private
;; print-cause-trace, .lastIndexOf (unavailable here) by a .split-based
;; package/class partition, failure locations come from the report event
;; instead of JVM stacks, and macro expansions use fully qualified refs
;; (this runtime analyzes expansions at the use site).

;; test/junit.clj: Extension to clojure.test for JUnit-compatible XML output

;; by Jason Sankey
;; June 2009

;; DOCUMENTATION
;;

(ns ^{:doc "clojure.test extension for JUnit-compatible XML output.

  JUnit (http://junit.org/) is the most popular unit-testing library
  for Java.  As such, tool support for JUnit output formats is
  common.  By producing compatible output from tests, this tool
  support can be exploited.

  To use, wrap any calls to clojure.test/run-tests in the
  with-junit-output macro, like this:

    (use 'clojure.test)
    (use 'clojure.test.junit)

    (with-junit-output
      (run-tests 'my.cool.library))

  To write the output to a file, rebind clojure.test/*test-out* to
  your own PrintWriter (perhaps opened using
  clojure.java.io/writer)."
  :author "Jason Sankey"}
  clojure.test.junit
  (:require [clojure.test :as t]))
;; DIVERGENCE (requires): upstream also requires clojure.stacktrace (W4 owns
;; it); trace printing uses clojure.test's private print-cause-trace instead.
(def ^:private print-cause-trace @#'clojure.test/print-cause-trace)

;; copied from clojure.contrib.lazy-xml
(def ^{:private true}
     escape-xml-map
     (zipmap "'<>\"&" (map #(str \& % \;) '[apos lt gt quot amp])))
(defn- escape-xml [text]
  (apply str (map #(escape-xml-map % %) text)))

(def ^:dynamic *var-context*)
(def ^:dynamic *depth*)

(defn indent
  []
  (dotimes [n (* *depth* 4)] (print " ")))

(defn start-element
  [tag pretty & [attrs]]
  (if pretty (indent))
  (print (str "<" tag))
  (if (seq attrs)
    (doseq [[key value] attrs]
      (print (str " " (name key) "=\"" (escape-xml value) "\""))))
  (print ">")
  (if pretty (println))
  (set! *depth* (inc *depth*)))

(defn element-content
  [content]
  (print (escape-xml content)))

(defn finish-element
  [tag pretty]
  (set! *depth* (dec *depth*))
  (if pretty (indent))
  (print (str "</" tag ">"))
  (if pretty (println)))

(defn test-name
  [vars]
  (apply str (interpose "."
                        (reverse (map #(:name (meta %)) vars)))))

(defn package-class
  [name]
  ;; DIVERGENCE: upstream uses .lastIndexOf/.substring; this runtime has no
  ;; lastIndexOf, so partition on literal dots via String/split (regex "\\.").
  ;; (Trailing-dot names would differ; test namespaces never have them.)
  (let [parts (vec (.split ^String name "\\."))]
    (if (= 1 (count parts))
      [nil name]
      [(apply str (interpose "." (butlast parts))) (last parts)])))

(defn start-case
  [name classname]
  (start-element 'testcase true {:name name :classname classname}))

(defn finish-case
  []
  (finish-element 'testcase true))

(defn suite-attrs
  [package classname]
  (let [attrs {:name classname}]
    (if package
      (assoc attrs :package package)
      attrs)))

(defn start-suite
  [name]
  (let [[package classname] (package-class name)]
    (start-element 'testsuite true (suite-attrs package classname))))

(defn finish-suite
  []
  (finish-element 'testsuite true))

(defn message-el
  ;; DIVERGENCE: upstream recovers "at: file:line" via (t/file-position 5)
  ;; (JVM stacks); the port threads the report event's :file/:line through
  ;; new trailing args (the old arity is kept and falls back to file-position).
  ([tag message expected-str actual-str]
   (let [[file line] (t/file-position 5)]
     (message-el tag message expected-str actual-str file line)))
  ([tag message expected-str actual-str file line]
   (indent)
   (start-element tag false (if message {:message message} {}))
   (element-content
    (let [detail (apply str (interpose
                            "\n"
                            [(str "expected: " expected-str)
                             (str "  actual: " actual-str)
                             (str "      at: " file ":" line)]))]
      (if message (str message "\n" detail) detail)))
   (finish-element tag false)
   (println)))

(defn failure-el
  ([message expected actual]
   (failure-el message expected actual *file* nil))
  ([message expected actual file line]
   (message-el 'failure message (pr-str expected) (pr-str actual) file line)))

(defn error-el
  ([message expected actual]
   (error-el message expected actual *file* nil))
  ([message expected actual file line]
   (message-el 'error
               message
               (pr-str expected)
               (if (instance? Throwable actual)
                 (with-out-str (print-cause-trace actual t/*stack-trace-depth*))
                 (prn actual))
               file line)))

;; This multimethod will override test-is/report
(defmulti ^:dynamic junit-report :type)

(defmethod junit-report :begin-test-ns [m]
  (t/with-test-out
   (start-suite (name (ns-name (:ns m))))))

(defmethod junit-report :end-test-ns [_]
  (t/with-test-out
   (finish-suite)))

(defmethod junit-report :begin-test-var [m]
  (t/with-test-out
   (let [var (:var m)]
     (binding [*var-context* (conj *var-context* var)]
       (start-case (test-name *var-context*) (name (ns-name (:ns (meta var)))))))))

(defmethod junit-report :end-test-var [m]
  (t/with-test-out
   (finish-case)))

(defmethod junit-report :pass [m]
  (t/with-test-out
   (t/inc-report-counter :pass)))

(defmethod junit-report :fail [m]
  ;; DIVERGENCE: the event's :file/:line feed the "at:" detail (no stacks).
  (t/with-test-out
   (t/inc-report-counter :fail)
   (failure-el (:message m)
               (:expected m)
               (:actual m)
               (:file m)
               (:line m))))

(defmethod junit-report :error [m]
  ;; DIVERGENCE: the event's :file/:line feed the "at:" detail (no stacks).
  (t/with-test-out
   (t/inc-report-counter :error)
   (error-el (:message m)
             (:expected m)
             (:actual m)
             (:file m)
             (:line m))))

(defmethod junit-report :default [_])

(defmacro with-junit-output
  "Execute body with modified test-is reporting functions that write
  JUnit-compatible XML output."
  {:added "1.1"}
  [& body]
  ;; DIVERGENCE: report/context vars qualified; expansions analyze at the use site.
  `(binding [clojure.test/report clojure.test.junit/junit-report
             clojure.test.junit/*var-context* (list)
             clojure.test.junit/*depth* 1]
     (clojure.test/with-test-out
       (println "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")
       (println "<testsuites>"))
     (let [result# (do ~@body)]
       (clojure.test/with-test-out (println "</testsuites>"))
       result#)))
