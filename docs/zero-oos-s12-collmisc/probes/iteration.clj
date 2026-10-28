;; S12 oracle probes: iteration (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/iteration.clj > docs/zero-oos-s12-collmisc/probes/iteration.out
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
(throwof "it-basic" '(pr-str (doall (seq (iteration (fn [k] (when (< k 3) (inc k))) :initk 0)))))
(throwof "it-take-inf" '(pr-str (take 3 (iteration inc :initk 0))))
(throwof "it-empty" '(nil? (seq (iteration (fn [_] nil)))))
(throwof "it-reduce-empty" '(reduce + 7 (iteration (fn [_] nil))))
(throwof "it-reduce" '(reduce + 0 (iteration (fn [k] (when (< k 3) (inc k))) :initk 0)))
(throwof "it-reduced" '(reduce (fn [a v] (if (> v 1) (reduced a) (+ a v))) 0 (iteration (fn [k] (when (< k 3) (inc k))) :initk 0)))
(throwof "it-vf" '(pr-str (doall (seq (iteration (fn [page] (when (< page 3) {:page page :items [page]})) :initk 0 :vf :items :kf (fn [ret] (inc (:page ret))))))))
(throwof "it-kf-stop" '(pr-str (doall (seq (iteration (fn [k] k) :initk 3 :kf (fn [k] (when (pos? k) (dec k))))))))
(throwof "it-somef" '(pr-str (doall (seq (iteration inc :initk 0 :somef (fn [r] (< r 3)))))))
(throwof "it-somef-skips" '(let [calls (atom [])] (doall (iteration inc :initk 0 :somef (fn [r] (< r 2)) :vf (fn [r] (swap! calls conj r) r))) @calls))
(throwof "it-seqable" '(instance? clojure.lang.Seqable (iteration inc)))
(throwof "it-reducible" '(instance? clojure.lang.IReduceInit (iteration inc)))
(throwof "it-cq" '(chunked-seq? (seq (iteration inc :initk 0))))
(throwof "it-step-once" '(let [c (atom 0)] [(nil? (seq (iteration (fn [_] (swap! c inc) nil)))) @c]))
(throwof "it-take-count" '(let [c (atom 0)] [(pr-str (take 2 (iteration (fn [_] (let [n (swap! c inc)] (when (<= n 5) n))) :initk nil))) @c]))
(throwof "it-twice" '(let [c (atom 0) it (iteration (fn [_] (swap! c inc) nil))] [(pr-str (doall (seq it))) (pr-str (doall (seq it))) @c]))
(throwof "it-reduce-count" '(let [c (atom 0)] (reduce + 0 (iteration (fn [k] (swap! c inc) (when (< k 2) (inc k))) :initk 0)) @c))
(throwof "it-unknown-opt" '(pr-str (take 1 (iteration inc :initk 0 :bogus 1))))
;; ---- odd-opts kwargs rule (R2: lone element IS the opts map; longer odd tails fold via cons) ----
(throwof "it-opts-lonemap" '(pr-str (take 3 (iteration identity {:initk 5}))))
(throwof "it-opts-loneset" '(pr-str (take 3 (iteration identity #{:initk}))))
(throwof "it-opts-trailmap" '(pr-str (take 3 (iteration identity :vf identity {:initk 5}))))
(throwof "it-opts-trailnil" '(pr-str (take 3 (iteration identity :vf identity nil))))
(throwof "it-opts-trailpair" '(pr-str (take 3 (iteration identity :vf identity [:a :b]))))
(throwcause "it-opts-trailstr" '(take 3 (iteration identity :vf identity "x")))
(throwcause "it-opts-traillist" '(take 3 (iteration identity :vf identity (quote ([:initk 5])))))
(throwcause "it-opts-trailvec1" '(take 3 (iteration identity :vf identity [[:initk 5]])))
(throwof "it-opts-lonekw" '(pr-str (take 1 (iteration (fn [_] 7) :initk))))
(throwcause "it-opts-trailkw" '(take 1 (iteration inc :initk 0 :vf)))
(throwof "it-opts-strkey" '(pr-str (take 1 (iteration inc "x" 1 :initk 0))))
(throwof "it-opts-nskey" '(pr-str (take 1 (iteration inc :foo/bar 1 :initk 0))))
