# Requirement close-card vectors

`vectors.json` is the single source of truth for requirement groups on
Bob's managed Done successor card (`src/native/task_complete/req_close.rs`,
contract `docs/task-requirements.md` §7). Bob-cli owns this file; clients
copy it byte for byte. A behavior change edits the contract and this
fixture together.

The `rows` family (RV1–RV11) pins pre-close collection, snapshot-bound
owner and row keys, canonical group copy, historical folds, and
pending-descendant counts. The `choices` family (RV12–RV19) pins final
verdict conflicts, requirement-first card numbering, mark-all exclusion,
interactive folds, ancestor overlap, navigation-only pins, multi-owner
choices, stale replay after duplicate-label reordering, and duplicate
owner refusal. The `writes` family (RV20–RV28) pins atomic carry/drop bytes, receipts,
created-task identities, and descendant moves without adding another commit
path. Later close-engine phases append jump families to the same file
without renumbering these cases.

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

Row keys are opaque: `owner.key` + `/req/` + sibling path + `~` +
the first eight hex characters of sha256(clean text). Clients compare
the complete supplied key to a freshly enumerated forest.
