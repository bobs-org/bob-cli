---
keyword: Copy Chip
aliases:
  - chip
  - copy card fence
  - bob-copy
---

Authored click-to-copy text in an Obsidian note: an inline code span that starts
with `copy:` (or `copy[LABEL]:`) or a `bob-copy` fence. The `bob-chips`
plugin renders the span as a quiet pill and the fence as a titled card. One
click, `gy` (normal mode, the `copy-on-cursor-line` command), or `Mod+Y` then
`C` copies the payload byte-exact from source. `Mod+Shift+Y` makes, converts,
or edits a chip; Alt+Click on a Live Preview chip opens the same card. Payloads
are literal: nothing is interpolated, and rendering never writes the vault.

An unlabeled chip shows its own text in monospace. A labeled chip shows the
label in the text font. Text with a newline becomes a `bob-copy` fence; a
longer backtick run holds text that itself contains backticks. With the plugin
off, every chip still reads as inline code or a fence.

The cursor rule, shared by `gy` and the Copy card: a chip under the cursor wins;
otherwise the only chip on the line; otherwise the `bob-copy` fence the cursor is
inside; otherwise, on a line with no chip, the only chip in the cursor's task (its
own lines, never a nested subtask's). Two or more chips on the line, or in the task,
with the cursor on none is ambiguous and never guessed. `Mod+Shift+Y` uses the rule
without the task step.

Not a secret store, a protocol handler, a Copy card template
(`copy.templates`), or a [[command-chip]] (`run:` spans, which `gy` never
copies). See [Bob Chips](https://bobs-org.github.io/reference/plugins/bob-chips/).
