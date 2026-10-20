---
title: "Sync the vault across machines"
description: "Preview and run one Git vault-sync cycle, then confirm status on each machine that holds the vault."
tags:
  - CLI
  - Obsidian
---

# Sync the vault across machines

This guide runs one lock-protected Git reconcile for a vault you already
clone from a remote you control. `bob vault-sync` is the vault's Git engine:
commit local changes, fetch `origin/master`, fast-forward or merge, and
push. Close Obsidian Sync on that vault while this command is the engine.

## Before you start

Install the CLI. The vault directory must be a Git checkout of `master` with
a working `origin` remote and credentials that can fetch and push
unattended. `bob vault-sync` reads `BOB_DIR` (then `~/bob`). It has no
`--bob-dir` / `-b`. Set `BOB_DIR` in the shell on each machine.

```bash
export BOB_DIR=/path/to/vault
git -C "$BOB_DIR" rev-parse --abbrev-ref HEAD
git -C "$BOB_DIR" remote -v
```

Confirm the branch is `master` and `origin` points at your remote, for
example `git@github.com:example/vault.git`. Configure Git to keep
maintenance in the foreground so a later cycle is not blocked on
`.git/HEAD.lock`:

```bash
git -C "$BOB_DIR" config maintenance.autoDetach false
git -C "$BOB_DIR" config gc.autoDetach false
```

## 1. Preview one cycle

```bash
export BOB_DIR=/path/to/vault
bob vault-sync run --dry-run
```

`--dry-run` reports the cycle without staging, committing, merging, or
pushing, and leaves the status file unchanged. Confirm it exits 0 and names
the steps it would take. Close other writers that share `bob_sync.lock`
(`bob nightly`, writing `bob ref scan`, `bob task reconcile`) before a real
run.

## 2. Run the cycle and read status

```bash
bob vault-sync
bob vault-sync status
```

A bare `bob vault-sync` runs one cycle. Confirm `last_error: none` and that
`local_sha` matches `remote_sha` after a successful push. Files at or above
95 MiB are refused before staging; 50 MiB warns. `_conflicts/` holds
quarantined conflict copies and is excluded from Bob's vault walkers, so
those files do not appear on dashboards.

## 3. Repeat from the other machine

On the second machine, set the same `BOB_DIR` to that checkout and run
`bob vault-sync run --dry-run`, then `bob vault-sync`. Confirm
`bob vault-sync status` shows a current `last_success_at` and matching SHAs.
Reload Obsidian so it rereads the updated Markdown.

## If it goes wrong

If `last_error` starts with `halted:`, an interrupted merge was already
aborted and retried once. Stop and follow
[Resolve a vault-sync conflict copy](resolve-vault-conflict-copy.md) for the
halted-merge steps. If `conflicts:` lists paths, remote content won in place
and a local copy was saved under `_conflicts/`; resolve those copies on that
same page. Do not run `git reset --hard` or force-push.

## Related

- [Bob vault Git sync runbook](../vault-git-sync.md)
- [bob vault-sync](../reference/cli/vault-sync.md)
- [Point bob at your vault](point-bob-at-your-vault.md)
- [Resolve a vault-sync conflict copy](resolve-vault-conflict-copy.md)
