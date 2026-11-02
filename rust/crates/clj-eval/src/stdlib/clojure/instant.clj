;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/instant.clj (clojure.instant) for
;; clojure-rs: same public API (parse-timestamp, validated,
;; read-instant-date, read-instant-calendar, read-instant-timestamp).
;;
;; Port notes:
;; - No java.util.Date / Calendar / GregorianCalendar / TimeZone /
;;   java.sql.Timestamp: the (:import ...) clause is dropped and all three
;;   readers answer the native instant value (millis UTC) for the same point
;;   in time. The Calendar representation's offset preservation and the
;;   Timestamp representation's sub-millis nanos narrow away (the reader
;;   truncates nanos to millis, exactly like #inst literals do).
;; - construct-* builds a canonical RFC3339 string and reads it through the
;;   default #inst reader (with *data-readers* rebound empty so a user 'inst
;;   binding cannot recurse). Leap seconds (:60) roll forward like the
;;   upstream lenient GregorianCalendar.
;; - fail / verify throw ex-info (upstream: RuntimeException; no such ctor
;;   without a JVM). Messages are unchanged.
;; - The print integration (ThreadLocal + proxy + SimpleDateFormat, the
;;   print-method / print-dup defmethods for Date/Calendar/Timestamp) is
;;   dropped: the engine already prints instants as #inst "...", and
;;   print-method here dispatches on token strings, so class-dispatched
;;   methods could never fire.
;; - (set! *warn-on-reflection* true) is guarded by thread-bound?: the loader
;;   does not bind the var during load, so a bare set! would fail the require.

(ns clojure.instant)


