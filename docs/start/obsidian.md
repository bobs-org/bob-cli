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

## 2. Install Tasks and confirm the demo settings

<!-- tutorial-test: skip Tasks install needs the Obsidian app and network -->
```bash
ls "$BOB_DIR/.obsidian/plugins/obsidian-tasks-plugin/data.json"
```

In Obsidian, install and enable **Tasks** from Community plugins. Confirm
the Tasks settings the demo ships with:

- Task format is **Dataview**.
- Global filter is `#task`.

The status registry in the demo maps `[ ]` to Ready, `[*]` to Next, `[/]`
to In Progress, `[?]` to Blocked, `[x]` to Done, and `[-]` to Cancelled.
Match these values before reviewing; the CLI refuses a non-Dataview format
for freshness and ready reports.

## 3. Deploy the custom plugins from source

The custom Bob plugins do not live in Community plugins. Clone their public
source, then deploy that checkout into the demo vault:

<!-- tutorial-test: skip plugin clone needs network and a plugin source checkout -->
```bash
git clone https://github.com/bobs-org/bob-plugins.git ~/projects/github/bobs-org/bob-plugins
```

<!-- tutorial-test: skip plugin sync needs a plugin source checkout and Obsidian -->
```bash
bob plugins sync --repo ~/projects/github/bobs-org/bob-plugins --bob-dir "$BOB_DIR"
```

Pass `--repo` with the checkout path every time: a bare sync run from inside
a different checkout refuses rather than deploying the wrong source. Sync
copies plugin assets into the vault. It does not enable plugins in Obsidian
and does not create dashboard notes; enable **Bob Ledger Tools** and
**Bob Navigation Hotkeys** by hand and open notes by hand. See the mounted
[Obsidian plugins](../reference/plugins/index.md) reference for versions.

## 4. Read a rendered task and run one review gesture

A garden task renders as a card with its lane and date marks:

```bob-task
- [ ] #task Water the seedlings [created:: 2026-10-20]
```

With both custom plugins enabled, press `Ctrl+Alt+J` to land on the next
task due for freshness review, as documented in
[Bob Navigation Hotkeys](../reference/plugins/bob-navigation-hotkeys.md).
The walk and its freshness marks are rendered by
[Bob Ledger Tools](../reference/plugins/bob-ledger-tools.md). Then press
`Alt+F` to confirm the landed task.

When the landed task still lives in the inbox, the review first asks where
the task goes and moves it there before stamping; only a non-inbox task gets
a plain freshness stamp. A Pending task may also ask for an optional Work Log
summary. This routing confirmation is the desktop continuation the CLI
tutorial leaves for Obsidian.

## Next

- [Your first Bob day](first-bob-day.md) — the terminal path this continues
- [Continue with Mac Capture](mac-capture.md) — the optional menu-bar panel
- [How Bob fits together](../concepts/how-bob-fits-together.md) — vault, CLI, and plugins
