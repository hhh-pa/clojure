;; Oracle g123-alts: alts!! ready/default/priority/close/errors.
(require '[clojure.core.async :as a])

(defn show [label v]
  (println (str label " => " (pr-str v))))

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW (unexpected!)"))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

(defn port-tag [[v p] c1 c2]
  [v (cond (= p c1) :c1 (= p c2) :c2 (= p :default) :default :else :?)])

;; --- ready take wins ---
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :ready)
  (show "a-take" (port-tag (a/alts!! [c1 c2]) c1 c2)))

;; --- ready put wins ([val port] with val true) ---
(let [c1 (a/chan) c2 (a/chan 1)]
  (show "a-put" (port-tag (a/alts!! [[c2 :v]]) c1 c2))
  (show "a-put-took" (a/<!! c2)))

;; --- closed channel completes immediately with nil ---
(let [c1 (a/chan) c2 (a/chan)]
  (a/close! c1)
  (show "a-closed" (port-tag (a/alts!! [c1 c2]) c1 c2)))

;; --- put to closed channel completes with false ---
(let [c1 (a/chan) c2 (a/chan)]
  (a/close! c2)
  (show "a-put-closed" (port-tag (a/alts!! [[c2 :v]]) c1 c2)))

;; --- :default when nothing ready ---
(let [c1 (a/chan) c2 (a/chan)]
  (show "a-default" (a/alts!! [c1 c2] :default :dflt))
  (show "a-default-nil" (a/alts!! [c1 c2] :default nil)))

;; --- ready op beats :default ---
(let [c1 (a/chan 1) c2 (a/chan)]
  (a/>!! c1 :ready)
  (show "a-ready-beats-default" (port-tag (a/alts!! [c1 c2] :default :dflt) c1 c2)))

;; --- :priority tries in order ---
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :first)
  (a/>!! c2 :second)
  (show "a-priority" (port-tag (a/alts!! [c1 c2] :priority true) c1 c2))
  (show "a-priority-rev" (port-tag (a/alts!! [c2 c1] :priority true) c1 c2)))

;; --- blocking alts woken by a later put ---
(let [c1 (a/chan) c2 (a/chan)
      f (future (a/alts!! [c1 c2]))]
  (Thread/sleep 200)
  (a/>!! c2 :late)
  (show "a-woken" (port-tag @f c1 c2)))

;; --- blocking alts put woken by a later take ---
(let [c1 (a/chan)
      f (future (a/alts!! [[c1 :pv]]))]
  (Thread/sleep 200)
  (show "a-put-woken-take" (a/<!! c1))
  (show "a-put-woken" (port-tag @f c1 c1)))

;; --- ports containers ---
(let [c (a/chan 1)]
  (a/>!! c :in-set)
  (show "a-set" (let [[v p] (a/alts!! #{c})] [v (= p c)])))
(show "a-nil" (try (a/alts!! nil) (catch Throwable t [(str (class t)) (.getMessage t)])))
(thrown1 "e-alts-str" #(a/alts!! "x"))

;; --- errors ---
(thrown1 "e-alts-empty" #(a/alts!! []))
(thrown1 "e-alts-nil-put" #(a/alts!! [[(a/chan 1) nil]]))
(thrown1 "e-alts-nonchan" #(a/alts!! [42]))
(thrown1 "e-alts-odd-opts" #(a/alts!! [(a/chan 1)] :default))
(thrown1 "e-alts-list-op" #(a/alts!! [(list (a/chan 1) :v)]))
(thrown1 "e-altsbang-outside-go" #(a/alts! [(a/chan)]))
