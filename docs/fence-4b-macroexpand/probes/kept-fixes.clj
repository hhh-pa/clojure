;; Fence 4b oracle probes: kept fixes (F3/F5/F7/F8b) + B3 rule.
;; Oracle: stock Clojure 1.13.0-alpha9. Run:
;;   java -cp <1.13.0-alpha9 jars> clojure.main kept-fixes.clj
;; Output: kept-fixes.out (committed). Each row shows the JVM output
;; the port pin mirrors (gensym counters and port-side paths differ).

(defn root-msg [t]
  (loop [t t]
    (if (.getCause ^Throwable t)
      (recur (.getCause ^Throwable t))
      (str (pr-str (type t)) " " (pr-str (.getMessage ^Throwable t))))))

(defn probe [label thunk]
  (println label)
  (prn (try (thunk)
                (catch Throwable t (str "THROW " (root-msg t))))))

(probe "letfn-list-fnspecs:"
  #(eval '(letfn ((f [x] x)) (f 1))))

(probe "letfn-map-fnspecs:"
  #(eval '(letfn {f [x]} 1)))

(probe "letfn-set-fnspecs:"
  #(eval '(letfn #{(f [x] x)} (f 1))))

(probe "wrd-map-odd-drop:"
  #(eval '(with-redefs {a 1} 1)))

(probe "wrd-map-paired:"
  #(eval '(with-redefs {a 1 b 2} 1)))

(probe "letfn*-nonfn-init:"
  #(eval '(letfn* [f 1] f)))

(probe "locking-nil:"
  #(eval '(locking nil 1)))

(println "--- monitor nil matrix (B3: observed split held->IMS / empty->NPE on Oracle 27+35, default + -Xint; port implements JVMS NPE-always) ---")

(probe "monitor-exit-nil-held:"
  #((fn [] (let [x (Object.)] (do (monitor-enter x) (monitor-exit nil))))))

(probe "monitor-exit-nillocal-held:"
  #((fn [] (let [x (Object.) n nil] (monitor-enter x) (monitor-exit n)))))

(probe "monitor-exit-callnil-held:"
  #((fn [] (let [x (Object.)] (monitor-enter x) (monitor-exit (identity nil))))))

(probe "monitor-exit-nil-empty:"
  #((fn [] (monitor-exit nil))))

(probe "monitor-exit-nillocal-empty:"
  #((fn [] (let [n nil] (monitor-exit n)))))

(probe "monitor-enter-nil:"
  #((fn [] (monitor-enter nil))))

(probe "monitor-exit-unheld:"
  #((fn [] (monitor-exit (Object.)))))
