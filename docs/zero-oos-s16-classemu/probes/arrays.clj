;; S16 oracle probes: array classes (ORACLE-ONLY for the S15 follow-up; S16 has no
;; array values on its own surface. Oracle: alpha10 jars, issue #60).
;; Run from repo root (/home/hhu/workspace/clojure-s16):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/arrays.clj > docs/zero-oos-s16-classemu/probes/arrays.out
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
(throwof "a-class-obj" '(.getName ^Class (class (into-array [1 2]))))
(throwof "a-class-int" '(.getName ^Class (class (int-array [1]))))
(throwof "a-class-str" '(.getName ^Class (class (into-array ["a"]))))
(throwof "a-class-2d" '(.getName ^Class (class (make-array Long/TYPE 2 2))))
(throwof "a-bases-obj" '(vec (map #(.getName ^Class %) (bases (class (into-array [1]))))))
(throwof "a-bases-int" '(vec (map #(.getName ^Class %) (bases (class (int-array [1]))))))
(throwof "a-supers-int" '(vec (sort (map #(.getName ^Class %) (supers (class (int-array [1])))))))
(throwof "a-instance-t" '(instance? (class (int-array [1])) (int-array [2])))
(throwof "a-instance-f" '(instance? (class (int-array [1])) (long-array [2])))
(throwof "a-instance-obj" '(instance? Object (int-array [1])))
(throwof "a-instance-cloneable" '(instance? Cloneable (int-array [1])))
(throwof "a-cast-ok" '(vec (cast (class (int-array [1])) (int-array [2]))))
(throwcause "a-cast-miss" '(cast (class (int-array [1])) (long-array [2])))
(throwof "a-class-array?" '(class? (class (int-array [1]))))
(throwof "a-str" '(str (class (int-array [1]))))
(throwof "a-pr" '(pr-str (class (int-array [1]))))
(throwof "a-isarray" '(.. (identity (class (int-array [1]))) (isArray)))
(throwof "a-component" '(.getName ^Class (.. (identity (class (int-array [1]))) (getComponentType))))
