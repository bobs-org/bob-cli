---
create_time: 2026-06-04
status: research
topic: Improving day-to-day Obsidian usage (Vim ergonomics, Tasks, Bases, capture automation)
---
# Research: Improving Obsidian Usage

## Question

Broad ask: "Help me improve my use of Obsidian." Rather than re-survey ground already
covered (Dataview↔Bases parity, relative line numbers, Logseq tradeoffs, bulk task
properties), this file focuses on **actionable workflow upgrades** grounded in the Bob
vault's *actual* configuration, ordered by expected payoff.

## Current Vault State (observed)

From `/home/bryan/bob/.obsidian/`:

- `app.json` → `vimMode: true`, `showLineNumber: true`, `alwaysUpdateLinks: true`,
  `promptDelete: false`.
- **Community plugins:** `dataview`, `obsidian-tasks-plugin`, `templater-obsidian`,
  `quickadd`, `task-status-cycler`, `mrj-jump-to-link`, `bob-navigation-hotkeys`,
  `bob-ledger-tools`, `block-id-prompt`, `obsidian-relative-line-numbers`,
  `note-refactor-obsidian`.
- **Core plugins of note:** `bases: true` (enabled), `daily-notes`, `templates`,
  `properties`, `note-composer`, `bookmarks`, `workspaces`, `sync`, `publish`.
- **CSS snippets:** `task-statuses`, `dataview-properties`.
- **Notably absent:** `obsidian-vimrc-support`, `obsidian-periodic-notes`. The
  `webviewer`, `slash-command`, and `zk-prefixer` core plugins are off.

Two facts jump out and drive most of the recommendations below:

1. **Many bespoke CM6 plugins exist purely for tiny Vim ergonomics.** From `sdd/prompts/`
   and `sdd/tales/` (202606): `obsidian_vim_o_list_continuation`, `obsidian_daily_vim_minus`,
   `obsidian_backslash_daily_fallback`, `obsidian_file_link_caret_jump`,
   `obsidian_alias_block_completion_cursor`, `obsidian_transclusion_toggle_keymap`,
   `child_note_keymap_dash`. That's a lot of hand-rolled JavaScript and per-feature
   maintenance for keybindings.
2. **`bases` is enabled but the whole toolchain (CLI, skills, research) is Dataview-first.**
   The native engine is sitting unused.

---

## Finding 1 — Consolidate simple Vim keymaps into `obsidian-vimrc-support` (highest payoff)

The vault hand-rolls CodeMirror 6 plugins for several keybindings. CM6 is the *right* tool
when a keymap needs **editor logic** (inspect the current line, compute indentation, move
the caret to a specific offset). It is **overkill** when the keymap is just "press a key →
run an existing Obsidian command." Those belong in a single declarative
`~/bob/.obsidian.vimrc` loaded by [`esm7/obsidian-vimrc-support`][vimrc], with zero JS and
zero rebuild-per-feature.

### The dividing line for the Bob vault's custom keymaps

| Existing custom feature | Needs CM6 logic? | Could move to `.obsidian.vimrc`? |
| --- | --- | --- |
| `vim_o_list_continuation` (smart `- [ ] `/`- ` insertion + indent match) | **Yes** — inspects line, computes indent | No |
| `file_link_caret_jump` (place caret at a computed offset) | **Yes** | No |
| `alias_block_completion_cursor` (cursor placement after completion) | **Yes** | No |
| `daily_vim_minus` (key → open/create daily note) | No — it's an obcommand | **Yes** |
| `backslash_daily_fallback` (key → daily note fallback) | Mostly no | **Likely** |
| `transclusion_toggle_keymap` (key → toggle a command) | No — it's a command toggle | **Yes** |
| `child_note_keymap_dash` (key → run a Bob command) | No | **Yes** |

Moving the bottom four into vimrc would shrink the custom-plugin surface area and make those
bindings editable without a build step.

### Concrete vimrc syntax worth adopting

