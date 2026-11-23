# Task-split vectors

`vectors.json` is the single source of truth for split-bullet planners in
bob-cli and bob-plugins (`docs/task-split.md` §6). Its cases are `SP1`–`SP52`,
covering cursor, count, clamped count, line-range, non-contiguous `nth`, and
`match` selection; anchor reuse; silent owner mint; alias flattening (`|`,
`]]`, `#`, links, emphasis); 80-character truncation; ALL-CAPS drawers;
tab- and space-indented notes; daily-note routing to a root note and to the
inbox; section bullets routed to `## Tasks` (created when missing); disabled
mentions; every refusal code in §5.1; and requirement carry
(`docs/task-requirements.md` §6): `[>]` stubs, whole-branch moves with
rebased depth, the NOTES rule, move maps, the innermost-candidate rule, and
the `split_req_excluded` and `split_overlap` refusals.

bob-plugins copies this file byte-for-byte. A behavior change edits
bob-cli first; never hand-edit the copy.

## Provenance

Minted block IDs were computed with the real `mint_block_id`
(`src/native/capture_block_ids.rs`), via a throwaway `cargo test --lib`
probe printout, never by guessing. The alias strings follow §5.2
(flatten wikilinks/Markdown links, strip emphasis, `|` → `/`, drop `[`/`]`,
collapse whitespace, truncate at 80 characters on a word boundary with `…`).

## Section contracts

| Section | Pins |
| --- | --- |
| `cases` | input files, owner line, and selection → byte-exact postimage files, new task records, cursor line, and notice |
| `SP25`–`SP29` | refusal codes `split_already_task`, `split_stub`, `split_managed_line`, `split_not_splittable` with their messages |
| `SP5` | clamped counts report `requested` vs `split` |
| `SP10` | silent owner mint reported as `owner_minted_id` |
| `SP20`–`SP21`, `SP30` | cross-note placements reported as `root_note`, `inbox`, or `tasks_section` |
| `SP31` | punctuated `WHAT IS SASE?` drawer entry splits inside its drawer |
| `SP32`–`SP39`, `SP44`–`SP48`, `SP50` | carried requirements: `[>]` stub bytes, the new block's layout, `req_depth`, and the `moves` map (1-based `from` in the owner's note before, `to` in the task's note after) |
| `SP40`–`SP43`, `SP49` | requirement refusals `split_not_splittable` (dropped, carried), `split_req_excluded`, and `split_overlap` |
| `SP51` | a split with children lands directly below the owner's block when more follows |
| `SP52` | a bold note under a drawer remains splittable note content |

## Selection kinds

`cursor {line}`, `count {line, count}`, `lines {from, to}`, `nth {picks}`,
`match {texts}`, and `section {title}`. Requirement candidates nest, so
`cursor` and `lines` pick the innermost candidate holding each line, and
`count` walks siblings under one structural parent. `nth` picks are 1-based ordinals
among the owner's splittable bullets as listed; the planner always writes
new tasks in document order. `match` texts are literal substrings; the
vectors pin the unique-match path, and the CLI owns the ambiguous and
missing refusals.
