# Note-pull release latency

- Source revision: `3d1822be` (verification working-tree changes included: yes)
- Build: `cargo build --release --locked --bin bob` (`bob`)
- Host: `athena` (Linux 6.12.113+deb13-amd64)
- Isolated dataset: 6,853 Markdown notes, 23,897,300 bytes including the Tasks settings and synthetic day note
- Largest root area/project note: `sase.md` (43,547 bytes)
- Inbox route: `mac_inbox.md` (718 bytes)
- Command: `bob capture --dry-run -f json -b <isolated-vault> -- @<route>`
- Clock/day: `BOB_NOW=2026-11-03 09:30:00`, `BOB_DAY_FILE=<isolated-vault>/20261103.md`, `TZ=UTC0`
- Sampling: one warm-up and 20 timed samples per route; timings include process startup and JSON output.
- Acceptance: p95 at most 70 ms per route; `sase` misses the target (82.60 ms p95); `mac_inbox` meets the target (44.88 ms p95).

| Route | Exit codes (warm-up + samples) | Median | p95 | Maximum |
| --- | --- | ---: | ---: | ---: |
| `sase` | 21 success / 0 empty-pull refusals | 78.17 ms | 82.60 ms | 83.00 ms |
| `mac_inbox` | 0 success / 21 empty-pull refusals | 39.67 ms | 44.88 ms | 51.09 ms |
