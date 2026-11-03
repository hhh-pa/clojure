;; dispatch.clj -- part of the pretty printer for Clojure

;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Author: Tom Faulhaber
;; April 3, 2009


;; This module implements the default dispatch tables for pretty printing code and
;; data.

(in-ns 'clojure.pprint)

;; W3 divergence: upstream installs dispatch entries with (use-method mm
;; Class fn), which calls (. multifn addMethod dispatch-val func). Method
;; values are unavailable here, so each site below is a plain defmethod.

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;; Implementations of specific dispatch table entries
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;;; Handle forms that can be "back-translated" to reader macros
;;; Not all reader macros can be dealt with this way or at all. 
;;; Macros that we can't deal with at all are:
;;; ;  - The comment character is absorbed by the reader and never is part of the form
;;; `  - Is fully processed at read time into a lisp expression (which will contain concats
;;;      and regular quotes).
;;; ~@ - Also fully eaten by the processing of ` and can't be used outside.
;;; ,  - is whitespace and is lost (like all other whitespace). Formats can generate commas
;;;      where they deem them useful to help readability.
;;; ^  - Adding metadata completely disappears at read time and the data appears to be
;;;      completely lost.
;;;
;;; Most other syntax stuff is dealt with directly by the formats (like (), [], {}, and #{})
;;; or directly by printing the objects using Clojure's built-in print functions (like
;;; :keyword, \char, or ""). The notable exception is #() which is special-cased.

(def ^{:private true} reader-macros
     {'quote "'", 'clojure.core/deref "@", 
      'var "#'", 'clojure.core/unquote "~"})

(defn- pprint-reader-macro [alis]
  (let [^String macro-char (reader-macros (first alis))]
    (when (and macro-char (= 2 (count alis)))
      (pp-write *out* macro-char)
      (write-out (second alis))
      true)))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;; Dispatch for the basic data types when interpreted
;; as data (as opposed to code).
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;;; TODO: inline these formatter statements into funcs so that we
;;; are a little easier on the stack. (Or, do "real" compilation, a
;;; la Common Lisp)

(declare pprint-map)

(defn- pprint-meta [obj]
  (when *print-meta*
    (when-let [m (meta obj)]
      (pp-write *out* "^")
      (pprint-map m)
      (pp-write *out* " ")
      (pprint-newline :linear))))

;;; (def pprint-simple-list (formatter-out "~:<~@{~w~^ ~_~}~:>"))
(defn- pprint-simple-list [alis]
  (pprint-meta alis)
  (pprint-logical-block :prefix "(" :suffix ")"
    (print-length-loop [alis (seq alis)]
      (when alis
	(write-out (first alis))
	(when (next alis)
	  (pp-write *out* " ")
	  (pprint-newline :linear)
	  (recur (next alis)))))))

(defn- pprint-list [alis]
  (if-not (pprint-reader-macro alis)
    (pprint-simple-list alis)))

;;; (def pprint-vector (formatter-out "~<[~;~@{~w~^ ~_~}~;]~:>"))
(defn- pprint-vector [avec]
  (pprint-meta avec)
  (pprint-logical-block :prefix "[" :suffix "]"
    (print-length-loop [aseq (seq avec)]
      (when aseq
	(write-out (first aseq))
	(when (next aseq)
	  (pp-write *out* " ")
	  (pprint-newline :linear)
	  (recur (next aseq)))))))

(def ^{:private true} pprint-array (formatter-out "~<[~;~@{~w~^, ~:_~}~;]~:>"))

;; W3 divergence: upstream calls (#'clojure.core/lift-ns amap) and
;; (#'clojure.core/deref-as-map o), private helpers that live in
;; core_print.clj (sibling workstream W4, not embedded yet). Local ports of
;; both, transliterated from the upstream source, follow.
(defn- strip-ns
  [named]
  ;; Upstream (symbol nil (name named)) / (keyword nil (name named)); the
  ;; 2-arity ctors reject a nil ns here, and 1-arity answers the same value.
  (if (symbol? named)
    (symbol (name named))
    (keyword (name named))))

(defn- lift-ns
  "Returns [lifted-ns lifted-kvs] or nil if m can't be lifted."
  [m]
  (when *print-namespace-maps*
    (loop [ns nil
           [[k v :as entry] & entries] (seq m)
           kvs []]
      (if entry
        (when (qualified-ident? k)
          (if ns
            (when (= ns (namespace k))
              (recur ns entries (conj kvs [(strip-ns k) v])))
            (when-let [new-ns (namespace k)]
              (recur new-ns entries (conj kvs [(strip-ns k) v])))))
        [ns kvs]))))

(defn- deref-as-map [o]
  (let [pending (and (instance? clojure.lang.IPending o)
                     (not (.isRealized ^clojure.lang.IPending o)))
        [ex val]
        (when-not pending
          (try [false (deref o)]
               (catch Throwable e
                 [true e])))]
    {:status
     (cond
      (or ex
          (and (instance? clojure.lang.Agent o)
               (agent-error o)))
      :failed

      pending
      :pending

      :else
      :ready)

     :val val}))

;;; (def pprint-map (formatter-out "~<{~;~@{~<~w~^ ~_~w~:>~^, ~_~}~;}~:>"))
(defn- pprint-map [amap]
  (pprint-meta amap)
  (let [[ns lift-map] (when (not (record? amap))
                        (lift-ns amap))
        amap (or lift-map amap)
        prefix (if ns (str "#:" ns "{") "{")]
    (pprint-logical-block :prefix prefix :suffix "}"
      (print-length-loop [aseq (seq amap)]
        (when aseq
          (pprint-logical-block
            (write-out (ffirst aseq))
            (pp-write *out* " ")
            (pprint-newline :linear)
            (set! *current-length* 0) ; always print both parts of the [k v] pair
            (write-out (fnext (first aseq))))
          (when (next aseq)
            (pp-write *out* ", ")
            (pprint-newline :linear)
            (recur (next aseq))))))))

;;; (def ^{:private true} pprint-set (formatter-out "~<#{~;~@{~w~^ ~:_~}~;}~:>"))
(defn- pprint-set [aset]
  (pprint-meta aset)
  (pprint-logical-block :prefix "#{" :suffix "}"
    (print-length-loop [aseq (seq aset)]
      (when aseq
        (write-out (first aseq))
        (when (next aseq)
          (pp-write *out* " ")
          (pprint-newline :linear)
          (recur (next aseq)))))))

(defn- pprint-ideref [o]
  (pp-write *out* (str "#object[" (.getName (class o)) " " (format "0x%x" (System/identityHashCode o)) " "))
  (write-out (deref-as-map o))
  (pp-write *out* "]"))

(def ^{:private true} pprint-pqueue (formatter-out "~<<-(~;~@{~w~^ ~_~}~;)-<~:>"))

(defn- pp-is-array?
  "Upstream (.isArray (class obj)); our isArray answers false for every class,
  so also accept the JVM array class-name prefix [ (e.g. [I for int arrays)."
  [obj]
  (let [c (class obj)]
    (and c (or (.isArray c) (= \[ (first (.getName c)))))))

(defn- pprint-simple-default [obj]
  (cond
    (pp-is-array? obj) (pprint-array obj)
    ;; Upstream records reach pprint-map through the IPersistentMap method;
    ;; record classes miss that ancestor here, so route them explicitly.
    (record? obj) (pprint-map obj)
    ;; Likewise, reify-based IDerefs (promise/future) answer instance? but
    ;; miss multimethod isa? dispatch; upstream routes every IDeref here.
    (instance? clojure.lang.IDeref obj) (pprint-ideref obj)
    (and *print-suppress-namespaces* (symbol? obj)) (pp-print (name obj))
    :else (pp-pr obj)))


(defmulti 
  simple-dispatch
  "The pretty print dispatch function for simple data structure format."
  {:added "1.2" :arglists '[[object]]} 
  class)

(defmethod simple-dispatch clojure.lang.ISeq [o] (pprint-list o))
(defmethod simple-dispatch clojure.lang.IPersistentVector [o] (pprint-vector o))
(defmethod simple-dispatch clojure.lang.IPersistentMap [o] (pprint-map o))
(defmethod simple-dispatch clojure.lang.IPersistentSet [o] (pprint-set o))
(defmethod simple-dispatch clojure.lang.PersistentQueue [o] (pprint-pqueue o))
(defmethod simple-dispatch clojure.lang.Var [o] (pprint-simple-default o))
(defmethod simple-dispatch clojure.lang.IDeref [o] (pprint-ideref o))
(defmethod simple-dispatch nil [o] (pp-pr o))
(defmethod simple-dispatch :default [o] (pprint-simple-default o))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Dispatch for the code table
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(declare pprint-simple-code-list)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Format the namespace ("ns") macro. This is quite complicated because of all the
;;; different forms supported and because programmers can choose lists or vectors
;;; in various places.
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defn- brackets
  "Figure out which kind of brackets to use"
  [form]
  (if (vector? form)
    ["[" "]"]
    ["(" ")"]))

(defn- pprint-ns-reference
  "Pretty print a single reference (import, use, etc.) from a namespace decl"
  [reference]
  (if (sequential? reference)
    (let [[start end] (brackets reference)
          [keyw & args] reference]
      (pprint-logical-block :prefix start :suffix end
        ((formatter-out "~w~:i") keyw)
        (loop [args args]
          (when (seq args)
            ((formatter-out " "))
            (let [arg (first args)]
              (if (sequential? arg)
                (let [[start end] (brackets arg)]
                  (pprint-logical-block :prefix start :suffix end
                    (if (and (= (count arg) 3) (keyword? (second arg)))
                      (let [[ns kw lis] arg]
                        ((formatter-out "~w ~w ") ns kw)
                        (if (sequential? lis)
                          ((formatter-out (if (vector? lis)
                                            "~<[~;~@{~w~^ ~:_~}~;]~:>"
                                            "~<(~;~@{~w~^ ~:_~}~;)~:>"))
                           lis)
                          (write-out lis)))
                      (apply (formatter-out "~w ~:i~@{~w~^ ~:_~}") arg)))
                  (when (next args)
                    ((formatter-out "~_"))))
                (do
                  (write-out arg)
                  (when (next args)
                    ((formatter-out "~:_"))))))
            (recur (next args))))))
    (when reference (write-out reference))))

(defn- pprint-ns
  "The pretty print dispatch chunk for the ns macro"
  [alis]
  (if (next alis) 
    (let [[ns-sym ns-name & stuff] alis
          [doc-str stuff] (if (string? (first stuff))
                            [(first stuff) (next stuff)]
                            [nil stuff])
          [attr-map references] (if (map? (first stuff))
                                  [(first stuff) (next stuff)]
                                  [nil stuff])]
      (pprint-logical-block :prefix "(" :suffix ")"
        ((formatter-out "~w ~1I~@_~w") ns-sym ns-name)
        (when (or doc-str attr-map (seq references))
          ((formatter-out "~@:_")))
        (when doc-str
          (cl-format true "\"~a\"~:[~;~:@_~]" doc-str (or attr-map (seq references))))
        (when attr-map
          ((formatter-out "~w~:[~;~:@_~]") attr-map (seq references)))
        (loop [references references]
          (pprint-ns-reference (first references))
          (when-let [references (next references)]
            (pprint-newline :linear)
            (recur references)))))
    (write-out alis)))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Format something that looks like a simple def (sans metadata, since the reader
;;; won't give it to us now).
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(def ^{:private true} pprint-hold-first (formatter-out "~:<~w~^ ~@_~w~^ ~_~@{~w~^ ~_~}~:>"))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Format something that looks like a defn or defmacro
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;;; Format the params and body of a defn with a single arity
(defn- single-defn [alis has-doc-str?]
  (if (seq alis)
    (do
      (if has-doc-str?
        ((formatter-out " ~_"))
        ((formatter-out " ~@_")))
      ((formatter-out "~{~w~^ ~_~}") alis))))

;;; Format the param and body sublists of a defn with multiple arities
(defn- multi-defn [alis has-doc-str?]
  (if (seq alis)
    ((formatter-out " ~_~{~w~^ ~_~}") alis)))

;;; TODO: figure out how to support capturing metadata in defns (we might need a 
;;; special reader)
(defn- pprint-defn [alis]
  (if (next alis) 
    (let [[defn-sym defn-name & stuff] alis
          [doc-str stuff] (if (string? (first stuff))
                            [(first stuff) (next stuff)]
                            [nil stuff])
          [attr-map stuff] (if (map? (first stuff))
                             [(first stuff) (next stuff)]
                             [nil stuff])]
      (pprint-logical-block :prefix "(" :suffix ")"
        ((formatter-out "~w ~1I~@_~w") defn-sym defn-name)
        (if doc-str
          ((formatter-out " ~_~w") doc-str))
        (if attr-map
          ((formatter-out " ~_~w") attr-map))
        ;; Note: the multi-defn case will work OK for malformed defns too
        (cond
         (vector? (first stuff)) (single-defn stuff (or doc-str attr-map))
         :else (multi-defn stuff (or doc-str attr-map)))))
    (pprint-simple-code-list alis)))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Format something with a binding form
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defn- pprint-binding-form [binding-vec]
  (pprint-logical-block :prefix "[" :suffix "]"
    (print-length-loop [binding binding-vec]
      (when (seq binding)
        (pprint-logical-block binding
          (write-out (first binding))
          (when (next binding)
            (pp-write *out* " ")
            (pprint-newline :miser)
            (write-out (second binding))))
        (when (next (rest binding))
          (pp-write *out* " ")
          (pprint-newline :linear)
          (recur (next (rest binding))))))))

(defn- pprint-let [alis]
  (let [base-sym (first alis)]
    (pprint-logical-block :prefix "(" :suffix ")"
      (if (and (next alis) (vector? (second alis)))
        (do
          ((formatter-out "~w ~1I~@_") base-sym)
          (pprint-binding-form (second alis))
          ((formatter-out " ~_~{~w~^ ~_~}") (next (rest alis))))
        (pprint-simple-code-list alis)))))


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Format something that looks like "if"
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(def ^{:private true} pprint-if (formatter-out "~:<~1I~w~^ ~@_~w~@{ ~_~w~}~:>"))

(defn- pprint-cond [alis]
  (pprint-logical-block :prefix "(" :suffix ")"
    (pprint-indent :block 1)
    (write-out (first alis))
    (when (next alis)
      (pp-write *out* " ")
      (pprint-newline :linear)
     (print-length-loop [alis (next alis)]
       (when alis
         (pprint-logical-block alis
          (write-out (first alis))
          (when (next alis)
            (pp-write *out* " ")
            (pprint-newline :miser)
            (write-out (second alis))))
         (when (next (rest alis))
           (pp-write *out* " ")
           (pprint-newline :linear)
           (recur (next (rest alis)))))))))

(defn- pprint-condp [alis]
  (if (> (count alis) 3) 
    (pprint-logical-block :prefix "(" :suffix ")"
      (pprint-indent :block 1)
      (apply (formatter-out "~w ~@_~w ~@_~w ~_") alis)
      (print-length-loop [alis (seq (drop 3 alis))]
        (when alis
          (pprint-logical-block alis
            (write-out (first alis))
            (when (next alis)
              (pp-write *out* " ")
              (pprint-newline :miser)
              (write-out (second alis))))
          (when (next (rest alis))
            (pp-write *out* " ")
            (pprint-newline :linear)
            (recur (next (rest alis)))))))
    (pprint-simple-code-list alis)))

;;; The map of symbols that are defined in an enclosing #() anonymous function
(def ^:dynamic ^{:private true} *symbol-map* {})

(defn- pprint-anon-func [alis]
  (let [args (second alis)
        nlis (first (rest (rest alis)))]
    (if (vector? args)
      (binding [*symbol-map* (if (= 1 (count args)) 
                               {(first args) "%"}
                               (into {} 
                                     (map 
                                      #(vector %1 (str \% %2)) 
                                      args 
                                      (range 1 (inc (count args))))))]
        ((formatter-out "~<#(~;~@{~w~^ ~_~}~;)~:>") nlis))
      (pprint-simple-code-list alis))))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; The master definitions for formatting lists in code (that is, (fn args...) or
;;; special forms).
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;;; This is the equivalent of (formatter-out "~:<~1I~@{~w~^ ~_~}~:>"), but is
;;; easier on the stack.

(defn- pprint-simple-code-list [alis]
  (pprint-logical-block :prefix "(" :suffix ")"
    (pprint-indent :block 1)
    (print-length-loop [alis (seq alis)]
      (when alis
	(write-out (first alis))
	(when (next alis)
	  (pp-write *out* " ")
	  (pprint-newline :linear)
	  (recur (next alis)))))))

;;; Take a map with symbols as keys and add versions with no namespace.
;;; That is, if ns/sym->val is in the map, add sym->val to the result.
(defn- two-forms [amap]
  (into {} 
        (mapcat 
         identity 
         (for [x amap] 
           [x [(symbol (name (first x))) (second x)]]))))

(defn- add-core-ns [amap]
  (let [core "clojure.core"]
    (into {}
          (map #(let [[s f] %] 
                  (if (not (or (namespace s) (special-symbol? s)))
                    [(symbol core (name s)) f]
                    %))
               amap))))

(def ^:dynamic ^{:private true} *code-table*
     (two-forms
      (add-core-ns
       {'def pprint-hold-first, 'defonce pprint-hold-first, 
	'defn pprint-defn, 'defn- pprint-defn, 'defmacro pprint-defn, 'fn pprint-defn,
        'let pprint-let, 'loop pprint-let, 'binding pprint-let,
        'with-local-vars pprint-let, 'with-open pprint-let, 'when-let pprint-let,
	'if-let pprint-let, 'doseq pprint-let, 'dotimes pprint-let,
	'when-first pprint-let,
        'if pprint-if, 'if-not pprint-if, 'when pprint-if, 'when-not pprint-if,
        'cond pprint-cond, 'condp pprint-condp,
        'fn* pprint-anon-func,
        '. pprint-hold-first, '.. pprint-hold-first, '-> pprint-hold-first,
        'locking pprint-hold-first, 'struct pprint-hold-first,
        'struct-map pprint-hold-first, 'ns pprint-ns 
        })))

(defn- pprint-code-list [alis]
  (if-not (pprint-reader-macro alis) 
    (if-let [special-form (*code-table* (first alis))]
      (special-form alis)
      (pprint-simple-code-list alis))))

(defn- pprint-code-symbol [sym]
  (if-let [arg-num (sym *symbol-map*)]
    (pp-print arg-num)
    (if *print-suppress-namespaces*
      (pp-print (name sym))
      (pp-pr sym))))

(defmulti 
  code-dispatch
  "The pretty print dispatch function for pretty printing Clojure code."
  {:added "1.2" :arglists '[[object]]} 
  class)

(defmethod code-dispatch clojure.lang.ISeq [o] (pprint-code-list o))
(defmethod code-dispatch clojure.lang.Symbol [o] (pprint-code-symbol o))

;; The following are all exact copies of simple-dispatch
(defmethod code-dispatch clojure.lang.IPersistentVector [o] (pprint-vector o))
(defmethod code-dispatch clojure.lang.IPersistentMap [o] (pprint-map o))
(defmethod code-dispatch clojure.lang.IPersistentSet [o] (pprint-set o))
(defmethod code-dispatch clojure.lang.PersistentQueue [o] (pprint-pqueue o))
(defmethod code-dispatch clojure.lang.IDeref [o] (pprint-ideref o))
(defmethod code-dispatch nil [o] (pp-pr o))
(defmethod code-dispatch :default [o] (pprint-simple-default o))

(set-pprint-dispatch simple-dispatch)


;;; For testing
(comment

(with-pprint-dispatch code-dispatch 
  (pprint 
   '(defn cl-format 
      "An implementation of a Common Lisp compatible format function"
      [stream format-in & args]
      (let [compiled-format (if (string? format-in) (compile-format format-in) format-in)
            navigator (init-navigator args)]
        (execute-format stream compiled-format navigator)))))

(with-pprint-dispatch code-dispatch 
  (pprint 
   '(defn cl-format 
      [stream format-in & args]
      (let [compiled-format (if (string? format-in) (compile-format format-in) format-in)
            navigator (init-navigator args)]
        (execute-format stream compiled-format navigator)))))

(with-pprint-dispatch code-dispatch 
  (pprint
   '(defn- -write 
      ([this x]
         (condp = (class x)
           String 
           (let [s0 (write-initial-lines this x)
                 s (.replaceFirst s0 "\\s+$" "")
                 white-space (.substring s0 (count s))
                 mode (getf :mode)]
             (if (= mode :writing)
               (dosync
                (write-white-space this)
                (.col_write this s)
                (setf :trailing-white-space white-space))
               (add-to-buffer this (make-buffer-blob s white-space))))

           Integer
           (let [c ^Character x]
             (if (= (getf :mode) :writing)
               (do 
                 (write-white-space this)
                 (.col_write this x))
               (if (= c (int \newline))
                 (write-initial-lines this "\n")
                 (add-to-buffer this (make-buffer-blob (str (char c)) nil))))))))))

(with-pprint-dispatch code-dispatch 
  (pprint 
   '(defn pprint-defn [writer alis]
      (if (next alis) 
        (let [[defn-sym defn-name & stuff] alis
              [doc-str stuff] (if (string? (first stuff))
                                [(first stuff) (next stuff)]
                                [nil stuff])
              [attr-map stuff] (if (map? (first stuff))
                                 [(first stuff) (next stuff)]
                                 [nil stuff])]
          (pprint-logical-block writer :prefix "(" :suffix ")"
                                (cl-format true "~w ~1I~@_~w" defn-sym defn-name)
                                (if doc-str
                                  (cl-format true " ~_~w" doc-str))
                                (if attr-map
                                  (cl-format true " ~_~w" attr-map))
                                ;; Note: the multi-defn case will work OK for malformed defns too
                                (cond
                                  (vector? (first stuff)) (single-defn stuff (or doc-str attr-map))
                                  :else (multi-defn stuff (or doc-str attr-map)))))
        (pprint-simple-code-list writer alis)))))
)
nil

