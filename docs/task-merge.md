# Merge tasks into a task, drawer, or section

Merging folds one or more source tasks into a destination-shaped container:
another task, one of its drawers, or a note's H2 section. The destination
decides the shape: a bare note holds tasks, so `>` to a bare note moves them
(see [Move open tasks between notes](task-move.md)); `>` to a task, drawer,
or section merges the sources into it. Split is the inverse: it turns notes
back into tasks (see [Split bullets out into tasks](task-split.md)). Merge
labels committed through capture `~` / `~~` or the Obsidian Links stage run
this same engine (see [Task relations](task-relations.md#21-merge-labels)).

This file is the contract every implementation cites. The pure planner is
`plan_task_merge` in `src/native/task_merge/`; capture `>` transfer lines,
capture relation merges, and `bob task merge` all compose it, and the
Obsidian Links stage previews and commits through `bob task merge` without
reimplementing it. If an implementation disagrees with a vector, fix this
doc first in the same change.

## 1. The rule

> Merging S into T writes T first. S's description becomes one plain title
> bullet in the container. S's authored children are **cut** beneath it, and
> their anchors' inbound links are repaired. Bob then re-points S's open
> dependents to T, prunes S's live links under today's open Pomodoros, and
> offers T in their place. S is written last: it is cancelled, carrying
> `🪢 LINKS: merged into [[T]]`.

Vocabulary used below:

- **S** is a source task.
- **T** is the target task.
- **P** is the parent (owner) of the bullets being split (split docs only).
- A **container** is T itself, one of T's drawers, or a note's H2 section.

A task holds notes, so `>` to a bare note **moves** them (identity kept,
links repaired, no back-pointer). A task, a task's drawer, or a note's
section holds notes, so `>` to any of these **merges** the sources into it
(source retires as a cancelled tombstone, `🪢 LINKS: merged into [[T]]` on S,
which T shows as _merged from_). Split turns notes back into tasks (new tasks
are born, live `[[#^id|Title]]` stubs stay in the parent).

## 2. The fate of everything a merge touches

| Thing | Fate |
| --- | --- |
| S's task line | `[-]` with `[cancelled:: today]`. `^id`, fields, and the creation date are kept, so every link to S still resolves. |
| S's description | Becomes the group title bullet `- <description>` in the container. The text comes from Bob's description extractor (`note_tasks::clean_description`). It carries **no link back to S**: a ring would draw ⊘ beside live work, and S would gain a reverse mention. Provenance is T's _merged from_ chip. |
| S's authored body (continuation paragraphs, tables, fences, and authored children) | Cut under the title bullet with relative structure kept, including bytes before the first child and body text without a list wrapper. Descendant anchors move, with transplant link repair and collision renaming. Same-named drawers are **not** flattened into T's drawers. |
| S's open nested tasks | The merge is refused (`merge_source_open_subtasks`). |
| S's managed children: Depends-On, BLOCKS, LINKS, LINKED FROM, and the Schedule, Work, Rotten, and Cancel logs | Stay on S as history. An existing Cancel Log gets `*date* — 🪢 merged into [[T]]` prepended, as relation cancels do today. |
| Open tasks that depend on S (other than T) | The production dependency graph finds direct and inherited project dependents. Resolved Depends-On links to any source are replaced with T and deduplicated; unresolved or ambiguous links stay byte-for-byte and are reported, with their field IDs retained as breadcrumbs. A dependent without a block ID refuses the merge because it cannot be safely rewritten. BLOCKS mirrors follow the configured relation policy. A resulting cycle refuses the whole operation (`merge_dependency_cycle`). Re-pointing happens before the cancel, so cancel recovery never releases them. |
| T depending on S | Left in place. Cancel's existing recovery then treats it exactly as it would any cancelled prerequisite, which may recover T with `[unblocked::]`. |
| S's own open prerequisites | Not transferred. Reported in `prerequisites_left[]`, with a hint to add them by hand. |
| `starts after` successors pointing at S | Untouched. Reported in `successors_orphaned[]`, with a warning. |
| S's links under today's **open** Pomodoros | Pruned with cancel's rules (`task_cancel::prune`). |
| Closed sessions, other days, and mentions of S | Untouched. |
| T's status, schedule, priority, type, and freshness | Unchanged. T owns them. |
| T's LINKED FROM | Gains `[[S]] (merged into)`, which displays as _merged from_. |

## 3. Offers

When S lost at least one live link under today's open Pomodoros, T has no
live link in any of today's open Pomodoros, and T can hold one (open and not
Blocked), then **T is offered once**.

- The offer sits under the earliest open entry, in ledger order, that lost a
  source link. Its destination is the first removed source bullet's slot, so
  T takes S's place.
- The offer reuses group `hand_off` and adds an additive `merged_from`
  field (S's key) to the offer entry. Pickers, the Mac Pull-In card, and
  capture's `-p/--pull-in` therefore need no new group.
- A pick links T and applies the existing pick semantics: it raises T to Next
  and stamps it fresh. Bob never auto-links.

## 4. Containers and the strict section resolver

- **Task:** T's plain-child anchor, computed the way `capture/sub_bullet.rs`
  computes it.
- **Drawer:** `resolve_drawer_for_write` (or `resolve_h2_for_write` on
  `^prj`). Insert at `drawer_insertion` / `h2_insert_position`. A missing
  drawer is created through the shared ensure helper.
- **Section (`@c#section`, `bob task merge -s`):**
  - The candidates are the note's eligible non-structural H2s. `## Tasks`,
    status-group headings, and reserved headings are never sections.
  - An exact-key or slug match selects that H2.
  - A strict prefix, or a slug at edit distance 1, of an existing heading is
    refused (`merge_section_near_duplicate`, copy "did you mean
    ## Future Work?").
  - Otherwise `## Title Case` is appended at the end of the note.
  - `#"Exact Title"` forces creation with the exact text.
  - The CLI's `-s TITLE` matches exactly (case-insensitive) and creates the
    section as typed, like `--drawer`.
  - This is deliberately stricter than `@c#Prefix` section bullets, which
    fall back to the zeroth section and never create a heading.
- **Section provenance:**
  - Sections have no task endpoint, so S gets a Cancel Log entry
    `*date* — 📥 merged into [[c#Section]]` instead of a relation.
  - A section merge is refused when S has open dependents
    (`merge_section_has_dependents`), because there is no task for them to
    wait on.
  - Typing `@proj+prj#section` instead gives a relation-bearing merge into
    `^prj`'s H2 drawer.
- **Payload shape:**
  - The title bullet sits at the container's child indentation, and S's
    children sit one level below it.
  - Relative indentation is preserved. When the two notes use different
    indent units, leading list indentation is converted unit for unit.
    Fenced-code content is only re-based.
  - With several sources, each one is a separate group, in typed order,
    appended after the container's existing entries.

## 5. Refusals

Every refusal refuses the whole operation, before any write. The codes are:

| Code | Refused when |
| --- | --- |
| `merge_source_closed` | S is closed, unless it is already merged into this same T (see §6) |
| `merge_source_recurring` | S is a recurring task |
| `merge_source_project` | S is a `^prj` task |
| `merge_source_open_subtasks` | S has open nested tasks |
| `merge_target_closed` | T is closed |
| `merge_target_is_source` | T is one of the sources |
| `merge_target_inside_source` | T is nested under S |
| `merge_dependency_cycle` | re-pointing a dependent would create a cycle |
| `merge_section_has_dependents` | a section merge's S has open dependents |
| `merge_section_near_duplicate` | the section name nearly matches an existing heading |
| `merge_section_reserved` | the section name is reserved or structural |
| `ambiguous_drawer`, `reserved_drawer`, `drawer_near_duplicate` | the existing drawer-resolver codes |

S **may** be nested (its tombstone stays in place) and may live in the same
note as T.

## 6. Idempotency and the gesture variants

- **Re-runs.** When S is cancelled and its LINKS line already holds this
  merge label to this T (or, for a section, its Cancel Log already holds
  this entry), that source's outcome is `unchanged`. It costs zero writes
  and exits 0. A whole-`unchanged` run makes no commit.
- **Label gestures** (capture `~` / `~~` and the Links stage with a
  configured merge label) with an already-closed S write the relation only,
  with outcome `already_closed`. The explicit forms (`>`, `bob task merge`)
  refuse instead.
- **Reconcile.** `merge` followed by `bob task reconcile` must be a no-op.

## 7. Write order

1. T's note.
2. Dependents' notes.
3. Other link-repaired notes.
4. The daily note.
5. S's notes, last.

A failure after T leaves S open and intact: a visible duplicate, never a
loss. Capture inherits this order from `CaptureBatchPlanner`'s
destination-before-source staging. The CLI inherits it from the task-move
write-plan ranking.

## 8. Merge labels (summary)

Configurable merge labels live in `relations.merge_labels` (default `[]`;
the `>` and `bob task merge` gestures always write the first configured
label, falling back to `merged into`). Committing a merge label through
capture `~` / `~~` or the Links stage merges the source into the target at
commit time. The one-effect-per-label overlap rule spans all four lists;
a key in more than one list is dropped from all of them with a warning.
The full contract, gestures, verdicts (`merged`, `already_closed`,
`new_task`, `conflict`), equivalences, and copy live in
[Task relations §21](task-relations.md#21-merge-labels).

## 9. Command line

`bob task merge` rewrites task lines across the vault, so it lives under
the `bob task` noun. Options are alphabetical and every long option has a
short alias.

```text
bob task merge [OPTIONS] -i <TASK|NOTE> [-D TITLE | -s TITLE] <TASK>...
  -D, --drawer TITLE           T's drawer, exact title; created if missing (^prj: H2, Title Case)
  -d, --dry-run                Plan and print only; never locks, syncs, mints, creates, or writes
  -f, --format human|json      Default: human
  -i, --into TASK|NOTE         a task, or a note when -s is given
  -o, --offline                Skip both sync cycles; commit locally
  -p, --pull-in PICKS          all, none, or offer numbers/keys — same parser as capture
  -r, --retry-timeout SECONDS  Lock wait and re-plan budget
  -s, --section TITLE          the note's H2, created if missing
```

- **Teaching misuse.**
  - `merge -i dev` without `-s` is refused (`merge_into_note_without_section`)
    with "a note holds tasks; use `bob task move -t dev`, or name a section
    with -s".
  - `move -t dev:upgrade` is refused (`move_to_task`) with "to fold tasks
    into dev:upgrade, use `bob task merge -i dev:upgrade …`".
- **Shared shell:** every verb takes the maintenance lock, pre-syncs (skipped
  by `-o`), runs the scan → plan → write-plan retry loop, makes guarded
  staged writes (destination first, source last), makes one scoped git
  commit, and post-syncs. `--dry-run` skips the lock, sync, Git, and writes,
  and its plan equals the live plan. Exit 2 means usage and exit 1 means
  operational. The commit subject is
  `bob task merge -> db#^upgrade (2 tasks)`.
- **JSON** uses schema 1, the same envelope as `bob task move`: `ok`,
  `dry_run`, `warnings`, `repaired_files[]`, `git`, `applied_paths[]`,
  `deferred_paths[]`, `recovery_directory`, and
  `error{stage, code, message, hint}`. Merge records, targets, sources, and
  offers use snake_case field names; absent optional fields are omitted.
  - **`merges[]`** (`MergeRecord`, shared with capture):
    - `source{path, block_id, line, title}`;
    - `target{path, block_id?, title?, drawer?, section?, kind}`;
    - `label`, `outcome` (`merged`, `unchanged`, or `already_closed`),
      `previous_status`;
    - `moved_lines`, `moved_notes`;
    - `dependents_repointed[]`, `prerequisites_left[]`,
      `successors_orphaned[]`;
    - `links_pruned`, `offers[]`;
    - `message` (Bob-authored).
  - The top level adds `pull_in` (capture's shape).
- **Human output.** Coloured with the capture styler. A dry run says
  _Would …_. Merge output looks like this:

```text
⤵ Merged 1 task into “Upgrade Postgres to 17” · db.md › FUTURE WORK
  ⊘ Benchmark logical replication   inbox.md:1 · Next → Cancelled · 2 notes moved
  ⛓ Cut over prod                   now waits on “Upgrade Postgres to 17”
  ◌ BUILD link removed              offer 1: link “Upgrade Postgres to 17” under BUILD (-p 1)
  ↻ repaired 1 link in 1 note
git  committed 1a2b3c4 · synced
```

When a live run leaves offers unpicked, it prints one hint naming the
capture that links T later (`bob capture '@db:upgrade'`).

## 10. Canonical copy

The `contract` phase records this table here and in
[Split bullets out into tasks](task-split.md). Rust, JS, and the Mac app
use these strings verbatim. Bob authors `message`/`detail` fields, and
clients render them.

| Key | Text |
| --- | --- |
| merge header | `⤵ Merged N task(s) into “T” · c.md[ › CONTAINER]` / `Would merge …` |
| source row | `⊘ S  path:line · Prev → Cancelled · N notes moved` (or `· title only`) |
| dependent row | `⛓ D  now waits on “T”` |
| prune/offer row | `◌ ENTRY link removed  offer N: link “T” under ENTRY` |
| prerequisite warning | `⚠ “S” waited on “P”; “T” does not · add &note:id` |
| successor warning | `⚠ N task(s) start after “S”; they won't be offered when “T” finishes · relink with ~` |
| repair row | `↻ repaired N link(s) in M note(s)` |
| section Cancel Log | `📥 merged into [[c#Section]]` |
| merge_section_has_dependents | `“D” waits on ^s; merge into a task (@c+id) so it can wait there` |
| merge_source_open_subtasks | `“S” has N open subtasks; move or close ^sub first` |
| merge_dependency_cycle | `“D” would wait on “T”, which already waits on it` |
| merge_target_inside_source | `^t is nested under ^s; merge ^s elsewhere or move ^t out first` |
| transfer_extra_text | `a > line takes one destination and nothing else; put notes in a separate @c+id item` |
| transfer_source_suffix | `sources are whole tasks; drop #drawer from @a+x` |
| transfer_arrow_spacing | `put spaces around >: @a+x > @c` |
| shell_ate_transfer_arrow | `your shell ate “>”: quote the draft (bob capture '@a+x > @c') and delete the empty file “@c”` |
| nav merge strip | `⤵ Merges “S” into “T” · moves N notes · cancels “S” · N dependent(s) now wait on “T” · offers “T” under ENTRY` |
| nav mobile | `⤵ merging needs the desktop app` |

Glyphs: ↪ move, ⤵ merge, ✂ split, ⊘ cancelled, ⛓ dependency, ◌ removed
link, ↻ repair, and ⚠ warning.

## 11. Reliability guarantees and invariants

Markdown files cannot give crash-atomic multi-file transactions. What is
promised instead:

- each operation is validated as one plan;
- writes are guarded, run in copy-then-retire order, and leave recoverable
  partial failures;
- outcomes are reported as `applied`, `unchanged`, `stale`, or `partial`
  with paths;
- every input is revalidated at commit, including the dependency graph's
  read set.

Property tests (Rust) must hold for merge, split, and many-source move:

1. Every block link that resolved before still resolves, to the moved task,
   the tombstone, or the relocated anchor.
2. `bob task reconcile` is a no-op afterwards: mirrors, BLOCKS, fields,
   and LINKED FROM all converge.
3. A dry run equals the result except for `dry_run`.
4. A re-run is `unchanged`, with zero writes.
5. A merge never produces a transient unblock of a re-pointed dependent.

## 12. MG vector index

Each MG row names the preimage, the gesture, and the byte-exact postimage
expectation. The implementation tests live in
`src/native/task_merge/tests.rs`.

| ID | Covers | Preimage → gesture → postimage |
| --- | --- | --- |
| MG1 | Title-only S into T | S with no children merges as one title bullet under T's plain-child anchor; S retires with `merged into` |
| MG2 | Nested drawers travel | S with nested drawers cuts them under the title bullet; same-named drawers are not flattened |
| MG3 | Anchors linked from a daily note | Descendant anchors move with transplant link repair and collision renaming; daily-note links still resolve |
| MG4 | Authored body | Continuation prose before children, wrapper-free tables and fences, and authored children travel; direct managed logs stay on S |
| MG5 | Re-point dependents | The production graph finds direct and inherited project dependents; resolved links re-point, unresolved links and field breadcrumbs remain, and ID-less dependents refuse |
| MG6 | T depends on S | Left in place; cancel recovery may recover T with `[unblocked::]` |
| MG7 | Dependency cycle | Re-pointing that would create a cycle refuses the whole operation (`merge_dependency_cycle`) |
| MG8 | Successor warning | Configured `starts after` successors of S are untouched and reported in `successors_orphaned[]`; labels outside the supplied policy are ignored |
| MG9 | No transient unblock | Re-pointing lands before the cancel, so cancel recovery never releases a re-pointed dependent |
| MG10 | T already linked | No offer when T already has a live link under today's open Pomodoros |
| MG11 | T Blocked | No offer when T cannot hold one; the prune still lands |
| MG12 | Offer placed | T with no live link is offered once under the earliest open entry that lost a source link, at the first removed bullet's rebased slot after edits, with `merged_from` |
| MG13 | Section dependents refusal | A section merge whose S has open dependents is refused (`merge_section_has_dependents`) |
| MG14 | Section near-duplicate refusal | A section name at strict prefix or slug edit distance 1 is refused (`merge_section_near_duplicate`) |
| MG15 | Section Title Case creation | A missing section is appended as `## Title Case`; `#"Exact Title"` forces exact text |
| MG16 | Re-run unchanged | A merged S re-run against the same T is `unchanged` with zero writes and no commit |
| MG17 | Reconcile no-op | `merge` followed by `bob task reconcile` changes nothing |
| MG18 | Nightly archive | A tombstone archived by `bob task archive` keeps T's _merged from_ entry rewritten to the `done/` path |
| MG19 | Label in two effect lists | A key in `merge_labels` and any of `cancel_labels`, `complete_labels`, or `successor_labels` has no effect and is dropped from all lists with one warning |
| MG20 | Payload order | Several sources land as separate groups in typed order after the container's existing entries |

The current test names for those vectors are:

| ID | Test |
| --- | --- |
| MG1 | `mg1_title_only_source_is_prepared_as_one_plain_child` |
| MG2 | `mg2_nested_drawers_travel_under_the_source_title_without_flattening` |
| MG3 | `mg3_descendant_anchor_collision_repairs_external_links` |
| MG4 | `mg4_tables_and_fences_are_rebased_as_authored_payload`, `mg_successful_merge_fixtures_hold_identity_reconcile_and_repeatability` (wrapper-free body fixture) |
| MG5 | `mg5_open_dependents_repoint_with_mirrors_and_provenance`, `mg5_shared_dependent_dedups_to_one_target_link`, `repoint_preserves_unresolved_links_and_field_breadcrumbs`, `idless_open_dependents_are_refused_for_task_and_section_merges`, `relation_policy_controls_mirrors_and_preserves_existing_mentions`, `malformed_linked_from_refuses_only_when_mirror_is_enabled` |
| MG6 | `mg6_target_prerequisite_on_source_is_never_repointed` |
| MG7 | `mg7_repoint_cycle_refuses_the_whole_batch`, `cycle_checks_include_project_and_inherited_resident_edges` |
| MG8 | `mg8_starts_after_successors_are_reported_untouched`, `successor_orphans_use_only_the_supplied_normalized_labels` |
| MG9 | `mg9_repoint_lands_before_cancel_without_transient_recovery`, `mg9_source_prerequisites_are_retained_and_reported` |
| MG10 | `mg10_existing_target_link_suppresses_merge_offer` |
| MG11 | `mg11_blocked_target_is_not_offered_after_pruning` |
| MG12 | `mg12_offer_uses_earliest_removed_slot_and_selected_pick_semantics`, `mg12_picked_slot_rebases_after_same_daily_note_edits_and_source_permutations` |
| MG13 | `mg13_section_merge_with_open_dependents_refuses`, `mg13_section_provenance_writes_one_cancel_log_entry`, `idless_open_dependents_are_refused_for_task_and_section_merges`, `section_batch_logs_each_source_and_retry_is_write_free`, `section_retry_ignores_matching_prose_fences_and_later_task_logs` |
| MG14 | `mg14_section_slug_near_duplicate_refuses` |
| MG15 | `mg15_section_slug_title_cases_and_exact_literal_preserves_text` |
| MG16 | `mg16_rerun_against_same_target_is_unchanged`, `mg16_public_planner_rerun_has_no_writes_and_serializes_source_order`, `planner_validates_duplicates_and_source_structure_before_retry_filtering`, `unchanged_retry_skips_unrelated_preexisting_dependency_cycles`, `closed_label_gesture_writes_only_relations_and_keeps_unrelated_entries` |
| MG17 | `mg17_production_reconciliation_is_a_noop_after_merge` |
| MG18 | `mg18_archive_planner_repairs_merged_from_to_tombstone_archive` |
| MG19 | `mg19_shared_key_with_merge_labels_drops_everywhere_once`, `mg19_three_list_warnings_stay_byte_identical` |
| MG20 | `mg20_source_groups_keep_typed_order_after_existing_entries` |

The JSON wire shape is pinned by
`merge_record_and_offer_json_use_snake_case_and_omit_absent_fields`.
