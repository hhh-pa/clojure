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
(println ">>> HASHES" (pr-str [(hash 0.0) (hash -0.0) (hash ##NaN) (hash [1]) (hash '(1)) (hash 1) (hash 1.0) (hash "a")]))
(doseq [s ["#{(1) [1]}" "{(quote (0.0)) :a [-0.0] :b}" "{(quote (0.0)) :a (quote (-0.0)) :b}"
           "{[0.0] :a [-0.0] :b}" "{{:x 0.0} :a {:x -0.0} :b}" "{#{0.0} :a #{-0.0} :b}"
           "#{0.0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 -0.0}"
           "#{##NaN 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 ##NaN}"
           "#{[1] 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 (1)}"
           "{0.0 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10 11 11 12 12 13 13 14 14 15 15 16 16 17 17 18 18 19 19 20 20 -0.0}"]]
  (t s))
(t "{#?(:a #inst \"2020-01-01\") 1 #?(:a #inst \"2020-01-01\") 2}" PRESERVE)
(t "#{#?(:a #inst \"2020-01-01\") #?(:a #inst \"2020-01-01\")}" PRESERVE)
(println ">>> PROBE7-DONE")
