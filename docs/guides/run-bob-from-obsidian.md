---
title: "Run bob from Obsidian"
description: "Open Bob Command Mode from Vim, find a bob command, and run it with write guards."
tags:
  - Obsidian
---

# Run bob from Obsidian

Bob Command Mode lets you find and run `bob` commands without leaving the
Obsidian desktop app. It uses the same versioned command catalog as the CLI,
saves open Markdown editors before writes, and keeps output and run history in
the command sheet.

## Before you start

- Use Obsidian on desktop with the **Bob Command Mode** plugin enabled.
- Install a current `bob` release and make sure the plugin can find it on
  `PATH`. If it cannot, set **Bob Command Mode → bob path** to the executable.
- For the `:` shortcut, enable Vim mode in Obsidian. The command is also
  available from the command palette as **Bob: Command mode** in any view.

## Open the command sheet

In Vim normal mode, press `:`. Type a few letters such as `sync`, `pdf`, or
`keep` to filter and rank matching commands. Use `↑` / `↓` or `Ctrl+P` /
`Ctrl+N` to move through the rows. The sheet also shows recent runs, commands
for the active note, and matching clipboard links when the query is empty.

Press `Enter` to run the selected row. Use `?` after a query to show help for
the selected bob command. Reports open in the output pager; other commands
finish in an outcome card. The status-bar chip shows long-running work.

### Preview and confirm

Use `Shift+Enter` to preview a row that supports preview. When the preview
finishes, press `Ctrl+Enter` to apply it. A writer without a preview shows a
confirmation card; press `Ctrl+Enter` or choose **Run** to confirm. `Escape`
backs out of a preview or confirmation. Before a write, the plugin saves open
Markdown editors and refuses to run if a save fails. It runs one writer at a
time.

Pomodoro session rows are available for starting, stopping, extending, and
shortening a session. Free-text `bob capture TEXT` is not part of command
mode; use the dedicated capture workflow for free-text capture.

### Run a sequence

Press `Ctrl+M` on a bob command row to mark it as the next step. Mark more rows
in order; the sequence tray shows the steps, and `Ctrl+M` on a marked row
removes it. Only bob commands can be marked. Press `Enter` to run the marked
steps, or `Ctrl+Enter` to run them without the review card. If a step needs
confirmation or is preview-first, `Enter` shows one card to review the whole
sequence first. Steps run one at a time and stop after a failure, unknown
result, or stopped run. A step that finishes with a needs-attention result
continues to the next step.

Press `Backspace` on an empty query to remove the last step. `Escape` clears
all marks and keeps the sheet open; press it again to close. Marks last only
while the sheet is open. A sequence appears as one row in RECENT, where it can
be run again. **Bob: Rerun last command** reruns the whole sequence when its
most recent run was a sequence step. While it runs, the status-bar chip shows
`N/M`, the current step title, and the sequence clock. When it finishes, one
summary card offers Output, Copy, Rerun all, and Resume from k if it halted.

### Run a command with arguments

Select a command and press `Tab` to enter argument mode. Type the arguments;
`Tab` accepts the highlighted completion, `↑` / `↓` (or `Ctrl+P` /
`Ctrl+N`) changes the selection, and `Enter` accepts it. `Shift+Enter`
previews when supported, while `Ctrl+Enter` runs now. `Escape` returns to the
command list.

You can also type a complete `bob` command line into the sheet. Arguments are
parsed as words with shell-style quoting, but are passed directly to bob; no
shell is started. Commands excluded from the catalog are refused. To run a
Vim ex command instead, prefix it with `:` in the sheet (for example,
`:%s/foo/bar/`) or use the Vim routing described below.

## Run a command from a note

A command chip turns a bob command written in a note into a one-gesture run.
It needs the **Bob Chips** plugin beside Bob Command Mode. Write an inline
code span that starts with `run:` and holds a complete `bob` command line,
optionally with a label in brackets:

```markdown
- [ ] Run `run: bob task reconcile`
- [ ] Run `run[Sync calendar and inboxes]: bob gcal sync -n 2 && bob gkeep pull && bob gmail sync`
```

With the cursor elsewhere, the first span shows a ▶ pill with its command,
and the second shows its label and a faint step count. Plain `` `bob …` ``
spans stay inline code; only the `run:` prefix makes a chip.
`Mod+Shift+Y` (**Make or edit chip**) writes one for you and checks each
step as you type.

