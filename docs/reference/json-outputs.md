---
title: JSON outputs
description: Every bob machine-readable output, its flag, envelope shape, schema version, and contract link.
---

# JSON outputs

Fixtures use this checkout's `bob`, isolated `HOME`/`XDG_*`, a synthetic
vault, `BOB_NOW=2026-10-20 12:00:00`, `TZ=UTC`, `BOB_REF_JOBS_KICK=off`, and
`BOB_GMAIL_KICK=off`; network, clipboard, sync, and hooks are stubbed. Stored
freshness/review versions differ; only stdout `schema_version` counts.

## Framing

JSON reports print to stdout. Warnings, progress, and hook chatter print to
stderr so a JSON pipe stays parseable. Usage errors exit 2.

Envelopes vary by command family:

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

Examples show selected fields; `…` marks omissions. The `bob plan` vault
has no daily note.

```bash
bob capture-parse -f json -- 'Buy milk @dev'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":25,"input":"Buy milk @dev","body":"Buy milk","mode":"task","route":"dev","section":null,"block_id":null,"needs":[],"spans":[{"start":9,"end":13,"kind":"route"}],"diagnostics":[]}
```

```bash
bob plan -f json -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":3,"date":"2026-10-20","daily_file":"2026/20261020.md","status":"ok","caps":{"max_links":…,"max_next":…,"max_pending":…,"max_ready":…},"today":{"count":0},"today_tasks":[],"today_roots":[],"links":{"count":0,"cap":…,"over":false,"off_clock":0},"next":{"count":0,"cap":…,"over":false},"pending":{"count":0,"cap":…,"over":false},"theme_names":[],"entries":[],"warnings":[]}
```

