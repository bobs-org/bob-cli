---
create_time: 2026-06-18
status: research
topic: Replicating tpope vim-surround keymaps inside Obsidian's Vim mode
---
# Research: vim-surround Keymaps in Obsidian

## Question

Can we get the same keymaps that the `vim-surround` (tpope/vim-surround) Vim plugin
provides inside Obsidian? If so, what implementation options exist, and which should we
use for Bob's vault?

## Short Answer

Partly yes. Obsidian's Vim mode is the `@replit/codemirror-vim` extension, which does **not**
ship tpope's surround commands. The `obsidian-vimrc-support` plugin we already use adds a
custom `:surround` Ex command. Mapped to keys, it reproduces the **add-surround** half of
vim-surround (wrap the visual selection, or the word under the cursor in normal mode). It
does **not** natively provide `cs` (change a surrounding pair) or `ds` (delete a surrounding
pair), and the normal-mode form only targets the current word, not arbitrary motions/text
objects like `ysi(` or `ys$`.

To get full `ys` / `cs` / `ds` parity you must drop down to the plugin's `jscommand` /
`jsfile` JavaScript escape hatch and implement change/delete yourself — which requires
turning on `supportJsCommands`, currently deliberately disabled in this vault.

Recommendation: ship a no-JS surround mapping set built on `:surround` (visual-mode `S{char}`
plus a normal-mode `ys{char}` family), which covers the overwhelmingly common case, and treat
`cs`/`ds` as an optional later upgrade gated behind enabling JS commands.

## Background: what tpope/vim-surround actually provides

The real plugin has four headline operations:

- `ys{motion}{char}` — add a surround around a motion/text object, e.g. `ysiw"` wraps the
  inner word in quotes; `yss)` wraps the whole line.
- `S{char}` — in visual mode, surround the selection.
- `cs{old}{new}` — change an existing surrounding pair, e.g. `cs'"` turns `'x'` into `"x"`.
- `ds{char}` — delete a surrounding pair, e.g. `ds"` removes the quotes around the cursor.

Any "vim-surround in Obsidian" answer has to be measured against these four. The add
operations (`ys`, visual `S`) are achievable today; the change/delete operations (`cs`, `ds`)
are the hard part.

## Current Vault State

Verified locally from `/home/bryan/bob`:

- `.obsidian/app.json` has `"vimMode": true`.
- `obsidian-vimrc-support` is installed and enabled; its `data.json` sets
  `"vimrcFileName": "obsidian_vimrc.md"` and `"supportJsCommands": false`.
- `obsidian_vimrc.md` opens with the comment *"JavaScript vimrc commands intentionally remain
  disabled."* — disabling JS is a deliberate choice here, not an oversight.
- The vimrc already uses **bare-key `nmap`s**, including `nmap [[ :bob_prev_link<CR>` and
  `nmap ]] :bob_next_link<CR>`, plus `nmap -`, `nmap !`, `nmap [<Space>`, `nmap ]<Space>`,
  `nmap <C-j>`, `nmap <C-k>`. This matters: the surround example in the plugin README remaps
  `[[`, which would collide with our existing link-navigation maps (see Findings #3).
- No surround mappings exist in the vimrc today.
- See the sibling note [[obsidian_vim_keymaps_embedded_focus_consolidated]] for how this
  vault's Vim maps relate to native commands and the editor-focus boundary.

## Key Findings

### 1. The base Vim engine has no surround

Obsidian's Vim mode "is mostly the codemirror-vim extension for CodeMirror" (CM6, the
`@replit/codemirror-vim` package). That package implements operators, motions, and Ex
commands, but there is no built-in tpope-style surround (`cs`/`ds`/`ys`). So nothing happens
for these keys out of the box, and there is no setting to turn it on.

### 2. `obsidian-vimrc-support` adds a custom `:surround` command — add-only

The plugin defines an Ex command:

> `surround [prefixText] [postfixText]` — surrounds the selected text in visual mode, or the
> word under the cursor in normal mode.

The README's "vim-surround emulation" example, verbatim:

