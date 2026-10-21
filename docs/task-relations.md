# Task relations

A task relation connects two distinct tasks. With no label, it is an
undirected plain link; with a label, it is a directed statement read
from its source. Storage stays the same: each record is an entry on
one task's `🪢 **LINKS:**` line, and reconcile maintains the generated
`🪢 **LINKED FROM:**` view. Capture `~` creates a plain link, while a
label on `~` or `~~` creates a statement. The Task Card has one Links
stage (`l`); `⇧L` opens it in reverse-label mode. Relations never
block, promote, consume, schedule, stamp, or route anything.

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

- **Task relation** (or **relation**): an inert relation between two
  tasks. It is an **undirected relation** when it has no label and a
  **directed relation** when it has one.
- **Undirected relation** (or **plain link**): a relation with no
  label. It says that two tasks belong together without giving the
  relation a direction.
- **Directed relation** (or **labelled relation**, or **statement**):
  a relation with a label, read from its source as `S ‹label› T`.
- **Label**: the relation text in a directed relation, phrased from
  its source. Writers trim it, collapse inner whitespace, strip one
  trailing `:`, and keep the 1–40 character result.
- **Record**: one entry on one task's `🪢 **LINKS:**` line, identified
  by its owner task, other task, and optional label. There is at most
  one record per ordered pair.
- **Pair**: two distinct tasks `{A, B}`. Its meaning comes from its
  zero, one, or two records; no separate pair flag is stored.
- **LINKS line**: the single managed direct-child line that holds a
  task's outgoing relations: `🪢 **LINKS:**`.
- **LINKED FROM line**: the single generated direct-child line that
  mirrors the relations pointing at a task:
  `🪢 **LINKED FROM:**`. Reconcile owns it; nothing reads it back as
  input.
- **Relationship modifiers**: capture's existing umbrella for
  task-to-task modifiers. It covers `&` (depends on), `*` (blocks),
  `~` (links to), and `~~` (directed relation from the selected task).
  A bare `~~` is incomplete until a label is supplied. The existing JSON names
  (`picker.relationship`, `dependency_target`,
  `relationship_needs_text`, mode `task_dependency`) keep their
  spelling and now cover all four.
- **Task Link** keeps its glossary meaning: a block link to an
  Obsidian task. Entries on a LINKS line are block links to tasks,
  but they are **never** Pomodoro Task Links.

Navigation has one Links stage. `l` opens it for plain links and
outgoing statements; `⇧L` opens that same stage in reverse-label mode.
Neither reads a LINKED FROM line as input.

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
  archived. Relations are history.

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
Common labels such as `related`, `related to`, `relates to`, `see`,
and `see also` remain valid, but completion explains that they create
a directed statement: `‘see also’ makes this directed · leave it empty
for a plain link`.

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
- **LINKS order is authored.** New entries are appended. An explicit
  non-empty label relabels an existing entry in place; an unlabelled
  add never clears an existing label. Re-adding any existing relation
  as a plain link is a no-op.
- **LINKED FROM order is canonical.** Sort by canonical link text
  after ASCII case-folding, comparing Unicode code points, with raw
  code-point order as the tie-break: exactly BK21. Same-note entries
  therefore come first. JS compares code points, never UTF-16 units.
- **Duplicates.** A writer that touches a task with two LINKS lines
  merges the later line's entries into the first (appended, first
  label wins) and deletes the later one. On duplicate targets within
  one line, the first entry wins. Reconcile keeps the first LINKED
  FROM line and deletes the rest.

## 5. Identity, pair rules, and minting

Record identity is the ordered pair `(owner, other)`, each task
resolved by vault-relative path plus block id. A pair `{A, B}` has at
most two records, one owned by each task. Cycles are allowed;
self-relations and non-task targets are refused.

| Records between A and B | Meaning | Shown on each task |
| --- | --- | --- |
| None | Unrelated | Nothing |
| One plain record, or plain records on both sides | One undirected relation | One `(○ other)` chip under `🪢 links` |
| One or two labelled records, with or without plain records | One directed relation per labelled record; plain records are redundant | Labelled entries only |

