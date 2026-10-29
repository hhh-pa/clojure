;; S9 scalar-kinds oracle probes: Inst/inst-ms (oracle: alpha10 jars, issue #52).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s9-scalarkinds/probes/inst.clj > docs/zero-oos-s9-scalarkinds/probes/inst.out
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
;; ---- A: millis vectors (every S6-valid shape + edges) ----
(throwof "i-ms-day" '(inst-ms #inst "2024-01-01"))
(throwof "i-ms-offset" '(inst-ms #inst "2024-01-01T10:00:00.000+02:00"))
(throwof "i-ms-nozone" '(inst-ms #inst "2024-01-01T00:00:00"))
(throwof "i-ms-ym" '(inst-ms #inst "2024-01"))
(throwof "i-ms-year" '(inst-ms #inst "2024"))
(throwof "i-ms-nosecs" '(inst-ms #inst "2024-01-01T00:00"))
(throwof "i-ms-zonez" '(inst-ms #inst "2024-01-01T00:00:00Z"))
(throwof "i-ms-frac7" '(inst-ms #inst "2024-01-01T00:00:00.1234567"))
(throwof "i-ms-frac1" '(inst-ms #inst "2024-01-01T00:00:00.1"))
(throwof "i-ms-frac-round" '(inst-ms #inst "2024-01-01T00:00:00.9999"))
(throwof "i-ms-leapsec" '(inst-ms #inst "2024-01-01T00:59:60"))
(throwof "i-ms-yearzone" '(inst-ms #inst "2024Z"))
(throwof "i-ms-monthzone" '(inst-ms #inst "2024-01Z"))
(throwof "i-ms-dayzone" '(inst-ms #inst "2024-01-01+02:00"))
(throwof "i-ms-negzone" '(inst-ms #inst "2024-01-01T00:00:00-05:30"))
(throwof "i-ms-leap" '(inst-ms #inst "2024-02-29"))
(throwof "i-ms-y2k" '(inst-ms #inst "2000-02-29"))
(throwof "i-ms-pre" '(inst-ms #inst "1960-05-05"))
(throwof "i-ms-pre-tz" '(inst-ms #inst "1960-05-05T12:30:15.250-04:00"))
(throwof "i-ms-year1" '(inst-ms #inst "0001-01-01"))
(throwof "i-ms-epoch" '(inst-ms #inst "1970-01-01T00:00:00.000-00:00"))
(throwof "i-ms-maxday" '(inst-ms #inst "9999-12-31T23:59:59.999Z"))
;; ---- B: failure modes ----
(throwof "i-ms-nil" '(inst-ms nil))
(throwof "i-ms-str" '(inst-ms "2024-01-01"))
(throwof "i-ms-int" '(inst-ms 42))
(throwof "i-ms-uuid" '(inst-ms (parse-uuid "550e8400-e29b-41d4-a716-446655440000")))
(throwof "i-ms-vec" '(inst-ms [1]))
(throwof "i-ms-arity0" '(inst-ms))
(throwof "i-ms-arity2" '(inst-ms #inst "2024-01-01" #inst "2024-01-02"))
(throwof "i-msm-arity0" '(inst-ms*))
(throwof "i-msm-arity2" '(inst-ms* #inst "2024-01-01" #inst "2024-01-02"))
;; ---- C: inst? battery + satisfies? equivalence ----
(throwof "i-p-inst" '(inst? #inst "2024-01-01"))
(throwof "i-p-nil" '(inst? nil))
(throwof "i-p-str" '(inst? "2024-01-01"))
(throwof "i-p-int" '(inst? 42))
(throwof "i-p-date" '(inst? (java.util.Date. 0)))
(throwof "i-p-sql" '(inst? (java.sql.Timestamp. 0)))
(throwof "i-p-jtime" '(inst? (java.time.Instant/EPOCH)))
(throwof "i-p-cal" '(inst? (java.util.GregorianCalendar.)))
(throwof "i-p-uuid" '(inst? (random-uuid)))
(throwof "i-sat-jtime" '(try (inst-ms (java.time.Instant/EPOCH)) (catch Throwable t (str "THROW " (.getName (class t))))))
(throwof "i-sat-eq" '(map (fn [x] [(inst? x) (satisfies? Inst x)]) [#inst "2024-01-01" nil "s" 42 (random-uuid) (java.util.Date. 0)]))
;; ---- D: equality + hashing + ordering ----
(throwof "i-eq-parse" '(= #inst "2024-01-01" #inst "2024-01-01T00:00:00.000-00:00"))
(throwof "i-eq-zone" '(= #inst "2024-01-01T02:00:00+02:00" #inst "2024-01-01T00:00:00Z"))
(throwof "i-eq-str" '(= #inst "2024-01-01" "2024-01-01"))
(throwof "i-eq-nil" '(= #inst "2024-01-01" nil))
(throwof "i-hash-a" '(hash #inst "2024-01-01"))
(throwof "i-hash-b" '(hash #inst "1970-01-01T00:00:00.000-00:00"))
(throwof "i-hash-c" '(hash #inst "1960-05-05"))
(throwof "i-hash-eq" '(= (hash #inst "2024-01-01T02:00:00+02:00") (hash #inst "2024-01-01T00:00:00Z")))
(throwof "i-hash-map" '(get {#inst "2024-01-01" :hit} #inst "2024-01-01T00:00:00.000-00:00"))
(throwof "i-cmp-eq" '(compare #inst "2024-01-01" #inst "2024-01-01T00:00:00.000-00:00"))
(throwof "i-cmp-lt" '(compare #inst "2024-01-01" #inst "2024-01-02"))
(throwof "i-cmp-gt" '(compare #inst "2024-01-02" #inst "2024-01-01"))
(throwof "i-cmp-neg" '(compare #inst "1960-05-05" #inst "2024-01-01"))
(throwof "i-cmp-str" '(compare #inst "2024-01-01" "s"))
(throwof "i-cmp-uuid" '(compare #inst "2024-01-01" (random-uuid)))
;; ---- E: printing ----
(throwof "i-pr-day" '(pr-str #inst "2024-01-01"))
(throwof "i-pr-frac" '(pr-str #inst "2024-01-01T00:00:00.1234567"))
(throwof "i-pr-pre" '(pr-str #inst "1960-05-05"))
(throwof "i-pr-year1" '(pr-str #inst "0001-01-01"))
(throwof "i-pr-offset" '(pr-str #inst "2024-01-01T10:00:00.000+02:00"))
(throwof "i-print-day" '(print-str #inst "2024-01-01"))
(throwof "i-str-day" '(str #inst "2024-01-01"))
(throwof "i-str-pre" '(str #inst "1960-05-05T12:30:15.250-04:00"))
(throwof "i-pr-roundtrip" '(= #inst "2024-06-15T12:30:15.250Z" (read-string (pr-str #inst "2024-06-15T12:30:15.250Z"))))
(throwof "i-class" '(class #inst "2024-01-01"))
;; ---- F: user extension through the protocol (runs last: mutates String) ----
(throwof "i-ext-str" '(do (extend-protocol Inst String (inst-ms* [s] 42)) [(inst-ms "x") (inst? "x") (inst-ms #inst "2024-01-01")]))
(deftype S9Box [ms])
(throwof "i-ext-deftype" '(do (extend-protocol Inst S9Box (inst-ms* [b] (.-ms b))) [(inst-ms (S9Box. 7)) (inst? (S9Box. 7))]))
