---
create_time: 2026-06-12 13:05:37
status: done
prompt: sdd/prompts/202606/obsidian_keymaps_prefer_open_tab.md
---
# Plan: Prefer Already-Open Tabs For All Obsidian File-Jump Keymaps

## Goal

Make every custom Obsidian keymap that jumps to a different file prefer an already-open instance of the target file:

- If the target file is already shown in the current tab, stay there (no reopen).
- If the target file is open in another tab/leaf, activate that tab instead of opening the file again in the current
  tab.
- Otherwise, keep today's behavior (open in the current tab, creating the note when the flow supports creation).

The `\\` Pomodoro-jump keymap (bob-ledger-tools) and the `\|` dash-tasks keymap (bob-navigation-hotkeys) already
implement this pattern; this change brings the remaining file-jump keymaps up to parity.

## Context Reviewed

- This is an ephemeral `bob-cli_<N>` workspace; the actual implementation lives in the Bob vault at `/home/bryan/bob`,
  not in bob-cli Rust code. No new CLI subcommands are involved, so `memory/long/cli_rules.md` does not apply.
- Vault rules in `/home/bryan/bob/AGENTS.md`: inspect `git -C /home/bryan/bob status` before editing, never touch
  unrelated dirty files (several exist right now: `dash.md`, `bob.md`, templates, daily notes, etc.), and commit
  task-related vault changes with `/sase_git_commit` before terminating.
- Active vimrc is `/home/bryan/bob/obsidian_vimrc.md` (per `obsidian-vimrc-support/data.json`).
- Precedent commit `d5338b2` ("add dash tasks vim shortcut") edited `main.js` + `obsidian_vimrc.md` directly with no
  manifest version bump; this change follows the same convention.

### Complete inventory of custom file-jump keymaps

