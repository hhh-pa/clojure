;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W8 (core parity) redesign (issue #204) of upstream
;; `src/clj/clojure/inspector.clj`: the same public API over text
;; rendering to `*out*` (no Swing — there is no GUI toolkit here).
;; The navigation layer (`atom?`, `collection-tag`, `is-leaf`,
;; `get-child`, `get-child-count`, `list-provider` and their methods)
;; is a verbatim port; only the view layer changes:
;; - `tree-model`, `old-table-model`, `list-model`, `table-model`
;;   return rendered STRINGS (upstream returns Swing models);
;; - `inspect-tree`, `inspect-table`, `inspect` print to `*out*` and
;;   return nil (upstream returns the JFrame).
;; The `(:import ...)` clause is dropped with the widgets.

(ns ^{:doc "Text object inspector for Clojure data structures."
       :author "Rich Hickey"}
    clojure.inspector)

(defn atom? [x]
  (not (coll? x)))

(defn collection-tag [x]
  (cond
   (map-entry? x) :entry
   (instance? java.util.Map x) :seqable
   (instance? java.util.Set x) :seqable
   (sequential? x) :seq
   (instance? clojure.lang.Seqable x) :seqable
   :else :atom))

(defmulti is-leaf collection-tag)
(defmulti get-child (fn [parent index] (collection-tag parent)))
(defmulti get-child-count collection-tag)

(defmethod is-leaf :default [node]
  (atom? node))
(defmethod get-child :default [parent index]
  (nth parent index))
(defmethod get-child-count :default [parent]
  (count parent))

(defmethod is-leaf :entry [e]
  (is-leaf (val e)))
(defmethod get-child :entry [e index]
  (get-child (val e) index))
(defmethod get-child-count :entry [e]
  (count (val e)))

(defmethod is-leaf :seqable [parent]
  false)
(defmethod get-child :seqable [parent index]
  (nth (seq parent) index))
(defmethod get-child-count :seqable [parent]
  (count (seq parent)))

(defn- render-grid
  ;; Aligned text columns: every cell padded to its column width,
  ;; one row per line. Headers print as the first row when given.
  ([rows] (render-grid nil rows))
  ([headers rows]
   (let [grid (if headers
                (cons (vec headers) (map vec rows))
                (map vec rows))]
     (if (empty? grid)
       ""
       (let [widths (apply map
                           (fn [& col] (apply max (map count col)))
                           grid)]
         (apply str
           (map (fn [row]
                  (str (apply str
                           (interpose " "
                             (map (fn [cell w]
                                    (format (str "%-" w "s") (str cell)))
                                  row widths)))
                       "\n"))
                grid)))))))

(defn tree-model
  [data]
  ;; The root's children render at depth 0 (the root itself prints
  ;; no header line), so top-level nodes start unindented.
  (letfn [(walk [node depth]
            (let [pad (apply str (repeat (max depth 0) "  "))]
              (if (is-leaf node)
                (str pad (pr-str node) "\n")
                (apply str
                  (map #(walk (get-child node %) (inc depth))
                       (range (get-child-count node)))))))]
    (if (is-leaf data)
      (walk data 0)
      (walk data -1))))

(defn old-table-model [data]
  (let [row1 (first data)
        colcnt (count row1)
        vals-fn (if (map? row1) vals identity)
        header (if (map? row1)
                 (map #(name (nth (keys row1) %)) (range colcnt))
                 (map str (range colcnt)))
        rows (map (fn [row]
                    (map #(pr-str (nth (vals-fn row) %)) (range colcnt)))
                  data)]
    (render-grid header rows)))

(defn inspect-tree
  "prints a text inspector on the supplied hierarchical data"
  {:added "1.0"}
  [data]
  (print (tree-model data))
  nil)

(defn inspect-table
  "prints a text inspector on the supplied regular data, which must
  be a sequential data structure of data structures of equal length"
  {:added "1.0"}
  [data]
  (print (old-table-model data))
  nil)

(defmulti list-provider class)

(defmethod list-provider :default [x]
  {:nrows 1 :get-value (fn [i] x) :get-label (fn [i] (.getName (class x)))})

(defmethod list-provider java.util.List [c]
  (let [v (if (vector? c) c (vec c))]
    {:nrows (count v)
     :get-value (fn [i] (v i))
     :get-label (fn [i] i)}))

(defmethod list-provider java.util.Map [c]
  (let [v (vec (sort (map (fn [[k v]] (vector k v)) c)))]
    {:nrows (count v)
     :get-value (fn [i] ((v i) 1))
     :get-label (fn [i] ((v i) 0))}))

(defn list-model [provider]
  (let [{:keys [nrows get-value get-label]} provider]
    (render-grid
      (map (fn [i] [(str (get-label i)) (print-str (get-value i))])
           (range nrows)))))

(defmulti table-model class)

(defmethod table-model :default [x]
  (render-grid [[(str (class x)) (pr-str x)]]))

(defn inspect
  "prints a text inspector on the supplied object"
  {:added "1.0"}
  [x]
  (print (list-model (list-provider x)))
  nil)
