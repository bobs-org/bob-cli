---
create_time: 2026-06-18
status: research
topic: Consolidated options for vim-surround-style keymaps in Obsidian Vim mode
---
# Research: Vim Surround Keymaps in Obsidian

## Question

Can Bob's Obsidian setup provide the same keymaps as Tim Pope's
`vim-surround` plugin, especially `ys{motion}{char}`, `yss{char}`,
`ds{char}`, `cs{old}{new}`, and visual `S{char}`? If so, what implementation
options exist, and which should we use?

## Short Answer

Yes, but not by installing `tpope/vim-surround` itself. Obsidian's Vim mode is
CodeMirror Vim emulation, not real Vim/Neovim, so Vimscript plugins cannot be
loaded directly.

The strongest current option is now the community plugin **More Vim**
(`more-vim`). The earlier agent notes missed it. As of 2026-06-18, Obsidian's
official community plugin manifest lists it, and its README/source advertise
and implement a focused surround subset: `ys`, `ds`, `cs`, and visual `S` for
quotes, brackets, backticks, angle brackets, and a few aliases.

More Vim is not full `vim-surround`: it is desktop-only, currently young
(`0.2.0`, published 2026-05-17), marked in the community manifest as not
manually reviewed by Obsidian staff, and it also changes other Vim behavior
such as `o`, `gd`, `gx`, `scrolloff`, multi-cursor movement, clipboard
registers, and `Mod-D`. That matters because this vault already has custom Vim
mappings and plugins.

Recommendation: install and test More Vim first in Bob's desktop vault, because
it is the only off-the-shelf option that directly targets the requested
`ys`/`ds`/`cs`/`S` keymaps without enabling vault-stored JavaScript. If it
conflicts with existing Bob mappings or its subset is not enough, fork or build
a small owned `bob-vim-surround` plugin using the same CodeMirror Vim
integration pattern already used by local Bob plugins.

## Current Vault State

Verified locally from `/home/bryan/bob`:

- `.obsidian/app.json` has `"vimMode": true`.
- `obsidian-vimrc-support` is installed and enabled.
- Vimrc Support reads `obsidian_vimrc.md`.
- `.obsidian/plugins/obsidian-vimrc-support/data.json` has
  `"supportJsCommands": false`.
- `obsidian_vimrc.md` explicitly says JavaScript vimrc commands are disabled on
  purpose.
- Existing Vimrc maps include `[[` / `]]` for Bob link navigation, `-`, `!`,
  `[<Space>`, `]<Space>`, `<C-j>`, and `<C-k>`.
- No surround mappings currently exist in `obsidian_vimrc.md`.
- More Vim is not installed or enabled in the local vault.
- Local Bob plugins already use `window.CodeMirrorAdapter.Vim` and
  `vim.mapCommand(...)`:
  - `bob-ledger-tools`
  - `task-status-cycler`
- `task-status-cycler` already maps normal-mode `o` and `O`, which may matter
  because More Vim also maps `o`.

## What Real `vim-surround` Provides

The real Vim plugin is a Vimscript plugin for real Vim/Neovim. Its value is not
just wrapping selected text. It adds a small Vim grammar:

| Operation | Example | Meaning |
| --- | --- | --- |
| Add by motion | `ysiw"` | Wrap the inner word in quotes. |
| Add line | `yss)` | Wrap the current line in parentheses. |
| Visual add | `S]` | Wrap the visual selection in square brackets. |
| Delete | `ds"` | Delete the surrounding double quotes. |
| Change | `cs"'` | Change surrounding double quotes to single quotes. |
| Tags | `ysiw<em>` | Wrap a target with an HTML/XML tag. |

The hard part is preserving this grammar inside CodeMirror Vim: `ys` needs an
operator-like prefix, a target, then a replacement key; `cs` needs both an old
target and a new replacement; `ds` needs a reliable pair finder around the
cursor.

## Key Findings

### 1. Obsidian cannot load `tpope/vim-surround` directly

