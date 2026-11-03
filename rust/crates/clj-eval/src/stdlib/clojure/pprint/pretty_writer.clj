;;; pretty_writer.clj -- part of the pretty printer for Clojure

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
;; Revised to use proxy instead of gen-class April 2010

;; This module implements a wrapper around a java.io.Writer which implements the
;; core of the XP algorithm.

(in-ns 'clojure.pprint)

;; W3 divergence: upstream imports clojure.lang.IDeref and java.io.Writer for
;; its (proxy [Writer IDeref PrettyFlush]). Writers here are atoms (see
;; column_writer.clj), so no import is needed.

;; TODO: Support for tab directives


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Forward declarations
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(declare get-miser-width)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Macros to simplify dealing with types and classes. These are
;;; really utilities, but I'm experimenting with them here.
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defmacro ^{:private true}
  getf
  "Get the value of the field named by the argument (which should be a keyword)."
  [sym]
  ;; Upstream `(~sym @@~'this): the proxy derefs to a ref of fields. Here this
  ;; is the fields atom itself, so one deref.
  `(~sym @~'this))

(defmacro ^{:private true}
  setf
  "Set the value of the field SYM to NEW-VAL"
  [sym new-val]
  ;; Upstream `(alter @~'this assoc ~sym ~new-val) inside dosync; swap! on the
  ;; atom is equivalent under single-threaded use.
  `(swap! ~'this assoc ~sym ~new-val))

(defmacro ^{:private true} 
  deftype [type-name & fields]
  (let [name-str (name type-name)]
    `(do
       (defstruct ~type-name :type-tag ~@fields)
       (alter-meta! #'~type-name assoc :private true)
       (defn- ~(symbol (str "make-" name-str)) 
         [& vals#] (apply struct ~type-name ~(keyword name-str) vals#))
       (defn- ~(symbol (str name-str "?")) [x#] (= (:type-tag x#) ~(keyword name-str))))))

(defmacro ^{:private true}
  write-to-base
  "Write args through the column-writer atom in (getf :base). Upstream calls
  .write on the base Writer; the atom cannot take interop calls."
  [& args]
  `(col-write (getf :base) ~@args))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; The data structures used by pretty-writer
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defstruct ^{:private true} logical-block
           :parent :section :start-col :indent
           :done-nl :intra-block-nl
           :prefix :per-line-prefix :suffix
           :logical-block-callback)

(defn- ancestor? [parent child]
  (loop [child (:parent child)]
    (cond 
     (nil? child) false
     (identical? parent child) true
     :else (recur (:parent child)))))

(defstruct ^{:private true} section :parent)

(defn- buffer-length [l] 
  (let [l (seq l)]
    (if l 
      (- (:end-pos (last l)) (:start-pos (first l)))
      0)))

; A blob of characters (aka a string)
(deftype buffer-blob :data :trailing-white-space :start-pos :end-pos)

; A newline
(deftype nl-t :type :logical-block :start-pos :end-pos)

(deftype start-block-t :logical-block :start-pos :end-pos)

(deftype end-block-t :logical-block :start-pos :end-pos)

(deftype indent-t :logical-block :relative-to :offset :start-pos :end-pos)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Functions to write tokens in the output buffer
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(def ^:private pp-newline (memoize #(System/getProperty "line.separator")))

(declare emit-nl)

(defmulti ^{:private true} write-token #(:type-tag %2))
(defmethod write-token :start-block-t [this token]
   (when-let [cb (getf :logical-block-callback)] (cb :start))
   (let [lb (:logical-block token)]
    (do
     (when-let [^String prefix (:prefix lb)]
       (write-to-base prefix))
     (let [col (get-column (getf :base))]
       (reset! (:start-col lb) col)
       (reset! (:indent lb) col)))))

(defmethod write-token :end-block-t [this token]
  (when-let [cb (getf :logical-block-callback)] (cb :end))
  (when-let [^String suffix (:suffix (:logical-block token))]
    (write-to-base suffix)))

(defmethod write-token :indent-t [this token]
  (let [lb (:logical-block token)]
    (reset! (:indent lb)
             (+ (:offset token)
                (condp = (:relative-to token)
		  :block @(:start-col lb)
		  :current (get-column (getf :base)))))))

(defmethod write-token :buffer-blob [this token]
  (write-to-base ^String (:data token)))

(defmethod write-token :nl-t [this token]
;  (prlabel wt @(:done-nl (:logical-block token)))
;  (prlabel wt (:type token) (= (:type token) :mandatory))
  (if (or (= (:type token) :mandatory)
           (and (not (= (:type token) :fill))
                @(:done-nl (:logical-block token))))
    (emit-nl this token)
    (if-let [^String tws (getf :trailing-white-space)]
      (write-to-base tws)))
  (setf :trailing-white-space nil))

(defn- write-tokens [this tokens force-trailing-whitespace]
  (doseq [token tokens]
    (if-not (= (:type-tag token) :nl-t)
      (if-let [^String tws (getf :trailing-white-space)]
	(write-to-base tws)))
    (write-token this token)
    (setf :trailing-white-space (:trailing-white-space token)))
  (let [^String tws (getf :trailing-white-space)] 
    (when (and force-trailing-whitespace tws)
      (write-to-base tws)
      (setf :trailing-white-space nil))))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; emit-nl? method defs for each type of new line. This makes
;;; the decision about whether to print this type of new line.
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;


(defn- tokens-fit? [this tokens]
;;;  (prlabel tf? (get-column (getf :base) (buffer-length tokens))
  (let [maxcol (get-max-column (getf :base))]
    (or 
     (nil? maxcol) 
     (< (+ (get-column (getf :base)) (buffer-length tokens)) maxcol))))

(defn- linear-nl? [this lb section]
;  (prlabel lnl? @(:done-nl lb) (tokens-fit? this section))
  (or @(:done-nl lb)
      (not (tokens-fit? this section))))

(defn- miser-nl? [this lb section]
  (let [miser-width (get-miser-width this)
        maxcol (get-max-column (getf :base))]
    (and miser-width maxcol
         (>= @(:start-col lb) (- maxcol miser-width))
         (linear-nl? this lb section))))

(defmulti ^{:private true} emit-nl? (fn [t _ _ _] (:type t)))

(defmethod emit-nl? :linear [newl this section _]
  (let [lb (:logical-block newl)]
    (linear-nl? this lb section)))

(defmethod emit-nl? :miser [newl this section _]
  (let [lb (:logical-block newl)]
    (miser-nl? this lb section)))

(defmethod emit-nl? :fill [newl this section subsection]
  (let [lb (:logical-block newl)]
    (or @(:intra-block-nl lb)
        (not (tokens-fit? this subsection))
        (miser-nl? this lb section))))

(defmethod emit-nl? :mandatory [_ _ _ _]
  true)

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Various support functions
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;


(defn- get-section [buffer]
  (let [nl (first buffer) 
        lb (:logical-block nl)
        section (seq (take-while #(not (and (nl-t? %) (ancestor? (:logical-block %) lb)))
                                 (next buffer)))]
    [section (seq (drop (inc (count section)) buffer))])) 

(defn- get-sub-section [buffer]
  (let [nl (first buffer) 
        lb (:logical-block nl)
        section (seq (take-while #(let [nl-lb (:logical-block %)]
                                    (not (and (nl-t? %) (or (= nl-lb lb) (ancestor? nl-lb lb)))))
                            (next buffer)))]
    section)) 

(defn- update-nl-state [lb]
  (do
   (reset! (:intra-block-nl lb) false)
   (reset! (:done-nl lb) true)
   (loop [lb (:parent lb)]
     (if lb
       (do (reset! (:done-nl lb) true)
           (reset! (:intra-block-nl lb) true)
           (recur (:parent lb)))))))

(defn- emit-nl [this nl]
  (write-to-base ^String (pp-newline))
  (setf :trailing-white-space nil)
  (let [lb (:logical-block nl)
        ^String prefix (:per-line-prefix lb)] 
    (if prefix 
      (write-to-base prefix))
    (let [^String istr (apply str (repeat (- @(:indent lb) (count prefix))
					  \space))] 
      (write-to-base istr))
    (update-nl-state lb)))

(defn- split-at-newline [tokens]
  (let [pre (seq (take-while #(not (nl-t? %)) tokens))]
    [pre (seq (drop (count pre) tokens))]))

;;; Methods for showing token strings for debugging

(defmulti ^{:private true} tok :type-tag)
(defmethod tok :nl-t [token]
  (:type token))
(defmethod tok :buffer-blob [token]
  (str \" (:data token) (:trailing-white-space token) \"))
(defmethod tok :default [token]
  (:type-tag token))
(defn- toks [toks] (map tok toks))

;;; write-token-string is called when the set of tokens in the buffer
;;; is longer than the available space on the line

(defn- write-token-string [this tokens]
  (let [[a b] (split-at-newline tokens)]
;;    (prlabel wts (toks a) (toks b))
    (if a (write-tokens this a false))
    (if b
      (let [[section remainder] (get-section b)
            newl (first b)]
;;         (prlabel wts (toks section)) (prlabel wts (:type newl)) (prlabel wts (toks remainder)) 
        (let [do-nl (emit-nl? newl this section (get-sub-section b))
              result (if do-nl 
                       (do
;;                          (prlabel emit-nl (:type newl))
                         (emit-nl this newl)
                         (next b))
                       b)
              long-section (not (tokens-fit? this result))
              result (if long-section
                       (let [rem2 (write-token-string this section)]
;;;                              (prlabel recurse (toks rem2))
                         (if (= rem2 section)
                           (do ; If that didn't produce any output, it has no nls
                                        ; so we'll force it
                             (write-tokens this section false)
                             remainder)
                           (into [] (concat rem2 remainder))))
                       result)
;;              ff (prlabel wts (toks result))
              ] 
          result)))))

(defn- write-line [this]
  (do
   (loop [buffer (getf :buffer)]
;;     (prlabel wl1 (toks buffer))
     (setf :buffer (into [] buffer))
     (if (not (tokens-fit? this buffer))
       (let [new-buffer (write-token-string this buffer)]
;;          (prlabel wl new-buffer)
         (if-not (identical? buffer new-buffer)
                 (recur new-buffer)))))))

;;; Add a buffer token to the buffer and see if it's time to start
;;; writing
(defn- add-to-buffer [this token]
;  (prlabel a2b token)
  (do
   (setf :buffer (conj (getf :buffer) token))
   (if (not (tokens-fit? this (getf :buffer)))
     (write-line this))))

;;; Write all the tokens that have been buffered
(defn- write-buffered-output [this]
  (write-line this)
  (if-let [buf (getf :buffer)]
    (do
      (write-tokens this buf true)
      (setf :buffer []))))

(defn- write-white-space [this]
  (when-let [^String tws (getf :trailing-white-space)]
    ; (prlabel wws (str "*" tws "*"))
    (write-to-base tws)
    (setf :trailing-white-space nil)))

;;; If there are newlines in the string, print the lines up until the last newline, 
;;; making the appropriate adjustments. Return the remainder of the string
(defn- write-initial-lines
  [this ^String s]
  (let [lines (.split s "\n" -1)]
    (if (= (count lines) 1)
      s
      (do
       (let [^String prefix (:per-line-prefix (first (getf :logical-blocks)))
             ^String l (first lines)]
         (if (= :buffering (getf :mode))
           (let [oldpos (getf :pos)
                 newpos (+ oldpos (count l))]
             (setf :pos newpos)
             (add-to-buffer this (make-buffer-blob l nil oldpos newpos))
             (write-buffered-output this))
           (do
             (write-white-space this)
             (write-to-base l)))
         (write-to-base (int \newline))
         (doseq [^String l (next (butlast lines))]
           (write-to-base l)
           (write-to-base ^String (pp-newline))
           (if prefix
             (write-to-base prefix)))
         ;; Upstream dead store (sets :buffering, never :mode): mirrored as-is.
         (setf :buffering :writing)
         (last lines))))))


(defn- p-write-char [this c]
  (let [c (int c)]
    (if (= (getf :mode) :writing)
      (do
        (write-white-space this)
        (write-to-base c))
      (if (= c \newline)
        (write-initial-lines this "\n")
        (let [oldpos (getf :pos)
              newpos (inc oldpos)]
          (setf :pos newpos)
          (add-to-buffer this (make-buffer-blob (str (char c)) nil oldpos newpos)))))))


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Initialize the pretty-writer instance
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;


(defn- pp-regex-whitespace?
  "Java regex \\s (space, tab, newline, vertical tab, form feed, return)."
  [c]
  (contains? #{\space \tab \newline \formfeed \return (char 11)} c))

(defn- pp-rtrim-whitespace
  "Upstream (.replaceFirst s0 \"\\\\s+$\" \"\"); no replaceFirst here, and
  Character/isWhitespace is a wider set than \\s, so trim the exact set."
  [^String s]
  (loop [i (dec (count s))]
    (if (and (>= i 0) (pp-regex-whitespace? (nth s i)))
      (recur (dec i))
      (subs s 0 (inc i)))))

(defn- ppw-write-string [this ^String x]
  ;; Upstream: the proxy (write [x]) String arm.
  (let [^String s0 (write-initial-lines this x)
        ^String s (pp-rtrim-whitespace s0)
        white-space (subs s0 (count s))
        mode (getf :mode)]
    (if (= mode :writing)
      (do
        (write-white-space this)
        (write-to-base s)
        (setf :trailing-white-space white-space))
      (let [oldpos (getf :pos)
            newpos (+ oldpos (count s0))]
        (setf :pos newpos)
        (add-to-buffer this (make-buffer-blob s white-space oldpos newpos))))))

(defn- ppw-write
  "Write x (a string or a char code) to a pretty-writer atom. Upstream: the
  proxy's (write [x]) condp on (class x) over String/Integer/Long."
  [this x]
  (if (string? x)
    (ppw-write-string this x)
    (p-write-char this x)))

(defn- pp-flush
  "Upstream (.ppflush ^PrettyFlush this): emit buffered tokens (buffering
  mode) or pending trailing whitespace (writing mode)."
  [this]
  (if (= (getf :mode) :buffering)
    (do
     (write-tokens this (getf :buffer) true)
     (setf :buffer []))
    (write-white-space this)))

(defn- pp-flush-wrapped
  "Upstream (.flush wrapped-stream) on a pretty writer: ppflush, then flush
  the real base writer underneath the column writer."
  [this]
  (pp-flush this)
  (.flush ^java.io.Writer (:base @(getf :base))))

(defn- pretty-writer [writer max-columns miser-width]
  ;; Upstream builds (proxy [Writer IDeref PrettyFlush]) over a ref of fields;
  ;; here the atom IS the writer (see column_writer.clj). The proxy's cbuf and
  ;; close arms had no callers and are dropped.
  (let [lb (struct logical-block nil nil (atom 0) (atom 0) (atom false) (atom false))]
    (atom {:pretty-writer true
           :base (column-writer writer max-columns)
           :logical-blocks lb
           :sections nil
           :mode :writing
           :buffer []
           :buffer-block lb
           :buffer-level 1
           :miser-width miser-width
           :trailing-white-space nil
           :pos 0})))


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;; Methods for pretty-writer
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

(defn- start-block
  [this
   ^String prefix ^String per-line-prefix ^String suffix]
  (do
   (let [lb (struct logical-block (getf :logical-blocks) nil (atom 0) (atom 0)
                    (atom false) (atom false)
                    prefix per-line-prefix suffix)]
     (setf :logical-blocks lb)
     (if (= (getf :mode) :writing)
       (do
         (write-white-space this)
          (when-let [cb (getf :logical-block-callback)] (cb :start))
          (if prefix
           (write-to-base prefix))
         (let [col (get-column (getf :base))]
           (reset! (:start-col lb) col)
           (reset! (:indent lb) col)))
       (let [oldpos (getf :pos)
             newpos (+ oldpos (if prefix (count prefix) 0))]
         (setf :pos newpos)
         (add-to-buffer this (make-start-block-t lb oldpos newpos)))))))

(defn- end-block [this]
  (do
   (let [lb (getf :logical-blocks)
         ^String suffix (:suffix lb)]
     (if (= (getf :mode) :writing)
       (do
         (write-white-space this)
         (if suffix
           (write-to-base suffix))
         (when-let [cb (getf :logical-block-callback)] (cb :end)))
       (let [oldpos (getf :pos)
             newpos (+ oldpos (if suffix (count suffix) 0))]
         (setf :pos newpos)
         (add-to-buffer this (make-end-block-t lb oldpos newpos))))
     (setf :logical-blocks (:parent lb)))))

(defn- nl [this type]
  (do
   (setf :mode :buffering)
   (let [pos (getf :pos)]
     (add-to-buffer this (make-nl-t type (getf :logical-blocks) pos pos)))))

(defn- indent [this relative-to offset]
  (do
   (let [lb (getf :logical-blocks)]
     (if (= (getf :mode) :writing)
       (do
         (write-white-space this)
         (reset! (:indent lb)
                  (+ offset (condp = relative-to
			      :block @(:start-col lb)
			      :current (get-column (getf :base))))))
       (let [pos (getf :pos)]
         (add-to-buffer this (make-indent-t lb relative-to offset pos pos)))))))

(defn- get-miser-width [this]
  (getf :miser-width))

(defn- set-miser-width [this new-miser-width]
  (setf :miser-width new-miser-width))

(defn- set-logical-block-callback [this f]
  (setf :logical-block-callback f))
