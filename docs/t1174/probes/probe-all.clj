(require '[clojure.string :as str])
(println "NS:" (str *ns*))
(require '[clojure.string :as str])
(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn pr-val [v]
  (cond (instance? Class v) (str "#<Class " (.getName ^Class v) ">")
        :else (pr-str v)))
(defn show-err [e]
  (let [r (root-cause e)
        xd (try (ex-data e) (catch Throwable _ :no-ex-data))]
    (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r)) "|" (pr-str xd)])))
(def ALLOW {:read-cond :allow})
(def FEAT-AB {:read-cond :allow :features #{:a :b}})
(def DENY {})
(def PRESERVE {:read-cond :preserve})
(defn try-read
  ([label s] (try-read label s ALLOW false))
  ([label s opts] (try-read label s opts false))
  ([label s opts readeval]
   (print (str ">>> " label " "))
   (flush)
   (try
     (binding [*read-eval* readeval]
       (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
             o1 (clojure.lang.LispReader/read rdr opts)
             forms (loop [acc [o1]]
                     (let [nxt (clojure.lang.LispReader/read rdr (assoc opts :eof EOFMARK))]
                       (if (identical? nxt EOFMARK) acc (recur (conj acc nxt)))))]
         (prn (into [:ok] (map pr-val) forms))))
     (catch Throwable e (show-err e)))))
(defn t [s & args] (apply try-read (pr-str s) s args))
(defn try-meta [label s]
  (print (str ">>> META " label " "))
  (flush)
  (try (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
             o (clojure.lang.LispReader/read rdr ALLOW)]
         (prn [:ok (pr-val o) :meta (pr-str (meta o))]))
       (catch Throwable e (show-err e))))
(defn tm [s] (try-meta (pr-str s) s))
;; numbers
(doseq [s ["08" "09" "00" "0" "00N" "08N" "1N" "1M" "3.14M" "1/2" "1/0" "2/4" "-3/9" "0/5" "5/0"
           "99999999999999999999/3" "2r2" "37r10" "1r1" "36rzz" "0xFF" "-0x10" "+42" "8r17" "2r101"
           "10r99" "5." ".5" "1a" "1e" "1e+" "1N2" "12345678901234567890123" "0x" "0r1" "9r8" "10r"
           "2R10" "0X1F" "017" "018" "007" "0N" "0M" "00M" "1e3M" "200r10" "0.0" "5.e3" "5.e" "0xG" "1r"
           "-0.0" "1e5" "1E5" "+" "-" "--" "1e309" "1e-324" "0.30000000000000004" "1.7976931348623157e308"
           "5e-324" "2.5e-7" "100000.0" "1.0E7" "-.5" "+.5" "5a" "1/2x" "2r101" "36rZZ"]]
  (t s))
;; chars
(doseq [s ["\\o" "\\o12" "\\o1234" "\\o8" "\\u0041" "\\uD800" "\\u" "\\u12" "\\u0041x" "\\ab"
           "\\newline" "\\9" "\\" "\\o0" "\\ " "\\a" "\\u004" "\\o123"]]
  (t s))
;; strings
(doseq [s ["\"\\z\"" "\"\\uZ\"" "\"\\u12\"" "\"\\1\"" "\"\\18\"" "\"\\400\"" "\"\\0\"" "\"a\nb\""
           "\"abc" "\"\\ud800\"" "\"\\ud800\\udc00\"" "\"\\u0041\"" "\"a\\tb\"" "\"x\""]]
  (t s))
(t "\"a\\u")
(t "\"a\\")
;; symbols/tokens
(doseq [s ["a'b" "a#b" "%" "%&" "%1" "a/b/c" "foo:/bar" ":a::b" "::foo" "::str/foo" "::clojure.string/foo"
           "/" "..." "+a" "-a" "//" "a/" "/a" "/1" "a/1" "ab/12" "::" ":::" ":" "a:" ":a:" "a::b" "." "r" "e5"]]
  (t s))