```
exmap surround_wiki surround [[ ]]
exmap surround_double_quotes surround " "
exmap surround_single_quotes surround ' '
exmap surround_backticks surround ` `
exmap surround_brackets surround ( )
exmap surround_square_brackets surround [ ]
exmap surround_curly_brackets surround { }

map [[ :surround_wiki<CR>
nunmap s
vunmap s
map s" :surround_double_quotes<CR>
```

Important documented detail: **"must use `map` and not `nmap`"** — the command keys off the
current mode (visual selection vs. word-under-cursor), so the mapping has to be live in both
normal and visual mode. There is also a companion `:pasteinto` command (paste clipboard into
the selection/word, handy for `[text](url)` links). Both `surround` and `pasteinto` were fixed
to work with the CM6 editor as of plugin v0.6.0.

What this gives us vs. real vim-surround:

- ✅ Visual `S{char}` equivalent (wrap a selection).
- ✅ A normal-mode "wrap the current word" equivalent (rough stand-in for `ysiw{char}`).
- ❌ No `cs` (change pair) — not supported.
- ❌ No `ds` (delete pair) — not supported.
- ❌ Normal-mode form is **word-only**; no motion/text-object targeting (`ysi(`, `ys$`, `yss`).

Community forum consensus matches this: the Vimrc `surround` command is a usable partial
solution but is explicitly "not as capable as real Vim and its vim-surround plugin," with no
native `cs`/`ds`/`ys`.

### 3. The README example collides with our existing maps

The canonical example does two things that are wrong for this vault:

- `map [[ :surround_wiki<CR>` would override our `nmap [[ :bob_prev_link<CR>` (and `map`
  applies in normal mode too). We must **not** remap `[[`.
- `nunmap s` / `vunmap s` repurpose `s` as a surround prefix, sacrificing Vim's `s`
  (substitute char) command. That is a real muscle-memory cost; `cl` is the usual fallback.

So we cannot paste the README example as-is. A vault-specific mapping scheme is required.

### 4. JavaScript commands are the only route to `cs`/`ds` parity

The plugin exposes `jscommand "<code>"` and `jsfile <path> "<code>"`, which run arbitrary JS
with `editor`, `view`, and `selection` arguments. That is powerful enough to implement true
change/delete-surround (find the nearest enclosing pair, rewrite or remove it). But:

- It requires enabling `supportJsCommands`, which is **currently `false` by deliberate vault
  policy**. Flipping it on means any code in the vimrc/jsfile runs with plugin privileges — a
  real security/trust tradeoff for a synced vault.
- We would be writing and maintaining the surround logic ourselves; there is no
  ready-made, well-maintained drop-in.

### 5. No mature dedicated "surround" community plugin exists

There is no established standalone Obsidian community plugin that implements tpope surround.
`nvim-surround` and the original `vim-surround` target Neovim/Vim, not Obsidian. For Obsidian,
the realistic universe of options is: the Vimrc `:surround` command, the Vimrc JS escape
hatch, or upstream changes to `codemirror-vim` (not available today).

## Implementation Options

### Option A — Vimrc `:surround` mappings (no JS) — RECOMMENDED baseline

Add a surround block to `obsidian_vimrc.md` using `:surround`, designed around our existing
bare-key maps. Covers add-surround in visual and normal (word) mode.

- Pros: no new plugin, no JS, keeps `supportJsCommands: false`, low risk, matches the most
  common real-world vim-surround use (wrap a word/selection).
- Cons: no `cs`/`ds`; normal mode is word-only (no motions/text objects).

### Option B — Vimrc JS commands for full `cs`/`ds`/`ys` parity

Use `jscommand`/`jsfile` to implement change- and delete-surround (and richer add-surround).

- Pros: can reach near-full vim-surround behavior, including `cs`/`ds`.
- Cons: requires enabling `supportJsCommands` (reverses a deliberate security choice);
  custom code to write, test against CM6, and maintain; no upstream support if it breaks.

### Option C — Wait for / contribute upstream surround in `codemirror-vim`