`today_roots` tasks sum to `today.count`; `hue` is null for `@unfiled`.
`type_mismatch` adds `link`, `entry`, `task_type`, and `entry_types`. See
[Plan](../plan.md#lints).

## Capture and protocol helpers

`bob capture` writes the vault; most `capture-*` helpers are read-only
except `capture-task-id`/`capture-pomodoro-name` (write; `-d` previews) and
`capture-templates save`/`delete`/`clear` (mutate the shelf).

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob clipboard` | `-f json` | Read-only selection and provider diagnostics; reports `would_succeed`, the selected provider and entry ages on success, or a refusal `code` and message. Clipboard and history text are never included | `schema_version: 1` | [Capture](../capture.md#clipboard) |
| `bob capture` | `-f json` | Item results and batch summaries for captures, warnings, budgets, Pomodoro/task/section blocks, open notes, routes, move choices, and files. Results include operation-specific data for lists, note pulls, project moves, transfers/merges, drawers, clipboard, relations, Pomodoro changes, and notebook notes (`kind: notebook_note` / `notebook_sub_note` / `notebook_create` plus a `notebook` object with `route`, `title`, `breadcrumb`, `section`, `section_action`, `block_id`, `parent_block_id`). Create items also add top-level `note_files[]` with `action: "create"`. | Unversioned (`ok`, no `schema_version`) | [Capture JSON](../capture.md#input-stdin-and-json-output), [task relations](../task-relations.md) |
| `bob capture-parse` | `-f json` | Per-item modes, spans, diagnostics, and needs (`grammar: 25`). Notebook routes add `notebook_note` / `notebook_sub_note` / `notebook_create`, shared route/section/task-block/sub-bullet span labels with `notebook: true`, and opaque bodies. Other modes cover Pomodoro timing, Gmail URLs, target runs, records, transfers, moves, lists, note pulls, sections, relations, drawers, and merge-ins. | `schema_version: 1` | [Capture](../capture.md), [cancel labels](../task-relations.md#17-cancel-labels), [complete labels](../task-relations.md#18-complete-labels) |
| `bob capture-complete` | `-f json` | Completion contexts, candidates, replacement ranges, picker state, selection, effects, and creation choices. Contexts cover routes, tasks, types, records, relations, moves, transfers, merge-ins, and past sessions. Notebook routes add 📓 route rows (breadcrumb label, summary detail), H2-only section rows with an additive `notebook_section_create` row, `destination_kind: "notebook"` on `@nb^`/`#section^` block-ID fields, `item_kind: "note"` on `@nb+` rows, and a `New notebook` `%n` row. `-n` walks required fields; `-F` adds record-value assistance. | `schema_version: 1` | [Capture completion](../capture.md#bob-capture-complete) |
| `bob capture-pomodoro-name` | `-f json` | Name write (or `-d` plan) for one open Pomodoro, plus optional `warnings` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-pomodoros` | `-f json` | Ledger entries, `warnings`, and `max_task_link_count`; Pomodoro rows include `start_token` when selectable and their slug is non-empty; `--tasks` adds task agendas, optional `events` calendar rows/hints, event chips, `off_clock` and deprecated `untracked` fields plus `clock_source` (`on_tag`\|`off_tag`\|`note`\|`invalid_note`\|`default`) and `clock_note` (deciding note path or null), completed off-clock totals, and buffer objects with occupied spans on buffered Event Tasks | `schema_version: 1` | [Capture](../capture.md), [Pomodoro homes](../pomodoro-homes.md) |
| `bob capture-related` | `-f json` | Similar tasks or same-folder records, UTF-8 draft anchors, actions, and already-linked tasks; `--query` ranks task titles with scores | `schema_version: 1` | [Capture](../capture.md#bob-capture-related) |
| `bob capture-rewrite` | `-f json` | `input`, rewritten `text`, `changed`, `cursor`, `rule` (`collapse_global_destination` for glued `@@route` triggers, plus the alias/absorption rules), `edits`, and optional forward `alias`; unchanged cursor requests add an object or `null` `sigil_alias_tip` | `schema_version: 1` | [Capture-rewrite contract](../capture.md#bob-capture-rewrite) |
| `bob capture-sections` | `-f json` | `route`, `count`, and non-`Tasks` `sections` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-targets` | `-f json` | `bob_dir`, `count`, and route `targets`; notebook rows use `kind: "notebook"` with `title`, `summary`, `owner`, and `breadcrumb`, record rows use `kind: "record"` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md#record-notes) |
| `bob capture-task-id` | `-f json` | ID write (or `-d` plan) plus `note_path` and picker `*_replacement` strings; notebook notes add `item_kind: "note"` and `parent_replacement: "@nb+ID"` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-drawers` | `-f json` | ALL-CAPS child drawers of one task plus catalog recents | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-tasks` | `-f json` | Open tasks of one note with block IDs and status names | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-templates list` | `-f json` | Saved-template shelf entries | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates save` | `-f json` | Saved definition plus shelf position | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates show` | `-f json` | One saved definition | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates delete` | `-f json` | Removed entries plus remaining count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates clear` | `-f json` | Cleared count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |

Capture item results optionally carry `gmail: [{url, verdict, thread_id?, note?, link?, date_hint?, message}]`. These offline previews contain no bodies; see [Gmail links](../capture.md#gmail-links).

### Record notes {#record-notes}

These excerpts use the [restaurant schema](../records.md#schema), including
required `status`, `category`, and `location`. Update examples assume
`eat/ninja_sushi.md` exists with `status: to_try`; section completion assumes
`eat.md` has a `# Ideas` heading.

```bash
bob capture-parse -f json -- 'Ramen Nagi @eat/'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":25,"input":"Ramen Nagi @eat/","body":"Ramen Nagi","mode":"record_note","route":"eat","needs":["record_value"],"record":{"action":"create","type":"restaurant","label":"Restaurant","dir":"eat","path":"eat/ramen_nagi.md","stem":"ramen_nagi","stem_source":"title","title":"Ramen Nagi","missing":["status","category","location"],"notes":0,"assist_version":1,"fields":[{"key":"status","kind":"choices","required":true,"state":"missing"}]}}
```

Existing `@eat/ninja_sushi` parses as `record_update` / `action: "update"`, with
`stem_source: "existing"` and stored title. Fields expose `value`/`display` and
prior `before`/`before_display`; `missing` lists required fields still empty.
`bob records check` reports legacy defects. Updates use `kind: "record_update"`,
`placement: "updated"`, and `note_files[].action: "update"` with `before` and `lines`.

```bash
bob capture -f json --dry-run -- "$(printf '%s\n' 'Ramen Nagi @eat/' '- status:: liked' '- category:: Ramen' '- location:: Clark')"
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"dry_run":true,"kind":"record_note","placement":"created","relative_target":"eat/ramen_nagi.md","record_note":{"action":"create","label":"Restaurant","emoji":"🍽️","path":"eat/ramen_nagi.md","notes":0,"properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed"}]},"note_files":[{"path":"eat/ramen_nagi.md","action":"create","record_type":"restaurant","properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed"}],"markdown":"---\n…\n"}]}
```

Record items add `record_note` summaries; incomplete drafts include `draft`,
`missing[]`, and `invalid[]`.

```bash
bob capture -f json --dry-run -- "$(printf '%s\n' 'Went back, still great @eat/ninja_sushi' '- status:: liked')"
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"dry_run":true,"kind":"record_update","placement":"updated","relative_target":"eat/ninja_sushi.md","note_files":[{"path":"eat/ninja_sushi.md","action":"update","record_type":"restaurant","properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed","before":"to_try","before_display":"🔖 To try"}],"markdown":"---\n…\n","lines":[{"text":"- 2026-10-20: Went back, still great","depth":0,"change":"added"}]}]}
```

Properties pair canonical `value` and human `display`. Lists are arrays, numbers
stay numeric, and dates, links, and URLs are strings. Empty optional fields omit
values.
See [Capture](../capture.md#record-notes) for the full representation.

```bash
bob capture-complete -n -c 16 -f json -- 'Ramen Nagi @eat/'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":25,"cursor":16,"context":"record_value","record":{"type":"restaurant","dir":"eat","label":"Restaurant","field":"status","assist_version":1},"candidates":[{"replacement":"\n- status:: liked","value":"liked","display":"💚 Liked","kind":"choices","cursor_after":33,"continuation":{"cursor":33,"next_missing_field":true}}]}
```

Accepting inserts `- status:: liked`; `-n` at `continuation.cursor` walks
remaining required fields, and the last omits `continuation`. `-F visited`
on the same draft is the unfiltered named-field request, with `record.edit`
for starting free-form entry:

```bash
bob capture-complete -F visited -c 16 -f json -- 'Ramen Nagi @eat/'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":25,"cursor":16,"context":"record_value","record":{"type":"restaurant","dir":"eat","label":"Restaurant","field":"visited","assist_version":1,"edit":{"replacement_range":{"start":16,"end":16},"replacement":"\n- visited:: ","cursor_after":29,"continuation":{"cursor":29,"next_missing_field":false}}},"candidates":[{"replacement":"\n- visited:: today","value":"today","display":"today","kind":"date","cursor_after":34}]}
```

`@` inserts `eat/ ` (`kind: "record"`). Empty `@eat#` expands `replacement`
to the token and prepends `kind: "record_create"`:

```bash
bob capture-complete -c 5 -f json -- '@eat#'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":25,"cursor":5,"replacement":{"start":0,"end":5},"context":"section","candidates":[{"kind":"record_create","replacement":"@eat/ ","label":"🍽️ New restaurant · eat/","route":"eat/","type":"restaurant","cursor_after":6,"continuation":{"cursor":6,"next_missing_field":true}},{"replacement":"@eat#Ideas","title":"Ideas","level":1}]}
```

Near-spellings warn `record_value_near` with no rewrite. Parse reports
`severity`/`code`/`message`/`range`; complete adds `authored`/`proposed`.

Past results are `kind: "pomodoro_past"` with `placement`; batches report
`recorded`/`trimmed`, warn `pomodoro_past_trim`, omit `pomodoro_close`.
Pinned runs add `segment`, `stack.pin`, `window.requested_minutes`/`cut`/`wall`/`fit`.
Stale previews refuse `pomodoro_past_windows_changed`; shell-expanded `~`
refuses `shell_expanded_tilde`; `pomodoro_past_segment` lists tail/`~1…~N`/head
rows (empty all, `~` head, digits filter, `+` end).

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

### Notebook reports {#notebook-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob notebooks list` | `-f json` | Notebooks, counts, issues, and host `catalogs` | `schema_version: 1` | [Notebooks](../notebooks.md#json) |
| `bob notebooks check` | `-f json` | Same report as list; `ok` reflects error findings | `schema_version: 1` | [Notebooks](../notebooks.md#json) |
| `bob notebooks sync`, `create` | `-f json` | Envelope: `ok`, `changes[]`, `commit`, `error`; `create` adds `notebook` | `schema_version: 1` | [Notebooks](../notebooks.md#sync) |

Each notebook carries `stem`, `path`, `title`, `summary`, `parent`,
`parent_kind`, `owner`, `root`, `breadcrumb`, `children`, `notes`,
`sections`, `last_note`, and `issues`. `catalogs[]` records: `host`, `path`, `root`,
`state`, `notebooks`, `issues`.

### Session edits and successor offers

Capture results are unversioned, including these per-item session payloads:

| Operation | Payload |
| --- | --- |
| Named task landing | Destination endpoint `merged: {before, after, added}` when the destination gains the task's type; `task_theme` is omitted unless an explicit type marker reports its result or a MEET landing changes the task's type |
| `=/` split | `kind: "pomodoro_split"`, with `pomodoro_split: {path, entry_line, before, after, base, created, stayed_links}`; `created[]` contains `type`, `line`, `name`, and `moved_links` |
| `=*` merge | `kind: "pomodoro_merge"`, with `pomodoro_merge: {path, entry_line, before, after, source_role, explicit?, absorbed}`; `source_role` is `current`, `next_up`, or `named`, `explicit` is the equivalent fully named token when an implied choice round-trips, and `absorbed[]` contains `name`, `role` (`named` or `next`), `from_line`, `moved_lines`, and `dropped_duplicates` |

Both return batch-level `pomodoro_blocks`.
Survivors show line changes, including removed lines. Removed
entries carry `removed: true`, previous lines `change: "removed"`. Parse reports modes
`pomodoro_split` and `pomodoro_merge`; a dest-less merge is complete, while
a trailing `#` still needs `pomodoro_merge_dest`.
See [split](../capture.md#splitting-a-merged-pomodoro) and
[merge](../capture.md#merging-pomodoros).

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

`bob capture -f json` successes return `move_choice` with `supported: true`;
refusals may include it. A `- ^^` slot adds `slots[]`. A valid dry run without
`-m` returns `answered: false` and `ok: true`, with no choices applied.

A draft has at most one choice slot. `item_line` is one-based;
`draft_range` is a half-open UTF-8 byte pair for `^^`.
`pomodoro_name` is optional. Bob supplies the display
strings in `copy`; `then` is a string or `null`. Example:

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

Rows follow shared movable-root eligibility in parent order.
Every listed row field is always present, with `null` where it does not apply.
They include ID-less and `#hide` roots, excluding `^prj`, closed/nested tasks,
and the template placeholder. `open_count` is the number of
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

These write-free refusals follow Pomodoro-override, lexical, and move-bullet
refusals, before `pull_in_choice_required` and `inbox_route_choice_required`:

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
task-creating or record-create item. `index` is the item's draft position.
Templates and drafts without new tasks/records return `items: []`.

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
| `matches[].scope` + record fields | Record rows only (`scope` `"record"`): type, stem, status/fields, `add_to` |
| `linked` | Keys already referenced by the item, with their link kind |

A candidate appears under only the item with its highest match score; ties
go to the earliest eligible item. Tasks explicitly linked by any task-creating
item are excluded from every item's matches. Record matches use the item's folder.
Non-exact matches need at least
two distinct meaningful terms. A `hold` row is below the display threshold;
clients use it only to retain an already-visible suggestion while wording
changes.
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
| `bob freshness` and `bob freshness list` | `-f json` | Review queue (`config` stages/limits; per-row `keeps`/`decide`/`stage`/`stage_rank`/`stage_total`) | `schema_version: 16` | [Freshness](../freshness.md) |
| `bob freshness seed` | `-f json` | Seed dry-run: stamped counts, buckets, skipped | `schema_version: 16` | [Freshness](../freshness.md) |
| `bob stats overview` | `-f json` | Weekly glance report | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats show` | `-f json` | One or more metric details | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats metrics` | `-f json` | Metric id, kind, and definition catalog | `schema_version: 1` | [Stats](../stats.md) |

Do not copy the freshness `16` onto stored files: it is the command
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

## Period notes

Period JSON uses `schema_version: 1`; brief successes use `2` and
`brief_version: "brief v2"`. `show`, `brief`, and `draft` accept
`last-week`, `last-quarter`, `last-year`.

| Command | Flag | Shape | Contract |
| --- | --- | --- | --- |
| `bob periods` and `bob periods list` | `-f json` | `rows[]` with identity, path, review state, Headline; `-p` adds `problems[]` | [CLI](../periods.md#cli-so-far) |
| `bob periods ensure` | `-f json` | `created`, `filled`, `existing`, `dry_run`; `-R` reports `refreshed`, `skipped_reflected`, `up_to_date`, commit | [CLI](../periods.md#cli-so-far) |
| `bob periods show` | `-f json` | Period/note metadata, navigation, facts, lanes, columns, groups, children, catalogs, warnings | [Reports](../periods.md#period-reports) |
| `bob periods brief` | `-f json` (default Markdown) | Facts, time, parts, grouped completions, open work, previous Review, quotes, caveats | [Agent briefs](../periods.md#agent-briefs-brief-v2) |
| `bob periods draft` | `-f json` | `id`, `words`, `review_state: drafted`; refusals carry `error` | [Drafting reviews](../periods.md#drafting-reviews) |
| `bob periods migrate` | `-f json` | Dry-run/write status, path changes, skipped navigation, sample diff, ISO years, commit | [Migration](../periods.md#migration) |

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
| `bob task restore` | `-f json` | Same move report for an archive source: `moves[]` carries the bound or `-d` destination, `status_symbol` reflects `--reopen`, commit subject is `bob task restore …` | `schema_version: 1` | [Task archive](../task-archive.md#restore) |
| `bob task archive` | `-f json` | `moves`, `holds`, `skips`, `grandfathered`, `link_repairs`, `renames` (`source`, `from`, `to`), `diagnostics` (`complete`, `counts`, `findings`), `git`, and `exit` (applied/deferred paths, recovery directory); `threshold` is nullable and `scan.files_selected` reports moved sources; `-n` previews | `schema_version: 2` | [Task archive](../task-archive.md#json) |
| `bob task merge` | `-f json` | `merges[]` with source, target, outcome, repaired dependencies and links; `pull_in` counts; write paths, Git results, and error | `schema_version: 1` | [Task merge](../task-merge.md#9-command-line) |
| `bob task split` | `-f json` | `candidates[]`, `splits[]` with owner, new tasks, stubs, placement, cursor, notice, and clamped counts; write paths, Git results, and error | `schema_version: 1` | [Task split](../task-split.md#3-command-line) |
| `bob task reconcile` (aliases `task-status-hooks`, `mark-next-tasks`) | `-f json` | Sync result: marked, cleared, blocked, dependency, and reference changes | Unversioned (serialized sync result, no `schema_version`) | [Task Status Hooks](../task-status-hooks.md) |
| `bob task reroll` (alias `randomize`) | `-f json` | Re-roll plan and per-task outcomes | `schema_version: 1` | [Randomize](../randomize.md) |
| `bob query` | `-f json` (default is `paths`) | `engine`, `query_kind`, `format`, `paths`, `result`, and `warnings` | Unversioned (no `schema_version`) | [Bob query](../dataview.md) |
| `bob articles format` | `-f json` | Article plans | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles list` | `-f json` | Article notes needing work | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles outline` | `-f json` | Section outline of one note | `schema_version: 1` | [Articles](../articles.md) |

`bob projects` and `bob nightly` are human-only in this checkout: they expose
no `--format json` or `--json` flag.

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
| `bob gcal sync` | `-f json` (default is `human`); `-d` previews without writing; `-q` silences success chatter while warnings stay visible | One day's Events rows and MEET links: per-row actions with buffer and description bullet counts, skipped events, the MEET Pomodoro entries, the Events stamp, waiting/held outcomes, warnings, and Google clock deprecations | `schema_version: 1`, `ok`, `day`, `path`, `status`, `force`, `rows`, `in_sync`, `skipped`, `waiting`, `held`, `pomodoro`, `warnings`, `deprecations`, `stamp` | [Bob Gcal](../gcal.md#sync) |
| `bob gcal login` | `-f json` | One-time setup report: `already_connected`, `account`, `client`, calendar counts, `sign_in` (`already_connected`, `browser`, `paste`), `client_source` (`stored`, `file`, `gog_client`, `downloads`, `pasted`), `client_project`, `reconnected` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal logout` | `-f json` | Removed grant: `account`, `client`, `revoke_url` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gkeep` and `bob gkeep list` | `-f json` (default is `table`) | Keep inbox notes with counts and error hints | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep doctor` | `-f json` | Adapter and credential diagnosis | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-markers` | `-f json` | Marker-removal plan | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-tasks` | `-f json` | Link-repair plan for open tasks | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep pull` | `-f json` | Inbox import report | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep status` | `-f json` (default is `human`) | Cached Keep pull count, check state, and open inbox count | `schema_version: 1` | [Bob Gkeep](../gkeep.md#states-and-the-json-document) |
| `bob gmail` and `bob gmail list` | `-f json` (default is `table`) | Saved threads with `turn` and `turn_since` (`null` when unrecorded), `total`, `shown`, and `truncated`; `-r` adds `new_messages` (remote count minus saved count, floored at zero) | `schema_version: 1` | [List](../gmail.md#list) |
| `bob gmail save` | `-f json` | `dry_run`, `results[]`, and `warnings`; each result includes identity, note, link, status, and message counts. Dry runs add per-result `preview`; a single result also exposes `note`, `link`, and any `preview` at the top level | `schema_version: 1` | [Save](../gmail.md#save) |
| `bob gmail search` | `-f json` (default is `table`) | `query`, `shown`, `limit`, and metadata-only `threads[]` with optional saved-note matches | `schema_version: 1` | [Search](../gmail.md#search) |
| `bob gmail peek` | `-f json` | Identity verdict, thread metadata, saved note, label, and message; no body or vault writes | `schema_version: 1` | [Peek](../gmail.md#peek) |
| `bob gmail link` | `-f json` | Link replacements | `schema_version: 1` | [Link](../gmail.md#link) |
| `bob gmail sync` | `-f json`; `-d` allowed on a non-writer host | `ok`, `dry_run`, `pulled`, `results` (`thread_id`, `note`, `link`, `status`, `task_file`, `route`; statuses `filed`, `would_file`, `adopted`, `already_filed`, `already_pulled`, `problem`, `refiled`, `would_refile`), `warnings`, `watch` (`thread_id`, `note`, `tasks`, `turn`, `turn_since`, `action`, optional `nudge_on`), `problems` (`thread_id`, `note`, `label`, `kind`, `message`, `hint`, `first_seen`), `summary` (`watching`, `awaiting`). `pulled` counts filed problems; first-seen fail `ok`. | `schema_version: 1` | [Sync](../gmail.md#sync), [Watch](../gmail.md#watch) |
| `bob gmail attach` | `-f json` | Attachment import results | `schema_version: 1` | [Attachments](../gmail.md#attachments) |
| `bob gmail new`, `reply` | `-f json` | `status`, `note`, and `cursor: {line, column}` for the local edit | `schema_version: 1` | [Compose notes](../gmail.md#compose-notes) |
| `bob gmail draft`, `send` | `-f json` | Draft and `send -d`: `status: preview`, `preview`, and `digest` after storing a remote draft. Send: status and message identities. Send recovery runs first, including with `-d`, and can return without a preview | `schema_version: 1` | [Gmail drafts](../gmail.md#gmail-drafts), [Writing and sending](../gmail.md#writing-and-sending) |
| `bob gmail doctor`, `login`, `logout` | `-f json` | Setup checks, account, sign-in, and revocation results | `schema_version: 1` | [Bob Gmail](../gmail.md#json-output) |
| `bob plugins` and `bob plugins list` | `-f json` (default is `table`) | Repo plugins with version, sync, vault state, and optional deployment provenance | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob plugins sync` | `-f json` | Copied, skipped, and unchanged files with backups and optional deployment provenance | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob vault-sync status` | `--json` / `-j` | Direct `StatusRecord`: `last_attempt_at`, `last_success_at`, `local_sha`, `remote_sha`, `files_committed`, `push_retries`, `duration_ms`, `conflicts`, `interrupted_merge_recovered`, `aborted_merge_head`, `halted_merge_head`, `interrupted_fast_forward_recovered`, `last_error`, `clobbers`, `recent_clobbers`, `pending_task_losses` (`id`, `path`, `kind`, `open`, `task`, `base_commit`, `line`, `first_seen`, `inbound_links`), `task_losses` and `recent_task_losses` (`id`, `path`, `kind`, `task`, `inbound_links`, `base_commit`, `restore_hint`, `detected_at`) | Unversioned (no `ok`, no `schema_version`) | [Vault Git sync](../vault-git-sync.md) |
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
