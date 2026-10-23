(def ^:dynamic *dyn* 0)
(println "alter-meta-ret" (pr-str (try (alter-meta! #'*dyn* (fn [_] 5)) (catch Exception ex (str (class ex) "::" (.getMessage ex))))))
(println "reset-meta-nonmap" (pr-str (try (reset-meta! #'*dyn* 5) (catch Exception ex (str (class ex) "::" (.getMessage ex))))))
