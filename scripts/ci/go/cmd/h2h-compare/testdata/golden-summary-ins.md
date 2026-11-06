## h2h recipes-rsrs: rs-to vs rs-from

common cases: 8 (rs-to shards 2, rs-from shards 1); both-win 4/8

| metric | rs-from wins | losses | ties |
| --- | ---: | ---: | ---: |
| INS (median instructions) | 4 | 1 | 3 |

worst rs-from ins ratio: 2.500x (sort/bubble_sort/test_case_1)

### GATE: FAIL

1 INS losses; 1 not-ok cases; 1 test-count drifts; leg mismatch (only-rs-to 1, only-rs-from 1); sweep failures (rs-to 2, rs-from 1); missing cases (rs-to 1, rs-from 1)

<details><summary>INS losses (1, worst first)</summary>

| ratio | case | rs-to | rs-from |
| ---: | --- | ---: | ---: |
| 2.500x | sort/bubble_sort/test_case_1 | 1,000,000 | 2,500,000 |
</details>
