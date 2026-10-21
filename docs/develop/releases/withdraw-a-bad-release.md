---
title: Withdraw a bad release
description: Yank a live crates.io version or correct a rehearsal GitHub Release without moving tags.
---

# Withdraw a bad release

Use the action for the release mode that produced the version. A withdrawal does not
reset the daily guard.

## Before you start

Identify the version and whether the train was in rehearsal or live mode. In live mode,
you need a crates.io account with permission to manage bob-obsidian.

## 1. Yank a live crates.io version

~~~bash
cargo yank bob-obsidian --version X.Y.Z
~~~

Yanking prevents new installs of that version through crates.io. It does not delete the
version or erase its publication time; the guard continues to count it for that New
York date.

## 2. Correct a rehearsal release

For rehearsal mode, edit or mark the matching GitHub Release so readers can see that
the version is withdrawn. Keep the vX.Y.Z tag where it is. Never move or delete a tag
to try to make the guard run again.

## If it goes wrong

If the version was published to crates.io, do not treat a GitHub Release edit as a
withdrawal. Check the crates.io version state and use cargo yank for the registry
version. See [Recover a failed release](recover-a-failed-release.md) for a missing tag
or Release after publication.

## Related

- [Ship a hotfix](ship-a-hotfix.md)
- [Recover a failed release](recover-a-failed-release.md)
- [Release train reference](reference.md)
