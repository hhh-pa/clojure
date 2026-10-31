;; Oracle g123-edge: buffer-size edges (non-integers, zero/negative
;; windowed buffers, garbage buf-or-n).
(require '[clojure.core.async :as a])

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW " (pr-str :done)))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

(defn show [label v]
  (println (str label " => " (pr-str v))))

(thrown1 "e-drop-str" #(a/dropping-buffer "x"))
(thrown1 "e-slide-str" #(a/sliding-buffer "x"))
(thrown1 "e-chan-15" #(a/chan 1.5))
(thrown1 "e-chan-str2" #(a/chan "x"))
(show "drop0-put" (let [c (a/chan (a/dropping-buffer 0))] [(a/>!! c 1)]))
;; NOTE: prints a stack trace to stderr (the default ex-handler
;; swallows the `removeLast` crash) and the put returns normally.
(thrown1 "slide0-put" #(let [c (a/chan (a/sliding-buffer 0))] (a/>!! c 1)))
(show "dropneg-put" (let [c (a/chan (a/dropping-buffer -1))] [(a/>!! c 1)]))
(show "slideneg" (let [c (a/chan (a/sliding-buffer -1))] (a/>!! c 1) (a/<!! c)))
(thrown1 "e-chan-vec" #(let [c (a/chan [1])] (a/>!! c 1)))
