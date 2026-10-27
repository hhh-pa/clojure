;; S9 scalar-kinds oracle probes: URI (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/uri.clj > docs/zero-oos-s9-scalarkinds/probes/uri.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- A: uri? battery ----
(throwof "r-p-uri" '(uri? (java.net.URI. "http://example.com/x")))
(throwof "r-p-nil" '(uri? nil))
(throwof "r-p-str" '(uri? "http://example.com/x"))
(throwof "r-p-int" '(uri? 42))
(throwof "r-p-url" '(uri? (java.net.URL. "http://example.com/x")))
(throwof "r-p-uuid" '(uri? (random-uuid)))
(throwof "r-p-inst" '(uri? #inst "2024-01-01"))
(throwof "r-p-vec" '(uri? []))
;; ---- B: equality semantics (case rules per component) ----
(throwof "r-eq-same" '(= (java.net.URI. "http://example.com/x") (java.net.URI. "http://example.com/x")))
(throwof "r-eq-scheme" '(= (java.net.URI. "HTTP://example.com/x") (java.net.URI. "http://example.com/x")))
(throwof "r-eq-host" '(= (java.net.URI. "http://EXAMPLE.com/x") (java.net.URI. "http://example.com/x")))
(throwof "r-eq-path" '(= (java.net.URI. "http://example.com/X") (java.net.URI. "http://example.com/x")))
(throwof "r-eq-query" '(= (java.net.URI. "http://example.com/x?A=1") (java.net.URI. "http://example.com/x?a=1")))
(throwof "r-eq-frag" '(= (java.net.URI. "http://example.com/x#F") (java.net.URI. "http://example.com/x#f")))
(throwof "r-eq-user" '(= (java.net.URI. "http://U@example.com/x") (java.net.URI. "http://u@example.com/x")))
(throwof "r-eq-port" '(= (java.net.URI. "http://example.com:80/x") (java.net.URI. "http://example.com/x")))
(throwof "r-eq-portsame" '(= (java.net.URI. "http://example.com:80/x") (java.net.URI. "http://example.com:80/x")))
(throwof "r-eq-portdiff" '(= (java.net.URI. "http://example.com:81/x") (java.net.URI. "http://example.com:80/x")))
(throwof "r-eq-slash" '(= (java.net.URI. "http://example.com/x") (java.net.URI. "http://example.com/x/")))
(throwof "r-eq-opaque" '(= (java.net.URI. "mailto:a@b.c") (java.net.URI. "mailto:a@b.c")))
(throwof "r-eq-opaque-case" '(= (java.net.URI. "MAILTO:a@b.c") (java.net.URI. "mailto:a@b.c")))
(throwof "r-eq-ssp-case" '(= (java.net.URI. "mailto:A@b.c") (java.net.URI. "mailto:a@b.c")))
(throwof "r-eq-rel" '(= (java.net.URI. "/a/b") (java.net.URI. "/a/b")))
(throwof "r-eq-rel-case" '(= (java.net.URI. "/A/b") (java.net.URI. "/a/b")))
(throwof "r-eq-str" '(= (java.net.URI. "http://example.com/x") "http://example.com/x"))
(throwof "r-eq-nil" '(= (java.net.URI. "http://example.com/x") nil))
(throwof "r-eq-ipv6" '(= (java.net.URI. "http://[::1]/x") (java.net.URI. "http://[::1]/x")))
(throwof "r-eq-reg" '(= (java.net.URI. "http://EXAMPLE/x") (java.net.URI. "http://example/x")))
;; ---- C: hashing (bit-exact pins) ----
(throwof "r-hash-a" '(hash (java.net.URI. "http://example.com/x")))
(throwof "r-hash-b" '(hash (java.net.URI. "HTTP://EXAMPLE.com/x")))
(throwof "r-hash-c" '(hash (java.net.URI. "mailto:a@b.c")))
(throwof "r-hash-d" '(hash (java.net.URI. "/a/b")))
(throwof "r-hash-e" '(hash (java.net.URI. "http://u@example.com:8080/x?y=1#f")))
(throwof "r-hash-eq" '(= (hash (java.net.URI. "HTTP://EXAMPLE.com/x")) (hash (java.net.URI. "http://example.com/x"))))
(throwof "r-hash-map" '(get {(java.net.URI. "http://example.com/x") :hit} (java.net.URI. "HTTP://EXAMPLE.com/x")))
;; ---- D: ordering ----
(throwof "r-cmp-eq" '(compare (java.net.URI. "http://example.com/x") (java.net.URI. "HTTP://EXAMPLE.com/x")))
(throwof "r-cmp-scheme" '(compare (java.net.URI. "ftp://example.com/x") (java.net.URI. "http://example.com/x")))
(throwof "r-cmp-path" '(compare (java.net.URI. "http://example.com/a") (java.net.URI. "http://example.com/b")))
(throwof "r-cmp-query" '(compare (java.net.URI. "http://example.com/x?a=1") (java.net.URI. "http://example.com/x?a=2")))
(throwof "r-cmp-frag" '(compare (java.net.URI. "http://example.com/x#f1") (java.net.URI. "http://example.com/x#f2")))
(throwof "r-cmp-frag-vs-nofrag" '(compare (java.net.URI. "http://example.com/x") (java.net.URI. "http://example.com/x#f")))
(throwof "r-cmp-port" '(compare (java.net.URI. "http://example.com:81/x") (java.net.URI. "http://example.com:80/x")))
(throwof "r-cmp-port-vs-noport" '(compare (java.net.URI. "http://example.com/x") (java.net.URI. "http://example.com:80/x")))
(throwof "r-cmp-host" '(compare (java.net.URI. "http://a.example/x") (java.net.URI. "http://b.example/x")))
(throwof "r-cmp-opaque" '(compare (java.net.URI. "mailto:a@b.c") (java.net.URI. "mailto:b@b.c")))
(throwof "r-cmp-opaque-vs-hier" '(compare (java.net.URI. "mailto:a@b.c") (java.net.URI. "http://example.com/x")))
(throwof "r-cmp-rel" '(compare (java.net.URI. "/a") (java.net.URI. "/b")))
(throwof "r-cmp-str" '(compare (java.net.URI. "http://example.com/x") "s"))
;; ---- E: printing + class ----
(throwof "r-pr" '(pr-str (java.net.URI. "http://example.com/x")))
(throwof "r-print" '(print-str (java.net.URI. "http://example.com/x")))
(throwof "r-str" '(str (java.net.URI. "http://example.com/x")))
(throwof "r-class" '(class (java.net.URI. "http://example.com/x")))
(throwof "r-readback" '(read-string "#uri \"http://example.com/x\""))