- **A labelled record subsumes a plain record.** A plain record never
  adds a chip to a pair that already has a statement.
- **A pair has at most one statement in each direction.** Opposite
  statements may coexist, such as `A —x→ B` and `B —y→ A`.
- **Omission never erases meaning.** A plain add never removes or
  changes a label; clearing a label is an explicit write.
- **Direction comes from the sentence, not the storage side.** For a
  plain link, the owner is provenance only. When a label is added, its
  record belongs to the task named as the sentence's source.
- **Writers clean up redundant plain records** whenever a write
  already changes the pair and the record is writable. Redundancy can
  come from hand edits, sync races, or a failed cross-note follow-up;
  it renders nothing, reconcile warns about it, and the next writer
  touching the pair removes it when possible.

An endpoint is read-only when it is in `done/` or in the previous
daily note. A `~~` source must also pass its existing refusal rules: it
cannot be a daily note, `done/`, `_templates`, `_generated`,
`_conflicts`, a dot-directory, or fenced code. Removing the other
task's redundant plain record follows mirror writability: any active
note, including today's daily note, is writable, but `done/` and the
previous daily note are not.

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
- **Plain-link storage.** A new plain record goes on the current task.
  In Obsidian, if that task is read-only and the other task is
  writable, the record goes on the other task instead; a plain link
  has no direction. Capture's owner is always writable.
- **A broken link is never rebound** by title or by a reused id.
  Supported moves (`bob task move`, Ctrl+Shift+M, `bob task archive`,
  and Obsidian renames) already rewrite links on every line.

## 6. The LINKED FROM projection

```text
sources   = #task lines with a LINKS line in: active notes (writable)
            ∪ the previous daily note (read-only) ∪ every .md under done/ (read-only)
edges     = { (S, T, label) : an entry on S's LINKS resolves to #task T ∧ T ≠ S }
            (unresolved, non-task, self, and malformed are warned and omitted; never pruned from LINKS)
mirror(T) = sort_canonical({ (S, label) : (S, T, label) ∈ edges
                             ∧ ¬(label = ∅ ∧ T's own LINKS names S) })
T owns a LINKED FROM line ⇔ mirror(T) ≠ ∅ ∧ T is writable (not done/, not the previous daily note)
```

- **Open or closed T** gets a maintained mirror. Unlike BLOCKS, it
  never freezes.
- A plain incoming entry is omitted when T already names its source on
  T's own LINKS line. Labelled incoming entries are always mirrored.
- A malformed LINKS line on T names nobody; it is never evidence for
  pruning a mirror entry.
- **Never prune on uncertainty.** If S's LINKS line is malformed, or
  S's note has duplicate block ids, existing LINKED FROM entries
  naming S are kept and S is never newly added.
- **The `done/` read is narrow.** Byte-prefilter every `done/**/*.md`
  for `LINKS`, parse only those files, and never write there.
  Document it beside the exact-`done/`-target exception in
  `docs/task-status-hooks.md`.

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

Reconcile reports `redundant_relation` once per pair with a redundant
plain record, only when that record is writable, anchored on the
record's LINKS line. For reciprocal plain records, anchor the warning
on the writable record whose `(path, block id)` sorts last. A redundant
record in `done/` or the previous daily note is not warned about. The
messages are `plain link to [[B]] is redundant: [[B]] already links
here · remove one` and `plain link to [[B]] is subsumed by ‘y’ from
[[B]] · remove it`.

## 8. Immediate writers

