;; S5D differential corpus 20: *print-dup* atom witness (converged zero-oos S8 #51).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s9-plan.md S9B (reader-contract §24 item 11), s5d-contract,
;; concurrency.rs print_dup_atom_throws_per_flag.
;; CONVERGED: the JVM prints the atom under *print-dup* false and
;; THROWS IllegalArgumentException under true (no print-dup method
;; for Atom); the port matches both sides since zero-oos S8 (#51).
;; Golden = JVM truth. StartsWith booleans keep the 0x address out
;; of the golden.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(p ":dup-atom-false" #(binding [*print-dup* false] (.startsWith ^String (pr-str (atom 1)) "#object[clojure.lang.Atom")))
(p ":dup-atom-true" #(binding [*print-dup* true] (.startsWith ^String (pr-str (atom 1)) "#object[clojure.lang.Atom")))
(println "S5D-20 DONE")
