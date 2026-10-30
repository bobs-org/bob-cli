---
title: "See which tasks mention a task"
description: "Read inert 💬 mention chips on a task's LINKED FROM row, and turn a mention into a relation when you want one."
tags:
  - CLI
  - Obsidian
---

# See which tasks mention a task

When another task's own text links here, Bob shows that on this task's
generated `🪢 **LINKED FROM:**` line as an inert `💬 mentions` entry. The
chip answers which task talked about this one, what it said, and why the
row is here. It is not a relation.

## Before you start

Install the CLI, open the vault in Obsidian, and enable **Bob Ledger
Tools** and **Bob Navigation Hotkeys**. Mentions ride
`relations.linked_from_mentions` in `~/.config/bob/config.yml` (default
`true`). Set it to `false` to hide chips and stop writers from minting
mention entries.

## 1. Find a mention on the target

Open a task that another task names in its title, a child note, or a
Cancel / Schedule / Work / Rotten Log entry, with a block link such as
`[[note#^id]]`. Confirm the LINKED FROM row shows a `💬 mentioned in`
or `💬 embedded in` pill, then the other task's chip. A quote line
appears for prose mentions that still have other letters or digits on
the line. Title and sole-link mentions have no quote, because the chip
already says it.

Without the plugin the stored line still reads as English:
`🪢 LINKED FROM: [[other#^s]] (💬 mentions)`.

## 2. Read the pill, quote, and card

The pill is italics on purpose: reported speech, not a label you chose.
Hover or focus it for the context card (`Why is this here?`). The card
lists up to three occurrences with a live excerpt and a location such
as `in its title` or `in its Cancel Log`. Click the chip to jump to the
first occurrence in the mentioning task; Mod-click opens a new pane.

A trailing `💬` on a labelled relation chip means that source also
mentions this task in its text.

## 3. Edit the source, not the chip

A mention chip has no ×. Removal is refused with
`💬 This comes from a link in ‹S›'s text · edit it there (↵ opens it)`.
Delete or retarget the link in S, then leave the line (nav writes
within seconds) or wait for `bob task reconcile`.

To make the pair a relation instead, add a LINKS entry from S to this
task (`~` / `~~`, or Task Card `l` / `⇧L`). The relation keeps its
label; the mention entry goes away. See
[Link related tasks](link-related-tasks.md).

A Depends-On link between the pair already names the other task, so
there is no mention entry.

## If it goes wrong

- **No chip, but the link is in S.** Mentions skip edge lines
  (Depends-On, BLOCKS, Sub-projects, LINKS, LINKED FROM), Pomodoro
  ledger rows, headings, note prose, code, comments, inline fields,
  blockquotes, unresolved or non-task targets, self-links, and a
  task whose whole description is one embed. Details are in
  [Task relations §19](../task-relations.md#19-text-mentions).
- **The chip is dashed.** The stored mention is stale. The next
  reconcile refreshes the line.
- **Mentions vanished after a config edit.**
  `relations.linked_from_mentions: false` removes only
  `(💬 mentions)` entries on writable tasks. Flipping it back rebuilds
  the same bytes.

## Related

- [Task relations](../task-relations.md#19-text-mentions)
- [Link related tasks](link-related-tasks.md)
- [Configuration](../reference/configuration.md)
