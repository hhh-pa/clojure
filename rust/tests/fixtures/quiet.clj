;   Copyright (c) Hao Hu. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.

; Fixture: `run` prints program output only (S7A, pins P7).
; Definitions and values produce no per-form echo.
(def qx 41)
(+ qx 1)
