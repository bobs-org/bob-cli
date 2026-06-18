---
create_time: 2026-06-18
status: research
topic: Options for vim-surround-style keymaps in Obsidian Vim mode
---
# Research: Vim Surround Keymaps in Obsidian

## Question

Bob's Obsidian vault uses Vim mode. Can Obsidian provide the same keymaps as
Tim Pope's `vim-surround` plugin, especially `ys{motion}{replacement}`,
`yss{replacement}`, `ds{target}`, `cs{target}{replacement}`, and visual
`S{replacement}`? If so, what are the implementation options?

## Short Answer

Yes, but not by installing the real Vim plugin. Obsidian's Vim mode is
CodeMirror Vim emulation, not embedded Vim/Neovim, so Vimscript plugins such as
`tpope/vim-surround` cannot be loaded directly.

There is a partial solution already available: `obsidian-vimrc-support` provides
a `:surround` Ex command and examples for mappings like `s"` or `s(` that wrap
the visual selection or the word under the cursor. That is useful and low-risk,
but it is not the same key grammar as `vim-surround`: it does not provide full
`ys`, `ds`, `cs`, text-object, tag, repeat, or custom-surround semantics.

For literal `vim-surround` keymaps inside Obsidian, the right implementation is
a small owned Obsidian plugin that integrates with `window.CodeMirrorAdapter.Vim`
the same way our existing Bob plugins already do. It should implement a focused
subset first: quotes, backticks, parentheses, brackets, braces, angle brackets,
wiki links, `ys{motion}{char}`, `yss{char}`, `ds{char}`, `cs{old}{new}`, and
visual `S{char}`.

## Current Vault State

Observed locally in `/home/bryan/bob`:

- `.obsidian/app.json` has `"vimMode": true`.
- `obsidian-vimrc-support` is installed and enabled.
- `.obsidian/plugins/obsidian-vimrc-support/data.json` sets
  `"vimrcFileName": "obsidian_vimrc.md"` and
  `"supportJsCommands": false`, which is a good default to keep.
- `obsidian_vimrc.md` currently uses Vimrc Support for command-oriented mappings
  such as `-`, `[[`, `]]`, `!`, `[<Space>`, `]<Space>`, `<C-j>`, and `<C-k>`.
- No surround mappings exist in the vimrc today.
- Local custom plugins already use `window.CodeMirrorAdapter.Vim`:
  `bob-ledger-tools` and `task-status-cycler` register Vim actions with
  `vim.defineAction(...)` and `vim.mapCommand(...)`.
- Local plugins also already use CM6 editor extensions via
  `registerEditorExtension(...)`, so an owned editor/Vim plugin fits the vault's
  current implementation style.

## What `vim-surround` Actually Provides

`vim-surround` is not just "wrap selected text." It adds a small Vim language
for adding, changing, and deleting paired surroundings:

| Operation | Example | Meaning |
| --- | --- | --- |
| Delete | `ds"` | Delete surrounding double quotes. |
| Change | `cs"'` | Change surrounding double quotes to single quotes. |
| Add by motion | `ysiw]` | Surround inner word with square brackets. |
| Add line | `yss)` | Surround the current line with parentheses. |
| Visual | `S<p>` | Surround the visual selection with a tag. |

The high-value part is the grammar: an operator-like prefix, a Vim motion or
target, then a replacement. That grammar is why this is harder than a few
Obsidian hotkeys.

## Key Findings

### 1. Obsidian cannot load `vim-surround` directly

`vim-surround` is Vimscript. Obsidian's editor uses CodeMirror and a JavaScript
Vim-emulation layer. The CodeMirror Vim docs explicitly say the implementation
tries to emulate useful Vim features but is not a complete Vim implementation.

Practical implication: there is no `Plug 'tpope/vim-surround'` equivalent inside
Obsidian. Exact support must be reimplemented in JavaScript against CodeMirror
Vim, or the note must be edited in real Vim/Neovim outside Obsidian.

### 2. Vimrc Support already has a useful but limited `surround` command

`obsidian-vimrc-support` documents a `:surround [prefixText] [postfixText]`
command. Its README gives mappings such as:

