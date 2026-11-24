---
title: JSON outputs
description: Machine-readable Bob output contracts.
---

# JSON outputs

Fixtures use this checkout's `bob`, isolated `HOME`/`XDG_*`, a synthetic
vault, `BOB_NOW=2026-10-20 12:00:00`, `TZ=UTC`, `BOB_REF_JOBS_KICK=off`, and
`BOB_GMAIL_KICK=off`; network, clipboard, sync, and hooks are stubbed. Only
stdout `schema_version` counts.

## Framing

JSON goes to stdout, chatter to stderr; usage errors exit 2.

Most versioned helpers include `ok` and `schema_version`; `tick` uses
`exit_code` without `ok`. Some capture-helper errors return
`{"ok":false,"error":"…"}` unversioned.

`…` marks omissions.

```bash
bob capture-parse -f json -- 'Buy milk @dev'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":29,"input":"Buy milk @dev","body":"Buy milk","mode":"task","route":"dev","section":null,"block_id":null,"needs":[],"spans":[{"start":9,"end":13,"kind":"route"}],"diagnostics":[]}
```

```bash
bob plan -f json -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":3,"date":"2026-10-20","daily_file":"2026/20261020.md","status":"ok","caps":{"max_links":…,"max_next":…,"max_pending":…,"max_ready":…},"today":{"count":0},"today_tasks":[],"today_roots":[],"links":{"count":0,"cap":…,"over":false,"off_clock":0},"next":{"count":0,"cap":…,"over":false},"pending":{"count":0,"cap":…,"over":false},"theme_names":[],"entries":[],"warnings":[]}
```

