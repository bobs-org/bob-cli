---
keyword: Theme Preferences Live In A Per-Host Journal
aliases:
  - theme journal
  - hide journal
  - color journal
summary:
  Hide, unhide, color, and rename events live in vault-synced
  `.bob/themes/<host>.jsonl`; fold is last-writer-wins and bob is the only
  writer.
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli, Bob Mac Capture, Obsidian plugins that hide, pin, or
rename themes.

**Claim.** Preferences live in `.bob/themes/<host>.jsonl`, one append-only file
per host inside the vault so Git syncs them. `host` is the same label
vault-sync uses in commit subjects. Events are `v: 1` `theme.hide`,
`theme.unhide`, `theme.color` (`hue` is a slot name or `null` to unpin), and
`theme.rename`. Fold reads every `*.jsonl` file, skips malformed lines with a
bounded warning, sorts by `(ts, host, path, line)`, and lets the last writer
win per key. A rename moves the source key's explicit state unless the target
already has state. bob is the only writer; other clients call bob.

**Why.** Frontmatter on daily notes would mix derived history with preferences.
A machine-local SQLite file would not sync hide/color across apollo and the
Mac. A single shared JSON file would lose concurrent host appends. Rejected
alternatives:

- **Daily-note frontmatter** — every hide would dirty today's ledger.
- **Machine-local SQLite** — hide on apollo would not reach the Mac.
- **One shared JSON object** — two hosts writing at once would clobber.

Evidence: `plan:202610/pomodoro_themes.md` PT4; `docs/pomodoro-themes.md`.

**Cost.** Fold must skip malformed lines. Rename is a logged event plus a
ledger rewrite, not a journal-only alias. Writers serialize through bob.

**Reopens when.** Concurrent host folds mis-order clocks often enough to need
a vector clock, or preferences must live outside the vault.
