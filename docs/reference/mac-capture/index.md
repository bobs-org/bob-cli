---
title: "Mac Capture reference"
description: "Install, settings, permissions, panel, pickers, keyboard, Bob Refs, diagnostics, and protocol for Bob Mac Capture."
---

Bob Mac Capture is the optional native menu-bar panel for capture. It is a thin client of the CLI: it never parses capture grammar or writes the vault itself. It runs `bob`, renders the preview, and submits one `bob capture` call per draft that only hides once `bob` reports success. The reasoning is recorded under [Bob Mac Capture Is A Thin Client Of bob](../../develop/decisions.md#mac-capture-is-a-thin-client).

Build it from source with [Continue with Mac Capture](../../start/mac-capture.md). Platform install and signing are in [Install](install.md). The canonical token contract is [Capture syntax](../capture-syntax.md); install and platform requirements are also in [Requirements and compatibility](../compatibility.md).

## Install and settings

- [Install](install.md) for platform requirements, signed install, updates, rollback, and uninstall.
- [Settings](settings.md) for the Settings window in `SettingsView` order.
- [Permissions](permissions.md) for Accessibility, notifications, launch at login, and what is stored.

## Capture

- [Capture panel](capture-panel.md) for the panel, editor, completion, drafts, live preview, submit, and stash.
- [Pickers](pickers.md) for the shared picker lifecycle, with [task pickers](task-pickers.md) and [relationship pickers](relationship-pickers.md).
- [Capture previews](capture-previews.md) for preview cards, session controls, completions, and successor offers.

## Keyboard and agenda

- [Keyboard](keyboard.md) for every binding: editor, completion, prompts, stash, shelf, and today's Pomodoro.
- [Idle agenda](idle-agenda.md) for today's Pomodoro agenda in an empty draft.

## Draft aids

- [Saved templates](saved-templates.md) for the Control-T template shelf.
- [Wikilink completion](wikilink-completion.md) for Obsidian note, heading, block, and alias rows.

## Bob Refs

- [Bob Refs](refs.md) for the library panel, ranking, cards, opening, and Tabs.
- [Bob Refs inspector](refs-inspector.md) for the hydrated inspector column.

## Protocol and diagnostics

- [Diagnostics](diagnostics.md) for the menu-bar icon, signposts, timeouts, Copy Diagnostic, and `refs-rank`.
- [Protocol](protocol.md) for subprocess commands, JSON boundaries, capability requirements, and older-Bob compatibility.
- [Development](../../develop/mac-capture/index.md) for architecture, build, and CI.

Recovery steps live in the [Mac README troubleshooting](https://github.com/bobs-org/bob-mac-capture#troubleshooting) until the Fix problems guides land.