(when (thread-bound? #'*warn-on-reflection*)
  (set! *warn-on-reflection* true))

;;; ------------------------------------------------------------------------
;;; convenience macros

(defmacro ^:private fail
  [msg]
  ;; Upstream: `(throw (RuntimeException. ~msg)). Built with list (not
  ;; syntax-quote): the loader reads the whole file before evaluating it, so
  ;; syntax-quote here would qualify against the stale *ns* (the
  ;; java_shell.clj discipline).
  (list 'throw (list (symbol "clojure.core" "ex-info") msg nil)))

(defmacro ^:private verify
  ([test msg] (list 'when-not test (list 'fail msg)))
  ([test] (list 'verify test (str "failed: " (pr-str test)))))

(defn- divisible?
  [num div]
  (zero? (mod num div)))

(defn- indivisible?
  [num div]
  (not (divisible? num div)))


;;; ------------------------------------------------------------------------
;;; parser implementation

(defn- parse-int [^String s]
  (Long/parseLong s))

(defn- zero-fill-right [^String s width]
  (cond (= width (count s)) s
        (< width (count s)) (.substring s 0 width)
        ;; Upstream loops a StringBuilder to width, but .length is not
        ;; emulated on StringBuilder (append-only), so pad functionally.
        :else (apply str s (repeat (- width (count s)) \0))))

(def ^:private timestamp
  #"(\d\d\d\d)(?:-(\d\d)(?:-(\d\d)(?:[T](\d\d)(?::(\d\d)(?::(\d\d)(?:[.](\d+))?)?)?)?)?)?(?:[Z]|([-+])(\d\d):(\d\d))?")

(defn parse-timestamp
     "Parse a string containing an RFC3339-like like timestamp.

The function new-instant is called with the following arguments.

                min  max           default
                ---  ------------  -------
  years          0           9999      N/A (s must provide years)
  months         1             12        1
  days           1             31        1 (actual max days depends
  hours          0             23        0  on month and year)
  minutes        0             59        0
  seconds        0             60        0 (though 60 is only valid
  nanoseconds    0      999999999        0  when minutes is 59)
  offset-sign   -1              1        0
  offset-hours   0             23        0
  offset-minutes 0             59        0

These are all integers and will be non-nil. (The listed defaults
will be passed if the corresponding field is not present in s.)

Grammar (of s):

  date-fullyear   = 4DIGIT
  date-month      = 2DIGIT  ; 01-12
  date-mday       = 2DIGIT  ; 01-28, 01-29, 01-30, 01-31 based on
                            ; month/year
  time-hour       = 2DIGIT  ; 00-23
  time-minute     = 2DIGIT  ; 00-59
  time-second     = 2DIGIT  ; 00-58, 00-59, 00-60 based on leap second
                            ; rules
  time-secfrac    = '.' 1*DIGIT
  time-numoffset  = ('+' / '-') time-hour ':' time-minute
  time-offset     = 'Z' / time-numoffset

  time-part       = time-hour [ ':' time-minute [ ':' time-second
                    [time-secfrac] [time-offset] ] ]

  timestamp       = date-year [ '-' date-month [ '-' date-mday
                    [ 'T' time-part ] ] ]

Unlike RFC3339:

  - we only parse the timestamp format
  - timestamp can elide trailing components
  - time-offset is optional (defaults to +00:00)

Though time-offset is syntactically optional, a missing time-offset
will be treated as if the time-offset zero (+00:00) had been
specified.
"
    [new-instant ^CharSequence cs]
         (if-let [[_ years months days hours minutes seconds fraction
                   offset-sign offset-hours offset-minutes]
                  (re-matches timestamp cs)]
           (new-instant
            (parse-int years)
            (if-not months   1 (parse-int months))
            (if-not days     1 (parse-int days))
            (if-not hours    0 (parse-int hours))
            (if-not minutes  0 (parse-int minutes))
            (if-not seconds  0 (parse-int seconds))
            (if-not fraction 0 (parse-int (zero-fill-right fraction 9)))
            (cond (= "-" offset-sign) -1
                  (= "+" offset-sign)  1
                  :else                0)
            (if-not offset-hours   0 (parse-int offset-hours))
            (if-not offset-minutes 0 (parse-int offset-minutes)))
           (fail (str "Unrecognized date/time syntax: " cs))))


;;; ------------------------------------------------------------------------
;;; Verification of Extra-Grammatical Restrictions from RFC3339

(defn- leap-year?
  [year]
  (and (divisible? year 4)
       (or (indivisible? year 100)
           (divisible? year 400))))

(def ^:private days-in-month
     (let [dim-norm [nil 31 28 31 30 31 30 31 31 30 31 30 31]
           dim-leap [nil 31 29 31 30 31 30 31 31 30 31 30 31]]
       (fn [month leap-year?]
         ((if leap-year? dim-leap dim-norm) month))))

(defn validated
  "Return a function which constructs an instant by calling constructor
after first validating that those arguments are in range and otherwise
plausible. The resulting function will throw an exception if called
with invalid arguments."
  [new-instance]
  (fn [years months days hours minutes seconds nanoseconds
       offset-sign offset-hours offset-minutes]
    (verify (<= 1 months 12))
    (verify (<= 1 days (days-in-month months (leap-year? years))))
    (verify (<= 0 hours 23))
    (verify (<= 0 minutes 59))
    (verify (<= 0 seconds (if (= minutes 59) 60 59)))
    (verify (<= 0 nanoseconds 999999999))
    (verify (<= -1 offset-sign 1))
    (verify (<= 0 offset-hours 23))
    (verify (<= 0 offset-minutes 59))
    (new-instance years months days hours minutes seconds nanoseconds
                  offset-sign offset-hours offset-minutes)))


;;; ------------------------------------------------------------------------
;;; print integration
;;;
;;; Dropped (see the file header): no Date/Calendar/Timestamp classes, no
;;; ThreadLocal/proxy/SimpleDateFormat, and class-dispatched print methods
;;; could never fire. The engine prints instants as #inst "..." already.


;;; ------------------------------------------------------------------------
;;; reader integration

(defn- construct-instant
  "Build the native instant for validated components by reading a canonical
  RFC3339 string through the default #inst reader. *data-readers* is rebound
  empty so a user 'inst binding (e.g. read-instant-date itself) cannot
  recurse."
  [years months days hours minutes seconds nanoseconds
   offset-sign offset-hours offset-minutes]
  (binding [*data-readers* {}]
    (read-string
     (str "#inst \""
          (format "%04d-%02d-%02dT%02d:%02d:%02d.%09d%s%02d:%02d"
                  years months days hours minutes seconds nanoseconds
                  (if (neg? offset-sign) "-" "+")
                  offset-hours offset-minutes)
          "\""))))

(defn- construct-calendar
  "Upstream: construct a java.util.Calendar, preserving the timezone
offset, but truncating the subsecond fraction to milliseconds.
Port: the same point in time as a native instant (the reader folds the
offset into UTC)."
  [years months days hours minutes seconds nanoseconds
   offset-sign offset-hours offset-minutes]
  (construct-instant years months days hours minutes seconds nanoseconds
                     offset-sign offset-hours offset-minutes))

(defn- construct-date
  "Upstream: construct a java.util.Date, which expresses the original
instant as milliseconds since the epoch, UTC.
Port: the native instant, which is exactly that."
  [years months days hours minutes seconds nanoseconds
   offset-sign offset-hours offset-minutes]
  (construct-instant years months days hours minutes seconds nanoseconds
                     offset-sign offset-hours offset-minutes))

(defn- construct-timestamp
  "Upstream: construct a java.sql.Timestamp, which has nanosecond precision.
Port: the native instant (millis precision; sub-millis nanos truncate,
exactly like #inst literals)."
  [years months days hours minutes seconds nanoseconds
   offset-sign offset-hours offset-minutes]
  (construct-instant years months days hours minutes seconds nanoseconds
                     offset-sign offset-hours offset-minutes))

(defn read-instant-date
  "To read an instant as a java.util.Date, bind *data-readers* to a map with
this var as the value for the 'inst key. The timezone offset will be used
to convert into UTC.
Port: answers the native instant value."
  [^CharSequence cs]
  (parse-timestamp (validated construct-date) cs))

(defn read-instant-calendar
  "To read an instant as a java.util.Calendar, bind *data-readers* to a map with
this var as the value for the 'inst key.  Calendar preserves the timezone
offset.
Port: answers the native instant value (same point in time; the offset
representation narrows away)."
  [^CharSequence cs]
  (parse-timestamp (validated construct-calendar) cs))

(defn read-instant-timestamp
  "To read an instant as a java.sql.Timestamp, bind *data-readers* to a
map with this var as the value for the 'inst key. Timestamp preserves
fractional seconds with nanosecond precision. The timezone offset will
be used to convert into UTC.
Port: answers the native instant value (millis precision)."
  [^CharSequence cs]
  (parse-timestamp (validated construct-timestamp) cs))
