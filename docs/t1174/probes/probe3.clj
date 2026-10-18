(require '[clojure.string :as str])
(println "NS3:" (str *ns*))
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
(defn pos-probe [label s ops]
  (print (str ">>> POS " label " "))
  (flush)
  (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))]
    (prn (mapv (fn [op]
                 (if (= op :r)
                   (let [c (.read rdr)] [:r c (.getLineNumber rdr) (.getColumnNumber rdr)])
                   (do (.unread rdr (second op)) [:u (second op) (.getLineNumber rdr) (.getColumnNumber rdr)])))
               ops))))
(pos-probe "empty" "" [:r :r])
(pos-probe "sp" " " [:r :r :r])
(pos-probe "a" "a" [:r :r :r])
(pos-probe "nl" "\n" [:r :r :r])
(pos-probe "a-nl" "a\n" [:r :r :r :r])
(pos-probe "a-nl-b" "a\nb" [:r :r :r :r :r])
(pos-probe "crlf" "\r\n" [:r :r :r])
(pos-probe "cr" "\r" [:r :r :r])
(pos-probe "a-cr-b" "a\rb" [:r :r :r :r :r])
(pos-probe "a-crlf-b" "a\r\nb" [:r :r :r :r :r :r])
(pos-probe "unread-b" "ab" [:r :r [:u 98] :r :r :r])
(pos-probe "unread-nl" "a\nb" [:r :r [:u 10] :r :r :r :r])
(pos-probe "unread-a" "a" [:r [:u 97] :r :r :r])
(pos-probe "unread-eof-after" "ab" [:r :r :r [:u 98] :r :r])
(doseq [s ["foo:\n1" "foo: 1" "1a\n" "08\n2" "(a 2r2)\n99" "#?(##-Inf 1)" "#?(##NaN 1)" "#?(##Inf 1)"
           "#?(#\"a\" 1)" "#?({:b 2 :a 1} 1)" "{[1] :a [1] :b}" "{1 :a 1 :b}" "#{##NaN ##NaN}"
           "#{0.0 -0.0}" "{[1] :a (quote (1)) :b}" "{[1] :a [1.0] :b}" "#{1 1.0}" "#{0.5 0.5}"
           "#{nil nil}" "#{true true}" "{nil 1 nil 2}" "{{:a 1} :x {:a 1} :y}" "\"a\rb\"" "\"a\r\nb\""
           ";c\rd" "a\rb" "1\r2" "(a\rb)" "9223372036854775807" "9223372036854775808"
           "-9223372036854775808" "-9223372036854775809" "0xFFFFFFFFFFFFFFFF" "0x7FFFFFFFFFFFFFFF"
           "-0x8000000000000000" "0x8000000000000000" "+1/2" "-0/5" "007/2" "01.5" "00.5" "+2r10"
           "-2r10" "+0x10" "--5" "++5" "-5-" "+5-" "x#" "#_@x" "`##Inf" "1 a" ":a.b/c.d" ":a/b/c"
           "::a/b/c" "::nope/x" "~@(1)" "#'" "#{" "@{1}" "\"a\\\"b\"" "\"\\\\\"" "#?@(:cljs (1))"
           "[#?@(:cljs (1))]" "(1 #?@(:cljs (2)) 3)" "#{1 #?@(:clj (2))}" "{:a 1 #?@(:clj (:b 2))}"
           "{:a 1 #?@(:clj (:b))}" "`#:a{:b 1}" "`[~@xs]" "``x" "#(%1x)" "#(%00)" "%00" "#(a %1 %1)"
           "#(%)" "#(f %2 %&)" "1 #!c\n2" "1 #!c\r2" "\\," "10/4" "100/10" "-0" "-0N" "-0M" "5.5.5"
           "\\1234" "\\o7" "\\o00" "()0" "#_()" "()()" "((((((1))))))" "() " "[]" "#{}" "{}" "()"
           "#inst \"2020-01-01T05:06:07.000-00:00\"" "(defn f ^String [x] x)" "`#?(:clj a)" "^:a #'x"]]
  (t s))
(println ">>> CHARCODES3")
(prn [(mapv int (str (clojure.lang.LispReader/read
                      (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\\o")) ALLOW)))
       (mapv int (str (clojure.lang.LispReader/read
                       (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. "\\o0")) ALLOW)))
       (mapv int (pr-str (clojure.lang.LispReader/read
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
(println ">>> NSMAP-PRINT3")
(prn [(pr-str (hash-map 'a/b 1 'a/c 2))
      (pr-str {:a/b 1 'a/c 2})
      (pr-str (hash-map 'a/b 1 'd/e 2))
      (pr-str {:a/b 1})
      (pr-str {:a/b 1 :c/d 2})])
(println ">>> PRINTMETA3")
(prn [(binding [*print-meta* true]
        (mapv (fn [s] (pr-str (clojure.lang.LispReader/read
                               (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s)) ALLOW)))
              ["^:k 'x" "^\"d\" 'x" "^[String] 'x" "^:a ^:b 'x" "^{:tag String :a 1} 'x" "^:k [1]" "^:k :kw" "^:k \"s\""]))])
(doseq [cp [0x2002 0x2003 0x2004]]
  (println "ISWS" (format "%04X" cp) (str (Character/isWhitespace (char cp)))))
(println ">>> PROBE3-DONE")
