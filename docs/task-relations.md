# Task relations

A task can relate to any other task with an optional label, from Bob
Mac Capture (`~` / `~~`) and from the Obsidian Task Card (`l` /
`⇧L`). The relation is authored once on the source's
`🪢 **LINKS:**` line, and a generated `🪢 **LINKED FROM:**` line on
the target always converges to it. Live chips render both ends
correctly, and relations never block, promote, consume, schedule,
stamp, or route anything.

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
  no other relation writer can change those notes.

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
| Closes, cancels, reopens | None. |

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
   minted `^id`.
3. **Engine agreement (LW14, LK\*).** Rust and JS agree on every LK,
   LW, and LM vector, malformed cases included.
4. **Live view (LC8).** Chips render from the live index, never from
   the stored mirror.
5. **Convergence (LR3).** Canonical sorting makes two machines produce
   the same bytes, and the next reconcile heals any textual conflict.
6. **Read-only respect (LR7).** Relation writes leave `done/` and the
   previous daily note alone, except `bob relations rename`, which
   rewrites relation-label spans only and preserves every other byte.

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
| LR18 | relation added / removed / relabelled between runs | only LINKED FROM lines and minted ids differ |
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
| LX13 | any relation capture | no `[fresh::]`, lane, status, or Pomodoro change on S or T |
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
| LG13 | any relation gesture on an inbox task or a `]s` landing | no stamp, lane change, inbox routing, or walk advance |
| LG14 | kill switch off | no mirror writes and no source mints |
| LG15 | Tab marks two targets, then ↵ | both linked in one gesture and one notice |

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

Rules RV1–RV9 are shipped.

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
  bob relations rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline]
  bob relations unhide LABEL… [-f/--format human|json]
  ```

  Hidden internal arguments: `--via CLIENT` (default `bob-cli`) on
  every writer, and `--allow-unused` on `hide`. Clients that hide a
  label they just wrote, before bob can see it on disk, pass
  `--allow-unused`.

- **Human list** (mint relation accent on a TTY, dim counts; still
  readable with `NO_COLOR`):

  ```text
  RELATIONS  3 labels · 6 links · 1 hidden
    inspired by      3 links
    implements       2 links
    duplicate of     1 link     ⇄ duplicated by     ◌ hidden   (with -a)
  ```

- **JSON list:**
  `{schema_version, ok, counts:{labels, links, hidden},
  labels:[{key, name, count, hidden, inverse, spellings,
  replacement, hide_replacement}], renamed_reappeared, warnings}`.
  `renamed_reappeared` reports an OLD label used again after its rename;
  it reports the row and never rewrites the new use.
- **Writers** emit `{schema_version, ok, …}`. Human hide output reads:
  `🪢 Hid “implements” from relation suggestions · 2 links keep their
  label · undo: bob relations unhide implements`.
- **Refusal codes:** `unknown_label`, `invalid_label`, `unchanged`,
  `label_exists`, `malformed_links_line`, `lock_busy`, `dirty_paths`,
  `not_a_worktree`, `sync_failed`, `verify_failed`, and `stale_note`.
- **`capture-complete` `relation_label`**: rows come from the fast relation-label
  catalog, use canonical keys and display spelling, include inverse phrases and
  counts, omit hidden rows, expose exact typed matches (including hidden state),
  and return `hidden_count`. zsh describes each row as `N links`. Every row
  and the typed row carry `hide_replacement` (always the `||` form);
  a `||` probe keeps its pipe count in `replacement` and gains
  `one_off: true`; the response carries `hidden_count` (present, possibly 0)
  in this context.
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

### Shared vectors

`tests/fixtures/relation_labels/vectors.json` is the single source.
Its `key`, `catalog`, `fold`, `rank`, and `grammar` (LD vectors)
sections are consumed by Rust tests; rename fold cases live in
`fold`. bob-plugins copies the file byte-for-byte.
