---
create_time: 2026-06-13 13:11:36
status: done
prompt: sdd/prompts/202606/mark_closed_subprojects.md
---
# Plan: Mark closed sub-projects instead of removing them

## Problem & goal

`bob projects sync` keeps a machine-owned **Sub-projects** sub-bullet under each active project's `^prj` task, e.g.:

```markdown
- [ ] #task Ship the parent outcome [p::2] ^prj
  - 🧩 **Sub-projects:** [[Alpha]] • [[Beta]]
```

Today, the moment a listed sub-project closes (its own `^prj` is checked/canceled), sync **deletes that link** from the
line — and deletes the whole line once the last open child closes. The completed sub-projects vanish, so the parent note
loses any record of the work that rolled up into it.

**Goal:** stop deleting a sub-project link when it closes. Instead keep it on the line and render it with a clear,
beautiful "closed" indicator that distinguishes _done_ from _canceled_. The line should now act as a living ledger:
active sub-projects up top, completed ones trailing as a struck-through record.

This is purely about the _presentation/retention_ of closed children. It must **not** change the dashboard hide/surface
logic (priority `[p::2]` add/remove still keys off **open** sub-projects only).

## Recommended visual design

The note line is rendered in Obsidian, where `🧩` already sets an emoji house style. Closed children get a strikethrough
(the universal "done/closed" signal, readable in any theme) plus a trailing colored status emoji:

| State                     | Render             | Rationale                           |
| ------------------------- | ------------------ | ----------------------------------- |
| Open                      | `[[Alpha]]`        | unchanged                           |
| Done (`[x]`/`[X]` `^prj`) | `~~[[Gamma]]~~ ✅` | green check = success               |
| Canceled (`[-]` `^prj`)   | `~~[[Delta]]~~ ❌` | red X = the user's suggested marker |

**Ordering:** open children first (case-insensitively sorted, as today), then closed children (case-insensitively
sorted). This keeps the active work prominent and lets completed items settle at the end. Full example:

```markdown
    - 🧩 **Sub-projects:** [[Alpha]] • [[Beta]] • ~~[[Gamma]]~~ ✅ • ~~[[Delta]]~~ ❌
```

`✅`/`❌` mirror the existing terminal vocabulary (`✓ done` green / `✕ canceled`) used by `bob projects list`, but in
their emoji-presentation form so Obsidian renders them in color. The `~~…~~` wraps the wikilink; Obsidian still renders
the link as clickable, just struck-through (**flagged for manual verification** — see Verification).

**Alternative considered (documented for the review):** trailing emoji only, no strikethrough (`[[Gamma]] ✅`). Lower
rendering risk and slightly less busy, but a much weaker "this is closed" signal — open and closed links look nearly
identical at a glance. I recommend the strikethrough variant for clarity; this is the easiest knob to flip during
review.

## Key behavioral decision: preserve-and-mark (incremental), not show-everything

When a child closes there are two possible philosophies:

- **(A) Preserve-and-mark (recommended):** a closed child is shown _only if it is already on the line_ (i.e. it was
  tracked while open, then closed). Closed children that were never on the line are not added.
- **(B) Show-all-closed:** rebuild the line from _every_ child of the parent, open or closed, always listing all
  terminal children.

I recommend **(A)** because:

1. It matches the request precisely — "when that sub-project is closed, mark it instead of removing it." Marking happens
   at the moment of closure, to things we were already tracking.
2. **Clean migration.** Children that the old behavior already deleted stay deleted; shipping this does not
   retroactively resurrect every historical done-child onto big parent notes (which (B) would do on the first sync —
   potentially dumping long lists).