- **Map a key to any Obsidian command** (the workhorse). Because CodeMirror's vim only
  passes the first argument to `:map`, you alias first with `exmap`, then map:
  ```vim
  exmap daily   obcommand daily-notes
  exmap tnext   obcommand bob-navigation-hotkeys:next-child   " example
  nmap <Space>d :daily<CR>
  ```
  Obsidian ≥1.7.2 requires the trailing `<CR>` on Ex-command maps.
- **A real leader key**, so all Bob bindings live under one prefix:
  ```vim
  let mapleader = " "
  nmap <leader>t :obcommand bob-ledger-tools:toggle<CR>
  ```
- **`vim-surround` equivalent** — wrap selection/word with wiki-link or bold markers
  (relevant given the vault's heavy `[[link]]` and bold-ledger usage):
  ```vim
  exmap surround_wiki surround [[ ]]
  exmap surround_bold surround ** **
  vmap <leader>l :surround_wiki<CR>
  vmap <leader>b :surround_bold<CR>
  ```
- **`pasteinto`** — turn a selected word into a link with the clipboard URL as the target:
  `[selected](pasted-url)`:
  ```vim
  vmap <A-p> :pasteinto<CR>
  ```
- **System-clipboard yank** (often wanted with Vim mode on Linux):
  ```vim
  set clipboard=unnamed
  ```
- **`jsfile` / `jscommand`** — the escape hatch. If a future keymap *does* need editor
  logic, you can run a JS snippet (with `editor`, `view`, `selection` in scope) from the
  vimrc instead of authoring a whole plugin. Disabled by default; enable in plugin settings.
  This is a lighter-weight middle ground than a new `bob-*` CM6 plugin for one-offs.

**Limitations to respect:** multi-arg commands *must* go through `exmap` (CodeMirror bug);
failed commands produce no visible error (test interactively); the line-number gutter is
Obsidian's, not codemirror-vim's, so `set relativenumber` in vimrc does **not** drive
relative numbers — that's exactly why the vault uses the `obsidian-relative-line-numbers`
plugin instead (see `obsidian_relative_line_numbers.md`). Keep them as separate tools.

**Recommendation:** Install `obsidian-vimrc-support`, create `~/bob/.obsidian.vimrc`, and
migrate the command-style keymaps (daily-note, transclusion toggle, child-note) out of
their bespoke plugins. Keep CM6 only for the logic-heavy three.

---

## Finding 2 — Put the already-enabled Bases engine to work

`bases: true` is set but unused; the entire workflow is Dataview-first. Native
[Bases][bases-migrate] renders near-instantly even on 50k-note vaults, ships in core (no
community-plugin risk), supports multiple views (Table, Cards) per `.base` file, and is
edited visually with the raw YAML one click away.

`.base` files are YAML, e.g.:
```yaml
filters:
  or:
    - file.inFolder("Daily Notes")
formulas:
  # calculated columns
views:
  - type: table
    # selected properties become columns; sort with direction
```

**What still keeps Dataview around (today):** no `GROUP BY` yet, only Table/Cards views
(List/Kanban in development), and a less expressive filter language than DQL. So Bases does
**not** wholesale replace the vault's Dataview use — but it's a strong fit for the simple,
performance-sensitive listing dashboards (e.g. "all daily notes," "open tasks by folder").
This complements the existing `dataview_parity_consolidated.md`: that file maps parity for
the **CLI**; this is about adopting Bases *inside the app* for the views where it already
wins.

**Migration aids:** the vault already runs the `dataview-properties` CSS snippet and (per
prior research) leans toward YAML properties — the prerequisite for Bases. A free "Dataview
to Bases converter" web tool handles straightforward query translation; rebuild the complex
ones visually.

**Recommendation:** Pick 1–2 read-only Dataview dashboards that don't use `GROUP BY` and
re-author them as a `.base`. Measure the render-speed difference; expand only if it pays off.

---

## Finding 3 — Exploit underused Tasks-plugin power features

The vault runs `obsidian-tasks-plugin` + `task-status-cycler` + a `task-statuses` CSS
snippet, so custom statuses are already in play. Underused [power features][tasks-power]:

- **Status *types* in queries**, not just symbols — survives symbol changes:
  ```tasks
  not done
  status.type is IN_PROGRESS
  ```
- **Filter by file property** via function (ties Tasks to the vault's YAML front matter):
  ```tasks
  filter by function task.file.property('project') === 'Project 1'
  ```
- **Group + limit** for compact daily dashboards:
  ```tasks
  not done
  group by path
  limit groups to 1 tasks
  ```
- **On-completion automation** — `🏁 delete` removes a task on completion (good for ephemeral
  daily checklist items so they don't accrete).
- **Task dependencies & postpone** — newer releases add blocking/blocked-by relationships and
  one-click date postponement; worth a look for the ledger workflow. (Confirm exact syntax in
  the official Tasks user guide before adopting — the survey sources were thin here.)

**Recommendation:** Add one "today" dashboard note using `status.type` + `group by` +
`limit groups`, and adopt `🏁 delete` for throwaway daily items.

---

## Finding 4 — Capture automation: QuickAdd macros + Templater logic

Both `quickadd` and `templater-obsidian` are installed; the division of labor is the win.
[QuickAdd][quickadd] = the launcher (Captures append to predefined files/sections; Macros
chain commands + user scripts + format syntax like `{{DATE}}`, `{{VALUE}}`, `{{FIELD:status}}`).
Templater = the in-note logic (JS, conditional sections, computed due dates, front-matter
from prompts). Pattern: **QuickAdd launches → Templater decides.**

This is the natural automation home for repetitive Bob captures (ledger entries, child
notes, daily scaffolding) — and a QuickAdd Macro can be bound to a key via Finding 1's
`obcommand`, closing the loop: one keystroke → capture → templated insert.

**Recommendation:** Convert the most frequent manual capture into a QuickAdd Capture/Macro,
and trigger it from the vimrc leader key.

---

## Suggested next steps (in priority order)

1. Install `obsidian-vimrc-support`; create `~/bob/.obsidian.vimrc`; migrate command-style
   keymaps off their bespoke CM6 plugins (keep the logic-heavy three).
2. Re-author one non-`GROUP BY` Dataview dashboard as a `.base`; compare render speed.
3. Add a Tasks "today" dashboard using `status.type` + `group by` + `limit groups`.
4. Convert one frequent capture into a QuickAdd Macro bound to a vimrc leader key.

## Related research

- `obsidian_relative_line_numbers.md` — why the gutter plugin (not vimrc) handles relative
  numbers.
- `dataview_parity_consolidated.md` / `dataview_cli_commandline.md` — CLI-side Dataview/Bases
  parity (distinct from in-app Bases adoption above).
- `bulk_obsidian_task_properties.md` — bulk task property editing.
- `obsidian_to_logseq_tradeoffs.md` — platform-level comparison.

## Sources

- [esm7/obsidian-vimrc-support — README][vimrc]
- [Moving to Obsidian Bases from Dataview — Practical PKM][bases-migrate]
- [Power Features of Tasks in Obsidian — Obsidian Rocks][tasks-power]
- [QuickAdd documentation][quickadd]
- [Dataview vs Datacore vs Obsidian Bases — Obsidian Rocks](https://obsidian.rocks/dataview-vs-datacore-vs-obsidian-bases/)
- [Obsidian Tasks Plugin Guide 2026 — Taskforge](https://taskforge.md/blog/obsidian-tasks-guide/)

[vimrc]: https://github.com/esm7/obsidian-vimrc-support/blob/master/README.md
[bases-migrate]: https://practicalpkm.com/moving-to-obsidian-bases-from-dataview/
[tasks-power]: https://obsidian.rocks/power-features-of-tasks-in-obsidian/
[quickadd]: https://quickadd.obsidian.guide/docs/
