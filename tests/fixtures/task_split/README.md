# Task-split vectors

`vectors.json` is the single source of truth for split-bullet planners in
bob-cli and bob-plugins (`docs/task-split.md` §6). Its cases are `SP1`–`SP30`,
covering cursor, count, clamped count, line-range, non-contiguous `nth`, and
`match` selection; anchor reuse; silent owner mint; alias flattening (`|`,
`]]`, `#`, links, emphasis); 80-character truncation; ALL-CAPS drawers;
tab- and space-indented notes; daily-note routing to a root note and to the
inbox; section bullets routed to `## Tasks` (created when missing); disabled
mentions; and every refusal code in §5.1.

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

## Selection kinds

`cursor {line}`, `count {line, count}`, `lines {from, to}`, `nth {picks}`,
`match {texts}`, and `section {title}`. `nth` picks are 1-based ordinals
among the owner's splittable bullets as listed; the planner always writes
new tasks in document order. `match` texts are literal substrings; the
vectors pin the unique-match path, and the CLI owns the ambiguous and
missing refusals.
