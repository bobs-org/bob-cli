---
title: "Point bob at your vault"
description: "Select a vault with --bob-dir or BOB_DIR and confirm a read-only command."
tags:
  - CLI
---

# Point bob at your vault

This guide points `bob` at a vault directory you already have and confirms a
read-only command sees it. Bob does not create a vault.

## Before you start

Install the CLI so `bob` is on your `PATH`. Have an existing vault directory.
The default location is `~/bob`. Many commands accept `--bob-dir` / `-b`; some,
including `bob vault-sync`, read `BOB_DIR` only.

## 1. Pass the vault on a command that supports `-b`

```bash
bob plan -b /path/to/vault
```

```text { .no-copy title="Output" }
bob plan - Tue 2026-10-20 - no daily note yet
```

Confirm the command exits 0. An empty vault still reports `ok` with zero
counts. `-h` on that same command lists `-b` when the option exists.

## 2. Set `BOB_DIR` for the rest of the shell

```bash
export BOB_DIR=/path/to/vault
bob plan
```

The second `bob plan` uses `$BOB_DIR` and prints the same vault. `-b` on a
later command still overrides the environment for that invocation.

## 3. Confirm a second read-only view

```bash
bob ready -b /path/to/vault
```

```text { .no-copy title="Output" }
bob ready · cap 5
  (no area or project notes)
```

An empty vault is a valid result. `bob ready` also accepts `-b`. Commands that
do not list `--bob-dir` in `-h` keep using `BOB_DIR` or `~/bob`.

## If it goes wrong

A missing directory fails before any plan is printed. Create the folder or pass
the vault you actually use. If a command ignores `-b`, check its `-h` text and
set `BOB_DIR` instead.

## Related

- [Getting started](../getting-started.md)
- [bob plan](../reference/cli/plan.md)
- [Configuration](../reference/configuration.md)
- [Sync the vault across machines](sync-vault-across-machines.md)
