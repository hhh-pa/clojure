;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; #120 sub-slice 4: a `clojure.reflect` facsimile over the emulated
;; lattice (no JVM at runtime, ever). Oracle is the upstream
;; `clojure/reflect.clj` + `clojure/reflect/java.clj` source text
;; (read, never executed); `type-reflect`/`reflect` map shapes match
;; upstream exactly (`:bases`, `:flags`, `:members`, member keys).
;; Members are plain maps, not the upstream `Constructor`/`Method`/
;; `Field` records: port records do not `=` maps, so records would
;; break map-shape equality (and the port cannot `instance?` record
;; classes anyway).
;;
;; Port adaptations (pinned by `rust/crates/clj-eval/tests/reflect.rs`):
;; - One `EmulatedReflector` replaces `JavaReflector`/`AsmReflector`:
;;   `do-reflect` resolves the typeref through the hidden
;;   `__reflect*` native, which reads the `ClassVal` lattice row
;;   (bases, interface flag) plus the emulated member table in
;;   `reflect.rs`. There are no classloaders and no class bytes, so
;;   the reflector's `classloader` field is always nil.
;; - Members are exactly the emulated surface (`..` members,
;;   `Class/forName`, the `System` statics, the `String`/`Object`
;;   ctors); unemulated JVM members (e.g. `Object.wait`,
;;   `String.indexOf`) are absent, and there are no fields (the port
;;   exposes none). `:exception-types` is always empty.
;; - Class modifiers are emulated (`public` throughout, plus
;;   `abstract`+`interface` on interfaces); member modifiers add
;;   `abstract` on interface methods and `static` on statics.
;;   Modifiers cross as `:flags-raw` ints so the single upstream
;;   `parse-flags` below does every conversion.
;; - `TypeReference` extends `Symbol` and `Class` only (upstream also
;;   extends the ASM `Type`, which the port has no equivalent of;
;;   strings stay unextended, as upstream). Symbol `<>`/`[]` surgery
;;   lives in the `__reflect_typename*` native: the port embeds
;;   neither `clojure.string` nor a `String.replace` host method, so
;;   there is no `typesym` here.
;; - `clojure.set` is not embedded either; the one set operation
;;   `type-reflect` needs is the local `set-difference`.
;; - No syntax-quote and no `::auto-resolve` anywhere in this file:
;;   the port reads a whole load batch before evaluating it, so both
;;   would qualify against the *requiring* namespace instead of this
;;   one (the `spec_alpha.clj` precedent).
(ns ^{:author "Stuart Halloway"
      :added "1.3"
      :doc "Reflection on Host Types
Alpha - subject to change.

Two main entry points:

* type-reflect reflects on something that implements TypeReference.
* reflect (for REPL use) reflects on the class of an instance, or
  on a class if passed a class

Key features:

* Exposes the read side of reflection as pure data. Reflecting
  on a type returns a map with keys :bases, :flags, and :members.

* Canonicalizes class names as Clojure symbols. Types can extend
  to the TypeReference protocol to indicate that they can be
  unambiguously resolved as a type name. The canonical format
  requires one non-Java-ish convention: array brackets are <>
  instead of [] so they can be part of a Clojure symbol.

* Pluggable Reflectors for different implementations. This port
  ships one Reflector: the EmulatedReflector below, which reflects
  over the emulated class lattice (there is no JVM to ask).

Platform implementers must:

* Create an implementation of Reflector.
* Create one or more implementations of TypeReference.
* def default-reflector to be an instance that satisfies Reflector."}
  clojure.reflect)

(defprotocol Reflector
  "Protocol for reflection implementers."
  (do-reflect [reflector typeref]))

(defprotocol TypeReference
  "A TypeReference can be unambiguously converted to a type name on
   the host platform.

   All typerefs are normalized into symbols. If you need to
   normalize a typeref yourself, call typesym."
  (typename [o] "Returns Java name as returned by ASM getClassName, e.g. byte[], java.lang.String[]"))

(declare default-reflector)

(extend-protocol TypeReference
  clojure.lang.Symbol
  (typename [s] (clojure.core/__reflect_typename* s))

  java.lang.Class
  (typename [c] (clojure.core/__reflect_typename* c)))

(defn- access-flag
  [[name flag & contexts]]
  {:name name :flag flag :contexts (set (map keyword contexts))})

(def ^{:doc "The Java access bitflags, along with their friendly names and
the kinds of objects to which they can apply."}
  flag-descriptors
  (vec
   (map access-flag
        [[:public 0x0001 :class :field :method]
         [:private 0x002 :class :field :method]
         [:protected 0x0004  :class :field :method]
         [:static 0x0008  :field :method]
         [:final 0x0010  :class :field :method]
         ;; :super is ancient history and is unfindable (?) by
         ;; reflection. skip it (upstream carries it as a #_ comment).
         [:synchronized 0x0020  :method]
         [:volatile 0x0040  :field]
         [:bridge 0x0040  :method]
         [:varargs 0x0080  :method]
         [:transient 0x0080  :field]
         [:native 0x0100  :method]
         [:interface 0x0200  :class]
         [:abstract 0x0400  :class :method]
         [:strict 0x0800  :method]
         [:synthetic 0x1000  :class :field :method]
         [:annotation 0x2000  :class]
         [:enum 0x4000  :class :field :inner]])))

(defn- parse-flags
  "Convert reflection bitflags into a set of keywords."
  [flags context]
  (reduce
   (fn [result fd]
     (if (and (get (:contexts fd) context)
              (not (zero? (bit-and flags (:flag fd)))))
       (conj result (:name fd))
       result))
   #{}
   flag-descriptors))

(defn- convert-member
  "A raw `__reflect*` member map to its upstream shape: the parsed
   `:flags` in place of `:flags-raw`. Every member parses in the
   `:method` context — ctors as upstream, methods as upstream (the
   port reports no fields, which would take `:field`)."
  [m]
  (assoc (dissoc m :flags-raw) :flags (parse-flags (:flags-raw m) :method)))

(defn- set-difference
  "The `clojure.set/difference` this namespace needs (`clojure.set`
   is not embedded): elements of `s1` not in `s2`."
  [s1 s2]
  (reduce disj (set s1) s2))

(deftype EmulatedReflector [classloader]
  Reflector
  (do-reflect [_ typeref]
    (let [raw (clojure.core/__reflect* (typename typeref))]
      {:bases (:bases raw)
       :flags (parse-flags (:flags-raw raw) :class)
       :members (set (map convert-member (:members raw)))})))

(def ^:private default-reflector
  (EmulatedReflector. nil))

(defn type-reflect
  "Alpha - subject to change.
   Reflect on a typeref, returning a map with :bases, :flags, and
  :members. In the discussion below, names are always Clojure symbols.

   :bases            a set of names of the type's bases
   :flags            a set of keywords naming the boolean attributes
                     of the type.
   :members          a set of the type's members. Each member is a map
                     and can be a constructor, method, or field.

   Keys common to all members:
   :name             name of the type
   :declaring-class  name of the declarer
   :flags            keyword naming boolean attributes of the member

   Keys specific to constructors:
   :parameter-types  vector of parameter type names
   :exception-types  vector of exception type names

   Key specific to methods:
   :parameter-types  vector of parameter type names
   :exception-types  vector of exception type names
   :return-type      return type name

   Keys specific to fields:
   :type             type name

   Options:

     :ancestors     in addition to the keys described above, also
                    include an :ancestors key with the entire set of
                    ancestors, and add all ancestor members to
                    :members.
     :reflector     implementation to use. Defaults to the
                    EmulatedReflector; any Reflector is an option."
  {:added "1.3"}
  [typeref & options]
  (let [{:keys [ancestors reflector]}
        (merge {:reflector default-reflector}
               (apply hash-map options))
        refl (partial do-reflect reflector)
        result (refl typeref)]
    ;; could make simpler loop of two args: names an
    (if ancestors
      (let [make-ancestor-map (fn [names]
                            (zipmap names (map refl names)))]
        (loop [reflections (make-ancestor-map (:bases result))]
          (let [ancestors-visited (set (keys reflections))
                ancestors-to-visit (set-difference (set (mapcat :bases (vals reflections)))
                                               ancestors-visited)]
            (if (seq ancestors-to-visit)
              (recur (merge reflections (make-ancestor-map ancestors-to-visit)))
              (apply merge-with into result {:ancestors ancestors-visited}
                     (map (fn [r] (select-keys r [:members])) (vals reflections)))))))
      result)))

(defn reflect
  "Alpha - subject to change.
   Reflect on the type of obj (or obj itself if obj is a class).
   Return value and options are the same as for type-reflect. "
  {:added "1.3"}
  [obj & options]
  (apply type-reflect (if (class? obj) obj (class obj)) options))
