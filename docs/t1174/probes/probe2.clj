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
