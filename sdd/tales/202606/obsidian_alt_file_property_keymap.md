---
create_time: 2026-06-12 08:15:23
status: wip
prompt: sdd/prompts/202606/obsidian_alt_file_property_keymap.md
---
# Obsidian `alt_file` Frontmatter Property + `<Ctrl+,>` Keymap Plan

## Goal

Add support to Bryan's `~/bob` Obsidian vault for a new `alt_file` frontmatter property and a new `<Ctrl+,>` keymap that
jumps to the file the property points to — mirroring the existing `template` property + `<Ctrl+.>` keymap behavior
exactly.

## Context Reviewed

- Workspace short memory: `memory/short/sase.md`. Workspace long memory contains only `memory/long/cli_rules.md`, which
  is scoped to new CLI subcommands/options; this task changes no bob-cli code, so no audited long-memory read is
  required.
- Vault instructions: `/home/bryan/bob/AGENTS.md` — the vault is actively synced by Obsidian Sync; inspect `git status`
  before editing, never touch unrelated dirty files, and commit task files via `/sase_git_commit` before terminating.
- Live plugin: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (~4284 lines, plain readable JS
  maintained in place — established by prior tales such as `sdd/tales/202606/obsidian_ctrl_jk_header_navigation.md`).
- Live hotkey file: `/home/bryan/bob/.obsidian/hotkeys.json`.
- Live vim mappings: `/home/bryan/bob/obsidian_vimrc.md` (loaded by `obsidian-vimrc-support`).
- Prior keymap plans/tales: `obsidian_ctrl_jk_header_navigation`, `obsidian_ctrl_hotkey_rotation`,
  `ctrl_backslash_cursor_position`.

## Current Findings

### How `template` / `<Ctrl+.>` works today

1. `main.js` registers command `open-template-note` ("Open template note") whose callback is `openTemplateNote()`, a
   thin wrapper: `openFrontmatterLink("template", "No template link found", "Template note not found")`.
2. `openFrontmatterLink(fieldName, ...)` reads the active file's frontmatter from the metadata cache, extracts the first
   link-like value via `getFrontmatterLink` → `extractLinkTarget` (accepts wikilinks `[[x]]`, markdown links, and plain
   path strings; arrays use the first entry), and routes through `openOrCreateLinkTarget`, which opens the resolved note
   or falls into the existing create-missing-note flow (Templater scaffolding) when the target does not exist.
3. `hotkeys.json` binds `bob-navigation-hotkeys:open-template-note` to `{"modifiers": ["Ctrl"], "key": "."}`.
4. There is **no** vimrc mapping for `<Ctrl+.>` — punctuation Ctrl-chords pass through CodeMirror Vim, so the Obsidian
   hotkey alone covers normal mode, insert mode, and non-vim contexts. `<Ctrl+,>` behaves the same way, so this plan
   also needs no `obsidian_vimrc.md` change.

### Constraints discovered

- **Naming collision risk:** the plugin already has an unrelated `open-alternate-file` command (`<Ctrl+\>`, vim-style
  "last visited file" toggle backed by `this.alternateFilePath`). The new command must be named so the two cannot be
  confused. Following the `open-parent-note`/`open-template-note` convention, the new command will be
  `open-alt-file-note` ("Open alt file note") with method `openAltFileNote()`.
- **Hotkey conflict:** `hotkeys.json` has no `Ctrl+,` binding today, but Obsidian's built-in default for
  `app:open-settings` is `Mod+,` (= `Ctrl+,` on Linux, where this vault runs). Repo precedent
  (`obsidian_ctrl_jk_header_navigation`, which cleared `editor:insert-link` to free `Ctrl+K`) is to explicitly clear the
  conflicting default rather than rely on shadowing. Settings remain reachable via the gear icon and the command
  palette.
- The vault currently has unrelated dirty/untracked files (`_templates/daily.md`, `gtd_daily.md`, today's daily note, a
  ref chat note). These must be left untouched; only the two task files may be staged.
- No property registration is needed elsewhere: `template` appears in neither `.obsidian/types.json` nor the Metadata
  Menu config, so `alt_file` stays consistent by also not being registered.

## Design

`alt_file` is a free-form frontmatter property whose value is a link to any vault file, e.g.:

```yaml
alt_file: "[[some_note]]"
```

Wikilinks, markdown links, and plain vault paths all work, with identical semantics to `template` (first value wins for
arrays; missing targets are created through the existing create-missing-note flow). `<Ctrl+,>` jumps to that file;
sensible Notices appear when the property is absent or unparsable.

## Implementation Steps

1. **Extend `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`** (no manifest change needed):
   - In `onload()`, register the new command immediately after `open-template-note`:
     - id: `open-alt-file-note`, name: `Open alt file note`, callback: `() => this.openAltFileNote()`.
   - Add `openAltFileNote()` directly after `openTemplateNote()`, mirroring it:
     `openFrontmatterLink("alt_file", "No alt_file link found", "Alt file note not found")`.

2. **Update `/home/bryan/bob/.obsidian/hotkeys.json`**:
   - Add `"bob-navigation-hotkeys:open-alt-file-note": [{"modifiers": ["Ctrl"], "key": ","}]` next to the other
     `bob-navigation-hotkeys` entries.
   - Add `"app:open-settings": []` to explicitly clear the built-in `Mod+,` default and avoid the conflict (precedent:
     the existing `"editor:insert-link": []` entry).

3. **No changes** to `obsidian_vimrc.md`, plugin `manifest.json`, Metadata Menu config, `types.json`, or any bob-cli
   code.

## Validation

1. Static checks:
   - `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
2. Ad-hoc Node smoke test (established pattern for this plugin): require `main.js` with a mocked `obsidian` module,
   instantiate the plugin, run `onload()`, and assert that `open-alt-file-note` is registered and that its callback
   invokes `openFrontmatterLink` with field name `"alt_file"`. No new pure helpers are introduced, so no
   `module.exports.helpers` additions are needed.
3. Manual verification by Bryan in Obsidian after reload:
   - `<Ctrl+,>` on a note with `alt_file: "[[x]]"` jumps to `x`.
   - `<Ctrl+,>` on a note without `alt_file` shows "No alt_file link found".
   - `<Ctrl+.>` (template) still works unchanged.

## Commit Workflow

Per `/home/bryan/bob/AGENTS.md`: re-inspect `git -C /home/bryan/bob status` before editing, stage **only**
`.obsidian/plugins/bob-navigation-hotkeys/main.js` and `.obsidian/hotkeys.json`, and commit via the `/sase_git_commit`
skill before terminating.
