;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/java/process.clj (clojure.java.process)
;; for clojure-rs: same public API (to-file, from-file, start, stdin, stdout,
;; stderr, exit-ref, io-task, exec).
;;
;; There is no java.lang.Process without a JVM, so start is emulated over
;; clojure.java.shell (run-to-completion capture) instead of ProcessBuilder.
;; The resulting narrows, all documented on the fns they touch:
;; - LAZY START: when :in is :pipe (the default), the child spawns on first
;;   need (first stdout/stderr/exit-ref use), so stdin can be written after
;;   start returns. Writes after the first read are ignored. Spawn failures
;;   (bad program, bad :dir) therefore surface on first use, not from start.
;;   Other :in shapes spawn eagerly in a background future.
;; - NO TRUE PIPES: stdin is a temp file (write it with spit before the
;;   first read); stdout/stderr readers hold the full capture, so reads
;;   block until the child exits. Interleaved read/write conversations and
;;   infinite-stream children are unsupported.
;; - :env REPLACES the environment (it merges upstream): without env access
;;   there is no merge. Prefer :clear-env true with a full :env (exact).
;; - :in :inherit reads EOF (the child cannot share our stdin); :out/:err
;;   :inherit print the capture when the child exits (delayed, not live).
;; - :err :stdout concatenates the captures (upstream merges the live
;;   streams; any interleave is timing anyway).
;; - stderr arrives lossy-UTF-8 (the shell facsimile decodes it); to-file
;;   redirects materialize through memory (no streaming to disk).
;; - The Process/Redirect/ThreadFactory/ExecutorService imports, the null-file
;;   delay, and the io-thread-factory/io-executor defoncs are dropped: io-task
;;   is a future (which conveys bindings like the upstream bound-fn*).
;; - (set! *warn-on-reflection* true) is guarded by thread-bound?: the loader
;;   does not bind the var during load, so a bare set! would fail the require.

(ns clojure.java.process
  "A process invocation API wrapping the Java process API.

   The primary function is 'start' which starts a process and handles the
   streams as directed. It returns the Process object. Use 'exit-ref' to wait
   for completion and receive the exit value, and ‘stdout', 'stderr', 'stdin'
   to access the process streams. The 'exec' function handles the common case
   to 'start' a process, wait for process exit, and return stdout.

   Port: 'start' returns an opaque handle map (not a java.lang.Process);
   to-file/from-file return redirect maps (not Redirects); stdin answers a
   temp-file path to spit (or nil unless :in is :pipe); stdout/stderr answer
   readers over the capture. See the file header for the narrows."
  (:require
   [clojure.java.io :as jio]
   [clojure.java.shell :as sh]))