3. **User can curate.** If the user manually deletes a `~~[[X]]~~ ✅` entry, sync leaves it gone. Open children remain
   authoritative (always re-added, since they're active work hidden from the dash), but the _closed_ ledger is prunable.
   This asymmetry is both sensible and tidy.

Net rule set for a link relative to the marker line:

- Open child of this parent → always present, rendered plain. _(unchanged)_
- Listed child that just went terminal (done/canceled), still parented here → **kept, marked.** _(new — replaces
  deletion)_
- Listed link that is no longer a child at all (re-parented, deleted, not a project) → removed. _(unchanged removal, now
  only for genuine non-children)_
- Terminal child **not** on the line → ignored.
- Line deleted only when nothing remains to show (no open children **and** no tracked closed children).

## Technical design

All changes are in `src/native/projects.rs` (plus docs and tests). The existing pipeline stays intact: scan → build
per-parent child map → `plan_project_sync` produces `ProjectChange`s + events → `apply_project_changes` turns them into
`TextEdit`s. The Sub-projects line remains fully machine-owned and re-rendered into canonical form.

### 1. Classify children with state (not just "open")

Replace `open_subproject_children_by_parent_link_name` (currently returns `HashMap<parent, Vec<open_stem>>`) with a
state-aware version returning, per parent, a list of `SubprojectChild { stem, link_name, state }` where
`state ∈ { Open, Done, Canceled }`.

- Reuse each child's reconciled terminal state: prefer the `^prj` task state (`Done`/`Canceled`), falling back to
  terminal frontmatter `status`. (`PrjTask::target_status` / `ProjectStatus::is_terminal` already express this.)
- Exclude "broken" children (Missing/Malformed/Multiple `^prj` that are non-terminal) — same as today they are neither
  counted as open nor listed; they raise their own warnings.
- `has_open_subprojects` for the priority/surfacing logic = "any child with state `Open`." **This keeps dash
  hide/surface behavior identical.**

### 2. Compute the desired line in `plan_project_sync`

Given the parent's classified children and the existing marker line's links:

- `open_entries` = all `Open` children (always shown).
- `closed_entries` = `Done`/`Canceled` children **whose `link_name` already appears in the existing marker line** (the
  preserve-and-mark filter).
- `desired` = `open_entries` ∪ `closed_entries`, in canonical order (open group sorted, then closed group sorted).

Diff `desired` against the existing marker line to emit granular changes/events:

- name in `desired` not on line → `AddSubprojectLink { stem, reason }` (reason: open / done / canceled).
- name on line not in `desired` → `RemoveSubprojectLink { stem }` (now means "no longer a sub-project").
- name on both but display state changed (e.g. open → done) → new `MarkSubproject { stem, state }`.
- otherwise, if the rendered canonical text differs from the existing line (order, separators, indent, duplicate
  markers, legacy styling) → `NormalizeSubprojects`.

Idempotency holds: once a child is rendered `~~[[X]]~~ ✅`, the next run computes the same `desired`, the same canonical
text, and emits nothing.

### 3. Render & apply edits

- Extend the renderer so each entry carries its state: `render_subprojects_line_text` takes `(stem, state)` entries and
  emits `[[stem]]`, `~~[[stem]]~~ ✅`, or `~~[[stem]]~~ ❌`. Add the two emoji as named constants alongside
  `SUBPROJECTS_MARKER_PREFIX`.
- `sync_subprojects_line_edits` rebuilds the first marker line from `desired` (open-then-closed canonical order),
  deletes any duplicate marker lines, and deletes the line entirely only when `desired` is empty. The simplest,
  least-stateful approach is to pass the resolved `desired` entries straight from the plan into the edit step (rather
  than re-deriving open/closed inside the applier), since the line is fully machine-owned.
- Parsing already tolerates the new markup for free: `wikilink_refs_in_line` still extracts `[[X]]` regardless of
  surrounding `~~`/emoji, and `is_marker` detection keys only off the `🧩 **Sub-projects:**` prefix. A small helper will
  infer an existing link's _display state_ (struck + ✅/❌) from the line so step 2 can detect open→closed transitions;
  this is only used at plan time for event/no-op detection, not in a hot path.

### 4. Events / terminal output

`SyncEvent::PrjEdit` already renders `<verb> <field> <prep> ^prj  <reason>` and is colored via `Styler`. New/changed
messages (all `PrjEdit`, so they keep counting toward "N ^prj edited"):

- done: `updated [[X]] on ^prj  sub-project completed` (action `Update`)
- canceled: `updated [[X]] on ^prj  sub-project canceled` (action `Update`)
- removal reason wording: `… no longer an open sub-project` → `… no longer a sub-project` (removal now only fires for
  genuine non-children).
- add ("open sub-project") and normalize ("canonical format") unchanged.

## Docs to update

`docs/projects.md` Sync Rules / sub-project section:

- Replace "deleted when there are no open sub-projects" with the new rule: closed children are retained and marked
  (`~~[[X]]~~ ✅` done, `~~[[X]]~~ ❌` canceled); the line is removed only when no open _and_ no tracked-closed children
  remain.
- Document the open-then-closed ordering and the preserve-and-mark filter (closed children are kept only while already
  listed; deleting a closed entry by hand prunes it permanently).
- Refresh the example output block with a "sub-project completed/canceled" line.

## Tests to update / add

Integration (`tests/cli.rs`):

- **Rewrite** `projects_sync_unhides_parent_when_child_prj_is_checked_same_run`: parent still loses `[p::2]` (no open
  children) but now **keeps** `🧩 **Sub-projects:** ~~[[Child]]~~ ✅` instead of deleting the line; assert the new
  "sub-project completed" event; assert second run is a no-op (idempotent).
- **Add** a canceled-child case asserting `~~[[Child]]~~ ❌`.
- **Add** a mixed case (open + done + canceled children) asserting ordering and that an added open child + a
  newly-closed child are both handled in one run.
- **Add** a curation case: a hand-deleted closed entry stays gone; a re-parented/deleted child is still removed.

Unit (`src/native/projects.rs`):

- **Rewrite** `project_changes_delete_subproject_line_when_last_child_closes` → "marks last child closed and keeps the
  line"; keep a separate assertion that the line _is_ deleted when the last entry stops being a child.
- Update `open_subproject_parent_links_only_count_open_prj_children` and the `project_sync_plan_*subproject*` plan tests
  for the new state-aware classification, `MarkSubproject` change, and render strings.
- Add render unit tests for done/canceled formatting and open-then-closed ordering.

## Edge cases & invariants

- **Dash surfacing unchanged:** `[p::2]` add/remove keys only off open children; a parent whose children are all closed
  correctly resurfaces on the dash while still showing the closed ledger.
- **Idempotency & CRLF:** re-render must be byte-identical on re-run; preserve existing CRLF handling
  (`line_content_end`, `line_ending`).
- **Duplicate/mangled marker lines:** still collapsed to one canonical line.
- **User sub-bullets** under `^prj` remain untouched.
- **Terminal parents** still get no `^prj` line edits.

## Verification

1. `cargo test` (unit + `tests/cli.rs`).
2. `cargo clippy` / `cargo fmt` per repo norms.
3. Manual: `bob projects sync --dry-run --bob-dir <fixture>` on a fixture with open + just-closed
   - just-canceled children; confirm event lines and the rewritten note.
4. **Obsidian render check** (the one visual assumption): confirm `~~[[Note]]~~ ✅` shows a struck-through,
   still-clickable link with a colored trailing icon in both reading and live- preview modes. If strikethrough degrades
   the link, fall back to the documented emoji-only alternative.

## Out of scope

- No new subcommands/flags. No change to scanning, frontmatter reconciliation, or the priority/surfacing algorithm
  beyond reusing "open child" semantics.
- No "completed sub-projects" second line or collapsing/summarizing long closed lists (could be a follow-up if the
  single-line ledger grows unwieldy).
- No reopening animation/among-machines history; a child going terminal→open again is simply re-rendered as open.
