# T1174 slice progress (update as slices land)

`#` landed · `>` active · `-` pending.

```
[#######>-] 7/9 slices landed
```

| Slice | State | Landed as |
| ----- | ----- | --------- |
| S1 scaffold+CI | landed | (pre-S5 history) |
| S2 reader+printer | landed | (pre-S5 history) |
| S3 values+collections | landed | (pre-S5 history) |
| S4 evaluator | landed | (pre-S5 history) |
| S5 namespaces/core | landed | `8115f1c7` (PR #6, 3-round review) |
| S6 concurrency | landed | `0c950c8f` (PR #7, 2-round review) |
| S7 repl+cli | landed | `20707c38` (PR #8) |
| S8 llvm-aot | active | — |
| S9 hardening | pending | — |

End state (post-S9): remove the original Java implementation.
