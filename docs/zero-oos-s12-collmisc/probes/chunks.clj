;; S12 oracle probes: chunks (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/chunks.clj > docs/zero-oos-s12-collmisc/probes/chunks.out
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
;; ---- build ----
(throwof "ch-buf-class" '(str (class (chunk-buffer 32))))
(throwcause "ch-buf-print" '(pr-str (chunk-buffer 32)))
(throwof "ch-chunk-class" '(str (class (chunk (chunk-buffer 32)))))
(throwof "ch-build" '(let [b (chunk-buffer 32)] (chunk-append b :a) (chunk-append b :b) (let [c (chunk b)] [(count c) (nth c 0) (nth c 1)])))
(throwcause "ch-print" '(pr-str (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b))))
(throwof "ch-empty-count" '(count (chunk (chunk-buffer 32))))
(throwof "ch-nil" '(let [b (chunk-buffer 32)] (chunk-append b nil) (let [c (chunk b)] [(count c) (nil? (nth c 0))])))
(throwcause "ch-overflow" '(let [b (chunk-buffer 2)] (chunk-append b 1) (chunk-append b 2) (chunk-append b 3)))
(throwof "ch-bigcap" '(let [b (chunk-buffer 64)] (dotimes [i 40] (chunk-append b i)) (count (chunk b))))
(throwof "ch-zero-cap" '(count (chunk (chunk-buffer 0))))
(throwcause "ch-zero-append" '(let [b (chunk-buffer 0)] (chunk-append b 1)))
(throwcause "ch-neg-cap" '(chunk-buffer -1))
(throwof "ch-seq" '(seq (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b))))
(throwcause "ch-nth-oob" '(let [b (chunk-buffer 32)] (chunk-append b 1) (nth (chunk b) 5)))
(throwof "ch-nth-cap" '(let [b (chunk-buffer 32)] (chunk-append b 1) (nil? (nth (chunk b) 31))))
(throwcause "ch-nth-cap-oob" '(let [b (chunk-buffer 32)] (chunk-append b 1) (nth (chunk b) 32)))
(throwcause "ch-use-once" '(let [b (chunk-buffer 32)] (chunk-append b 1) (let [c (chunk b)] (chunk-append b 2) [(count c) (nth c 0) (nth c 1)])))
(throwcause "ch-chunk-twice" '(let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b) (let [c2 (chunk b)] [(count c2) (nil? (nth c2 0))])))
(throwof "ch-append-ret" '(let [b (chunk-buffer 32)] [(nil? (chunk-append b 1)) (count (chunk b))]))
;; ---- cons ----
(throwof "ch-cons-empty" '(let [r (quote (1 2))] (identical? (chunk-cons (chunk (chunk-buffer 32)) r) r)))
(throwof "ch-cons-print" '(pr-str (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk-append b 2) (chunk b)) (quote (3)))))
(throwof "ch-cons-class" '(str (class (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil))))
(throwof "ch-cons-nil-print" '(pr-str (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil)))
;; ---- project ----
(throwof "ch-first-vec" '(count (chunk-first (seq (vec (range 100))))))
(throwof "ch-first-small" '(let [c (chunk-first (seq [1 2 3]))] [(count c) (nth c 0) (nth c 2)]))
(throwof "ch-rest-vec" '(pr-str (chunk-rest (seq (vec (range 40))))))
(throwof "ch-next-vec" '(pr-str (chunk-next (seq (vec (range 40))))))
(throwof "ch-rest-end" '(let [r (chunk-rest (seq [1]))] [(pr-str r) (nil? r)]))
(throwof "ch-next-end" '(nil? (chunk-next (seq [1]))))
(throwcause "ch-first-list" '(chunk-first (seq (quote (1 2)))))
(throwcause "ch-first-str" '(chunk-first (seq "ab")))
(throwof "ch-rest-cons" '(pr-str (chunk-rest (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) (quote (2 3))))))
(throwof "ch-next-cons-end" '(nil? (chunk-next (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil))))
(throwof "ch-first-cons" '(let [c (chunk-first (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 7) (chunk b)) (quote (8))))] [(count c) (nth c 0)]))
;; ---- chunked-seq? battery ----
(throwof "ch-cq-vec" '(chunked-seq? (seq [1 2 3])))
(throwof "ch-cq-list" '(chunked-seq? (seq (quote (1 2)))))
(throwof "ch-cq-map" '(chunked-seq? (seq {:a 1 :b 2})))
(throwof "ch-cq-set" '(chunked-seq? (seq #{1 2})))
(throwof "ch-cq-str" '(chunked-seq? (seq "ab")))
(throwof "ch-cq-range" '(chunked-seq? (seq (range 100))))
(throwof "ch-cq-repeat" '(chunked-seq? (seq (repeat 5))))
(throwof "ch-cq-cycle" '(chunked-seq? (seq (cycle [1]))))
(throwof "ch-cq-iterate" '(chunked-seq? (seq (iterate inc 0))))
(throwof "ch-cq-lazy" '(chunked-seq? (lazy-seq [1 2])))
(throwof "ch-cq-cons" '(chunked-seq? (cons 1 [2])))
(throwof "ch-cq-concat" '(chunked-seq? (concat [1] [2])))
(throwof "ch-cq-mapfn" '(chunked-seq? (map inc [1 2])))
(throwof "ch-cq-nil" '(chunked-seq? nil))
(throwof "ch-cq-vecval" '(chunked-seq? [1 2]))
(throwof "ch-cq-cons-chunk" '(chunked-seq? (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil)))
(throwof "ch-cq-takelast" '(chunked-seq? (take-last 2 (range 100))))
(throwof "ch-cq-drop" '(chunked-seq? (drop 40 (range 100))))
(throwof "ch-cq-rseq" '(chunked-seq? (rseq [1 2 3])))
(throwof "ch-cq-subvec" '(chunked-seq? (seq (subvec [1 2 3 4] 1))))
(throwof "ch-cq-reseq" '(chunked-seq? (re-seq #"a" "aba")))
(throwof "ch-cq-treeseq" '(chunked-seq? (tree-seq sequential? seq [1 [2]])))
(throwof "ch-cq-range-inf" '(chunked-seq? (range)))
(throwof "ch-cq-repeat-n" '(chunked-seq? (repeat 3 :x)))
(throwof "ch-cq-eduction" '(chunked-seq? (seq (eduction (map inc) [1 2]))))