;; ##
(doseq [s ["##Inf" "## Inf" "##foo" "##123" "##\"x\"" "##(a)" "##nil" "##-Inf" "##NaN"]]
  (t s))
;; dispatch basics
(doseq [s ["#" "#a 1" "#123" "#<x" "#!sheb\n1" "#!sheb" "#_a b" "#_1" "#'x" "#inst \"x\""
           "#inst \"2020-01-01\"" "#uuid \"nope\"" "#uuid \"f81d4fae-7dec-11d0-a765-00a0c91e6bf6\""
           "#foo.bar [1]" "#foo.bar 123" "#\"a+\"" "#\"((\"" "#\"\n\"" "#\"(?i)x\"" "#\"a\nb\""]]
  (t s))
(t "#=(+ 1 2)" ALLOW false)
(t "#=(+ 1 2)" ALLOW true)
(t "#=String" ALLOW true)
;; conditionals
(doseq [s ["#?a" "#?(:else 1)" "#?(:default 1)" "#?(:clj 1 :cljs 2)" "#?(:cljs 2 :clj 1)" "#?(:cljs 2)"
           "#?(1 2)" "#?([1] 1)" "#?({:a 1} 1)" "#?(#{1} 1)" "#?((1) 1)" "#?(\"s\" 1)" "#?(nil 1)"
           "#?(\\c 1)" "#?(1.5 1)" "#?(true 1)" "#?@(:clj (1 2))" "[1 #?@(:clj (2 3))]" "[1 #?@(:clj [2])]"
           "#?(:clj)" "#?()" "#?(:a)" "#? (:clj 1)" "#?(:clj 1 :dangling)" "#?" "#?@" "[1 #?@(:cljs (2)) 3]"]]
  (t s))
(t "#?a" DENY)
(t "#?(:a 1 :b)" FEAT-AB)
(t "#?(:a 1)" FEAT-AB)
(t "[1 #?@(:a (2 3)) 4]" FEAT-AB)
(t "#?@(:clj)" ALLOW)
(t "[1 #?@(:clj) 2]" ALLOW)
(t "#?(:a 1)" PRESERVE)
(t "#?@(:a (1))" PRESERVE)
(t "#inst \"2020-01-01\"" PRESERVE)
;; ns maps
(doseq [s ["#:a{:b 1}" "#::{:a 1}" "#::str{:b 1}" "#::nope{:b 1}" "#:a 1" "#:  {}" "#:: {:a 1}"
           "#:a{:b 1 :c}" "#:1{}" "#:a/b{}" "#::{:a 1 :_/b 2 :c/d 3 'e 4}" "#:a{}" "#:" "#::" "#:a{:b"]]
  (t s))
;; meta
(doseq [s ["^:k 'x" "^String 'x" "^\"d\" 'x" "^[String] 'x" "^{:a 1} 'x" "^:a ^:b 'x" "^:a 1" "^1 'x" "(a)" "^String [x]"]]
  (tm s))
;; fn
(doseq [s ["#(f %)" "#(f %3 %)" "#(%&)" "#()" "#( %& %&)" "#(%2)" "#(%0)" "#(#(%))" "#(f"]]
  (t s))
;; syntax quote
(doseq [s ["`x" "`a/b" "`x#" "`.m" "`C." "`def" "`fn" "`(a ~b ~@c)" "`[a ~b]" "`{:a ~b}" "`#{a}" "`()"
           "`~x" "`~@x" "`^:k x" "~x" "~@x" "''x" "@x" "@!x" "`" "'" "~" "~@" "@" "^:a"]]
  (t s))
;; sets/maps/delims/eof
(doseq [s ["#{1 1}" "#{1 2}" "{:a}" "{:a 1 :a 2}" ")" "]" "}" "(" "[" "{" "\"" "\"abc" "(a b" "(a\nb"
           "[1 2" "#{1" "#(f" "`" "#_1" "" "   " ";" "#_" "#_(" "#\"" "#\"abc" "#_ #_ 1 2 3" "#_a"
           "(a 2r2)" "(a :b" "'a"]]
  (t s))