```vim
exmap surround_double_quotes surround " "
exmap surround_brackets surround ( )
map s" :surround_double_quotes<CR>
map sb :surround_brackets<CR>
map s( :surround_brackets<CR>
```

Usage is visual selection plus `s"` / `s(`, or cursor on a word plus `s"` /
`s(`. This is probably the fastest way to get a useful "surround selected text
or current word" workflow in the current vault.

Limitations:

- It does not give the same keymaps as `vim-surround`.
- It is add-only; a GitHub discussion asks specifically about `cs` and `ds`, and
  there is no documented built-in answer.
- It wraps the current selection or word, not arbitrary Vim motions in the full
  `ys{motion}{replacement}` form.
- The local installed plugin maps a prompt-style surround operator to `<A-y>s`,
  but that still prompts for the replacement rather than accepting the final
  replacement key exactly like `ysiw"`.

### 3. Selection-wrapping community plugins are not Vim-surround

Plugins such as "Wrap with shortcuts", "Code Editor Shortcuts", and "Shortcuts
extender" can wrap selections or expand selections to quotes/brackets. These are
useful for non-Vim workflows, but they are normal Obsidian command/hotkey
plugins. They do not understand Vim operator-pending mode, motions, `iw`, `yss`,
`cs`, or `ds`.

Practical implication: these plugins can solve "wrap selected text with a
shortcut"; they cannot preserve `vim-surround` muscle memory.

### 4. CodeMirror Vim exposes enough API for a proper custom implementation

`@replit/codemirror-vim` exposes the old CodeMirror Vim API through
`Vim`, including:

- `Vim.map(...)`
- `Vim.unmap(...)`
- `Vim.defineEx(...)`
- `Vim.defineOperator(...)`
- `Vim.mapCommand(...)`

The source also documents the operator callback shape: an operator receives the
computed selection ranges after a motion. That is the key primitive for
`ys{motion}` and visual `S`.

This matches local precedent: our Bob plugins already register custom Vim
actions through `window.CodeMirrorAdapter.Vim`.

### 5. Exact `ys`/`cs`/`ds` is more than one `defineOperator`

The tricky part is the replacement key position:

- CodeMirror Vim operators normally complete after `operator + motion`.
- `vim-surround`'s `ysiw"` completes after `operator + motion + replacement`.
- `cs"'` needs both an old target and a new replacement.
- `ds"` needs to search for and remove the surrounding pair around the cursor.

A prompt-based operator is straightforward: capture the motion range, open a
dialog, and wrap with the prompt result. That is essentially what Vimrc Support's
default prompt operator does.

Literal key compatibility needs an extra pending-replacement state. A custom
plugin would need to capture the motion range, temporarily intercept the next
normal-mode key before CodeMirror Vim consumes it, resolve that key to a pair,
then apply the edit. `ds` and `cs` also need a pair-finding engine for the
current cursor context.

This is feasible, but it is real editor work. It should be scoped deliberately.

## Implementation Options

### Option A: Add Vimrc Support `:surround` mappings now

Add mappings to `obsidian_vimrc.md`, but do not paste the README example as-is.
The README maps `[[` to wiki-link wrapping, which would collide with the
existing `[[` / `]]` link-navigation maps. It also repurposes `s` as a surround
prefix, which sacrifices Vim's standard `s` substitute command.

A safer add-only baseline is:

```vim
" --- vim-surround emulation (add-surround only; via obsidian-vimrc-support) ---
exmap surround_double_quotes surround " "
exmap surround_single_quotes surround ' '
exmap surround_backticks     surround ` `
exmap surround_parens        surround ( )
exmap surround_square        surround [ ]
exmap surround_curly         surround { }
exmap surround_wiki          surround [[ ]]

" Visual mode: select text, then S<char> wraps it.
vmap S" :surround_double_quotes<CR>
vmap S' :surround_single_quotes<CR>
vmap S` :surround_backticks<CR>
vmap S( :surround_parens<CR>
vmap S) :surround_parens<CR>
vmap S[ :surround_square<CR>
vmap S] :surround_square<CR>
vmap S{ :surround_curly<CR>
vmap S} :surround_curly<CR>
vmap Sw :surround_wiki<CR>

