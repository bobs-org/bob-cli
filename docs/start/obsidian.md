---
title: "Continue in Obsidian"
description: "Open the demo vault in Obsidian with Tasks and Bob plugins enabled."
search:
  boost: 2
---

# Continue in Obsidian

The first-day tutorial stays in the terminal. This page opens the same demo
vault in Obsidian and confirms the desktop review surface. Every step here
needs the Obsidian app, so the automated tutorial replay skips the commands
and you follow them by hand.

## 1. Open the demo copy in Obsidian

<!-- tutorial-test: skip opening Obsidian needs the desktop app -->
```bash
ls "$BOB_DIR/mac_inbox.md" "$BOB_DIR/garden.md"
```

Open the `$BOB_DIR` copy from the tutorial as an Obsidian vault with
**Open folder as vault**. Keep using the temporary copy, not your real vault.

## 2. Install and enable the review plugins

<!-- tutorial-test: skip plugin install needs the Obsidian app and network -->
```bash
ls "$BOB_DIR/.obsidian/plugins/obsidian-tasks-plugin/data.json"
```

In Obsidian, install and enable **Tasks**, **Bob Ledger Tools**, and
**Bob Navigation Hotkeys** from Community plugins. Confirm the Tasks
settings the demo ships with:

- Task format is **Dataview**.
- Global filter is `#task`.

The status registry in the demo maps `[ ]` to Ready, `[*]` to Next, `[/]`
to In Progress, `[?]` to Blocked, `[x]` to Done, and `[-]` to Cancelled.
Match these values before reviewing; the CLI refuses a non-Dataview format
for freshness and ready reports.

## 3. Deploy the custom plugins from source

<!-- tutorial-test: skip plugin sync needs a plugin source checkout and Obsidian -->
```bash
bob plugins sync --bob-dir "$BOB_DIR"
```

Clone the public
[bob-plugins](https://github.com/bobs-org/bob-plugins#readme) source, then
run sync explicitly against the demo vault as shown. Sync copies plugin
assets into the vault. It does not enable plugins in Obsidian and does not
create dashboard notes; enable each plugin and open notes by hand.

## 4. Read a rendered task and run one review gesture

A garden task renders as a card with its lane and date marks:

```bob-task
- [ ] #task Water the seedlings [created:: 2026-10-20]
```

With Bob Ledger Tools and Bob Navigation Hotkeys enabled, open the review
queue with `]s`, land on the inbox task, and press `Alt+F` to confirm it.
The stamp is the desktop continuation the CLI tutorial leaves for Obsidian.

## Next

- [Your first Bob day](first-bob-day.md) — the terminal path this continues
- [Continue with Mac Capture](mac-capture.md) — the optional menu-bar panel
- [How Bob fits together](../concepts/how-bob-fits-together.md) — vault, CLI, and plugins