| Writer | Behaviour |
| --- | --- |
| capture `~` / `~~` (CLI and Bob Mac Capture through bob) | S's LINKS edit, any minted ids, and T's LINKED FROM edit go into **one staged batch** via the cli-core delta planner. A skipped mirror (archived or previous-daily T, a malformed mirror, or the kill switch) is reported, never fatal. |
| nav Links stage, chip ×, Ctrl+D, `api.relations` v2 | Same note: one transaction, one Ctrl+Z. Cross note: the new labelled record commits first, then a best-effort follow-up removes the old plain record (open editor with the relations machine-write annotation, else a preimage-checked `vault.process`). A failed redundant-record removal adds no suffix because the statement already shows correctly; a failed mirror follow-up keeps `· 🪢 LINKED FROM not updated`. Reconcile warns about any redundant plain record. |
| nav hand-edit mirror (cursor leaves an edited LINKS line; undo re-arms) | Diff the burst baseline's (target, label) set against the final set, then mirror the difference as follow-ups. |
| Hand edit of LINKED FROM | Never an edge change. A notice appears once per burst, and reconcile restores the line. |
| Ctrl+Shift+M, `bob task move`, `bob task archive`, Obsidian rename | The LINKS line moves with its task, and generic link rebasing applies. Reconcile re-sorts and repairs the mirrors. |
| Closes, cancels, reopens | None. |

All writers share one planner, implemented in Rust with a JS twin and
pinned by the same LW vectors. Writers compare canonical output
before writing, so a no-op writes nothing and adds no undo entry.
Writes stamp nothing.

The pure pair planner receives each side's optional label and
writability, plus one request. It returns per-side LINKS operations,
LINKED FROM deltas computed from the projection before and after,
`action`, resulting `kind`, optional `moved_from`, and an optional
refusal. It never mints block ids; the writer does that in its own
transaction or batch.

| Request | Pair before | Result | Reported |
| --- | --- | --- | --- |
| Link N to T (plain `~T` or stage ↵) | Unrelated | Add a plain record to N; in Obsidian, use writable T if N is read-only | `added`, `kind: undirected` |
| Link N to T | Any relation | No write; keep every label | `unchanged`, with existing kind and label |
| State N —x→ T (`~T|x`; `~~T|x` makes T the source) | Unrelated | Add `x [[T]]` to the source | `added`, `kind: directed` |
| State N —x→ T | Plain record on source | Relabel it; remove reciprocal plain record if writable | `relabeled`, `previous_label: null`; `moved_from` if removed |
| State N —x→ T | Plain record only on target | Add statement to source, then remove target's record if writable; read-only record remains subsumed | `relabeled`, with `moved_from` if removed |
| State N —x→ T | Source already states y (or x) | Relabel in place (or do nothing) | `relabeled` (or `unchanged`) |
| State N —x→ T | Only reverse statement T —y→ N | Add N's statement; keep the reverse statement | `added` |
| ClearLabel N —x→ T | No reverse statement | Keep the record without a label | `relabeled`, `label: null` |
| ClearLabel N —x→ T | T —y→ N also exists | Remove N's record; keep T's statement | `removed`; notice says `Removed ‘x’ · still linked by ‘y’` |
| Swap N —x→ T | T writable; no reverse statement | Add `x [[N]]` on T, then remove N's record | `relabeled`, `moved_from: N` |
| RemoveStatement S —x→ T | Any | Remove the statement; if no visible relation remains, remove every remaining writable pair record | `removed` |
| Unlink `{A, B}` | Any | Remove every writable pair record; name any read-only record that remains | `removed` |

Refuse only when no writable record can change, naming whether it is
stored on an archived task or the previous daily note. Cross-note nav
moves add the new labelled record before removing the old plain one.
Capture performs both sides in one all-or-nothing batch.

## 9. Gestures on relation lines