;; whitespace behavior
(doseq [cp [0x09 0x0B 0x0C 0x1C 0x85 0xA0 0x1680 0x2000 0x2007 0x200A 0x2028 0x202F 0x205F 0x3000 0x0020 0x002C]]
  (t (str "WS-" (format "%04X" cp)) (str "1" (char cp) "2")))
(doseq [cp [0x09 0x0B 0x0C 0x1C 0x1D 0x85 0xA0 0x1680 0x2000 0x2007 0x200A 0x2028 0x202F 0x205F 0x3000]]
  (println "ISWS" (format "%04X" cp) (str (Character/isWhitespace (char cp)))))
;; print-meta + misc
(println ">>> PRINTMETA")
(prn [(binding [*print-meta* true]
        [(pr-str (clojure.lang.LispReader/read
                  (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "(a)")) ALLOW))
         (pr-str (clojure.lang.LispReader/read
                  (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "^String [x]")) ALLOW))])])
(println ">>> SURR-COUNT")
(prn [(count (clojure.lang.LispReader/read
              (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\"\\ud800\"")) ALLOW))
       (count (clojure.lang.LispReader/read
               (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\"\\ud800\\udc00\"")) ALLOW))])
(println ">>> DONE")
(println ">>> PROBE2")
(doseq [cp [0x1E 0x1F 0x2001 0x2005 0x2006 0x2008 0x2009 0x2029 0xFEFF 0x000A 0x000D]]
  (println "ISWS" (format "%04X" cp) (str (Character/isWhitespace (char cp)))))
(doseq [cp [0x0009 0x000B 0x000C 0x001C 0x001E 0x0085 0x00A0 0x1680 0x2000 0x2005 0x2007 0x200A 0x2028 0x2029 0x202F 0x205F 0x3000 0x0020 0x002C 0xFEFF]]
  (try-read (str "WS-" (format "%04X" cp)) (str "1" (char cp) "2")))
(doseq [s ["#?([\"s\" \\newline] 1)" "#?(\\newline 1)" "{\"a\" 1 \"a\" 2}" "#:a{:b 1 :b 2}" "{1 :a 1.0 :b}"
           "#{1 1.0}" "[1 #?@(:clj {:a 1})]" "[1 #?@(:clj #{2})]" "[1 #?@(:clj 5)]" "`true" "`nil" "`#\"a\""
           "`String" "`1" "`:k" "`\\c" "`\"s\""]]
  (t s))
(t "#?(:a #inst \"2020-01-01\")" PRESERVE)
(println ">>> NSMAP-PRINT")
(prn [(pr-str {(quote a/b) 1 (quote a/c) 2})
      (pr-str {:a/b 1 (quote a/c) 2})
      (pr-str {(quote a/b) 1 (quote d/e) 2})
      (pr-str {:a/b 1 :a/b 2})])
(println ">>> PRINTMETA2")
(prn [(binding [*print-meta* true]
        (mapv (fn [s] (pr-str (clojure.lang.LispReader/read
                               (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s)) ALLOW)))
              ["^:k 'x" "^\"d\" 'x" "^[String] 'x" "^:a ^:b 'x" "^{:tag String :a 1} 'x" "^:k [1]" "^:k :kw" "^:k \"s\""]))])
(println ">>> CHARCODES")
(prn [(mapv int (pr-str (clojure.lang.LispReader/read
                         (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "#\"a\nb\"")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
                          (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "#\"a\\\"b\"")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
                          (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "#\"a\\\\b\"")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
                          (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "#\"a\tb\"")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
                          (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\"a\tb\"")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
                          (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\"a\rb\"")) ALLOW)))])
(println ">>> PROBE2-DONE")
