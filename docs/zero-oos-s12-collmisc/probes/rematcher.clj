;; S12 oracle probes: re-matcher/re-groups (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/rematcher.clj > docs/zero-oos-s12-collmisc/probes/rematcher.out
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
(throwof "rm-class" '(str (class (re-matcher #"a+" "aaab"))))
(throwcause "rm-print" '(pr-str (re-matcher #"a+" "aaab")))
(throwof "rm-find-groups" '(let [m (re-matcher #"(\d+)-(\d+)" "12-34")] [(re-find m) (re-groups m)]))
(throwof "rm-find-plain" '(let [m (re-matcher #"a+" "xxaay")] [(re-find m) (re-groups m)]))
(throwof "rm-find-none" '(nil? (let [m (re-matcher #"z+" "abc")] (re-find m))))
(throwcause "rm-groups-nomatch" '(let [m (re-matcher #"z+" "abc")] (re-find m) (re-groups m)))
(throwcause "rm-groups-fresh" '(re-groups (re-matcher #"a" "a")))
(throwof "rm-multi" '(let [m (re-matcher #"a" "aba")] [(re-find m) (re-find m) (re-find m)]))
(throwof "rm-groups-each" '(let [m (re-matcher #"(a)(b)?" "ab a")] [(re-find m) (re-groups m) (re-find m) (re-groups m)]))
(throwof "rm-exhaust" '(let [m (re-matcher #"a" "a")] [(re-find m) (re-find m) (re-find m)]))
(throwcause "rm-bad-pattern" '(re-matcher "a+" "aa"))
(throwcause "rm-bad-subject" '(re-matcher #"a" 42))
(throwcause "rm-nil-subject" '(re-matcher #"a" nil))
(throwcause "rm-groups-bad" '(re-groups "nope"))
(throwof "rm-empty-advance" '(let [m (re-matcher #"a*" "b")] [(re-find m) (re-find m) (re-find m)]))
