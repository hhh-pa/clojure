;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Compatibility namespace for upstream src/clj/clojure/gvec.clj.
;;
;; Upstream gvec.clj defines NO namespace: it opens with (in-ns
;; 'clojure.core) and extends clojure.core with a generic primitive-vector
;; implementation (VecNode / IVecImpl / ArrayChunk / VecSeq / Vec, the
;; ArrayManager reifies, ams) whose only public API is clojure.core/vector-of.
;; None of that machinery is ported: clojure-rs implements vector-of natively
;; (typed primitive vectors with the same "Unrecognized type ..." error), so
;; every user-visible behavior of upstream gvec.clj already exists.
;;
;; This file therefore only declares the clojure.gvec namespace (which has no
;; upstream counterpart) so that (require 'clojure.gvec) succeeds uniformly
;; with the other stdlib files. It intentionally defines no vars.

(ns clojure.gvec
  "Compatibility namespace for upstream clojure/gvec.clj, which defines no
  namespace of its own: it extends clojure.core with vector-of, implemented
  natively here. See the file header.")
