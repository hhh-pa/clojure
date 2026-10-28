;; S16 oracle probes: bean + PrintWriter-on (oracle: alpha10 jars, issue #60).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/beans.clj > docs/zero-oos-s16-classemu/probes/beans.out
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
;; ---- A: bean shapes ----
(throwof "b-str-keys" '(sort (map str (keys (bean "a")))))
(throwof "b-str-empty" '(get (bean "a") :empty))
(throwcause "b-str-class" '(get (bean "a") :class))
(throwof "b-str-bytes" '(class (get (bean "a") :bytes)))
(throwof "b-long-keys" '(sort (map str (keys (bean 1)))))
(throwcause "b-long-class" '(get (bean 1) :class))
(throwof "b-vec-keys" '(sort (map str (keys (bean [1 2])))))
(throwof "b-map-keys" '(sort (map str (keys (bean {:a 1})))))
(throwof "b-kw-keys" '(sort (map str (keys (bean :k)))))
(throwof "b-bool-keys" '(sort (map str (keys (bean true)))))
(throwof "b-count" '(count (bean "a")))
(throwof "b-contains" '(contains? (bean "a") :empty))
(throwof "b-get-missing" '(get (bean "a") :nope))
(throwof "b-get-default" '(get (bean "a") :nope :dflt))
(throwof "b-seq-first" '(first (bean "a")))
(throwcause "b-bean-class" '(class (bean "a")))
(throwcause "b-nil" '(bean nil))
(throwof "b-sym-keys" '(sort (map str (keys (bean 's)))))
(throwof "b-str-blank" '(get (bean "a") :blank))
(throwof "b-str-blank2" '(get (bean "") :blank))
(throwof "b-vec-first" '(get (bean [1 2]) :first))
(throwof "b-vec-last" '(get (bean [1 2]) :last))
(throwof "b-vec-empty" '(get (bean []) :empty))
(throwof "b-map-empty" '(get (bean {:a 1}) :empty))
(throwof "b-kw-name" '(get (bean :a/b) :name))
(throwof "b-kw-ns" '(get (bean :a/b) :namespace))
(throwof "b-sym-name" '(get (bean 'a/b) :name))
(throwof "b-sym-ns" '(get (bean 'a/b) :namespace))
;; ---- B: PrintWriter-on ----
(throwof "w-basic" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (write "hi")) (. w (flush)) @seen))
(throwof "w-empty-flush" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (flush)) @seen))
(throwof "w-close-flushes" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (write "hi")) (. w (close)) @seen))
(throwof "w-close-fn" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj [:f s])) (fn [] (swap! seen conj [:c])))] (. w (write "hi")) (. w (close)) @seen))
(throwof "w-close-noflushfn-call" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) (fn [] (swap! seen conj :closed)))] (. w (close)) @seen))
(throwof "w-accumulate" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (write "a")) (. w (write "b")) (. w (flush)) @seen))
(throwcause "w-class" '(class (PrintWriter-on (fn [s] nil) nil)))
(throwof "w-write-offlen" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (write "hello" 1 3)) (. w (flush)) @seen))
(throwof "w-autoflush" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil true)] (. w (write "a\nb")) @seen))
(throwof "w-autoflush-then" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil true)] (. w (write "a\nb")) (. w (flush)) @seen))
(throwof "w-noautoflush" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (. w (write "a\nb")) @seen))
(throwcause "w-arity1" '(PrintWriter-on (fn [s] nil)))
(throwof "w-print-binding" '(let [seen (atom []) w (PrintWriter-on (fn [s] (swap! seen conj s)) nil)] (binding [*out* w] (print "x") (flush)) @seen))
