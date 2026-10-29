# Task relations

A task can relate to any other task with an optional label, from Bob
Mac Capture (`~` / `~~`) and from the Obsidian Task Card (`l` /
`⇧L`). In either client's relation picker, typing `~` anywhere in the
task filter flips Links to ↔ Linked from in place and keeps the filter
text. The relation is authored once on the source's
`🪢 **LINKS:**` line, and a generated `🪢 **LINKED FROM:**` line on
the target always converges to it. Live chips render both ends
correctly, and relations never block, promote, consume, schedule,
stamp, or route anything. The exceptions are the commit-time
cancel of [§17](#17-cancel-labels) and the commit-time complete of
[§18](#18-complete-labels).

This file is the contract every implementation cites. The Rust side is
`src/native/task_relations/` (LINKS and LINKED FROM parsers, label
rules, formatters, canonical sort, slot rules, and edit planners) plus
the `reconcile_relation_lines` pass in `bob task reconcile`. The
JavaScript twins are the relation grammar in bob-navigation-hotkeys,
the live relation index and chip model in bob-ledger-tools, and the
small recognisers in task-status-cycler and block-id-prompt. Each
implementation copies the vector tables it needs from this file into
its tests, citing the vector id. If an implementation disagrees with a
vector, fix this doc first in the same change.

## 1. Vocabulary

- **Task relation** (or **relation**): a directed, optionally
  labelled, inert edge from a **source** task S to a **target** task
  T. It is stored as one entry on S's LINKS line.
- **LINKS line**: the single managed direct-child line that holds a
  task's outgoing relations: `🪢 **LINKS:**`.
- **LINKED FROM line**: the single generated direct-child line that
  mirrors the relations pointing at a task:
  `🪢 **LINKED FROM:**`. Reconcile owns it; nothing reads it back as
  input.
- **Relation label** (or **label**): optional free text phrased from
  the source's side, such as `follow-up of`. Writers trim it,
  collapse inner whitespace, strip one trailing `:`, and keep the
  1–40 character result.
- **Relationship modifiers**: capture's existing umbrella for
  task-to-task modifiers. It covers `&` (depends on), `*` (blocks),
  `~` (links to), and `~~` (linked from). The existing JSON names
  (`picker.relationship`, `dependency_target`,
  `relationship_needs_text`, mode `task_dependency`) keep their
  spelling and now cover all four.
- **Cancel label**: a relation label listed in
  `relations.cancel_labels`. Committing one through an explicit
  relation gesture cancels the relation's source exactly as Task
  Card Cancel would ([§17](#17-cancel-labels)).
- **Complete label**: a relation label listed in
  `relations.complete_labels`. Committing one through an explicit
  relation gesture marks the relation's source Done exactly as that
  surface's managed Done gesture would ([§18](#18-complete-labels)).
- **Closing label**: a cancel label or a complete label. One label
  carries at most one effect.
- **Task Link** keeps its glossary meaning: a block link to an
  Obsidian task. Entries on a LINKS line are block links to tasks,
  but they are **never** Pomodoro Task Links.

Navigation has two symmetric writers: the Links stage (writes S's
LINKS line) and the Linked-from stage (writes other tasks' LINKS
lines so they point here). Neither reads a LINKED FROM line as input.

## 2. What relations never do

A relation write, a relation line, or a relation gesture never:

- blocks, unblocks, promotes, releases, or consumes anything; never
  flips a checkbox to `[?]`, and never recovers one;
- counts as a Pomodoro Task Link, a plan-budget item, a completable
  sighting, a successor anchor, or a successor offer. Ctrl+Shift+Enter,
  `!`, and tree close never act on a relation line;
- changes a status, lane, `[fresh::]`, `[keeps::]`, `[unblocked::]`,
  schedule, priority, `[id::]`, or `[dependsOn::]`. The only task-line
  edit a relation write may make is appending a minted `^block-id`;
- counts as a review answer, advances the `]s` walk, becomes a walk
  tier, or triggers inbox routing;
- disappears because either end closes, is cancelled, reopens, or is
  archived. Relations are history;
- except the commit-time cancel of [§17](#17-cancel-labels), which
  may cancel the source, prune its links from today's open
  Pomodoros, and recover blocked dependents, and the commit-time
  complete of [§18](#18-complete-labels), which may mark the source
  Done, retire its links from today's open Pomodoros, recover
  blocked dependents, and offer successors.

A link that needs to block is a dependency (`&`, `b`).

## 3. Grammar

Example:

```markdown
- [ ] #task Fix flaky capture retries ^fix-retries
  - 🪢 **LINKS:** follow-up of [[done/bob_done#^capture-star]] • [[sase#^retry-design]]
```

```text
<indent>- 🪢 **LINKS:** <entry> • <entry> • …
<entry>        := [<label> ' ']<link>
<indent>- 🪢 **LINKED FROM:** <mirror-entry> • <mirror-entry> • …
<mirror-entry> := <link>[' (' <label> ')']
<link>         := [[#^id]] | [[basename#^id]] | [[dir/note#^id]]
```

### 3.1 Writer form

- Starts with `🪢` (U+1FAA2, one code point, no VS16), one space, the
  bold label, and one space. Entries are joined by `•` (U+2022, one
  space on each side).
- No aliases, embeds, strikes, or duplicate targets.
- A line that would hold no entries is deleted, with no placeholder.

### 3.2 Labels

A label reads "S ‹label› T". Writers trim it and collapse whitespace
runs to one space, strip a trailing `:`, and require 1–40 characters.

- A label may not contain `[`, `]`, `(`, `)`, `"`, `\`, `|`, `•`,
  `*`, `~`, or a control character.
- It may not start with `·` or `,`.
- Case is preserved. Suggestions dedupe case-insensitively.
- Readers accept any length that obeys the character rules.

### 3.3 Parse algorithm

Run the shared `block_link_spans` scanner first and never split on
separators, because an alias can contain one.

1. Find the block links first (the shared link scanner already
   handles aliases, `!`, and `~~`), then check what surrounds them.
2. After each link, skip one separator (`•`, `·`, or `,`) and
   whitespace. The text up to the next link is that entry's label.
3. A struck (`~~[[…]]~~`) or embedded (`![[…]]`) link is accepted and
   canonicalised.
4. An aliased link's alias becomes the label only when no label
   precedes it.
5. On LINKED FROM, a `(label)` after a link is its label.

### 3.4 Reader tolerance

Writers canonicalise all of these:

- the glyph is missing, or is `🔗`;
- the authored label is `LINKS` or `LINKS TO`; the mirror label is
  `LINKED FROM` or `BACKLINKS`. Labels must be bold and uppercase;
- the separator is `·`, `,`, or whitespace instead of `•`;
- the line is in any direct-child slot.

### 3.5 Not a line

- Non-bold `LINKS:` (the 660 `ref/` note links are plain text);
- lowercase labels, and `RELATED` / `SEE ALSO`;
- anything that is not a direct child of a `#task`;
- fenced, nested, blockquoted, or marker-less lines.

### 3.6 Malformed

- Text after the last link;
- a label that breaks the character rules;
- a half-typed link, a bare note link, or a heading link;
- separators only.

A malformed line is left untouched and warned about
(`malformed_links_line` / `malformed_linked_from_line`). Writers never
insert a second line beside one. An authored write to a malformed
LINKS line is refused with "its LINKS line is malformed".

### 3.7 Cross-parse

The Depends-On, BLOCKS, LINKS, and LINKED FROM parsers each return
not-a-line for the others' lines (LK19, MC15).

## 4. Slot and order

```markdown
- [ ] #task T ^t
  - ❌ **CANCEL LOG**        (first, if present)
  - ⛓️ **DEPENDS ON:** …     upstream
  - 🧱 **BLOCKS:** …         downstream
  - 🧩 **Sub-projects:** …   structure (^prj only)
  - 🪢 **LINKS:** …          what T relates to (authored)
  - 🪢 **LINKED FROM:** …    what relates to T (generated)
  - user children, 🗓️ SCHEDULE LOG, 🛠️ WORK LOG …
```

- **Slots.** LINKS goes after the last direct-child subtree of Cancel
  Log, Depends-On, BLOCKS, or Sub-projects. Otherwise it is the first
  direct child (the second after a first-child Cancel Log). LINKED
  FROM goes after the LINKS subtree when one exists, otherwise in the
  LINKS slot.
  - Reuse the task's existing child indent, otherwise use the parent
    indent plus one tab.
  - Preserve CRLF and the final-newline state.
- **Existing lines are never moved.** A Depends-On, BLOCKS, or
  Sub-projects line inserted later lands in its own earlier slot, so
  the order still reads upstream, downstream, structure, out, then
  in. `bob projects sync`'s anchor rule (after Depends-On and BLOCKS)
  is unchanged.
- **LINKS order is authored.** New entries are appended. Re-linking an
  existing target with a different label relabels it in place; with
  the same label it is a no-op.
- **LINKED FROM order is canonical.** Sort by canonical link text
  after ASCII case-folding, comparing Unicode code points, with raw
  code-point order as the tie-break: exactly BK21. Same-note entries
  therefore come first. JS compares code points, never UTF-16 units.
- **Duplicates.** A writer that touches a task with two LINKS lines
  merges the later line's entries into the first (appended, first
  label wins) and deletes the later one. On duplicate targets within
  one line, the first entry wins. Reconcile keeps the first LINKED
  FROM line and deletes the rest.

## 5. Identity and minting

- **Edge identity is (S, T),** each resolved by vault-relative path
  plus block id. A source holds at most one entry per target. A → B
  and B → A are independent edges, and cycles are allowed.
  Self-relations and non-task targets are refused.
- **The target needs a `^block-id`.** Pickers show `＋ id` and run
  the existing new-block-ID step (nav `showBlockIdStage`; capture
  `requires_block_id` / `block_id_suggestions` plus
  `capture-task-id`). Completion, dry-run, and Escape mint nothing. An
  id-less archived target is disabled ("archived tasks can't get an
  id").
- **The source needs a `^block-id` only for the mirror.** It is
  minted silently with `mint_block_id(description, used)` (§11.7 of
  the dependency contract), once, inside the same transaction or
  batch, exactly like BLOCKS dependents (§14.4).
  - No `[id::]` is ever added.
  - When `relations.linked_from_lines` is false, no source is minted.
  - In `~~`, the new task is the _target_, so it is always minted.
- **Never emit a link to an id that will not be on disk after the
  write.** Reconcile skips listing a source whose mint would land in a
  note that is quiet-deferred this run (BR5).
- **Link form.** Use `canonical_link` from the owning note:
  `[[#^id]]`, then `[[basename#^id]]`, then `[[dir/note#^id]]`. A
  `done/` source or target always keeps the explicit
  `[[done/…#^id]]` form, the only form the hooks resolve into
  archives.
- **A broken link is never rebound** by title or by a reused id.
  Supported moves (`bob task move`, Ctrl+Shift+M, `bob task archive`,
  and Obsidian renames) already rewrite links on every line.

## 6. The LINKED FROM projection

```text
sources   = #task lines with a LINKS line in: active notes (writable)
            ∪ the previous daily note (read-only) ∪ every .md under done/ (read-only)
edges     = { (S, T, label) : an entry on S's LINKS resolves to #task T ∧ T ≠ S }
            (unresolved, non-task, self, and malformed are warned and omitted; never pruned from LINKS)
mirror(T) = sort_canonical({ (S, label) : (S, T, label) ∈ edges })
T owns a LINKED FROM line ⇔ mirror(T) ≠ ∅ ∧ T is writable (not done/, not the previous daily note)
```

- **Open or closed T** gets a maintained mirror. Unlike BLOCKS, it
  never freezes.
- **Never prune on uncertainty.** If S's LINKS line is malformed, or
  S's note has duplicate block ids, existing LINKED FROM entries
  naming S are kept and S is never newly added.
- **The `done/` read is narrow.** Byte-prefilter every `done/**/*.md`
  for `LINKS`, parse only those files, and never write there during
  reconcile. The explicit rename exception below changes label spans
  only.
  Document it beside the exact-`done/`-target exception in
  `docs/task-status-hooks.md`.
- **Rename exception (LR7).** `bob relations rename` rewrites matching
  label spans in `LINKS` and `LINKED FROM`, including `done/` and the
  previous daily note. It preserves each edge and every other byte;
  no other relation writer can change those notes. A reversed rename
  ([RV10](#rv10-reversing-a-label), planned, phase `reverse-rename`)
  may move an edge's entry between the two ends' LINKS lines,
  including in `done/`. It still never changes a status, a field, or
  another byte.

## 7. Reconciliation

Reconcile is the authority for LINKED FROM. Gestures and capture make
early courtesy writes, and closes, vim edits, agent edits, moves, and
failed follow-ups are repaired by reconcile.

1. **New pass `reconcile_relation_lines`** in
   `task_status_hooks/relations.rs`. It runs right after
   `reconcile_blocks_lines`, over the re-parsed text, with the same
   structure.
2. **Owners to check:** every T with mirror(T) ≠ ∅, plus every
   writable task that already has a LINKED FROM child.
   Byte-prefilter for `LINKED FROM` / `BACKLINKS`; never walk every
   task's children.
3. **Steps for each owner:** mint missing source ids, render against
   the post-mint state, then call the cli-core mirror planner with
   the desired entries:

   | Existing line | Result |
   | --- | --- |
   | missing | `inserted` |
   | different entries or labels | `rewritten` |
   | same entries, non-canonical | `canonicalized` |
   | not desired | `removed` |
   | duplicates | keep the first (`deduplicated`) |
   | any malformed LINKED FROM child | leave every LINKED FROM child of that owner alone and warn |

4. **Guarded output.** The edits are structural. They join the
   reconciled set, so the 2 s quiet interval, preimage checks,
   recovery snapshots, and `pre_reconcile` originals all apply.
5. **Reconcile never edits a LINKS line.** It only reads, warns, and
   mints.

## 8. Immediate writers

| Writer | Behaviour |
| --- | --- |
| capture `~` / `~~` (CLI and Bob Mac Capture through bob) | S's LINKS edit, any minted ids, and T's LINKED FROM edit go into **one staged batch** via the cli-core delta planner. A skipped mirror (archived or previous-daily T, a malformed mirror, or the kill switch) is reported, never fatal. |
| nav Links / Linked-from stages, chip ×, Ctrl+D, api `removeLink` | Same note: one transaction, one Ctrl+Z. Cross note: S commits first, then a best-effort follow-up per touched T (open editor with the relations machine-write annotation, else a preimage-checked `vault.process`). On failure the notice gains `· 🪢 LINKED FROM not updated`, and nothing rolls back. |
| nav hand-edit mirror (cursor leaves an edited LINKS line; undo re-arms) | Diff the burst baseline's (target, label) set against the final set, then mirror the difference as follow-ups. |
| Hand edit of LINKED FROM | Never an edge change. A notice appears once per burst, and reconcile restores the line. |
| Ctrl+Shift+M, `bob task move`, `bob task archive`, Obsidian rename | The LINKS line moves with its task, and generic link rebasing applies. Reconcile re-sorts and repairs the mirrors. |
| Closes, cancels, reopens | None. Lifecycle events never write or remove a relation. |
| Cancel-label commit | [§17](#17-cancel-labels). An explicit add or relabel into a cancel label cancels the source as Task Card Cancel would. |
| Complete-label commit | [§18](#18-complete-labels). An explicit add or relabel into a complete label marks the source Done as that surface's managed Done gesture would. |

All writers share one planner, implemented in Rust with a JS twin and
pinned by the same LW vectors. Writers compare canonical output
before writing, so a no-op writes nothing and adds no undo entry.
Writes stamp nothing.

## 9. Gestures on relation lines

| Gesture | LINKS line | LINKED FROM line |
| --- | --- | --- |
| Ctrl+Shift+P | Opens the owner's Links stage | Opens the owner's Linked-from stage |
| Ctrl+Shift+L / Ctrl+Shift+Alt+L | Act on the owning task, never the chip under the cursor | Same |
| `!` / `N!`, Ctrl+Shift+Enter | Refused: `🪢 LINKS entries are relations, not Task Links · edit them with Ctrl+Shift+L` | Refused with the generated-line notice |
| Ctrl+Enter / Alt+] / Alt+[ on a link | Acts on the linked task root-only (no strike, re-embed, tree close, or reformat), as on dependency lines | Same |
| Hand edit | Mirrored when the cursor leaves the line | `🪢 LINKED FROM is generated from other tasks' LINKS lines · change it with Ctrl+Shift+Alt+L` once per burst |
| Ctrl+D on the Task Card row | Links to: removes every outgoing link and its mirror entries | Linked from: refused with the generated-line notice |

On a dedicated Task Link bullet, Ctrl+Shift+L / Ctrl+Shift+Alt+L act on the linked task, opening its note first (LG18).

## 10. Chips

Chips render a live relation index (§11), never the stored mirror.

- **Anatomy.** Reuse the `bob-dep-row` / `bob-dep-chip` family: a
  mini status checkbox from `--task-status-*`, about 40 characters of
  description, `↗ note` for cross-note entries, a hover preview, and
  click / Mod-click. The row accent stays neutral. **Colour carries
  status, never direction.** Closed and archived targets are dimmed
  and never struck. `#hide` tasks are muted. There is no new palette,
  and no `frees` / `waiting on` badges.
- **Labels** are a muted small-caps pill on the arrow side.
  - LINKS: `‹label› → (chip)`.
  - LINKED FROM: `‹inverse› (chip)` for a known label (the inverse
    table below), otherwise `(chip) · ‹label› → this`.
  - Rows with labels wrap one entry per visual row. Unlabelled rows
    wrap between chips. Chips never change line height.
- **Stale and broken.** A stored LINKED FROM entry that is no longer
  live is dashed, with the tooltip `No longer links here · refreshes
  on the next reconcile`. A live edge missing from the text is
  appended in canonical order. A LINKS entry whose target cannot be
  resolved is a dashed `⚠ ^id not found`.
- **Collapse.** Three or more closed chips on a row collapse to
  `✓×N`, which expands on click.
- **Controls** (only when nav `api.relations?.version >= 1`):
  - a trailing ＋ on LINKS (aria "Link this task to another") opens
    the Links stage;
  - a trailing ＋ on LINKED FROM (aria "Choose a task that links
    here") opens the Linked-from stage;
  - a hover × on each live LINKS chip (aria "Unlink ‹name›") removes
    the link;
  - a hover × on each live LINKED FROM chip (aria "Remove the link
    from ‹name›") calls `removeLink(S, T)`. The notice names S's note.
- **Toggle.** `toggle-dependency-chips` covers both relation rows too.
- **Without the plugin**, both lines still read as English.

**Inverse phrases** (display only; case-insensitive match; never
stored):

| Stored label | Shown on LINKED FROM |
| --- | --- |
| `merged into` | `merged from` |
| `duplicate of` | `duplicated by` |
| `follow-up of` | `followed up by` |
| `superseded by` | `supersedes` |
| `split from` | `split into` |
| `implemented by` | `implements` |
| `verified by` | `verifies` |

The nav Linked-from stage's CURRENT rows use the same table.

## 11. The live relation index

bob-ledger-tools keeps a raw-text relation index and publishes it as
`api.relationIndex` v1:

- **Startup scan.** A chunked, generation-guarded scan with a cheap
  prefilter: skip files whose metadata cache has no `#^` block links,
  then text-prefilter for `🪢`, `**LINKS`, `**LINKED FROM`, or
  `**BACKLINKS`. The scan includes `done/`, as read-only sources.
- **Updates.** Reparse per file on `metadataCache.changed`, and handle
  vault `delete` and `rename`. Overlay the active editor's unsaved
  text, so a just-typed entry shows at the target before save.
- **Publishing.** Forward (`S → [(T, label)]`) and reverse
  (`T → [(S, label)]`) maps are published together under one
  generation, so a count and its rows never disagree. Targets resolve
  by `getFirstLinkpathDest` plus the file's cached block ids. Status
  and text come from the Tasks memo, falling back to the cached line
  for notes the memo omits (`done/`).
- **API.** `api.relationIndex` v1 exposes `version`, `generation`,
  `ready()`, `outgoing(path, blockId|line)`, `incoming(path,
  blockId)`, and `labelSuggestions(prefix?) → [{label, count}]`
  (case-insensitive dedupe, count order). Chips subscribe through
  `onChange`.

## 12. Kill switch, outputs, and warnings

```yaml
relations:
  linked_from_lines: true # write 🪢 LINKED FROM mirrors (task-relations.md)
```

Rust and nav read it from `~/.config/bob/config.yml`
(`BOB_CONFIG_FILE` / `XDG_CONFIG_HOME`). An invalid value falls back
to `true` and warns.

- With `false`, reconcile removes every LINKED FROM line on writable
  tasks, mints nothing, and leaves LINKS alone, so the switch is
  lossless.
- Gestures and capture write no mirror and mint no source.

**`bob task reconcile` JSON** gains `linked_from_line_updates`,
`minted_relation_source_block_ids`, and `relation_warnings[]`
(`{kind, path, line, message}`), where `kind` is one of
`malformed_links_line`, `malformed_linked_from_line`,
`unresolved_relation_link`, `relation_target_not_task`,
`self_relation`, or `duplicate_block_id`. Human output gets a `🪢`
group in the dependency style, and `--dry-run` previews all of it.

**Capture JSON** (every `schema_version` stays 1; additive only):

- `capture-parse`: span kinds `relation_sigil`, `relation_note`,
  `relation_block_id`, `relation_label_separator` (covering both pipes
  for `||`), and `relation_label`; top-level and per-item `relations[]`
  (`{raw, direction: "to"|"from", note, block_id, quoted, hidden, label,
  label_range, range}`, with `hidden` only for the `||` spelling),
  omitted when empty; needs `relation_target` and `relation_label`
  (an empty `||` probe is the `relation_label` need); top-level
  `relation_grammar: 1` whenever the draft contains a `~` byte.
  Drafts without `~` are byte-identical to today, as is every `|`
  spelling. Relations never appear in `dependencies[]` or
  `blocked_tasks[]`. `~note:id|||label` is an `invalid_relation`
  teaching `use || to hide a label, | to name one`.
- `capture-complete`: contexts `relation_to` and `relation_from`
  (the `DependencyCandidate` shape, plus `already_linked`,
  `existing_label`, `also_prerequisite`, and `disabled_reason`);
  context `relation_label` with catalog-backed `suggestions[]` (visible rows
  `{label, key, count, replacement, hide_replacement, inverse?}` and a leading
  typed row `{label, key, replacement, hide_replacement, typed: true,
  existing?, count?, hidden?, disabled_reason?}`); hidden rows are omitted
  except for an exact typed match. A `||` probe keeps its pipe count in
  `replacement` and gains `one_off: true`, with `hide_replacement` always
  the `||` form. The response includes `hidden_count` in this context.
  Typed key matches use the catalog display spelling. Shell
  completion gains `links to` / `linked from` groups.
- `capture-task-id`: `relation_to_replacement` (`~note:id`) and
  `relation_from_replacement` (`~~note:id`).
- `capture` / `--dry-run`, per item: `relation_updates[]`
  (`{direction, source, target, label, previous_label, action:
  added|relabeled|unchanged}`), `linked_from_line_updates[]`,
  `minted_relation_block_ids[]`, `mirror_skipped[]`
  (`{reason: archived_target|previous_daily_target|malformed_line|disabled}`),
  and `relation_label_hides[]` (`{label, key, existing_count, was_hidden,
  action: hide|already_hidden}`), plus a `hides_existing_relation_label`
  warning object (`{code, label, count, message}`) when a one-off hides a
  visible existing label and a `relation_label_hide_failed` object
  (`{code, message}`) when the journal append fails.

## 13. Guarantees

1. **Single source (LR11).** Delete every LINKED FROM line and run
   reconcile once: the lines come back byte-identical, and a second
   run changes nothing.
2. **Inertness (LR18, LX13, LG13).** Adding, removing, or relabelling
   any relation leaves every status, lane, field, schedule, Pomodoro
   section, and Depends-On / BLOCKS line byte-identical, apart from a
   minted `^id`. The carve-outs are the commit-time cancel of
   [§17](#17-cancel-labels) and the commit-time complete of
   [§18](#18-complete-labels).
3. **Engine agreement (LW14, LK\*).** Rust and JS agree on every LK,
   LW, and LM vector, malformed cases included.
4. **Live view (LC8).** Chips render from the live index, never from
   the stored mirror.
5. **Convergence (LR3).** Canonical sorting makes two machines produce
   the same bytes, and the next reconcile heals any textual conflict.
6. **Read-only respect (LR7).** Relation writes leave `done/` and the
   previous daily note alone, except `bob relations rename`, which
   rewrites relation-label spans only and preserves every other byte.
   A reversed rename may move an edge between the two ends' LINKS
   lines, including in `done/`, and still never changes a status.

## 14. Lifecycle

- **Close, cancel, reopen S or T.** None. Relations are history;
  nothing changes.
- **Relabel away from a cancel label, or unlink.** S stays
  Cancelled. Nothing ever reopens because of a label
  ([§17](#17-cancel-labels)).
- **Relabel away from a complete label, or unlink.** S stays Done.
  Nothing ever reopens because of a label
  ([§18](#18-complete-labels)).
- **Archive S.** Link repair rewrites T's mirror entry to the
  `[[done/…#^s]]` form; reconcile keeps it unchanged (LR22).
- **Archive T.** No mirror is written in `done/`; chips show the live
  view only.
- **Move or rename S or T.** Generic link rebasing applies on every
  line; reconcile re-sorts and repairs the mirrors (LR10).
- **T deleted.** The LINKS entry stays, warned as
  `unresolved_relation_link`, and chips show `⚠ ^id not found`. It is
  never auto-removed.
- **S deleted.** Its edges vanish with its note; the next reconcile
  removes the orphaned mirror entries.
- **Duplicate ids.** A source whose note carries a duplicate `^s` is
  not listed, with a `duplicate_block_id` warning (LR23).
- **Raw cut-and-paste.** A pasted LINKS line is authored data and is
  kept; a pasted LINKED FROM line is regenerated on the next
  reconcile.
- **Adopting old idioms.** Hand-written `Merged into [[…]]`, `See
  [[…]]`, and `**RELATED:**` bullets are not adopted; adopting them is
  an opt-in follow-up.

## 15. Conformance vectors

### LK — parse

| # | Line (direct child of a `#task` unless stated) | Verdict |
| --- | --- | --- |
| LK1 | `🪢 **LINKS:** [[#^a]]` | accept: 1 entry, no label, canonical |
| LK2 | `🪢 **LINKS:** follow-up of [[done/bob_done#^cap]] • [[sase#^retry]]` | accept: labels `follow-up of`, none; canonical |
| LK3 | `🪢 **LINKS:** see [[#^a]] · merged into [[b#^c]]` and the `,` / whitespace-only separator forms | accept 2; canonicalise separators |
| LK4 | `**LINKS:** [[#^a]]`, `🔗 **LINKS:** [[#^a]]`, `🪢 **LINKS TO:** [[#^a]]` | accept; canonicalise glyph and label |
| LK5 | `🪢 **LINKS:** [[#^a\|merged into]]` | accept; alias adopted → `merged into [[#^a]]` |
| LK6 | `🪢 **LINKS:** see [[#^a\|x]]` | label `see`; alias dropped |
| LK7 | `🪢 **LINKS:** ~~[[#^a]]~~`, `🪢 **LINKS:** ![[#^a]]` | accept; canonicalise |
| LK8 | `🪢 **LINKS:** see [[#^a]] • see also [[#^a]]` | accept; the canonical form keeps the first entry |
| LK9 | `🪢 **LINKS:** see:   also [[#^a]]` | label `see: also`; whitespace collapsed (`:` stripped only when trailing) |
| LK10 | `🪢 **LINKS:**` | empty |
| LK11 | `🪢 **LINKS:** [[#^a]] until Friday` | malformed (trailing prose) |
| LK12 | `🪢 **LINKS:** [[#^a`, `[[note]]`, `[[note#Heading]]`, `• ,` | malformed |
| LK13 | `🪢 **LINKS:** see (old) [[#^a]]`, `"quoted" [[#^a]]` | malformed (label characters) |
| LK14 | `- LINKS: [[ref#^a]]`, `**links:** [[#^a]]`, `**RELATED:** [[#^a]]`, `**SEE ALSO:** [[#^a]]` | not-a-line |
| LK15 | LK1 fenced, as a grandchild, blockquoted, without a list marker, or under a non-task | not-a-line |
| LK16 | `🪢 **LINKED FROM:** [[bob#^fix]] (follow-up of) • [[#^b]]` | accept 2 with labels |
| LK17 | `**BACKLINKS:** [[#^b]]` | accept as LINKED FROM; canonicalise |
| LK18 | `🪢 **LINKED FROM:** [[#^b]] extra`, `[[#^b]] (a) (b)` | malformed |
| LK19 | each relation line through the Depends-On, BLOCKS, and the other relation parser, and DP1 / BK1 through both relation parsers | not-a-line both ways |
| LK20 | mirror entries `[[zeta#^e]]`, `[[élan#^d]]`, `[[cash#^c]]`, `[[Cash#^a]]`, `[[#^b]]` | sorted `[[#^b]] • [[Cash#^a]] • [[cash#^c]] • [[zeta#^e]] • [[élan#^d]]` |

### LW — planner writes (LINKS and LINKED FROM)

| # | Situation | Outcome |
| --- | --- | --- |
| LW1 | S has no LINKS; add T, unlabelled | `🪢 **LINKS:** [[T]]` inserted in the §4 slot |
| LW2 | add T with label `see` to an existing line | ` • see [[T]]` appended |
| LW3 | re-add T with the same label | no-op (`unchanged`); byte-identical |
| LW4 | re-add T with a new label | relabeled in place; order kept |
| LW5 | remove the only entry | line deleted |
| LW6 | remove a middle entry | line rewritten; the others keep their order |
| LW7 | Cancel Log, Depends-On (nested child), BLOCKS, Sub-projects present | LINKS after the Sub-projects subtree; LINKED FROM right after LINKS |
| LW8 | no managed children; user child and Schedule Log present | LINKS first; existing children untouched |
| LW9 | two LINKS lines | merged into the first (append, first label wins); the second deleted |
| LW10 | malformed LINKS line | authored write refused: "its LINKS line is malformed"; nothing written |
| LW11 | label ` Follow-up  of:` / 41 characters / `see (old)` | `Follow-up of` / refused (too long) / refused (character) |
| LW12 | CRLF note without a final newline; a task using a 4-space child indent | preserved; indent reused |
| LW13 | target in a unique-basename note / an ambiguous basename / `done/` | `[[note#^t]]` / `[[dir/note#^t]]` / `[[done/x_done#^t]]` |
| LW14 | the same input rendered by Rust and nav | byte-identical lines |

### LR — reconcile (mirror)

| # | Situation | Outcome |
| --- | --- | --- |
| LR1 | S → T in the same note; T has no mirror | `🪢 **LINKED FROM:** [[#^s]]` (`inserted`) |
| LR2 | labelled edge | `[[#^s]] (follow-up of)` |
| LR3 | sources in the same note, another note, and `done/` | one line sorted per LK20; the `done/` source as `[[done/…#^s]]` |
| LR4 | S has no `^id` | S gains a minted `^id` (no `[id::]`, no `[fresh::]`); T lists it; a `minted_relation_source_block_ids` row |
| LR5 | S needs a mint and S's note is quiet-deferred | S not listed this run; the next run mints and lists it; then zero changes |
| LR6 | T closed in an active note | mirror maintained |
| LR7 | T in `done/` or the previous daily note | never written |
| LR8 | S in `done/` / in the previous daily note | listed |
| LR9 | hand-added mirror entry with no edge / hand-deleted entry | removed / restored (`rewritten`) |
| LR10 | renamed note leaves the mirror unsorted | `canonicalized` |
| LR11 | delete every LINKED FROM line in a reconciled vault | one run rebuilds identical bytes; a second run makes zero changes |
| LR12 | malformed LINKED FROM under T | every LINKED FROM child of T untouched; warn |
| LR13 | S's LINKS malformed | existing entries naming S kept; S never newly added; warn |
| LR14 | LINKS entry unresolved / non-task / self | no entry; matching warning; LINKS untouched |
| LR15 | two LINKED FROM lines | first rewritten, the rest deleted (`deduplicated`) |
| LR16 | `relations.linked_from_lines: false`, then `true` | every LINKED FROM on writable tasks removed, no mints; then rebuilt |
| LR17 | invalid `relations.linked_from_lines` | behaves as `true`; stderr warning |
| LR18 | relation added / removed / relabelled between runs | only LINKED FROM lines and minted ids differ; an open source whose LINKS already carries a cancel label stays open (LR25); an open source whose LINKS already carries a complete label stays open (LR26) |
| LR25 | reconcile an open source whose LINKS carries a configured cancel label | status, checkbox, and `[cancelled::]` byte-identical; reconcile never cancels |
| LR26 | reconcile an open source whose LINKS carries a configured complete label | status, checkbox, and `[completion::]` byte-identical; reconcile never completes |
| LR19 | A → B and B → A | each lists the other |
| LR20 | `--dry-run -f json` | identical rows; nothing written; no mints persisted |
| LR21 | owner's note modified less than 2 s ago | edits deferred |
| LR22 | S archived by `bob task archive` | T's entry already rewritten to `[[done/…#^s]]` by link repair; reconcile keeps it unchanged |
| LR23 | S's note has a duplicate `^s` | S not listed; `duplicate_block_id` warning |
| LR24 | target is a `^prj` task | the `^prj` line gets the mirror like any task |

### LX — capture execution

| # | Draft or situation | Outcome |
| --- | --- | --- |
| LX1 | `Fix retries ~bob_done:capture-star\|"follow-up of"` | new N with `🪢 **LINKS:** follow-up of [[done/bob_done#^capture-star]]`; `mirror_skipped` `archived_target`; N not minted for a mirror |
| LX2 | `Write outline ~~sase_blog:prj\|"next step"` | `sase_blog`'s `^prj` LINKS gains `next step [[mac_inbox#^write-outline]]`; N minted; N born with `🪢 **LINKED FROM:** [[sase_blog#^prj]] (next step)` |
| LX3 | `@bob+fix-retries ~sase:retry-design\|see`, then a rerun, then a new label | mirror updated in the same batch; rerun `unchanged`; new label `relabeled` |
| LX4 | `Ask plumber &job:parts ~job:cleanup` | `&` blocks as today; `~` changes no status |
| LX5 | `N ~T`, T in an active note (today's daily note included) | T's mirror written in the same batch; N minted |
| LX6 | kill switch off | no mirror and no source mint; `~~` still mints N |
| LX7 | `~~` target in a daily note, `done/`, `_templates`, or fenced code | refused; vault byte-identical |
| LX8 | self target, non-task target, missing block id | refused with a teaching error |
| LX9 | S's LINKS line malformed | refused; vault byte-identical |
| LX10 | T's LINKED FROM malformed | capture succeeds; `mirror_skipped` `malformed_line` and a warning |
| LX11 | `x ~job:cleanup` | `operator_relation_error` |
| LX12 | any LX vector, dry run vs live | identical JSON apart from `dry_run` |
| LX13 | any relation capture | no `[fresh::]`, lane, status, or Pomodoro change on S or T, except a [§17](#17-cancel-labels) cancel-label commit or a [§18](#18-complete-labels) complete-label commit |
| LX14 | `N ~T1 ~T2 ~T1` / `N ~T1\|a ~T1\|b` | T1 then T2, duplicate no-op / refused |
| LX15 | `N ~T ~~T` | two independent edges |
| LX16 | a same-note S and T | both edits composed against staged text; one write of the note |

### LP — capture parse

| # | Draft | Reading |
| --- | --- | --- |
| LP1 | `Fix it ~work:foo` / `~work:foo Fix it` / `@cash ~work:foo Fix it` | relation `to` (trailing / leading / past `@cash`) |
| LP2 | `Fix it ~~work:foo` | relation `from` |
| LP3 | `Fix ~work:foo now` | literal (mid-line) |
| LP4 | `Plan trip ~5m`, `~2h`, `~5pm`, `Clean ~/Downloads` | literal |
| LP5 | `Read ~~old~~ notes`, `Read ~~old~~` | literal (strike guard) |
| LP6 | `Fix it ~`, `Fix it ~~`, `Fix it ~wo`, `Fix it ~work:` | need `relation_target` |
| LP7 | `Fix it ~work:foo\|see`, `Fix it ~work:foo\|"follow up of"` | label spans and label |
| LP8 | `Fix it ~work:foo\|`, `Fix it ~work:foo\|"follow up` | need `relation_label` |
| LP9 | `Fix it ~work:foo\|"a (b)"` | invalid label with a teaching message |
| LP10 | `Fix it \~work:foo` | literal `~work:foo` |
| LP11 | `=~2`, `x~1` / `x~~1`, `=~~2` | unchanged drop lists / error "use one `~` list" |
| LP12 | `~work:foo` alone | need `dependency_target` |
| LP13 | `x ~work:foo` | `operator_relation_error` |
| LP14 | `Fix ~~~work:foo` | invalid |
| LP15 | `` Fix `~work:foo` ``, `Fix [[~work:foo]]` | never scanned |
| LP16 | `Meet ~noon` | need `relation_target`; submit refuses with the `\~` teaching error |
| LP17 | `@bob+fix ~sase:retry` | mode `task_dependency`; no task text needed |
| LP18 | any draft without `~` | byte-identical JSON (no `relation_grammar`) |

### LD — double-pipe one-off labels

The executable cases live in `tests/fixtures/relation_labels/vectors.json`
(`grammar`, `rust_only`).

| # | Draft | Reading |
| --- | --- | --- |
| LD1 | `Fix trip ~tasks:target\|see` | complete, `hidden: false`, separator `[22, 23)` |
| LD2 | `Fix trip ~tasks:target\|\|see` | complete, `hidden: true`, separator `[22, 24)` |
| LD3 | `Fix trip ~~tasks:source\|\|see` | complete `from`, `hidden: true` |
| LD4 | `Fix trip ~tasks:target\|\|"follow-up of"` | quoted label, `hidden: true` |
| LD5 | `Fix trip ~tasks:target\|"follow-up of"` | quoted label, `hidden: false` |
| LD6 | `Fix trip ~tasks:target\|\|` | need `relation_label`, partial hides |
| LD7 | `Fix trip ~tasks:target\|` | need `relation_label` |
| LD8 | `Fix trip ~tasks:target\|\|\|see` | invalid: `use \|\| to hide a label, \| to name one` |
| LD9 | `Fix trip ~tasks:target\|\|\|` | invalid: same teaching error |
| LD10 | `Fix 🪢 trip ~work:foo\|\|see` | byte offsets past multibyte text |
| LD11 | `a \|\| b` | never scanned |
| LD12 | `Fix ~tasks:target \|\|` | complete unlabelled until the rewrite glues ` \|\|` |
| LD13 | `Fix trip ~tasks:target\|\|"open` | open quote, partial hides |

### LG — Obsidian gestures

| # | Gesture | Outcome |
| --- | --- | --- |
| LG1 | Links stage ↵ on a same-note T | S's LINKS, T's mirror, and S's mint in one transaction; one Ctrl+Z undoes all |
| LG2 | cross-note T / the follow-up finds T changed | S commits, then T's mirror follows / S stands, suffix `· 🪢 LINKED FROM not updated` |
| LG3 | ⇧↵ then a label / an empty label | labelled entry / plain entry |
| LG4 | CURRENT ↵ / ⇧↵ | unlink with the mirror entry removed / relabel in place |
| LG5 | id-less T | block-ID prompt; Escape writes and mints nothing |
| LG6 | closed T / archived T | allowed / allowed, no mirror, suffix `· archived: shown live only` |
| LG7 | Linked-from stage ↵ on S | S's LINKS gains this task; this task's mirror updated |
| LG8 | Ctrl+Shift+P on a LINKS / LINKED FROM line | the owner's Links / Linked-from stage |
| LG9 | Ctrl+D on the Links to / Linked from row | every link and its mirror entries removed / refused |
| LG10 | hand edit adds / removes / relabels a LINKS entry, cursor leaves | mirrors follow; Ctrl+Z re-fires the mirror |
| LG11 | hand edit confined to LINKED FROM | notice once; no edge change |
| LG12 | `3 Ctrl+Shift+L`, choose T with a label | four sources each gain one labelled entry |
| LG13 | any relation gesture on an inbox task or a `]s` landing | no stamp, lane change, inbox routing, or walk advance, except a [§17](#17-cancel-labels) cancel of the landed source or a [§18](#18-complete-labels) complete of the landed source |
| LG14 | kill switch off | no mirror writes and no source mints |
| LG15 | Tab marks two targets, then ↵ | both linked in one gesture and one notice |
| LG16 | `~` typed anywhere in a Links / Linked-from filter | the stage flips direction in place; filter text and caret kept, the `~` never typed, marks cleared; a counted session refuses the flip to Linked from with "Linked from needs a single task" |
| LG17 | Links / Linked-from stage query that a Blocked task matches best | the Blocked row ranks by score inside RESULTS; the stage never shows a BLOCKED section; equal scores order In Progress, Next, then Ready and Blocked together |
| LG18 | Ctrl+Shift+L / Ctrl+Shift+Alt+L on a dedicated Task Link bullet | opens the linked task's Links / Linked-from stage in its own note, for open or closed targets |
| LG19 | the same gesture on a read-only linked task (`done/` or the previous daily note) | refused before the linked note is focused |
| LG20 | `N Ctrl+Shift+L` on Task Links | opens the first resolved linked task's Links stage and reports that it opened the first of N linked tasks |
| LG21 | Ctrl+Shift+L / Ctrl+Shift+Alt+L on a Depends-On or BLOCKS line | opens the owner's Links / Linked-from stage |
| LG22 | Ctrl+Shift+B / Ctrl+Shift+Alt+B on a LINKS / LINKED FROM line | opens the owner's Depends on / Blocks stage |

### LC — chips

| # | Situation | Render |
| --- | --- | --- |
| LC1 | one unlabelled open target | `🪢 links to → (○ T)` |
| LC2 | labelled target | pill `follow-up of`, then `→ (○ T)` |
| LC3 | closed / archived target | dimmed `✓` (never struck) / plus `↗ bob_done` |
| LC4 | unresolvable target | dashed `⚠ ^id not found` |
| LC5 | LINKED FROM with label `merged into` | `merged from (✕ S)` |
| LC6 | LINKED FROM with an unknown label `see` | `(○ S) · see → this` |
| LC7 | a live edge missing from stored text | appended in canonical order |
| LC8 | a stored entry that is no longer live | dashed; refresh tooltip; not counted |
| LC9 | four closed sources | `✓×4`; click expands |
| LC10 | labels present | one entry per visual row; line height unchanged |
| LC11 | nav api present / absent | ＋ and × controls per §10 / navigation only |
| LC12 | `toggle-dependency-chips` | toggles both relation rows with the dependency rows |
| LC13 | malformed or not-a-line row | no chips |
| LC14 | an entry typed in the editor before save | the target's chip row updates in the same session |

## 16. Label vocabulary

Rules RV1–RV10 are shipped.

### RV1 Words

- **Relation label** (contracts, CLI, JSON, code) is the optional text
  on a LINKS entry. The UI mostly says **relation**. Never use
  "description" in code or contracts: bob already uses it for task
  text.
- **Hide / Unhide / Show hidden.** Never "disable":
  `disabled_reason` already means "cannot be inserted", and a hidden
  label can still be inserted.
- **One-off**: a label created hidden (`||`). Wording that is never
  suggested again, not an expiring link. Relations are history.
- **Naming in code:** `relation_labels` (Rust module),
  `RelationLabel*` (Swift), `api.relationLabels` (JS).

### RV2 Key, catalog, and display spelling

- **Key.** `relation_label_key(label)` applies the writer
  normalization (trim, collapse whitespace runs, strip one trailing
  `:`) and then Unicode `to_lowercase`. This one function feeds the
  catalog, the typed-row match, the journal, rename, and, through
  vectors, the JS mirror. No NFC or other new equivalence.
- **Catalog.** One row per key, counting every accepted LINKS entry
  with that label: open, closed, `done/`, daily notes, nested
  projects, and references. The walk visits every `.md` in sorted
  path order (the dependency discover set, `done/` included),
  byte-prefilters for `**LINKS`, then parses direct children of
  `#task` lines with `task_relations::parse::links_children_of`.
  Malformed lines are skipped with a bounded warning. Completion
  tolerates them; rename refuses them. Never calls
  `capture_dependency_tasks::discover`.
  Parsed note rows use the machine-local bob-cli cache and per-file
  modification-time and size fingerprints. The full snapshot is reused
  for up to 60 seconds; a cache miss checks changed, new, and deleted
  notes, and successful `bob capture` writes invalidate it immediately.
  Every full rescan that stores also prunes the cache directory: vanished-vault
  and stale-version files go, stale atomic-write `.tmp` files go, and
  unparseable or unrelated files stay.
- **Display spelling.** The most-used spelling of the key, with ties
  going to the first seen. `spellings[]` lists every spelling with
  its count, so case drift is visible.
- **Inverse.** The §10 display-only table, matched case-insensitively
  and never stored (`task_relations::label::inverse_phrase`).
- **Journal-only keys.** A hidden key with zero uses stays listable
  (count 0) from the journal's stored name, so it can always be found
  and unhidden.

### RV3 Preferences journal

- **Location:** `.bob/relations/<host>.jsonl`, one append-only file
  per host. The host label is the vault-sync host label.
- **Events (`v: 1`):** `relation_label.hide` (`key`, `name`),
  `relation_label.unhide` (`key`, `name`), and
  `relation_label.rename` (`from`, `to`, `from_name`, `to_name`), each
  with `v`, `ts`, `host`, and
  `via`.
- **Fold:** exactly the theme rule, over the shared
  `pref_journal` module: read every `*.jsonl`, skip malformed lines
  with a bounded warning, sort by `(ts instant, host, path, line)`,
  last writer wins per key, and a rename moves the source key's
  explicit state unless the target already has state. Events after a
  rename on the old key apply to the old key.
- **Writers:** bob only. Appends happen under `bob_sync.lock` with a
  3 s wait (busy gives `lock_busy`), one `O_APPEND` write per event,
  and the directory is created on first use.
- **Vault allowlist:** `!/.bob/relations/*.jsonl` in `.gitignore`,
  and `.bob/relations/*.jsonl merge=union` in `.gitattributes`.

### RV4 What hiding means

- **Hiding removes a label from suggestions only.** Card rows, the
  Obsidian stage, `relations list` defaults, and shell completion.
  LINKS text, LINKED FROM mirrors, chips, counts, and typed use are
  unchanged. The twin of `theme-hide-is-recent-only`.
- **Using a hidden label never unhides it.** Typing its exact name
  still works and is flagged `hidden`.
- **No silent state.** Nothing auto-hides. A hide is always an
  explicit Ctrl+D, `bob relations hide`, or `||`.

### RV5 The relation card

Shipped in Bob Mac Capture and the Obsidian relation-label stage.
Ctrl+N/P navigate without touching the query, Tab accepts or
completes the top match, selection is by key, Ctrl+D hides what
Return would insert, Ctrl+H toggles the Hidden section, and Ctrl+R
renames everywhere.

### RV6 Capture grammar: `||`

`~note:id||label` stores the plain label and, once the capture
succeeds, hides it from suggestions. The `||` spelling covers `~~`
targets and `"multi word"` labels; an empty `||` is an incomplete
`relation_label` need; `|||` is a teaching error. The stored Markdown
is identical to the `|` form, and ` ||` glues the way ` |` glues.
Warn, never block: the hide lands only after the notes commit succeeds
(a dry run or a failed capture leaves no trace), one event per key in
a multi-item draft, counts from the pre-capture catalog, and a failed
append warns with the retry command while the capture succeeds.
Hiding a visible existing label carries an amber warning with the link
count; an already-hidden label writes no event. Shell examples quote
the draft, so the shell never sees `||`.

### RV7 Contracts

Every `schema_version` stays 1; JSON grows additively.

- **`bob relations`**: a group in **Tasks and projects**, between
  `projects` and `task`. Its membership rule is "the
  relation-label vocabulary" (the `bob themes` precedent: a catalog
  noun whose `rename` also rewrites notes). A bare group runs the
  read-only `list`. Hide and unhide do not rewrite task lines, so
  they stay out of `bob task`, and nothing goes under `bob capture`.

  ```text
  bob relations [list] [-a/--all] [-f/--format human|json] [-H/--hidden] [-n/--limit N] [-q/--query Q]
  bob relations hide LABEL…   [-f/--format human|json] [--allow-unused]
  bob relations rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline] [-r/--reverse]
  bob relations unhide LABEL… [-f/--format human|json]
  ```

  Hidden internal arguments: `--via CLIENT` (default `bob-cli`) on
  every writer, and `--allow-unused` on `hide`. Clients that hide a
  label they just wrote, before bob can see it on disk, pass
  `--allow-unused`. `-r/--reverse` reverses every OLD link;
  see [RV10](#rv10-reversing-a-label).

- **Human list** (mint relation accent on a TTY, dim counts; still
  readable with `NO_COLOR`):

  ```text
  RELATIONS  3 labels · 6 links · 1 hidden
    inspired by      3 links
    implements       2 links
    duplicate of     1 link     ⇄ duplicated by     ◌ hidden   (with -a)
  ```

  With `relations.cancel_labels` configured the header gains
  `· N cancel`, cancel rows read `N links     ⊘ cancels source`, and
  `-a` lists zero-use configured labels with `configured: true`.
  With `relations.complete_labels` configured the header gains
  `· N complete`, complete rows read `✓ completes source`, and `-a`
  lists zero-use configured labels with `configured: true`.

- **JSON list:**
  `{schema_version, ok, counts:{labels, links, hidden},
  labels:[{key, name, count, hidden, inverse, spellings,
  replacement, hide_replacement, cancels_source?, completes_source?, configured?}],
  renamed_reappeared, warnings, cancel_labels?, complete_labels?}`.
  `cancels_source` / `completes_source` are true on configured effect
  rows; `configured` marks zero-use overlay rows; false fields are omitted.
  The configured counts are omitted when zero.
  `renamed_reappeared` reports an OLD label used again after its rename;
  it reports the row and never rewrites the new use.
- **Writers** emit `{schema_version, ok, …}`. Human hide output reads:
  `🪢 Hid “implements” from relation suggestions · 2 links keep their
  label · undo: bob relations unhide implements`.
- **Refusal codes:** `unknown_label`, `invalid_label`, `unchanged`,
  `label_exists`, `malformed_links_line`, `lock_busy`, `dirty_paths`,
  `not_a_worktree`, `vault_mismatch`, `sync_failed`, `verify_failed`, and
  `stale_note`.
- **`capture-complete` `relation_label`**: rows come from the fast relation-label
  catalog, use canonical keys and display spelling, include inverse phrases and
  counts, omit hidden rows, expose exact typed matches (including hidden state),
  and return `hidden_count`. zsh describes each row as `N links`. Every row
  and the typed row carry `hide_replacement` (always the `||` form);
  a `||` probe keeps its pipe count in `replacement` and gains
  `one_off: true`; the response carries `hidden_count` (present, possibly 0)
  in this context. A row whose key is a configured cancel label carries
  `cancels_source: true`; zero-use configured labels appear as a trailing
  `configured: true` overlay that respects hiding. zsh describes a cancel
  row as `N links · cancels source`. The response gains `relation_cancel`
  (`source`, `verdict`, `message`, `hint?`) when a cancel label is
  configured. A cancel-label row for a recurring or new-task source is
  disabled with the CL6 reason.
- **`capture` / `--dry-run`**, per item: `relation_label_hides[]`
  (`{label, key, existing_count, was_hidden, action: hide|already_hidden}`;
  pre-capture counts; one entry per distinct hidden label key, omitted when
  empty). Hiding a visible existing label also adds a `warnings[]` object
  (`{code: "hides_existing_relation_label", label, count, message}`) with a
  bob-authored message that clients render without computing the count, and
  a human `🪢 hides "label" from suggestions (N links)` line. A failed
  journal append adds `{code: "relation_label_hide_failed", message}` with
  the exact retry command; the capture still succeeds. A dry run previews
  all of this and writes nothing.
- **Cancel labels** ([§17](#17-cancel-labels)): additive
  `cancels_source` / `configured` row flags, `relation_cancel`,
  `relation_cancels[]`, `cancel_effect`, and the
  `relation_cancelled_source` warning. The `capture`, `capture-parse`,
  `capture-complete`, and `relations list` fields are implemented.
- **Complete labels** ([§18](#18-complete-labels)): additive
  `completes_source` / `configured` row flags, `relation_complete`,
  `relation_completes[]`, `complete_effect`, and the
  `relation_completed_source` warning. Fields are omitted when they
  do not apply. See CP7.

### RV8 The rename transaction

`bob relations rename OLD NEW` uses one previewed transaction and one
revertable commit (`bob relations rename: OLD → NEW`). It scans every
`.md` note in the RV2 scope, including `done/` and the previous daily
note, and rewrites only matching label spans on accepted LINKS and
LINKED FROM lines. A malformed matching line or unreadable note refuses
the plan rather than under-covering it. Case-only renames unify spelling;
an unchanged spelling refuses; an existing NEW key requires `--merge`.
Merge keeps every edge and its order. The source hidden state moves to
NEW unless NEW already has explicit state. The preview lists each file
and changed line, link and mirror counts (including `done/`), state and
inverse changes, and NEW's `replacement` and `hide_replacement` forms.
Renaming into or out of a cancel label never cancels; the preview warns
`“{label}” is a cancel label in config.yml — update
relations.cancel_labels`. Renaming into or out of a complete label
never completes ([§18](#18-complete-labels)); the preview warns
`“{label}” is a complete label in config.yml · a rename never completes anything — check relations.complete_labels`.
A reversed rename ([RV10](#rv10-reversing-a-label)) never completes or
cancels either.

Before taking a lock, an online rename requires `-b/--bob-dir` and
`BOB_DIR` (default `~/bob`) to resolve to the same directory. A mismatch or
unresolved path refuses without syncing either vault. Previews and offline
writes do not require this match; symlink aliases work.

| Refusal | Meaning | Recovery |
| --- | --- | --- |
| `unknown_label` | OLD is absent from the current catalog | Choose a listed label |
| `invalid_label` | OLD or NEW fails the writer label rules | Use a valid 1–40 character label |
| `unchanged` | The requested spelling makes no change | Choose a different NEW spelling |
| `label_exists` | NEW already has a different key | Review the preview with `--merge` |
| `malformed_links_line` | A matching relation line is malformed or a note cannot be read | Repair access or the line, then retry |
| `dirty_paths` | An offline target note has uncommitted changes | Commit or restore those paths |
| `stale_note` | A target changed after planning | Re-run to preview the latest contents |
| `verify_failed` | Rescan found a mismatch; the transaction restores its before-images | Inspect the reported note, then retry |
| `lock_busy` | Another vault maintenance run holds the lock | Wait for it to finish |
| `not_a_worktree` | The vault is not a Git worktree | Open or initialize the vault worktree |
| `vault_mismatch` | The selected vault differs from `BOB_DIR`, or a path cannot be resolved | Set `BOB_DIR` to the selected vault, or use `--offline` |
| `sync_failed` | Pre-sync failed or conflicted | Resolve sync, or intentionally use `--offline` |

Undo a successful rename with `git revert <rename-commit>`; the commit
contains the note-span rewrites and the journal event. No live-vault
rename is part of acceptance.

### RV9 Obsidian

Shipped. One ranker
and one key shared through byte-for-byte vectors, a read-only
journal fold, `api.relationLabels` v1 with desktop-only
hide/unhide/rename writes through bob, and the nav relation-label
stage with the card's keys and copy.

### RV10 Reversing a label

*Status: shipped.*

`bob relations rename OLD NEW -r/--reverse` moves every accepted LINKS
entry `S —OLD→ T` to `T —NEW→ S` in one previewed transaction and one
revertable commit.

**Meaning.** For each accepted OLD entry:

1. Remove the entry from S's LINKS line. Delete the line when it
   empties, and keep the order of the remaining entries.
2. Append `NEW [[canonical link to S]]` to T's LINKS line, creating
   the line in its §4 slot. The link uses §5's `canonical_link`, so
   `done/` keeps `[[done/…#^id]]`.
3. Rewrite the LINKED FROM mirrors of S and T to the §6 projection.
   Only writable notes get mirrors. `linked_from_lines: false` writes
   none.

**Scope and transaction.** These are RV8's: every note in the RV2
scope (`done/` and the previous daily note included); pre-sync, the
lock, the `vault_mismatch` check, `stale_note`, and verify by rescan;
one `relation_label.rename` journal event (unchanged v1 schema; hidden
state moves); one commit,
`bob relations rename: OLD → NEW (reversed)`, undone by
`git revert`.

LR7 grows one clause: a reversed rename may move an edge's entry
between the two ends' LINKS lines, including in `done/`. It still
never changes a status, a field, or another byte.

NEW may equal OLD under `--reverse`, which is a pure reversal of a
symmetric label. NEW's existing key still needs `-m/--merge`.

**Collisions.**

- If T already links S with NEW's key, merge: drop S's entry and
  report `merged`.
- If T already links S with another label, refuse `reverse_conflict`
  and list each pair.

**Refusals added.**

| Refusal | Meaning | Recovery |
| --- | --- | --- |
| `reverse_conflict` | T already links S with a different label | Relabel or unlink the existing edge, then retry |
| `unresolved_target` | An OLD entry does not resolve to a `#task` | Repair the link, then retry |
| `source_needs_block_id` | S has no id and lives in `done/` or the previous daily note | Mint an id on a writable S (the same transaction does this silently) or repair the archived source |

**Never completes or cancels**, even when NEW is a closing label. When
NEW is configured, the preview adds a muted
`stay open  {k} new sources keep their status — a rename never closes`
row listing them.

**Output.**

- The human preview groups moves by source note, with mint, mirror,
  merge, and status counts.
- JSON adds `reversed: true`,
  `moves[]: {source: {path, line, block_id, text}, target: {path, line, block_id, text}, from_label, to_label, action: "moved"|"merged"}`,
  and `minted_block_ids[]`.

Help lists `-r/--reverse` alphabetically after `-o/--offline`. The
flag is a new option; it is not a contract break.

### Shared vectors

`tests/fixtures/relation_labels/vectors.json` is the single source.
Its `key`, `catalog`, `fold`, `rank`, and `grammar` (LD vectors)
sections are consumed by Rust tests; rename fold cases live in
`fold`. bob-plugins copies the file byte-for-byte.

Cancel-label bytes live in
`tests/fixtures/task_cancel/vectors.json` ([§17](#17-cancel-labels)).
Complete-label bytes live in
`tests/fixtures/relation_complete/vectors.json`
([§18](#18-complete-labels)).

## 17. Cancel labels

A **cancel label** is a relation label listed in
`relations.cancel_labels`. Committing one through an explicit
relation gesture cancels the relation's **source** exactly as Task
Card Cancel would.

### CL1 The rule

*Status: implemented in capture and the nav relation writer.*

When an explicit relation gesture **adds** a cancel label to an open
task, or **relabels** one of its links **into** a cancel label, the
same gesture cancels that task (the relation's **source**) exactly as
Task Card Cancel would. Nothing else cancels because of a label, and
nothing ever reopens because of one.

**Explicit gestures** are capture `~` / `~~` (CLI and Mac), the nav
Links-to and Linked-from stages (`l` / `⇧L` → `⇧↵`), and counted or
marked batches in those stages.

It **never** fires on: reconcile, `bob relations rename`, hand edits
of LINKS, agent edits, unlink, relabel *away* from a cancel label,
mirrors, vault sync, config changes, or an `Unchanged` re-add.

It **fires once, at commit.** It is never a state that reconcile
enforces. A reopened source stays open. Only a *new* explicit add or
relabel can cancel it again.

### CL2 Config

*Status: implemented; chezmoi enables `duplicate of` and `superseded by`.*

The config lives in chezmoi `home/dot_config/bob/config.yml`. bob's
built-in default is `[]`.

```yaml
# Task relations (docs/task-relations.md in bob-cli).
relations:
  # linked_from_lines: true   # write 🪢 LINKED FROM mirrors (default)
  # Committing a link with one of these labels also cancels the task the label
  # is phrased from ("S duplicate of T" cancels S). Matched like relation
  # labels (case, spacing, trailing colon ignored). Empty cancels nothing.
  cancel_labels:
    - duplicate of
    - superseded by
    # - merged into
```

- **Matching** uses `relation_label_key`. `Duplicate  Of:` matches;
  `duplicated by`, `supersedes`, and `dup of` do not. There is no
  substring or fuzzy matching.
- **Parsing fails toward fewer cancels.**
  - A missing key, `null`, or `[]` is a valid empty policy.
  - A non-sequence value warns
    `invalid relations.cancel_labels in <path>: expected a list of relation labels; cancelling nothing`
    and yields an empty set.
  - An entry that is not a string, or not a legal relation label
    (the writer's length and character rules in §3.2), warns and is
    skipped.
  - Entries that share a key collapse.
  - A `cancel_labels` problem never becomes a `ConfigError`, and
    never changes `linked_from_lines`.
- **Hide is not disable.** A hidden or `||` one-off cancel label
  still cancels when typed
  (`relation-label-hide-is-suggestions-only`). To turn the effect
  off, change the config.
- **Catalog overlay.** A configured cancel label appears in
  suggestions and in `bob relations list -a` even with zero uses. It
  is marked `configured`, ranks as a zero-use row, and is never
  boosted. Hidden still wins.
- **Host-wide YAML**, like `linked_from_lines`. It never goes in the
  per-host label journal, and bob never edits `config.yml`.
- **One effect per label.** A key
  listed in both `cancel_labels` and `complete_labels` closes nothing.
  bob drops it from **both** lists and warns once:
  `relations: “{label}” is in both cancel_labels and complete_labels in <path>; it closes nothing`.
  This is the only change to shipped cancel parsing, and it is
  reachable only through a misconfiguration. See
  [§18 CP2](#cp2-config).

### CL3 Which task is cancelled

*Status: implemented in capture and the nav relation writer.*

The source is always the task the label is phrased from: "S duplicate
of T" cancels S.

| Gesture | Source S (cancelled) | Target T (survives) |
| --- | --- | --- |
| Task Card `l` → `⇧↵` | task under the cursor | picked task |
| Task Card `⇧L` → `⇧↵` | **picked task**, possibly in another note | task under the cursor |
| Capture `@note+id ~t:id\|"duplicate of"` | routed existing task | picked task |
| Capture `New task ~~old:id\|"superseded by"` | **picked existing task** | the new task |
| Capture `New task ~old:id\|"duplicate of"` | — refused `relation_cancels_new_task` | — |
| Counted `N Ctrl+Shift+L` / marked rows | each source | one target (second Return if ≥2 cancels) |

### CL4 What one commit writes

*Status: the pure Rust cancel engine and the capture composition are
implemented.* Nav folds these writes into the source write.

Every step below is existing cancel behavior. The only new part is
composing them.

1. **S's note, one write.** The LINKS add or relabel, then `[-]` and
   an upserted `[cancelled:: today]` on S's task line. "Today" is
   bob's configured day timezone in Rust, and nav's existing Task
   Card Cancel date in JS.
   - There is no new Cancel Log.
   - If S already keeps a `❌ CANCEL LOG`, prepend
     `*date* — 🪢 duplicate of [[T]]`, using the same link text the
     LINKS entry uses. This is nav's `fallbackReason`. Two cancel
     edges on one source in one commit join in authored order,
     separated by `·`.
2. **T's LINKED FROM mirror.** The usual best-effort follow-up;
   reconcile stays the authority. `linked_from_lines: false` skips
   only this step.
3. **Today's daily note.** Prune S's links under **open** Pomodoros
   with Task Card Cancel's immediate semantics:
   - a dedicated link bullet goes with its subtree;
   - in mixed prose, only the token goes;
   - struck links, closed Pomodoros, top-level entry lines, fenced
     examples, text outside `## Pomodoros`, and other days stay.

   Never create the note: an absent note means 0 links, and an
   unreadable note is an error.

4. **Recovery.** S's Blocked dependents recover through the existing
   engines, stamping `[unblocked:: today]`
   (`unblocked-tasks-resurface-as-ticklers`). For a `^prj`, this
   includes residents blocked only by the project. There are **no
   successor offers** and no dependency transfer.
5. **Feedback.** One composed result names the cancel, the prune
   count, and how many tasks were unblocked.

For the `cash.md` case, the result is byte-identical to the
hand-made cancel:

```markdown
- [-] #task Sell stocks! [fresh:: 2026-10-16] [created::2026-10-12] [priority::high] [scheduled:: 2026-10-22] [cancelled:: 2026-10-22] ^sell-stocks
  - 🪢 **LINKS:** duplicate of [[dev_plex_hack_patch#^sell-stocks-now]]
```

### CL5 Verdicts and edge cases

The Rust engine and the JavaScript client use one verdict function each:
Rust in `task_cancel`, JS `planRelationCancelEffects`. The shared `rule`
vectors pin both, so the cue can never disagree with the write.

| Source state | Verdict | Effect |
| --- | --- | --- |
| Open: `[ ]` Ready, `[*]` Next, `[/]` In Progress, `[?]` Blocked (the Task Card Cancel open set) | `cancels` | link and cancel |
| `^prj` task, open | `cancels` (project) | as Task Card `x`; copy adds "its N open tasks stay open" |
| Done, Cancelled, or any status outside the open set | `already_closed` | link or relabel only; keep the original date; never Done → Cancelled |
| Recurring (`is_recurring_task_line`) | `recurring` | refused before writing; cancel-label rows disabled |
| `~` source is the task this capture item creates | `new_task` | refused before writing, with a `~~` hint |
| Item also completes, starts, Pomodoro-links, or adds `&` / `*` to S | `conflict` | refused `relation_cancel_conflicts_with_item_action` |

More edge cases:

- **Same edge, same label key (`Unchanged`):** no cancel and no log.
  Retries are safe.
- **Relabel from one cancel label to another** on an already-cancelled
  S: relabel only.
- **Relabel away, or unlink:** S stays Cancelled. The nav notice says
  `stays Cancelled · reopen with Alt+]`.
- **T is Cancelled or archived:** allowed. Amber caveat `“T” is
  Cancelled too`. T is never changed.
- **Inverse label typed** (`supersedes`, `duplicated by`): no cancel,
  plus a muted flip hint (CL6).
- **`bob relations rename` into or out of a cancel label:** never
  cancels. The preview warns that the config needs updating.
- **Midnight rollover or a config change between cue and commit:**
  re-plan at commit. If the consequence changed, nav writes nothing
  and re-renders for a fresh Return. Capture always plans at commit,
  so its result reports what happened.

### CL6 The indication

*Status: implemented in capture, cancel-surfaces, nav, and Mac.*

Name the task, change the verb, use ⊘, stay calm.

- **⊘ plus words**, in the existing Cancelled-status tint, marks the
  intended consequence. Meaning never depends on color alone.
- **Amber** stays reserved for caveats (T already cancelled, the `||`
  hide warning).
- **Red** stays reserved for errors and refusals.
- **No strikethrough** on a task that has not been cancelled yet. Bob
  already uses strikethrough for closed ledger children.

Canonical strings. `{S}` and `{T}` are task texts. `{note}` is the
note stem, shown only when S is not in the active note (nav), or for
`~~` (capture). Plurals follow the count.

| Where | Text |
| --- | --- |
| Row meta (nav) / capsule (Mac) / `relations list` / zsh | `⊘ cancels` / `circle.slash cancels` / `⊘ cancels source` / `N links · cancels source` |
| Live sentence (nav, Mac) | `⊘ {S} — {label} → {T}`, source dimmed; remote: `⊘ {S} ({note}) — …` |
| Verdict `cancels` (bob `relation_cancel.message`) | `⊘ Cancels “{S}” · {Status} → Cancelled` |
| Consequence strip (nav) | `⊘ Cancels “{S}” · removes its links from today's open Pomodoros · unblocks dependents` (no Pomodoro clause when S has no block id, as Task Card does) |
| `^prj` suffix | `· its 1 open task stays open` / `· its {N} open tasks stay open` |
| Verdict `already_closed` (muted) | `Already Cancelled · link only` / `Stays Done · link only`; capture prints this as a muted human line only |
| Verdict `recurring` (disabled row) | `“{S}” repeats — cancel it with Obsidian Tasks first` |
| Verdict `new_task` (disabled row, refusal) | `“{S}” is new — it would be born cancelled · to retire “{T}”, use ~~` |
| Inverse hint (muted) | `“supersedes” never cancels · to retire “{T}”, use ~~ with “superseded by”` (nav says `⇧L` instead of `~~`) |
| Return hint (nav) | `↵ link & cancel` |
| Batch banner (nav) | `⊘ Cancels {N} tasks — ↵ again to confirm` |
| Notice (nav) | `🪢 {S} — {label} → {T} · ⊘ cancelled · removed {n} Pomodoro link · unblocked {k}` |
| Capture row / preview (bob-authored) | `⊘ cancelled “{S}” · {Status} → Cancelled · removed {n} Pomodoro link · unblocked “{D}”`; two or more recovered dependents use `unblocked {k} tasks`; dry run uses `would cancel`, `would remove`, `would unblock` |
| Structured warning `relation_cancelled_source` | `⊘ cancelled “{S}” ({label} “{T}”)`; dry run `⊘ would cancel …` |
| Hand-edit hint (nav, once per burst) | `⊘ “{label}” cancels only from the Links stage or capture · this task stays open` |
| Rename warning | `“{label}” is a cancel label in config.yml — update relations.cancel_labels` |

### CL7 Contracts

*Status: the `capture`, `capture-parse`, `capture-complete`, and
`relations list` fields are implemented.*

All additions are additive, every `schema_version` stays 1, and every
new field is omitted when it does not apply, so older output stays
byte-identical. Names below are final.

- **`capture-complete`, `relation_label` context:**
  - Rows gain `cancels_source: true` when the row's key is
    configured, and `configured: true` on overlay rows.
  - A cancel-label row gets the existing `disabled_reason` when the
    verdict is `recurring` or `new_task`.
  - The response gains
    `relation_cancel: {source: {path, block_id?, text, status_symbol, status_name, project_open_tasks?}, verdict: "cancels"|"already_closed"|"recurring"|"new_task", message, hint?}`.
    It is present only when at least one cancel label is configured.
    It resolves only the token's source: the item owner for `~`, the
    `~~` token's task for `~~`. It never scans the vault.
- **`capture-parse`:** relation-label spans gain `cancels_source: true`.
- **`capture` (dry run and commit), per item:**
  - `relation_cancels[]: {source: {path, line, block_id, text}, target: {path, block_id, text}, labels[], action: "added"|"relabeled", previous_status, previous_status_name, project_open_tasks?, cancel_log: "none"|"prepended", pomodoro_links_removed, recovered[]: {path, line, block_id, text}, message}`. One row per source the item cancels; already-closed sources appear only as `relation_updates[].cancel_effect`.
  - `relation_updates[]` rows gain `cancel_effect: "cancelled"|"already_closed"`.
  - `warnings[]` gains `{code: "relation_cancelled_source", label, message}`.
    The human renderer prints the ⊘ row once and does not repeat the
    warning.
  - Refusal codes: `relation_cancels_new_task`,
    `relation_cancels_recurring_task`,
    `relation_cancel_conflicts_with_item_action`.
- **`bob relations list`:** JSON rows gain `cancels_source` and
  `configured`. The human header gains `· N cancel`.

### CL8 Architecture

*Status: implemented in the Rust engine, capture, nav, and Mac.*

Three layers, with one authority per surface:

1. **Policy:** label key → cancels or not, plus config validity.
2. **Pure effect plan:** edge delta plus snapshots → source
   transition, optional log line, daily prune, recovery, refusals,
   and summary.
3. **Existing writers:** apply the planned contents through today's
   guarded paths.

- bob owns capture (CLI and Mac). Swift renders bob's flags and
  strings and decides nothing (`mac-capture-is-a-thin-client`).
- nav keeps its JS planner and editor transaction: unsaved buffers,
  same-note transactions, and undo stay local. Obsidian never goes
  through `bob capture`.
- Never call the relation writer and then the cancel UI handler.
  Compose the plans before the write.
- **Shared vectors** `tests/fixtures/task_cancel/vectors.json`
  (bob-cli is the source of truth; bob-plugins copies it
  byte-for-byte) keep the two engines writing identical bytes:
  - `cancel` and `prune` bytes are **generated from nav's current
    Task Card Cancel planners**, so "just like other cancels" is
    checked, not assumed;
  - `rule`, `key`, and `config` are hand-authored from this section.
- **Atomicity, stated honestly:**
  - Capture: S's LINKS, status, and log, T's mirror, today's prune,
    and recovery share one staged, preimage-checked batch.
  - Obsidian, when S's note is the daily note: one transaction and
    one undo.
  - Obsidian, otherwise: S's note commits as one undo step. The
    prune, mirror, and recovery are follow-ups that show a chip on
    failure, and reconcile heals them. This is the contract Task
    Card Cancel already ships with.

### CL vector index

`tests/fixtures/task_cancel/vectors.json` is the single source.
bob-plugins copies it byte-for-byte. A behavior change edits bob-cli
first.

| Section | Pins | Representative cases |
| --- | --- | --- |
| `cancel` | Task Card Cancel source-note bytes (`planTaskCancelBatch`, `reason: ""`) | each open status; fields present or absent; existing `❌ CANCEL LOG` prepend vs none; already Done or Cancelled; recurring refused; `^prj`; `cash-md-cl4` |
| `prune` | immediate Pomodoro prune (`planDeferredPomodoroLinkCleanup`) | dedicated bullet with children; mixed-prose token; struck link kept; closed Pomodoro kept; top-level entry kept; fenced example kept; outside `## Pomodoros` kept; several links; empty daily note |
| `rule` | verdict and `cancels` for an edge | every CL5 row; every CL1 never path; the CL3 table |
| `key` | cancel-label matching (`relation_label_key`) | case/spacing/colon; inverses; substrings |
| `config` | YAML → effective keys and warning count | missing, `null`, `[]`, scalar, mapping, non-string, illegal label, duplicate keys, invalid `linked_from_lines` beside valid `cancel_labels` |

## 18. Complete labels

A **complete label** is a relation label listed in
`relations.complete_labels`. Committing one through an explicit
relation gesture marks the relation's **source** Done exactly as that
surface's managed Done gesture would.

### CP1 The rule

*Status: implemented in capture and nav.*

When an explicit relation gesture **adds** a complete label to an open
task, or **relabels** one of its links **into** a complete label, the
same gesture marks that task (the relation's **source**) Done exactly
as that surface's managed Done gesture would. Nothing else completes
because of a label, and nothing ever reopens because of one.

**Explicit gestures** are capture `~` / `~~` (CLI and Mac), the nav
Links-to and Linked-from stages (`l` / `⇧L` → `⇧↵`), and counted or
marked batches in those stages.

It **never** fires on: reconcile, `bob relations rename` (including
`--reverse`), hand edits of LINKS, agent edits, unlink, relabel *away*
from a complete label, mirrors, vault sync, config changes, or an
`Unchanged` re-add.

It **fires once, at commit.** It is never a state that reconcile
enforces. A reopened source stays open. Only a *new* explicit add or
relabel can complete it again.

**The source closes, always.** Closing labels are phrased from the
task that closes: "S duplicate of T" cancels S; "S implemented by T"
and "S verified by T" complete S.

### CP2 Config

*Status: implemented in capture; live config rollout stays with the outer phases.*

The config lives in chezmoi `home/dot_config/bob/config.yml`. bob's
built-in default is `[]`.

```yaml
# Task relations (docs/task-relations.md in bob-cli).
relations:
  # linked_from_lines: true   # write 🪢 LINKED FROM mirrors (default)
  cancel_labels:
    - duplicate of
    - superseded by
    # - merged into
  # Committing a link with one of these labels also completes (marks Done) the task
  # the label is phrased from ("S implemented by T" completes S), exactly as
  # Ctrl+Enter or capture `!` would. Matched like cancel_labels. Empty completes
  # nothing.
  complete_labels:
    - implemented by
    - verified by
```

- **Matching** uses `relation_label_key`. `Implemented  By:` matches;
  `implements`, `verifies`, and `implement` do not. There is no
  substring or fuzzy matching.
- **Parsing fails toward fewer completes**, and is CL2's verbatim:
  - A missing key, `null`, or `[]` is a valid empty policy.
  - A non-sequence value warns
    `invalid relations.complete_labels in <path>: expected a list of relation labels; completing nothing`
    and yields an empty set.
  - An entry that is not a string, or not a legal relation label
    (the writer's length and character rules in §3.2), warns and is
    skipped.
  - Entries that share a key collapse.
  - A `complete_labels` problem never becomes a `ConfigError`, and
    never changes `linked_from_lines`.
- **One effect per label.** A key listed in both lists closes nothing.
  bob drops it from **both** lists and warns once:
  `relations: “{label}” is in both cancel_labels and complete_labels in <path>; it closes nothing`.
  This rule also governs cancel parsing, in Rust and JS. It is the
  only change to shipped cancel behavior, and it is reachable only
  through a misconfiguration. See [CL2](#cl2-config).
- **Hide is not disable.** A hidden or `||` one-off complete label
  still completes when typed
  (`relation-label-hide-is-suggestions-only`). To turn the effect
  off, change the config.
- **Catalog overlay.** A configured complete label appears in
  suggestions and in `bob relations list -a` even with zero uses. It
  is marked `configured`, ranks as a zero-use row, and is never
  boosted. Hidden still wins.
- **Host-wide YAML**, like `linked_from_lines`. It never goes in the
  per-host label journal, and bob never edits `config.yml`.

### CP3 Which task completes

*Status: implemented in capture and nav.*

The source is always the task the label is phrased from: "S
implemented by T" completes S.

| Gesture | Source S (completed) | Target T (unchanged) |
| --- | --- | --- |
| Task Card `l` → `⇧↵` | task under the cursor | picked task |
| Task Card `⇧L` → `⇧↵` | **picked task**, possibly in another note | task under the cursor |
| Capture `@note+id ~t:id\|"implemented by"` | routed existing task | picked task |
| Capture `New task ~~ref:id\|"implemented by"` | **picked existing task** | the new task |
| Capture `New task ~old:id\|"verified by"` | — refused `relation_completes_new_task` | — |
| Counted `N Ctrl+Shift+L` / marked rows | each source | one target (second Return if ≥2 closes) |

### CP4 What one commit writes

*Status: implemented in capture and nav.*

There is nothing new about Done. Each surface composes its **existing**
managed Done with the relation write.

**Capture (CLI and Mac)** runs exactly what `!note:id` runs, against
the same staged batch as the LINKS edit:

1. tree close: `[x]` plus `  [completion:: today]` on S, and embedded
   `![[…]]` subtasks per `!`'s rules;
2. scoped Task Link retirement in today's daily note;
3. dependent recovery with `[unblocked::]`;
4. successor offers. They join the batch's single offer set: numbered,
   picked with `-p/--pull-in`, and gated by
   `--require-pull-in-choice`.

The LINKS edit, T's mirror, and all of the above are one
all-or-nothing batch. `!`'s other rules hold verbatim: Blocked roots
complete, embedded descendants follow `!`, and status-group moves stay
with reconcile.

**Obsidian (nav)** runs exactly what Ctrl+Enter on the task line runs:

1. S's note, one write: the LINKS add or relabel plus the cycler's own
   Done line rewrite. One Ctrl+Z restores both.
2. T's LINKED FROM mirror, as the usual best-effort follow-up.
3. After the stage closes, the cycler's
   `finalizeClosedTasks([S], context)`: recovery, embed retirement, the
   Unblocked notice, and the successor picker.

Ctrl+Enter on a task line strikes no plain Pomodoro Task Link
immediately, so this path strikes none either; reconcile retires it as
it does after Ctrl+Enter today.

**No Work Log, no Cancel Log, no freshness stamp.** Done never writes
them, so neither does this.

### CP5 Verdicts and edge cases

*Status: implemented in capture and nav.*

There is one verdict function per engine: Rust
`task_complete::verdicts` and JS `planRelationCompleteEffects`. Shared
`rule` vectors pin both, so the cue can never disagree with the write.

| Source state | Verdict | Effect |
| --- | --- | --- |
| Completable: `[ ]` Ready, `[*]` Next, `[/]` In Progress, `[?]` Blocked | `completes` | link and complete (Blocked included, as `!` allows) |
| `^prj` task, completable | `completes` (project) | as `!` / Ctrl+Enter; copy adds "its N open tasks stay open" |
| Done, Cancelled, or any other status | `already_closed` | link or relabel only; never Cancelled → Done; keep original date |
| Recurring (`is_recurring_task_line`) | `recurring` | refused before writing; complete-label rows disabled |
| `~` source is the task this capture item creates | `new_task` | refused before writing, with a `~~` hint |
| Item also starts, Pomodoro-links, or adds `&` / `*` to S | `conflict` | refused `relation_complete_conflicts_with_item_action` |
| One commit both cancels and completes the same S | `conflict` | refused `relation_close_effects_conflict` |

More edge cases:

- **Same edge, same label key (`Unchanged`):** no-op. Retries are
  safe.
- **Relabel from one complete label to another** on an already-Done
  S: relabel only.
- **Relabel away, or unlink:** S stays Done. The nav notice says
  `stays Done · reopen with Alt+[`.
- **T is Cancelled or archived:** allowed, with the amber caveat
  `“{T}” is Cancelled`. T is never changed.
- **T is open:** normal, and no caveat. `verified by` usually points
  at an open verification task.
- **Inverse label typed** (`implements`, `verifies`): no completion,
  plus a muted flip hint (CP6).
- **`bob relations rename` into or out of a complete label,**
  including `--reverse`: never completes. The preview warns that the
  config needs checking.
- **Midnight rollover or a config change between cue and commit:**
  re-plan at commit. If the consequence changed, nav writes nothing
  and re-renders for a fresh Return. Capture always plans at commit,
  so its result reports what happened.
- **Old cycler** (nav finds `api.version < 3`): complete-label rows
  are disabled with
  `Update Task Status Cycler to complete from a link`, and nothing
  completes.

### CP6 The indication

*Status: implemented in capture; planned (`nav-complete-cue`, `mac-complete-cue`).*

Name the task, change the verb, use ✓, stay calm.

- **✓ plus words**, in the Done-status green, mark the consequence;
  meaning never depends on color.
- **Red** stays for errors; **amber** stays for caveats.
- **No strikethrough** before the write.
- **Mac:** `checkmark.circle` + green, matching the picker's Done
  glyph and `!`'s seal.

Canonical strings. `{S}` and `{T}` are task texts. `{note}` is the
note stem, shown only when S is not in the active note (nav), or for
`~~` (capture). Plurals follow the count.

| Where | Text |
| --- | --- |
| Row meta (nav) / capsule (Mac) / `relations list` / zsh | `✓ completes` / `checkmark.circle completes` / `✓ completes source` / `N links · completes source` |
| Live sentence (nav, Mac) | `✓ {S} — {label} → {T}`, source dimmed; remote: `✓ {S} ({note}) — …` |
| Verdict `completes` (bob `relation_complete.message`) | `✓ Completes “{S}” · {Status} → Done` |
| Consequence strip (nav) | `✓ Completes “{S}” · unblocks dependents · offers successors` |
| `^prj` suffix | `· its {N} open tasks stay open` |
| Verdict `already_closed` (muted) | `Already Done · link only` / `Stays Cancelled · link only` / `Stays {Status} · link only` |
| Verdict `recurring` (disabled row) | `“{S}” repeats — complete it in Obsidian so Tasks writes the next one` |
| Verdict `new_task` (disabled row, refusal) | `“{S}” is new — it would be born done · to finish “{T}”, use ~~` |
| Inverse hint (muted) | `“{typed}” never completes · to finish “{T}”, use ~~ with “{label}”` (`~` from `~~`; nav says `⇧L` / `l`) |
| Return hint (nav) | `↵ link & complete` |
| Batch banner (nav) | `✓ Completes {N} tasks — ↵ again to confirm`; mixed: `Closes {N} tasks · ✓ {a} · ⊘ {b} — ↵ again to confirm` |
| Notice (nav) | `🪢 {S} — {label} → {T} · ✓ done · unblocked {k}`, plus the cycler's returned successor text |
| Capture row / preview (bob-authored) | `✓ completed “{S}” · {Status} → Done · struck {n} Pomodoro link · unblocked “{D}”`, then `!`'s ledger and offer rows; dry run says `would complete` / `would strike` / `would unblock` |
| Structured warning `relation_completed_source` | `✓ completed “{S}” ({label} “{T}”)`; dry run `✓ would complete …` |
| Both-effects refusal | `“{S}” can't be both cancelled and completed in one commit` |
| Hand-edit hint (nav, once per burst) | `✓ “{label}” completes only from the Links stage or capture · this task stays open` |
| Rename warning | `“{label}” is a complete label in config.yml · a rename never completes anything — check relations.complete_labels` |
| Mac submit hint | `Capture & complete {N} task(s)`; mixed with cancels: `Capture & close {N} tasks` |

### CP7 Contracts

*Status: capture fields implemented.*

All additions are additive, every `schema_version` stays 1, and every
new field is omitted when it does not apply, so output with no
`complete_labels` stays byte-identical. Names below are final.

- **`capture-complete`, `relation_label` context:**
  - Rows gain `completes_source: true` when the row's key is
    configured, and the shared `configured: true` on overlay rows.
  - A complete-label row gets the existing `disabled_reason` when the
    verdict is `recurring` or `new_task`.
  - The response gains
    `relation_complete: {source: {path, block_id?, text, status_symbol, status_name, project_open_tasks?}, verdict: "completes"|"already_closed"|"recurring"|"new_task", message, hint?}`.
    It is present only when at least one complete label is configured.
    It never scans the vault. It sits beside `relation_cancel` when
    both kinds are configured.
- **`capture-parse`:** relation-label spans gain `completes_source: true`.
- **`capture` (dry run and commit), per item:**
  - `relation_completes[]: {source: {path, line, block_id, text}, target: {path, block_id, text}, labels[], action: "added"|"relabeled", previous_status, previous_status_name, project_open_tasks?, task_complete, message}`.
    `task_complete` is exactly the object `!note:id` reports for S,
    with `raw` set to the relation token. It is serialized only for
    sources that completed.
  - `relation_updates[]` rows gain
    `complete_effect: "completed"|"already_closed"`.
  - `warnings[]` gains `{code: "relation_completed_source", label, message}`.
    The human renderer prints the ✓ row once.
  - Offers join top-level `pull_in` and its numbering.
  - Refusal codes: `relation_completes_new_task`,
    `relation_completes_recurring_task`,
    `relation_complete_conflicts_with_item_action`,
    `relation_close_effects_conflict`.
- **`bob relations list`:**
  - JSON rows gain `completes_source`, and the top level gains
    `complete_labels?` (a count);
  - the human header gains `· N complete`;
  - rows read `✓ completes source`.

### CP8 Architecture

*Status: implemented in Rust capture and nav.*

- **One Done per engine, never a new one.**
  - Rust: complete-label commits call the same function `!` calls.
    `complete-capture` extracts it from `plan_task_complete_item`, so
    `!` output stays byte-identical.
  - JS: the cycler owns Done. Nav calls the cycler `api` v3:
    - `completeTaskLineText(lineText, {date})` returns the exact
      fallback bytes;
    - `finalizeClosedTasks(identities, context)` runs the post-Done
      pass.

    Plugins never import each other's `main.js`.

- **Policy layer shared with cancel:**
  - Rust: `RelationsConfig::close_effect(key) -> Option<CloseEffect>`,
    where `enum CloseEffect { Cancel, Complete }`, and one parser for
    both lists.
  - JS: one loader, `loadRelationsCloseLabels()` →
    `{cancel, complete, warnings}`, of which
    `loadRelationsCancelLabels()` becomes a view.
- **Shared vectors** are
  `tests/fixtures/relation_complete/vectors.json`. bob-cli is the
  source, and bob-plugins copies it byte-for-byte.
  - `done` is generated from the cycler's
    `rewriteTaskLineForLocalFallback` +
    `addOrReplaceCompletionField`, and a Rust test proves
    `complete_task_tree`'s root rewrite writes the same bytes.
  - `rule`, `key`, and `config` are hand-authored from this section.
- **Atomicity, stated honestly:**
  - Capture is one staged, preimage-checked batch.
  - Nav writes S's note in one transaction with one undo. The mirror
    and the post-Done pass are follow-ups, the same contract
    Ctrl+Enter and cancel labels ship with.
- **Mac decides nothing** (`mac-capture-is-a-thin-client`).

### CP vector index

`tests/fixtures/relation_complete/vectors.json` is the single source.
bob-plugins copies it byte-for-byte. A behavior change edits bob-cli
first.

| Section | Pins | Representative cases |
| --- | --- | --- |
| `done` | cycler local-fallback Done bytes (`rewriteTaskLineForLocalFallback` + `addOrReplaceCompletionField`) | each completable status; fields present or absent; `#prj_is_blocked`; `^prj`; existing `[completion::]`; CRLF; already Done or Cancelled unchanged; recurring refused |
| `rule` | verdict, `effect`, and refusal for an edge | every CP5 row; every CP1 never path; the CP3 table; both-effects; both-lists |
| `key` | complete-label matching (`relation_label_key`) | case/spacing/colon; inverses `implements` / `verifies`; substring `implement` |
| `config` | YAML → effective cancel and complete keys and warning count | missing, `null`, `[]`, scalar, mapping, non-string, illegal label, duplicate keys, a key in both lists (dropped from both), `cancel_labels` alone |
