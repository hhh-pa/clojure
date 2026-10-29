;; S9 scalar-kinds oracle probes: URI components (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/uriparts.clj > docs/zero-oos-s9-scalarkinds/probes/uriparts.out
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
(doseq [[label s] [["c-emptyauth" "http:///x"]
                   ["c-noauth" "http:/x"]
                   ["c-emptyport" "http://h:/x"]
                   ["c-negport" "http://h:-1/x"]
                   ["c-badport" "http://h:8x/x"]
                   ["c-hugeport" "http://h:99999999999/x"]
                   ["c-ipv6" "http://[::1]/x"]
                   ["c-upper" "HTTP://H/X"]
                   ["c-opaquefrag" "mailto:a@b.c#f"]
                   ["c-empty" ""]
                   ["c-query" "?q"]
                   ["c-frag" "#f"]
                   ["c-nonascii" "http://h/é"]
                   ["c-relscheme" "http:a"]
                   ["c-schemechars" "s+1.x-2:y"]
                   ["c-user" "http://u@h/x"]
                   ["c-dots" "https://h/a/b/../c"]
                   ["c-escape" "http://h/a%2Fb"]
                   ["c-scope" "http://[fe80::1%eth0]/x"]
                   ["c-regcolon" "http://a:b:c/x"]]]
  (throwof label `(parts ~s)))
(throwof "c-eq-negport" '(= (java.net.URI. "http://h:-1/x") (java.net.URI. "http://h/x")))
(throwof "c-eq-emptyport" '(= (java.net.URI. "http://h:/x") (java.net.URI. "http://h/x")))
(throwof "c-eq-badport-self" '(= (java.net.URI. "http://h:8x/x") (java.net.URI. "http://h:8x/x")))
(throwof "c-hash-negport" '(hash (java.net.URI. "http://h:-1/x")))
(throwof "c-hash-emptyport" '(hash (java.net.URI. "http://h:/x")))
(throwof "c-hash-badport" '(hash (java.net.URI. "http://h:8x/x")))
(throwof "c-del" '(java.net.URI. "http://h/a\u007fb"))
(throwof "c-c1" '(java.net.URI. "http://h/a\u0080b"))
(throwof "c-nbsp" '(java.net.URI. "http://h/a\u00a0b"))
