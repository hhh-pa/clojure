(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn show-err [e]
  (let [r (root-cause e)
        xd (try (ex-data e) (catch Throwable _ :no-ex-data))]
    (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r)) "|" (pr-str xd)])))
(def ALLOW {:read-cond :allow})
(def PRESERVE {:read-cond :preserve})
(defn t ([s] (t s ALLOW)) ([s opts]
  (print (str ">>> " (pr-str s) " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr opts)
            forms (loop [acc [o1]]
                    (let [nxt (clojure.lang.LispReader/read rdr (assoc opts :eof EOFMARK))]
                      (if (identical? nxt EOFMARK) acc (recur (conj acc nxt)))))]
        (prn (into [:ok] (map pr-str) forms))))
    (catch Throwable e (show-err e)))))
(t (str "{0.0 0 " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 1 19))) " -0.0 99}"))
(t (str "{1 1 2 2 0.0 0 " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 3 19))) " -0.0 99}"))
(t "#?(:a {#inst [0.0] 1 #inst [-0.0] 2})" PRESERVE)
(t "{{:a 1} {:a 1}}")
(t "{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9}")
(t "{1 2 3}")
(println ">>> PROBE9-DONE")