| Keymap                | Defined in                                                                                                               | Flow today                                                                                                | Tab reuse? |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- | ---------- |
| `\\` (vim)            | bob-ledger-tools `vim.mapCommand`                                                                                        | local Pomodoro jump; daily fallback via `openTodayDailyNote`                                              | ✅ already |
| `\|` (vim)            | vimrc → `bob-navigation-hotkeys:open-dash-tasks`                                                                         | `openDashTasks` (active-view check + `findMarkdownLeafByPath` + `activateWorkspaceLeaf`)                  | ✅ already |
| `-` (vim)             | vimrc → core `daily-notes` command                                                                                       | core command always opens in current tab                                                                  | ❌         |
| `[[` / `]]` (vim)     | vimrc → `open-prev-link` / `open-next-link`                                                                              | `openLabeledBodyLink` → `openOrCreateLinkTarget` → `openResolvedLink` → `openLinkText(..., false)`        | ❌         |
| `<CR>` / `<BS>` (vim) | task-status-cycler `vim.mapCommand`, delegates to nav plugin `handleVimEnterLinkAction` / `handleVimBackspaceLinkAction` | `openOrCreateLinkCandidate` → `openResolvedLink` (resolved) or `createNoteFromLinkCandidate` (unresolved) | ❌         |
| `Ctrl+-`              | hotkeys.json → `open-parent-note`                                                                                        | `openFrontmatterLink("parent")` → `openResolvedLink`                                                      | ❌         |
| `Ctrl+.`              | hotkeys.json → `open-template-note`                                                                                      | `openFrontmatterLink("template")` → `openResolvedLink`                                                    | ❌         |
| `Ctrl+,`              | hotkeys.json → `open-alt-file-note`                                                                                      | `openFrontmatterLink("alt_file"/"type")` → `openResolvedLink`                                             | ❌         |
| `Ctrl+=`              | hotkeys.json → `open-child-note`                                                                                         | `openChildNote` → `workspace.getLeaf(false).openFile` (single child, or via `ChildNotePickerModal`)       | ❌         |
| `Ctrl+\`              | hotkeys.json → `open-alternate-file`                                                                                     | `openAlternateFile` → `workspace.getLeaf(false).openFile`                                                 | ❌         |

The `LinkCandidatePickerModal` (multi-link lines) and `ChildNotePickerModal` both route through
`openOrCreateLinkCandidate` / `openChildNote`, so fixing those functions covers the pickers automatically.
`createNoteFromLinkCandidate` also has an existing-file branch that calls `getLeaf(false).openFile` directly; it is part
of the Enter/link flow and is in scope.

### Out of scope

- `create-project-note` / `create-project-note-from-task` (`Ctrl+Shift+N`, `Ctrl+Alt+Shift+N`): always create brand-new
  uniquely named notes, so "already open" cannot apply.
- `mrj-jump-to-link` (`Ctrl+L`): third-party community plugin; not a custom keymap we maintain.
- Built-in `app:go-back` / `app:go-forward` hotkeys.
- Non-file-jump keymaps (`!`, `[<Space>`, `]<Space>`, `<C-j>/<C-k>`, `\p`, `\P`, `\o`, `\O`, task togglers, yank
  commands).
- `task-status-cycler/main.js` needs no changes — its Enter/Backspace actions delegate to the navigation plugin.

## Design

All file-open flows in `bob-navigation-hotkeys` funnel through two choke points, and the plugin already contains the
needed building blocks (`findMarkdownLeafByPath`, `activateWorkspaceLeaf`, both added for `open-dash-tasks`). The `-`
daily keymap is best served by `bob-ledger-tools`, which already owns `openTodayDailyNote` with exactly the desired
reuse semantics.

### 1. Central leaf-reuse open helper (bob-navigation-hotkeys)

Add one method, e.g. `openMarkdownFileWithLeafReuse(file, notFoundNotice)`:

1. If the active Markdown view already shows `file.path`, return success without doing anything (current-tab
   preference).
2. Otherwise look up `findMarkdownLeafByPath(file.path)`; if a leaf is found and `activateWorkspaceLeaf(leaf)` succeeds,
   return success.
3. Otherwise fall back to the existing `this.app.workspace.getLeaf(false).openFile(file)`.

This mirrors the proven `openDashTasks` sequence so behavior stays consistent across keymaps.

### 2. Teach `openResolvedLink` to reuse open tabs

`openResolvedLink(linkTarget, sourcePath, notFoundMessage)` covers Enter/Backspace line links, `[[`/`]]`,
parent/template/alt-file frontmatter links, and the link-candidate picker. Update it to:

1. Resolve the target file as today (`resolveLinkTargetFile`); keep the not-found notice path unchanged.
2. If the resolved file is the active file, keep the current `openLinkText` call — it already jumps in-file (including
   `#heading` / `#^block` subpaths) without leaving the tab.
3. Otherwise, if `findMarkdownLeafByPath(resolvedFile.path)` finds an open leaf and activation succeeds:
   - When the link text carries a subpath (detected with the existing `stripLinkSubpath` logic: stripped lookup text
     differs from the full link text), follow up with `openLinkText(linkText, sourcePath, false)` so the now-active leaf
     scrolls to the heading/block instead of opening a duplicate.
   - When there is no subpath, stop after activation so the tab keeps its live cursor/scroll state.
4. If no open leaf exists or activation fails, keep the existing fallback (`openLinkText` when available, else
   `getLeaf(false).openFile`).

Pure-subpath links (`#heading` on the current note) already resolve to the source file and stay in-file — unchanged.

### 3. Convert the direct `openFile` call sites

- `openChildNote(file)`: replace the direct `getLeaf(false).openFile(file)` with the central helper. Keep
  `captureActiveFilePosition()` and the "Could not open child note" notice.
- `createNoteFromLinkCandidate` existing-file branch: replace its direct `openFile` with the central helper.
- `openAlternateFile()`: replace the direct `openFile` with the central helper. Keep the `captureActiveFilePosition()` /
  `restoreFilePosition(file.path, restorePosition)` bracketing in both paths — `filePositions` is live-tracked via the
  selection listener, so restoring against a reused tab is consistent with its live state and keeps the vim-style
  alternate-file cursor contract.

