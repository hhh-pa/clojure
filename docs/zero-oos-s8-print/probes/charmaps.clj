;; Zero-oos S8 (issue #51) oracle probes: char-escape-string / char-name-string (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/charmaps.clj > docs/zero-oos-s8-print/probes/charmaps.out
(require 'clojure.string)
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str (str (.getMessage ^Throwable c)))))))))
;; ---- full contents (sorted for determinism; the port's tables must equal these) ----
(throwof "c-escape" '(into (sorted-map) char-escape-string))
(throwof "c-name" '(into (sorted-map) char-name-string))
(throwof "c-counts" '[(count char-escape-string) (count char-name-string)])
(throwof "c-types" '[(every? char? (keys char-escape-string)) (every? string? (vals char-escape-string)) (every? char? (keys char-name-string)) (every? string? (vals char-name-string))])
(throwof "c-escape-lookup" '[(char-escape-string \newline) (char-escape-string \a)])
(throwof "c-name-lookup" '[(char-name-string \space) (char-name-string \a)])
;; ---- the vars are plain (non-dynamic) roots ----
(throwof "c-dynamic" '[(:dynamic (meta #'char-escape-string)) (:dynamic (meta #'char-name-string))])
(throwof "c-bind-err" '(binding [char-escape-string {}] 1))
;; ---- behavioral anchors: the JVM printer reads these maps ----
(throwof "c-uses-escape" '(pr-str "a\nb\t\"\\"))
(throwof "c-uses-name" '(pr-str [\newline \tab \space \backspace \formfeed \return]))
;; ---- root redefinition reaches the JVM printer (informational: the port
;; asserts table equality in tests instead of consulting the vars at print time) ----
(throwof "c-redef-escape" '(with-redefs [char-escape-string {}] (pr-str "a\nb")))
(throwof "c-redef-name" '(with-redefs [char-name-string {}] (pr-str \newline)))
(println "S8-charmaps DONE")
