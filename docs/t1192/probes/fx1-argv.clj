;; T1192 FX1 argv-shape probe (oracle: stock Clojure 1.13.0-alpha9).
;; Run twice: with no args and with two args.
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.13.0-alpha9 jars> clojure.main fx1-argv.clj [args...]
(println "argv:" (pr-str *command-line-args*) "seq?" (pr-str (seq? *command-line-args*)))
