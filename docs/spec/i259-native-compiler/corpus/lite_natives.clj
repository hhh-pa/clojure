;; #271 Slice 2 (spec 6.1): BootMode::Lite natives standing in for
;; interpreted definitions — reduce-kv / run! (core.clj) and the
;; clojure.string slow paths (string.clj) — answer exactly what run
;; answers: values, exception classes, and messages.
(require '[clojure.string :as str])

(defn show [label f]
  (println label (try (pr-str (f))
                      (catch Throwable e (str "THROW " (.getName (class e)) " " (.getMessage e))))))

(def colls [{:a 1 :b 2} [10 20 30] nil {} 5 "ab" #{1} (first {:a 1}) (list (first {:a 1}))
            (sorted-map 2 :b 1 :a) (subvec [1 2 3] 1) (vector-of :long 4 5)])

(doseq [c colls]
  (show [:kv-assoc c] #(reduce-kv assoc {} c))
  (show [:kv-vec c] #(reduce-kv (fn [acc k v] (conj acc [k v])) [] c))
  (show [:kv-reduced c] #(reduce-kv (fn [acc k v] (reduced [:stop k v])) [] c))
  (show [:kv-nonfn c] #(reduce-kv 5 [] c))
  (show [:kv-nil-fn c] #(reduce-kv nil [] c))
  (show [:kv-arity c] #(reduce-kv (fn [acc k] acc) [] c))
  (show [:kv-apply c] #(apply reduce-kv [assoc {} c]))
  (show [:run c] #(let [a (atom [])] [(run! (fn [x] (swap! a conj x)) c) @a]))
  (show [:run-reduced c] #(let [a (atom [])] [(run! (fn [x] (swap! a conj x) (reduced x)) c) @a]))
  (show [:run-nonfn c] #(run! 5 c))
  (show [:run-apply c] #(apply run! [identity c])))
(show :kv-arity-call #(reduce-kv assoc {}))
(show :run-arity-call #(run! identity))

(def receivers [["str" " a b "] ["empty" ""] ["nil" nil] ["long" 5] ["double" 1.5] ["char" \a]
                ["kw" :k] ["vec" [1 "x"]] ["list" '("p" "q")] ["re" #"b"] ["sym" 'ab] ["true" true]
                ["false" false] ["map" {:a 1}] ["sb" (StringBuilder. " a b ")] ["sb-empty" (StringBuilder.)]
                ["file" (java.io.File. "lite-natives-no-such-file")]])

(def args2 [["a" "a"] ["empty" ""] ["nil" nil] ["long" 5] ["char" \a] ["re" #"b"] ["sb" (StringBuilder. "a")]])

(doseq [[sl s] receivers]
  (show [:trim sl] #(str/trim s))
  (show [:blank? sl] #(str/blank? s))
  (show [:split-lines sl] #(str/split-lines s))
  (show [:join1 sl] #(str/join s))
  (doseq [[tl t] args2]
    (show [:starts-with? sl tl] #(str/starts-with? s t))
    (show [:ends-with? sl tl] #(str/ends-with? s t))
    (show [:includes? sl tl] #(str/includes? s t))
    (show [:join2 tl sl] #(str/join t s))
    (show [:split sl tl] #(str/split s t))))
(show :trim-arity #(str/trim))
(show :split-arity #(str/split "a"))
