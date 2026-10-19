# T1174 slice progress (update as slices land)

`#` landed · `>` active · `-` pending.

```
[#####>---] 5/9 slices landed
```

| Slice | State | Landed as |
| ----- | ----- | --------- |
| S1 scaffold+CI | landed | (pre-S5 history) |
| S2 reader+printer | landed | (pre-S5 history) |
| S3 values+collections | landed | (pre-S5 history) |
| S4 evaluator | landed | (pre-S5 history) |
| S5 namespaces/core | landed | `8115f1c7` (PR #6, 3-round review) |
| S6 concurrency | active (plan) | — |
| S7 repl+cli | pending | — |
| S8 llvm-aot | pending | — |
| S9 hardening | pending | — |

End state (post-S9): remove the original Java implementation.
