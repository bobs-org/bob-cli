---
title: "Resolve a vault-sync conflict copy"
description: "Inspect a vault-sync conflict copy, keep the edits you want, verify, and sync again."
tags:
  - CLI
  - Obsidian
---

# Resolve a vault-sync conflict copy

This guide recovers work after `bob vault-sync` auto-resolved a supported
conflict. Remote content wins in the live note. The previous local bytes are
saved under `_conflicts/`, and `_conflicts/sync_conflicts.md` logs the
event. A halted merge is a different failure: the command stopped before
changing the vault again.

## Before you start

Set `BOB_DIR` to the vault. Have `git` and a pager or diff tool. Do not run
`git reset --hard` or force-push.

## 1. Distinguish a conflict copy from a halted merge

```bash
export BOB_DIR=/path/to/vault
bob vault-sync status
```

If `conflicts:` lists vault-relative paths, continue below. If `last_error`
starts with `halted:` and `halted_merge_head` is a SHA, skip to
[Halted merge](#halted-merge). Confirm there are no leftover conflict
markers in the live tree:

```bash
git -C "$BOB_DIR" status --short
rg -n '<<<<<<<|=======|>>>>>>>' "$BOB_DIR"
```

An empty `rg` result is the expected live tree after a supported auto-resolve.

## 2. Compare the saved copy with the current note

Open the live note and the matching file under `_conflicts/`. The log
`_conflicts/sync_conflicts.md` names both paths. Copy any local edits you
still want into the live note. Conflict copies are committed so they reach
the other machines; they stay out of dashboards because `_conflicts/` is
excluded from Bob's walkers.

## 3. Verify and sync

```bash
rg -n '<<<<<<<|=======|>>>>>>>' "$BOB_DIR"
bob vault-sync run --dry-run
bob vault-sync
bob vault-sync status
```

Confirm no conflict markers remain, the dry run exits 0, and
`last_error: none`. Reload Obsidian on each machine after the push lands.

## Halted merge

A halted cycle found the same interrupted merge after it had already aborted
and retried once. Later cycles keep halting until you finish or abort that
merge by hand:

```bash
git -C "$BOB_DIR" status
```

Finish it with `git add` on the resolved paths and `git commit --no-edit`,
or discard it with `git merge --abort`. A later successful `bob vault-sync`
clears the halt fields. Full recovery notes are in
[Halted sync](../vault-git-sync.md#halted-sync).

## If it goes wrong

Unhandled conflicts abort the merge and set `last_error`. Inspect
`git status`, keep the wanted bytes, and follow the halted-merge steps.
Never use `reset --hard`, force-push, or `-X ours` / `-X theirs` to clear
the vault.

## Related

- [Bob vault Git sync runbook](../vault-git-sync.md)
- [Sync the vault across machines](sync-vault-across-machines.md)
- [bob vault-sync](../reference/cli/vault-sync.md)
