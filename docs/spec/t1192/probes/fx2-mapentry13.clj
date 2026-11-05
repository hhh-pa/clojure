(def ^:dynamic *dyn* 0)
(println "push-vec-entry" (pr-str (try (do (push-thread-bindings [(first {#'*dyn* 1})]) (pop-thread-bindings) :ok) (catch Exception ex (str (class ex) "::" (.getMessage ex))))))
(println "alter-meta-nonmap" (pr-str (try (alter-meta! #'*dyn* 5) (catch Exception ex (str (class ex) "::" (.getMessage ex))))))
