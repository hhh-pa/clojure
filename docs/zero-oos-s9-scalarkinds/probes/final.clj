;; S9 scalar-kinds oracle probes: final edges (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/final.clj > docs/zero-oos-s9-scalarkinds/probes/final.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn parts [s]
  (let [u (java.net.URI. s)]
    [(.getScheme u) (.getAuthority u) (.getUserInfo u) (.getHost u) (.getPort u)
     (.getPath u) (.getQuery u) (.getFragment u) (.isOpaque u) (.getSchemeSpecificPart u)]))
(throwof "f-auth-space" '(java.net.URI. "http://h x/y"))
(throwof "f-ipv6-nfe" '(java.net.URI. "http://[::12345678901234567890]/x"))
(throwof "f-empty-user" '(parts "http://@h/x"))
(throwof "f-empty-user-eq" '(= (java.net.URI. "http://@h/x") (java.net.URI. "http://h/x")))
(throwof "f-empty-frag-eq" '(= (java.net.URI. "http://h/x#") (java.net.URI. "http://h/x")))
(throwof "f-empty-query-eq" '(= (java.net.URI. "http://h/x?") (java.net.URI. "http://h/x")))
(throwof "f-empty-scope" '(java.net.URI. "http://[::1%]/x"))
(throwof "f-y0-eq" '(= (inst-ms #inst "0000-06-15") (inst-ms #inst "0001-06-15")))
(throwof "f-y0-leap" '(inst-ms (read-string "#inst \"0000-02-29\"")))
(throwof "f-y0-jan" '(inst-ms #inst "0000-01-01"))
(throwof "f-y0-pr-leap" '(pr-str (read-string "#inst \"0000-02-29\"")))
(throwof "f-y0-date-ctor" '(pr-str (java.util.Date. -62153049600000)))
(throwof "f-y0-str-ctor" '(str (java.util.Date. -62153049600000)))
(throwof "f-y0-jun15-y1" '(inst-ms #inst "0001-06-15"))
(doseq [[label s] [["f-sw-jan02" "0000-01-02"]
                   ["f-sw-feb01" "0000-02-01"]
                   ["f-sw-feb28" "0000-02-28"]
                   ["f-sw-mar01" "0000-03-01"]
                   ["f-sw-mar02" "0000-03-02"]
                   ["f-sw-dec31" "0000-12-31"]
                   ["f-sw-y1feb28" "0001-02-28"]
                   ["f-sw-y1mar01" "0001-03-01"]]]
  (throwof label `(inst-ms (read-string ~(str "#inst \"" s "\"")))))
