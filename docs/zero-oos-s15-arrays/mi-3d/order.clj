;; S15 ad-hoc probes: error-order corners (issue #59). Run from repo root:
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s15-arrays/mi-3d/order.clj > docs/zero-oos-s15-arrays/mi-3d/order.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t))
                     (when-let [c (.getCause ^Throwable t)]
                       (str " CAUSE " (.getName (class c)) "|" (pr-str (.getMessage c)))))))))
(throwof "ix-str-idx" '(aget "s" "x"))
(throwof "ix-order" '(aset (int-array [1]) 5 "v"))
(throwof "ix-setint-order" '(aset-int (object-array [1]) 5 1))
(throwof "ix-body" '(aset (object-array [nil]) 0 "x" 1))
(throwof "ix-nil-literal" '(aget nil "x"))
(throwof "ia-int-hugedbl" '(into-array Integer/TYPE [1e300]))
(throwof "ia-short-hugedbl" '(into-array Short/TYPE [1e300]))
(throwof "ia-char-70000" '(into-array Character/TYPE [70000]))
(throwof "t2d-map" '(to-array-2d {:a 1}))
(throwof "t2d-str" '(to-array-2d "ab"))
(throwof "t2d-arr" '(to-array-2d (int-array [1 2])))
(throwof "setx-both-bad" '(aset-int (int-array [1]) "x" "y"))
(throwof "setx-badidx-nilarr" '(aset-int nil "x" 1))
(throwof "setx-badval-nilarr" '(aset-int nil 0 "y"))
(throwof "tail-oob-mismatch" '(aset (make-array Integer/TYPE 1 1) 0 5 "s"))
(throwof "tail-oob-nilprim" '(aset (make-array Integer/TYPE 1 1) 0 5 nil))
(throwof "tail-ref-mismatch" '(let [s2 (make-array (class "s") 1 1)] (aset s2 0 0 7)))
(throwof "tail-ref-oob-mismatch" '(let [s2 (make-array (class "s") 1 1)] (aset s2 0 5 7)))
(throwof "tail-ref-nil" '(let [s2 (make-array (class "s") 1 1)] [(aset s2 0 0 nil) (seq (aget s2 0))]))
(throwof "ix-aset-idx" '(aset "s" "x" 9))