Click a chip to run it the way `Ctrl+Enter` runs a sheet row. A writer runs
now and skips its preview, a report opens its output, and a command that
asks for confirmation still shows its confirm card. Shift+Click opens the
chip in Command Mode instead, so you can preview or review it first. While
it runs, the chip's glyph spins, and clicking it again opens the live
output instead of starting a second run.

In the editor, `⌘↵` (Meta+Enter) runs the chip under the cursor, the
cursor line's only chip, or, on a line without links, the only chip in the
cursor's task. The chip it would run gets an accent border, and a muted
`⌘↵ Run` keycap appears at the end of the cursor line. With two chips on
the line, `⌘↵` asks you to move onto one; with no chip, it keeps its usual
behavior.

To run several commands in order, join them with `&&` in one span, or
write a `bob-run` fence with one command per line:

````markdown
```bob-run Morning sync
bob gcal sync -n 2
bob gkeep pull
bob gmail sync
```
````

The steps run as one Command Mode sequence and stop at the first failure.

Note text is an attack surface, so a chip can only do what you could
already do from the sheet. Every step must start with the word `bob` and
name a command in bob's catalog. There is no shell: pipes, redirects, `;`,
and variables are refused, and a refused chip shows ⊘ with bob's reason.
Commands that ask for confirmation always ask. Chips run only on desktop.
In notes under `mail/`, `ref/`, `ref_history/`, `.bob/`, or `_conflicts/`,
on mobile, or without Command Mode, a chip shows a copy glyph and only
copies its text.

Running a chip never completes, freshens, or advances its task; complete
the task yourself with `Ctrl+Enter` in the editor. See
[Bob Chips](../reference/plugins/bob-chips.md#command-chips) for the full
rules.

## Read output and history

The pager streams standard output and errors, and saved output remains
available after the command finishes. Use `j` / `k` to scroll, `d` / `u` for
half-page movement, `g` / `G` for the top and bottom, `/` to search, and `w`
to toggle wrapping. Press `y` to copy output, `Y` to copy the command,
`r` to rerun, and `[` / `]` to step through run history. `:` returns to the
list, `Escape` goes back, and `q` closes the sheet. **Bob: Show last output**
opens the newest run; **Bob: Rerun last command** repeats that command's
original arguments and preview setting, or the whole sequence when the newest
run was a sequence step.

Run records and output files live under
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/command-mode/runs/`. This history is
local to the host, not stored in the vault. The plugin keeps up to 50 runs
for 14 days by default; change **Keep runs** or **Keep days** in plugin
settings to adjust retention.

## Keep Vim's ex line available

The vault Vim configuration binds `:` only in normal mode and keeps `g:` as
the direct route to Vim's ex line:

```vim
exmap bob_command obcommand bob-command-mode:open
nnoremap : :bob_command<CR>
nnoremap g: :
```

Visual-mode `:` remains Vim's native range command. In the command sheet,
`::` switches to Vim's ex line. Vim-shaped input such as `:%s/foo/bar/` or
`:42` is routed to Vim, and matching `exmap` commands appear as suggestions.
The plugin warns about recursive mappings that replay `:`; change those maps
to `nnoremap` so they do not reopen the command sheet recursively. If `:`
does nothing, check that the `exmap` and `nnoremap` lines are present, then
confirm the plugin itself is deployed: plugin folders are host-local and
gitignored, so each desktop host needs its own `bob plugins sync`. `bob plugins list`
should show `bob-command-mode` as `synced` and `enabled`; after enabling or
deploying it, reload Obsidian so the new enabled list takes effect. Meanwhile
`g:` still opens Vim's ex line.

## If bob is not found

Set the **bob path** setting to the full path of the executable, then reopen
the command sheet. In a terminal, `command -v bob` shows the path available
to a shell. Obsidian launched from the Dock may have a different `PATH`; an
explicit path avoids that difference. Update bob if its command catalog is
older than the plugin expects.

## Related

- [Bob Command Mode reference](https://bobs-org.github.io/reference/plugins/bob-command-mode/)
- [Bob Chips reference](../reference/plugins/bob-chips.md)
- [Obsidian plugins](../reference/plugins/index.md)
- [Continue in Obsidian](../start/obsidian.md)