| Gesture | LINKS line | LINKED FROM line |
| --- | --- | --- |
| Ctrl+Shift+P | Opens the owner's Links stage | Opens the owner's Links stage |
| Ctrl+Shift+L | Opens the Links stage for the current task | Same |
| Ctrl+Shift+Alt+L | Opens the Links stage in reverse-label mode | Same |
| `!` / `N!`, Ctrl+Shift+Enter | Refused: `🪢 LINKS entries are relations, not Task Links · edit them with Ctrl+Shift+L` | Refused with the generated-line notice |
| Ctrl+Enter / Alt+] / Alt+[ on a link | Acts on the linked task root-only (no strike, re-embed, tree close, or reformat), as on dependency lines | Same |
| Hand edit | Mirrored when the cursor leaves the line | `🪢 LINKED FROM is generated from other tasks' LINKS lines · change it with Ctrl+Shift+L` once per burst |
| Ctrl+D on the Task Card Links row | Removes every relation shown whose record is writable, including plain records at either end and statements in either direction | Same |

Ctrl+D follows `clear_scope = shown`: it removes each plain pair's
writable records, this task's statements, and other tasks' statements
about this task. Read-only records stay and the notice names them. The
Links row shows a single merged list of related tasks, so clear removes
that shown list wherever its writable records are stored.

## 10. Chips

Chips render from the live `related()` view (§11), never from the
stored mirror. Each task has one `🪢 links` block: plain links first
in canonical order, then this task's outgoing statements in authored
order, then incoming statements in canonical order. A pair appears
once; a plain record is hidden when a statement exists for that pair.

- **Plain links** are arrow-free chips, `(○ T)`, in one wrapping row.
- **Outgoing statements** show `‹label› → (chip)`; incoming
  statements show `‹inverse› (chip)` for a known inverse, otherwise
  `(chip) · ‹label› → this`. Each statement gets one visual row.
- **Anatomy.** Reuse the `bob-dep-row` / `bob-dep-chip` family: a
  mini status checkbox from `--task-status-*`, about 40 characters of
  description, `↗ note` for cross-note entries, a hover preview, and
  click / Mod-click. The row accent stays neutral. **Colour carries
  status, never direction.** Closed and archived targets are dimmed
  and never struck. `#hide` tasks are muted. There is no new palette,
  and no `frees` / `waiting on` badges.
- **Mount and collapse.** Put the block on the first relation line,
  LINKS or LINKED FROM. In Live Preview, collapse the other line; in
  Reading view, hide its list item. Selecting either raw relation line
  reveals both. Live Preview uses a StateField-backed block replace
  decoration. If that collapse path proves fragile, use the documented
  continuation row for incoming statements and record the shipped path
  in the plugin docs.
- **Stale and broken.** A stored LINKED FROM entry that is no longer
  live is dashed, with the tooltip `No longer links here · refreshes
  on the next reconcile`. A live edge missing from the text is
  appended in canonical order. An unresolved LINKS target is a dashed
  `⚠ ^id not found`.
- **Collapse.** Three or more closed chips on a row collapse to
  `✓×N`, which expands on click.
- **Controls** (only when nav `api.relations?.version >= 2`): one
  trailing ＋ (aria `Link another task`) calls
  `api.relations.openStage(ref, {reverse?})`. The v2 API also exposes
  `unlink(refA, refB)` and `removeStatement(sourceRef, targetRef)`;
  `removeLink(sourceRef, targetRef)` remains as a documented v1
  compatibility alias. A × on a plain chip (aria `Unlink ‹name›`)
  calls `unlink(this, other)`; a × on a statement (aria
  `Remove ‘‹label›’`) calls `removeStatement(source, target)`. The ×
  is visible on hover and focus. Without navigation, there are no
  controls.
- **Toggle.** `toggle-dependency-chips` covers the relation block too.
- Without the plugin, the raw LINKS and LINKED FROM lines remain
  readable.

**Inverse phrases** (display only; case-insensitive match; never
stored):

| Stored label | Shown on LINKED FROM |
| --- | --- |
| `merged into` | `merged from` |
| `duplicate of` | `duplicated by` |
| `follow-up of` | `followed up by` |
| `superseded by` | `supersedes` |
| `split from` | `split into` |

The Links stage's CURRENT rows use the same table.

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
- **API.** `api.relationIndex` exposes `version`, `generation`,
  `ready()`, `outgoing(path, blockId|line)`, `incoming(path,
  blockId)`, `related(path, blockIdOrLine) → {plain, outgoing,
  incoming}`, and `labelSuggestions(prefix?) → [{label, count}]`
  (case-insensitive dedupe, count order). `related()` merges both
  directions, dedupes plain records by pair, and suppresses plain
  records subsumed by statements. Its plain rows use canonical order,
  outgoing statements use authored order, and incoming statements use
  canonical order. Chips subscribe through `onChange`.

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
`self_relation`, `duplicate_block_id`, or `redundant_relation`. Human output gets a `🪢`
group in the dependency style, and `--dry-run` previews all of it.

**Capture JSON** (every `schema_version` stays 1; additive only):

- `capture-parse`: span kinds `relation_sigil`, `relation_note`,
  `relation_block_id`, `relation_label_separator`, and
  `relation_label`; top-level and per-item `relations[]` (`{raw,
  direction: "to"|"from", kind: "undirected"|"directed", note,
  block_id, quoted, label, label_range, range}`), omitted when empty; needs `relation_target` and
  `relation_label`; top-level `relation_grammar: 1` whenever the draft
  contains a `~` byte. Drafts without `~` are byte-identical to today.
  Relations never appear in `dependencies[]` or `blocked_tasks[]`.
  `kind` is `directed` when the token has a `|` or the `~~` sigil;
  otherwise it is `undirected`. A bare `~~` reports
  `needs: ["relation_label"]` and `relation_label_required` with
  `~~ links need a label · use ~ for a plain link`. Submit refuses it
  with `cannot relate ~~work:foo: ~~ needs a label, as in
  ~~work:foo|"next step" · use ~work:foo for a plain link`; it is
  never silently rewritten and writes or mints nothing.
- `capture-complete`: contexts `relation_to` and `relation_from`
  (the `DependencyCandidate` shape, plus symmetric `already_linked`,
  `existing_kind`, `existing_direction`, `existing_label`,
  `also_prerequisite`, and `disabled_reason`); existing rows remain
  enabled because a label or reverse statement can still be added.
  `existing_direction` is `to`, `from`, or `both` relative to the
  owner for directed pairs;
  `existing_label` prefers the row's direction, then the reverse.
  Context `relation_label` has `suggestions[] {label, count,
  replacement}` and may include `relation.hint` when the normalized,
  case-insensitive label is `related`, `related to`, `relates to`,
  `see`, or `see also`. Its hint is `‘see also’ makes this directed ·
  leave it empty for a plain link`; it does not reject the label.
  `picker.relationship`
  `relations[]` gain `kind`. Every `~~` replacement ends in `|`, so
  clients land in the label step. Shell groups are `links` for `~`
  and `reverse · needs a label` for `~~`.
- `capture-task-id`: `relation_to_replacement` (`~note:id`) and
  `relation_from_replacement` (`~~note:id|`).
- `capture` / `--dry-run`, per item: `relation_updates[]`
  (`{direction, source, target, label, previous_label, kind,
  moved_from?: {path, block_id}, action: added|relabeled|unchanged}`), `linked_from_line_updates[]`,
  `minted_relation_block_ids[]`, and `mirror_skipped[]`
  (`{reason: archived_target|previous_daily_target|malformed_line|disabled}`).
  `direction` identifies which task authors the reported record (`to`
  is the owner, `from` the other task); added and relabeled rows use
  the token's sigil. An unchanged row reports the record that
  subsumes the token, preferring the owner's statement, the reverse
  statement, the owner's plain record, then the other task's plain
  record. Its source, target, label, kind, and direction describe that
  record. Clients ignore direction when `kind` is `undirected`.
  `moved_from` appears when the write removes the other task's plain
  record. Schema version remains 1.

**Copy across surfaces.** Sentence-shaped relation previews use an em
dash and never a `↔` arrow; colour always shows task status, not
direction.

| Relation | Chips and Links-stage CURRENT | CLI capture | Mac preview | Notice |
| --- | --- | --- | --- | --- |
| Plain | `(○ X)` under `🪢 links` | `🪢 links "X"` | `🪢 links “X”` | `🪢 Linked “This” and “X”` |
| This —x→ X | `‹x› → (○ X)` | `🪢 x → "X"` | `🪢 x → “X”` | `🪢 “This” — x → “X”` |
| X —x→ this | `‹inverse› (○ X)`, or `(○ X) · ‹x› → this` | `🪢 "X" — x → this` | `🪢 “X” — x → this` | `🪢 “X” — x → “This”` |
| Already related | — | `🪢 already linked "X"`, plus ` · x` when directed | Same | — |
| Relabelled | — | `🪢 relabeled x → "X"`, plus ` · was y` | Same | `🪢 “This” — x → “X”` |
| Removed | — | — | — | `🪢 Unlinked “X”` / `🪢 Removed ‘x’ · still linked by ‘y’` |
| Dry run | — | `would link "X"` / `would add x → "X"` / `would add "X" — x → this` / `would relabel …` | — | — |

The CLI locator suffix always names the other task. The inverse-phrase
table above is display-only and unchanged; unknown labels have no
inverse. Bob Mac Capture keeps `arrow.left.arrow.right` for plain rows
and uses `arrow.right` for statements.

## 13. Guarantees

1. **One plain relation per pair.** Adding the same plain link from A
   and B yields one relation; the second write is `unchanged`.
2. **Labels are preserved.** A plain add never changes or removes a
   label, and a pair renders at most once, never both plain and
   labelled.
3. **Direction follows the sentence.** A labelled relation means the
   same thing whichever task stores its record.
4. **Safe follow-up failure.** If a cross-note redundant-record
   removal fails after adding a statement, the display is correct and
   reconcile reports `redundant_relation`.
5. **Single source (LR11).** Delete every LINKED FROM line and run
   reconcile once: the lines come back byte-identical, and a second
   run changes nothing.
6. **Inertness (LR18, LX13, LG13).** Relation operations leave every
   status, lane, field, dependency, Pomodoro section, and review-walk
   state byte-identical, apart from a minted `^id`.
7. **Engine agreement (LW14, LK*).** Rust and JS agree on every
   contract vector, malformed cases included.
8. **Live view (LC8).** Chips render from the live index, never from
   the stored mirror. Canonical sorting and reconcile make the mirror
   converge (LR3); no write touches `done/` or the previous daily note
   (LR7).

## 14. Lifecycle

- **Close, cancel, reopen S or T.** None. Relations are history;
  nothing changes.
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
| LW4 (narrowed) | re-add T with a new non-empty label | relabeled in place; order kept |
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
| LW15 | plain add when a plain record already exists on either side | `unchanged`; byte-identical |
| LW16 | plain add when a statement exists in either direction, including add-without-label on a labelled entry | `unchanged`; label kept |
| LW17 | N —x→ T requested; writable T holds plain `[[N]]` | N gains `x [[T]]`; T's record is removed; T's mirror gains `[[N]] (x)`; N's mirror drops `[[T]]`; `moved_from: T` |
| LW18 | as LW17, but T's record is in `done/` | N gains `x [[T]]`; T's record stays subsumed and unwarned; N's mirror drops `[[T]]` |
| LW19 | clear the only label | the entry stays where it is, without its label |
| LW20 | clear N —x→ T while T —y→ N exists | N's record removed; T's statement kept |
| LW21 | swap N —x→ T | T gains `x [[N]]`; N's record removed; refused while T —y→ N exists |
| LW22 | unlink a pair with plain records on both sides / remove the only statement while a redundant plain remains | every writable record removed; pair unrelated |
| LW23 | N —x→ T requested; both sides hold plain records | N's entry relabeled in place; T's reciprocal removed (`moved_from: T`) |

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
| LR18 | relation added / removed / relabelled between runs | only LINKED FROM lines and minted ids differ |
| LR19 (changed) | plain A → B and plain B → A | neither gets a LINKED FROM entry; one `redundant_relation` |
| LR20 | `--dry-run -f json` | identical rows; nothing written; no mints persisted |
| LR21 | owner's note modified less than 2 s ago | edits deferred |
| LR22 | S archived by `bob task archive` | T's entry already rewritten to `[[done/…#^s]]` by link repair; reconcile keeps it unchanged |
| LR23 | S's note has a duplicate `^s` | S not listed; `duplicate_block_id` warning |
| LR24 | target is a `^prj` task | the `^prj` line gets the mirror like any task |
| LR25 | plain A → B and labelled B —y→ A | B's mirror omits A; A's mirror lists `[[B]] (y)`; `redundant_relation` on A's line |
| LR26 | labelled A —x→ B and labelled B —y→ A | each mirror lists the other with its label (old LR19) |
| LR27 | plain A → B with A in `done/`, and B —y→ A | B's mirror omits A; no warning; `done/` untouched |
| LR28 | B's LINKS line is malformed; plain A → B; B's malformed line mentions A | B's mirror keeps listing A |

### LX — capture execution

| # | Draft or situation | Outcome |
| --- | --- | --- |
| LX1 | `Fix retries ~bob_done:capture-star\|"follow-up of"` | new N with `🪢 **LINKS:** follow-up of [[done/bob_done#^capture-star]]`; `mirror_skipped` `archived_target`; N not minted for a mirror |
| LX2 | `Write outline ~~sase_blog:prj\|"next step"` | `sase_blog`'s `^prj` LINKS gains `next step [[mac_inbox#^write-outline]]`; N minted; N born with `🪢 **LINKED FROM:** [[sase_blog#^prj]] (next step)` |
| LX3 | `@bob+fix-retries ~sase:retry-design\|"follow-up of"`, then a rerun, then a new label | mirror updated in the same batch; rerun `unchanged`; new label `relabeled` |
| LX4 | `Ask plumber &job:parts ~job:cleanup` | `&` blocks as today; `~` changes no status |
| LX5 | `N ~T`, T in an active note (today's daily note included) | T's mirror written in the same batch; N minted |
| LX6 | kill switch off | no mirror and no source mint; `~~` still mints N |
| LX7 | `~~` target in a daily note, `done/`, `_templates`, or fenced code | refused; vault byte-identical |
| LX8 | self target, non-task target, missing block id | refused with a teaching error |
| LX9 | S's LINKS line malformed | refused; vault byte-identical |
| LX10 | T's LINKED FROM malformed | capture succeeds; `mirror_skipped` `malformed_line` and a warning |
| LX11 | `x ~job:cleanup` | `operator_relation_error` |
| LX12 | any LX vector, dry run vs live | identical JSON apart from `dry_run` |
| LX13 | any relation capture | no `[fresh::]`, lane, status, or Pomodoro change on S or T |
| LX14 | `N ~T1 ~T2 ~T1` / `N ~T1\|a ~T1\|b` | T1 then T2, duplicate no-op / refused |
| LX15 (retired) | `N ~T ~~T` | now an incomplete draft (LP2) |
| LX16 | a same-note S and T | both edits composed against staged text; one write of the note |
| LX17 | `N ~T ~~T\|x` | one statement T —x→ N; the plain token produces no row |
| LX18 | `@N+n ~T` where T —y→ N | `unchanged`; row `kind: directed`, `direction: from`, `label: y`; `🪢 already linked "T" · y` |
| LX19 | `@N+n ~T\|x` where T holds plain `[[N]]` | N gains `x [[T]]`; T's record removed; mirrors update; `relabeled` + `moved_from`; one batch |
| LX20 | `@N+n ~T` where N holds `see [[T]]` | `unchanged`; label `see` kept |

### LP — capture parse

| # | Draft | Reading |
| --- | --- | --- |
| LP1 | `Fix it ~work:foo` / `~work:foo Fix it` / `@cash ~work:foo Fix it` | relation `to` (trailing / leading / past `@cash`) |
| LP2 (changed) | `Fix it ~~work:foo` | relation `from`, `kind: directed`, needs `relation_label`, diagnostic `relation_label_required`; submit refuses; zero writes and mints |
| LP3 | `Fix ~work:foo now` | literal (mid-line) |
| LP4 | `Plan trip ~5m`, `~2h`, `~5pm`, `Clean ~/Downloads` | literal |
| LP5 | `Read ~~old~~ notes`, `Read ~~old~~` | literal (strike guard) |
| LP6 | `Fix it ~`, `Fix it ~~`, `Fix it ~wo`, `Fix it ~work:` | need `relation_target` |
| LP7 | `Fix it ~work:foo\|"follow-up of"`, `Fix it ~work:foo\|"follow up of"` | label spans and label |
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
| LP19 | `Fix it ~work:foo` / `~work:foo\|"follow-up of"` / `~work:foo\|` | `kind` `undirected` / `directed` / `directed` (needs label) |
| LP20 | `relation_to` row for a task that links here plainly / that states y here | `already_linked`, `existing_kind: undirected` / `directed`, `existing_direction: from`, `existing_label: y` |
| LP21 | `relation_from` row; `capture-task-id` for an id-less `~~` target | replacement `~~note:id\|`; `relation_from_replacement` `~~note:id\|` |
| LP22 | label query `See also` | `relation.hint` present; the label is still accepted |

### LG — Obsidian gestures

| # | Gesture | Outcome |
| --- | --- | --- |
| LG1 | Links stage ↵ on a same-note T | S's LINKS, T's mirror, and S's mint in one transaction; one Ctrl+Z undoes all |
| LG2 | cross-note T / the follow-up finds T changed | S commits, then T's mirror follows / S stands, suffix `· 🪢 LINKED FROM not updated` |
| LG3 | ⇧↵ then a label / an empty label | labelled entry / plain entry |
| LG4 | CURRENT ↵ / ⇧↵ | unlink with the mirror entry removed / relabel in place |
| LG5 | id-less T | block-ID prompt; Escape writes and mints nothing |
| LG6 | closed T / archived T | allowed / allowed, no mirror, suffix `· archived: shown live only` |
| LG7 (changed) | `⇧L` with a label on S | S's LINKS gains `label [[this]]` |
| LG8 | Ctrl+Shift+P on either relation line; Ctrl+Shift+Alt+L | the owner's Links stage; reverse-label mode |
| LG9 (changed) | Ctrl+D on the Task Card Links row | follows `clear_scope = shown` |
| LG10 | hand edit adds / removes / relabels a LINKS entry, cursor leaves | mirrors follow; Ctrl+Z re-fires the mirror |
| LG11 | hand edit confined to LINKED FROM | notice once; no edge change |
| LG12 | `3 Ctrl+Shift+L`, choose T with a label | four sources each gain one labelled entry |
| LG13 | any relation gesture on an inbox task or a `]s` landing | no stamp, lane change, inbox routing, or walk advance |
| LG14 | kill switch off | no mirror writes and no source mints |
| LG15 | Tab marks two targets, then ↵ | both linked in one gesture and one notice |
| LG16 | ⇧↵, a label, ⇄, ↵ | stored on the other task; a same-note write is one Ctrl+Z |
| LG17 | × on a plain chip at the non-storing end | the pair unlinked; notice `🪢 Unlinked “X”` |
| LG18 | ⇧L, pick a task, empty label, ↵ | "Make it a plain link" offered; nothing written until chosen |
| LG19 | Links stage RESULTS | tasks related in either direction are not listed (they are in CURRENT) |
| LG20 | Task Card `Links…` row | counts related tasks from the live index; loading state is never zero while the index loads |

### LC — chips

| # | Situation | Render |
| --- | --- | --- |
| LC1 (changed) | one unlabelled relation | `🪢 links (○ T)`, no arrow |
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
| LC12 | `toggle-dependency-chips` | toggles the merged relation block with the dependency rows |
| LC13 | malformed or not-a-line row | no chips |
| LC14 | an entry typed in the editor before save | the target's chip row updates in the same session |
| LC15 | plain with X (stored on both sides) and Y (stored on Y), `follow-up of → Z`, `merged into` from W | one block: `(○ X) (○ Y)`, then `‹follow-up of› → (Z)`, then `‹merged from› (W)` |
| LC16 | plain record stored on the other task | the same `🪢 links (○ S)` as at the storing end |
| LC17 | a pair with a plain record and a statement | only the statement renders |
| LC18 | a task with both LINKS and LINKED FROM | one block on the first line; the second collapses in Live Preview and is hidden in Reading view; selecting either reveals both raw lines |
