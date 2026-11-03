;; Copyright (c) Rich Hickey. All rights reserved.
;; Copyright (c) Hao Hu. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; W6 port of upstream clojure/core/server.clj (oracle: upstream source
;; text, read-only; no JVM at runtime, ever). Public API unchanged:
;; *session*, start-server, stop-server, stop-servers, start-servers,
;; repl-init, repl-read, repl, prepl, io-prepl, remote-prepl.
;;
;; Port adaptations (pinned by rust/crates/clj-eval/tests/w6_server.rs):
;; - Sockets are std TcpListener/TcpStream behind the __server-* /
;;   __socket-* natives (w6_runtime.rs): `start-server` returns a
;;   ServerSocket handle, the accept loop runs on a future (below),
;;   and `stop-server` unblocks the accept within milliseconds (like
;;   closing a ServerSocket). `__server-port*` answers the bound
;;   port (like getLocalPort).
;; - The ReentrantLock + `servers` var become an atom (same
;;   serialized updates; both private either way).
;; - The `thread` macro spawns futures (binding-conveyed threads that
;;   never block process exit, so daemon-ness is moot; names are kept
;;   for call-shape parity but futures are unnamed).
;; - `*in*` binds the client connection itself (socket reads go
;;   through `__socket-read-line*`, which answers nil at EOF — accept
;;   fns treat nil as end-of-input); `*out*` binds a PrintWriter-on
;;   forwarding flushed text to the socket (so println/prn work
;;   unchanged, flushing per line under *flush-on-newline*).
;;   accept-connection still swallows disconnect-shaped IO failures
;;   (java.io.IOException here instead of SocketException) and still
;;   closes the connection in a finally (after flushing *out*).
;; - clojure.main / clojure.edn belong to W4 and are NOT required at
;;   load: repl-init/repl-read/repl require clojure.main lazily at
;;   call time (they throw until W4 lands); parse-props prefers
;;   clojure.edn/read-string but falls back to read-string (identical
;;   for plain server-config data). `parse-props` takes a plain string
;;   map (no System/getProperties on the port); clojure.string is not
;;   required either (embedded-file convention), so its two uses here
;;   are local helpers (split-dot, starts-with?).
;; - `prepl` reads forms by accumulating lines and re-reading
;;   (socket conns have no pushback reader, and read+string is W4's):
;;   only EOF-shaped failures pull another line, multi-line forms
;;   work, and :form is the joined source text. Trailing forms
;;   sharing one line are dropped (socket clients send one form per
;;   message). The DynamicClassLoader dance is dropped (no
;;   classloaders); with-prepl-bindings covers the REPL-relevant
;;   subset of clojure.main/with-bindings (the process-level bindings
;;   — *compile-path*, *command-line-args*, reader fns, spec out —
;;   are left alone); :ms rides __nano-time* (no System/nanoTime).
;; - `remote-prepl` forwards line-wise (chars are equivalent for
;;   newline-terminated prepl traffic) and hands custom readf fns the
;;   connection rather than a LineNumberingPushbackReader.
;; - `(set! *warn-on-reflection* true)` is dropped (a port load
;;   hazard — the flag is unread here).
;; - resolve-fn throws ex-info (the port has no exception ctors).

(ns ^{:doc "Socket server support"
      :author "Alex Miller"}
  clojure.core.server)

(def ^:dynamic *session* nil)

;; W6: an atom serializes the same updates the ReentrantLock guarded.
(defonce ^:private servers (atom {}))

;; W6: futures stand in for raw threads (see the header note).
(defmacro ^:private thread
  [name daemon & body]
  `(future ~@body))

(defn- required
  "Throw if opts does not contain prop."
  [opts prop]
  (when (nil? (get opts prop))
    (throw (ex-info (str "Missing required socket server property " prop) opts))))

(defn- validate-opts
  "Validate server config options"
  [{:keys [name port accept] :as opts}]
  (doseq [prop [:name :port :accept]] (required opts prop))
  (when (or (not (integer? port)) (not (<= 0 port 65535)))
    (throw (ex-info (str "Invalid socket server port: " port) opts))))

(defn- accept-connection
  "Start accept function, to be invoked on a client thread, given:
    conn - client socket
    name - server name
    client-id - client identifier
    in - in stream
    out - out stream
    err - err stream
    accept - accept fn symbol to invoke
    args - to pass to accept-fn"
  [conn name client-id in out err accept args]
  (try
    (binding [*in* in
              *out* out
              *err* err
              *session* {:server name :client client-id}]
      (swap! servers assoc-in [name :sessions client-id] {})
      (require (symbol (namespace accept)))
      (let [accept-fn (resolve accept)]
        (apply accept-fn args)))
    (catch java.io.IOException _disconnect)
    (finally
      (swap! servers update-in [name :sessions] dissoc client-id)
      ;; W6: flush buffered *out* text before the socket goes away
      ;; (the PrintWriter-on only delivers on flush).
      (try
        (binding [*out* out] (flush))
        (catch java.io.IOException _disconnect))
      (__socket-close* conn))))

(defn start-server
  "Start a socket server given the specified opts:
    :address Host or address, string, defaults to loopback address
    :port Port, integer, required
    :name Name, required
    :accept Namespaced symbol of the accept function to invoke, required
    :args Vector of args to pass to accept function
    :bind-err Bind *err* to socket out stream?, defaults to true
    :server-daemon Is server thread a daemon?, defaults to true
    :client-daemon Are client threads daemons?, defaults to true
   Returns server socket."
  [opts]
  (validate-opts opts)
  (let [{:keys [address port name accept args bind-err server-daemon client-daemon]
         :or {bind-err true
              server-daemon true
              client-daemon true}} opts
        socket (__server-socket* address port)]
    (swap! servers assoc name {:name name, :socket socket, :sessions {}})
    (thread
      (str "Clojure Server " name) server-daemon
      (try
        (loop [client-counter 1]
          (when-not (__server-closed?* socket)
            (let [conn (__server-accept* socket)]
              (when conn
                (let [client-id (str client-counter)
                      out (PrintWriter-on (fn [s] (__socket-write* conn s)) nil true)]
                  (thread
                    (str "Clojure Connection " name " " client-id) client-daemon
                    (accept-connection conn name client-id conn out (if bind-err out *err*) accept args)))))
            (recur (inc client-counter))))
        (finally
          (swap! servers dissoc name))))
    socket))

(defn stop-server
  "Stop server with name or use the server-name from *session* if none supplied.
  Returns true if server stopped successfully, nil if not found, or throws if
  there is an error closing the socket."
  ([]
   (stop-server (:server *session*)))
  ([name]
   (let [server-socket (:socket (get @servers name))]
     (when server-socket
       (swap! servers dissoc name)
       (__server-close* server-socket)
       true))))

(defn stop-servers
  "Stop all servers ignores all errors, and returns nil."
  []
  (doseq [name (keys @servers)]
    (future (stop-server name))))

;; W6: clojure.string/split on dots, carried locally (embedded files
;; do not require clojure.string).
(defn- split-dot [s]
  (loop [parts [] cur "" cs (seq s)]
    (cond
      (nil? cs) (conj parts cur)
      (= (first cs) \.) (recur (conj parts cur) "" (next cs))
      :else (recur parts (str cur (first cs)) (next cs)))))

;; W6: clojure.edn is W4's — prefer it when present, else read-string
;; (identical for plain server-config data).
(defn- read-server-edn [v]
  (if-let [r (try (require 'clojure.edn)
                  (resolve 'clojure.edn/read-string)
                  (catch Exception _ nil))]
    (r v)
    (read-string v)))

(defn- parse-props
  "Parse clojure.server.* from properties to produce a map of server configs."
  [props]
  ;; W6: props is a plain string map (no Properties/System statics on
  ;; the port); (keys props) replaces .stringPropertyNames.
  (reduce
   (fn [acc k]
     (let [[k1 k2 k3] (split-dot k)]
       (if (and (= k1 "clojure") (= k2 "server"))
         (let [v (get props k)]
           (conj acc (merge {:name k3} (read-server-edn v))))
         acc)))
   []
   (keys props)))

(defn start-servers
  "Start all servers specified in the system properties."
  [system-props]
  (doseq [server (parse-props system-props)]
    (start-server server)))

;; W6: clojure.main is W4's — these require it lazily at call time so
;; the namespace still loads (and serves sockets) without it.
(defn repl-init
  "Initialize repl in user namespace and make standard repl requires."
  []
  (require 'clojure.main)
  (in-ns 'user)
  (apply require @(resolve 'clojure.main/repl-requires)))

(defn repl-read
  "Enhanced :read hook for repl supporting :repl/quit."
  [request-prompt request-exit]
  (require 'clojure.main)
  (let [skip-whitespace (resolve 'clojure.main/skip-whitespace)
        skip-if-eol (resolve 'clojure.main/skip-if-eol)]
    (or ({:line-start request-prompt :stream-end request-exit}
          (skip-whitespace *in*))
        (let [input (read {:read-cond :allow} *in*)]
          (skip-if-eol *in*)
          (case input
            :repl/quit request-exit
            input)))))

(defn repl
  "REPL with predefined hooks for attachable socket server."
  []
  (require 'clojure.main)
  ((resolve 'clojure.main/repl) :init repl-init :read repl-read))

(defn- ex->data
  [ex phase]
  (assoc (Throwable->map ex) :phase phase))

;; W6: the REPL-relevant subset of clojure.main/with-bindings (see the
;; header note for what is left out and why).
(defmacro ^:private with-prepl-bindings
  [& body]
  `(binding [*ns* *ns*
             *warn-on-reflection* *warn-on-reflection*
             *math-context* *math-context*
             *print-meta* *print-meta*
             *print-length* *print-length*
             *print-level* *print-level*
             *1 nil
             *2 nil
             *3 nil
             *e nil]
     ~@body))

;; W6: clojure.string/starts-with? carried locally (see split-dot).
(defn- starts-with? [^CharSequence s ^String prefix]
  (.startsWith (.toString s) prefix))

;; W6: line-accumulating form reader (see the header note). read-string
;; reports truncated input as "EOF while reading…", which is the only
;; failure that pulls another line.
(def ^:private eof-sentinel (list ::eof))

(defn- read-form+string [read-line-fn]
  (loop [acc nil]
    (let [line (read-line-fn)]
      (if (nil? line)
        (if (nil? acc)
          [eof-sentinel nil]
          [(read-string {:read-cond :allow} acc) acc])
        (let [text (if (nil? acc) line (str acc "\n" line))
              parsed (try (read-string {:eof ::empty :read-cond :allow} text)
                          (catch RuntimeException e
                            (if (starts-with? (str (ex-message e)) "EOF while reading")
                              ::more
                              (throw e))))]
          (cond
            (= parsed ::empty) (recur acc)
            (= parsed ::more) (recur text)
            :else [parsed text]))))))

(defn prepl
  "a REPL with structured output (for programs)
  reads forms to eval from in-reader (a LineNumberingPushbackReader)
  Closing the input or passing the form :repl/quit will cause it to return

  Calls out-fn with data, one of:
  {:tag :ret
   :val val ;;eval result, or Throwable->map data if exception thrown
   :ns ns-name-string
   :ms long ;;eval time in milliseconds
   :form string ;;iff successfully read
   :exception true ;;iff exception thrown
  }
  {:tag :out
   :val string} ;chars from during-eval *out*
  {:tag :err
   :val string} ;chars from during-eval *err*
  {:tag :tap
   :val val} ;values from tap>

  You might get more than one :out or :err per eval, but exactly one :ret
  tap output can happen at any time (i.e. between evals)
  If during eval an attempt is made to read *in* it will read from in-reader unless :stdin is supplied

  Alpha, subject to change."
  {:added "1.10"}
  [in-reader out-fn & {:keys [stdin]}]
  ;; W6: no classloaders here — the DynamicClassLoader dance is dropped.
  (let [EOF eof-sentinel
        tapfn #(out-fn {:tag :tap :val %1})
        ;; W6: socket conns read through the native, stream readers
        ;; through read-line.
        read-line-fn (if (__socket-conn?* in-reader)
                       (fn [] (__socket-read-line* in-reader))
                       (fn [] (binding [*in* in-reader] (read-line))))]
    (with-prepl-bindings
      (in-ns 'user)
      (binding [*in* (or stdin in-reader)
                *out* (PrintWriter-on #(out-fn {:tag :out :val %1}) nil true)
                *err* (PrintWriter-on #(out-fn {:tag :err :val %1}) nil true)]
        (try
          (add-tap tapfn)
          (loop []
            (when (try
                    (let [[form s] (read-form+string read-line-fn)]
                      (try
                        (when-not (identical? form EOF)
                          (let [start (__nano-time*)
                                ret (eval form)
                                ms (quot (- (__nano-time*) start) 1000000)]
                            (when-not (= :repl/quit ret)
                              (set! *3 *2)
                              (set! *2 *1)
                              (set! *1 ret)
                              (out-fn {:tag :ret
                                       :val (if (instance? Throwable ret)
                                              (Throwable->map ret)
                                              ret)
                                       :ns (str (ns-name *ns*))
                                       :ms ms
                                       :form s})
                              true)))
                        (catch Throwable ex
                          (set! *e ex)
                          (out-fn {:tag :ret :val (ex->data ex (or (-> ex ex-data :clojure.error/phase) :execution))
                                   :ns (str (ns-name *ns*)) :form s
                                   :exception true})
                          true)))
                    (catch Throwable ex
                      (set! *e ex)
                      (out-fn {:tag :ret :val (ex->data ex :read-source)
                               :ns (str (ns-name *ns*))
                               :exception true})
                      true))
              (recur)))
          (finally
           (remove-tap tapfn)))))))

(defn- resolve-fn [valf]
  (if (symbol? valf)
    (or (resolve valf)
        (when-let [nsname (namespace valf)]
          (require (symbol nsname))
          (resolve valf))
        ;; W6: ex-info (the port has no exception ctors).
        (throw (ex-info (str "can't resolve: " valf) {:valf valf})))
    valf))

(defn io-prepl
  "prepl bound to *in* and *out*, suitable for use with e.g. server/repl (socket-repl).
  :ret and :tap vals will be processed by valf, a fn of one argument
  or a symbol naming same (default pr-str)

  Alpha, subject to change."
  {:added "1.10"}
  [& {:keys [valf] :or {valf pr-str}}]
  (let [valf (resolve-fn valf)
        out *out*
        lock (Object.)]
    (prepl *in*
           (fn [m]
             (binding [*out* out, *flush-on-newline* true, *print-readably* true]
               (locking lock
                 (prn (if (#{:ret :tap} (:tag m))
                        (try
                          (assoc m :val (valf (:val m)))
                          (catch Throwable ex
                            (assoc m :val (valf (ex->data ex :print-eval-result))
                                     :exception true)))
                        m))))))))

;; W6: the default response reader (responses are prn'd one per line).
(defn- default-socket-read [conn eof-val]
  (let [line (__socket-read-line* conn)]
    (if (nil? line)
      eof-val
      (read-string line))))

(defn remote-prepl
  "Implements a prepl on in-reader and out-fn by forwarding to a
  remote [io-]prepl over a socket.  Messages will be read by readf, a
  fn of a LineNumberingPushbackReader and EOF value or a symbol naming
  same (default #(read %1 false %2)),
  :ret and :tap vals will be processed by valf, a fn of one argument
  or a symbol naming same (default read-string). If that function
  throws, :val will be unprocessed.

  Alpha, subject to change."
  {:added "1.10"}
  [host port
   in-reader out-fn & {:keys [valf readf] :or {valf read-string, readf default-socket-read}}]
  (let [valf (resolve-fn valf)
        readf (resolve-fn readf)
        port (if (string? port) (Integer/parseInt port) port)
        socket (__socket-connect* host port)
        EOF eof-sentinel]
    ;; W6: custom readf fns receive the connection (see the header).
    (future
      ;; W6: upstream destructures {:keys [tag val] :as m} around (readf ...)
      ;; before the identical? EOF check, but this port's map destructuring
      ;; of a non-map (the EOF sentinel list) does not preserve :as identity,
      ;; so the EOF check must come first; message maps still destructure below.
      (try (loop []
             (let [m (readf socket EOF)]
               (when-not (identical? m EOF)
                 (let [{:keys [tag val]} m]
                   (out-fn
                    (if (#{:ret :tap} tag)
                      (try
                        (assoc m :val (valf val))
                        (catch Throwable ex
                          (assoc m :val (ex->data ex :read-eval-result)
                                   :exception true)))
                      m)))
                 (recur))))
           (finally
             (__socket-close* socket))))
    (let [next-line (if (__socket-conn?* in-reader)
                      (fn [] (__socket-read-line* in-reader))
                      (fn [] (binding [*in* in-reader] (read-line))))]
      ;; W6: upstream closes the socket here, which on the JDK still
      ;; delivers in-flight replies to the reader thread; this port
      ;; half-closes instead, so the server reads EOF while pending
      ;; responses stay readable (a full close would discard them).
      (try (loop []
             (let [line (next-line)]
               (when-not (nil? line)
                 (__socket-write* socket (str line "\n"))
                 (recur))))
           (finally
             (__socket-shutdown-write* socket))))))
