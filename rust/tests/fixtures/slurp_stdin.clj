; Fixture: `run` over piped stdin — `(slurp *in*)` drains the
; whole input (stdin scripts read their input this way).
(print (slurp *in*))
