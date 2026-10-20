---
title: "Tutorial: Your first Bob day"
description: "Capture a task, work from today's ledger, finish it, and inspect review."
search:
  boost: 2
---

# Your first Bob day

In about 15 minutes, you will capture two tasks in a synthetic gardening vault,
choose one for today's work, finish it, and inspect the review queue. Use one
Bash terminal so the variables below remain available. This path exercises the Bob
CLI; reviewing a task is a separate desktop continuation.

## Before you start

Install Bob from a checkout of the public [bob-cli repository](https://github.com/bobs-org/bob-cli).
The commands below start from that checkout's root. They always copy the bundled
demo into a new temporary directory and select that copy explicitly, even when
`BOB_DIR` already points somewhere else.

## 1. Copy and select a fresh demo vault

```bash
set -euo pipefail
BOB_TUTORIAL_SOURCE="$PWD/examples/demo-vault"
BOB_TUTORIAL_TMP="$(mktemp -d "${TMPDIR:-/tmp}/bob-first-day.XXXXXX")"
BOB_TUTORIAL_VAULT="$BOB_TUTORIAL_TMP/vault"
mkdir -p "$BOB_TUTORIAL_VAULT"
cp -R "$BOB_TUTORIAL_SOURCE/." "$BOB_TUTORIAL_VAULT/"
export BOB_DIR="$BOB_TUTORIAL_VAULT"
printf 'Using demo vault: %s\n' "$BOB_DIR"
```
<!-- tutorial-test: checkpoint fixture-copy -->

```text { .no-copy title="Output" }
Using demo vault: /temporary/path/bob-first-day.xxxxxx/vault
```

The output path is new for this run. Bob will use that copy rather than your
default vault. The fixture includes two areas, a project, and a synthetic
reference; your captures will add two tasks.

## 2. Capture an inbox task

```bash
bob capture 'Sort the seed packets @mac_inbox^seed-packet'
```
<!-- tutorial-test: checkpoint inbox-captured -->

```text { .no-copy title="Output" }
✓ captured  mac_inbox.md
  - [ ] #task Sort the seed packets [created::2026-10-20] ^seed-packet
```

The `^seed-packet` block ID gives this task a stable link target. Bob does not
stamp a new task as reviewed, so it enters the freshness review queue.

## 3. Route another task to the garden

```bash
bob capture 'Water the seedlings @garden^seedlings'
```
<!-- tutorial-test: checkpoint area-captured -->

```text { .no-copy title="Output" }
✓ captured  garden.md
  - [ ] #task Water the seedlings [created::2026-10-20] ^seedlings
```

The route writes to `garden.md`; the ID makes the task easy to link from the
daily ledger. New unstamped tasks enter review even when they live in an area.

## 4. Inspect the plan

```bash
bob plan
```
<!-- tutorial-test: checkpoint initial-plan -->

The plan reads the selected vault and today's ledger. The fixture does not
include a dated daily note yet, so Bob reports an empty Today list. Bob does not
create daily files for you.

## 5. Create today's daily-note skeleton

Create the ledger before starting a session. This step requires Bash: it uses
Bash-specific `[[ ... =~ ... ]]` and `BASH_REMATCH`, and the tutorial runner
executes fences in a persistent Bash session. The command honors Bob's fixed
`BOB_NOW` clock when one is set and otherwise uses your local date.

```bash
if [[ "${BOB_NOW:-}" =~ ^([0-9]{4}-[0-9]{2}-[0-9]{2}) ]]; then
  BOB_TUTORIAL_DAY="${BASH_REMATCH[1]}"
else
  BOB_TUTORIAL_DAY="$(date +%F)"
fi
BOB_TUTORIAL_YEAR="${BOB_TUTORIAL_DAY%%-*}"
BOB_TUTORIAL_COMPACT="${BOB_TUTORIAL_DAY//-/}"
BOB_TUTORIAL_DAILY="$BOB_DIR/$BOB_TUTORIAL_YEAR/$BOB_TUTORIAL_COMPACT.md"
mkdir -p "$(dirname "$BOB_TUTORIAL_DAILY")"
cat > "$BOB_TUTORIAL_DAILY" <<EOF
# Sprout Shed — $BOB_TUTORIAL_DAY

## Pomodoros
EOF
export BOB_DAY_FILE="$BOB_TUTORIAL_DAILY"
printf 'Created daily note: %s\n' "$BOB_TUTORIAL_DAILY"
```
<!-- tutorial-test: checkpoint daily-note -->

The ledger starts with an empty `## Pomodoros` section. Bob needs this file and
section before it can add today's work.

## 6. Start a named Pomodoro

```bash
bob capture '=#focus'
```
<!-- tutorial-test: checkpoint pomodoro-started -->

The named `FOCUS` session is now open. Starting it alone does not change a task
to Pending or mark a task as Today; Today comes from a Task Link in an open
Pomodoro.

## 7. Link the garden task to today's work

```bash
bob capture '@garden:seedlings'
bob plan -f json
```
<!-- tutorial-test: checkpoint task-linked -->

The task stays in `garden.md` and becomes Next. Its dedicated Task Link is a
direct child of the open `FOCUS` Pomodoro, so `bob plan` now includes it in
Today. Lane status and Today are separate facts: the task line records Next,
while the open ledger link records today's work.

## 8. Finish the linked task

```bash
bob capture 'x!'
```
<!-- tutorial-test: checkpoint task-completed -->

`x!` closes the open Pomodoro and completes its one linked task. The zero-minute
outcome belongs to the automated fixed-clock replay (`BOB_NOW`); a human
following these instructions normally uses their local clock and records real
elapsed minutes. No time is invented. The completed task leaves Today while
the inbox task remains open for review.

## 9. Inspect the review queue

```bash
bob freshness list -f json
```
<!-- tutorial-test: checkpoint review-listed -->

This read-only list includes the unfinished inbox task because it has not been
confirmed. The freshness command inspects the queue; answering it is a desktop
continuation with Bob's Obsidian plugins.

## Continue

The terminal path ends here. Continue on the desktop with the same demo copy:

- [Continue in Obsidian](obsidian.md) — open the demo in Obsidian and confirm one review gesture
- [Continue with Mac Capture](mac-capture.md) — submit one capture from the menu-bar panel on macOS

Then read the contracts this tutorial exercised:

- [Getting started contract](../getting-started.md)
- [Capture syntax](../reference/capture-syntax.md)
- [Vault format](../reference/vault-format.md)
- [JSON outputs](../reference/json-outputs.md)
- [Configuration](../reference/configuration.md)
- [Plan budget, Today, and lanes](../plan.md)
- [Task freshness](../freshness.md)
