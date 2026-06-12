---
create_time: 2026-06-12 08:27:18
status: done
prompt: sdd/prompts/202606/obsidian_alt_file_type_fallback.md
---
# Obsidian `<Ctrl+,>` Keymap: Fall Back to `type` When `alt_file` Is Missing

## Goal

Extend the just-shipped `<Ctrl+,>` keymap (`open-alt-file-note`, committed as `636b65f`) so that when the active note
has no usable `alt_file` frontmatter link, the command falls back to jumping to the file linked by the `type`
frontmatter property instead. `alt_file` keeps priority when both are present.

## Context Reviewed

- Prior approved plan/tale: `sdd/tales/202606/obsidian_alt_file_property_keymap.md` (implemented earlier today; this
  plan builds directly on it).
- Live plugin: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` — current `openAltFileNote()` is a
  thin wrapper at line ~2965: `openFrontmatterLink("alt_file", "No alt_file link found", "Alt file note not found")`.
- `openFrontmatterLink(fieldName, missingMessage, notFoundMessage)` (line ~3544): reads the active file's frontmatter
  from the metadata cache, extracts the first link via `getFrontmatterLink` → `getFrontmatterLinks` →
  `extractLinkTarget`, shows `missingMessage` when no link is extractable, otherwise routes through
  `openOrCreateLinkTarget` (which falls into the create-missing-note flow when the target doesn't exist).
- The plugin already treats `type` as a link-valued property: `isAreaOrProjectNote()` calls
  `getFrontmatterLinks(frontmatter, "type")`.
- Vault survey: `type` is consistently a wikilink across ~1500 notes (`type: "[[day]]"` ×1070, `"[[ref]]"` ×319,
  `"[[restaurant]]"` ×91, plus `project`/`done`/`area`/`inbox`), and every one of those seven target notes exists at the
  vault root (`day.md`, `ref.md`, …). So the fallback almost always has a real target.
- Vault instructions: `/home/bryan/bob/AGENTS.md` — inspect `git status` first, never touch unrelated dirty files,
  commit task files via `/sase_git_commit` before terminating.
- Workspace long memory: only `memory/long/cli_rules.md` exists, scoped to new bob-cli subcommands/options; this task
  changes no bob-cli code, so no audited long-memory read is required (the Obsidian-domain read was already performed
  earlier in this session).
- Current vault dirt (all unrelated, must stay untouched): `_templates/daily.md`, `_templates/new_project.md`,
  `gtd_daily.md`, untracked `2026/20260612.md`, `Foobar_Mccar.md`, `ref/chat/repeat_stop_variable_consolidated.md`.

## Design

### Behavior

Pressing `<Ctrl+,>`:

1. If a link can be extracted from the active note's `alt_file` frontmatter property → jump to it (unchanged).
2. Otherwise, if a link can be extracted from the `type` property → jump to that note (e.g. a daily note with no
   `alt_file` jumps to `day.md`).
3. Otherwise → Notice `"No alt_file or type link found"`.

"Not found" means _no extractable link_: the fallback triggers when `alt_file` is absent, empty, or yields no link-like
value — the same condition under which the current code shows its missing-link Notice. All existing link semantics carry
over to the `type` lookup unchanged: wikilinks/markdown links/plain paths all work, the first entry wins for arrays, and
a link whose target file doesn't exist goes through the existing create-missing-note flow (same as `template`/`parent`;
in practice all current `type` targets exist).

### Decisions

- **Command id and name stay `open-alt-file-note` / "Open alt file note".** The id is referenced by `hotkeys.json`
  (`bob-navigation-hotkeys:open-alt-file-note`); renaming would break the binding for no benefit.
- **`openFrontmatterLink` stays untouched.** It is shared by `openParentNote()` and `openTemplateNote()`; the fallback
  logic lives entirely in `openAltFileNote()`, which picks the field first and then delegates, keeping the shared helper
  single-purpose.
- **Notices stay field-accurate.** Missing-link case reports both fields (`"No alt_file or type link found"`); the
  unresolvable-target case keeps `"Alt file note not found"` when `alt_file` was used and says `"Type note not found"`
  when the fallback was used.

## Implementation Steps

1. **Rewrite `openAltFileNote()` in `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`** (the only file
   change):
   - Get the active markdown file via `getActiveMarkdownFile()` (return silently if none, matching
     `openFrontmatterLink`'s own guard).
   - Read frontmatter from `this.app.metadataCache.getFileCache(file)?.frontmatter` and pick the field: `"alt_file"` if
     `getFrontmatterLink(frontmatter, "alt_file")` returns a link, else `"type"`.
   - Delegate to
     `this.openFrontmatterLink(fieldName, "No alt_file or type link found", <field-specific not-found message>)` as
     described under Design.
2. **No other changes**: `hotkeys.json` already binds `<Ctrl+,>` to this command; no vimrc, manifest, types.json, or
   bob-cli changes.

## Validation

1. Static check: `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
2. Ad-hoc Node smoke test (the established mocked-`obsidian` pattern from the previous tale), mocking
   `getActiveMarkdownFile` and `metadataCache.getFileCache`, asserting the `open-alt-file-note` callback:
   - frontmatter `{alt_file: "[[x]]", type: "[[day]]"}` → `openFrontmatterLink` invoked with `"alt_file"`;
   - frontmatter `{type: "[[day]]"}` → invoked with `"type"`;
   - frontmatter `{}` (and an empty `alt_file` value) → invoked with `"type"`, whose own missing-link path yields the
     combined Notice text `"No alt_file or type link found"`.
3. Manual verification by Bryan after reload:
   - `<Ctrl+,>` on a note with `alt_file` still jumps to the `alt_file` target;
   - `<Ctrl+,>` on a normal note without `alt_file` (e.g. a daily note) jumps to its `type` note;
   - `<Ctrl+,>` on a note with neither property shows "No alt_file or type link found".

## Commit Workflow

Per `/home/bryan/bob/AGENTS.md`: re-inspect `git -C /home/bryan/bob status` immediately before editing, stage **only**
`.obsidian/plugins/bob-navigation-hotkeys/main.js`, and commit via the `/sase_git_commit` skill before terminating.
