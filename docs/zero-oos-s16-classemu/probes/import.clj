;; S16 oracle probes: import macro + import table (oracle: alpha10 jars, issue #60).
;; Run from repo root (/home/hhu/workspace/clojure-s16):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/import.clj > docs/zero-oos-s16-classemu/probes/import.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"__\d+" "__N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; In-ns helper: create (idempotent) + bind *ns* (no root switch, so the
;; loader ns survives; import/refer/read respect the binding).
(defmacro inimp [ns & body]
  `(do (create-ns '~ns) (binding [*ns* (the-ns '~ns)] ~@body)))
;; ---- A: macro shape ----
(throwof "i-expand-quote" '(macroexpand-1 '(import 'java.util.Date)))
(throwof "i-expand-bare" '(macroexpand-1 '(import java.util.Date)))
(throwof "i-expand-list" '(macroexpand-1 '(import '(java.util Date Calendar))))
(throwof "i-expand-list-bare" '(macroexpand-1 '(import (java.util Date))))
(throwof "i-expand-multi" '(macroexpand-1 '(import 'java.lang.Long '(java.util Date Calendar))))
(throwcause "i-expand-str" '(macroexpand-1 '(import "java.util.Date")))
(throwof "i-import*-resolve" '(resolve 'import*))
(throwof "i-import*-resolve-q" '(resolve 'clojure.core/import*))
;; ---- B: behavior (each in its own ns) ----
(throwof "i-ret" '(inimp imp1 (import 'java.util.Date)))
(throwof "i-ns-imports" '(inimp imp1 (import 'java.util.Date) (sort (map str (keys (ns-imports 'imp1))))))
(throwof "i-ns-imports-val" '(inimp imp1 (import 'java.util.Date) (get (ns-imports 'imp1) 'Date)))
(throwof "i-resolve" '(inimp imp1 (import 'java.util.Date) (resolve 'Date)))
(throwof "i-ns-resolve" '(inimp imp1 (import 'java.util.Date) (ns-resolve 'imp1 'Date)))
(throwof "i-value-pos" '(inimp imp1 (import 'java.util.Date) (eval 'Date)))
(throwof "i-construct" '(inimp imp1 (import 'java.util.Date) (eval '(Date. 0))))
(throwof "i-dup" '(inimp imp1 (import 'java.util.Date) (import 'java.util.Date)))
(throwcause "i-conflict-class" '(inimp imp2 (import 'java.util.Date) (import 'java.sql.Date)))
(throwof "i-conflict-def" '(inimp imp3 (eval '(def Date 1)) (import 'java.util.Date) [(find-var 'imp3/Date) (resolve 'Date)]))
(throwcause "i-def-over-import" '(inimp imp4 (import 'java.util.Date) (eval '(def Date 1))))
(throwof "i-def-over-import-after" '(inimp imp4b (import 'java.util.Date) (try (eval '(def Date 1)) (catch Throwable t :caught)) [(resolve 'Date) (find-var 'imp4b/Date)]))
(throwof "i-intern-over-import" '(inimp imp5 (import 'java.util.Date) (let [sw (java.io.StringWriter.) pw (java.io.PrintWriter. sw)] (binding [*err* pw] (intern 'imp5 'Date 5)) (.flush pw) [(resolve 'Date) (get (ns-imports 'imp5) 'Date) (str sw)])))
(throwcause "i-unknown" '(inimp imp6 (import 'foo.bar.Baz)))
(throwcause "i-unknown-list" '(inimp imp6 (import '(foo.bar Baz))))
(throwof "i-unmap" '(inimp imp7 (import 'java.util.Date) (ns-unmap 'imp7 'Date) [(resolve 'Date) (contains? (ns-imports 'imp7) 'Date)]))
(throwof "i-ns-map" '(inimp imp1 (import 'java.util.Date) (get (ns-map 'imp1) 'Date)))
(throwof "i-ns-publics" '(inimp imp1 (import 'java.util.Date) (contains? (ns-publics 'imp1) 'Date)))
(throwof "i-ns-interns" '(inimp imp1 (import 'java.util.Date) (contains? (ns-interns 'imp1) 'Date)))
(throwcause "i-import-over-refer" '(do (inimp impa (eval '(def Date 1))) (inimp impb (clojure.core/refer 'impa :only '[Date]) (import 'java.util.Date))))
(throwof "i-refer-over-import" '(do (inimp impc (eval '(def Date 1))) (inimp impd (import 'java.util.Date) (let [sw (java.io.StringWriter.) pw (java.io.PrintWriter. sw)] (binding [*err* pw] (clojure.core/refer 'impc :only '[Date])) (.flush pw)) (resolve 'Date))))
(throwof "i-refer-warn" '(do (inimp impe (eval '(def Date 1))) (inimp impf (import 'java.util.Date) (let [sw (java.io.StringWriter.) pw (java.io.PrintWriter. sw)] (binding [*err* pw] (clojure.core/refer 'impe :only '[Date])) (.flush pw) (str sw)))))
(throwof "i-refer-removes-import" '(do (inimp impg (eval '(def Date 1))) (inimp imph (import 'java.util.Date) (let [sw (java.io.StringWriter.) pw (java.io.PrintWriter. sw)] (binding [*err* pw] (clojure.core/refer 'impg :only '[Date])) (.flush pw)) [(resolve 'Date) (get (ns-imports 'imph) 'Date)])))
(throwof "i-deftype-import" '(do (deftype ImpPoint [x]) (inimp imp8 (import 'user.ImpPoint) (resolve 'ImpPoint))))
(throwof "i-deftype-construct" '(inimp imp8 (import 'user.ImpPoint) (.getName ^Class (class (eval '(ImpPoint. 1))))))
(throwof "i-fqn-eval" '(eval 'java.lang.String))
(throwof "i-defaults" '(do (create-ns 'impfresh2) (sort (map str (keys (ns-imports 'impfresh2))))))
(throwof "i-ns-imports-nsobj" '(inimp imp1 (import 'java.util.Date) (get (ns-imports (the-ns 'imp1)) 'Date)))
(throwcause "i-ns-imports-str" '(inimp imp1 (import 'java.util.Date) (ns-imports "imp1")))
(throwcause "i-ns-imports-nil" '(ns-imports nil))
(throwcause "i-ns-imports-missing" '(ns-imports 'nosuchnsxyz))
(throwof "i-ns-clause" '(do (eval '(ns impns2 (:import java.util.Date))) ((fn [v] (clojure.core/eval '(in-ns 'user)) v) (clojure.core/eval '(clojure.core/resolve 'Date)))))
(throwcause "i-import-nil" '(inimp imp9 (eval '(clojure.core/import nil))))
(throwcause "i-import-empty" '(inimp imp9 (eval '(clojure.core/import '()))))
(throwcause "i-import-package" '(inimp imp9 (eval '(clojure.core/import 'java.util))))
(throwcause "i-import-slash" '(inimp imp9 (eval '(clojure.core/import 'java.util/Date))))
(throwof "i-import-multi-ret" '(inimp imp10 (import 'java.util.Date 'java.util.Calendar)))
(throwcause "i-import-num" '(inimp imp9 (eval '(clojure.core/import 5))))
(throwcause "i-import-badlist" '(inimp imp9 (eval '(clojure.core/import '((a) B)))))
(throwcause "i-import-lonely" '(inimp imp9 (eval '(clojure.core/import '(java.util)))))
(throwcause "i-import-qual-pkg" '(inimp imp9 (eval '(clojure.core/import (quote (a/b C))))))
(throwcause "i-import-qual-cls" '(inimp imp9 (eval '(clojure.core/import (quote (p a/b))))))
(throwcause "i-import-list-str" '(inimp imp9 (eval '(clojure.core/import (quote (p "C"))))))
(throwof "i-import-exdata" '(inimp imp9 (try (eval '(clojure.core/import 5)) (catch Throwable t (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))] [(ex-message c) (sort (map str (keys (ex-data c))))])))))
;; oracle-only: static surface (follow-up, not in the S16 slice)
(throwof "i-static-field" '(inimp imp1 (eval 'Integer/MAX_VALUE)))
(throwof "i-static-call" '(inimp imp1 (eval '(Integer/parseInt "42"))))
