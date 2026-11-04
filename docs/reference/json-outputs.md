---
title: JSON outputs
description: Every bob machine-readable output, its flag, envelope shape, schema version, and contract link.
---

# JSON outputs

This page is the command-level lookup for machine-readable output. Each row
names the command, the flag that selects the shape, what the envelope
carries, the schema version the command actually prints (or explicitly
unversioned when the JSON carries no version), and the contract that owns the
semantics. Generated CLI pages own the exact flags; contracts own the
behavior.

Fixture prerequisites match the other reference pages: run the examples from
this checkout's `bob` in an isolated `HOME`/`XDG_*` tree, a synthetic vault,
`BOB_NOW=2026-10-20 12:00:00`, `TZ=UTC`, and `BOB_REF_JOBS_KICK=off`. Network,
clipboard, sync, and hook execution are stubbed or avoided. Stored file
versions are not command versions: freshness stored state and the review
journal carry their own numbers, and only the `schema_version` printed on
stdout counts below.

## Framing

JSON reports print to stdout. Warnings, progress, and hook chatter print to
stderr so a JSON pipe stays parseable. Usage errors exit 2.

Success and failure envelopes are per command family. Commands do not share
one `ok` / `error` / `schema_version` shape:

- Versioned helpers such as `capture-parse`, `plan`, `freshness`, and most
  `ref` commands print `ok: true` plus a payload on success. Many of those
  success envelopes carry `schema_version`.
- Capture-helper error envelopes are often `{"ok":false,"error":"…"}` and
  need not repeat the success `schema_version`.
- `bob capture -f json` is unversioned (`ok`, no `schema_version`).
- `bob vault-sync status -j` prints the status record directly, with no `ok`
  and no `schema_version`.
- A failing command may print JSON, a human message on stderr, or both; see
  that command's contract.

The `bob capture-parse` envelope below is a verbatim captured line from the
isolated fixture. The `bob plan` illustration after it is a shortened excerpt
of the schema-3 keys from a synthetic vault with no daily note yet; `…` marks
omitted nested numbers.

```bash
bob capture-parse -f json -- 'Buy milk @dev'
```

```text { .no-copy title="Output" }
{"ok":true,"schema_version":1,"grammar":6,"input":"Buy milk @dev","body":"Buy milk","mode":"task","route":"dev","section":null,"block_id":null,"needs":[],"spans":[{"start":9,"end":13,"kind":"route"}],"diagnostics":[]}
```

```bash
bob plan -f json -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":3,"date":"2026-10-20","daily_file":"2026/20261020.md","status":"ok","caps":{"max_links":…,"max_next":…,"max_pending":…,"max_ready":…},"today":{"count":0},"today_tasks":[],"today_roots":[],"links":{"count":0,"cap":…,"over":false},"next":{"count":0,"cap":…,"over":false},"pending":{"count":0,"cap":…,"over":false},"theme_names":[],"entries":[],"warnings":[]}
```

