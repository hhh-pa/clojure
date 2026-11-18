;; A compiled program drops top-level defs nothing reads whose inits are
;; pure and total (the snippets' unused digit tables, separator sets,
;; max-long): nothing observable changes (#271). An unread def with an
;; effect still runs it; read defs keep their values.
(def digit-values (zipmap "0123456789" (range)))
(def separators #{\space \tab \newline \return \formfeed \u000B})
(def min-long (bit-shift-left 1 63))
(def max-long (bit-not min-long))
(def noisy (do (println "the unread def's effect runs") 1))
(def used-digits (zipmap "01" (range)))
(println (used-digits \1) (count (keys used-digits)))
(defn parse-token [tok] (when (re-matches #"[+-]?[0-9]+" tok) (parse-long tok)))
(println (mapv parse-token ["12" "x" "-3"]))
