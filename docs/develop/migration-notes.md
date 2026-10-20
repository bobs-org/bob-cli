---
title: Migration notes
description: Canonical names for old bob command spellings and retired capture tokens.
---

# Migration notes

Old command spellings stay as permanent hidden aliases with the same
behavior and no deprecation notices. New integrations should use the
canonical names.

| Old spelling | Canonical path |
| --- | --- |
| `bob mark-next-tasks` | `bob task reconcile` |
| `bob move-done-tasks` | `bob task archive` |
| `bob notify` | `bob pomodoro notify` |
| `bob randomize` | `bob task reroll` |
| `bob task-status-hooks` | `bob task reconcile` |
| `bob task-status-setter` | `bob task reconcile` |
| `bob tmux-pomodoro` | `bob pomodoro tmux` |
| `bob highlights` | `bob ref` |
| `bob highlights-ref` | `bob ref` |
| `bob_pomodoro`, `bob_notify`, `tmux_bob_pomodoro` binaries | unchanged, same leaves |

Reroll and archive commits are labeled `bob task reroll` and
`bob task archive`. Older history says `bob randomize` and
`bob move-done-tasks`.

These top-level names also moved: `bob collect-done` is `bob task archive`,
`bob dataview` is `bob query`, and `bob highlights-ref` is `bob ref`
(`bob highlights` stays as a permanent alias). `bob sync`,
`bob bulk-git-commit`, and the `bob_sync` binary were retired in favor of
`bob vault-sync`. Those retired top-level names are no longer registered.

The original script implementations remain embedded as a rollback path.
New integrations should rely on the native Rust command behavior. See
[Requirements and compatibility](../reference/compatibility.md) for the
shim map and `BOB_CLI_USE_SCRIPT`.

## Capture tokens

The `@<route>::<block-id>` capture spelling is not accepted. Use
`@<route>^<block-id>` for an ordinary task with a requested block ID, and
`@<route>:<block-id>` for a Pomodoro-linked Next task, optionally with
`#<pomodoro>` to select or create a named Pomodoro. Sub-bullet capture uses
`@<route>+<block-id>`.

## Related

- [CLI reference](../reference/cli/index.md)
- [Requirements and compatibility](../reference/compatibility.md)
- [Capture](../capture.md)
- [Release checklist](release-checklist.md)
