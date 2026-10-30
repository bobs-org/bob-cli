---
keyword: Command Mode
aliases:
  - "Bob Command Mode"
  - "command sheet"
---

The `bob-command-mode` Obsidian desktop plugin's bottom command sheet: a fuzzy finder
for running `bob` commands without leaving the vault. With Obsidian's Vim mode on, `:`
in normal mode opens it, because the vault vimrc (`obsidian_vimrc.md`) maps
`nnoremap : :bob_command<CR>` onto `exmap bob_command obcommand bob-command-mode:open`;
**Bob: Command mode** in the command palette opens it in any view. Its rows come from
bob's hidden, versioned `bob __catalog` handshake, so the binary, never the plugin,
decides which commands, arguments, and write policies exist, and excluded commands are
refused. An empty query shows RECENT runs, FOR THIS NOTE rows, FROM CLIPBOARD URLs, and
one group per catalog section. `Enter` runs a row, `Tab` enters argument mode,
`Shift+Enter` previews, `Ctrl+Enter` runs now or applies a preview, and a trailing `?`
shows help; reports stream into an output pager. Each run spawns `bob` directly, with no
shell, and its record and output stay host-local under
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/command-mode/runs/`, never in the vault.
Writers run one at a time and only after every open Markdown editor saves; a writer
without a preview asks first on a confirm card, while reads run in parallel. Catalog
rows that need no arguments, preview, or confirmation also appear in the command palette
as `Bob: <title>`, and sibling plugins run commands through the same engine
(`api.commandMode`). In Bob, "command mode" never means Vim's normal mode or its `:`
command line: `g:` in normal mode, or `::` in the sheet, still opens Vim's ex line, and
Vim-shaped input such as `:%s/a/b/` or `:42` is routed there. Not a [[capture]]:
free-text `bob capture TEXT` is refused here, and a capture draft's leading `:` opens
the task-link picker instead. Not the SASE TUI's command-mode panel. See
`docs/guides/run-bob-from-obsidian.md`.