### 4. Daily-note keymap `-` (bob-ledger-tools + vimrc)

`bob-ledger-tools` already exposes `openTodayDailyNote(app)`: reuse an open daily leaf → run the core Daily Notes
command (preserves template-based creation) → resolve/create + open fallback. Reuse it rather than duplicating
daily-path logic in the navigation plugin:

1. Add a command to bob-ledger-tools, e.g. `id: "open-today-daily-note"`, name "Open today's daily note", whose callback
   awaits `openTodayDailyNote(this.app)` and shows the existing "Could not open daily note"-style notice when it returns
   null. No Pomodoro jump — this command only opens/activates the note.
2. Update `/home/bryan/bob/obsidian_vimrc.md`: change `exmap bob_daily obcommand daily-notes` →
   `exmap bob_daily obcommand bob-ledger-tools:open-today-daily-note`. The `nmap - :bob_daily<CR>` line stays as-is.

### 5. Testability

Export the new/changed navigation helper through the existing `module.exports.helpers` block (same pattern as prior
tasks) so focused Node checks with stubbed `app`/`workspace` objects can exercise the logic without Obsidian.

## Files Changed

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`
- `/home/bryan/bob/obsidian_vimrc.md`

No changes to: manifests, `hotkeys.json`, `task-status-cycler`, daily-notes config, templates, notes, bob-cli code, or
memory files.

## Verification

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
```

Focused Node checks (stubbed `app.workspace` with fake leaves, calling plugin prototype methods and exported helpers):

- Central helper: short-circuits when the active view already shows the file; activates a matching open leaf without
  calling `openFile`; falls back to `getLeaf(false).openFile` when no leaf matches or activation fails.
- `openResolvedLink`: activates an existing tab for a plain link without re-opening; calls `openLinkText` after
  activation only when the link has a `#heading`/`#^block` subpath; preserves the unresolved/not-found notice path.
- `openChildNote` / `openAlternateFile` / `createNoteFromLinkCandidate`: reuse an open leaf, with unchanged fallback and
  notices; alternate-file position restore still runs.
- bob-ledger-tools: new command is registered and routes to `openTodayDailyNote`.

Manual live-vault acceptance (report as not-run if no GUI session is available):

- With `dash.md`-style setup: open note B in tab 2, focus note A in tab 1, press each of `Ctrl+-`, `Ctrl+.`, `Ctrl+,`,
  `Ctrl+=`, `Ctrl+\`, `<CR>` on a link line, and `[[`/`]]` where the target is note B — Obsidian must switch to tab 2
  instead of opening B in tab 1.
- `-` with today's daily note open in another tab activates that tab; with it closed, the Daily Notes command still
  opens/creates it as before.
- Enter on a link with a `#heading` subpath whose file is open in another tab switches tabs and scrolls to the heading.
- Each keymap still works when the target is not open anywhere (opens in current tab) and when the target is the current
  file (no tab churn).

Vault hygiene:

```bash
git -C /home/bryan/bob status --short
```

Only the three files above may be staged; the pre-existing unrelated dirty files (templates, `dash.md`, `bob.md`, daily
notes, etc.) must remain untouched. Commit task-related vault changes with `/sase_git_commit` before terminating.

## Risks

- `openLinkText` after leaf activation could, in some Obsidian versions, push a duplicate history entry; acceptable
  since it only happens for subpath links and matches Obsidian's own click-to-open behavior.
- Leaf activation timing: `revealLeaf`/`setActiveLeaf` may return before the editor is focused. The helper treats
  activation failure as "fall back to plain open", matching `openDashTasks`, so the worst case is today's behavior.
- The vimrc change requires the Vimrc Support plugin to reload mappings (Obsidian reload); stale sessions keep the old
  `daily-notes` behavior until reloaded — harmless.
- Multiple tabs holding the same file: the first match from `iterateAllLeaves` wins (same as the existing `\\`/`\|`
  behavior).
