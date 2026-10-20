---
title: "Print today's plan as JSON"
description: "Export the read-only plan budget as JSON from a vault named with --bob-dir."
tags:
  - CLI
---

# Print today's plan as JSON

This guide gets you to a JSON snapshot of today's plan. It assumes Bob is installed
and you can point at a vault; if you have not seen the human form yet, start with
[Getting started](../getting-started.md) instead.

## Before you start

You need a vault directory. `bob plan` is read-only. `--format` / `-f` accepts `human`
(the default) or `json`. `--bob-dir` / `-b` selects the vault.

## 1. Choose the vault

Pass `-b` so the command does not depend on `~/bob`:

```bash
bob plan -b /path/to/vault -f json
```

```text { .no-copy title="Output" }
{"caps":{"max_links":15,"max_next":15,"max_pending":5,"max_ready":45,"max_themes":5,"strict":false},"daily_file":"2026/20261020.md","date":"2026-10-20","entries":[],"links":{"cap":15,"count":0,"over":false},"next":{"cap":15,"count":0,"over":false},"ok":true,"pending":{"cap":5,"count":0,"over":false},"schema_version":2,"status":"ok","theme_names":[],"themes":{"cap":5,"count":0,"over":false},"today":{"count":0},"today_tasks":[],"warnings":[]}
```

Confirm `status` is `ok`. On an empty vault `today.count` is 0 and `entries` is empty.

## 2. Keep the flags you verified

`-f json` is the machine-readable form. `-b` overrides `BOB_DIR`. `-h` prints the
same flags. See docs/plan.md for the field meanings.

## If it goes wrong

A missing vault directory fails before any plan is printed. Create the directory or
pass the vault you actually use. A missing daily note is not a failure: the JSON
still reports `ok` with zero counts.

## Related

- [Plan budget, Today, and lanes](../plan.md)
- [Getting started](../getting-started.md)
