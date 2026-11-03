;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; PORT (stdlib W2, issue #204): same public API as upstream
;; `src/clj/clojure/test/tap.clj`. JVM divergences are marked DIVERGENCE
;; below: clojure.stacktrace (W4) is replaced by clojure.test's private
;; print-cause-trace, and macro expansions use fully qualified refs (this
;; runtime analyzes expansions at the use site).

;;; test_is/tap.clj: Extension to test for TAP output

;; by Stuart Sierra
;; March 31, 2009

;; Inspired by ClojureCheck by Meikel Brandmeyer:
;; http://kotka.de/projects/clojure/clojurecheck.html


;; DOCUMENTATION
;;



(ns ^{:doc "clojure.test extensions for the Test Anything Protocol (TAP)

  TAP is a simple text-based syntax for reporting test results.  TAP
  was originally developed for Perl, and now has implementations in
  several languages.  For more information on TAP, see
  http://testanything.org/ and
  http://search.cpan.org/~petdance/TAP-1.0.0/TAP.pm

  To use this library, wrap any calls to
  clojure.test/run-tests in the with-tap-output macro,
  like this:

    (use 'clojure.test)
    (use 'clojure.test.tap)

    (with-tap-output
     (run-tests 'my.cool.library))"
       :author "Stuart Sierra"}
  clojure.test.tap
  (:require [clojure.test :as t]))
;; DIVERGENCE (requires): upstream also requires clojure.stacktrace (W4 owns
;; it); trace printing uses clojure.test's private print-cause-trace instead.
(def ^:private print-cause-trace @#'clojure.test/print-cause-trace)

(defn print-tap-plan
  "Prints a TAP plan line like '1..n'.  n is the number of tests"
  {:added "1.1"}
  [n]
  (println (str "1.." n)))

(defn print-tap-diagnostic
  "Prints a TAP diagnostic line.  data is a (possibly multi-line)
  string."
  {:added "1.1"}
  [data]
  (doseq [line (.split ^String data "\n")]
    (println "#" line)))

(defn print-tap-pass
  "Prints a TAP 'ok' line.  msg is a string, with no line breaks"
  {:added "1.1"}
  [msg]
  (println "ok" msg))

(defn print-tap-fail 
  "Prints a TAP 'not ok' line.  msg is a string, with no line breaks"
  {:added "1.1"}
  [msg]
  (println "not ok" msg))

;; This multimethod will override test/report
(defmulti ^:dynamic tap-report :type)

(defmethod tap-report :default [data]
  (t/with-test-out
   (print-tap-diagnostic (pr-str data))))

(defn print-diagnostics [data]
  (when (seq t/*testing-contexts*)
    (print-tap-diagnostic (t/testing-contexts-str)))
  (when (:message data)
    (print-tap-diagnostic (:message data)))
  (print-tap-diagnostic (str "expected:" (pr-str (:expected data))))
  (if (= :pass (:type data))
    (print-tap-diagnostic (str "  actual:" (pr-str (:actual data))))
    (do
      (print-tap-diagnostic
       (str "  actual:"
        (with-out-str
          (if (instance? Throwable (:actual data))
            (print-cause-trace (:actual data) t/*stack-trace-depth*)
            (prn (:actual data)))))))))

(defmethod tap-report :pass [data]
  (t/with-test-out
   (t/inc-report-counter :pass)
   (print-tap-pass (t/testing-vars-str data))
   (print-diagnostics data)))

(defmethod tap-report :error [data]
  (t/with-test-out
   (t/inc-report-counter :error)
   (print-tap-fail (t/testing-vars-str data))
   (print-diagnostics data)))

(defmethod tap-report :fail [data]
  (t/with-test-out
   (t/inc-report-counter :fail)
   (print-tap-fail (t/testing-vars-str data))
   (print-diagnostics data)))

(defmethod tap-report :summary [data]
  (t/with-test-out
   (print-tap-plan (+ (:pass data) (:fail data) (:error data)))))


(defmacro with-tap-output
  "Execute body with modified test reporting functions that produce
  TAP output"
  {:added "1.1"}
  [& body]
  ;; DIVERGENCE: report vars qualified; expansions analyze at the use site.
  `(binding [clojure.test/report clojure.test.tap/tap-report]
     ~@body))
