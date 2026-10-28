;; S16 oracle probes: import + ns-imports (oracle: alpha10 jars, issue #60).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/imports.clj > docs/zero-oos-s16-classemu/probes/imports.out
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
;; ---- A: import macro shapes ----
(throwof "m-single" '(macroexpand-1 '(import java.util.Date)))
(throwof "m-quoted" '(macroexpand-1 '(import 'java.util.Date)))
(throwof "m-list" '(macroexpand-1 '(import '(java.util Date Calendar))))
(throwof "m-unquoted-list" '(macroexpand-1 '(import (java.util Date Calendar))))
(throwof "m-multi" '(macroexpand-1 '(import java.util.Date java.sql.Time)))
(throwof "m-empty" '(macroexpand-1 '(import)))
(throwof "m-importstar" '(resolve 'clojure.core/import*))
;; ---- B: fresh ns-imports (default java.lang table) ----
(throwof "n-fresh" '(do (create-ns 's16fresh) (sort (map str (keys (ns-imports 's16fresh))))))
(throwcause "n-fresh-val" '(do (create-ns 's16fresh) (get (ns-imports 's16fresh) 'String)))
(throwof "n-user-count" '(count (ns-imports 'user)))
(throwof "n-user-sample" '(sort (take 8 (sort (map str (keys (ns-imports 'user)))))))
;; ---- C: import behavior in scratch nses (referred, one ns per probe) ----
;; Each probe binds a FRESH ns (create-ns is bare) and refers
;; clojure.core into it first, so `import` resolves.
(throwcause "d-return" '(binding [*ns* (create-ns 's16w01)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date))))
(throwcause "d-table" '(binding [*ns* (create-ns 's16w02)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (get (ns-imports 's16w02) 'Date)))
(throwof "d-keys" '(binding [*ns* (create-ns 's16w03)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (sort (map str (filter #(= "Date" (str %)) (keys (ns-imports 's16w03)))))))
(throwcause "d-value" '(binding [*ns* (create-ns 's16w04)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval 'Date)))
(throwcause "d-class-of" '(binding [*ns* (create-ns 's16w05)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (class (eval 'Date))))
(throwof "d-eq" '(binding [*ns* (create-ns 's16w06)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (= (eval 'Date) java.util.Date)))
(throwcause "d-listform" '(binding [*ns* (create-ns 's16w07)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import '(java.util Calendar))) (eval 'Calendar)))
(throwcause "d-repeat" '(binding [*ns* (create-ns 's16w08)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(import 'java.util.Date)) (eval 'Date)))
(throwcause "d-javalang" '(binding [*ns* (create-ns 's16w09)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.lang.String)) (eval 'String)))
(throwof "d-resolve" '(binding [*ns* (create-ns 's16w10)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(resolve 'Date))))
(throwof "d-nsresolve" '(binding [*ns* (create-ns 's16w11)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(ns-resolve 's16w11 'Date))))
(throwof "d-nsmap-has" '(binding [*ns* (create-ns 's16w12)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (contains? (ns-map 's16w12) 'Date)))
(throwof "d-empty" '(binding [*ns* (create-ns 's16w13)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import)) (count (filter #(= "Date" (str %)) (keys (ns-imports 's16w13))))))
;; shadowing: an intern wins over the import
(throwof "d-shadow" '(binding [*ns* (create-ns 's16w14)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(def Date 42)) (eval 'Date)))
;; unmap drops the import
(throwof "d-unmap" '(binding [*ns* (create-ns 's16w15)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(ns-unmap 's16w15 'Date)) (contains? (ns-imports 's16w15) 'Date)))
;; unmap of a default import drops it too
(throwof "d-unmap-default" '(binding [*ns* (create-ns 's16w16)] (eval '(clojure.core/refer 'clojure.core)) (eval '(ns-unmap 's16w16 'String)) (contains? (ns-imports 's16w16) 'String)))
;; ---- D: import errors ----
(throwcause "e-missing" '(binding [*ns* (create-ns 's16e01)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'no.such.Klass))))
(throwcause "e-conflict" '(binding [*ns* (create-ns 's16e02)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date)) (eval '(import 'java.sql.Date))))
(throwcause "e-conflict2" '(binding [*ns* (create-ns 's16e03)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.Date 'java.sql.Date))))
(throwcause "e-lowercase" '(binding [*ns* (create-ns 's16e04)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 'java.util.date))))
(throwcause "e-number" '(binding [*ns* (create-ns 's16e05)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import 5))))
(throwcause "e-string" '(binding [*ns* (create-ns 's16e06)] (eval '(clojure.core/refer 'clojure.core)) (eval '(import "java.util.Date"))))
;; ---- F: default import table, simple -> fully-qualified ----
(throwof "n-fresh-full" '(do (create-ns 's16fresh2) (sort (map (fn [[k v]] (str k " -> " (.getName ^Class v))) (ns-imports 's16fresh2)))))
;; ---- E: class-name value resolution ----
(throwcause "r-fqcn" '(eval 'java.util.Date))
(throwcause "r-short" '(eval 'String))
(throwcause "r-short2" '(eval 'Integer))
(throwof "r-resolve-short" '(resolve 'String))
(throwof "r-nsresolve-short" '(ns-resolve 'user 'String))