`today_roots` sum to `today.count`; `hue` is null for `@unfiled`. See
[Plan](../plan.md#lints).

## Capture and protocol helpers

`bob capture` writes the vault; most `capture-*` helpers are read-only.

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob clipboard` | `-f json` | Read-only selection and provider diagnostics; reports `would_succeed`, the selected provider and entry ages on success, or a refusal `code` and message. Clipboard and history text are never included | `schema_version: 1` | [Capture](../capture.md#clipboard) |
| `bob capture` | `-f json` | Capture effects, notebook/record results, file previews, and attachment warnings/refusals. Optional `requirements[]` reports owner, added count (including nested requirements), note filing, and summary. Preview lines add optional `req: {depth, state}`. Record creates report changed neighbours in `walk_files[]` | Unversioned (`ok`, no `schema_version`) | [Capture JSON](../capture.md#input-stdin-and-json-output), [task requirements](../task-requirements.md#5-authoring) |
| `bob capture-parse` | `-f json` | Per-item modes, spans, diagnostics, and needs (`grammar: 29`). Requirement markers carry `depth`; refusals include `req_in_drawer`, `req_parent`, `req_state`, and `req_depth`. Notebook modes carry `notebook: true` spans | `schema_version: 1` | [Capture parse](../capture.md#bob-capture-parse) |
| `bob capture-complete` | `-f json` | Contexts, candidates, replacement ranges, pickers, effects, and creation choices. `work_log` recalls Work Log phrases. `-L` completes prose wikilinks; `-T` adds `((` task links and `-S PATH` supplies same-note context (both require `-L`). `-n` offers the first missing required record field; `-F KEY` offers values for a named field. Neither edits the draft | `schema_version: 1` | [Capture completion](../capture.md#bob-capture-complete) |
| `bob capture-pomodoro-name` | `-f json` | Name write (or `-d` plan) for one open Pomodoro, plus optional `warnings` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-pomodoros` | `-f json` | Ledger entries, `warnings`, `max_task_link_count`, and optional `start_token`. `--tasks` adds `intentions[]` display strings, `headline` (first intention or `null`), task agendas, optional calendar `events`, event chips, off-clock data (`off_clock`, deprecated `untracked`, `clock_source`, `clock_note`), totals, and Event Task buffers. Agenda `items[].lines[]` add optional `req: {depth, state}` on requirement rows | `schema_version: 1` | [Capture](../capture.md), [Pomodoro homes](../pomodoro-homes.md) |
| `bob capture-related` | `-f json` | Similar tasks or same-folder records, UTF-8 draft anchors, actions, and already-linked tasks; `--query` ranks task titles with scores | `schema_version: 1` | [Capture](../capture.md#bob-capture-related) |
| `bob capture-rewrite` | `-f json` | `input`, rewritten `text`, `changed`, `cursor`, `rule` (`collapse_global_destination` for glued `@@route` triggers, plus the alias/absorption rules), `edits`, and optional forward `alias`; unchanged cursor requests add an object or `null` `sigil_alias_tip` | `schema_version: 1` | [Capture-rewrite contract](../capture.md#bob-capture-rewrite) |
| `bob capture-sections` | `-f json` | `route`, `count`, and non-`Tasks` `sections` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-targets` | `-f json` | `bob_dir`, `count`, `targets`; notebook `kind: "notebook"`. Record rows: `kind: "record"`, `type` (album noun), optional `emoji`, `count`. Albums add `title`, optional `summary`/`owner`, and `breadcrumb` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md#record-notes) |
| `bob capture-task-id` | `-f json` | ID write (or `-d` plan) plus `note_path` and picker `*_replacement` strings; notebook notes add `item_kind: "note"` and `parent_replacement: "@nb+ID"` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-drawers` | `-f json` | Direct-child drawers plus catalog recents; drawer rows include `marker` (`legacy_caps` or `bold`) and project-note sections use `section` | `schema_version: 1` | [Task drawers](../task-drawers.md) |
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
{"ok":true,"schema_version":1,"grammar":29,"input":"Ramen Nagi @eat/","body":"Ramen Nagi","mode":"record_note","route":"eat","needs":["record_value"],"record":{"action":"create","type":"restaurant","label":"Restaurant","dir":"eat","path":"eat/ramen_nagi.md","stem":"ramen_nagi","stem_source":"title","title":"Ramen Nagi","missing":["status","category","location"],"notes":0,"assist_version":1,"fields":[{"key":"status","kind":"choices","required":true,"state":"missing"}]}}
```

Existing `@eat/ninja_sushi` is `record_update` (`stem_source: "existing"`). Fields
expose `value`/`display` and `before`/`before_display`; `missing` lists empty
required fields. Capture writes `kind: "record_update"`, `placement: "updated"`,
and `note_files[].action: "update"`.

```bash
bob capture -f json --dry-run -- "$(printf '%s\n' 'Ramen Nagi @eat/' '- status:: liked' '- category:: Ramen' '- location:: Clark')"
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"dry_run":true,"kind":"record_note","placement":"created","relative_target":"eat/ramen_nagi.md","record_note":{"action":"create","label":"Restaurant","emoji":"🍽️","path":"eat/ramen_nagi.md","notes":0,"properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed"}]},"note_files":[{"path":"eat/ramen_nagi.md","action":"create","record_type":"restaurant","body":"","properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed"}],"markdown":"---\n…\n\n[[eat|↑ Eat]]\n","lines":[{"text":"[[eat|↑ Eat]]","depth":0,"change":"added"}]}]}
```

Record items add `record_note` summaries; incomplete drafts include `draft`,
`missing[]`, `invalid[]`. Targets enrich identity fields. Album-backed writes add top-level `album_files[]` (`path`, `state`,
`before`, `after`) whenever the records index changes. Record creates also add
`walk_files[]` for pre-existing neighbours whose walk line changes; each entry
contains the original and final Markdown. The created record's `markdown` is the
exact committed text, including its walk line, while `body` remains authored
content. Capture record updates do not add `walk_files[]`. Damaged neighbours
appear as `record_walk_damaged` warnings and are not rewritten. Dry-run and
commit share those bytes.

```bash
bob capture -f json --dry-run -- "$(printf '%s\n' 'Went back, still great @eat/ninja_sushi' '- status:: liked')"
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"dry_run":true,"kind":"record_update","placement":"updated","relative_target":"eat/ninja_sushi.md","note_files":[{"path":"eat/ninja_sushi.md","action":"update","record_type":"restaurant","properties":[{"key":"status","label":"Status","value":"liked","display":"💚 Liked","source":"typed","before":"to_try","before_display":"🔖 To try"}],"markdown":"---\n…\n","lines":[{"text":"- 2026-10-20: Went back, still great","depth":0,"change":"added"}]}]}
```

Lists are arrays; numbers stay numeric; dates, links, URLs are strings.
Empty optionals omit values. See
[Capture](../capture.md#record-notes).

```bash
bob capture-complete -n -c 16 -f json -- 'Ramen Nagi @eat/'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":29,"cursor":16,"context":"record_value","record":{"type":"restaurant","dir":"eat","label":"Restaurant","field":"status","assist_version":1},"candidates":[{"replacement":"\n- status:: liked","value":"liked","display":"💚 Liked","kind":"choices","cursor_after":33,"continuation":{"cursor":33,"next_missing_field":true}}]}
```

Accepting inserts `- status:: liked`. `-n` walks remaining required fields.
`-F visited` is the unfiltered named-field request (`record.edit`):

```bash
bob capture-complete -F visited -c 16 -f json -- 'Ramen Nagi @eat/'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":29,"cursor":16,"context":"record_value","record":{"type":"restaurant","dir":"eat","label":"Restaurant","field":"visited","assist_version":1,"edit":{"replacement_range":{"start":16,"end":16},"replacement":"\n- visited:: ","cursor_after":29,"continuation":{"cursor":29,"next_missing_field":false}}},"candidates":[{"replacement":"\n- visited:: today","value":"today","display":"today","kind":"date","cursor_after":34}]}
```

`@` inserts `eat/ ` (`kind: "record"`). Empty `@eat#` expands `replacement`
to the token and prepends `kind: "record_create"`:

```bash
bob capture-complete -c 5 -f json -- '@eat#'
```

```text { .no-copy title="Output (selected fields)" }
{"ok":true,"schema_version":1,"grammar":29,"cursor":5,"replacement":{"start":0,"end":5},"context":"section","candidates":[{"kind":"record_create","replacement":"@eat/ ","label":"🍽️ New restaurant · eat/","route":"eat/","type":"restaurant","cursor_after":6,"continuation":{"cursor":6,"next_missing_field":true}},{"replacement":"@eat#Ideas","title":"Ideas","level":1}]}
```

Near-spellings warn `record_value_near` with no rewrite. Parse reports
`severity`/`code`/`message`/`range`; complete adds `authored`/`proposed`.
Past results are `kind: "pomodoro_past"`; batches report `recorded`/`trimmed`
and warn `pomodoro_past_trim`. Pinned runs add `segment` and `stack.pin`.
Stale previews refuse `pomodoro_past_windows_changed`; `pomodoro_past_segment`
lists tail/`~1…~N`/head rows.

`relation_label` rows include `label`, `key`, `count`, `replacement`,
`inverse`, `hide_replacement` (`||`). A typed query row is first (`typed`,
`existing`, `hidden`; `inverse` per §10). Hidden labels drop unless typed
exactly (`hidden: true`, `hidden_count`). A `||` probe keeps both pipes and
sets `one_off: true`.