" Normal mode: ys<char> wraps the word under the cursor.
nmap ys" :surround_double_quotes<CR>
nmap ys' :surround_single_quotes<CR>
nmap ys` :surround_backticks<CR>
nmap ys( :surround_parens<CR>
nmap ys) :surround_parens<CR>
nmap ys[ :surround_square<CR>
nmap ys] :surround_square<CR>
nmap ys{ :surround_curly<CR>
nmap ys} :surround_curly<CR>
nmap ysw :surround_wiki<CR>
```

Pros:

- Fastest path.
- Uses an already-installed plugin.
- No JavaScript command support required.
- Good for "wrap current word" and "wrap current visual selection."

Cons:

- Not the same keymaps.
- No `ds`.
- No `cs`.
- No true `ys{motion}{replacement}`.
- Normal-mode `ys` adds a short timeout to plain `y` because Vim waits to see
  whether `s` follows.
- Vimrc Support's README says `surround` mappings should use `map`, not `nmap`,
  so this split `vmap` / `nmap` version should be tested in the installed plugin
  version before relying on it. If normal-mode `ys` misbehaves, use a leader
  mapping instead.

Best for: immediate partial relief.

### Option B: Use Vimrc Support JS commands or QuickAdd startup JavaScript

Enable JavaScript commands in Vimrc Support or use a startup script to call
`window.CodeMirrorAdapter.Vim` and register custom actions/operators.

Pros:

- Can prototype quickly.
- No full plugin scaffold required.
- Uses the same CodeMirror Vim API that a real plugin would use.

Cons:

- Vimrc Support warns that JS commands execute arbitrary code from the vault; the
  local vault currently keeps this disabled.
- Harder to test, version, and package than a normal plugin.
- Still needs the same surround parsing logic for `cs`/`ds`.

Best for: throwaway proof of concept only.

### Option C: Install a selection-wrapping community plugin

Use plugins such as:

- Wrap with shortcuts
- Code Editor Shortcuts
- Shortcuts extender

Pros:

- No custom code.
- Good for modifier hotkeys and selected-text wrapping.
- Can support arbitrary custom wrappers in some cases.

Cons:

- Does not preserve Vim-surround keymaps.
- Does not integrate with Vim motions/text objects.
- Does not provide `ds`/`cs` semantics.

Best for: users who only need "wrap current selection", not Vim muscle memory.

### Option D: Implement an owned `bob-vim-surround` Obsidian plugin

Create a small plugin dedicated to surround behavior. It would register Vim
mappings through `window.CodeMirrorAdapter.Vim`, using the same pattern already
present in `bob-ledger-tools` and `task-status-cycler`.

Suggested first milestone:

