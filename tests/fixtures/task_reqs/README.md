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
