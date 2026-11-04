# Split bullets out into tasks

Splitting promotes bullets that grew into real work — a meeting note, a
task's note, a Future Work entry — into tasks of their own. It is the inverse
of [merging tasks](task-merge.md): merge folds tasks into a container, split
turns notes back into tasks. Moving tasks between notes keeps their identity
(see [Move open tasks between notes](task-move.md)).

This file is the contract every implementation cites. The Rust side is
`src/native/task_split/plan.rs` (pure planner), surfaced as
`bob task split` and the Obsidian Ctrl+Shift+T gesture. The JavaScript twin
ports the planner against the copied vectors. If an implementation disagrees
with a vector, fix this doc first in the same change.

## 1. What is splittable

A splittable bullet is one of:

- a **first-level authored note** of a task;
- a **drawer entry**, meaning a first-level bullet inside a drawer;
- a **section bullet**, meaning a column-0 bullet under an eligible H2 with
  no owning task.

It must be a plain bullet with non-empty text once its `^anchor` is removed.
An unchecked `- [ ] text` without `#task` is also splittable, and it becomes
an open task.

These are never splittable, and each gets a teaching notice or code:

| Line | Code |
| --- | --- |
| a `#task` line | `split_already_task` |
| a stub (a bullet whose only content is one task link) | `split_stub` |
| a drawer title | it opens the picker instead |
| any managed line (Depends-On, BLOCKS, LINKS, LINKED FROM, any log) | `split_managed_line` |
| a Pomodoro line | `split_managed_line` |
| a checked checkbox | `split_not_splittable` |

## 2. What it writes

- **The new task.** `- [ ] #task <bullet text> [created:: today] ^<id>`,
  which is exactly the line capture writes for a new task with a minted ID.
  It therefore enters the NEW tier.
  - An existing trailing `^anchor` on the bullet **becomes** the task ID, so
    links to it now resolve to a task. Otherwise the ID is minted with
    `mint_block_id` against the destination note's used IDs.
  - The bullet's children travel byte-for-byte, re-based one level up. An
    ALL-CAPS child with notes therefore naturally becomes the new task's
    drawer.
  - Nothing is inherited from P: no priority, schedule, dependencies, lane,
    freshness, or explicit type. Content is never re-parsed as capture
    grammar.
