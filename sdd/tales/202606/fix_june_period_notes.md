---
create_time: 2026-06-07 08:51:16
status: wip
prompt: sdd/prompts/202606/fix_june_period_notes.md
---
# Plan: Fix June 2026 Daily and Period Notes

## Goal

Bring the existing June 2026 Bob daily notes into the parent-chain format introduced by the approved period-template
work, create the missing June 2026 monthly period note at `~/bob/2026/202606.md`, and adjust `~/bob/2026.md` only where
needed so it points at the new nested monthly-note structure.

The intended hierarchy from the approved plan is:

- `~/bob/2026/202606DD_day.md` parents to `[[2026/202606]]`
- `~/bob/2026/202606.md` parents to `[[2026]]`
- `~/bob/2026.md` remains the yearly note and links to month notes

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read required Obsidian long memory with:
  `sase memory read long/obsidian.md --reason "Need Bob vault note structure and frontmatter conventions before planning June daily note and period note fixes"`.
- Read `/home/bryan/bob/AGENTS.md`; vault changes must preserve unrelated dirty state and be committed through the SASE
  git commit workflow before terminating after implementation.
- Read the approved prior plan at `/home/bryan/.sase/plans/202606/bob_period_templates.md`.
- Inspected the current templates:
  - `/home/bryan/bob/_templates/daily.md`
  - `/home/bryan/bob/_templates/monthly.md`
  - `/home/bryan/bob/_templates/yearly.md`
- Inspected current June files:
  - `/home/bryan/bob/2026/20260601_day.md`
  - `/home/bryan/bob/2026/20260602_day.md`
  - `/home/bryan/bob/2026/20260603_day.md`
  - `/home/bryan/bob/2026/20260604_day.md`
  - `/home/bryan/bob/2026/20260605_day.md`
  - `/home/bryan/bob/2026/20260606_day.md`
  - `/home/bryan/bob/2026/20260607_day.md`
- Inspected existing root-level generated month notes such as `202601.md`, `202605.md`, `202506.md`, and `202406.md`.

## Current Findings

- The vault is already dirty from unrelated user/sync changes.
- `2026/20260606_day.md` and `2026/20260607_day.md` are currently untracked, but they are existing June daily notes and
  are in scope for this request.
- All currently existing June 2026 daily notes lack `parent`.
- All currently existing June 2026 daily notes also lack `created`; the current daily template includes it.
- Filesystem birth times are available for the June daily files, so `created` can be populated in the same format as the
  daily template (`YYYY-MM-DDTHH:mm:ssZ`) without inventing timestamps.
- `~/bob/2026/202606.md` does not exist.
- `~/bob/202606.md` does not exist.
- Existing generated month notes for January through May 2026 live at the vault root (`202601.md` through `202605.md`).
  Moving those legacy files is outside this task.
- `~/bob/2026.md` already has `parent: [[journal]]` and lists all months by basename, including `[[202606]]`.

## Design

Treat "new format" for existing June daily notes as frontmatter parity with the current daily template where it is safe
and non-destructive:

- Add `created` if missing, using the file's birth time.
- Add `parent: '[[2026/202606]]'` if missing, matching the current daily template's quoted wikilink style.
- Preserve existing daily note bodies, task queries, headings, aliases, tags, IDs, and user content.
- Do not regenerate daily notes from the template, because the existing notes have content and small body differences.

Create `~/bob/2026/202606.md` as the resolved form of the current monthly template:

- `type: monthly`
- `created: <current creation timestamp>`
- `date: 2026-06`
- `parent: "[[2026]]"`
- `template: "[[monthly]]"`
- `aliases: [2026-06 Jun]`
- `tags: [monthly]`
- `id: 202606`
- heading `# 2026-06 Jun`

Do not create or move a root-level `~/bob/202606.md`.

For `~/bob/2026.md`, make only link-target corrections needed by the new nested month-note structure:

- Keep existing generated metadata and `parent: [[journal]]`; replacing the whole file with the new yearly template
  would discard useful generated context and is not necessary for the requested fix.
- Change the June link to target the nested file explicitly, while preserving visible text, for example
  `[[2026/202606|202606]]`.
- Consider changing July through December links to explicit nested missing targets as well, because those files do not
  exist yet and this lets the Enter-link creation plugin select the monthly template later. Keep January through May
  pointing at the existing root-level generated month files.

## Implementation Steps

1. Re-check `git -C /home/bryan/bob status --short` immediately before editing.
2. Generate an audit of all existing `2026/202606??_day.md` files and confirm the exact target set is June 1 through
   June 7 only.
3. Patch each target daily note frontmatter:
   - insert `created: <birth-time timestamp>` after `type: daily` if absent;
   - insert `parent: '[[2026/202606]]'` after `date: 2026-06-DD` if absent;
   - leave all other lines unchanged.
4. Add `/home/bryan/bob/2026/202606.md` from the resolved monthly template.
5. Patch `/home/bryan/bob/2026.md` month links as described above, without changing generated frontmatter or unrelated
   year-note content.
6. Inspect the vault diff and verify it is limited to:
   - `2026.md`
   - `2026/202606.md`
   - existing June daily notes `2026/20260601_day.md` through `2026/20260607_day.md`

## Verification

Run after implementation:

```bash
git -C /home/bryan/bob diff --check -- \
  2026.md \
  2026/202606.md \
  2026/20260601_day.md \
  2026/20260602_day.md \
  2026/20260603_day.md \
  2026/20260604_day.md \
  2026/20260605_day.md \
  2026/20260606_day.md \
  2026/20260607_day.md
```

Focused content checks:

- every existing `2026/202606??_day.md` has exactly one `parent: '[[2026/202606]]'`;
- every existing `2026/202606??_day.md` has exactly one `created:` field;
- `2026/202606.md` has monthly frontmatter and `parent: "[[2026]]"`;
- `2026.md` links June to `2026/202606` explicitly and does not lose its generated metadata;
- no root `202606.md` was created.

## Risks and Mitigations

- The vault is already dirty. Mitigation: inspect status before editing, patch only requested files, and stage only
  task-owned paths.
- Two June daily notes are untracked. Mitigation: because they are explicitly in the requested June daily set, include
  them in the task diff if implementation proceeds; call this out before committing.
- Retrofitting `created` onto existing notes could be ambiguous if birth times were unavailable. Mitigation: birth times
  are available now; if any become unavailable before implementation, skip `created` for that file and report it.
- Legacy root month notes coexist with the new nested monthly-note format. Mitigation: do not migrate January through
  May in this task; only create the requested nested June note and adjust `2026.md` links needed for current/future
  nested month creation.

## Commit Plan

After implementation, commit only the task-related vault changes using the required SASE git commit workflow. If the
unrelated dirty vault state would pollute SASE commit metadata, temporarily stash unrelated unstaged/untracked changes
with `--keep-index`, commit the staged task diff, then restore the stash.
