;;   Copyright (c) Rich Hickey. All rights reserved.
;;   The use and distribution terms for this software are covered by the
;;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;;   which can be found in the file epl-v10.html at the root of this distribution.
;;   By using this software in any fashion, you are agreeing to be bound by
;;   the terms of this license.
;;   You must not remove this notice, or any other, from this software.

;; Embedded S5D clojure.core composites. Boot loads this file with the
;; current namespace already clojure.core and every native installed,
;; so there is intentionally no ns form. Only the Control-table macros
;; that are neither specials nor analyzer builtins live here; assert,
;; time, and with-out-str resolve to analyzer builtins and must never
;; be redefined in this file. case/condp ARE real macros below
;; (case emits the case* analyzer form); lazy-seq expands to the
;; lazy-seq* analyzer form.
;;
;; Porting notes (vs src/clj/clojure/core.clj, which this adapts):
;; - and/or emit bare let* (our let never expands further).
;; - Emitted recur/if/do/def/quote/var are always bare: only bare
;;   non-shadowable specials analyze (qualified let/loop/fn are fine,
;;   so plain syntax-quote is used wherever templates need just those;
;;   ~'x splices a bare x otherwise).
;; - Emitted calls are built with list/apply, never cons: cons may be
;;   lazy while analysis only treats concrete lists as forms.
;; - if-not swaps its branches instead of calling not (same
;;   evaluation count/order, one less call).
;; - even? is out of the allowed vocabulary: cond->/cond->> shadow a
;;   local even? so the stock assert text is preserved, and
;;   cond->/cond->>/as-> step with loop instead of partition.
;; - Errors that need IllegalArgumentException on the JVM throw
;;   through the private throw-illegal-arg! native (no `new` exists);
;;   doseq's invalid-keyword path throws the stock steppair
;;   NullPointerException verbatim through throw-npe!.
;; - assert-args matches the JVM macro, except it renders the
;;   namespace via (ns-name *ns*): a raw *ns* str would print the
;;   port's #object[...] handle instead of the bare name.
;; - for/doseq are map/filter/take-while/mapcat composites (no chunk
;;   fns or atoms exist). Groups without :while fuse their :let/:when
;;   chain into one mapcat stage (single evaluation, JVM order);
;;   groups with :while keep the take-while-outside shape so :while
;;   tests stop after the first false exactly like the JVM. Two count
;;   caveats are inherent to the composite design: :let inits before
;;   a :while re-evaluate (pred plus element), and a :when before a
;;   :while tests eagerly per chunk instead of stopping with it.
;; - (for [] body) is (list body) (single evaluation, consistent with
;;   doseq); stock errors on 1.13 (nil binding spec) and returned nil
;;   pre-spec.
;; - dotimes truncates fractional n via (- n (mod n 1)), matching
;;   (long n) iteration counts. defonce keeps [name & body] and
;;   bound? (.hasRoot is inexpressible; extra/missing exprs are an
;;   accepted superset of the JVM's strict [name expr]).
;; - defn lands :arglists (with sigs-style prepost meta) via
;;   alter-meta!/assoc after a plain def; each clause's prepost
;;   map rides the emitted fn unchanged (fn enforces :pre/:post).
;; - case prep is a verbatim port minus the shift-mask search (no bit
;;   ops; :sparse is always correct), sorted-map (plain {}, order is
;;   eval-irrelevant), int/Integer casts (identity/literals), and
;;   into1/reduce1/ffirst (into/reduce/inlined); dispatch hashing is
;;   the private hash-code native (Util/hash, not murmur).

(defmacro ^{:private true} assert-args
  [& pairs]
  (let [test (first pairs)
        what (second pairs)
        more (nnext pairs)]
    (list 'do
          (list 'when-not test
                ;; `#'` (not a qualified call): expansions are
                ;; analyzed at the use site, where the private is
                ;; unreachable — the `var` form bypasses that (probed).
                (list (list 'var 'clojure.core/throw-illegal-arg!)
                      (list 'str (list 'first '&form)
                            " requires " what " in "
                            (list 'ns-name '*ns*)
                            ":" (list :line (list 'meta '&form)))))
          (if more
            (apply list 'assert-args more)))))

(defmacro and
  "Evaluates exprs one at a time, from left to right. If a form
  returns logical false (nil or false), and returns that value and
  doesn't evaluate any of the other expressions, otherwise it returns
  the value of the last expr. (and) returns true."
  {:added "1.0"}
  ([] true)
  ([x] x)
  ([x & next]
   (let [g (gensym "and")]
     (list 'let* (vector g x)
           (list 'if g (apply list 'clojure.core/and next) g)))))

(defmacro or
  "Evaluates exprs one at a time, from left to right. If a form
  returns a logical true value, or returns that value and doesn't
  evaluate any of the other expressions, otherwise it returns the
  value of the last expression. (or) returns nil."
  {:added "1.0"}
  ([] nil)
  ([x] x)
  ([x & next]
   (let [g (gensym "or")]
     (list 'let* (vector g x)
           (list 'if g g (apply list 'clojure.core/or next))))))

(defmacro when
  "Evaluates test. If logical true, evaluates body in an implicit do."
  {:added "1.0"}
  [test & body]
  (list 'if test (apply list 'do body)))

(defmacro when-not
  "Evaluates test. If logical false, evaluates body in an implicit do."
  {:added "1.0"}
  [test & body]
  (list 'if test nil (apply list 'do body)))

(defmacro if-not
  "Evaluates test. If logical false, evaluates and returns then expr,
  otherwise else expr, if supplied, else nil."
  {:added "1.0"}
  ([test then] (list 'clojure.core/if-not test then nil))
  ([test then else] (list 'if test else then)))

(defmacro if-let
  "bindings => binding-form test

  If test is true, evaluates then with binding-form bound to the value of
  test, if not, yields else"
  {:added "1.0"}
  ([bindings then]
   (list 'clojure.core/if-let bindings then nil))
  ([bindings then else & oldform]
   (assert-args
     (vector? bindings) "a vector for its binding"
     (nil? oldform) "1 or 2 forms after binding vector"
     (= 2 (count bindings)) "exactly 2 forms in binding vector")
   (let [form (bindings 0) tst (bindings 1)]
     `(let [temp# ~tst]
        (~'if temp# (let [~form temp#] ~then) ~else)))))

(defmacro when-let
  "bindings => binding-form test

  When test is true, evaluates body with binding-form bound to the value of test"
  {:added "1.0"}
  [bindings & body]
  (assert-args
    (vector? bindings) "a vector for its binding"
    (= 2 (count bindings)) "exactly 2 forms in binding vector")
  (let [form (bindings 0) tst (bindings 1)]
    `(let [temp# ~tst]
       (when temp#
         (let [~form temp#]
           ~@body)))))

(defmacro if-some
  "bindings => binding-form test

   If test is not nil, evaluates then with binding-form bound to the
   value of test, if not, yields else"
  {:added "1.6"}
  ([bindings then]
   (list 'clojure.core/if-some bindings then nil))
  ([bindings then else & oldform]
   (assert-args
     (vector? bindings) "a vector for its binding"
     (nil? oldform) "1 or 2 forms after binding vector"
     (= 2 (count bindings)) "exactly 2 forms in binding vector")
   (let [form (bindings 0) tst (bindings 1)]
     `(let [temp# ~tst]
        (~'if (nil? temp#)
          ~else
          (let [~form temp#]
            ~then))))))

(defmacro when-some
  "bindings => binding-form test

   When test is not nil, evaluates body with binding-form bound to the
   value of test"
  {:added "1.6"}
  [bindings & body]
  (assert-args
    (vector? bindings) "a vector for its binding"
    (= 2 (count bindings)) "exactly 2 forms in binding vector")
  (let [form (bindings 0) tst (bindings 1)]
    `(let [temp# ~tst]
       (~'if (nil? temp#)
         nil
         (let [~form temp#]
           ~@body)))))

(defmacro cond
  "Takes a set of test/expr pairs. It evaluates each test one at a
  time.  If a test returns logical true, cond evaluates and returns
  the value of the corresponding expr and doesn't evaluate any of the
  other tests or exprs. (cond) returns nil."
  {:added "1.0"}
  [& clauses]
  (when clauses
    (list 'if (first clauses)
          (if (next clauses)
            (second clauses)
            (throw-illegal-arg! "cond requires an even number of forms"))
          (apply list 'clojure.core/cond (next (next clauses))))))

(defmacro ->
  "Threads the expr through the forms. Inserts x as the
  second item in the first form, making a list of it if it is not a
  list already. If there are more forms, inserts the first form as the
  second item in second form, etc."
  {:added "1.0"}
  [x & forms]
  (loop [x x, forms forms]
    (if forms
      (let [form (first forms)
            threaded (if (seq? form)
                       (with-meta (apply list (first form) x (next form))
                         (meta form))
                       (list form x))]
        (recur threaded (next forms)))
      x)))

(defmacro ->>
  "Threads the expr through the forms. Inserts x as the
  last item in the first form, making a list of it if it is not a
  list already. If there are more forms, inserts the first form as the
  last item in second form, etc."
  {:added "1.1"}
  [x & forms]
  (loop [x x, forms forms]
    (if forms
      (let [form (first forms)
            threaded (if (seq? form)
                       (with-meta (apply list (first form)
                                         (concat (next form) (list x)))
                         (meta form))
                       (list form x))]
        (recur threaded (next forms)))
      x)))

(defmacro as->
  "Binds name to expr, evaluates the first form in the lexical context
  of that binding, then binds name to that result, repeating for each
  successive form, returning the result of the last form."
  {:added "1.5"}
  [expr name & forms]
  (if (empty? forms)
    `(let [~name ~expr]
       ~name)
    (let [mid (loop [fs (pop (vec forms)) out []]
                (if (empty? fs)
                  out
                  (recur (rest fs) (conj out name (first fs)))))]
      `(let [~name ~expr ~@mid]
         ~(last forms)))))

(defmacro cond->
  "Takes an expression and a set of test/form pairs. Threads expr (via ->)
  through each form for which the corresponding test
  expression is true. Note that, unlike cond branching, cond-> threading does
  not short circuit after the first true test expression."
  {:added "1.5"}
  [expr & clauses]
  ;; even? is out of the native vocabulary; a local with the same
  ;; name keeps the stock "Assert failed: (even? ...)" text.
  (let [even? (fn [n] (zero? (mod n 2)))]
    (assert (even? (count clauses))))
  (let [g (gensym)
        steps (loop [cs clauses acc []]
                (if cs
                  (let [test (first cs) step (second cs)]
                    (recur (nnext cs)
                           (conj acc `(~'if ~test (-> ~g ~step) ~g))))
                  acc))]
    (if (empty? steps)
      `(let [~g ~expr]
         ~g)
      (let [mid (loop [ss (pop steps) out []]
                  (if (empty? ss)
                    out
                    (recur (rest ss) (conj out g (first ss)))))]
        `(let [~g ~expr ~@mid]
           ~(peek steps))))))

(defmacro cond->>
  "Takes an expression and a set of test/form pairs. Threads expr (via ->>)
  through each form for which the corresponding test expression
  is true.  Note that, unlike cond branching, cond->> threading does not short circuit
  after the first true test expression."
  {:added "1.5"}
  [expr & clauses]
  ;; even? is out of the native vocabulary; a local with the same
  ;; name keeps the stock "Assert failed: (even? ...)" text.
  (let [even? (fn [n] (zero? (mod n 2)))]
    (assert (even? (count clauses))))
  (let [g (gensym)
        steps (loop [cs clauses acc []]
                (if cs
                  (let [test (first cs) step (second cs)]
                    (recur (nnext cs)
                           (conj acc `(~'if ~test (->> ~g ~step) ~g))))
                  acc))]
    (if (empty? steps)
      `(let [~g ~expr]
         ~g)
      (let [mid (loop [ss (pop steps) out []]
                  (if (empty? ss)
                    out
                    (recur (rest ss) (conj out g (first ss)))))]
        `(let [~g ~expr ~@mid]
           ~(peek steps))))))

(defmacro doto
  "Evaluates x then calls all of the functions with the
  value of x supplied at the front of the given arguments.  The forms
  are evaluated in order.  Returns x."
  {:added "1.0"}
  [x & forms]
  (let [gx (gensym)]
    `(let [~gx ~x]
       ~@(map (fn [f]
                (with-meta
                  (if (seq? f)
                    (apply list (first f) gx (next f))
                    (list f gx))
                  (meta f)))
              forms)
       ~gx)))

(defmacro comment
  "Ignores body, yields nil"
  {:added "1.0"}
  [& body])

(defmacro defn
  "Same as (def name (fn [params* ] exprs*)) or (def
   name (fn ([params* ] exprs*)+)) with any doc-string or attrs added
   to the var metadata. prepost-map defines a map with optional keys
   :pre and :post that contain collections of pre or post conditions."
  {:added "1.0"
   :arglists '([name doc-string? attr-map? [params*] prepost-map? body]
               [name doc-string? attr-map? ([params*] prepost-map? body)+ attr-map?])}
  [name & fdecl]
  (when-not (symbol? name)
    (throw-illegal-arg! "First argument to defn must be a symbol"))
  (let [doc (when (string? (first fdecl)) (first fdecl))
        fdecl (if (string? (first fdecl)) (next fdecl) fdecl)
        attr (when (map? (first fdecl)) (first fdecl))
        fdecl (if (map? (first fdecl)) (next fdecl) fdecl)
        fdecl (if (vector? (first fdecl)) (list fdecl) fdecl)
        attr2 (when (map? (last fdecl)) (last fdecl))
        fdecl (if (map? (last fdecl)) (take (dec (count fdecl)) fdecl) fdecl)
        attr (merge attr attr2)
        arglists (apply list
                        (map (fn [sig]
                               (let [args (first sig)
                                     args (if (= '&form (first args))
                                            (vec (drop 2 args))
                                            args)
                                     pp (second sig)]
                                 (if (and (map? pp) (nnext sig))
                                   (with-meta args (merge (meta args) pp))
                                   args)))
                             fdecl))
        fnform (apply list 'fn fdecl)
        defform (if doc
                  (list 'def name doc fnform)
                  (list 'def name fnform))
        metaform (list 'alter-meta! (list 'var name)
                       'assoc :arglists (list 'quote arglists))]
    (if (seq attr)
      (list 'do defform metaform
            (list 'alter-meta! (list 'var name)
                  'merge (list 'quote attr))
            (list 'var name))
      (list 'do defform metaform (list 'var name)))))

(defmacro defn-
  "same as defn, yielding non-public def"
  {:added "1.0"}
  [name & decls]
  (list* 'defn (with-meta name (assoc (meta name) :private true)) decls))

(defmacro defonce
  "defs name to have the root value of the expr iff the named var has no root value,
  else expr is unevaluated"
  {:added "1.0"}
  [name & body]
  `(let [v# (~'def ~name)]
     (when-not (bound? v#) (~'def ~name ~@body))))

(defmacro dotimes
  "bindings => name n

  Repeatedly executes body (presumably for side-effects) with name
  bound to integers from 0 through n-1."
  {:added "1.0"}
  [bindings & body]
  (assert-args
    (vector? bindings) "a vector for its binding"
    (= 2 (count bindings)) "exactly 2 forms in binding vector")
  (let [i (first bindings) n (second bindings)]
    `(let [n# ~n]
       ;; (- n (mod n 1)) truncates like (long n) for iteration counts.
       (let [n# (- n# (mod n# 1))]
         (loop [~i 0]
           (when (< ~i n#)
             ~@body
             (~'recur (inc ~i))))))))

(defmacro doseq
  "Repeatedly executes body (presumably for side-effects) with
  bindings and filtering as provided by \"for\".  Does not retain
  the head of the sequence. Returns nil."
  {:added "1.0"}
  [seq-exprs & body]
  (assert-args
    (vector? seq-exprs) "a vector for its binding"
    (zero? (mod (count seq-exprs) 2)) "an even number of forms in binding vector")
  (let [groups (loop [ps (seq seq-exprs) groups []]
                 (if ps
                   (let [k (first ps) v (second ps)]
                     (if (keyword? k)
                       (recur (nnext ps)
                              (conj (pop groups) (conj (peek groups) [k v])))
                       (recur (nnext ps) (conj groups [[k v]]))))
                   groups))]
    (if (empty? groups)
      (apply list 'do body)
      (let [firstg (first groups)
            pat (first (first firstg))
            xs (second (first firstg))
            mods (rest firstg)
            tmp (gensym)
            wrap-let (fn [lets inner]
                       (if (empty? lets) inner (list 'let lets inner)))
            scan (loop [ms mods w false t false]
                   (if (empty? ms)
                     [w t]
                     (let [k (first (first ms))]
                       (recur (rest ms)
                              (or w (= k :when))
                              (or t (= k :while))))))
            has-when (first scan)
            has-while (second scan)
            flat (loop [gs (rest groups) out []]
                   (if (empty? gs)
                     out
                     (let [gg (first gs)
                           bx (first gg)]
                       (recur (rest gs)
                              (loop [pp (rest gg)
                                     o (conj out (first bx) (second bx))]
                                (if (empty? pp)
                                  o
                                  (let [p (first pp)]
                                    (recur (rest pp)
                                           (conj o (first p) (second p))))))))))
            core (if (empty? (rest groups))
                   (apply list 'do body)
                   (apply list 'doseq flat body))]
        (if (and has-when (not has-while))
          ;; Fuse the :let/:when chain into one mapcat stage, so lets
          ;; and tests evaluate exactly once per element, in order.
          ;; The nest builds inside-out, hence the reversed walk.
          (let [guard (loop [ms (reverse mods) inner (list 'list core)]
                        (if (empty? ms)
                          inner
                          (let [k (first (first ms)) v (second (first ms))]
                            (cond (= k :let)
                                  (recur (rest ms) (list 'let v inner))
                                  (= k :when)
                                  (recur (rest ms) (list 'if v inner nil))
                                  :else
                                  (throw-npe! "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"steppair\" is null")))))
                element (list 'let (vector pat tmp) guard)]
            (list 'dorun (list 'mapcat (list 'fn (vector tmp) element) xs)))
          ;; Take-while-outside: :while stops after the first false,
          ;; exactly like the JVM.
          (let [pair (loop [ms mods se xs ls []]
                       (if (empty? ms)
                         [se ls]
                         (let [k (first (first ms)) v (second (first ms))]
                           (cond (= k :let)
                                 (recur (rest ms) se (into ls v))
                                 (= k :when)
                                 (recur (rest ms)
                                        (list 'filter
                                              (list 'fn (vector tmp)
                                                    (list 'let (vector pat tmp)
                                                          (wrap-let ls v)))
                                              se)
                                        ls)
                                 (= k :while)
                                 (recur (rest ms)
                                        (list 'take-while
                                              (list 'fn (vector tmp)
                                                    (list 'let (vector pat tmp)
                                                          (wrap-let ls v)))
                                              se)
                                        ls)
                                 :else
                                 (throw-npe! "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"steppair\" is null")))))
                fseq (first pair)
                lets (second pair)
                element (list 'let (vector pat tmp) (wrap-let lets core))]
            (list 'dorun (list 'map (list 'fn (vector tmp) element) fseq))))))))

(defmacro for
  "List comprehension. Takes a vector of one or more
   binding-form/collection-expr pairs, each followed by zero or more
   modifiers, and yields a lazy sequence of evaluations of expr.
   Collections are iterated in a nested fashion, rightmost fastest,
   and nested coll-exprs can refer to bindings created in prior
   binding-forms.  Supported modifiers are: :let [binding-form expr ...],
   :while test, :when test.

  (take 100 (for [x (range 100000000) y (range 1000000) :while (< y x)] [x y]))"
  {:added "1.0"}
  [seq-exprs body-expr]
  (assert-args
    (vector? seq-exprs) "a vector for its binding"
    (zero? (mod (count seq-exprs) 2)) "an even number of forms in binding vector")
  (let [groups (loop [ps (seq seq-exprs) groups []]
                 (if ps
                   (let [k (first ps) v (second ps)]
                     (if (keyword? k)
                       (recur (nnext ps)
                              (conj (pop groups) (conj (peek groups) [k v])))
                       (recur (nnext ps) (conj groups [[k v]]))))
                   groups))]
    (if (empty? groups)
      (list 'list body-expr)
      (let [firstg (first groups)
            pat (first (first firstg))
            xs (second (first firstg))
            mods (rest firstg)
            tmp (gensym)
            wrap-let (fn [lets inner]
                       (if (empty? lets) inner (list 'let lets inner)))
            scan (loop [ms mods w false t false]
                   (if (empty? ms)
                     [w t]
                     (let [k (first (first ms))]
                       (recur (rest ms)
                              (or w (= k :when))
                              (or t (= k :while))))))
            has-when (first scan)
            has-while (second scan)
            flat (loop [gs (rest groups) out []]
                   (if (empty? gs)
                     out
                     (let [gg (first gs)
                           bx (first gg)]
                       (recur (rest gs)
                              (loop [pp (rest gg)
                                     o (conj out (first bx) (second bx))]
                                (if (empty? pp)
                                  o
                                  (let [p (first pp)]
                                    (recur (rest pp)
                                           (conj o (first p) (second p))))))))))
            core (if (empty? (rest groups))
                   body-expr
                   (apply list 'for flat (list body-expr)))]
        (if (and has-when (not has-while))
          ;; Fuse the :let/:when chain into one mapcat stage, so lets
          ;; and tests evaluate exactly once per element, in order.
          ;; The nest builds inside-out, hence the reversed walk. A
          ;; nested core is already a seq, so only a single-group
          ;; body gets wrapped for the concat.
          (let [seed (if (empty? (rest groups)) (list 'list core) core)
                guard (loop [ms (reverse mods) inner seed]
                        (if (empty? ms)
                          inner
                          (let [k (first (first ms)) v (second (first ms))]
                            (cond (= k :let)
                                  (recur (rest ms) (list 'let v inner))
                                  (= k :when)
                                  (recur (rest ms) (list 'if v inner nil))
                                  :else
                                  (throw-illegal-arg! (str "Invalid 'for' keyword " k))))))
                element (list 'let (vector pat tmp) guard)]
            (list 'mapcat (list 'fn (vector tmp) element) xs))
          ;; Take-while-outside: :while stops after the first false,
          ;; exactly like the JVM.
          (let [pair (loop [ms mods se xs ls []]
                       (if (empty? ms)
                         [se ls]
                         (let [k (first (first ms)) v (second (first ms))]
                           (cond (= k :let)
                             (recur (rest ms) se (into ls v))
                             (= k :when)
                             (recur (rest ms)
                                    (list 'filter
                                          (list 'fn (vector tmp)
                                                (list 'let (vector pat tmp)
                                                      (wrap-let ls v)))
                                          se)
                                    ls)
                             (= k :while)
                             (recur (rest ms)
                                    (list 'take-while
                                          (list 'fn (vector tmp)
                                                (list 'let (vector pat tmp)
                                                      (wrap-let ls v)))
                                          se)
                                    ls)
                             :else
                             (throw-illegal-arg! (str "Invalid 'for' keyword " k))))))
                fseq (first pair)
                lets (second pair)
                element (list 'let (vector pat tmp) (wrap-let lets core))
                mapper (list 'fn (vector tmp) element)]
            (if (empty? (rest groups))
              (list 'map mapper fseq)
              (list 'mapcat mapper fseq))))))))

(defmacro lazy-seq
  "Takes a body of expressions that returns an ISeq or nil, and yields
  a Seqable object that will invoke the body only the first time seq
  is called, and will cache the result and return it on all subsequent
  seq calls. See also - realized?"
  {:added "1.0"}
  [& body]
  (list* 'lazy-seq* body))

(defmacro lazy-cat
  "Expands to code which yields a lazy sequence of the concatenation
  of the supplied colls.  Each coll expr is not evaluated until it is
  needed.

  (lazy-cat xs ys zs) === (concat (lazy-seq xs) (lazy-seq ys) (lazy-seq zs))"
  {:added "1.0"}
  [& colls]
  `(lazy-seq (concat ~@colls)))

(defmacro delay
  "Takes a body of expressions and yields a Delay object that will
  invoke the body only the first time it is forced (with force or deref/@), and
  will cache the result and return it on all subsequent force
  calls. See also - realized?"
  {:added "1.0"}
  [& body]
  (list* 'delay* body))

(defn memoize
  "Returns a memoized version of a referentially transparent function. The
  memoized version of the function keeps a cache of the mapping from arguments
  to results and, when calls with the same arguments are repeated often, has
  higher performance at the expense of higher memory use."
  {:added "1.0"}
  [f]
  (let [mem (atom {})]
    (fn [& args]
      (if-let [e (find @mem args)]
        (val e)
        (let [ret (apply f args)]
          (swap! mem assoc args ret)
          ret)))))

(defmacro future
  "Takes a body of expressions and yields a future object that will
  invoke the body in another thread, and will cache the result and
  return it on all subsequent calls to deref/@. If the computation has
  not yet finished, calls to deref/@ will block, unless the variant of
  deref with timeout is used. See also - realized?."
  {:added "1.1"}
  [& body]
  (list 'future-call (list* 'fn [] body)))

;; --- `pmap`/`pcalls`/`pvalues` (S6D): verbatim ports, except the
;; lookahead's `(.. Runtime getRuntime availableProcessors)` is a
;; fixed count (interop is N1; the lookahead only paces future
;; creation — timing-only — while `future-call` conveys bindings).

(defn pmap
  "Like map, except f is applied in parallel. Semi-lazy in that the
  parallel computation stays ahead of the consumption, but doesn't
  realize the entire result unless required. Only useful for
  computationally intensive functions where the time of f dominates
  the coordination overhead."
  {:added "1.0"}
  ([f coll]
   (let [n (+ 2 4)
         rets (map #(future (f %)) coll)
         step (fn step [[x & xs :as vs] fs]
                (lazy-seq
                 (if-let [s (seq fs)]
                   (cons (deref x) (step xs (rest s)))
                   (map deref vs))))]
     (step rets (drop n rets))))
  ([f coll & colls]
   (let [step (fn step [cs]
                (lazy-seq
                 (let [ss (map seq cs)]
                   (when (every? identity ss)
                     (cons (map first ss) (step (map rest ss)))))))]
     (pmap #(apply f %) (step (cons coll colls))))))

(defn pcalls
  "Executes the no-arg fns in parallel, returning a lazy sequence of
  their values"
  {:added "1.0"}
  [& fns] (pmap #(%) fns))

(defmacro pvalues
  "Returns a lazy sequence of the values of the exprs, which are
  evaluated in parallel"
  {:added "1.0"}
  [& exprs]
  `(pcalls ~@(map #(list `fn [] %) exprs)))

;; --- `case`/`condp` (S5D): verbatim ports, except `condp`/`case`
;; throw through `throw-illegal-arg!` (no `new` exists), `Util/hash`
;; is the private `hash-code` native, `into1`/`reduce1` are
;; `into`/`reduce`, `ffirst` is inlined, `int` is `identity`
;; (tests are already int-sized; pr-identical), `Integer/MIN_VALUE`
;; is an inlined literal, the imap builds over `{}` (`sorted-map`
;; is out of scope; order is eval-irrelevant), and the shift-mask
;; search is skipped (no bit ops; `:sparse` is always correct).

(def ^:private max-switch-table-size 8192) ;; (bit-shift-left 1 13)

(defn- fits-table?
  "Returns true if the collection of ints can fit within the
  max-table-switch-size, false otherwise."
  [ints]
  (< (- (apply max (seq ints)) (apply min (seq ints))) max-switch-table-size))

(defn- case-map
  "Transforms a sequence of test constants and a corresponding sequence of then
  expressions into a sorted map to be consumed by case*. The form of the map
  entries are {(case-f test) [(test-f test) then]}."
  [case-f test-f tests thens]
  (into {}
    (zipmap (map case-f tests)
            (map vector
              (map test-f tests)
              thens))))

(defn- prep-ints
  "Takes a sequence of int-sized test constants and a corresponding sequence of
  then expressions. Returns a tuple of [shift mask case-map switch-type] where
  case-map is a map of int case values to [test then] tuples, and switch-type
  is either :sparse or :compact."
  [tests thens]
  (if (fits-table? tests)
    ; compact case ints, no shift-mask
    [0 0 (case-map identity identity tests thens) :compact]
    ; sparse case ints, no shift-mask
    [0 0 (case-map identity identity tests thens) :sparse]))

(defn- merge-hash-collisions
  "Takes a case expression, default expression, and a sequence of test constants
  and a corresponding sequence of then expressions. Returns a tuple of
  [tests thens skip-check-set] where no tests have the same hash. Each set of
  input test constants with the same hash is replaced with a single test
  constant (the case int), and their respective thens are combined into:
  (condp = expr
    test-1 then-1
    ...
    test-n then-n
    default).
  The skip-check is a set of case ints for which post-switch equivalence
  checking must not be done (the cases holding the above condp thens)."
  [expr-sym default tests thens]
  (let [buckets (loop [m {} ks tests vs thens]
                  (if (and ks vs)
                    (recur
                      (update m (hash-code (first ks)) (fnil conj []) [(first ks) (first vs)])
                      (next ks) (next vs))
                    m))
        assoc-multi (fn [m h bucket]
                      (let [testexprs (mapcat (fn [kv] [(list 'quote (first kv)) (second kv)]) bucket)
                            expr `(condp = ~expr-sym ~@testexprs ~default)]
                        (assoc m h expr)))
        hmap (reduce
               (fn [m [h bucket]]
                 (if (== 1 (count bucket))
                   (assoc m (first (first bucket)) (second (first bucket)))
                   (assoc-multi m h bucket)))
               {} buckets)
        skip-check (->> buckets
                     (filter #(< 1 (count (second %))))
                     (map first)
                     (into #{}))]
    [(keys hmap) (vals hmap) skip-check]))

(defn- prep-hashes
  "Takes a sequence of test constants and a corresponding sequence of then
  expressions. Returns a tuple of [shift mask case-map switch-type skip-check]
  where case-map is a map of int case values to [test then] tuples, switch-type
  is either :sparse or :compact, and skip-check is a set of case ints for which
  post-switch equivalence checking must not be done (occurs with hash
  collisions)."
  [expr-sym default tests thens]
  (let [hashcode #(hash-code %)
        hashes (into #{} (map hashcode tests))]
    (if (== (count tests) (count hashes))
      (if (fits-table? hashes)
        ; compact case ints, no shift-mask
        [0 0 (case-map hashcode identity tests thens) :compact]
        ; sparse case ints, no shift-mask
        [0 0 (case-map hashcode identity tests thens) :sparse])
      ; resolve hash collisions and try again
      (let [[tests thens skip-check] (merge-hash-collisions expr-sym default tests thens)
            [shift mask case-map switch-type] (prep-hashes expr-sym default tests thens)]
        ; (the shift-mask skip-check remap is dead: mask is always 0)
        [shift mask case-map switch-type skip-check]))))

(defmacro condp
  "Takes a binary predicate, an expression, and a set of clauses.
  Each clause can take the form of either:

  test-expr result-expr

  test-expr :>> result-fn

  Note :>> is an ordinary keyword.

  For each clause, (pred test-expr expr) is evaluated. If it returns
  logical true, the clause is a match. If a binary clause matches, the
  result-expr is returned, if a ternary clause matches, its result-fn,
  which must be a unary function, is called with the result of the
  predicate as its argument, the result of that call being the return
  value of condp. A single default expression can follow the clauses,
  and its value will be returned if no clause matches. If no default
  expression is provided and no clause matches, an
  IllegalArgumentException is thrown."
  {:added "1.0"}

  [pred expr & clauses]
  (let [gpred (gensym "pred__")
        gexpr (gensym "expr__")
        emit (fn emit [pred expr args]
               (let [[[a b c :as clause] more]
                       (split-at (if (= :>> (second args)) 3 2) args)
                     n (count clause)]
                 (cond
                  (= 0 n) `((var throw-illegal-arg!) (str "No matching clause: " ~expr))
                  (= 1 n) a
                  (= 2 n) `(if (~pred ~a ~expr)
                             ~b
                             ~(emit pred expr more))
                  :else `(if-let [p# (~pred ~a ~expr)]
                           (~c p#)
                           ~(emit pred expr more)))))]
    `(let [~gpred ~pred
           ~gexpr ~expr]
       ~(emit gpred gexpr clauses))))

(defmacro case
  "Takes an expression, and a set of clauses.

  Each clause can take the form of either:

  test-constant result-expr

  (test-constant1 ... test-constantN)  result-expr

  The test-constants are not evaluated. They must be compile-time
  literals, and need not be quoted.  If the expression is equal to a
  test-constant, the corresponding result-expr is returned. A single
  default expression can follow the clauses, and its value will be
  returned if no clause matches. If no default expression is provided
  and no clause matches, an IllegalArgumentException is thrown.

  Unlike cond and condp, case does a constant-time dispatch, the
  clauses are not considered sequentially.  All manner of constant
  expressions are acceptable in case, including numbers, strings,
  symbols, keywords, and (Clojure) composites thereof. Note that since
  lists are used to group multiple constants that map to the same
  expression, a vector can be used to match a list if needed. The
  test-constants need not be all of the same type."
  {:added "1.2"}

  [e & clauses]
  (let [ge (with-meta (gensym) '{:tag Object})
        default (if (odd? (count clauses))
                  (last clauses)
                  `((var throw-illegal-arg!) (str "No matching clause: " ~ge)))]
    (if (> 2 (count clauses))
      `(let [~ge ~e] ~default)
      (let [pairs (partition 2 clauses)
            assoc-test (fn assoc-test [m test expr]
                         (if (contains? m test)
                           (throw-illegal-arg! (str "Duplicate case test constant: " test))
                           (assoc m test expr)))
            pairs (reduce
                       (fn [m [test expr]]
                         (if (seq? test)
                           (reduce #(assoc-test %1 %2 expr) m test)
                           (assoc-test m test expr)))
                       {} pairs)
            tests (keys pairs)
            thens (vals pairs)
            mode (cond
                   (every? #(and (integer? %) (<= -2147483648 % 2147483647)) tests)
                   :ints
                   (every? keyword? tests)
                   :identity
                   :else :hashes)]
        (condp = mode
          :ints
          (let [[shift mask imap switch-type] (prep-ints tests thens)]
            `(let [~ge ~e] (case* ~ge ~shift ~mask ~default ~imap ~switch-type :int)))
          :hashes
          (let [[shift mask imap switch-type skip-check] (prep-hashes ge default tests thens)]
            `(let [~ge ~e] (case* ~ge ~shift ~mask ~default ~imap ~switch-type :hash-equiv ~skip-check)))
          :identity
          (let [[shift mask imap switch-type skip-check] (prep-hashes ge default tests thens)]
            `(let [~ge ~e] (case* ~ge ~shift ~mask ~default ~imap ~switch-type :hash-identity ~skip-check))))))))

;; --- `random-sample` (S-rng-tower): verbatim composition over
;; `filter` + `rand` (zero Rust — the lazy semantics ride free).
(defn random-sample
  ([prob] (filter (fn [_] (< (rand) prob))))
  ([prob coll] (filter (fn [_] (< (rand) prob)) coll)))

;; --- Corelib small vars (zero-oos S4a, #46): control macros,
;; printer one-liners, `rand-nth`, and `test` — verbatim ports
;; over implemented vars (the three `parse-*` fns are `core.rs`
;; natives for JVM parse parity, not here). Owned by S4a: no
;; other slice edits inside this block.
(defmacro some->
  "When expr is not nil, threads it into the first form (via ->),
  and when that result is not nil, through the next etc"
  {:added "1.5"}
  [expr & forms]
  (let [g (gensym)
        steps (map (fn [step] `(~'if (nil? ~g) nil (-> ~g ~step)))
                   forms)]
    `(let [~g ~expr
           ~@(interleave (repeat g) (butlast steps))]
       ~(if (empty? steps)
          g
          (last steps)))))

(defmacro some->>
  "When expr is not nil, threads it into the first form (via ->>),
  and when that result is not nil, through the next etc"
  {:added "1.5"}
  [expr & forms]
  (let [g (gensym)
        steps (map (fn [step] `(~'if (nil? ~g) nil (->> ~g ~step)))
                   forms)]
    `(let [~g ~expr
           ~@(interleave (repeat g) (butlast steps))]
       ~(if (empty? steps)
          g
          (last steps)))))

(defmacro when-first
  "bindings => x xs

  Roughly the same as (when (seq xs) (let [x (first xs)] body)) but xs is evaluated only once"
  {:added "1.0"}
  [bindings & body]
  (assert-args
     (vector? bindings) "a vector for its binding"
     (= 2 (count bindings)) "exactly 2 forms in binding vector")
  (let [[x xs] bindings]
    `(when-let [xs# (seq ~xs)]
       (let [~x (first xs#)]
         ~@body))))

(defmacro while
  "Repeatedly executes body while test expression is true. Presumes
  some side-effect will cause test to become false/nil. Returns nil"
  {:added "1.0"}
  [test & body]
  `(loop []
     (when ~test
       ~@body
       (~'recur))))

(defn printf
  "Prints formatted output, as per format"
  {:added "1.0"}
  [fmt & args]
  (print (apply format fmt args)))

(defn println-str
  "println to a string, returning it"
  {:added "1.0"}
  [& xs]
  (with-out-str
    (apply println xs)))

(defn prn-str
  "prn to a string, returning it"
  {:added "1.0"}
  [& xs]
  (with-out-str
    (apply prn xs)))

(defn rand-nth
  "Return a random element of the (sequential) collection. Will have
  the same performance characteristics as nth for the given
  collection."
  {:added "1.2"}
  [coll]
  (nth coll (rand-int (count coll))))

(defn test
  "test [v] finds fn at key :test in var metadata and calls it,
  presuming failure will throw exception"
  {:added "1.0"}
  [v]
  (let [f (:test (meta v))]
    (if f
      (do (f) :ok)
      :no-test)))

;; --- Types (S-typechain stage 1): `deftype` / `defrecord` / `reify` /
;; `proxy` / `gen-class` / `gen-interface` / `definterface` expand to
;; the hidden `__deftype*` et al natives (methods become `(fn [this
;; …] …)` closures; the namespace is the registry, so constructors
;; resolve through ordinary vars). `definterface` sigs exclude
;; `this` (it would be a phantom parameter); `proxy` bodies exclude
;; it too; `deftype` / `reify` bodies include it (oracle pins).

(defn- check-type-name [kind name]
  (when-not (symbol? name)
    (throw-illegal-arg! (str "First argument to " kind " must be a symbol")))
  (when (or (namespace name) (some #{\.} (str name)))
    (throw-illegal-arg! (str kind " names must be simple symbols")))
  name)

(defn- check-type-fields [kind fields]
  (when-not (and (vector? fields)
                 (every? (fn [f] (and (symbol? f) (nil? (namespace f)))) fields))
    (throw-illegal-arg! (str kind " fields must be a vector of simple symbols")))
  (mapv (fn [f] (symbol (name f))) fields))

(defn- type-method-argc [kind argv m]
  (when-not (vector? argv)
    (throw-illegal-arg! (str "Invalid method argv: " (pr-str argv))))
  ;; `proxy` bodies exclude `this` (oracle `proxy-str`); every other
  ;; kind counts past it.
  (let [args (if (= kind "proxy") (seq argv) (rest argv))]
    (when (some #{'&} args)
      (throw-illegal-arg! (str "Can't define variadic method: " m)))
    (count args)))

(defn- method-triples [kind mname bodies]
  (when-not (and (symbol? mname) (seq bodies))
    (throw-illegal-arg! (str "Invalid " kind " method: " (pr-str (apply list mname bodies)))))
  (if (vector? (first bodies))
    (do (type-method-argc kind (first bodies) mname)
        [[mname (first bodies) (rest bodies)]])
    (do (doseq [b bodies]
          (when-not (and (seq? b) (vector? (first b)))
            (throw-illegal-arg! (str "Invalid " kind " method: " (pr-str (apply list mname bodies))))))
        (map (fn [b] [mname (first b) (rest b)]) bodies))))

(defn- emit-type-methods [kind triples]
  (apply list 'clojure.core/hash-map
    (mapcat (fn [[m entries]]
              (list (str m)
                    (apply list 'clojure.core/hash-map
                      (mapcat (fn [[_ argv body]]
                                (list (type-method-argc kind argv m)
                                      (apply list 'fn argv body)))
                              entries))))
            (group-by first triples))))

(defn- finish-type-def [name doc attr init]
  ;; The attr map evaluates at runtime (like the JVM — computed
  ;; entries resolve there, not as quoted forms).
  (let [defform (if doc (list 'def name doc init) (list 'def name init))]
    (if (seq attr)
      (list 'do defform
        (list 'alter-meta! (list 'var name) 'merge attr)
        name)
      (list 'do defform name))))

(defn- parse-type-head [kind name args]
  (check-type-name kind name)
  (let [[doc args] (if (string? (first args)) [(first args) (rest args)] [nil args])
        [attr args] (if (map? (first args)) [(first args) (rest args)] [nil args])]
    {:doc doc :attr attr
     :fields (check-type-fields kind (first args))
     :specs (rest args)}))

(defn- parse-iface-head [kind name args]
  (check-type-name kind name)
  (let [[doc args] (if (string? (first args)) [(first args) (rest args)] [nil args])
        [attr args] (if (map? (first args)) [(first args) (rest args)] [nil args])]
    {:doc doc :attr attr :sigs args}))

(defn- parse-type-specs [kind specs]
  (loop [ss specs, cur nil, ifaces [], triples []]
    (if (empty? ss)
      {:ifaces ifaces :triples triples}
      (let [s (first ss)]
        (cond (symbol? s)
              (recur (rest ss) s (conj ifaces s) triples)
              (keyword? s)
              (throw-illegal-arg! (str "Unsupported " kind " option: " s))
              (seq? s)
              (do (when (nil? cur)
                    (throw-illegal-arg! (str "Can't define method not in interfaces: " (first s))))
                  (recur (rest ss) cur ifaces
                    (into triples (method-triples kind (first s) (rest s)))))
              :else
              (throw-illegal-arg! (str "Invalid " kind " spec: " (pr-str s))))))))

(defmacro deftype [name & args]
  (let [{:keys [doc attr fields specs]} (parse-type-head "deftype" name args)
        {:keys [ifaces triples]} (parse-type-specs "deftype" specs)]
    (finish-type-def name doc attr
      (list 'clojure.core/__deftype*
        (list 'str (list 'ns-name '*ns*))
        (str name)
        (list 'quote fields)
        (emit-type-methods "deftype" triples)
        (list 'quote (vec ifaces))))))

(defmacro defrecord [name & args]
  (let [{:keys [doc attr fields specs]} (parse-type-head "defrecord" name args)
        {:keys [ifaces triples]} (parse-type-specs "defrecord" specs)
        arrow (symbol (str "->" name))
        map-arrow (symbol (str "map->" name))]
    (list 'do
      (finish-type-def name doc attr
        (list 'clojure.core/__record*
          (list 'str (list 'ns-name '*ns*))
          (str name)
          (list 'quote fields)
          (emit-type-methods "defrecord" triples)
          (list 'quote (vec ifaces))))
      (list 'def arrow
        (list 'let ['t name]
          (list 'fn arrow fields
            (apply list 'clojure.core/__new* 't fields))))
      (list 'def map-arrow
        (list 'let ['t name]
          (list 'fn map-arrow '[m]
            (list 'clojure.core/__record_map* 't 'm))))
      name)))

(defmacro reify [& specs]
  (let [[things methods] (split-with symbol? specs)
        triples (mapcat (fn [m] (method-triples "reify" (first m) (rest m))) methods)]
    (list 'clojure.core/__reify*
      (list 'quote (vec things))
      (emit-type-methods "reify" triples))))

(defmacro proxy [super args & methods]
  (when-not (and (vector? super) (seq super) (every? symbol? super))
    (throw-illegal-arg! "proxy superclass must be a nonempty vector of symbols"))
  (when-not (vector? args)
    (throw-illegal-arg! "proxy args must be a vector"))
  (let [triples (mapcat (fn [m] (method-triples "proxy" (first m) (rest m))) methods)]
    (list 'clojure.core/__proxy*
      (list 'quote (first super))
      (list 'quote (vec (rest super)))
      args
      (emit-type-methods "proxy" triples))))

;; S14: call the superclass method from a proxy method body. The
;; call takes the port `.meth` shape (the JVM's `.`-head
;; `(. this meth …)` interop form does not exist here); like the
;; JVM it captures `this`, which the caller binds, since port
;; proxy fns exclude it.
(defmacro proxy-super [meth & args]
  (list 'clojure.core/proxy-call-with-super
    (list 'clojure.core/fn []
      (apply list (symbol (str "." (name meth))) 'this args))
    'this
    (name meth)))

(defmacro definterface [name & args]
  (let [{:keys [doc attr sigs]} (parse-iface-head "definterface" name args)]
    (doseq [s sigs]
      (when-not (and (seq? s) (= 2 (count s)) (symbol? (first s)) (vector? (second s)))
        (throw-illegal-arg! (str "Invalid interface signature: " (pr-str s)))))
    (let [grouped (group-by first sigs)]
      (finish-type-def name doc attr
        (list 'clojure.core/__interface*
          (list 'str (list 'ns-name '*ns*))
          (str name)
          (apply list 'clojure.core/hash-map
            (mapcat (fn [[m entries]]
                      (list (str m) (vec (map (fn [e] (count (second e))) entries))))
                    grouped)))))))

(defn- simple-class-name [raw]
  (apply str (reverse (take-while (fn [c] (not= c \.)) (reverse (seq raw))))))

(defmacro gen-interface [& opts]
  (let [m (apply hash-map opts)]
    (when-not (contains? m :name)
      (throw-illegal-arg! "Must supply :name to gen-interface"))
    (doseq [k (keys m)]
      (when-not (#{:name :methods} k)
        (throw-illegal-arg! (str "Unsupported gen-interface option: " k))))
    (when-not (or (symbol? (:name m)) (string? (:name m)))
      (throw-illegal-arg! "gen-interface :name must be a symbol or string"))
    (let [raw (str (:name m))
          methods (get m :methods [])]
      (doseq [e methods]
        (when-not (and (or (seq? e) (vector? e)) (= 3 (count e)) (symbol? (first e)) (vector? (second e)))
          (throw-illegal-arg! (str "Invalid gen-interface method: " (pr-str e)))))
      (let [grouped (group-by first methods)
            simple (symbol (simple-class-name raw))]
        (finish-type-def simple nil nil
          (list 'clojure.core/__interface*
            (list 'str (list 'ns-name '*ns*))
            raw
            (apply list 'clojure.core/hash-map
              (mapcat (fn [[n entries]]
                        (list (str n) (vec (map (fn [e] (count (second e))) entries))))
                      grouped))))))))

(defmacro gen-class [& opts]
  (let [m (apply hash-map opts)]
    (when-not (contains? m :name)
      (throw-illegal-arg! "Must supply :name to gen-class"))
    (doseq [k (keys m)]
      (when-not (#{:name :prefix :methods :implements :extends} k)
        (throw-illegal-arg! (str "Unsupported gen-class option: " k))))
    (when-not (or (symbol? (:name m)) (string? (:name m)))
      (throw-illegal-arg! "gen-class :name must be a symbol or string"))
    (let [raw (str (:name m))
          simple (simple-class-name raw)
          prefix (get m :prefix (str simple "-"))]
      (when-not (string? prefix)
        (throw-illegal-arg! "gen-class :prefix must be a string"))
      (let [methods (get m :methods [])
            implements (get m :implements [])]
        (doseq [e methods]
          (when-not (and (or (seq? e) (vector? e)) (= 3 (count e)) (symbol? (first e)) (vector? (second e)))
            (throw-illegal-arg! (str "Invalid gen-class method: " (pr-str e)))))
        (doseq [i implements]
          (when-not (symbol? i)
            (throw-illegal-arg! (str "Invalid gen-class interface: " (pr-str i)))))
        (when (and (contains? m :extends) (not= 'Object (:extends m)))
          (throw-illegal-arg! (str "Cannot extend " (:extends m) ": gen-class supports Object only")))
        (let [grouped (group-by first methods)
              class-expr (if (some #{\.} (seq raw))
                           raw
                           (list 'str (list 'ns-name '*ns*) "." simple))]
          (finish-type-def (symbol simple) nil nil
            (list 'clojure.core/__genclass*
              class-expr
              prefix
              (apply list 'clojure.core/hash-map
                (mapcat (fn [[n entries]]
                          (list (str n) (vec (map (fn [e] (count (second e))) entries))))
                        grouped))
              (list 'str (list 'ns-name '*ns*))
              (list 'quote (vec implements)))))))))

(defn- parse-protocol-sigs [sigs]
  (map (fn [s]
         (when-not (and (seq? s) (symbol? (first s)) (seq (rest s)))
           (throw-illegal-arg! (str "Invalid protocol signature: " (pr-str s))))
         (let [m (first s)
               body (rest s)
               [arglists doc] (if (vector? (first body))
                                [(apply list (take-while vector? body))
                                 (let [d (first (drop-while vector? body))]
                                   (when (string? d) d))]
                                [(apply list (map first body)) nil])]
           (doseq [a arglists]
             (when-not (and (vector? a) (seq a))
               (throw-illegal-arg! (str "Invalid protocol signature: " (pr-str s)))))
           {:m m :arglists arglists :doc doc}))
       sigs))

(defmacro defprotocol [name & args]
  (check-type-name "defprotocol" name)
  (let [[doc args] (if (string? (first args)) [(first args) (rest args)] [nil args])
        parsed (parse-protocol-sigs args)
        iface-sym (symbol (str name "-interface"))
        ns-str (list 'str (list 'ns-name '*ns*))
        iface-sigs (apply list 'clojure.core/hash-map
                     (mapcat (fn [{:keys [m arglists]}]
                               (list (str m) (vec (map (fn [a] (dec (count a))) arglists))))
                             parsed))
        proto-sigs (apply list 'clojure.core/hash-map
                     (mapcat (fn [{:keys [m arglists]}]
                               (list (keyword m)
                                     (list 'clojure.core/hash-map
                                       :arglists (list 'quote arglists))))
                             parsed))
        ;; One variadic arity per method: every call reaches
        ;; `__protocol_invoke*`, so its oracle arity error (not the
        ;; fn's own `ArityException`) fires on bad counts. Each
        ;; `defn` is followed by its S14 `__protocol_method*`
        ;; registration (the `-cache-protocol-fn` cache identity).
        fns (mapcat (fn [{:keys [m doc]}]
                      [(apply list 'clojure.core/defn m
                         (concat (when doc [doc])
                                 (list (list '[this & args]
                                         (list 'clojure.core/__protocol_invoke*
                                           (list 'var name) (str m)
                                           (list 'clojure.core/into '[this] 'args))))))
                       (list 'clojure.core/__protocol_method*
                         (list 'var name) (str m) m)])
                    parsed)]
    (list 'do
      (list 'def iface-sym
        (list 'clojure.core/__interface* ns-str (str name) iface-sigs))
      (if doc
        (list 'def name doc
          (list 'clojure.core/__protocol* proto-sigs (list 'var iface-sym)))
        (list 'def name
          (list 'clojure.core/__protocol* proto-sigs (list 'var iface-sym))))
      (apply list 'do fns)
      name)))

(defn- group-extend-specs [kind specs]
  ;; Group starts are symbols — plus `nil` (null extensions) and
  ;; keywords (which fall through to the native's `is not a
  ;; protocol` / `not a type` errors, like the oracle).
  (loop [ss specs, cur nil, started false, groups []]
    (if (empty? ss)
      groups
      (let [s (first ss)]
        (cond (or (symbol? s) (keyword? s) (nil? s))
              (recur (rest ss) s true (conj groups [s []]))
              (seq? s)
              (do (when-not started
                    (throw-illegal-arg! (str "Can't define method not in interfaces: " (first s))))
                  (let [g (peek groups)]
                    (recur (rest ss) cur true
                      (conj (pop groups) [(first g) (conj (second g) s)]))))
              :else
              (throw-illegal-arg! (str "Invalid " kind " spec: " (pr-str s))))))))

(defn- emit-extend-calls [kind proto-of type-of groups]
  (apply list 'do
    (map (fn [[target methods]]
           (let [triples (mapcat (fn [m] (method-triples kind (first m) (rest m))) methods)
                 mmap (emit-type-methods kind triples)]
             (list 'clojure.core/__extend* (proto-of target) (type-of target) mmap)))
         groups)))

(defmacro extend-type [t & specs]
  (emit-extend-calls "extend-type"
    (fn [p] (if (symbol? p) (list 'var p) (list 'quote p)))
    (fn [_] (list 'quote t))
    (group-extend-specs "extend-type" specs)))

(defmacro extend-protocol [p & specs]
  (emit-extend-calls "extend-protocol"
    (fn [_] (list 'var p))
    (fn [t] (list 'quote t))
    (group-extend-specs "extend-protocol" specs)))

(defmacro extend [t & proto+mmaps]
  (when (odd? (count proto+mmaps))
    (throw-illegal-arg! "extend requires an even number of proto/method-map forms"))
  (apply list 'do
    (map (fn [[p mmap]]
           (when-not (or (symbol? p) (keyword? p))
             (throw-illegal-arg! (str "extend protocol must be a symbol: " (pr-str p))))
           (list 'clojure.core/__extend*
             (if (symbol? p) (list 'var p) (list 'quote p))
             (list 'quote t) mmap))
         (partition 2 proto+mmaps))))

;; S9 scalar kinds (issue #52): `Inst` over the `Instant` kind.
;; The `java.util.Date` extension reads millis through the hidden
;; `__inst_millis*` native; user types extend `Inst` through the
;; ordinary machinery.
(defprotocol Inst
  (inst-ms* [inst]))
(extend-protocol Inst
  java.util.Date
  (inst-ms* [inst] (clojure.core/__inst_millis* inst)))
(defn inst-ms
  "Return the number of milliseconds since January 1, 1970, 00:00:00 GMT"
  {:added "1.9"}
  [inst]
  (inst-ms* inst))
(defn inst?
  "Return true if x satisfies Inst"
  {:added "1.9"}
  [x]
  (satisfies? Inst x))

;; Host class names as values (the port has no class literals):
;; `String` in value position is its name string, so `extends?` /
;; `satisfies?` take classes the way the pins write them.
(def String "java.lang.String")
(def Long "java.lang.Long")
(def Double "java.lang.Double")
(def Boolean "java.lang.Boolean")
(def Object "java.lang.Object")

;; S14: the shared empty gvec node, `(VecNode. nil
;; (object-array 32))` — a 32-nil vector stands for the array.
(def EMPTY-NODE (->VecNode nil (vec (repeat 32 nil))))

(defmacro with-open [bindings & body]
  (when-not (vector? bindings)
    (throw-illegal-arg! "with-open bindings must be a vector"))
  (when (odd? (count bindings))
    (throw-illegal-arg! "with-open requires an even number of bindings"))
  (let [inner (apply list 'do body)]
    (loop [pairs (reverse (partition 2 bindings)), form inner]
      (if (empty? pairs)
        form
        (let [[n init] (first pairs)]
          (recur (rest pairs)
            (list 'let [n init]
              (list 'try form (list 'finally (list (symbol ".close") n))))))))))

;; The global hierarchy (S13): the `derive` / `underive` short
;; arities and default multifns alter and read this root
;; (private, like the JVM — probe `h-private`).
(def ^:private global-hierarchy (make-hierarchy))

;; --- Multimethods (S-typechain stage 3): `defmulti` / `defmethod`
;; / `prefer-method` / `remove-method` / `remove-all-methods`
;; expand to the hidden `__multifn*` et al natives (`get-method` /
;; `methods` / `prefers` are plain natives on values). Dispatch
;; values are evaluated (keywords, vectors, and the class-name
;; vars all ride through); method fns are ordinary fns over the
;; call args. Table updates install a new var root, so the
;; mutating forms take a var (a symbol is read as one; anything
;; else falls through to the native's cast error, like `extend`).
;; Dispatch consults hierarchies (S13): exact entry, best `isa?`
;; match with preference ties broken, then the default entry.

(defn- parse-multi-head [name args]
  (check-type-name "defmulti" name)
  (let [[doc args] (if (string? (first args)) [(first args) (rest args)] [nil args])
        [attr args] (if (map? (first args)) [(first args) (rest args)] [nil args])]
    (when (empty? args)
      (throw-illegal-arg! "defmulti requires a dispatch fn"))
    {:doc doc :attr attr :dispatch (first args) :options (rest args)}))

(defn- parse-multi-options [options]
  ;; `:default` and `:hierarchy` are supported (S13); option
  ;; values are emitted for runtime evaluation; last wins. The
  ;; presence flag distinguishes absent `:hierarchy` (global)
  ;; from explicit nil (NPEs at the call, probe `x-hier-nil`).
  (when (odd? (count options))
    (throw-illegal-arg! "defmulti options must be key/value pairs"))
  (loop [opts options, default :default, hierarchy nil, present false]
    (if (empty? opts)
      [default hierarchy present]
      (let [[k v] opts]
        (cond (= :default k) (recur (drop 2 opts) v hierarchy present)
              (= :hierarchy k) (recur (drop 2 opts) default v true)
              :else (throw-illegal-arg! (str "Unsupported defmulti option: " k)))))))

(defmacro defmulti [name & args]
  ;; Answers the var (like the JVM's `def`-family shape), not the
  ;; value — the shared tail evaluates `name`, so re-read the var.
  (let [{:keys [doc attr dispatch options]} (parse-multi-head name args)
        [default hierarchy present] (parse-multi-options options)]
    (list 'do
      (finish-type-def name doc attr
        (list 'clojure.core/__multifn* (str name) dispatch default hierarchy present))
      (list 'var name))))

(defn- normalize-defmethod-tail [fn-tail]
  ;; A single `([argv] body...)` or several — every arity opens
  ;; with a vector (methods are plain fns over the call args).
  (when (empty? fn-tail)
    (throw-illegal-arg! "defmethod requires a function tail"))
  (if (vector? (first fn-tail))
    fn-tail
    (do (doseq [b fn-tail]
          (when-not (and (seq? b) (vector? (first b)))
            (throw-illegal-arg! (str "Invalid defmethod arity: " (pr-str b)))))
        fn-tail)))

(defmacro defmethod [m dispatch-val & fn-tail]
  (normalize-defmethod-tail fn-tail)
  (list 'clojure.core/__defmethod*
    (if (symbol? m) (list 'var m) (list 'quote m))
    dispatch-val
    (apply list 'fn fn-tail)))

(defmacro remove-method [m dispatch-val]
  (list 'clojure.core/__remove_method*
    (if (symbol? m) (list 'var m) (list 'quote m))
    dispatch-val))

(defmacro remove-all-methods [m]
  (list 'clojure.core/__remove_all_methods*
    (if (symbol? m) (list 'var m) (list 'quote m))))

(defmacro prefer-method [m preferred other]
  (list 'clojure.core/__prefer_method*
    (if (symbol? m) (list 'var m) (list 'quote m))
    preferred other))

;; --- `line-seq` (S-instream): eager first pull plus a lazy tail
;; (zero Rust, like `random-sample`). The port has one readable
;; kind, so this consumes the port's `StreamReader` by design —
;; the JVM instead rejects non-`BufferedReader`s (probes
;; `ls-basic`..`ls-empty` document the JVM side).
(defn line-seq [rdr] ((fn step [] (binding [*in* rdr] (when-let [line (read-line)] (cons line (lazy-seq (step))))))))

;; --- `with-precision` (zero-oos S2): upstream shape over
;; `binding`, except the `MathContext` comes from the port-only
;; `clojure.core/math-context` ctor (installed, never referred
;; — no interop here) and the `:rounding` mode splices quoted
;; (upstream resolves it as a static field at compile time).
(defmacro with-precision [precision & exprs]
  (let [[body rm] (if (= (first exprs) :rounding)
                    [(next (next exprs)) (list (second exprs))]
                    [exprs nil])]
    `(binding [*math-context* (clojure.core/math-context ~precision ~@(map (fn [m] (list 'quote m)) rm))]
       ~@body)))

;; --- `ffirst` (S1): the stock composite (probes `p-ffirst-*`).
(defn ffirst
  "Same as (first (first x))"
  {:added "1.0"
   :static true}
  [x] (first (first x)))

;; --- Deprecated aliases (S5 state introspection): verbatim ports,
;; meta included (oracle `j-*` pins value- and meta-equivalence
;; with the replacements).
(defn agent-errors
  "DEPRECATED: Use 'agent-error' instead.
  Returns a sequence of the exceptions thrown during asynchronous
  actions of the agent."
  {:added "1.0"
   :deprecated "1.2"}
  [a]
  (when-let [e (agent-error a)]
    (list e)))

(defn clear-agent-errors
  "DEPRECATED: Use 'restart-agent' instead.
  Clears any exceptions thrown during asynchronous actions of the
  agent, allowing subsequent actions to occur."
  {:added "1.0"
   :deprecated "1.2"}
  [a] (restart-agent a @a))

(defn replicate
  "DEPRECATED: Use 'repeat' instead.
   Returns a lazy seq of n xs."
  {:added "1.0"
   :deprecated "1.3"}
  [n x] (take n (repeat x)))

;; --- S4b macro machinery (zero-oos #47): the expansion-helper
;; surface, JVM name munging (natives), and the var-frame
;; macros. `definline` records `:inline` without honoring it
;; (the port is an interpreter; calls run the specimen body).
;; `selector` rides the destructuring outlets. `unquote` /
;; `unquote-splicing` are bare unbounds (upstream core.clj
;; lines 13-14) so reader-emitted `~` symbols resolve.
;; `with-local-vars` expands interop-free over the private
;; `__local-var*` (anonymous dynamic vars).

(def unquote)

(def unquote-splicing)

(defmacro definline
  "Experimental - like defmacro, except defines a named function whose
  body is the expansion, calls to which may be expanded inline as if
  it were a macro. Cannot be used with variadic (&) args."
  {:added "1.0"}
  [name & decl]
  (let [[pre-args [args expr]] (split-with (comp not vector?) decl)]
    `(do
       (defn ~name ~@pre-args ~args ~(apply (eval (list `fn args expr)) args))
       (alter-meta! (var ~name) assoc :inline (fn ~name ~args ~expr))
       (var ~name))))

(defn- selector-impl [m]
  (let [dirs [:select :excess :missing :all]
        names (zipmap (filter m dirs) (repeatedly gensym))]
    (if (empty? names)
      (throw-illegal-arg! "form must contain at least one of :select :excess :missing :all")
      `(fn ~(gensym "selector")
         [map#]
         (let [~(merge m names) map#]
           ~(if (= 1 (count names))
              (-> names first val)
              (list `some-vals names)))))))

(defmacro selector
  "Builds a selecting-fn from m, a map destructuring form that must
  include one or more of the :select, :all, :missing, and :excess
  directives. The return function takes a collection, destructures it
  per m, and returns a map of the result(s).

  If m has exactly one directive, the result is the value that
  directive would yield. If m has more than one directive, then it
  returns a map of directives to values.

  As in destructuring, :missing controls whether missing required keys
  throw or are collected.

  While a map destructuring form may and sometimes must include
  bindings, selector doesn't produce bindings, thus ignoring the
  associated directive names.

  Throws an exception if the argument is not a map."
  {:added "1.13"}
  [m]
  (when (not (map? m))
    (throw-illegal-arg! "expected a map"))
  (selector-impl m))

(defmacro with-local-vars
  "varbinding=> symbol init-expr

  Executes the exprs in a context in which the symbols are bound to
  vars with per-thread bindings to the init-exprs.  The symbols refer
  to the var objects themselves, and must be accessed with var-get and
  var-set"
  {:added "1.0"}
  [name-vals-vec & body]
  (assert-args
     (vector? name-vals-vec) "a vector for its binding"
     (even? (count name-vals-vec)) "an even number of forms in binding vector")
  `(let [~@(interleave (take-nth 2 name-vals-vec)
                       (repeat (list 'clojure.core/__local-var*)))]
     (clojure.core/push-thread-bindings (hash-map ~@name-vals-vec))
     ;; The `finally` clause is a concrete list (syntax-quote
     ;; would leave a seq, which the `try` clause matcher
     ;; rejects); it prints exactly like the upstream shape.
     (try ~@body ~(list 'finally (list 'clojure.core/pop-thread-bindings)))))

;; --- Zero-oos S3 seq/coll walks (issue #45): `tree-seq` /
;; `flatten` / `reduce-kv` / `merge-deep` / `merge-deep-with` /
;; `bounded-count` / `run!` / `splitv-at` / `partitionv` /
;; `partitionv-all` / `update-vals` / `update-keys`, ported from
;; alpha10 `clojure/core.clj` (docstrings and `:added` kept;
;; `:static` dropped — nothing compiles statically). Oracle:
;; `docs/zero-oos-s3-seqs/probes/seqwalks.{clj,out}`.
;;
;; Port deltas from upstream (each probed):
;; - `reduce-kv` dispatches in clj (no `IKVReduce` exists):
;;   nil answers init, vectors take an index loop (`reduced?`
;;   unwraps via `unreduced`, like `kvreduce`), everything
;;   else reduces over entries with `key`/`val` (the `Object`
;;   slow path). Bare map entries ride the slow path too
;;   (entries are vectors but not `IKVReduce` on the JVM):
;;   `s3-entry?` probes with `key`, the only discriminator
;;   either runtime exposes.
;; - `update-vals` tests editability in clj (`instance?` is
;;   N1): maps/vectors/sets except sorted ones (not
;;   editable on the JVM either), records (the port cannot
;;   transient a `HostType`; the JVM drops record-ness
;;   through the round-trip anyway), and bare entries.
;; - `partitionv-all` 1-arity inlines the stateful
;;   transducer in clj (an atom buffer for the `ArrayList`;
;;   the port's `partition-all` has no 1-arity).
;; - `merge-deep`/`merge-deep-with` self-recurse by name,
;;   so they are declared first.
(declare merge-deep merge-deep-with)

(defn- s3-entry? [x]
  (try (do (key x) true)
       (catch ClassCastException _ false)))

(defn- s3-editable? [m]
  (and (or (map? m) (vector? m) (set? m))
       (not (sorted? m))
       (not (record? m))
       (not (s3-entry? m))))

(defn tree-seq
  "Returns a lazy sequence of the nodes in a tree, via a depth-first walk.
   branch? must be a fn of one arg that returns true if passed a node
   that can have children (but may not).  children must be a fn of one
   arg that returns a sequence of the children. Will only be called on
   nodes for which branch? returns true. Root is the root node of the
  tree."
  {:added "1.0"}
  [branch? children root]
  (let [walk (fn walk [node]
               (lazy-seq
                (cons node
                 (when (branch? node)
                   (mapcat walk (children node))))))]
    (walk root)))

(defn flatten
  "Takes any nested combination of sequential things (lists, vectors,
  etc.) and returns their contents as a single, flat lazy sequence.
  (flatten nil) returns an empty sequence."
  {:added "1.2"}
  [x]
  (filter (complement sequential?)
          (rest (tree-seq sequential? seq x))))

(defn reduce-kv
  "Reduces an associative collection. f should be a function of 3
  arguments. Returns the result of applying f to init, the first key
  and the first value in coll, then applying f to that result and the
  2nd key and value, etc. If coll contains no entries, returns init
  and f is not called. Note that reduce-kv is supported on vectors,
  where the keys will be the ordinals."
  {:added "1.4"}
  [f init coll]
  (cond
    (nil? coll) init
    (and (vector? coll) (not (s3-entry? coll)))
    (let [n (count coll)]
      (loop [i 0
             acc init]
        (if (< i n)
          (let [ret (f acc i (nth coll i))]
            (if (reduced? ret)
              (unreduced ret)
              (recur (inc i) ret)))
          acc)))
    :else (reduce (fn [acc e] (f acc (key e) (val e))) init coll)))

(defn merge-deep
  "Returns a map that consists of the rest of the maps conj-ed onto
  the first. For each pairwise merge, when a key occurs in both maps,
  if both values are maps, then recursively merge-deep, else use the
  latter (left-to-right) value."
  {:added "1.13"}
  [& maps]
  (apply merge-with
    (fn [x y]
      (if (and (map? x) (map? y))
        (merge-deep x y)
        y))
    maps))

(defn merge-deep-with
  "Returns a map that consists of the rest of the maps conj-ed onto
  the first. For each pairwise merge, when a key occurs in both maps,
  if both values are maps, then recursively merge-deep-with, else use
  the result of (f val-in-prior val-in-latter)."
  {:added "1.13"}
  [f & maps]
  (apply merge-with
    (fn [x y]
      (if (and (map? x) (map? y))
        (merge-deep-with f x y)
        (f x y)))
    maps))

(defn bounded-count
  "If coll is counted? returns its count, else will count at most the first n
  elements of coll using its seq"
  {:added "1.9"}
  [n coll]
  (if (counted? coll)
    (count coll)
    (loop [i 0 s (seq coll)]
      (if (and s (< i n))
        (recur (inc i) (next s))
        i))))

(defn run!
  "Runs the supplied procedure (via reduce), for purposes of side
  effects, on successive items in the collection. Returns nil"
  {:added "1.7"}
  [proc coll]
  (reduce #(proc %2) nil coll)
  nil)

(defn splitv-at
  "Returns a vector of [(into [] (take n) coll) (drop n coll)]"
  {:added "1.12"}
  [n coll]
  [(into [] (take n) coll) (drop n coll)])

(defn partitionv
  "Returns a lazy sequence of vectors of n items each, at offsets step
  apart. If step is not supplied, defaults to n, i.e. the partitions
  do not overlap. If a pad collection is supplied, use its elements as
  necessary to complete last partition upto n items. In case there are
  not enough padding elements, return a partition with less than n items."
  {:added "1.12"}
  ([n coll]
   (partitionv n n coll))
  ([n step coll]
   (lazy-seq
     (when-let [s (seq coll)]
       (let [p (into [] (take n) s)]
         (when (= n (count p))
           (cons p (partitionv n step (nthrest s step))))))))
  ([n step pad coll]
   (lazy-seq
     (when-let [s (seq coll)]
       (let [p (into [] (take n) s)]
         (if (= n (count p))
           (cons p (partitionv n step pad (nthrest s step)))
           (list (into [] (take n) (concat p pad)))))))))

(defn partitionv-all
  "Returns a lazy sequence of vector partitions, but may include
  partitions with fewer than n items at the end.
  Returns a stateful transducer when no collection is provided."
  {:added "1.12"}
  ([n]
   (fn [rf]
     (let [a (atom [])]
       (fn
         ([] (rf))
         ([result]
          (let [result (if (empty? @a)
                         result
                         (let [v @a]
                           (reset! a [])
                           (unreduced (rf result v))))]
            (rf result)))
         ([result input]
          (let [v (swap! a conj input)]
            (if (= n (count v))
              (do (reset! a [])
                  (rf result v))
              result)))))))
  ([n coll]
   (partitionv-all n n coll))
  ([n step coll]
   (lazy-seq
     (when-let [s (seq coll)]
       (let [seg (into [] (take n) coll)]
         (cons seg (partitionv-all n step (drop step s))))))))

(defn update-vals
  "m f => {k (f v) ...}

  Given a map m and a function f of 1-argument, returns a new map where the keys of m
  are mapped to result of applying f to the corresponding values of m."
  {:added "1.11"}
  [m f]
  (persistent!
   (reduce-kv
    (fn [acc k v] (assoc! acc k (f v)))
    (transient
     (if (s3-editable? m) m
         (with-meta {} (meta m))))
    m)))

(defn update-keys
  "m f => {(f k) v ...}

  Given a map m and a function f of 1-argument, returns a new map whose
  keys are the result of applying f to the keys of m, mapped to the
  corresponding values of m.
  f must return a unique key for each key of m, else the behavior is undefined."
  {:added "1.11"}
  [m f]
  (let [ret (persistent!
             (reduce-kv (fn [acc k v] (assoc! acc (f k) v))
                        (transient {})
                        m))]
    (with-meta ret (meta m))))
;; --- `with-loading-context` (S7 read gates): the JVM binds the
;; loader around the body (probe `w-expand`); the port has no
;; classloaders, so the macro keeps the immediate-invocation shape
;; and the cycle guard stays native (never forked here).
(defmacro with-loading-context [& body] `((fn ~(gensym "loading") [] ~@body)))

;; --- `defstruct` (S12): the legacy structmap basis macro (oracle
;; `sm-defstruct-expand` pins the expansion shape).
(defmacro defstruct
  [name & keys]
  (list 'def name (cons 'clojure.core/create-struct keys)))

;; --- S10: `comparator` (verbatim JVM body) and `tap->`
;; (verbatim JVM one-liner). The `cond` (not `if`) matters —
;; truthy non-bool pred results order (probes `k-truthy-lt`,
;; `k-truthy-eq`). The call convention `(cmp a b)` is already
;; what `SortSpec::Custom` consumes, so `sort`, `sort-by`, and
;; `sorted-map-by` interoperate with no `coll.rs` change.
(defn comparator
  "Returns an implementation of java.util.Comparator based upon pred."
  {:added "1.0"}
  [pred]
  (fn [x y]
    (cond (pred x y) -1 (pred y x) 1 :else 0)))

(defn tap->
  "Sends x to any taps, drops if tap queue is full, returns x."
  {:added "1.13"}
  [x]
  (doto x tap>))

;; --- `amap` / `areduce` (S15): verbatim ports of the alpha10
;; `core.clj` macros except `unchecked-inc`/`unchecked-inc-int`
;; become `inc` (those vars do not exist in the port; the port's
;; own `dotimes` substitutes `inc` the same way). Oracles:
;; `docs/zero-oos-s15-arrays/probes/interop.{clj,out}` (`e-amap*`,
;; `e-areduce*`, `e-expand-*`).
(defmacro amap
  "Maps an expression across an array a, using an index named idx, and
  return value named ret, initialized to a clone of a, then setting
  each element of ret to the evaluation of expr, returning the new
  array ret."
  {:added "1.0"}
  [a idx ret expr]
  `(let [a# ~a l# (alength a#)
         ~ret (aclone a#)]
     (loop [~idx 0]
       (if (< ~idx l#)
         (do
           (aset ~ret ~idx ~expr)
           (~'recur (inc ~idx)))
         ~ret))))
(defmacro areduce
  "Reduces an expression across an array a, using an index named idx,
  and return value named ret, initialized to init, setting ret to the
  evaluation of expr at each step, returning ret."
  {:added "1.0"}
  [a idx ret init expr]
  `(let [a# ~a l# (alength a#)]
     (loop [~idx 0 ~ret ~init]
       (if (< ~idx l#)
         (~'recur (inc ~idx) ~expr)
         ~ret))))

;; --- `xml-seq` (zero-oos S17 JDK/IO adapters, issue #61):
;; verbatim port of alpha10 `clojure/core.clj` (`:static`
;; dropped — nothing compiles statically). Strings are leaf
;; nodes, not skipped (oracle `e-walk`); laziness rides
;; `tree-seq`. Oracle:
;; `docs/zero-oos-s17-jdkio/probes/jdkio.{clj,out}`.
(defn xml-seq
  "A tree seq on the xml elements as per xml/parse"
  {:added "1.0"}
  [root]
    (tree-seq
     (complement string?)
     (comp seq :content)
     root))
