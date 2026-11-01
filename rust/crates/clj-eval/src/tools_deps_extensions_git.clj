;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-124 slice 2: port of clojure.tools.deps.extensions.git
;; (org.clojure/tools.deps 0.31.1646, extensions/git.clj) — the
;; network-free arms only: `auto-git-url`, `full-sha?`,
;; `coord-type-keys`, `canonicalize` validation, `dep-id`,
;; `coord-summary`, `license-info`, and the equal-sha fast path of
;; `compare-versions`.
;;
;; Port decisions (see docs/gap-124/slice-2.md N1/B11):
;; - Every arm needing `gitlibs` (tag validity/resolution,
;;   procure, tag listing, ancestor checks) throws not-yet ex-info
;;   naming the git-network slice; pure arms are upstream-exact.
;; - `lib-location :git` stays in the shell (B8 layout rule).
;; - The both-tags message passes `tag` (not `lib`) to `format`,
;;   exactly as upstream does.
;; No syntax-quote or `::keywords` anywhere (the loader reads the
;; whole file before evaluating it — the spec_alpha.clj discipline).
(ns clojure.tools.deps.extensions.git
  (:require [clojure.tools.deps.extensions :as ext]))

(def ^:private git-services
  {:github {:service #"^(?:com|io).github.([^.]+)$" :url "https://github.com/%s/%s.git"}
   :gitlab {:service #"^(?:com|io).gitlab.([^.]+)$" :url "https://gitlab.com/%s/%s.git"}
   :codeberg {:service #"^(?:org|page).codeberg.([^.]+)$" :url "https://codeberg.org/%s/%s.git"}
   :bitbucket {:service #"^(?:org|io).bitbucket.([^.]+)$" :url "https://bitbucket.org/%s/%s.git"}
   :beanstalk {:service #"^(?:com|io).beanstalkapp.([^.]+)$" :url "https://%s.git.beanstalkapp.com/%s.git"}
   :sourcehut {:service #"^ht.sr.([^.]+)$" :url "https://git.sr.ht/~%s/%s"}})

(defn auto-git-url
  "Create url from lib name, ie:
    io.github.foo/bar => https://github.com/foo/bar.git
   or return nil if not a name that can be auto converted to a url."
  [lib]
  (let [group (namespace lib)
        project (name lib)]
    (when group
      (some (fn [{:keys [service url]}]
              (when-let [matches (re-matches service group)]
                (format url (second matches) project)))
            (vals git-services)))))

(defn full-sha?
  [sha]
  (boolean (and sha (= 40 (count sha)))))

(defmethod ext/coord-type-keys :git
  [_type]
  #{:git/url :git/sha :git/tag :sha})

(defn- coord-err
  [msg lib coord]
  (ex-info msg {:lib lib :coord coord}))

(defn- make-standard
  [coord url sha tag]
  (->
    (cond-> coord
      url (assoc :git/url url)
      sha (assoc :git/sha sha)
      tag (assoc :git/tag tag))
    (dissoc :sha :tag)))

(defmethod ext/canonicalize :git
  [lib {unsha :sha untag :tag :git/keys [url sha tag] :as coord} _config]
  (when (nil? (namespace lib)) (throw (coord-err (format "Invalid lib name: %s" lib) lib coord)))
  (when (and unsha sha) (throw (coord-err (format "git coord has both :sha and :git/sha for %s" lib) lib coord)))
  (when (and untag tag) (throw (coord-err (format "git coord has both :tag and :git/tag for %s" tag) lib coord)))

  (let [canon-sha (or sha unsha)
        canon-tag (or tag untag)
        canon-url (or url (auto-git-url lib))]
    (when (nil? canon-url)
      (throw (coord-err (format "Failed to infer git url for: %s" lib) lib coord)))
    ;; Any tag needs the network (validity + sha/tag agreement via
    ;; gitlibs), so all tag coords defer here (B11).
    (when canon-tag
      (throw (coord-err (str "Cannot canonicalize git tag for " lib ": deferred to the git-network slice") lib coord)))
    (if canon-sha
      (if (full-sha? canon-sha)
        [lib (make-standard coord canon-url canon-sha canon-tag)]
        (throw (ex-info (format "Library %s has prefix sha, use full sha or add tag" lib) {:lib lib :coord coord})))
      (throw (ex-info (format "Library %s has coord with missing sha" lib) {:lib lib :coord coord})))))

(defn- to-canonical
  [lib {:git/keys [url sha] :as coord} config]
  (if (and url (full-sha? sha))
    [lib coord]
    (ext/canonicalize lib coord config)))

(defmethod ext/dep-id :git
  [lib coord config]
  (let [[_lib {:git/keys [url sha]}] (to-canonical lib coord config)]
    {:git/url url, :git/sha sha}))

(defmethod ext/manifest-type :git
  [lib coord config]
  ;; Validation first (pure, upstream-exact), then the procure
  ;; itself defers: cloning needs the git-network slice (B11).
  (let [[lib {:git/keys [url sha]}] (to-canonical lib coord config)]
    (throw (ex-info (str "Cannot procure git coordinate for " lib ": deferred to the git-network slice")
             {:lib lib :coord coord}))))

(defmethod ext/coord-summary :git [lib coord]
  (let [[lib {:git/keys [url tag sha]}] (to-canonical lib coord nil)]
    (str lib " " (if tag tag (subs sha 0 7)))))

(defmethod ext/license-info :git
  [lib coord config]
  (let [coord (merge coord (ext/manifest-type lib coord config))]
    (ext/license-info-mf lib coord (:deps/manifest coord) config)))

;; 0 if x and y are the same commit; the ancestor check needs the
;; network (B11).
(defmethod ext/compare-versions [:git :git]
  [lib x-coord y-coord config]
  (let [[_ {x-url :git/url, x-sha :git/sha, :as x}] (to-canonical lib x-coord config)
        [_ {y-url :git/url, y-sha :git/sha, :as y}] (to-canonical lib y-coord config)]
    (if (= x-sha y-sha)
      0
      (throw (ex-info (str "Cannot compare git versions for " lib ": deferred to the git-network slice")
               {:x x :y y})))))

(defmethod ext/find-versions :git
  [lib coord _coord-type _config]
  (let [url (or (:git/url coord) (auto-git-url lib))]
    ;; No url, no versions (upstream-exact nil); a url would need
    ;; the network to list tags, so defer rather than report none.
    (when url
      (throw (ex-info (str "Cannot list git versions for " lib ": deferred to the git-network slice")
               {:lib lib :coord coord})))))
