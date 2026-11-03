;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Port of upstream src/clj/clojure/java/javadoc.clj (clojure.java.javadoc)
;; for clojure-rs: same public API (*feeling-lucky-url*, *feeling-lucky*,
;; *local-javadocs*, *core-java-api*, *remote-javadocs*, add-local-javadoc,
;; add-remote-javadoc, javadoc-url, javadoc).
;;
;; Port notes:
;; - The (:import (java.io File)) clause is dropped (java.io.File is not in
;;   the emulated lattice): local lookups join path strings, test existence
;;   by opening + closing a clojure.java.io/input-stream, and compose
;;   file: URIs by hand (absolute paths keep the exact upstream shape
;;   file:///....html; relative ones stay relative, since the working
;;   directory is not observable without a JVM).
;; - clojure.string is NOT required (it cannot load at top level while the
;;   loader leaves *warn-on-reflection* unbound): the two replacements are
;;   tiny local helpers. This file loads in any evaluator state.
;; - *core-java-api* defaults to the 21 doc tree: System/getProperty has no
;;   java.specification.version without a JVM.
;; - fill-in-module-name guesses the JPMS module from a prefix table (no
;;   Class/getModule without a JVM): java.base by default, java.desktop for
;;   UI, java.xml for XML, plus the small-name modules below. URLs for
;;   classes outside every entry are best-effort.

(ns
  ^{:author "Christophe Grand, Stuart Sierra",
     :doc "A repl helper to quickly open javadocs."}
  clojure.java.javadoc
  (:use [clojure.java.browse :only (browse-url)])
  (:require [clojure.java.io :as jio]))

(def ^{:dynamic true :deprecated "1.13"} *feeling-lucky-url* nil)
(def ^{:dynamic true :deprecated "1.13"} *feeling-lucky* false)

(def ^:dynamic *local-javadocs* (ref (list)))

(def ^:dynamic *core-java-api*
  ;; Upstream reads (System/getProperty "java.specification.version"), which
  ;; is nil without a JVM; default to the 21 doc tree.
  (str "https://docs.oracle.com/en/java/javase/"
       (or (System/getProperty "java.specification.version") "21")
       "/docs/api/%s/"))

(def ^:dynamic *remote-javadocs*
 (ref (sorted-map
       "com.google.common." "https://guava.dev/releases/snapshot-jre/api/docs/"
       "java." *core-java-api*
       "javax." *core-java-api*
       "org.ietf.jgss." *core-java-api*
       "org.omg." *core-java-api*
       "org.w3c.dom." *core-java-api*
       "org.xml.sax." *core-java-api*
       "org.apache.commons.codec." "https://commons.apache.org/proper/commons-codec/apidocs/"
       "org.apache.commons.io." "https://commons.apache.org/proper/commons-io/apidocs/"
       "org.apache.commons.lang." "https://commons.apache.org/proper/commons-lang/javadocs/api-2.6/"
       "org.apache.commons.lang3." "https://commons.apache.org/proper/commons-lang/apidocs/")))

(defn add-local-javadoc
  "Adds to the list of local Javadoc paths."
  {:added "1.2"}
  [path]
  (dosync (commute *local-javadocs* conj path)))

(defn add-remote-javadoc
  "Adds to the list of remote Javadoc URLs.  package-prefix is the
  beginning of the package name that has docs at this URL."
  {:added "1.2"}
  [package-prefix url]
  (dosync (commute *remote-javadocs* assoc package-prefix url)))

(defn- dotless
  "Upstream (str/replace classname #\"\\$.*\" \"\"): drop any $-suffix."
  [^String classname]
  (apply str (take-while #(not= % \$) classname)))

(defn- dots-to
  "Upstream (.replace classname \\. sep): rewrite dots with sep."
  [^String classname sep]
  (apply str (map #(if (= % \.) sep %) classname)))

(def ^:private module-prefixes
  ;; [package-prefix module-name], most specific first. Upstream calls
  ;; (.getName (.getModule (Class/forName classname))); without JPMS, guess.
  [["java.sql." "java.sql"]
   ["javax.sql." "java.sql"]
   ["java.naming." "java.naming"]
   ["javax.naming." "java.naming"]
   ["java.net.http." "java.net.http"]
   ["java.management." "java.management"]
   ["javax.management." "java.management"]
   ["javax.swing." "java.desktop"]
   ["javax.sound." "java.desktop"]
   ["javax.print." "java.desktop"]
   ["javax.accessibility." "java.desktop"]
   ["javax.imageio." "java.desktop"]
   ["java.awt." "java.desktop"]
   ["javax.script." "java.scripting"]
   ["javax.tools." "java.compiler"]
   ["javax.lang.model." "java.compiler"]
   ["javax.annotation.processing." "java.compiler"]
   ["java.rmi." "java.rmi"]
   ["java.security.jgss." "java.security.jgss"]
   ["org.ietf.jgss." "java.security.jgss"]
   ["javax.xml." "java.xml"]
   ["javax.crypto." "java.base"]
   ["javax.net." "java.base"]
   ["org.w3c." "java.xml"]
   ["org.xml." "java.xml"]
   ["org.omg." "java.corba"]])

(defn- module-for
  [^String classname]
  (or (some (fn [[prefix module]]
              (when (.startsWith classname prefix) module))
            module-prefixes)
      "java.base"))

(defn- fill-in-module-name [url ^String classname]
  ;; Upstream: (if (.contains url "%s") (format url module-of-class) url).
  (if (re-find #"%s" url)
    (format url (module-for classname))
    url))

(defn- file-exists?
  "Upstream (.exists ^File %): probe by opening + closing an input stream."
  [path]
  (try
    (let [s (jio/input-stream path)]
      (.close s)
      true)
    (catch Throwable _ false)))

(defn- join-path
  "Upstream (File. parent child): join with the file separator."
  [parent child]
  (let [sep (System/getProperty "file.separator")]
    (if (.endsWith (str parent) sep)
      (str parent child)
      (str parent sep child))))

(defn- file-uri
  "Upstream (-> file .toURI str): absolute paths keep the exact file:///
  shape; relative ones stay relative (see the file header)."
  [path]
  (let [path (str path)]
    (if (.startsWith path "/")
      (str "file://" path)
      (str "file:" path))))

(defn javadoc-url
  "Searches for a URL for the given class name.  Tries
  *local-javadocs* first, then *remote-javadocs*.  Returns a string."
  {:tag String,
   :added "1.2"}
  [^String classname]
  (let [classname (dotless classname)
        sep (System/getProperty "file.separator")
        file-path (dots-to classname sep)
        url-path (dots-to classname "/")]
    (if-let [file (first
                   (filter file-exists?
                     (map #(join-path % (str file-path ".html"))
                       @*local-javadocs*)))]
      (file-uri file)
      ;; If no local file, try remote URLs:
      (some (fn [[prefix url]]
                  (when (.startsWith classname prefix)
                    (str (fill-in-module-name url classname)
                         url-path ".html")))
            @*remote-javadocs*))))

(defn javadoc
  "Opens a browser window displaying the javadoc for the argument.
  Tries *local-javadocs* first, then *remote-javadocs*."
  {:added "1.2"}
  [class-or-object]
  (let [^Class c (if (instance? Class class-or-object)
                    class-or-object
                    (class class-or-object))]
    (if-let [url (javadoc-url (.getName c))]
      (browse-url url)
      (println "Could not find Javadoc for" c))))
