;   Copyright (c) Rich Hickey. All rights reserved.
;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; W8 (core parity) redesign (issue #204) of upstream
;; `src/clj/clojure/java/browse_ui.clj`: the same `open-url-in-swing`
;; entry point over text rendering to `*out*` (no Swing — there is no
;; GUI toolkit here). Upstream opens the URL in a Swing HTML pane and
;; returns the JFrame; the port prints the URL and returns nil.

(ns
    ^{:author "Christophe Grand",
      :doc "Helper namespace for clojure.java.browse.
            Prevents console apps from becoming GUI unnecessarily."}
  clojure.java.browse-ui)

(defn- open-url-in-swing
  [url]
  (println (str "Browsing " url)))