(when (thread-bound? #'*warn-on-reflection*)
  (set! *warn-on-reflection* true))

(defn to-file
  "Coerce f to a file per clojure.java.io/file and return a ProcessBuilder.Redirect writing to the file.
  Set ':append' in opts to append. This can be passed to 'start' in :out or :err.
  Port: answers a redirect map; the capture is spitted after the child exits."
  {:added "1.12"}
  [f & {:keys [append] :as opts}]
  (let [fo (jio/file f)]
    {:redirect :to-file :file (str fo) :append (boolean append)}))

(defn from-file
  "Coerce f to a file per clojure.java.io/file and return a ProcessBuilder.Redirect reading from the file.
  This can be passed to 'start' in :in.
  Port: answers a redirect map; the file is slurped as the child stdin."
  {:added "1.12"}
  [f]
  {:redirect :from-file :file (str (jio/file f))})

(defn- redirect?
  [x kind]
  (and (map? x) (= kind (:redirect x))))

(defn- bad-redirect!
  [opt x]
  (throw (ex-info (str "Invalid " (name opt) " redirect: " (pr-str x)) nil)))

(defn- check-redirect!
  "Upstream this CCEs inside .redirectInput/Output; here ex-info, eagerly."
  [opt x kinds]
  (let [ok (or (contains? kinds x)
               (and (map? x)
                    (contains? #{:to-file :from-file} (:redirect x))
                    (cond (= opt :in) (= :from-file (:redirect x))
                          :else (= :to-file (:redirect x)))))]
    (when-not ok
      (bad-redirect! opt x))))

(defn- stdin-temp-path
  []
  (str "clojure-process-stdin-" (rand-int 1000000000) ".tmp"))

(defn- handle?
  [p]
  (and (map? p) (true? (:process-handle p))))

(defn- handle-state!
  [p op]
  (if (handle? p)
    (:state p)
    (throw (ex-info (str "Not a process handle: " (pr-str p)) nil))))

(defn- run-child!
  "Run the child to completion (the delay body behind every handle): feed
  :in, capture both streams, apply the :out/:err redirects, and merge
  {:exit :stdout-reader :stderr-reader} into the state atom."
  [state]
  (let [{:keys [command in out err dir stdin-path]} @state
        in-str (cond (= in :pipe) (try (slurp stdin-path)
                                       (catch Throwable _ ""))
                     (= in :inherit) nil
                     (= in :discard) nil
                     (redirect? in :from-file) (slurp (:file in)))
        _ (when stdin-path
            (try (jio/delete-file stdin-path) (catch Throwable _ nil)))
        res (apply sh/sh (concat (vec command)
                                 [:in in-str :dir dir
                                  :env (:env-value @state)
                                  :out-enc :bytes]))
        exit (:exit res)
        out-bytes (:out res)
        err-str (:err res)
        ;; :err :stdout folds stderr into wherever :out goes.
        out-text (if (= err :stdout)
                   (str (String. out-bytes) err-str)
                   (String. out-bytes))]
    (cond (= out :inherit) (do (print out-text) (flush))
          (redirect? out :to-file) (spit (:file out) out-text
                                         :append (:append out)))
    (cond (= err :inherit) (binding [*out* *err*] (print err-str) (flush))
          (redirect? err :to-file) (spit (:file err) err-str
                                         :append (:append err)))
    (swap! state merge
           {:exit exit
            :stdout-reader (jio/reader
                            (cond (and (= out :pipe) (= err :stdout))
                                  (char-array out-text)
                                  (= out :pipe) out-bytes
                                  :else (char-array "")))
            :stderr-reader (jio/reader
                            (char-array (if (= err :pipe) err-str "")))})
    (:exit @state)))

(defn- ensure-started!
  [p]
  (let [state (handle-state! p :ensure)]
    (force (:run @state))
    @state))

(defn start
  "Start an external command, defined in args.
  The process environment vars are inherited from the parent by
  default (use :clear-env to clear them).

  If needed, provide options in map as first arg:
    :in - a ProcessBuilder.Redirect (default = :pipe) or :inherit
    :out - a ProcessBuilder.Redirect (default = :pipe) or :inherit :discard
    :err - a ProcessBuilder.Redirect (default = :pipe) or :inherit :discard :stdout
    :dir - current directory when the process runs (default=\".\")
    :clear-env - if true, remove all inherited parent env vars
    :env - {env-var value} of environment variables to set (all strings)

  Returns the java.lang.Process.
  Port: returns an opaque handle map; :in/:out/:err take redirect maps from
  to-file/from-file. :env replaces (see the file header). With :in :pipe the
  child spawns on first stdout/stderr/exit-ref use; otherwise it spawns in a
  background future before start returns."
  {:added "1.12"}
  [& opts+args]
  (let [[opts command] (if (map? (first opts+args))
                         [(first opts+args) (rest opts+args)]
                         [{} opts+args])
        {:keys [in out err dir env clear-env]
         :or {in :pipe, out :pipe, err :pipe, dir "."}} opts
        _ (check-redirect! :in in #{:pipe :inherit :discard})
        _ (check-redirect! :out out #{:pipe :inherit :discard})
        _ (check-redirect! :err err #{:pipe :inherit :discard :stdout})
        dir-str (str (jio/file (or dir ".")))
        env-value (if clear-env (or env {}) env)
        stdin-path (when (= in :pipe) (stdin-temp-path))
        state (atom {:command (vec command) :in in :out out :err err
                     :dir dir-str :env-value env-value
                     :stdin-path stdin-path})]
    (swap! state assoc :run (delay (run-child! state)))
    (let [h {:process-handle true :state state}]
      (when (not= in :pipe)
        (swap! state assoc :bg (future (force (:run @state)))))
      h)))

(defn stdin
  "Given a process, return the stdin of the external process (an OutputStream)
  Port: answers a temp-file path in the working directory: spit the child
  input to it before the first stdout/stderr/exit-ref use (the file is
  consumed and deleted when the child spawns; later writes are ignored).
  Answers nil unless :in is :pipe."
  {:added "1.12"}
  [process]
  (:stdin-path @(handle-state! process :stdin)))

(defn stdout
  "Given a process, return the stdout of the external process (an InputStream)
  Port: answers a reader over the captured stdout (spawning + waiting for the
  child on first use); empty unless :out is :pipe, merged with stderr when
  :err is :stdout. Like a stream, it reads once."
  {:added "1.12"}
  [process]
  (:stdout-reader (ensure-started! process)))

(defn stderr
  "Given a process, return the stderr of the external process (an InputStream)
  Port: answers a reader over the captured stderr (spawning + waiting for the
  child on first use); empty unless :err is :pipe."
  {:added "1.12"}
  [process]
  (:stderr-reader (ensure-started! process)))

(defn exit-ref
  "Given a Process (the output of 'start'), return a reference that can be
  used to wait for process completion then returns the exit value.
  Port: answers a future of the exit code (deref, timeout deref, and
  realized? all behave)."
  {:added "1.12"}
  [process]
  (let [state (handle-state! process :exit-ref)]
    (future (:exit (ensure-started! process)))))

(defn io-task
  {:skip-wiki true}
  [f]
  ;; Upstream submits bound-fn* f to a daemon-thread pool and reifies a
  ;; future-like; a future already conveys bindings and is future-like.
  (future (f)))

(defn exec
  "Execute a command and on successful exit, return the captured output,
  else throw RuntimeException. Args are the same as 'start' and options
  if supplied override the default 'exec' settings.
  Port: failure throws ex-info (no RuntimeException ctor without a JVM);
  the message keeps the upstream shape."
  {:added "1.12"}
  [& opts+args]
  (let [[opts command] (if (map? (first opts+args))
                         [(first opts+args) (rest opts+args)]
                         [{} opts+args])
        opts (merge {:err :inherit} opts)]
    (let [proc (apply start opts command)
          captured (io-task #(slurp (stdout proc)))
          exit (deref (exit-ref proc))]
      (if (zero? exit)
        @captured
        (throw (ex-info (str "Process failed with exit=" exit) nil))))))

(comment
  ;; shell out and inherit the i/o
  (start {:out :inherit, :err :stdout} "ls" "-l")

  ;; write out and err to files, wait for process to exit, return exit code
  @(exit-ref (start {:out (to-file "out") :err (to-file "err")} "ls" "-l"))

  ;; capture output to string
  (-> (start "ls" "-l") stdout slurp)

  ;; with exec
  (exec "ls" "-l")

  ;; read input from file
  (-> (exec {:in (from-file "deps.edn")} "wc" "-l") clojure.string/trim parse-long)
  )
