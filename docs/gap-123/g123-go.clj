;; Oracle g123-go: go blocks, parking ops (<! / >!), close/buffer/xform interplay, errors.
;; Run: java ... clojure.main docs/gap-123/g123-go.clj (fresh JVM; stderr interleaved)
(require '[clojure.core.async :as a]
         '[clojure.core.async.impl.protocols :as impl]
         '[clojure.string :as str])

(def closed? impl/closed?)

(defn show [label v]
  (println (str label " => " (pr-str v))))

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW (unexpected!)"))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

(defn norm [v]
  (-> (pr-str v)
      (str/replace #"@[0-9a-f]+" "@HEX")
      (str/replace #"0x[0-9a-f]+" "0xHEX")))

;; Take a go channel with a timeout guard (returns the value or :TIMEOUT).
(defn goo [label g]
  (let [[v p] (a/alts!! [g (a/timeout 10000)])
        v (if (= p g) v :TIMEOUT)]
    (println (str label " => " (pr-str v)))
    v))

;; --- go basics ---
(show "g-ret-class" (class (a/go 42)))
(show "g-ch-pr" (norm (a/go 42)))
(goo "g-val" (a/go 42))
(let [g (a/go 42)]
  (a/<!! g)
  (show "g-closed-after" [(a/<!! g) (closed? g)]))
(let [g (a/go nil)]
  (show "g-nil-body" [(a/<!! g) (closed? g)]))
(let [g (a/go false)]
  (show "g-false-body" [(a/<!! g) (a/<!! g) (closed? g)]))
(let [g (a/go)]
  (show "g-empty" [(a/<!! g) (closed? g)]))
(goo "g-multi" (a/go :a :b :c))
(let [g (a/go :v)]
  (Thread/sleep 500)
  (let [[v1 p1] (a/alts!! [g] :default :pending)
        [v2 p2] (a/alts!! [g] :default :pending)]
    (show "g-buf1" [v1 (= p1 g) v2 (= p2 g)])))
(let [main (.getName (Thread/currentThread))
      g (a/go (.getName (Thread/currentThread)))]
  (show "g-thread-diff" (not= main (goo "g-thread" g))))
(show "g-quote" (a/<!! (a/go '(a/<! 1))))
(let [c (a/chan 1)]
  (a/>!! c :q)
  (goo "g-squote" (a/go `(got ~(a/<! c)))))
(goo "g-var" (a/go (var a/<!)))
;; A local named <! shadows the parking op (outside-go and inside-go lets).
(let [<! (fn [_] :shadowed)]
  (goo "g-shadow" (a/go (<! :ignored))))
(goo "g-shadow-in" (a/go (let [<! (fn [_] :in)] (<! :ignored))))
;; go returns before the body completes (the put lands after go returns).
(let [c (a/chan) g (a/go (a/<! c))]
  (a/>!! c :v)
  (goo "g-async" g))

;; --- parking takes/puts ---
(let [c (a/chan) g (a/go (a/<! c))]
  (a/>!! c :v)
  (goo "p-take-then-put" g))
(let [c (a/chan) g (a/go (a/>! c :w))]
  (show "p-put-then-take" [(a/<!! c) (goo "p-put-ret" g)]))
(let [c (a/chan 2)]
  (a/>!! c 1) (a/>!! c 2)
  (goo "p-seq-takes" (a/go (+ (a/<! c) (a/<! c)))))
(let [c (a/chan 1)]
  (a/>!! c 41)
  (goo "p-let-take" (a/go (let [x (a/<! c)] (inc x)))))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (goo "p-if-take" (a/go (if (a/<! c) :t :f))))
(let [c (a/chan)]
  (a/close! c)
  (goo "p-if-take-nil" (a/go (if (a/<! c) :t :f))))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (goo "p-multi-expr" (a/go (a/<! c) :done)))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (goo "p-doseq" (a/go (doseq [ch [c]] (a/<! ch)) :drained)))
(goo "p-nested-go" (a/go (a/<!! (a/go :inner))))
(let [inner (a/go :in)]
  (goo "p-nested-go-chan" (a/go (a/<! inner))))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (goo "p-try" (a/go (try (a/<! c) (catch Exception e :caught)))))
(goo "p-try-throw" (a/go (try (throw (ex-info "in" {})) (catch clojure.lang.ExceptionInfo e :caught))))
(let [c (a/chan 8)
      gs (doall (for [i (range 8)] (a/go (a/>! c i))))]
  (show "p-fan-in" [(sort (for [_ (range 8)] (a/<!! c)))
                    (sort (for [g gs] (a/<!! g)))]))
(let [cs (doall (for [_ (range 16)] (a/chan)))
      gs (doall (for [c cs] (a/go (a/<! c))))]
  (doseq [[i c] (map-indexed vector cs)] (a/>!! c i))
  (show "p-scale16" (sort (for [g gs] (a/<!! g)))))

;; --- close interplay ---
(let [c (a/chan)]
  (a/close! c)
  (goo "c-take-closed" (a/go (a/<! c))))
(let [c (a/chan)]
  (a/close! c)
  (goo "c-put-closed" (a/go (a/>! c :v))))
(let [c (a/chan) g (a/go (a/<! c))]
  (future (Thread/sleep 300) (a/close! c))
  (goo "c-park-released" g))
(let [c (a/chan 1)]
  (a/>!! c :a)
  (let [g (a/go (a/>! c :b))]
    (Thread/sleep 300)
    (a/close! c)
    (show "c-put-full-then-close" [(a/<!! c) (a/<!! c) (goo "c-put-full-ret" g)])))
(let [c (a/chan 2)]
  (a/>!! c :x) (a/>!! c :y) (a/close! c)
  (goo "c-drain" (a/go [(a/<! c) (a/<! c) (a/<! c)])))

;; --- buffer interplay ---
(let [c (a/chan 1)]
  (a/>!! c :a)
  (let [g (a/go (a/>! c :b))]
    (Thread/sleep 300)
    (show "b-full-park" [(a/<!! c) (a/<!! c) (goo "b-full-ret" g)])))
(let [c (a/chan (a/dropping-buffer 1))]
  (a/>!! c :keep)
  (goo "b-drop" (a/go (a/>! c :drop) (a/<! c))))
(let [c (a/chan (a/sliding-buffer 1))]
  (a/>!! c :old)
  (goo "b-slide" (a/go (a/>! c :new) (a/<! c))))

;; --- transducer interplay ---
(let [c (a/chan 2 (map inc))]
  (goo "x-map" (a/go (a/>! c 41) (a/<! c))))
(let [c (a/chan 2 (filter odd?))]
  (goo "x-filter" (a/go (a/>! c 2) (a/>! c 3) (a/<! c))))
(let [c (a/chan 4 (mapcat identity))]
  (goo "x-mult" (a/go (a/>! c [1 2]) [(a/<! c) (a/<! c)])))
(let [c (a/chan 2 (take 2)) g (a/go (a/>! c 1) (a/>! c 2))]
  (show "x-reduced" [(goo "x-reduced-ret" g) (a/<!! c) (a/<!! c) (a/<!! c) (closed? c)]))
(let [c (a/chan 2 (map (fn [_] (throw (ex-info "x" {})))) (fn [t] :fb))]
  (goo "x-exh" (a/go (a/>! c 1) (a/<! c))))
(let [c (a/chan 2 (map (fn [x] (throw (ex-info "dflt" {:x x})))))]
  (goo "x-dflt-exh" (a/go (a/>! c 1) (a/alts!! [c] :default :empty))))
(Thread/sleep 1000)

(def ^:dynamic *dq* :root)
(show "g-binding" (binding [*dq* :bound] (a/<!! (a/go *dq*))))

;; --- errors (take, settle so the dispatch thread's trace flushes, then print) ---
(let [g (a/go (throw (ex-info "boom" {:n 1})))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "e-throw" [v (closed? g)]))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (let [g (a/go ((fn [] (a/<! c))))
        v (a/<!! g)]
    (Thread/sleep 1000)
    (show "e-fn-boundary" [v (closed? g)])))
(let [c (a/chan 1) g (a/go (a/>! c nil))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "e-bang-nil" [v (closed? g)]))
(let [g (a/go (a/<! 42))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "e-take-nonchan" [v (closed? g)]))
(let [c (a/chan) g (a/go (a/<!))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "e-arity" [v (closed? g)]))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (let [g (a/go (doall (for [ch [c]] (a/<! ch))))
        v (a/<!! g)]
    (Thread/sleep 1000)
    (show "e-for-fn" [v (closed? g)])))
(show "done" :done)
