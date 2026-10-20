---
title: "Is Bob for me?"
description: "Who Bob fits, what the CLI covers alone, and when Obsidian or Mac Capture helps."
search:
  boost: 2
---

# Is Bob for me?

Bob is an opinionated Get Things Done workflow for people who keep their work in
plain Markdown and want the computer to enforce the ritual. Capture everything,
choose a small Today, work it in Pomodoros, review the queue, and sync the vault
through Git. If that loop sounds like relief rather than overhead, Bob fits.

Bob assumes a technically capable Obsidian reader. You edit Markdown directly,
you are comfortable with a terminal, and you treat settings files as part of the
system. Bob never hides the vault: every task, Pomodoro, and review mark is a
line you can read and repair by hand.

## What the Rust CLI covers

The CLI alone captures tasks, links Today, runs Pomodoros, reports the plan
budget, and keeps the review queue honest. It reads Dataview-style task lines
and writes the same vault Obsidian opens. The
[first Bob day](first-bob-day.md) uses only the CLI in a synthetic vault.

The CLI does not render dashboards, task cards, or review walks. Those live in
Obsidian through Tasks and the custom Bob plugins.

## What needs Obsidian plugins

Install Tasks with the Dataview format and the `#task` filter, plus the Bob
Ledger Tools and Bob Navigation Hotkeys plugins, to get the desktop review:
rendered task cards, the tiered `]s` walk, freshness marks, plan badges, and
the Task Card. [Continue in Obsidian](obsidian.md) opens the demo vault there
and confirms the settings.

Syncing plugins with `bob plugins sync` deploys files. It does not enable
plugins or create dashboards; those remain manual steps in Obsidian.

## What Mac Capture adds

Bob Mac Capture is an optional menu-bar panel for fast capture on macOS. It
never parses grammar or writes the vault itself: it runs the same `bob`
binary, renders the preview, and submits one `bob capture` call. If you live
in the terminal or in Obsidian, skip it. See
[Continue with Mac Capture](mac-capture.md) when a global hotkey earns its
place.

## Next

- [Install Bob](install.md) — checkout, toolchain, and verification
- [Your first Bob day](first-bob-day.md) — the CLI path in about 15 minutes
