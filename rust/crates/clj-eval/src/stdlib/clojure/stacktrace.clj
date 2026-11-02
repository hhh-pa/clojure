;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;;; stacktrace.clj: print Clojure-centric stack traces

;; by Stuart Sierra
;; January 6, 2009

;; Port of upstream src/clj/clojure/stacktrace.clj (W4 entry/IO).
;;
;; The ns metadata lives in the positional docstring/attr slots: upstream
;; carries ^{:doc :author} on the ns symbol, which this loader does not read.
;; Upstream's top-level (set! *warn-on-reflection* true) is dropped:
;; reflection warnings are a JVM-compiler knob with no meaning here, and a
;; top-level set! would fail a bare require (no thread binding in flight).

(ns clojure.stacktrace
  "Print stack traces oriented towards Clojure, not Java."
  {:author "Stuart Sierra"})

;; Divergence: StackTraceElement values do not exist on this runtime, so
;; print-trace-element takes a StackTraceElement->vec [class method file
;; line] vector instead, and print-stack-trace prints the throwable's own
;; link :at (one vector — the whole of its emulated stack) rather than
;; (.getStackTrace tr), so chained causes stay out of the frame list as
;; documented. Everything else is verbatim: .getCause, .getMessage,
;; (.getName (class ...)), ex-data, printf, and *e all work.

(defn root-cause
  "Returns the last 'cause' Throwable in a chain of Throwables."
  {:added "1.1"}
  [^Throwable tr]
  (if-let [cause (.getCause tr)]
    (recur cause)
    tr))

(defn print-trace-element
  "Prints a Clojure-oriented view of one element in a stack trace."
  {:added "1.1"}
  [e]
  (let [[class method file line] e
        class (str class)
        method (str method)]
    (let [match (re-matches #"^([A-Za-z0-9_.-]+)\$(\w+)__\d+$" (str class))]
      (if (and match (= "invoke" method))
        (apply printf "%s/%s" (rest match))
        (printf "%s.%s" class method)))
    (printf " (%s:%d)" (or file "") line)))

(defn print-throwable
  "Prints the class and message of a Throwable. Prints the ex-data map
  if present."
  {:added "1.1"}
  [^Throwable tr]
  (printf "%s: %s" (.getName (class tr)) (.getMessage tr))
  (when-let [info (ex-data tr)]
    (newline)
    (pr info)))

(defn print-stack-trace
  "Prints a Clojure-oriented stack trace of tr, a Throwable.
  Prints a maximum of n stack frames (default: unlimited).
  Does not print chained exceptions (causes)."
  {:added "1.1"}
  ([tr] (print-stack-trace tr nil))
  ([^Throwable tr n]
     (let [st [(-> tr Throwable->map :via first :at)]]
       (print-throwable tr)
       (newline)
       (print " at ")
       (if-let [e (first st)]
         (print-trace-element e)
         (print "[empty stack trace]"))
       (newline)
       (doseq [e (if (nil? n)
                   (rest st)
                   (take (dec n) (rest st)))]
         (print "    ")
         (print-trace-element e)
         (newline)))))

(defn print-cause-trace
  "Like print-stack-trace but prints chained exceptions (causes)."
  {:added "1.1"}
  ([tr] (print-cause-trace tr nil))
  ([^Throwable tr n]
     (print-stack-trace tr n)
     (when-let [cause (.getCause tr)]
       (print "Caused by: " )
       (recur cause n))))

(defn e
  "REPL utility.  Prints a brief stack trace for the root cause of the
  most recent exception."
  {:added "1.1"}
  []
  (print-stack-trace (root-cause *e) 8))
