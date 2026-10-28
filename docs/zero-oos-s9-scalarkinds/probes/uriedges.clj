;; S9 scalar-kinds oracle probes: URI edges (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/uriedges.clj > docs/zero-oos-s9-scalarkinds/probes/uriedges.out
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
(doseq [[label s] [["g-empty-scheme" ":"]
                   ["g-empty-scheme-x" ":x"]
                   ["g-digit-rel" "1x"]
                   ["g-space-scheme" "a b:c"]
                   ["g-empty-ssp" "http:"]
                   ["g-empty-ssp-frag" "http:#f"]
                   ["g-empty-opaque" "mailto:"]
                   ["g-empty-auth-eof" "http://"]
                   ["g-host-only" "http://h"]
                   ["g-host-query" "http://h?q"]
                   ["g-host-frag" "http://h#"]
                   ["g-empty-frag" "http://h/x#"]
                   ["g-empty-query" "http://h/x?"]
                   ["g-rel-auth" "//h/x"]
                   ["g-rel-emptyauth" "///x"]
                   ["g-opaque-colons" "a:b:c"]
                   ["g-empty-host-user" "http://u@/x"]
                   ["g-bad-ipv6port" "http://[::1]:8x/x"]
                   ["g-zeropad-port" "http://h:0080/x"]
                   ["g-multi-at" "http://a@b@c/x"]
                   ["g-bad-auth-escape" "http://a%zz/x"]
                   ["g-trail-pct" "http://h/a%"]
                   ["g-bracket-rel" "[::1]/x"]
                   ["g-ipv6-full" "http://[2001:db8::1]/x"]
                   ["g-ipv6-mapped" "http://[::ffff:1.2.3.4]/x"]
                   ["g-ipv6-double" "http://[1::2::3]/x"]
                   ["g-ipv6-longgroup" "http://[12345::]/x"]
                   ["g-underscore" "http://h_ost/x"]
                   ["g-pct-host" "http://h%20ost/x"]]]
  (throwof label `(parts ~s)))