- Pros: would be the "right" long-term home, available to all Obsidian users.
- Cons: not available now; no indication it is planned; out of our control.

### Option D — Do nothing / use plain Vim edits

Use native edits for surrounds: `ciw` then retype with delimiters; `xp`-style fixes; visual
select + type. Free, but loses the ergonomics that motivated the request.

## Recommended Solution

Adopt **Option A now**, and keep **Option B as an explicit, opt-in upgrade** only if `cs`/`ds`
turn out to matter in daily use.

Rationale: add-surround (wrap a selection or word) is the dominant vim-surround use, and
Option A delivers it with zero new dependencies, zero JS, and no change to the vault's
deliberate `supportJsCommands: false` posture. `cs`/`ds` are nice but secondary, and the only
way to get them is the JS escape hatch, which carries a trust cost that should be a conscious,
separate decision rather than a default.

Concrete mapping scheme (avoids clobbering `[[` / `]]`, and does **not** sacrifice `s`):

- Visual mode: `S{char}` to surround the selection — this is exactly how real vim-surround
  behaves in visual mode, so the muscle memory transfers directly.
- Normal mode: a `ys{char}` family to wrap the word under the cursor — a close stand-in for
  `ysiw{char}`, and `ys` does not collide with any existing bare-key map.

Ready-to-paste block for `obsidian_vimrc.md` (append after the existing maps):

```
" --- vim-surround emulation (add-surround only; via obsidian-vimrc-support) ---
exmap surround_double_quotes surround " "
exmap surround_single_quotes surround ' '
exmap surround_backticks     surround ` `
exmap surround_parens        surround ( )
exmap surround_square        surround [ ]
exmap surround_curly         surround { }
exmap surround_wiki          surround [[ ]]

" Visual mode: select text, then S<char> wraps it (native vim-surround feel).
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

" Normal mode: ys<char> wraps the word under the cursor (stand-in for ysiw<char>).
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

Notes / caveats to verify when applying:

- The README says surround "must use `map` not `nmap`." We split into `vmap` (for visual `S`)
  and `nmap` (for normal `ys`) instead of a blanket `map` specifically to avoid touching
  operator-pending mode and to keep visual-`S` and normal-`ys` independent. Confirm both
  fire correctly in this plugin version after pasting; if normal-mode `ys` misbehaves, fall
  back to `map ys… ` per the README guidance and re-test that it does not disturb `[[`/`]]`.
- `ys` mappings introduce a short timeout on the `y` key (Vim waits to see if `s` follows).
  If that delay on yanks is annoying, switch the normal-mode prefix to a leader (e.g.
  `<leader>s"`), or drop normal-mode maps and rely on visual `S` only.
- This block does **not** remap `[[`, `]]`, `-`, `!`, or `s`, so existing Bob navigation maps
  and Vim's substitute command are preserved.

If/when `cs`/`ds` become important, open a follow-up that (1) enables `supportJsCommands`
with eyes open to the trust implications, and (2) adds `jscommand`/`jsfile`-based change- and
delete-surround. Keep that as a distinct decision.

## Sources

- [obsidian-vimrc-support README — surround & pasteinto](https://github.com/esm7/obsidian-vimrc-support/blob/master/README.md)
- [obsidian-vimrc-support repo](https://github.com/esm7/obsidian-vimrc-support)
- [Obsidian Forum — "Vim surround" thread](https://forum.obsidian.md/t/vim-surround/36661)
- [@replit/codemirror-vim (Obsidian's Vim engine)](https://github.com/replit/codemirror-vim)
- [tpope/vim-surround (reference for cs/ds/ys/S semantics)](https://github.com/tpope/vim-surround)
- [esm7/obsidian-vimrc-support — Obsidian integration commands (jscommand/jsfile)](https://deepwiki.com/esm7/obsidian-vimrc-support/4.2-obsidian-integration-commands)
- Local vault files inspected: `/home/bryan/bob/obsidian_vimrc.md`,
  `/home/bryan/bob/.obsidian/app.json`,
  `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`.
- Related prior research: [[obsidian_vim_keymaps_embedded_focus_consolidated]]
