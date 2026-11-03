# Note-pull release latency

- Source revision: `56d8dac3` (verification working-tree changes included: yes)
- Build: `cargo build --release --locked --bin bob` (`bob`)
- Host: `athena` (Linux 6.12.113+deb13-amd64)
- Isolated dataset: 6,906 Markdown notes, 24,134,820 bytes including the Tasks settings and synthetic day note
- Largest root area/project note: `sase.md` (43,755 bytes)
- Inbox route: `mac_inbox.md` (718 bytes)
- Command: `bob capture --dry-run -f json -b <isolated-vault> -- @<route>`
- Clock/day: `BOB_NOW=2026-11-03 09:30:00`, `BOB_DAY_FILE=<isolated-vault>/20261103.md`, `TZ=UTC0`
- Sampling: one warm-up and 20 timed samples per route; timings include process startup and JSON output.
- Acceptance: p95 at most 70 ms per route; `sase` misses the target (87.67 ms p95); `mac_inbox` meets the target (63.98 ms p95).

| Route | Exit codes (warm-up + samples) | Median | p95 | Maximum |
| --- | --- | ---: | ---: | ---: |
| `sase` | 21 success / 0 empty-pull refusals | 78.18 ms | 87.67 ms | 88.28 ms |
| `mac_inbox` | 0 success / 21 empty-pull refusals | 40.62 ms | 63.98 ms | 64.92 ms |