### Album reports {#album-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob albums list` | `-f json` | Albums, independent index groups, `walk_by`, and `index_group_by`, walk order/groups with first-member heads, issues, owner `catalogs`, and `capabilities` (`record_walk_groups_v1`) | `schema_version: 1` | [Albums and record notes](../records.md#albums) |
| `bob albums check` | `-f json` | Same report as list | `schema_version: 1` | [Albums and record notes](../records.md#albums) |
| `bob albums sync` | `-f json` | Index entries, grouped `walk`, owner `catalogs`, `kind`/`walk_lines`, transaction results | `schema_version: 1` | [Albums and record notes](../records.md#syncing-records-indexes) |
| `bob albums tidy` | `-f json` | Tidied albums, counts, kept H1s, transaction results | `schema_version: 1` | [Tidying album titles](../records.md#tidying-album-titles) |
| `bob albums create` | `-f json` | `album`, `changed`, `changes[]`, `catalogs`, `commit`, `error` | `schema_version: 1` | [Albums and record notes](../records.md#creating-an-album) |
| `bob albums update` | `-f json` | `stem`, grouping-bound fingerprints, ordered results, schema changes in `changes[]` including walk lines, `inverse_ops` including `walk_by.set` | `schema_version: 1` | [Album update](../records.md#updating-an-album-schema) |

### Record reports {#record-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob records show` | `-f json` | Schema for `TYPE`; with `STEM`, properties, identity, findings, full suggestions, fingerprint, group membership and directional group stops in the walk ring. Refusals add `error.code` | `schema_version: 1` | [Reading one record](../records.md#reading-one-record) |
| `bob records check` | `-f json` | Types with findings, including `record_walk_stale` and `record_walk_damaged` warnings | `schema_version: 1` | [`bob records`](../records.md#bob-records) |
| `bob records new` | `-f json` | Created record's exact `markdown`, optional `album_index`, `walk_files[]` for changed pre-existing neighbours, and `record_walk_damaged` warnings | `schema_version: 1` | [`bob records`](../records.md#bob-records) |
| `bob records tidy` | `-f json` | Tidied notes, counts, and transaction results | `schema_version: 1` | [Tidying empty Notes sections and redundant H1s](../records.md#tidying-empty-notes-sections-and-redundant-h1s) |
| `bob records update` | `-i FILE -f json` | Guarded envelope: `changed[]`, `changes[]`, fingerprints, `inverse_ops`, groups, `consequence`, and typed `error` | `schema_version: 1` | [Updating with an envelope](../records.md#updating-with-an-envelope) |

### Area reports {#area-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob areas list` | `-f json` | Area tree, catalogs, findings, `contract_issues`, counts | `schema_version: 1` | [Areas](../areas.md#json) |
| `bob areas check` | `-f json` | Same as list | `schema_version: 1` | [Areas](../areas.md#json) |
| `bob areas sync` | `-f json` | Sync envelope | `schema_version: 1` | [Areas](../areas.md#sync) |

### Notebook reports {#notebook-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob notebooks list` | `-f json` | Notebooks, counts, issues, and host `catalogs` | `schema_version: 1` | [Notebooks](../notebooks.md#json) |
| `bob notebooks check` | `-f json` | Same report as list; `ok` reflects error findings | `schema_version: 1` | [Notebooks](../notebooks.md#json) |
| `bob notebooks sync`, `create` | `-f json` | Envelope: `ok`, `changes[]`, `commit`, `error`; `create` adds `notebook` | `schema_version: 1` | [Notebooks](../notebooks.md#sync) |

### Project reports {#project-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob projects list` | `-f json` | Projects, catalogs, findings, `contract_issues`, counts | `schema_version: 1` | [Projects](../projects.md#json-report) |
| `bob projects sync` | `-f json` | Lifecycle/catalog events, changes, findings, commit, sync, warnings; dry runs add diffs | `schema_version: 1` | [Projects](../projects.md#commands) |

### Task log reports {#task-log-reports}

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob logs list` | `-f json` | Per-kind phrase catalog, match tier, frecency, recency, exclusions, and warnings | `schema_version: 1` | [Log Recall](../log-recall.md) |

Capture may include `project_note.parent_catalog` (`path`, `action`) when the
parent catalog changes; dry runs set `dry_run: true`. See
[capture JSON](../capture.md#input-stdin-and-json-output).

### Bank reports {#bank-reports}

`schema_version: 1`; [Bank JSON](../bank.md#json) with `day`/`week`/`quarter`/`iso_year`/`all_time` periods, `-s` `days`, `coverage`/`navigation`/`balances[]` ([Closing balances](../bank.md#closing-balances)). Bank rows carry `subscription`/`bank_recurring`/`subscription_candidate` with charge dates; `suggestion.source` gains `subscription`; `list`/`report` carry at most one bounded `bank_subscription_album_invalid` warning.
`report -c` adds [`charts`](../bank.md#report-charts) `{version, granularity, buckets[], groups[], categories}` for `week`/`quarter`/`iso_year`/`all_time` periods.
`subscriptions` adds `album`, `totals`, ledger entries, `issues`, `findings[]`.

### Session edits and successor offers

Per-item session payloads are unversioned:

| Operation | Payload |
| --- | --- |
| Named task landing | Destination endpoint `merged: {before, after, added}` when the destination gains the task's type; `task_theme` is omitted unless an explicit type marker reports its result or a MEET landing changes the task's type |
| `=/` split | `kind: "pomodoro_split"`, with `pomodoro_split: {path, entry_line, before, after, base, created, stayed_links}`; `created[]` contains `type`, `line`, `name`, and `moved_links` |
| `=*` merge | `kind: "pomodoro_merge"`, with `pomodoro_merge: {path, entry_line, before, after, source_role, explicit?, absorbed}`; `source_role` is `current`, `next_up`, or `named`, `explicit` is the equivalent fully named token when an implied choice round-trips, and `absorbed[]` contains `name`, `role` (`named` or `next`), `from_line`, `moved_lines`, and `dropped_duplicates` |

Both return batch `pomodoro_blocks`.
See [split](../capture.md#splitting-a-merged-pomodoro) and
[merge](../capture.md#merging-pomodoros).

Managed Done results include `keep_moving` beside `unblocked`.
See [keep-moving offers](../task-dependencies.md#1215-keep-moving-offers-leaf-projects).

Relation successors connect a completed or parked Pending task, without a
Task Link today, through a configured relation (such as `starts after`).
Selection is explicit. Fields:

| Location | Fields |
| --- | --- |
| `task_complete`, `pomodoro_close`, nested `relation_completes[].task_complete`, or a Pending-unlink capture item | `relation_successors[]` and `relation_held[]` are omitted when empty; `relation_check` (`checked` or `unavailable`) is present only when the lookup ran |
| A note-pull offered row | `offer.note` is the requested source, matching `pull_in.groups[].note`; `note_path` is the final home after a later move |
| An offered row with a relation reason | `relation: {label, key, event, after}`; `event` is `parked` or `done`, and `after[]` identifies the triggering tasks with `note_path`, `block_id`, and `text`. A relation-group row also has `offer.after`, a `note.md#^id` string |
| A held relation row | Task identity, the relation, and `reason`: `already_planned`, `already_started`, `blocked`, `hidden`, or `waits_for` |
| Batch `pull_in` | `supported`, `offer_count`, `choice` (`null`, `all`, `none`, or `subset`), `picked_count`; always-present `reqs: {supported, pending}`; optional `groups[]` and `copy: {title, capsule, glyph}` |
| `pull_in.groups[]` | `group`, optional `project` or note-pull `note`, requirement `kind`/`owner`/`rows` only on the `requirements` discriminator, `copy: {title, why?, tag?, fold?, glyph, scope?, history?, footer?, hint?}`, and optional `folded` counts; relation groups also carry `after: {key, note_path, block_id, text}`, `label`, and `event` |
| Batch `req_outcomes[]` | Applied requirement verdicts (`met`, `drop`, `carry`, `carry_pulled`); omitted when empty |

Empty `groups` and absent `copy` are omitted. Requirement groups carry
`kind: "requirements"` (clients switch on `kind`, fall back to `group`),
an opaque snapshot-bound `owner` (with `key`), one row per Pending req
(`offer` envelope, `req`, bob-authored `consequences`), canonical `copy`,
and historical `folded` counts only when nonzero; `req.parent_key` is
`null` at depth 1, `req_outcomes[].reason` appears only on a drop with a
nonblank reason, and `carried` appears only on a carry. See the [successor
JSON contract](../task-dependencies.md#125-reporting-model) with its
req-only close example, the [normative requirement
contract](../task-dependencies.md#1225-requirements-on-the-successor-card)
with its mixed-card example, the [RV
vectors](../task-dependencies.md#1226-rv-requirement-close-vectors),
and [relation events](../task-dependencies.md#1218-relation-successors-starts-after).

### Move-choice results

`bob capture -f json` successes return `move_choice` with `supported: true`;
refusals may include it. A `- ^^` slot adds `slots[]`. A valid dry run without
`-m` returns `answered: false` and `ok: true`, with none applied.

A draft has at most one choice slot. `item_line` is one-based;
`draft_range` is a half-open UTF-8 byte pair for `^^`.
`pomodoro_name` is optional. Bob supplies strings
in `copy`; `then` is a string or `null`. Example:

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

Rows follow shared movable-root eligibility in parent order. Every field is
present (`null` when inapplicable).

Repeatable `-m, --move-in` values union selections: bare numbers or keys mean
move (`^`), and a leading `:` means move and link. `all` moves every enabled,
unlocked row; `none` moves none.

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

Exit codes match the pull-in and inbox-route refusals.

### Inbox-routing results

Successful captures, including dry runs, return `inbox_route` with
`supported: true`, `candidate_count`, `routed_count`, `kept_count`, and
`candidates`. Each candidate is an existing top-level inbox task block whose
root or nested task changes status; new tasks are excluded. Each has an opaque
`key`, a 1-based `n`, the root's before/after status, an `answer` (initially
`null`), and an `outcome` (`pending`, `kept`, or `moved`).
`destinations` is present only when candidates exist.

See the
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

Draft results omit scores. The full
[capture-related contract](../capture.md#bob-capture-related) owns actions,
candidate eligibility, and error behavior.

## Plan, readiness, freshness, and stats

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob plan` | `-f json` | Date, status, caps, Today tasks and root counts, lane counts, and `warnings` | `schema_version: 3` | [Plan](../plan.md) |
| `bob ready` | `-f json` | Per-note Ready lane against `max_ready_per_note` | `schema_version: 1` | [Plan](../plan.md) |
| `bob freshness` and `bob freshness list` | `-f json` | Review queue (`config` stages/limits; per-row `keeps`/`decide`/`stage`/`stage_rank`/`stage_total`) | `schema_version: 16` | [Freshness](../freshness.md) |
| `bob freshness seed` | `-f json` | Seed dry-run: stamped counts, buckets, skipped | `schema_version: 16` | [Freshness](../freshness.md) |
| `bob walks` | `-f json` | `s`/`w`/`b` attention (`available`, `attention`, `tier`, `label`; family extras) | `schema_version: 1` | [Walk attention](../walks.md) |
| `bob stats overview` | `-f json` | Weekly glance report | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats show` | `-f json` | One or more metric details | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats metrics` | `-f json` | Metric id, kind, and definition catalog | `schema_version: 1` | [Stats](../stats.md) |

Do not copy the freshness `16` onto stored files: it is the command
envelope version. See
[Stats summaries](../stats.md#ranges-grouping-summaries-display),
[Clocks](../stats.md#clocks),
[Health](../stats.md#data-model-and-quality), and
[Daily Stats](../stats.md).

## Period notes

Period JSON uses `schema_version: 1`; brief successes use `3` and
`brief_version: "brief v3"` (error envelopes keep `schema_version:
1`). Day briefs carry the `day v1` instruction object; period briefs
carry the matching `weekly v3` kind. `show`, `brief`, and `draft`
accept `yesterday`, `last-day`, `last-week`, `last-quarter`,
`last-year`, and the other day/period subject forms.

| Command | Flag | Shape | Contract |
| --- | --- | --- | --- |
| `bob periods` and `bob periods list` | `-f json` | `rows[]` with identity, path, review state, Summary; `-p` adds `problems[]` | [CLI](../periods.md#cli-so-far) |
| `bob periods ensure` | `-f json` | `created`, `filled`, `existing`, `dry_run`; `-R` reports `refreshed`, `skipped_reflected`, `up_to_date`, commit | [CLI](../periods.md#cli-so-far) |
| `bob periods show` | `-f json` | Days: `subject`, `intentions`, `summary`, `summary_state`, `reflection`, `review_state`, warnings. Weeks/quarters/years/all time: note metadata, navigation, facts, lanes, charts, columns, groups, children, catalogs, warnings | [Reports](../periods.md#period-reports), [Daily subjects](../periods.md#daily-subjects-and-guarded-field-updates) |
| `bob periods brief` | `-f json` (default Markdown) | Instructions first, then header, intentions, facts, time, parts (periods only), grouped completions, open work, Bryan's words, previous day's summary (day only), events, v3 neighbours, week day summaries with coverage, off-clock, caveats | [Agent briefs](../periods.md#agent-briefs-brief-v3) |
| `bob periods draft` | `-f json` | `id`, `words`, actual `review_state`, additive `summary_state`; refusals carry `error` | [Drafting reviews](../periods.md#drafting-reviews) |
| `bob periods update` | `-i FILE` (`-` for stdin), `-f json`; `-n` previews | Subject, dry-run/changed flags, before/after fields and fingerprints, inverse operations, warnings | [Guarded field updates](../periods.md#daily-subjects-and-guarded-field-updates) |
| `bob periods migrate` | `-f json` | Dry-run/write status, path changes, skipped navigation, sample diff, ISO years, commit | [Migration](../periods.md#migration) |

## Note properties

Both commands use `schema_version: 1` and `ok`. NOTE is an existing
vault-relative Markdown path (optional `.md`) or an absolute path inside the vault.

| Command | Flag | Shape | Contract |
| --- | --- | --- | --- |
| `bob props show NOTE` | `-f json` | Path, note kind, `props_v1`/`props_contract_v1`, semantic fingerprint, frontmatter state, ordered properties with values, kinds, styles, ownership roles, diagnostics, settings `contract` | [Show report](../props.md#show-report), [Settings contract](../props.md#settings-contract) |
| `bob props update NOTE` | `-i FILE` (`-` for stdin), `-f json`; `-d` previews | Path, dry-run/changed flags, before/after fingerprints, inverse operations, frontmatter diff, warnings, `effects`, resulting properties | [Update envelope](../props.md#update-envelope-and-operations), [Settings contract](../props.md#settings-contract) |

`show` may succeed with damaged YAML; frontmatter state and diagnostics identify
repairs needed. An update's `expect` comes from `show`'s fingerprint, identifying
the property values read. Stale fingerprints and edits to owned keys are refused.
Neither command commits or syncs the vault.

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
| `bob attachments import` | `-f json` | `attachments` (`{source, display, saved, kind (image\|video\|file), reused, embed, size_bytes, sha256}`), `warnings` (`string[]`); errors: `missing`, `not_a_file`, `not_absolute`, `too_many`, `too_large`, `not_synced`, `lock_busy`, `io` | `schema_version: 1` | [Attachment origins](../attachments.md#bob-attachments) |
| `bob attachments origin` | `-f json` | `path`, `origin` (`{source, display, host, ts, via, state}`) or `null`, and `warnings` | `schema_version: 1` | [Attachment origins](../attachments.md#resolution) |
| `bob relations rename` | `-f json` | `mode`, `changed`, `from`, `to`, `merge`, affected `files` (`{path, links, mirrors, lines: [{line, kind, before, after}]}`), `counts` (`{links, notes, mirrors, done_links, done_notes}`), `state`, `inverse`, replacement strings, `commit` (`{sha, subject}`), `sync` (`{pre, post}`), and `warnings`. With `-r/--reverse`, adds `reversed: true`, `moves[]` (`{source: {path, line, block_id, text}, target: {path, line, block_id, text}, from_label, to_label, action: "moved"\|"merged"}`, one-based task lines, minted IDs included), and `minted_block_ids[]` (`{path, line, block_id}`) | `schema_version: 1` | [Rename transaction](../task-relations.md#rv8-the-rename-transaction), [Reversing a label](../task-relations.md#rv10-reversing-a-label) |

Theme metadata appears as `themes`, `theme_refs`, `added_theme_refs`, or
`theme_hues`; these additions keep existing versions (`bob plan` schema 3;
capture unversioned).

## Tasks, reconciliation, and queries

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob task move` | `-f json` | Move plan and repaired-link report | `schema_version: 1` | [Task move](../task-move.md) |
| `bob task restore` | `-f json` | Same move report for an archive source: `moves[]` carries the bound or `-d` destination, `status_symbol` reflects `--reopen`, commit subject is `bob task restore …` | `schema_version: 1` | [Task archive](../task-archive.md#restore) |
| `bob task archive` | `-f json` | `moves`, `holds`, `skips`, `grandfathered`, `link_repairs`, `renames` (`source`, `from`, `to`), `diagnostics` (`complete`, `counts`, `findings`), `git`, and `exit` (applied/deferred paths, recovery directory); `threshold` is nullable and `scan.files_selected` reports moved sources; `-n` previews | `schema_version: 2` | [Task archive](../task-archive.md#json) |
| `bob task merge` | `-f json` | `merges[]` with source, target, outcome, repaired dependencies and links; `pull_in` counts; write paths, Git results, and error | `schema_version: 1` | [Task merge](../task-merge.md#9-command-line) |
| `bob task rename-drawer` | `-f json` | Names, counts, per-file drawer/section/link changes, refusals, exclusions, unresolved links, template hits, commit, sync, warnings, I/O errors, error | `schema_version: 1` | [Renaming drawers](../task-requirements.md#35-renaming-drawers) |
| `bob task split` | `-f json` | `candidates[]`, `splits[]` with owner, new tasks, stubs, placement, cursor, notice, and clamped counts; write paths, Git results, and error | `schema_version: 1` | [Task split](../task-split.md#3-command-line) |
| `bob task tidy` | `-f json` | Routes, counts, per-file task changes with before/after lines, commit, sync, warnings, I/O errors, error | `schema_version: 1` | [Tidying task blocks](../task-requirements.md#34-tidying-task-blocks) |
| `bob task reconcile` (aliases `task-status-hooks`, `mark-next-tasks`) | `-f json` | Sync result: marked, cleared, blocked, dependency, and reference changes | Unversioned (serialized sync result, no `schema_version`) | [Task Status Hooks](../task-status-hooks.md) |
| `bob task reroll` (alias `randomize`) | `-f json` | Re-roll plan and per-task outcomes | `schema_version: 1` | [Randomize](../randomize.md) |
| `bob query` | `-f json` (default is `paths`) | `engine`, `query_kind`, `format`, `paths`, `result`, and `warnings` | Unversioned (no `schema_version`) | [Bob query](../dataview.md) |
| `bob articles format` | `-f json` | Article plans | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles list` | `-f json` | Article notes needing work | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles outline` | `-f json` | Section outline of one note | `schema_version: 1` | [Articles](../articles.md) |

`bob nightly` is human-only in this checkout: it exposes
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
| `bob gcal calendars` | `-f json` (default is `table`) | Calendar identity rows with optional `mark` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal` and `bob gcal list` | `-f json` (default is `table`); `-a` adds omitted events | Local-day events: RSVP, inclusion, root, Google identity, source calendar (`id`, `name`, `mark`), note summary (not text), directives, clock, buffer, description counts, warnings, vault state, and last sync; titles stay bare | `schema_version: 1`, `day`, `calendars`, `events`, `counts`, `last_sync`, `past_open`, `warnings`, `deprecations` | [Bob Gcal](../gcal.md), [Private event notes](../gcal-notes.md) |
| `bob gcal doctor` | `-f json` | Checks (`id`, `status`, `summary`, `fix`); calendar summary names the sync set and marks non-primary names | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal sync` | `-f json` (default is `human`); `-d` previews without writing; `-q` silences success chatter while warnings stay visible | One day's Events/MEET report: row actions with `calendar_id` and `calendar` identity (`id`, `name`, `mark`), buffer, description counts, note summary (not text), skipped events, MEET entries, stamp, waiting/held outcomes, warnings, and deprecations | `schema_version: 1`, `ok`, `day`, `path`, `status`, `force`, `rows`, `in_sync`, `skipped`, `waiting`, `held`, `pomodoro`, `warnings`, `deprecations`, `stamp` | [Bob Gcal](../gcal.md#sync), [Private event notes](../gcal-notes.md) |
| `bob gcal note` | `-f json` (default is `human`); `-l`, `-a`, `-D`, `-i`/`-C`, `-s` | 90-day selection with identity, RSVP, organizer (no email), `lands`, note summary (no text in listings); show adds text; ambiguous returns `candidates` | `schema_version: 1`, `ok`, `window`, `notes`/`events`, `event`, `candidates`, `orphans`, `warnings` | [Private event notes](../gcal-notes.md) |
| `bob gcal login` | `-f json` | One-time setup report: `already_connected`, `account`, `client`, calendar counts, `sign_in` (`already_connected`, `browser`, `paste`), `client_source` (`stored`, `file`, `gog_client`, `downloads`, `pasted`), `client_project`, `reconnected` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gcal logout` | `-f json` | Removed grant: `account`, `client`, `revoke_url` | `schema_version: 1` | [Bob Gcal](../gcal.md) |
| `bob gkeep` and `bob gkeep list` | `-f json` (default is `table`) | Keep inbox notes with counts and error hints | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep doctor` | `-f json` | Adapter and credential diagnosis | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-markers` | `-f json` | Marker-removal plan | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-tasks` | `-f json` | Link-repair plan for open tasks | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep pull` | `-f json` | Inbox import report | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep status` | `-f json` (default is `human`) | Cached Keep pull count, check state, and open inbox count | `schema_version: 1` | [Bob Gkeep](../gkeep.md#states-and-the-json-document) |
| `bob gmail` and `bob gmail list` | `-f json` (default is `table`) | Saved threads with `turn`/`turn_since` and active body-free `archive_receipts[]` keyed by account/thread; `-a` gives the complete offline snapshot. Unsafe receipt data has null values, `diagnostics[]`, and `ok: false`; `-r` adds remote `new_messages` | `schema_version: 1` | [List](../gmail.md#list) |
| `bob gmail save` | `-f json` | `dry_run`, `results[]`, and `warnings`; each result includes identity, note, link, status, and message counts. Dry runs add per-result `preview`; a single result also exposes `note`, `link`, and any `preview` at the top level | `schema_version: 1` | [Save](../gmail.md#save) |
| `bob gmail search` | `-f json` (default is `table`) | `query`, `shown`, `limit`, and metadata-only `threads[]` with optional saved-note matches | `schema_version: 1` | [Search](../gmail.md#search) |
| `bob gmail peek` | `-f json` | Identity verdict, thread metadata, saved note, label, and message; no body or vault writes | `schema_version: 1` | [Peek](../gmail.md#peek) |
| `bob gmail link` | `-f json` | Link replacements | `schema_version: 1` | [Link](../gmail.md#link) |
| `bob gmail sync` | `-f json`; `-d` preview, `-c` digest, `-n` (`archive_skipped`) | Filing, watch, problem, archive report with config, summary and warnings. Watch `reopened` rows add reply sender as `who`, `reason` (`replied`\|`bounced`), and restored locators. Archive `review` rows add `first_seen`, `hint`, never entering the digest. `recent_archive` reports distinct threads since previous local midnight; latest items: note, archive time, message count, receipt label/event id, state. `null` means unavailable. Digest: dry-run/paused only. `failure_kinds` (only when `ok` is false): sorted, deduped kinds of every failure cause. | `schema_version: 1` | [Sync](../gmail.md#sync), [Archive](../gmail.md#archive-on-close), [Watch](../gmail.md#watch), [Reopen](../gmail.md#reopen) |
| `bob gmail unarchive` | `-f json`; `-d` dry-run | Per-input results, frozen IDs, label, and errors | `schema_version: 1` | [Explicit undo](../gmail.md#explicit-undo) |
| `bob gmail attach` | `-f json` | Attachment import results | `schema_version: 1` | [Attachments](../gmail.md#attachments) |
| `bob gmail new`, `reply` | `-f json` | `status`, `note`, and `cursor: {line, column}` for the local edit | `schema_version: 1` | [Compose notes](../gmail.md#compose-notes) |
| `bob gmail draft`, `send` | `-f json` | Draft and `send -d`: `status: preview`, `preview`, and `digest` after storing a remote draft. Send: status and message identities. Send recovery runs first, including with `-d`, and can return without a preview | `schema_version: 1` | [Gmail drafts](../gmail.md#gmail-drafts), [Writing and sending](../gmail.md#writing-and-sending) |
| `bob gmail doctor`, `login`, `logout` | `-f json` | Setup checks, account, sign-in, and revocation results | `schema_version: 1` | [Bob Gmail](../gmail.md#json-output) |
| `bob plugins` and `bob plugins list` | `-f json` (default is `table`) | Repo plugins with version, sync, vault state, and optional deployment provenance | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob plugins sync` | `-f json` | Copied, skipped, and unchanged files with backups and optional deployment provenance, plus `not_configured` repo ids the `plugins.sync` allowlist leaves out | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob vault-sync status` | `--json` / `-j` | Direct `StatusRecord`: `last_attempt_at`, `last_success_at`, `local_sha`, `remote_sha`, `files_committed`, `push_retries`, `duration_ms`, `conflicts`, `interrupted_merge_recovered`, `aborted_merge_head`, `halted_merge_head`, `interrupted_fast_forward_recovered`, `last_error`, `last_error_kind` (`network` when the remote was unreachable), `clobbers`, `recent_clobbers`, `pending_task_losses` (`id`, `path`, `kind`, `open`, `task`, `base_commit`, `line`, `first_seen`, `inbound_links`), `task_losses` and `recent_task_losses` (`id`, `path`, `kind`, `task`, `inbound_links`, `base_commit`, `restore_hint`, `detected_at`) | Unversioned (no `ok`, no `schema_version`) | [Vault Git sync](../vault-git-sync.md) |
| `bob tick` | `-f json`; `-d` preview, `-s` status | `command: "tick"`, host, vault, trigger, timestamps, `exit_code`, and ordered `steps[]` with outcomes (`ok`, `failed`, `waiting`, `skipped`), reasons, summaries, gates, and scheduling history. Consecutive contention waits spanning two hours become `outcome: "failed"`, `reason: "stalled"`; gate waits never escalate. Read-only views add `mode`; plan rows add `would_run`. No top-level `ok`. | `schema_version: 1` | [Results and recovery](../tick.md#results-and-recovery) |
| `bob completion status` | `--json` / `-j` | Protocol, executable, and per-shell install state | `schema_version: 1`, `protocol: 1` | [Completion](../completion.md) |

## Other structured formats

These public non-JSON formats use their own flags.

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
