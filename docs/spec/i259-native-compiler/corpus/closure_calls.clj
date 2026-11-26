;; Calls through a value (#271): a compiled closure of the call's arity runs
;; its code directly; every other callee keeps the general invoke.
(defn msg [f] (try (f) (catch Throwable e (str (.getSimpleName (class e)) ": " (.getMessage e)))))
(defn make-adder [n] (fn [x] (+ x n)))
(defn twice [f x] (f (f x)))
(let [add5 (make-adder 5)
      k 10
      captured (fn [x y] (+ x y k))
      multi (fn ([] :zero) ([x] [:one x]) ([x y] [:two x y]))
      varargs (fn [x & more] [x (count more)])]
  (println (add5 1) (twice add5 1) (captured 1 2) (multi) (multi 1) (multi 1 2)
           (varargs 1) (varargs 1 2 3))
  (println (msg #(add5)) (msg #(add5 1 2)) (msg #(captured 1)))
  (println (msg #((fn [x] (throw (ex-info "inner" {:x x}))) 3)))
  (println (let [f :a] (f {:a 1})) (let [f {:b 2}] (f :b)) (let [f #{3}] (f 3))
           (let [f [7 8]] (f 1)) (let [f inc] (f 1)) (let [f nil] (msg #(f 1)))))
(let [fact (fn fact [n] (if (< n 2) 1 (* n (fact (dec n)))))
      countdown (fn [f n] (if (zero? n) :done (f f (dec n))))]
  (println (fact 10) (countdown countdown 1000)))
(let [deep (fn deep [n] (if (zero? n) 0 (inc (deep (dec n)))))]
  (println (msg #(deep 1000000))))
(let [fs (mapv (fn [i] (fn [x] (* x i))) (range 4))]
  (println (map #(% 10) fs) (reduce (fn [acc f] (f acc)) 1 [inc #(* 2 %) (make-adder 3)])))
