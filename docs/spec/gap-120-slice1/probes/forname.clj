;; #120 slice 1 oracle: static `Class/forName`, FQ class literals,
;; static-call analysis, instance-position statics, `resolve`, and
;; syntax-quote class resolution.
;; Run: CP=clojure-1.13.0-alpha9.jar:spec.alpha-0.6.249.jar:
;; core.specs.alpha-0.6.133-alpha10.jar
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;   -cp $CP clojure.main forname.clj > forname.out
(defn chain [t]
  (loop [t t, out []]
    (if (nil? t)
      out
      (recur (.getCause t) (conj out (str (.getName (class t)) " | " (.getMessage t)))))))

(defn show [label thunk]
  (try
    (let [v (thunk)]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " THROWS " (.getName (class t)) " | " (.getMessage t))))))

(defn show-chain [label src]
  (try
    (let [v (eval (read-string src))]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " CHAIN:"))
      (doseq [line (chain t)]
        (println (str "   " line))))))

;; --- 1-arg forName ---
(show "forName-String" #(Class/forName "java.lang.String"))
(show "forName-eq" #(= String (Class/forName "java.lang.String")))
(show "forName-missing" #(Class/forName "no.such.Klass"))
(show "forName-nil" #(Class/forName nil))
(show "forName-num" #(Class/forName 1))
(show "forName-int" #(Class/forName "int"))
(show "forName-arr" #(Class/forName "[Ljava.lang.String;"))
(show "forName-arrI" #(Class/forName "[I"))
(show "forName-arr-missing" #(Class/forName "[Lcom.foo.Bar;"))
(show "forName-arr-bad" #(Class/forName "[X"))
(show "fq-forName" #(eval '(java.lang.Class/forName "java.lang.String")))
(show-chain "o1-empty" "(Class/forName \"\")")

;; --- 3-arg forName ---
(show "forName-3arg" #(Class/forName "java.lang.String" false nil))
(show "forName-3arg-init" #(Class/forName "java.lang.String" true nil))
(show-chain "forName-init-str" "(Class/forName \"java.lang.String\" \"yes\" nil)")
(show-chain "forName-loader-str" "(Class/forName \"java.lang.String\" false \"loader\")")
(show-chain "forName-nil-init" "(Class/forName \"java.lang.String\" nil nil)")
(show-chain "o3-name-first" "(Class/forName 1 \"y\" \"z\")")
(show-chain "o3-init-first" "(Class/forName \"java.lang.String\" \"y\" \"z\")")
(show-chain "o3-missing-badinit" "(Class/forName \"missing.K\" \"bad\" nil)")
(show-chain "o3-nilname-badinit" "(Class/forName nil \"bad\" nil)")
(show-chain "o3-nilnilnil" "(Class/forName nil nil nil)")
(show-chain "o3-missing-nilnil" "(Class/forName \"missing.K\" nil nil)")
(show-chain "o3-init-unbox-first" "(Class/forName \"java.lang.String\" nil \"x\")")
(show-chain "forName-loader-real" "(Class/forName \"java.lang.String\" true (ClassLoader/getSystemClassLoader))")

;; --- 2-arg forName ---
(show "forName-2arg" #(eval '(Class/forName "a" "b")))
(show-chain "forName-2arg-nilmod" "(Class/forName nil \"java.lang.String\")")
(show-chain "forName-2arg-nil-bad" "(Class/forName nil 5)")
(show-chain "forName-nn" "(Class/forName nil nil)")
(show-chain "forName-order" "(Class/forName \"m\" 5)")

;; --- static-call analysis ---
(show-chain "forName-0arg" "(Class/forName)")
(show-chain "forName-4arg" "(Class/forName \"a\" true nil :extra)")
(show-chain "static-miss-0" "(String/nope)")
(show-chain "static-miss-arity" "(String/valueOf)")
(show-chain "static-miss-args" "(String/nope 1 2)")
(show-chain "static-unknown-class" "(NopeKlass/foo)")
(show-chain "static-unknown-fq" "(java.no.Klass/foo)")
(show "static-valueOf" #(String/valueOf 1))
(show "static-field-value" #(eval 'Integer/MAX_VALUE))
(show-chain "deftype-static-miss" "(do (deftype QE []) (eval '(user.QE/bar)))")
(show "import-then-fq" #(do (import 'java.util.HashMap) (java.util.HashMap.)))
(show "static-field-call" #(eval '(Integer/MAX_VALUE)))
(show "field-call-order" #(eval '(String/CASE_INSENSITIVE_ORDER)))
(show "static-0arg-method" #(eval '(System/nanoTime)))
(show "new-String" #(eval '(String. "x")))
(show "fq-ctor" #(eval '(java.util.HashMap. 4 0.5)))

;; --- FQ literals / namespace parts ---
(show "class-literal-eq" #(= java.lang.String (Class/forName "java.lang.String")))
(show-chain "val-fq-String" "java.lang.String")
(show-chain "val-fq-missing" "java.no.Klass")
(show-chain "val-simple-missing" "NopeKlass")
(show-chain "val-dotdot" "foo.bar")
(show-chain "val-simplequal-missing" "NopeKlass/foo")
(show-chain "removed-ns-call" "(do (ns foo.bar) (in-ns 'user) (remove-ns 'foo.bar) (eval '(foo.bar/baz)))")
(show-chain "removed-ns-val" "(do (ns foo.bar2) (in-ns 'user) (remove-ns 'foo.bar2) (eval 'foo.bar2/baz))")
(show-chain "v-dotdash-a" ".-a")
(show-chain "v-traildot" "foo.")
(show-chain "v-leaddot" ".foo")
(show-chain "v-dashdot" "-a.b")
(show-chain "call-fq-noargs" "(java.lang.String)")
(show-chain "v-method" "String/valueOf")
(show-chain "v-static-meth" "Integer/parseInt")
(show-chain "v-field" "Integer/MAX_VALUE")
(show-chain "v-int" "int")
(show-chain "bare-deftype" "(do (deftype QW []) (eval 'user.QW))")
(show-chain "deftype-forName" "(do (deftype Foo []) (Class/forName \"user.Foo\"))")
(show-chain "deftype-name" "(do (deftype Bar []) (.getName (Class/forName \"user.Bar\")))")
(show-chain "import-deftype" "(do (deftype Baz []) (import 'user.Baz) Baz)")

;; --- operators ---
(show-chain "invoke-num" "(1 2)")
(show-chain "invoke-class" "(do (import 'java.lang.String) (eval '(String)))")
(show-chain "invoke-str" "(\"s\" 1)")
(show-chain "op-args" "(do (import 'java.lang.String) (eval '(String 1 2)))")
(show-chain "call-method-on-class" "(. String (toString))")

;; --- instance position ---
(show-chain "dotdot-static" "(do (import 'java.lang.Class) (eval '(.. Class (forName \"java.lang.String\"))))")
(show-chain "inst-forName-str" "(. \"s\" forName)")
(show-chain "inst-valueOf" "(. \"s\" valueOf 1)")
(show-chain "inst-forName-1on-str" "(. \"s\" forName \"java.lang.String\")")
(show-chain "inst-getFirst-str" "(. \"s\" getFirst)")
(show-chain "inst-getMessage-num" "(. 1 getMessage)")
(show-chain "inst-getKey-str" "(. \"s\" getKey)")
(show-chain "nil-forName-1" "(. nil forName \"x\")")
(show-chain "nil-forName-0" "(. nil forName)")
(show-chain "nil-forName-3" "(. nil forName \"x\" true nil)")
(show-chain "nil-forName-5" "(. nil forName 1 2 3 4)")
(show-chain "nil-valueOf" "(. nil valueOf 1)")
(show-chain "nil-getFirst" "(. nil getFirst)")
(show-chain "nil-getFirst-2" "(. nil getFirst 1 2)")
(show-chain "nil-nosuch-2" "(. nil nosuch_xyz 1 2)")
(show-chain "nil-nosuch-0" "(. nil nosuch_xyz)")
(show-chain "nil-nosuch-1" "(. nil nosuch_xyz 1)")
(show-chain "dotdot-nil-0" "(.. nil (nosuch_xyz))")
(show-chain "dotdot-nil-1" "(.. nil (nosuch_xyz 1))")
(show-chain "dotdot-nil-2" "(.. nil (nosuch_xyz 1 2))")
(show-chain "nil-toString-2" "(. nil toString 1 2)")
(show-chain "meth-miss-str" "(. \"s\" length)")

;; --- class members ---
(show-chain "arr-getName" "(.getName (Class/forName \"[I\"))")
(show-chain "arr-eq" "(= (Class/forName \"[I\") (Class/forName \"[I\"))")
(show-chain "arr-isArray" "(.isArray (Class/forName \"[I\"))")
(show-chain "arr-super" "(.getSuperclass (Class/forName \"[Ljava.lang.String;\"))")
(show-chain "cls-getName" "(.getName (Class/forName \"java.lang.String\"))")
(show-chain "cls-toString" "(.toString (Class/forName \"java.lang.String\"))")
(show-chain "cls-eq-class" "(= (type (Class/forName \"java.lang.String\")) Class)")
(show-chain "cls-canon" "(.getCanonicalName (Class/forName \"java.lang.String\"))")
(show-chain "arr-pr" "(pr-str (Class/forName \"[I\"))")
(show-chain "arr-obj-pr" "(pr-str (Class/forName \"[Ljava.lang.String;\"))")
(show-chain "arr-print" "(print-str (Class/forName \"[I\"))")
(show-chain "arr-str" "(str (Class/forName \"[I\"))")
(show-chain "arr-simple" "(.getSimpleName (Class/forName \"[I\"))")
(show-chain "arr-obj-simple" "(.getSimpleName (Class/forName \"[Ljava.lang.String;\"))")
(show-chain "arr-canon" "(.getCanonicalName (Class/forName \"[I\"))")

;; --- resolve ---
(show-chain "resolve-String" "(do (import 'java.lang.String) (resolve 'String))")
(show-chain "resolve-fq" "(resolve 'java.lang.String)")
(show-chain "resolve-static" "(resolve 'String/valueOf)")
(show-chain "resolve-missing" "(resolve 'java.no.Klass)")
(show-chain "resolve-simple-missing" "(resolve 'NopeKlass)")
(show-chain "resolve-int" "(resolve 'int)")
(show-chain "nsresolve-String" "(do (import 'java.lang.Long) (ns-resolve 'user 'Long))")

;; --- syntax-quote (read-time) ---
(show-chain "rsq-imported" "(do (import 'java.util.HashMap) (read-string \"`HashMap\"))")
(show-chain "rsq-bare" "(read-string \"`TreeMap\")")
(show-chain "rsq-qual-imported" "(do (import 'java.util.HashMap) (read-string \"`HashMap/foo\"))")
(show-chain "rsq-qual-bare" "(read-string \"`TreeMap/foo\")")
(show-chain "sq-static" "(do (import 'java.lang.String) (eval '`(String/valueOf 1)))")
(show-chain "sq-class" "(do (import 'java.lang.Class) (eval '`(Class/forName \"x\")))")

;; --- alias-vs-static (last: the alias persists) ---
(show "var-vs-class" #(do (import 'java.lang.String)
                          (require '[clojure.string :as String])
                          (String/join "," [1 2])))
(show-chain "alias-vs-static" "(do (require '[clojure.string :as String]) (String/valueOf 1))")
(shutdown-agents)