- `ys{motion}{char}` for charwise and linewise motions.
- `yss{char}` for current line.
- Visual `S{char}`.
- `ds{char}` for quotes, backticks, `()`, `[]`, `{}`, `<>`, and maybe `[[ ]]`.
- `cs{old}{new}` for the same targets.
- Pair resolver for `"`, `'`, `` ` ``, `(` / `)`, `[` / `]`, `{` / `}`, `<` /
  `>`, `b`, `B`, and markdown wiki links.

Defer until later:

- HTML/XML tag parsing (`t` / `<tag>`).
- Dot-repeat fidelity.
- Counts.
- Function replacements (`f` / `F`).
- Vimscript-style custom replacements.
- Deep compatibility with every edge case in `vim-surround`.

Implementation shape:

- Register once on layout ready after `window.CodeMirrorAdapter?.Vim` is
  available.
- Use `defineOperator` for the range-capturing part of `ys`.
- Add a short-lived pending-replacement state to consume the next key as the
  surround replacement.
- Add normal-mode sequence handling for `ds` and `cs` because native `d` and `c`
  are already CodeMirror Vim operators.
- Keep all behavior editor-scoped; do not create app-global bare-key handlers.
- Stand down outside Markdown editor Vim normal/visual mode.
- Add fixtures/unit tests for pair finding and replacement.
- Manually test in Live Preview with normal text, lists, links, code spans,
  inline Markdown markup, and multi-line selections.

Pros:

- Only option that can preserve `vim-surround` muscle memory inside Obsidian.
- Keeps JavaScript out of vault notes.
- Fits the existing Bob plugin pattern.
- Can be scoped to the exact Markdown wrappers we care about.

Cons:

- More work than Vimrc mappings.
- CodeMirror Vim internals are not a formal Obsidian API.
- Full `vim-surround` compatibility is larger than it looks.

Best for: the actual requested goal, if "same keymaps" means literal `ys`,
`cs`, `ds`, and visual `S`.

### Option E: Edit notes in Neovim for exact upstream behavior

Use real Neovim/Vim against the Markdown files and install `vim-surround` or a
modern equivalent such as `nvim-surround`.

Pros:

- Exact Vim plugin ecosystem.
- Mature implementation.
- No Obsidian editor internals.

Cons:

- Not inside Obsidian's editor.
- Obsidian-specific live-preview widgets, commands, and UI state are out of
  scope.

Best for: users who primarily edit notes in Neovim and use Obsidian for reading,
graph, search, sync, and plugins.

## Recommended Solution

Use a two-step path.

First, add the safer Vimrc Support `:surround` mappings from Option A for the
common wrappers. This gives an immediate improvement for visual selections and
current-word wrapping without enabling unsafe JS commands and without new plugin
work. Avoid the README's `[[` mapping because Bob already uses `[[` / `]]` for
link navigation, and avoid a plain `s` prefix unless losing Vim's substitute key
is acceptable.

Then, if literal `vim-surround` muscle memory is still the goal, implement a
small owned `bob-vim-surround` plugin. Do not try to load the real Vimscript
plugin, and do not rely on vault-stored JavaScript snippets for the long term.
Start with the Markdown-focused subset that matters in Obsidian: `ys`, `yss`,
visual `S`, `ds`, and `cs` for quotes, brackets, braces, backticks, angle
brackets, and wiki links. Treat tag support, dot-repeat, counts, and custom
replacements as later compatibility passes.

Net: Vimrc Support is the pragmatic short-term workaround; an owned CodeMirror
Vim plugin is the recommended long-term solution for the same keymaps.

## Sources

- [tpope/vim-surround README](https://github.com/tpope/vim-surround)
- [vim-surround help text](https://raw.githubusercontent.com/tpope/vim-surround/master/doc/surround.txt)
- [Vim.org surround.vim page](https://www.vim.org/scripts/script.php?script_id=1697)
- [Obsidian Forum: Vim surround](https://forum.obsidian.md/t/vim-surround/36661)
- [Obsidian Vimrc Support README](https://github.com/esm7/obsidian-vimrc-support)
- [Vimrc Support source: `defineSurround`](https://raw.githubusercontent.com/esm7/obsidian-vimrc-support/master/main.ts)
- [Vimrc Support discussion #189: `cs` / `ds`](https://github.com/esm7/obsidian-vimrc-support/discussions/189)
- [replit/codemirror-vim README](https://github.com/replit/codemirror-vim)
- [replit/codemirror-vim source](https://raw.githubusercontent.com/replit/codemirror-vim/master/src/vim.js)
- [CodeMirror 5 Vim bindings demo](https://codemirror.net/5/demo/vim.html)
- [Obsidian Forum: Vim mode quality-of-life improvements](https://forum.obsidian.md/t/vim-mode-quality-of-life-improvements/429)
- [Wrap with shortcuts plugin](https://github.com/manic/obsidian-wrap-with-shortcuts)
- [Code Editor Shortcuts plugin](https://github.com/timhor/obsidian-editor-shortcuts)
- [Shortcuts extender plugin](https://www.obsidianstats.com/plugins/shortcuts-extender)
- Local vault files inspected:
  `/home/bryan/bob/obsidian_vimrc.md`,
  `/home/bryan/bob/.obsidian/app.json`,
  `/home/bryan/bob/.obsidian/community-plugins.json`,
  `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`,
  `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`,
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`,
  `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
