;; #271: a program whose leading ns requires clojure.string and calls a
;; clojure.string fn no Lite native covers (replace) compiles the vendored
;; definitions it needs (replace, replace-by) into itself; every shape of
;; replace answers exactly what run's interpreted library answers.
;; #273: both follow the JDK -- the replacement grammar (\ quotes, $10
;; with one group is $1 then 0, bad references throw), Matcher.find's
;; empty match right after a non-empty one, fn replacements in
;; replace-first, and the StringBuffer appendReplacement loop.
(ns corpus.clojure-string-replace
  (:require [clojure.string :as str]))

(println (str/replace "a-b-c" \- \+) (str/replace "a.b.c" "." "::")
         (str/replace "0042" #"^0+" "") (str/replace "x1y22" #"\d+" "N"))
(println (str/replace "Almost Pig Latin" #"\b(\w)(\w+)\b" "$2$1ay")
         (str/replace "abc" #"z" "q") (str/replace "" #"a*" "-"))
(println (try (str/replace "a1b22" #"\d+" str) (catch Exception e (.getMessage e))))
(println (try (str/replace "abc" 5 "x") (catch Exception e (.getMessage e)))
         (try (str/replace nil "a" "b") (catch Exception e (class e))))
(println (str/join "," (map #(str/replace % #"[aeiou]" "") ["banana" "kiwi"])))
(println (str/replace "abc" #"b" "\\$") (str/replace "abc" #"(b)" "$10")
         (str/replace "aaa" #"a*" "-") (str/replace "ab" #"" "-"))
(println (str/replace-first "a1b22" #"\d+" (fn [m] (str "<" m ">")))
         (str/replace-first "abc" #"(b)" "$10") (str/replace-first "aaa" #"a*" "-"))
(println (try (str/replace "abc" #"b" "$2") (catch Exception e (.getMessage e)))
         (try (str/replace "abc" #"b" "\\") (catch Exception e (.getMessage e))))
(println (re-seq #"a*" "baaac") (str/split "baaac" #"a*"))
(println (let [m (re-matcher #"\d" "a1b2c") sb (StringBuffer.)]
           (while (.find m) (.appendReplacement m sb "<$0>"))
           (str (.appendTail m sb))))
