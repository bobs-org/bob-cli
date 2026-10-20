---
title: "bob plan"
description: "Read-only flags and defaults for bob plan, which prints today's ledger budget."
---

# bob plan

This reference records `bob plan`'s exact flags, defaults, and behavior. Guides link
here for facts; this page links back to the tasks that use them. The contract is
docs/plan.md.

## Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `-b`, `--bob-dir <DIR>` | `BOB_DIR` or `~/bob` | Vault root |
| `-f`, `--format <FORMAT>` | `human` | `human` or `json` |
| `-h`, `--help` | | Show help |

The command is read-only. A missing daily note or missing Pomodoros section still
reports TODAY 0 and the lanes, and exits 0.

Put generated values inside a marked block produced by `just docs-gen`; never
hand-edit generated content.

## Related

- [Getting started](../getting-started.md)
- [Plan budget, Today, and lanes](../plan.md)
