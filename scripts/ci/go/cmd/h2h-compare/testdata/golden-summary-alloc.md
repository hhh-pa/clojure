## h2h recipes-alloc: jvm vs rs

common cases: 8 (jvm shards 2, rs shards 1); both-win 4/8

| metric | rs wins | losses | ties |
| --- | ---: | ---: | ---: |
| ALLOC (median allocated bytes) | 4 | 1 | 3 |

worst rs alloc ratio: 2.500x (sort/bubble_sort/test_case_1)

### GATE: FAIL

1 ALLOC losses; 1 not-ok cases; 1 test-count drifts; leg mismatch (only-jvm 1, only-rs 1); sweep failures (jvm 2, rs 1); missing cases (jvm 1, rs 1)

<details><summary>ALLOC losses (1, worst first)</summary>

| ratio | case | jvm | rs |
| ---: | --- | ---: | ---: |
| 2.500x | sort/bubble_sort/test_case_1 | 100,000B | 250,000B |
</details>