`today_roots` lists the root parents of Today's tasks as
`{"key","label","hue","tasks"}` rows (`hue` is null for `@unfiled`); the
`tasks` sum to `today.count`. See [Today roots](../plan.md#today-roots).

`type_mismatch` warnings in `bob plan` carry additive `link` (`route`,
`block_id`), `entry` (`line`, `name`), `task_type`, and `entry_types`
beside the usual `code`, `message`, and ledger `line`. See
[Plan](../plan.md#lints).

## Capture and protocol helpers

`bob capture` writes the vault. Most `capture-*` helpers are read-only
previews, pickers, and discovery calls for the Mac app and editors.
`capture-task-id` and `capture-pomodoro-name` write by default (`-d` /
`--dry-run` plans without writing). `capture-templates save`, `delete`, and
`clear` mutate the saved-template shelf; `list` and `show` are read-only.

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob capture` | `-f json` | Item results plus batch `captures`, `warnings`, `plan_budget`, `pomodoro_blocks`, `task_blocks`, `open_notes`, `pull_in`, `inbox_route`, and batch-level `move_choice`; adjacent `@route+id` target runs add `fanout: {line, position, count, marker}` to the item and every member capture, while human output prints one `targets  <marker> · <marker>` line; project-note move bullets add `project_note.moved_tasks[]`, `move_summary`, `repaired_files[]`, and optional `move_hint`, while offer rows add `moved_from`; list bullets add per-item `list` (`kind`, `route`, `section`, `header_line`, `position`, `count`), and human output prints one `list  <target> · N tasks|notes` line before each list's first item; a lone `@route` item adds `kind: "note_pull"` and `note_pull: {note, route, successors, folded}`, while `@route` with bullets retains its list result; one-off labels add per-item `relation_label_hides`; cancel-label commits add per-item `relation_cancels[]` (one row per cancelled source), `relation_updates[].cancel_effect`, and warning `relation_cancelled_source`; starts add `pomodoro_start.off_clock` and deprecated `pomodoro_start.tracked: false` fields, additive `pomodoro_start.clock_note` / `pomodoro_close.clock_note` when every counted off-clock link inherited from the same note, `pomodoro_start.event_run` (its span is the buffered span; `events[]` gain `buffer` when the Event Task has a `[buffer::]`), and `pomodoro_blocks[].off_clock` plus deprecated `pomodoro_blocks[].tracked: false` fields, plus warning `pomodoro_event_run_later`; non-MEET starts, `+N` adjusts, and `++N`/`--N` shifts that newly conflict with an open Event Task's occupied span add `event_conflict` on `pomodoro_start`/`pomodoro_adjust`/`pomodoro_shift` (`event.off_clock`, deprecated `event.untracked`, `event.buffer`, and `buffer_part` appear as applicable) plus warning `pomodoro_runs_past_event`; non-MEET starts whose written start is strictly inside an open Event Task's meeting add `pomodoro_start.event_in_progress` plus warning `pomodoro_starts_during_event`; complete-label commits add per-item `relation_completes[]` (one row per completed source, embedding `task_complete`), `relation_updates[].complete_effect`, and warning `relation_completed_source`; sub-bullet items carry per-item `drawer: {title, key, created}` (`null` for a plain child), drawer refusals carry a JSON `code`, and drawer creation near a catalog name emits warning `drawer_near_catalog`; note-less `@route+id` multi-note items omit `sub_bullets` and add `notes[]` (each with `text`, `task_line`, optional `sub_bullets`, and `drawer`) plus `drawers[]` in first-touch order, with `drawer` set only when one drawer received every note; clipboard-notes items report empty `text`/`task_line` with notes in `clip.lines` at the task's child level; transfer items use kind `task_move` (bare-note destination) or `task_merge` (task, drawer, or section destination) with `moves[]` or `merges[]` plus `repaired_files[]` (ordinary items omit the empty fields), and human output prints `↪ Moved N tasks into <note> › Tasks` or `⤵ merged N tasks into “<target>”` lines; a MEET close that carries later meetings with an open Pomodoro below adds `pomodoro_close.next_pomodoro.placed_last: true` (omitted otherwise) | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md), [cancel labels](../task-relations.md#17-cancel-labels), [complete labels](../task-relations.md#18-complete-labels) |
| `bob capture-parse` | `-f json` | Per-item `mode`, spans, diagnostics, and `needs` for incomplete input; top-level `grammar: 11` identifies the capture-language revision. Multi-target runs add `targets: [{start, end, marker, mode, route, block_id, section}, ...]` only for two or more members; legacy fields describe the first member, `needs` is the ordered union, and member spans retain absolute ranges. `duplicate_capture_target` and `split_capture_targets` are coded diagnostics. A complete `>` transfer reports mode `task_move` or `task_merge` with a `transfer` object (`sources[]`, `arrow`, `destination` with `kind: note|task|drawer|section`), `transfer_source`/`transfer_arrow`/`transfer_destination` spans, and the legacy `route`/`block_id` describing the destination; partial transfers need `transfer_destination`, `block_id`, `drawer`, or `section`, and malformed transfers carry `transfer_*` diagnostics. Move bullets add `sub_bullet_moves[]` and the `project_task_move_marker`, `project_task_move_link_marker`, and `project_task_move_block_id` spans; unfinished bullets need `project_task_move`, and lexical diagnostics use `move_bullet_*` codes. A `^^` choice adds `project_task_move_choice_marker` and `sub_bullet_move_choices[]` (null or `{range}` entries beside `sub_bullet_moves[]`), leaves the parse complete, and advances the grammar to 11. A lone `@route` reports mode `note_pull`, `needs: []`, no diagnostics, route spans, and the ordinary `list` summary (`kind: "tasks"`, `count: 0`, `header_line`, `header_range`); `@route` with bullets remains a list. Other list drafts report one `items[]` entry per bullet with per-item `list` and `header_range`; a section header with no bullets is `incomplete` needing `list_item`. List diagnostics carry `code` (`empty_list`, `list_header_marker`, `list_bullet_marker`, `list_bullet_checkbox`, `template_list`, `misplaced_project_task_id`, `invalid_project_task_id`, `duplicate_project_task_id`). An unterminated `@route#"…` section is `incomplete` needing `section` with no diagnostic; `bob capture` refuses it as `incomplete_section`. Relation-label spans carry `cancels_source` or `completes_source` when configured. Drawer spans carry `title` and `key` when the name is valid; a first-level `#` shortcut is a `drawer` span, and an ALL-CAPS drawer bullet with nested notes is a `drawer_bullet` span; incomplete shortcuts need `drawer` or `drawer_notes`. Drawer diagnostics carry `code` (`invalid_drawer`, `drawer_text`, `drawer_bullet_text`, `drawer_bullet_position`, `empty_drawer_bullet`, `multi_note_marker`, `ambiguous_drawer`, `reserved_drawer`, `drawer_near_duplicate`, `drawer_toggle`) | `schema_version: 1` | [Capture](../capture.md), [cancel labels](../task-relations.md#17-cancel-labels), [complete labels](../task-relations.md#18-complete-labels) |
| `bob capture-complete` | `-f json` | Completion context plus candidates and replacement ranges; top-level `grammar: 11` identifies the capture-language revision. The `project_task_move` context adds parent/destination metadata, a note-scoped picker descriptor with continuation, and movable parent-task rows including ID-less suggestions, today's Pomodoro chips, consequences, schedules, hidden state, and disabled reasons. At a lone `- ^` with an empty query, its descriptor adds `action_continuation_keys: ["^"]`. Bare `#` and `%` menus, an empty drawer query after `@route+id#`, and an empty `=*` merge source add `initial_selection: "none"`. A merge source resolves `current` to an unambiguous running or next planned Pomodoro; destinations list the next mergeable planned Pomodoro below the source as `next planned`. Drawer menus add `target: {route, block_id, text}`, the decoded `query`, and rows grouped `task`, `recent`, and `new`; each row carries Bob-authored `detail` and `consequence`, `creates`, an optional commit-derived `refused` reason, and a `replacement` that covers the `#` and the name, with `cursor_after`. At a first-level bullet start the same menu uses the bare ALL-CAPS title as `replacement` (`TITLE\n  - ` when it also opens the nested bullet, so `cursor_after` lands on it), and `target.block_id` is `null` for a new task (`"prj"` for a new project note). `section` rows keep `title` as the display heading and set `replacement` to a selector that resolves back to that heading (raw title, hyphen slug, or quoted `"Title"`). `project_task_block_id` accepts task-list bullets and nested lines as well as project-note task bullets and nested lines; `block_id.owner_line` and `block_id.splice` locate the owning line and preserve surrounding draft text. List prompts merge destination-note IDs and sibling-list IDs in `block_id.used`. `relation_label` rows carry `cancels_source`, `completes_source`, and `configured`; the response may include `relation_cancel` and/or `relation_complete` when configured. On a `>` transfer line, contexts serve each component: source `task` rows list movable/mergeable tasks; merge-target `task` rows drop the typed sources and add a Bob-authored `consequence` (`⤵ merge into …`); at `> +`, `task_parent` rows exclude sources and ID-less tasks and mark merges; `section` rows offer creation for unmatched names, while a near-duplicate warns instead of creating; and a complete bare destination adds the `+` hint to `warnings` | `schema_version: 1` | [Capture](../capture.md), [cancel labels](../task-relations.md#17-cancel-labels), [complete labels](../task-relations.md#18-complete-labels) |
| `bob capture-pomodoro-name` | `-f json` | Name write (or `-d` plan) for one open Pomodoro, plus optional `warnings` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-pomodoros` | `-f json` | Ledger entries plus `warnings`; `--tasks` adds task agendas, optional `events` calendar rows/hints, event chips, `off_clock` and deprecated `untracked` fields plus `clock_source` (`on_tag`\|`off_tag`\|`note`\|`invalid_note`\|`default`) and `clock_note` (deciding note path or null), completed off-clock totals, and buffer objects with occupied spans on buffered Event Tasks | `schema_version: 1` | [Capture](../capture.md), [Pomodoro homes](../pomodoro-homes.md) |
| `bob capture-related` | `-f json` | Candidate matches, UTF-8 draft anchors, verified actions, and already-linked tasks; `--query` adds scores | `schema_version: 1` | [Capture](../capture.md#bob-capture-related) |
| `bob capture-rewrite` | `-f json` | `input`, rewritten `text`, `changed`, `cursor`, `rule`, and `edits` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-sections` | `-f json` | `route`, `count`, and non-`Tasks` `sections` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-targets` | `-f json` | `bob_dir`, `count`, and route `targets` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-task-id` | `-f json` | ID write (or `-d` plan) plus `note_path` and picker `*_replacement` strings | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-drawers` | `-f json` | ALL-CAPS child drawers of one task plus catalog recents | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-tasks` | `-f json` | Open tasks of one note with block IDs and status names | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-templates list` | `-f json` | Saved-template shelf entries | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates save` | `-f json` | Saved definition plus shelf position | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates show` | `-f json` | One saved definition | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates delete` | `-f json` | Removed entries plus remaining count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates clear` | `-f json` | Cleared count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |

For `bob capture-complete` in `relation_label` context, visible catalog rows include
`label`, normalized `key`, `count`, `replacement`, and optional `inverse`. A nonempty
query adds a typed row first, with `typed: true`, `existing`, and `hidden`; that row
may also carry `inverse` from the §10 rule, including for a brand-new label.
When its key exists, it also includes `count` and uses the catalog spelling.
Hidden labels are omitted from
the visible rows, but an exact typed hidden label remains available with `hidden: true`.
The response includes `hidden_count` for this context. Rows also carry
`hide_replacement` for the `||` spelling; a `||` probe preserves both pipes
in each row's `replacement` and adds `one_off: true` to the response.

### Session edits and successor offers

Capture results are unversioned, including these per-item session payloads:

| Operation | Payload |
| --- | --- |
| Named task landing | Destination endpoint `merged: {before, after, added}` when the destination gains the task's type; `task_theme` is omitted unless an explicit type marker reports its result or a MEET landing changes the task's type |
| `=/` split | `kind: "pomodoro_split"`, with `pomodoro_split: {path, entry_line, before, after, base, created, stayed_links}`; `created[]` contains `type`, `line`, `name`, and `moved_links` |
| `=*` merge | `kind: "pomodoro_merge"`, with `pomodoro_merge: {path, entry_line, before, after, source_role, explicit?, absorbed}`; `source_role` is `current`, `next_up`, or `named`, `explicit` is the equivalent fully named token when an implied choice round-trips, and `absorbed[]` contains `name`, `role` (`named` or `next`), `from_line`, `moved_lines`, and `dropped_duplicates` |

Both split and merge return batch-level `pomodoro_blocks` for affected entries.
Surviving entries show line-level changes, including removed lines. Removed
entries carry `removed: true` and their previous lines marked `change: "removed"`. Parse reports modes
`pomodoro_split` and `pomodoro_merge`; a dest-less merge is complete, while
a trailing `#` still needs `pomodoro_merge_dest`.
See [split](../capture.md#splitting-a-merged-pomodoro) and
[merge](../capture.md#merging-pomodoros) for selection and refusal rules.

Managed Done results can carry `keep_moving` offers beside `unblocked`, with
`keep_moving_folds` counting hidden, blocked, and future-scheduled candidates.
These offers are eligible Pending, Next, and Ready tasks from a leaf project
whose last task linked in today's open Pomodoros was closed. A leaf project
is a subproject with a different root parent; it may itself have subprojects.
These offers do not require a dependency on the completed task. They join
the batch's `pull_in` choices and link only when picked.
See [keep-moving offers](../task-dependencies.md#1215-keep-moving-offers-leaf-projects).

Relation successors are tasks whose configured successor relation (such as
`starts after`) points to a task just completed or parked Pending with no
link left under today's open Pomodoros. They use the same explicit selection
flow. The additive fields are:

| Location | Fields |
| --- | --- |
| `task_complete`, `pomodoro_close`, nested `relation_completes[].task_complete`, or a Pending-unlink capture item | `relation_successors[]` and `relation_held[]` are omitted when empty; `relation_check` (`checked` or `unavailable`) is present only when the lookup ran |
| A note-pull offered row | `offer.note` is the requested source, matching `pull_in.groups[].note`; `note_path` is the final home after a later move |
| An offered row with a relation reason | `relation: {label, key, event, after}`; `event` is `parked` or `done`, and `after[]` identifies the triggering tasks with `note_path`, `block_id`, and `text`. A relation-group row also has `offer.after`, a `note.md#^id` string |
| A held relation row | Task identity, the relation, and `reason`: `already_planned`, `already_started`, `blocked`, `hidden`, or `waits_for` |
| Batch `pull_in` | `supported`, `offer_count`, `choice` (`null`, `all`, `none`, or `subset`), `picked_count`; optional `groups[]` and `copy: {title, capsule, glyph}` |
| `pull_in.groups[]` | `group`, optional `project` or note-pull `note`, `copy: {title, why?, tag?, fold?, glyph}`, and optional `folded` counts; relation groups also carry `after: {key, note_path, block_id, text}`, `label`, and `event` |

Group order follows offer numbering: hand-offs, relations, projects,
finished projects, then keep-moving, then note pulls in draft-item order. Clients use Bob's supplied display text
in `copy`.
`offer_count` counts selectable rows, including relation successors and note-pull rows;
held rows are not selectable. Empty `groups` and absent copy are omitted.
Note-pull empty refusals use `nothing_to_pull_in` with the `note_pull` fold payload; missing source notes use `pull_note_missing`. The flat `offers` list appears on a `pull_in_choice_required` refusal;
successful results keep the rows on their owning capture items.

These are the final batch's offers: later items can remove an offer by
linking, completing, blocking, or dropping its task, or by re-linking a
parked predecessor. A picked relation successor appears in `task_blocks`
with `relation_successor` in `roles[]`; its added Pomodoro line has
`reason: "relation"`. See the
[successor JSON contract](../task-dependencies.md#125-reporting-model)
and [relation events](../task-dependencies.md#1218-relation-successors-starts-after).

### Move-choice results

Every `bob capture -f json` success and refusal returns a top-level `move_choice`
object. It always has `supported: true`, which clients use for capability
detection; when the draft has a `- ^^` slot, it also has `slots[]`. This is
additive and leaves capture's unversioned JSON shape unchanged. A dry run
without `-m` reports the same rows with
`answered: false`, no choices applied, and `ok: true`.

A v1 draft has at most one choice slot; `slots[]` keeps the batch-level shape
additive for later versions. A slot is built from the staged parent, so earlier
capture items are visible, and reads today's daily note and the link-guard
inputs needed for its consequences. It includes `item`, the 1-based `item_line`,
the `draft_range` as a half-open UTF-8 byte pair for `^^`, `parent`,
`destination`, optional `pomodoro_name`,
`name_needs_link`, `open_count`, `answered`, `moved_count`, `linked_count`,
`copy`, and `rows[]`. The copy object has Bob-authored `title`, `subtitle`,
`slot`, `leave`, `ghost`, `band`, and optional `then` strings:

```json
{
  "move_choice": {
    "supported": true,
    "slots": [{
      "item": 1,
      "item_line": 4,
      "draft_range": [112, 114],
      "parent": "cash.md",
      "destination": "cash_goog_exit.md",
      "pomodoro_name": "ADMIN",
      "name_needs_link": false,
      "open_count": 25,
      "answered": false,
      "moved_count": 0,
      "linked_count": 0,
      "copy": {
        "title": "Move into cash_goog_exit.md",
        "subtitle": "from cash.md · 25 open",
        "slot": "Lands between “Pay the credit card” and “Book the lawyer”, in cash.md order",
        "leave": "Stays in cash.md",
        "ghost": "choose from cash.md's 25 open tasks on ↩",
        "band": "Asks about cash.md's 25 open tasks on ↩",
        "then": null
      },
      "rows": [{
        "key": "cash.md#^call-ms",
        "n": 2,
        "line": 12,
        "section": "Tasks",
        "block_id": "call-ms",
        "mint": null,
        "text": "Call Morgan Stanley about the 401k",
        "status_symbol": " ",
        "status_name": "Ready",
        "hidden": false,
        "scheduled": null,
        "pomodoro": null,
        "choices": [
          {"sigil": "^", "consequence": "Moves into cash_goog_exit.md", "adds_link": false},
          {"sigil": ":", "consequence": "Moves in · becomes Next · links under ADMIN", "adds_link": true}
        ],
        "locked": null,
        "disabled_reason": null,
        "answer": null
      }]
    }]
  }
}
```

Rows come from the shared movable-root eligibility in parent document order.
They include ID-less and `#hide` roots; `^prj`, closed and nested tasks, and
the project template placeholder are excluded. `open_count` is the number of
listed movable roots. `n` is the 1-based position among all rows, including
rows that are locked or disabled, so answer numbers never shift around them.
An identified row's key is `path#^id`; an ID-less
row uses `path@<sha8>[~k]`. `mint` is Bob's collision-free suggested block ID
for an ID-less row and is null otherwise. It avoids the parent's IDs, IDs
authored in the new note, `prj`, and every other row's suggestion. At execution,
Bob uses that suggestion if still free or chooses the next free suffix, also
avoiding descendants of moved blocks; a collision with an imported descendant
refuses with `move_task_id_collision`. The key is stable for the row and is the
preferred answer form for editor clients.

Each enabled, unlocked row has `^` and `:` choices with a Bob-authored
`consequence` and `adds_link`. Disabled and locked rows have no choices.
`locked` is null or `{sigil, item_line}` when an explicit move in this draft
already moves that row from the same parent. `disabled_reason` is Bob-authored.
A row also carries its task text, status, section, source line, hidden and
scheduled state, and today's Pomodoro chip (or null). After an answer, the slot
has `answered: true`, the moved and
linked counts, and every row's `answer` as `^`, `:`, or null. Dry-run JSON
matches the real-run JSON apart from `dry_run`.

`name_needs_link` is true on the create form when the item has a `#name`, no
other `:` link exists, and that name needs a link; the offer form defers this
check. Under a `:` seal, `then` says `Then: choose today's links for N new
tasks` when offers remain; otherwise it is null. `copy.slot` names the
neighboring roots, or says `first` or `last` at an edge.

Repeatable `-m, --move-in` values union their selections. Bare numbers and
keys mean move (`^`); a leading `:` means move and link. `all` selects every
enabled, unlocked row as a move, and `none` selects no rows. There is no
`:all`. Numbers resolve against the rows rebuilt from the staged parent at
execution. Successful choice moves add `via: "choice"` to
`project_note.moved_tasks[]`, with `draft_range` set to the `^^` range; authored
moves retain `via: "bullet"`. Choice rows are never added to successor offers.

These write-free refusal codes run after existing Pomodoro-override, lexical,
and move-bullet refusals, in this order, before `pull_in_choice_required` and
`inbox_route_choice_required`:

| Code | Trigger |
| --- | --- |
| `move_choice_unasked` | `-m` is supplied but the draft has no `- ^^` |
| `move_choice_empty` | No selectable rows; a dry run instead reports a warning and stays `ok: true` |
| `move_choice_required` | A real run has `- ^^` but no `-m` answer |
| `move_choice_conflict` | Duplicate row answers conflict, or `none`/`all` is combined with other picks |
| `move_choice_stale` | A number or key no longer resolves; the refusal carries fresh rows |
| `move_choice_refused` | The answer selects a locked or disabled row |
| `unused_project_note_pomodoro` | A required `#name` has no link after applying the answers |

Exit codes follow the corresponding pull-in and inbox-route refusals. The
human unanswered preview prints every row, even above 20 rows, with its number,
status, title, ID or suggested ID, and consequence or lock/disabled reason,
then a `choose with -m ...` hint.

### Inbox-routing results

Successful captures, including dry runs, return `inbox_route` with
`supported: true`, `candidate_count`, `routed_count`, `kept_count`, and
`candidates`. Each candidate is an existing top-level inbox task block whose
root or nested task changes status; new tasks are excluded. Each has an opaque
`key`, a 1-based `n`, the root's before/after status, an `answer` (initially
`null`), and an `outcome` (`pending`, `kept`, or `moved`).
`destinations` is present only when candidates exist.

Repeatable `--inbox-route` answers select a destination or keep a candidate
in place. A moved candidate gains `moved` with `destination`, `line`,
`block_id`, `previous_block_id`, and `fresh` (a date or `null`). The
`inbox_route` object adds `repaired_files`, `renamed_block_ids`, and
`needs_reconcile`; top-level `task_blocks` and `open_notes` follow the move.
Without an answer or `--require-inbox-route-choice`, a capture succeeds
with pending candidates left in their inbox. See the
[inbox-routing contract](../capture.md#routing-inbox-tasks-out) for answer
syntax, guard errors, and atomic-write guarantees.

### Similar-task draft results

`bob capture-related -f json -- DRAFT` returns one `items` entry per
task-creating draft item. `index` is the item's 0-based position in the full draft,
including items that do not create tasks. Templates and drafts without new
tasks return `items: []`.

| Field | Wire representation |
| --- | --- |
| `range` | Half-open `{start, end}` UTF-8 byte offsets for the item in the submitted draft |
| `route_range` | The item's own destination-token range; omitted when absent |
| `discard_range` | Range for removing the item and adjacent separator; omitted when the item has authored children |
| `marker_at` | UTF-8 byte offset for inserting an action marker |
| `matches[].matched_ranges` | Array of half-open `[start, end]` UTF-8 byte pairs in that match's clean `title` |
| `matches[].band` | `same_title`, `possible_duplicate`, `similar`, `closed_duplicate`, or `hold` |
| `matches[].actions`, `matches[].default_action` | Available actions and the recommended default; clients apply draft edits only after user selection |
| `matches[].replacements` | Present for targets with block IDs; ID-less targets use `requires_block_id` and `block_id_suggestions` |
| `linked` | Keys already referenced by the item, with their link kind |

A candidate appears under only the item with its highest match score; ties
go to the earliest eligible item. Tasks explicitly linked by any task-creating
item are excluded from every item's matches. Non-exact matches need at least
two distinct meaningful terms. A `hold` row is below the display threshold;
clients use it only to retain an already-visible suggestion while wording
changes. Bob can return it even if the client has never displayed it.
Draft results omit scores. `--query TEXT` is a separate
calibration shape with scores and without draft anchors or actions.

`route_range` and `discard_range` use the same object shape as `range`.
Draft offsets are valid only for the exact submitted text; match offsets
index the returned `title`. End offsets are exclusive. The full
[capture-related contract](../capture.md#bob-capture-related) owns actions,
candidate eligibility, and error behavior.

## Plan, readiness, freshness, and stats

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob plan` | `-f json` | Date, status, caps, Today tasks and root counts, lane counts, and `warnings` | `schema_version: 3` | [Plan](../plan.md) |
| `bob ready` | `-f json` | Per-note Ready lane against `max_ready_per_note` | `schema_version: 1` | [Plan](../plan.md) |
| `bob freshness` and `bob freshness list` | `-f json` | Review queue, `config.decay.limits: {pending, next, ready}` (integers or `false`), and per-row `keeps` / `decide` | `schema_version: 15` | [Freshness](../freshness.md) |
| `bob freshness seed` | `-f json` | Seed dry-run report: stamped counts, buckets, skipped | `schema_version: 15` | [Freshness](../freshness.md) |
| `bob stats overview` | `-f json` | Weekly glance report | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats show` | `-f json` | One or more metric details | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats metrics` | `-f json` | Metric id, kind, and definition catalog | `schema_version: 1` | [Stats](../stats.md) |

Do not copy the freshness `15` onto stored files: it is the command
envelope version. The freshness contract owns the stored review state.

`summary.headline.display` is the formatted headline; optional `suffix` is
its display suffix, and optional `stat` identifies the calculation. For `net`,
`value` sums complete, non-null daily values, excluding provisional today.
With at least one such value, `stat` and `suffix` are both `"total"`;
otherwise `value` is `null`, `display` is `"–"`, and both fields are omitted.
When present, the net delta says `shrinking`, `growing`, or `flat` from the
total's sign. Other numeric headlines use daily medians; ratios pool
numerators and denominators, and booleans count met days over days with data.
The envelope remains schema 1; see
[Stats summaries](../stats.md#ranges-grouping-summaries-display).

Stats reports add a top-level `clock` object (`value`, `label`, `since`, and
the three `options` with window minutes) for the `-k worked|off-clock|total`
lens, plus `off_clock` sub-shares on Pomodoro points and parts, `off_clock`
on `sessions`, and clock diagnostics under `health.homes`. Overlap details
are `{date, lines, minutes}` with optional `paths` on a cross-note pair;
see [Clocks](../stats.md#clocks) and
[Health](../stats.md#data-model-and-quality). The CSV form ends with a
`clock` column.

An unmet boolean for provisional today has `value: 0`, `display: "not yet"`,
and `provisional: true`; a completed day can display `missed`. The Obsidian
Day strip is a plugin view, not a `day_strip` field in CLI JSON. See
[Daily Stats](../stats.md).

## Task types and relation labels

Bare `bob types` and `bob relations` run their `list` command. Lists read
the vault and may update disposable local caches. Hide, unhide, and color
commands write preference journals; rename, split, and migrate preview with `-d` and
otherwise rewrite matching vault content in a verified transaction.

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob types list` | `-f json` | `today`, `window_days`, `counts`, `themes`, hue-collision `hints`, `renamed_reappeared`, and `warnings`; no success `ok` field | `schema_version: 1` | [Task types](../task-types.md#json-contract) |
| `bob types hide`, `bob types unhide` | `-f json` | `ok`, `hidden` or `unhidden` names, and `via` | `schema_version: 1` | [Task types](../task-types.md#json-contract) |
| `bob types color` | `-f json` | `ok`, `name`, `hue`, `pinned`, and `via`; `auto` returns `hue: null` and `pinned: false` | `schema_version: 1` | [Task types](../task-types.md#json-contract) |
| `bob types migrate` | `-f json` | `fields`, `defaults`, `files` (`{path, fields, defaults}`), `needs_repair`, `conflicts`; writes add `mode`, `commit`, and `sync` | `schema_version: 1` | [Migrating theme spellings](../task-types.md#migrating-theme-spellings) |
| `bob types rename` | `-f json` | `mode`, `changed`, `from`, `to`, `merge`, affected `files` and `entries`, `state`, `color`, `commit`, `sync`, and `warnings` | `schema_version: 1` | [Global rename](../task-types.md#pt10-global-rename) |
| `bob types split` | `-f json` | template: `mode: "template"`, `theme`, `entries`; preview/write: `mode`, `theme`, `targets` (`new`, `exempt`), `kept`, `unmapped`, `remaining`, `files` with `{line, ref, before, after}`, `commit`, `sync`, and `warnings` | `schema_version: 1` | [Split by entry](../task-types.md#pt11-split-by-entry) |
| `bob relations list` | `-f json` | `ok`, `counts`, `labels` with spelling variants and replacement strings, `renamed_reappeared`, and `warnings`. Rows carry `cancels_source`, `completes_source`, and `configured`; `-a` includes zero-use configured labels. Top-level `cancel_labels` / `complete_labels` count configured effects | `schema_version: 1` | [Relation vocabulary](../task-relations.md#rv7-contracts), [cancel labels](../task-relations.md#17-cancel-labels), [complete labels](../task-relations.md#18-complete-labels) |
| `bob relations hide`, `bob relations unhide` | `-f json` | `ok`, `hidden` or `unhidden` names, and `via` | `schema_version: 1` | [Relation vocabulary](../task-relations.md#rv7-contracts) |
| `bob attachments import` | `-f json` | `attachments` (`{source, display, saved, kind, reused, embed, sha256}`); failures print the relations-hide error envelope with codes `missing`, `not_a_file`, `not_absolute`, `too_many`, `lock_busy`, and `io` | `schema_version: 1` | [Attachment origins](../attachments.md#bob-attachments) |
| `bob attachments origin` | `-f json` | `path`, `origin` (`{source, display, host, ts, via, state}`) or `null`, and `warnings` | `schema_version: 1` | [Attachment origins](../attachments.md#resolution) |
| `bob relations rename` | `-f json` | `mode`, `changed`, `from`, `to`, `merge`, affected `files` (`{path, links, mirrors, lines: [{line, kind, before, after}]}`), `counts` (`{links, notes, mirrors, done_links, done_notes}`), `state`, `inverse`, replacement strings, `commit` (`{sha, subject}`), `sync` (`{pre, post}`), and `warnings`. With `-r/--reverse`, adds `reversed: true`, `moves[]` (`{source: {path, line, block_id, text}, target: {path, line, block_id, text}, from_label, to_label, action: "moved"\|"merged"}`, one-based task lines, minted IDs included), and `minted_block_ids[]` (`{path, line, block_id}`) | `schema_version: 1` | [Rename transaction](../task-relations.md#rv8-the-rename-transaction), [Reversing a label](../task-relations.md#rv10-reversing-a-label) |

Theme colors also appear in existing reports as `{key, name, hue, hue_source}`
objects: `themes` on Pomodoro-name parse spans, completion rows, and ledger
entries; `theme_refs` beside plan `theme_names`; and `added_theme_refs` in
capture's `plan_budget`. Stats supplies `theme_hues` keyed by normalized
theme key. These additions retain each command's existing envelope version;
`bob plan` uses schema 3 and capture remains unversioned.

## Tasks, reconciliation, and queries

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob task move` | `-f json` | Move plan and repaired-link report | `schema_version: 1` | [Task move](../task-move.md) |
| `bob task reconcile` (aliases `task-status-hooks`, `mark-next-tasks`) | `-f json` | Sync result: marked, cleared, blocked, dependency, and reference changes | Unversioned (serialized sync result, no `schema_version`) | [Task Status Hooks](../task-status-hooks.md) |
| `bob task reroll` (alias `randomize`) | `-f json` | Re-roll plan and per-task outcomes | `schema_version: 1` | [Randomize](../randomize.md) |
| `bob query` | `-f json` (default is `paths`) | `engine`, `query_kind`, `format`, `paths`, `result`, and `warnings` | Unversioned (no `schema_version`) | [Bob query](../dataview.md) |
| `bob articles format` | `-f json` | Article plans | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles list` | `-f json` | Article notes needing work | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles outline` | `-f json` | Section outline of one note | `schema_version: 1` | [Articles](../articles.md) |

`bob task archive`, `bob projects`, and `bob nightly` are human-only in this
checkout: they expose no `--format json` or `--json` flag.

## References, jobs, Keep, plugins, and status

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob ref find` | `-f json` | Library verdicts and title candidates | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref list` | `-f json` | Notes by reading state with dates and parents, plus additive `source_markdown` pointer when saved | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref show` | `-f json` | One note: reading state, dates, annotation pointers, additive `source_markdown` pointer, and an additive `sections` list (omitted when the region has none; `schema_version` stays 1) | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref code` | `-f json` | One reference's saved source and render-time code catalog, optional block/page filters, and a status (`available`, `no_code`, `no_catalog`, `no_source`, `source_missing`, `source_invalid`) | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md#bob-ref-code) |
| `bob ref links` | `-f json` | One reference's recorded URI links with bob-computed `label`, `site`, and `markdown`, optional link/page filters, and a status (`available`, `no_links`, `not_recorded`, `no_catalog`, `no_source`, `source_missing`, `source_invalid`) | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md#bob-ref-links) |
| `bob ref scan` | `-f json` | Scan report: `mode`, `write_pdfs`, `hook`, `intake`, `summary` (with `held`), `notes` (with `marker_write`, including `restored` when a stale save was restored), `failures`, `intake_only`, `queued`, `held`. Every writing scan that holds the writer lock also writes that exact line to `<state>/bob-cli/ref/last-scan.json` and (macOS) posts `org.bobs.bob.ref.scan-finished`. `--intake-only` files queued `xlib/` PDFs and audio only (no annotation pass); `ok` is then also false when a writing drain leaves `queued` nonempty. Hard failures use the error envelope (`error.code` / `hint` / `paths`) instead of `summary`/`notes`/`failures`, with no `held` field. | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [JSON output](../highlights-ref-sync.md#json-output) |
| `bob ref today` | `-f json` | Pulls one reference's reading task into a Pomodoro (`mode` `dry_run` or `write`; `-d` plans without writing). Success carries `changed`, `ref`, `task`, `pomodoros`, and `capture`. This is not a listing of today's references. | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [bob ref today](../ref.md#bob-ref-today) |
| `bob ref rename` | `-f json` | Rename plan and rewritten-link report | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref migrate-tasks` | `-f json` | Migration plan for open ref tasks | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref migrate-zorg` | `-f json` | Zorg-record migration plan | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref jobs list` | `-f json` | Queued background jobs plus a `summary` | `schema_version: 1` | [Ref jobs](../ref-jobs.md) |
| `bob ref jobs run` | Human only | Clips pending jobs through the shared ingest | No JSON flag | [Ref jobs](../ref-jobs.md) |
| `bob gcal calendars` | `-f json` (default is `table`) | Calendars with id, name, access role, colour, sync flag, and mapped root | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal` and `bob gcal list` | `-f json` (default is `table`); `-a` adds omitted events | Local-day event snapshot with RSVP, inclusion, selected root, event clock (`clock`, `clock_source`, `clock_note`), event buffer, description bullet counts, warnings, deprecations, vault state, and last sync | `schema_version: 1`, `day`, `calendars`, `events`, `counts`, `last_sync`, `past_open`, `warnings`, `deprecations` | [Bob Gcal](../gcal.md) |
| `bob gcal doctor` | `-f json` | gog, client, account, token, calendars, default root, today, journal, writer, and last sync checks (`id`, `status`, `summary`, `fix`) | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal sync` | `-f json` (default is `human`); `-d` previews without writing; `-q` silences success chatter while warnings stay visible | One day's Events rows and MEET links: per-row actions with buffer and description bullet counts, the MEET Pomodoro entries, the Events stamp, warnings, and Google clock deprecations | `schema_version: 1`, `ok`, `day`, `path`, `status`, `rows`, `pomodoro`, `warnings`, `deprecations`, `stamp` | [Bob Gcal](../gcal.md#sync) |
| `bob gcal login` | `-f json` | One-time setup report: `already_connected`, `account`, `client`, calendar counts, `sign_in` (`already_connected`, `browser`, `paste`), `client_source` (`stored`, `file`, `gog_client`, `downloads`, `pasted`), `client_project`, `reconnected` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal logout` | `-f json` | Removed grant: `account`, `client`, `revoke_url` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gkeep` and `bob gkeep list` | `-f json` (default is `table`) | Keep inbox notes with counts and error hints | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep doctor` | `-f json` | Adapter and credential diagnosis | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-markers` | `-f json` | Marker-removal plan | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-tasks` | `-f json` | Link-repair plan for open tasks | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep pull` | `-f json` | Inbox import report | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep status` | `-f json` (default is `human`) | Cached Keep pull count, check state, and open inbox count | `schema_version: 1` | [Bob Gkeep](../gkeep.md#states-and-the-json-document) |
| `bob plugins` and `bob plugins list` | `-f json` (default is `table`) | Repo plugins with version, sync, vault state, and optional deployment provenance | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob plugins sync` | `-f json` | Copied, skipped, and unchanged files with backups and optional deployment provenance | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob vault-sync status` | `--json` / `-j` | Direct `StatusRecord`: `last_attempt_at`, `last_success_at`, `local_sha`, `remote_sha`, `files_committed`, `push_retries`, `duration_ms`, `conflicts`, `interrupted_merge_recovered`, `aborted_merge_head`, `halted_merge_head`, `interrupted_fast_forward_recovered`, `last_error`, `clobbers`, `recent_clobbers` | Unversioned (no `ok`, no `schema_version`) | [Vault Git sync](../vault-git-sync.md) |
| `bob completion status` | `--json` / `-j` | Protocol, executable, and per-shell install state | `schema_version: 1`, `protocol: 1` | [Completion](../completion.md) |

## Other structured formats

These are public but are not JSON. They use their own flags on the same
commands.

| Format | Command and flag | Shape |
| --- | --- | --- |
| CSV | `bob stats overview -f csv`, `bob stats show -f csv` | Same report rows as JSON, comma-separated |
| TSV | `bob ref migrate-tasks -f tsv` | Migration plan rows for spreadsheets |
| Table | `bob plugins -f table`, `bob gkeep -f table`, `bob gcal calendars -f table` | Human tables; the default for those families |
| Markdown | `bob query -f markdown`, `bob ref find -f markdown`, `bob ref list -f markdown`, `bob ref show -f markdown` | Rendered notes output |
| Paths | `bob query` default (`-f paths`) | Newline-separated vault paths |

Hidden adapters (Keep, web-clip, Dataview internals) and on-disk state files
(`.bob/`, `templates.json`, the Stats review journal, ref job spool files)
are not public command outputs even when they contain JSON.

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Capture syntax](capture-syntax.md) for the tokens these envelopes describe.
- [CLI reference](cli/index.md) for the exact flags of every command.
- [Configuration and environment](configuration.md) for the `BOB_*` names that
  select vaults, adapters, and plugin roots used above.