- **The stub.** The bullet's head line is replaced, at the same position and
  indentation, by `- [[#^id|Alias]]` (or `- [[note#^id|Alias]]` across notes,
  in Bob's canonical link form). The alias is the bullet text with:
  - the `^anchor` and inline fields removed;
  - wikilinks and Markdown links flattened to their display text;
  - `**`, `__`, `==`, `~~`, and backticks removed;
  - `|` replaced by `/`, `[` and `]` dropped, and whitespace collapsed;
  - truncation at 80 characters on a word boundary with `…`.

  A split drawer entry is replaced inside its drawer, so the drawer is never
  emptied.

- **The mention.** Each new task gets
  `- 🪢 **LINKED FROM:** [[P]] (💬 mentions)` at `relation_insert_slot`, in
  the same write. If P lacks an `^id`, one is minted silently on P's line in
  the same write. With `relations.linked_from_mentions: false` (or
  `linked_from_lines: false`), the line is omitted; reconcile converges
  either way.
  - A section bullet has no owning task, so it yields no mention.
- **Placement.**
  - New tasks go directly after P's block, at P's indentation, in bullet
    order. That is one note and one transaction.
  - When P lives in a **daily note** (an Event Task or `^gtd`), new tasks go
    to the `## Tasks` intake of P's `[root::]` note, or else the default
    inbox, because daily notes hold no ordinary tasks. That is a cross-note
    write.
  - Section bullets go to their own note's `## Tasks` intake. `## Tasks` is
    appended when missing, as the move engine does for areas.
- **Selection order.** Picks may be non-contiguous. New tasks are always
  written in document order.
- **The walk and the cursor.** Split never answers or advances the `]s`
  walk. In Obsidian, the cursor lands on the first new task, and `Ctrl+O`
  returns.

## 3. Command line

`bob task split` rewrites task lines across the vault, so it lives under
the `bob task` noun. Options are alphabetical and every long option has a
short alias.

```text
bob task split [OPTIONS] <TASK|NOTE> [-m TEXT | -n N]...
  -d, --dry-run                Plan and print only; never locks, syncs, mints, creates, or writes
  -f, --format human|json      Default: human
  -m, --match TEXT             the splittable bullet containing TEXT; repeatable; unique or refuse
  -n, --nth N                  the Nth splittable bullet as listed; repeatable
  -o, --offline                Skip both sync cycles; commit locally
  -r, --retry-timeout SECONDS  Lock wait and re-plan budget
  -s, --section TITLE          split that section's bullets
      --bullet-ref LINE:DIGEST hidden stale-safe picker selector
```

A task locator is `note:block-id`, split on the last `:`; `[[note#^id]]`
is also accepted.

- **No selector** exits 2 with a numbered candidate list, for example:

```text
1  Ping Alice…
2  Write the rollback runbook (+1)
3  FUTURE WORK › Try logical replication
```

`-f json` emits the same candidates as `candidates[]`. A hidden
`--bullet-ref <line>:<digest>`, modelled on capture's hidden
`--task-ref`, gives pickers and agents a stale-safe selector. A stale
selector is refused (`split_stale_ref`) and never falls back to similar
text.

- **Shared shell** (extracted from `task_move/command.rs`): every verb takes
  the maintenance lock, pre-syncs (skipped by `-o`), runs the scan → plan →
  write-plan retry loop, makes guarded staged writes, makes one scoped git
  commit, and post-syncs. `--dry-run` skips the lock, sync, Git, and writes,
  and its plan equals the live plan. Exit 2 means usage and exit 1 means
  operational. The commit subject is `bob task split db#^upgrade (2 tasks)`.
- **JSON** uses schema 1, the same envelope as `bob task move`: `ok`,
  `dry_run`, `warnings`, `repaired_files[]`, `git`, `applied_paths[]`,
  `deferred_paths[]`, `recovery_directory`, and
  `error{stage, code, message, hint}`.
  - **`splits[]`**:
    - `owner{path, line, block_id, minted_id}`;
    - `tasks[{path, line, block_id, title, anchor_reused, child_lines}]`;
    - `stubs[{path, line, text}]`;
    - `placement{kind: below_owner|tasks_section|root_note|inbox, path}`.
- **Human output.** Coloured with the capture styler. A dry run says
  _Would …_. Split output looks like this:

```text
✂ Split 2 notes into tasks · db.md
  ◯ Ping Alice about the cutover window   ^ping-alice-about · db.md:7
  ◯ Write the rollback runbook (+1)       ^write-rollback · db.md:9
```

## 4. Obsidian gestures (summary)

Ctrl+Shift+T splits the bullet under the cursor, the counted bullets
(`N<Ctrl+Shift+T>`), or every splittable bullet a visual selection touches;
on a task line it opens a picker for all splittable notes, and on a drawer title
it opens the same picker scoped to that drawer. Nothing starts marked. `Tab`
marks or unmarks the highlighted row and advances; `Enter` splits marked rows,
or just the highlighted row if none are marked; `Esc` closes without writing.
Section bullets promote into that note's `## Tasks` intake and leave their live
link stub under the section.

Same-note splits are one editor transaction (one Ctrl+Z). A daily-note owner
routes new tasks to its `[root::]` note's `## Tasks` intake, or to the default
inbox when it has no root. Bob writes the destination first and guards each
file write against its planned preimage, rolling back a partial write when
possible. A cross-note split shows an **Undo split** notice action; it restores
the saved postimages only while every affected note still matches, and lists
the paths when edits make Undo unsafe. The cursor lands on the first new task,
and `Ctrl+O` returns to the source note; if Bob cannot open the destination, it
reports the destination path. Split never advances the review walk. The full
gesture contract and cursor landing are documented in this section and the
Obsidian keymap reference.

## 5. Canonical copy

The `contract` phase records this table here and in
[Merge tasks into a task, drawer, or section](task-merge.md). Rust, JS, and
the Mac app use these strings verbatim. Bob authors `message`/`detail`
fields, and clients render them.

| Key | Text |
| --- | --- |
| split notice | `✂ Split N note(s) into tasks · ◯ A… · ◯ B…` |
| split_managed_line | `Managed line — Bob writes it; split a note or drawer entry instead` |

Glyphs: ↪ move, ⤵ merge, ✂ split, ⊘ cancelled, ⛓ dependency, ◌ removed
link, ↻ repair, and ⚠ warning.

## 6. SP vector index

`tests/fixtures/task_split/vectors.json` pins both planners byte-for-byte.
Each case carries `{id, title, input, expect}` where `input` holds
`{files, owner, selection, today, settings}` and `expect` holds either
`{ok: true, files, tasks, cursor, notice}` or `{ok: false, code, message}`.
bob-plugins copies the file byte-for-byte; bob-cli is authoritative.

| ID | Covers |
| --- | --- |
| SP1 | Cursor on a first-level authored note |
| SP2 | Cursor on a nested line (splits the owning first-level note) |
| SP3 | Cursor on a drawer entry (replaced inside its drawer) |
| SP4 | Count mode selects the first N splittable siblings |
| SP5 | Clamped count shows a notice and splits what exists |
| SP6 | A contiguous line selection splits every touched head line |
| SP7 | Non-contiguous `nth` picks split in document order |
| SP8 | `match` text selects the unique bullet, else refuses ambiguous or missing |
| SP9 | A trailing `^anchor` on the bullet becomes the task ID (no mint) |
| SP10 | P without an `^id` gains one silently in the same write |
| SP11 | `|` in the text becomes `/` in the stub alias |
| SP12 | `]]` and `[` / `]` in the text are dropped in the alias |
| SP13 | `#` in the text stays in the task title and alias |
| SP14 | Wikilinks and Markdown links flatten to display text in the alias; the mint uses the flattened words |
| SP15 | Emphasis (`**`, `__`, `==`, `~~`, backticks) is stripped in the alias only |
| SP16 | An 80+ character title truncates on a word boundary with `…` in the alias |
| SP17 | An ALL-CAPS child with notes becomes the new task's drawer |
| SP18 | Tab-indented notes re-base to the destination indent unit |
| SP19 | Space-indented notes keep relative structure under a tab-indented parent |
| SP20 | A daily-note Event Task owner routes new tasks to P's `[root::]` note `## Tasks` |
| SP21 | A daily-note owner with no root routes to the inbox |
| SP22 | A section bullet routes to its own note's `## Tasks` intake |
| SP23 | `## Tasks` is appended when missing for section bullets |
| SP24 | Mentions disabled omits LINKED FROM and still converges under reconcile |
| SP25 | `#task` line refused as `split_already_task` |
| SP26 | Stub-only bullet refused as `split_stub` |
| SP27 | Managed Depends-On line refused as `split_managed_line` |
| SP28 | Pomodoro line refused as `split_managed_line` |
| SP29 | Checked checkbox refused as `split_not_splittable` |
| SP30 | Section-bullet selection kind routes without an owner mention |
