# Requirement vectors

`vectors.json` is the single source of truth for the recursive requirement
recognizer in bob-cli (`src/native/task_reqs.rs`, contract
`docs/task-requirements.md` §1–§2). Its cases are `RQ1`–`RQ21`, covering:

- every positive and negative position (notes, drawers, managed lines, logs,
  notebook-adjacent rows, fences inside list items, quotes, pasted checklists);
- depth up to 5, mixed tab and space indentation, and the `MAX_REQ_DEPTH`
  (16) bound;
- nested `#task` ownership boundaries;
- every mark (`[ ]`, `[-]`, `[>]`, `[x]`/`[X]`, unknown) under open, done,
  and cancelled owners;
- excluded descendants under dropped and carried ancestors;
- a hand-ticked `[x]` intermediate with pending descendants;
- duplicate texts in different branches, which must give distinct row keys;
- trailing `^anchor` capture and clean text;
- whole-token `#task` matching (`#tasks` and `#taskbar` are not tasks).

Bob-cli owns this file; the Obsidian twins copy it byte for byte. A behavior
change edits the contract and this fixture together.

## Shape

- `schema_version`: currently `1`.
- `contract`: the contract section this file pins.
- `max_depth`: mirrors `MAX_REQ_DEPTH`; writers refuse deeper reqs as
  `req_depth`.
- `cases[]`: `id`, `title`, `source_lines` (joined with `\n` plus a trailing
  newline), and `expect`.
- `expect.nodes[]` (forest order: owner line, then document order):
  `line`, `depth`, `parent`, `owner` (all 1-based; `parent` is `null` at
  depth 1), `mark` (`open`, `dropped`, `carried`, `legacy-met`, `unknown`),
  `state` (`pending`, `met`, `moot`, `dropped`, `carried`, `excluded`),
  `text` (checkbox and trailing `^anchor` removed), `anchor` (or `null`),
  `path` (dotted 1-based sibling ordinals), `subtree_end` (1-based,
  inclusive), `notes` (1-based inclusive ranges of directly attached note
  lines), `excluded_by`/`excluded_because` (only when excluded),
  `depth_exceeded` (only when true).
- `expect.probes[]` (optional): `line` (1-based) with the expected
  `owner` (1-based, or `null`) and `is_req`.
- `expect.assert_distinct_keys` (optional): every node row key (built with
  `owner_key`) must be distinct and shaped
  `<owner_key>/req/<path>~<8 hex>`.

# Layout and writer vectors

`layout.json` (`RL1`–`RL52`) and `writes.json` (`RW1`–`RW53`) pin the task
block layout module (`src/native/task_block_layout.rs`, contract
`docs/task-requirements.md` §3). bob-cli owns both files; each plugin copies
them byte for byte after updating bob-cli first. A behavior change edits the
contract and these fixtures together. Source notes are `source_lines` joined
with `\n` plus a trailing newline (`eol: "crlf"` joins with `\r\n`), and every
line number is 1-based.

## `layout.json`

`slots[]`, `violations[]`, `tidy[]`, and `note_homes[]` each hold cases with
`id`, `title`, `source_lines`, `task` (the task's line), and `expect`:

- `slots[]` adds `kind` (`cancel-log`, `managed-link`, `req`, `authored`,
  `drawer`, `notes-drawer`, `log`); `expect` is `{line, indent}`, the line to
  insert before (one past the last line appends) and the indentation.
- `violations[]`: `expect` lists `{line, kind}` with `kind` one of
  `out-of-zone`, `notes-not-first`, `note-outside-notes`, in document order.
- `tidy[]`: `expect` is `null` when the block is canonical (and for a task
  that is not open), else `{lines, moves, notes_filed, notes_created}`. Applying
  tidy to its own output must give `null`.
- `note_homes[]`: `expect` is `{home: "loose", line, indent}`,
  `{home: "notes", line}` (the existing `NOTES` drawer), or
  `{home: "new-notes"}`.

## `writes.json`

`date` is the day every Cancel Log entry carries. Each case adds `op` and its
inputs, plus an optional `preimage` that replaces the expected text of the
target line (a mismatch must refuse as `req_stale`):

- `drop`: `line`, `reason`. `expect`: `{lines, log, descendants}` with `log`
  one of `none`, `created`, `prepended`.
- `restore`: `line`. `expect`: `{lines, descendants}`.
- `toggle`: `line`. `expect`: `{lines, direction, line, depth, scooped,
  notes_created}` with `direction` `note-to-req` or `req-to-note`, and `line`
  the toggled item's line after the write.
- `insert`: `task`, `blocks` (each opens with `- [ ] text` and carries its own
  relative indentation). `expect`: `{lines, first_line, line_count, scooped,
  notes_created}`.

Every `expect` is either that object or `{refused: code}`. The `NOTES` drawer
is created only when a note lands in it (`notes_drawer_birth =
with_first_note`).