`tpope/vim-surround` is Vimscript. Obsidian's built-in Vim mode is based on
CodeMirror Vim, and Vimrc Support explicitly frames itself as a configuration
and extension layer on top of Obsidian's built-in Vim mode, not real Vim.

Practical result: there is no `Plug 'tpope/vim-surround'` equivalent inside
Obsidian. Exact behavior must come from a JavaScript Obsidian/CodeMirror plugin
or from editing the Markdown files in real Vim/Neovim outside Obsidian.

### 2. More Vim is the current off-the-shelf candidate

The official Obsidian community plugin manifest currently includes:

- `id`: `more-vim`
- `name`: `More Vim`
- `repo`: `colinlienard/obsidian-more-vim`
- description: adds missing Vim features including surround

The More Vim README says its Vim surround feature adds `ys`, `ds`, `cs`, and
visual-mode `S` for adding, deleting, and changing surrounding characters. Its
source implements a finite-state key handler for:

- `yss{char}` for the current line, excluding leading whitespace.
- `ysiw{char}` and `ysaw{char}` through CodeMirror Vim text-object selection.
- `ysi{pair}{char}` and `ysa{pair}{char}` by finding the surrounding pair.
- `ds{char}` by finding and removing the target pair around the cursor.
- `cs{old}{new}` by finding the old pair and replacing delimiters.
- Visual `S{char}` by escaping visual mode and wrapping the selection.

Supported wrappers from source:

- `(` / `)`, `[` / `]`, `{` / `}`, `<` / `>`
- `"`, `'`, `` ` ``
- `*`, `_`, `~`
- aliases `b -> (`, `B -> {`, `r -> [`

Important limits:

- No arbitrary motions such as `ys$`, `ysap`, or `ys2w`.
- No tag form such as `S<p>` or `ysiw<em>`.
- No wiki-link wrapper such as `[[ ]]`.
- No dot-repeat, counts, or custom surround definitions documented.
- Pair finding is simple source-level scanning, not a full Markdown/HTML parser.
- It uses capture-phase DOM `keydown` interception and internal CodeMirror Vim
  access, so it should be tested against Obsidian and Bob plugin updates.

Operational caveats:

- Desktop-only.
- Manifest min app version is `1.12.7`.
- Latest GitHub release inspected: `0.2.0`, published 2026-05-17.
- The community manifest description says it has not been manually reviewed by
  Obsidian staff.
- It includes other behavior beyond surround: `o`, `gd`, `gx`, `scrolloff`,
  multi-cursor motions, system clipboard register, and `Mod-D`.
- Settings currently expose toggles for clipboard and `Mod-D`, plus scrolloff,
  but not a narrow "surround only" mode.

### 3. Vimrc Support `:surround` is useful but add-only

`obsidian-vimrc-support` already provides:

```vim
:surround [prefixText] [postfixText]
```

It wraps the visual selection or the word under the cursor. Its README shows a
vim-surround-inspired mapping block using `map s"` / `map s(` and similar
bindings.

This is useful, but it is not the same as `vim-surround`:

- It can wrap a visual selection.
- It can wrap the current word in normal mode.
- It does not provide `ds`.
- It does not provide `cs`.
- It does not provide true `ys{motion}{char}`.
- It requires careful mapping choices.

Do not paste the README example into this vault unchanged:

- The README maps `[[`, which collides with Bob's existing `[[` / `]]`
  navigation maps.
- The README unmaps `s`, which sacrifices Vim's normal substitute command.
- The README says to use `map`, not `nmap`, for its example, so any split
  `nmap` / `vmap` variant should be tested in the installed plugin version.

### 4. Vimrc Support JavaScript can implement more, but is not the right default

Vimrc Support has `jscommand` and `jsfile`, and those commands can call into the
Obsidian editor and view. That is powerful enough to prototype change/delete
surround behavior.

This vault deliberately keeps `"supportJsCommands": false`, and the Vimrc
Support README warns that enabling the feature lets vault-stored code execute
inside Obsidian. That is a real trust and sync risk. For maintained surround
behavior, an installed plugin is better than JavaScript snippets stored in the
vault.

### 5. Normal Obsidian wrapping plugins do not preserve Vim muscle memory

Plugins that wrap selected text, add Markdown formatting hotkeys, or create
generic shortcuts can be useful, but they do not speak Vim operator grammar.
They generally cannot support `ysiw"`, `ds"`, `cs"'`, visual `S`, or Vim text
objects in the way a Vim user expects.

### 6. An owned plugin remains the best fallback if More Vim is insufficient

The CodeMirror Vim package exposes a compatibility API for defining Vim
commands/operators and mapping keys. Local Bob plugins already use this style
through `window.CodeMirrorAdapter.Vim`, so a small owned plugin is feasible.

The owned implementation could either:

- fork More Vim and add settings/toggles plus Bob-specific wrappers; or
- implement only a narrow `bob-vim-surround` plugin from scratch.

The fork path is attractive because More Vim already has a compact surround
state machine. The owned-from-scratch path is cleaner if Bob wants only
surround behavior and no extra `o`, `gd`, `gx`, clipboard, scrolloff, or
multi-cursor behavior.

## Implementation Options

### Option A: Install and test More Vim

Install `more-vim` from Community Plugins, then test only on desktop Obsidian
`>= 1.12.7`.

Test cases:

- `ysiw"`, `ysaw"`, `yss)`, visual `S]`
- `ds"`, `ds)`, `cs"'`, `cs)]`
- Regular `yy`, `dd`, `cc`, `cw`, `ciw`, `diw`, `y$`
- Bob mappings: `[[`, `]]`, `-`, `!`, `[<Space>`, `]<Space>`, `<C-j>`,
  `<C-k>`, `<CR>`, `<BS>`, `o`, `O`
- Clipboard behavior with the current `set clipboard=unnamedplus`
- Live Preview Markdown cases: links, inline code, emphasis, lists, and
  multi-line selections

Pros:

- Closest current no-code answer to the requested keymaps.
- Does not require enabling Vimrc Support JavaScript commands.
- Implements `ys`, `ds`, `cs`, and visual `S` directly.

Cons:

- Young, not manually reviewed by Obsidian staff, and desktop-only.
- Brings unrelated behavior along with surround.
- May conflict with local Bob plugin mappings, especially `o`.
- Not full `vim-surround` parity.

Best for: first practical attempt.

### Option B: Add Vimrc Support `:surround` mappings

Use Vimrc Support's add-only `:surround` command for visual selections and the
current word.

Suggested safe shape if More Vim is not used:

```vim
" Add-surround only, via obsidian-vimrc-support.
exmap surround_double_quotes surround " "
exmap surround_single_quotes surround ' '
exmap surround_backticks     surround ` `
exmap surround_parens        surround ( )
exmap surround_square        surround [ ]
exmap surround_curly         surround { }
exmap surround_wiki          surround [[ ]]

" Visual mode: select text, then S<char>.
vmap S" :surround_double_quotes<CR>
vmap S' :surround_single_quotes<CR>
vmap S` :surround_backticks<CR>
vmap S( :surround_parens<CR>
vmap S[ :surround_square<CR>
vmap S{ :surround_curly<CR>
vmap Sw :surround_wiki<CR>

" Normal mode: ys<char> wraps only the word under the cursor.
nmap ys" :surround_double_quotes<CR>
nmap ys' :surround_single_quotes<CR>
nmap ys` :surround_backticks<CR>
nmap ys( :surround_parens<CR>
nmap ys[ :surround_square<CR>
nmap ys{ :surround_curly<CR>
nmap ysw :surround_wiki<CR>
```

Pros:

- Already installed dependency.
- No JS commands.
- Good for wrapping a selection or current word.

Cons:

- No `ds`.
- No `cs`.
- No true `ys{motion}{char}`.
- `ys` may add a short timeout after `y`.
- The README's `map` guidance means this exact split should be tested.

Best for: safe add-only fallback.

### Option C: Enable Vimrc Support `jscommand` / `jsfile`

Use vault-stored JavaScript to implement `ds`, `cs`, and richer `ys`.

Pros:

- Quick prototype path.
- No plugin scaffold.

Cons:

- Reverses the vault's current JS-disabled policy.
- Runs code stored in synced vault files inside Obsidian.
- Harder to test, review, and package than a normal plugin.

Best for: throwaway proof of concept only.

### Option D: Fork More Vim or build `bob-vim-surround`

Use CodeMirror Vim integration in an owned plugin. Start with the Markdown
subset Bob actually needs:

- `ysiw`, `ysaw`, `yss`, visual `S`
- `ds` and `cs`
- quotes, backticks, `()`, `[]`, `{}`, `<>`
- optional Markdown wrappers such as `[[ ]]`, `** **`, `_ _`, and inline code

Defer:

- HTML/XML tag parsing
- counts
- dot-repeat fidelity
- custom surround definitions
- deep parity with every tpope edge case

Pros:

- Best control over conflicts and scope.
- Can ship surround-only behavior.
- Fits existing Bob plugin implementation style.

Cons:

- More engineering work.
- Depends on CodeMirror Vim internals that Obsidian does not expose as a stable
  first-class API.

Best for: long-term Bob-specific behavior if More Vim is not good enough.

### Option E: Edit in real Vim/Neovim

Use external Vim/Neovim against the Markdown files and install `vim-surround`
or `nvim-surround`.

Pros:

- True Vim plugin ecosystem.
- Mature surround behavior.

Cons:

- Not inside Obsidian's editor.
- Obsidian Live Preview, commands, widgets, and plugin interactions are not
  part of that editing session.

Best for: exact upstream behavior outside Obsidian.

## Recommended Solution

Use a staged approach.

First, test **More Vim** in the desktop vault. It is the only current
off-the-shelf Obsidian option found that directly implements `ys`, `ds`, `cs`,
and visual `S` without enabling vault JavaScript. Keep the test deliberate
because the plugin is young, desktop-only, not manually reviewed by Obsidian
staff, and it includes unrelated Vim changes. Pay special attention to Bob's
existing `o`/`O`, clipboard, and Vimrc Support mappings.

If More Vim behaves well, use it. It gives the closest practical version of the
requested keymaps today.

If More Vim conflicts or the missing pieces matter, fork it or build a narrow
owned `bob-vim-surround` plugin. Prefer that over `jscommand`/`jsfile`, because
it keeps JavaScript out of synced vault notes and allows proper review,
versioning, and tests.

Keep the Vimrc Support `:surround` block only as the safe fallback for add-only
wrapping. It is useful, but it no longer deserves to be the primary
recommendation now that More Vim exists.

Net: try More Vim first; own the plugin only if the current community
implementation is too broad, too young, or not compatible enough with Bob's
vault.

## Sources

- [tpope/vim-surround README](https://github.com/tpope/vim-surround)
- [Obsidian community plugins manifest](https://raw.githubusercontent.com/obsidianmd/obsidian-releases/master/community-plugins.json)
- [More Vim repository](https://github.com/colinlienard/obsidian-more-vim)
- [More Vim surround source](https://github.com/colinlienard/obsidian-more-vim/blob/main/src/surround.ts)
- [More Vim manifest](https://github.com/colinlienard/obsidian-more-vim/blob/main/manifest.json)
- [More Vim settings source](https://github.com/colinlienard/obsidian-more-vim/blob/main/src/settings.ts)
- [More Vim latest release](https://github.com/colinlienard/obsidian-more-vim/releases/tag/0.2.0)
- [Obsidian Vimrc Support README](https://github.com/esm7/obsidian-vimrc-support)
- [Vimrc Support discussion: `cs` / `ds`](https://github.com/esm7/obsidian-vimrc-support/discussions/189)
- [CodeMirror Vim README](https://github.com/replit/codemirror-vim)
- [Obsidian Forum: Vim surround](https://forum.obsidian.md/t/vim-surround/36661)
- Local vault files inspected:
  - `/home/bryan/bob/obsidian_vimrc.md`
  - `/home/bryan/bob/.obsidian/app.json`
  - `/home/bryan/bob/.obsidian/community-plugins.json`
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`
  - `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
