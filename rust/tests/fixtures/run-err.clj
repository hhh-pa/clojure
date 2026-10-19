;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

; Fixture: `run` error position (S7A). The `(if)` on line 13 is a
; compile error ("Too few arguments to if") at 13:1; the earlier
; `println` proves forms before the failure still ran.
(println "before")
(if)
