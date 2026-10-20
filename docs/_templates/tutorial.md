---
title: "Tutorial: See today's plan"
description: "Read today's Pomodoro plan from a vault in about two minutes."
search:
  boost: 2
---

# Tutorial: See today's plan

This tutorial walks you through reading today's plan with `bob plan` in about two
minutes. Follow the steps in order; each one builds on the last.

## Before you start

You need Bob installed and a vault directory. The command is read-only: it does not
create a daily note or change tasks. Point `--bob-dir` (`-b`) at the vault; it defaults
to `BOB_DIR` or `~/bob`. The contract is docs/plan.md.

## 1. Print the plan

```bash
bob plan -b /path/to/vault
```

```text { .no-copy title="Output" }
bob plan - Tue 2026-10-20 - no daily note yet

  PLAN  0/5 themes - 0/15 links      TODAY 0 - PENDING 0/5 - NEXT 0/15
```

An empty vault with no daily note still exits 0 and prints TODAY 0 plus the lane
counts. That line is the confirmation that Bob found the vault and read the plan.

## 2. Read the counts

The `PLAN` meter is themes and Task Links under today's open Pomodoros. `TODAY`,
`PENDING`, and `NEXT` are the lane counts from the Tasks index. A missing daily note
is a valid empty plan, not an error.

## You have seen today's plan

You have printed the read-only plan budget from a vault. Tutorials teach one path;
options, alternatives, and reasons belong in guides, concepts, or reference pages.

## Next

- [Getting started](../getting-started.md)
- [Plan budget, Today, and lanes](../plan.md)
