(require '[clojure.string :as str])
(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn show-err [e]
  (let [r (root-cause e)
        xd (try (ex-data e) (catch Throwable _ :no-ex-data))]
    (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r)) "|" (pr-str xd)])))
(def ALLOW {:read-cond :allow})
(defn try-read [label s]
  (print (str ">>> " label " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr ALLOW)
            forms (loop [acc [o1]]
                    (let [nxt (clojure.lang.LispReader/read rdr (assoc ALLOW :eof EOFMARK))]
                      (if (identical? nxt EOFMARK) acc (recur (conj acc nxt)))))]
        (prn (into [:ok] (map pr-str) forms))))
    (catch Throwable e (show-err e))))
(defn t [s] (try-read (pr-str s) s))
(doseq [s ["#{#\"a\" #\"a\"}" "{0.0 :a -0.0 :b}" "#{'a 'a}" "#{:a :a}" "#{(1) (1)}" "#{[1] (1)}" "{{} {}}"
           "#(x#)" "`(x# x#)" "#(x# %)" "#é 1" "~;c\nx" "^:a ;c\n'x" "\"a\r\nb" "2r100000000000000000000000000000000000000000000000000000000000000"
           "2r111111111111111111111111111111111111111111111111111111111111111"
           "-2r100000000000000000000000000000000000000000000000000000000000000" "-0x1" "/.5" "a/b/"
           ":a/" "1:2" ":1" ":1a" "a1:b2" "#:a{:b 1 :_/c 2}" "#:a{1 2 \"s\" 3}" "#::{1 2}"
           "#?(:cljs 1 :default 2)" "#?((quote :clj) 1)" "##" "#_##Inf x" "#'##Inf" "'##NaN"
           "#foo/bar 1" "#foo.bar/baz 1" "#foo/bar.baz 1" "0x10000000000000000" "#?@(:clj (:b 2) :extra)"
           "[1 #?@(:clj (2) :cljs (3)) 4]"]]
  (t s))
(println ">>> PRINTMETA4")
(prn [(binding [*print-meta* true]
        (mapv (fn [s] (try (pr-str (clojure.lang.LispReader/read
                                    (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s)) ALLOW))
                           (catch Throwable e (str "ERR: " (.getMessage ^Throwable (root-cause e))))))
              ["^:k 'x" "^\"d\" 'x" "^[String] 'x" "^:a ^:b 'x" "^{:tag String :a 1} 'x" "^:k [1]" "^:k :kw"
               "^{:a 1} 'x" "#(^:k %)" "(a)" "^String [x]"]))])
(println ">>> QCHARCODES")
(prn [(mapv int (pr-str (clojure.lang.LispReader/read
                         (clojure.lang.LineNumberingPushbackReader.
                          (java.io.StringReader. "#\"\\Qa\\\"b\\E\"")) ALLOW)))])
(doseq [cp [0x2002 0x2003 0x2004]]
  (println "ISWS" (format "%04X" cp) (str (Character/isWhitespace (char cp)))))
(println ">>> PROBE4-DONE")
