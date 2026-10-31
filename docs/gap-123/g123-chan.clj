;; Oracle g123-chan: chan arities, buffer kinds, close semantics, errors.
;; Run: java ... clojure.main docs/gap-123/g123-chan.clj
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

;; --- chan shapes ---
(show "c-unbuf-class" (class (a/chan)))
(show "c-unbuf-pr" (norm (a/chan)))
(show "c-fixed-pr" (norm (a/chan 3)))
(show "c-nil-pr" (norm (a/chan nil)))
(show "c-buf-pr" (norm (a/chan (a/buffer 2))))
(show "c-drop-pr" (norm (a/dropping-buffer 1)))
(show "c-slip-pr" (norm (a/sliding-buffer 1)))
(show "c-dropbuf-class" (class (a/dropping-buffer 2)))
(show "c-slipbuf-class" (class (a/sliding-buffer 2)))
(show "c-fixbuf-class" (class (a/buffer 2)))
(show "c-eq-self" (let [c (a/chan)] (= c c)))
(show "c-eq-other" (= (a/chan) (a/chan)))
(show "c-hash-eq" (let [c (a/chan)] (= (hash c) (hash c))))

;; --- fixed buffer fill/drain ---
(let [c (a/chan 2)]
  (show "f-put1" (a/>!! c :a))
  (show "f-put2" (a/>!! c :b))
  (show "f-take1" (a/<!! c))
  (show "f-take2" (a/<!! c)))

;; --- dropping buffer overflow ---
(let [c (a/chan (a/dropping-buffer 2))]
  (show "d-puts" [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3)])
  (show "d-takes" [(a/<!! c) (a/<!! c)]))

;; --- sliding buffer overflow ---
(let [c (a/chan (a/sliding-buffer 2))]
  (show "s-puts" [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3)])
  (show "s-takes" [(a/<!! c) (a/<!! c)]))

;; --- close: put-after-close, drain, take-after ---
(let [c (a/chan 2)]
  (a/>!! c :x)
  (show "x-close-ret" (a/close! c))
  (show "x-closed?" (closed? c))
  (show "x-put-closed" (a/>!! c :y))
  (show "x-drain" (a/<!! c))
  (show "x-take-closed" (a/<!! c))
  (show "x-close-again" (a/close! c))
  (show "x-closed?-still" (closed? c)))

;; --- close an empty unbuffered channel ---
(let [c (a/chan)]
  (a/close! c)
  (show "u-closed-take" (a/<!! c))
  (show "u-closed-put" (a/>!! c 1)))

;; --- errors ---
(thrown1 "e-nil-put" #(a/>!! (a/chan 1) nil))
(thrown1 "e-buf0" #(a/buffer 0))
(thrown1 "e-bufneg" #(a/buffer -3))
(thrown1 "e-chan0" #(a/chan 0))
(thrown1 "e-take-nonchan" #(a/<!! 42))
(thrown1 "e-put-nonchan" #(a/>!! :kw 1))
(thrown1 "e-take-nil" #(a/<!! nil))
(thrown1 "e-close-nonchan" #(a/close! "s"))
(thrown1 "e-closed-nonchan" #(closed? "s"))
(thrown1 "e-chan-str" #(let [c (a/chan "x")] (a/>!! c 1)))
(thrown1 "e-bang-outside-go" #(a/<! (a/chan)))
(thrown1 "e-rbang-outside-go" #(a/>! (a/chan) 1))
