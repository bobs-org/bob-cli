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

## Read output and history

The pager streams standard output and errors, and saved output remains
available after the command finishes. Use `j` / `k` to scroll, `d` / `u` for
half-page movement, `g` / `G` for the top and bottom, `/` to search, and `w`
to toggle wrapping. Press `y` to copy output, `Y` to copy the command,
`r` to rerun, and `[` / `]` to step through run history. `:` returns to the
list, `Escape` goes back, and `q` closes the sheet. **Bob: Show last output**
opens the newest run; **Bob: Rerun last command** repeats its original
arguments and preview setting.

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
- [Obsidian plugins](../reference/plugins/index.md)
- [Continue in Obsidian](../start/obsidian.md)
