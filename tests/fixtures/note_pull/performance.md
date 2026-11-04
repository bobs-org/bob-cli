# Note-pull release latency

- Source revision: `d25080e1` (verification working-tree changes included: yes)
- Build: `cargo build --release --locked --bin bob` (`bob`)
- Host: `athena` (Linux 6.12.113+deb13-amd64)
- Isolated dataset: 6,907 Markdown notes, 24,171,748 bytes including the Tasks settings and synthetic day note
- Largest root area/project note: `sase.md` (40,987 bytes)
- Inbox route: `mac_inbox.md` (718 bytes)
- Command: `bob capture --dry-run -f json -b <isolated-vault> -- @<route>`
- Clock/day: `BOB_NOW=2026-11-03 09:30:00`, `BOB_DAY_FILE=<isolated-vault>/20261103.md`, `TZ=UTC0`
- Sampling: one warm-up and 20 timed samples per route; timings include process startup and JSON output.
- Acceptance: p95 at most 70 ms per route; `sase` meets the target (50.41 ms p95); `mac_inbox` meets the target (30.37 ms p95).

| Route | Exit codes (warm-up + samples) | Median | p95 | Maximum |
| --- | --- | ---: | ---: | ---: |
| `sase` | 21 success / 0 empty-pull refusals | 47.58 ms | 50.41 ms | 53.59 ms |
| `mac_inbox` | 0 success / 21 empty-pull refusals | 19.17 ms | 30.37 ms | 31.27 ms |
