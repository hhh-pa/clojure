;; Issue #271 Slice 5j: streaming (println|print (clojure.string/join sep coll))
;; and (print (str (clojure.string/join sep coll) "\n")), zero-copy single-string
;; print/join, in-place PackedStrings::into_joined, and tight pure-int allow_fp.
(require '[clojure.string :as str])

(defn make-packed-strs [n]
  (loop [i 0 acc (transient [])]
    (if (= i n)
      (persistent! acc)
      (recur (inc i) (conj! acc (str "item-" i))))))

(defn pure-int-loop-with-global-and-local-init [v init]
  (let [n (count v)]
    (loop [i 0 acc init]
      (if (< i n)
        (recur (inc i) (+ acc (nth v i)))
        acc))))

(def INT_SHIFT 7)

(defn pure-int-loop-with-global [v]
  (let [n (count v)]
    (loop [i 0 acc 0]
      (if (< i n)
        (recur (inc i) (+ acc (nth v i) INT_SHIFT))
        acc))))

(let [packed (make-packed-strs 300)
      ints [10 20 30 40]
      mixed ["a" 1 \z true 2.5 nil]
      split (str/split "one two three" #" ")
      one-str ["single-piece"]
      big (apply str (repeat 20000 "q"))
      big-vec ["pre" big "post"]
      lazy-s (map str [1 2 3])
      map-vec [{:a 1} {:b 2}]
      char-vec (vec "abc")]
  ;; Streaming println, print, and (print (str (join ...) "\n"))
  (println (str/join "," (subvec packed 0 4)))
  (println (str/join "," ints))
  (println (str/join "|" mixed))
  (println (str/join "-" split))
  (println (str/join "," one-str))
  (print (str/join ":" ints))
  (println)
  (print (str (str/join "," ints) "\n"))

  ;; Rebound *out* via with-out-str and big >= 16 KiB chunks
  (let [captured (with-out-str (println (str/join "," big-vec)))]
    (println (count captured) (subs captured 0 6) (subs captured (- (count captured) 6))))

  ;; Fallback paths: lazy seq, non-fast separator, char vec, vector with map, *print-dup*
  (println (str/join "," lazy-s))
  (println (str/join 0 ints))
  (println (str/join "," char-vec))
  (println (str/join "," map-vec))
  (println (with-out-str (binding [*print-dup* true] (println (str/join "," ints)))))
  (println (with-out-str (print (str/join "," map-vec))))
  (println (with-out-str (print (str (str/join "," lazy-s) "\n"))))

  ;; In-place PackedStrings join (unique) + single-string join & print
  (let [j (str/join "\n" (make-packed-strs 270))]
    (println (count j) (subs j 0 13)))
  (let [s1 (str/join "\n" ["only-one"])]
    (println s1))

  ;; Pure-int scalar loops with local init and int global (+ double deopt safety)
  (println (pure-int-loop-with-global-and-local-init [1 2 3 4] 10))
  (println (pure-int-loop-with-global [1 2 3]))
  (println (pure-int-loop-with-global-and-local-init [1.5 2.5] 0)))
