# Requirement close-card vectors

`vectors.json` is the single source of truth for requirement groups on
Bob's managed Done successor card (`src/native/task_complete/req_close.rs`,
contract `docs/task-requirements.md` §7). Bob-cli owns this file; clients
copy it byte for byte. A behavior change edits the contract and this
fixture together.

The `rows` family (RV1–RV11) pins pre-close collection, snapshot-bound
owner and row keys, canonical group copy, historical folds, and
pending-descendant counts. The `choices` family (RV12–RV19, RV44) pins final
verdict conflicts, requirement-first card numbering, mark-all exclusion,
interactive folds, ancestor overlap (including across a hand-ticked
intermediate), navigation-only pins, multi-owner
choices, stale replay after duplicate-label reordering, and duplicate
owner refusal. The `writes` family (RV20–RV28) pins atomic carry/drop bytes, receipts,
created-task identities, and descendant moves without adding another commit
path. The `placement` family (RV29–RV35) pins where a `carry_pulled` task
links (slot, `inherit`, closing continuation, implicit fallback, daily
owner via the inbox), its Next status with no freshness stamp, card-order
links for several carries, and the no-destination refusal. The `jump`
family (RV36–RV43) pins final landings for a pinned row: the created task
for a carried row, the move-mapped line for a row carried with its
ancestor, the requirement's own line after sibling carries, drop logs, a
dropped ancestor, and an inbox route that renamed the owner's block, plus
identity (not label) landing between same-labelled carried branches.

## Shape

- `schema_version`: currently `1`.
- `contract`: the contract section this file pins.
- `family`: the initial family authored in this file (`rows`); each case
  carries its own family so later phases can append without renumbering.
- `copy`: canonical strings every surface must spell the same way.
- `cases[]`: every case has `id`, `family`, `title`, and `expect`.
- `rows` cases add `notes[]` and freeze collection output.
- `choices` cases add `input` and freeze selection behavior. Their
  `base_case` points at a `rows` case; selector inputs contain the full
  opaque keys that case produced. `source_lines_after` models an edit
  between preview and submit.
- `writes` cases add `input` (`base_case` or inline `notes` plus `requests`,
  `today`, and `date`) and freeze the atomic write plan. `expect` carries
  exact `postimages` bytes, per-row `outcomes` (verdict, receipt, carried
  task identity, and descendant `moves`), and aggregated `met` receipts.
- `placement` cases add `input` with `base_case` or inline `notes`,
  `requests`, the `day` file (`path` plus `source_lines`, or
  `source_lines: null` when the day file is one of the notes and is read
  after the writes), the note `index`, the gesture's `completed` set
  (`path`, `block_id`, `is_root`), and an optional Pomodoro `closing`
  context (`continuation_name`, 0-based `closed_line`, `continuation`).
  `expect` carries exact `day_after` and `notes_after` bytes after the
  writes and placement, one `links[]` row per linked task (status,
  destination role and name, fallback, 1-based entry and bullet lines,
  block link, entry creation), or a `refusal` code.
- `jump` cases add `input.jump` (the pinned row's full key) and optional
  `final_notes` (texts after a later route; `null` removes a note) plus
  `routes[]` (`source`, `dest`, `old_block_id`, `new_block_id`). `expect`
  carries the `landing` (`kind`: `carried_task`, `moved_with_ancestor`,
  or `requirement`; the effective `outcome`; final `path` and 1-based
  `line`; the created task's `block_id`; the landed line `text`; and the
  enclosing task `in_task`), the write plan's `postimage_count`, and the
  `carried` row-path → created block ID map, or a `refusal` code.
- `notes[]`: `path`, `source_lines` (joined with `\n` plus a trailing
  newline), and `closing[]` (`block_id` or 1-based `line`) in the
  explicit close order for that note. Recurring owners are omitted
  from `expect.owners`.
- `expect.owners[]`: snapshot identity for every collected owner, in
  close order. `key` is `task_key` + `@` + sha256 of the exact
  pre-close owner-block bytes. `preimage` is those bytes.
- `expect.groups[]`: one `requirements` group per owner that has
  Pending rows. Each group carries `group`, `kind`, `owner`, `copy`,
  `folded`, and `rows`. A row has an `offer` envelope, `req`, and
  `consequences`. Pending includes flagged unknown marks; excluded
  descendants are never Pending. Manual `[x]` is Met for that row
  only and counts as `met` in `folded`, never as a listed row.

`copy.glyph` is the semantic token `"requirement"` (like `relation`
and `note_pull`), never a drawn mark. A row's `run` is the
plain-text fallback mark (`req_glyph`): `"◇"` on every listed row,
since only Pending rows are listed. The `run` key name is historical.

Row keys are opaque: `owner.key` + `/req/` + sibling path + `~` +
the first eight hex characters of sha256(clean text). Clients compare
the complete supplied key to a freshly enumerated forest.
