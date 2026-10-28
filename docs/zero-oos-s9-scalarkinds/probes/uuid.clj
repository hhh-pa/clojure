;; S9 scalar-kinds oracle probes: UUID (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/uuid.clj > docs/zero-oos-s9-scalarkinds/probes/uuid.out
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
;; ---- A: parse-uuid valid shapes (canonicalization + lenient masking) ----
(throwof "u-parse-canon" '(str (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
(throwof "u-parse-upper" '(str (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
(throwof "u-parse-short1" '(str (parse-uuid "550e840-e29b-41d4-a716-446655440000")))
(throwof "u-parse-shortmid" '(str (parse-uuid "550e8400-e29b-41d-a716-446655440000")))
(throwof "u-parse-longshort" '(str (parse-uuid "123456789-123-1234-1234-123456789012")))
(throwof "u-parse-long1" '(str (parse-uuid "123456789abcdef0-e29b-41d4-a716-446655440000")))
(throwof "u-parse-long4" '(str (parse-uuid "550e8400-e29b-41d4-a7160-446655440000")))
(throwof "u-parse-long5" '(str (parse-uuid "550e8400-e29b-41d4-a716-1234567890123")))
(throwof "u-parse-zeros" '(str (parse-uuid "0-0-0-0-0")))
;; ---- B: parse-uuid failure shape (nil vs throw per input class) ----
(throwof "u-parse-nil" '(parse-uuid nil))
(throwof "u-parse-num" '(parse-uuid 42))
(throwof "u-parse-bool" '(parse-uuid true))
(throwof "u-parse-vec" '(parse-uuid [1]))
(throwof "u-parse-kw" '(parse-uuid :x))
(throwof "u-parse-bogus" '(parse-uuid "bogus"))
(throwof "u-parse-empty" '(parse-uuid ""))
(throwof "u-parse-braces" '(parse-uuid "{550e8400-e29b-41d4-a716-446655440000}"))
(throwof "u-parse-urn" '(parse-uuid "urn:uuid:550e8400-e29b-41d4-a716-446655440000"))
(throwof "u-parse-nonhex" '(parse-uuid "550e8400-e29b-41d4-a716-44665544000g"))
(throwof "u-parse-nodash" '(parse-uuid "550e8400e29b41d4a716446655440000"))
(throwof "u-parse-short" '(parse-uuid "550e8400-e29b-41d4-a716"))
(throwof "u-parse-emptypart" '(parse-uuid "550e8400--41d4-a716-446655440000"))
(throwof "u-parse-traildash" '(parse-uuid "550e8400-e29b-41d4-a716-446655440000-"))
(throwof "u-parse-dashes" '(parse-uuid "----"))
(throwof "u-parse-badidx" '(parse-uuid "g50e8400-e29b-41d4-a716-446655440000"))
(throwcause "u-parse-nonstr-cause" '(parse-uuid 42))
(throwcause "u-parse-nil-cause" '(parse-uuid nil))
;; ---- C: random-uuid (stable bits only — never the value) ----
(throwof "u-rand-len" '(count (str (random-uuid))))
(throwof "u-rand-ver" '(nth (str (random-uuid)) 14))
(throwof "u-rand-var" '(nth (str (random-uuid)) 19))
(throwof "u-rand-dash" '(map (fn [i] (nth (str (random-uuid)) i)) [8 13 18 23]))
(throwof "u-rand-pred" '(uuid? (random-uuid)))
(throwof "u-rand-roundtrip" '(= (random-uuid) (parse-uuid (str (random-uuid)))))
(throwof "u-rand-unique" '(= (random-uuid) (random-uuid)))
;; ---- D: uuid? battery ----
(throwof "u-p-uuid" '(uuid? (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
(throwof "u-p-nil" '(uuid? nil))
(throwof "u-p-str" '(uuid? "550e8400-e29b-41d4-a716-446655440000"))
(throwof "u-p-int" '(uuid? 42))
(throwof "u-p-vec" '(uuid? []))
(throwof "u-p-map" '(uuid? {}))
(throwof "u-p-kw" '(uuid? :x))
(throwof "u-p-sym" '(uuid? 'x))
(throwof "u-p-inst" '(uuid? #inst "2024-01-01"))
(throwof "u-p-uri" '(uuid? (java.net.URI. "http://example.com")))
(throwof "u-p-tagged" '(uuid? (tagged-literal 'uuid "550e8400-e29b-41d4-a716-446655440000")))
;; ---- E: equality + hashing (bit-exact pins) ----
(throwof "u-eq-case" '(= (parse-uuid "550E8400-E29B-41D4-A716-446655440000") (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
(throwof "u-eq-str" '(= (parse-uuid "550e8400-e29b-41d4-a716-446655440000") "550e8400-e29b-41d4-a716-446655440000"))
(throwof "u-eq-nil" '(= (parse-uuid "550e8400-e29b-41d4-a716-446655440000") nil))
(throwof "u-eq-ne" '(= (parse-uuid "550e8400-e29b-41d4-a716-446655440000") (parse-uuid "550e8400-e29b-41d4-a716-446655440001")))
(throwof "u-hash-a" '(hash (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
(throwof "u-hash-b" '(hash (parse-uuid "00000000-0000-0000-0000-000000000000")))
(throwof "u-hash-c" '(hash (parse-uuid "ffffffff-ffff-ffff-ffff-ffffffffffff")))
(throwof "u-hash-case" '(= (hash (parse-uuid "550E8400-E29B-41D4-A716-446655440000")) (hash (parse-uuid "550e8400-e29b-41d4-a716-446655440000"))))
(throwof "u-hash-map" '(get {(parse-uuid "550e8400-e29b-41d4-a716-446655440000") :hit} (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
;; ---- F: ordering (signed-long msb/lsb) ----
(throwof "u-cmp-eq" '(compare (parse-uuid "550e8400-e29b-41d4-a716-446655440000") (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
(throwof "u-cmp-lsb" '(compare (parse-uuid "00000000-0000-0000-0000-000000000001") (parse-uuid "00000000-0000-0000-0000-000000000002")))
(throwof "u-cmp-msb" '(compare (parse-uuid "00000001-0000-0000-0000-000000000000") (parse-uuid "00000000-ffff-ffff-ffff-ffffffffffff")))
(throwof "u-cmp-signed" '(compare (parse-uuid "ffffffff-ffff-ffff-ffff-ffffffffffff") (parse-uuid "00000000-0000-0000-0000-000000000000")))
(throwof "u-cmp-signed-lsb" '(compare (parse-uuid "00000000-0000-0000-ffff-ffffffffffff") (parse-uuid "00000000-0000-0000-0000-000000000000")))
(throwof "u-cmp-flip" '(compare (parse-uuid "00000000-0000-0000-0000-000000000002") (parse-uuid "00000000-0000-0000-0000-000000000001")))
(throwof "u-cmp-str" '(compare (parse-uuid "550e8400-e29b-41d4-a716-446655440000") "s"))
(throwof "u-cmp-inst" '(compare (parse-uuid "550e8400-e29b-41d4-a716-446655440000") #inst "2024-01-01"))
;; ---- G: printing ----
(throwof "u-pr" '(pr-str (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
(throwof "u-print" '(print-str (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
(throwof "u-str" '(str (parse-uuid "550E8400-E29B-41D4-A716-446655440000")))
(throwof "u-pr-roundtrip" '(= (parse-uuid "550e8400-e29b-41d4-a716-446655440000") (read-string (pr-str (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))))
(throwof "u-class" '(class (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
