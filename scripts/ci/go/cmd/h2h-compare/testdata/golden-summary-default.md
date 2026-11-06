## h2h recipes: jvm vs rs

common cases: 8 (jvm shards 2, rs shards 1); both-win 4/8

| metric | rs wins | losses | ties |
| --- | ---: | ---: | ---: |
| CPU (median cpu_s) | 4 | 2 | 2 |
| MEM (median maxrss_kb) | 6 | 1 | 1 |

worst rs cpu ratio: infx (sort/bubble_sort/test_case_3); worst rs mem ratio: 1.500x (sort/bubble_sort/test_case_1)

### GATE: FAIL

2 CPU losses; 1 MEM losses; 1 not-ok cases; 1 test-count drifts; leg mismatch (only-jvm 1, only-rs 1); sweep failures (jvm 2, rs 1); missing cases (jvm 1, rs 1)

<details><summary>CPU losses (2, worst first)</summary>

| ratio | case | jvm | rs |
| ---: | --- | ---: | ---: |
| 2.500x | sort/bubble_sort/test_case_1 | 1.000s | 2.500s |
| 1.005x | sort/bubble_sort/test_case_5 | 2.000s | 2.010s |
</details>

<details><summary>MEM losses (1, worst first)</summary>

| ratio | case | jvm | rs |
| ---: | --- | ---: | ---: |
| 1.500x | sort/bubble_sort/test_case_1 | 100000K | 150000K |
</details>
