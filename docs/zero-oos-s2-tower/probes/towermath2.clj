;; S2 tower-math oracle probes, round 2: minus/add rounding shape,
;; thread-binding of *math-context*, MathContext(String), bad-mode
;; cause (oracle: alpha10 jars, issue #44).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s2-tower/probes/towermath2.clj > docs/zero-oos-s2-tower/probes/towermath2.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"/Users/hhu/workspace/clojure/[^\"]*" "PATH"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; thread binding without the eval wrapper + base-frame contents
(println "mc-thread2" (pr-str (thread-bound? #'*math-context*)))
(println "mc-gtb" (pr-str (sort (map str (keys (get-thread-bindings))))))
(println "mc-gtbv" (pr-str (get (get-thread-bindings) #'*math-context* :absent)))
;; add/sub/mul rounding shape: post-round vs pre-round vs add-of-negate
(throwof "r2-add" '(with-precision 1 (+ 2.5M 2.5M)))
(throwof "r2-sub" '(with-precision 3 (- 1.234M 5.678M)))
(throwof "r2-sub3" '(with-precision 1 (- 2.5M 2.4M)))
(throwof "r2-mul" '(with-precision 1 (* 1.5M 1.5M)))
(throwof "r2-subfold" '(with-precision 3 (- 5.678M 1.234M 0.001M)))
(throwof "r2-quot" '(with-precision 1 (quot 55.678M 1.1M)))
(throwof "r2-ratrat" '(with-precision 1 (+ 1/3 1/6)))
(throwof "r2-mulmix" '(with-precision 3 (* 2M 1/3)))
(throwof "r2-eqrm" '(= (java.math.MathContext. 10) (java.math.MathContext. 10 java.math.RoundingMode/FLOOR)))
(throwof "r2-eqrm2" '(= (java.math.MathContext. 10) (java.math.MathContext. 10 java.math.RoundingMode/HALF_UP)))
;; denominator nil text
(throwof "r2-den-nil" '(denominator nil))
;; MathContext(String) surface
(throwof "r2-strctx" '(with-precision "precision=3 roundingMode=HALF_UP" (/ 1M 3M)))
(throwof "r2-strctx2" '(with-precision "precision=3 roundingMode=FLOOR" (str *math-context*)))
(throwof "r2-strshort" '(with-precision "precision=3" 1))
(throwof "r2-strbadp" '(with-precision "precision=x roundingMode=HALF_UP" 1))
(throwof "r2-strbadm" '(with-precision "precision=3 roundingMode=FOO" 1))
(throwof "r2-strneg" '(with-precision "precision=-3 roundingMode=HALF_UP" 1))
(throwof "r2-strempty" '(with-precision "" 1))
(throwof "r2-hugeprec" '(with-precision 3000000000 1))
;; bad-mode root cause (stable part only)
(throwcause "r2-badmode-cause" '(with-precision 10 :rounding FOO 1))
