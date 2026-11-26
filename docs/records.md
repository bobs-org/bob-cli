---
title: Albums and record notes
description: Vault album notes, record schemas, capture creation, and how Bob plans record-note files.
---

# Albums and record notes

A record note represents one durable thing, such as a restaurant, podcast, show,
or movie. Canonical records use `type: "[[record]]"` with
`parent: "[[<album>]]"`; `record.noun` names the kind and the legacy type
spelling (`[[restaurant]]`) warns as `record_type_legacy`. An **album** is a
vault-root note that owns one kind: its stem is the folder and capture route
(`eat.md` owns `eat/` and `@eat/`). Album schemas route every record consumer
through the effective vault (`--bob-dir`/`-b` wins over `BOB_DIR`). A leftover
`records:` key in host configuration is ignored; albums are the only source of
record schemas. The hidden `bob records init` command always exits 2 and
points to `bob albums create`.

This contract covers album reports, schema loading, value validation, filename
planning, and the Markdown Bob writes. Capture creates notes through the same
planner; exact token forms live in [Capture syntax](reference/capture-syntax.md#record-notes)
and the workflow in [Capture](capture.md#record-notes). Record types are
collections defined in an album's `record:` block. They are not Task Types
(`#fix`, `bob types`).

## Albums

An album is a vault-root Markdown file with `type: "[[album]]"` (any form
`frontmatter_type_names` accepts). Identity is the file stem; that stem is the
record folder (`eat.md` owns `eat/`). Albums must live at the vault root.

```markdown
---
parent: "[[body]]"
type: "[[album]]"
title: Eat
summary: Places to eat around here.
emoji: 🍽️
index_by: location
record:
  noun: restaurant
  label: Restaurant
  plural: restaurants
  fields:
    status:
      required: true
      values: {to_try: 🔖 To try, liked: 💚 Liked, not_liked: 🚫 Not liked}
    category: {required: true, suggest: true}
    location: {required: true, suggest: true}
---
```

- **Parent:** exactly one quoted `"[[stem]]"` that resolves to an area or
  project. The chain must reach a Root Parent and must not cycle. Folder and
  parent on the record schema are derived (`dir` = stem, `parent` =
  `"[[stem]]"`); they are not album schema keys.
- **Stem / noun:** the stem is a lower-case capture route and cannot be
  `done`, `ref`, a dot folder, or `.bob`. `record.noun` is required
  `[a-z][a-z0-9_]*`. Reserved nouns include today's record ids plus `album`
  and `record`.
- **`record.fields`:** optional. Field kinds, defaults, lists, choices, and
  legacy patterns follow [Schema](#schema) below. `label` defaults to the
  noun in title case; the display `singular` is that lowered label
  (acronym labels stay as-is), and `plural` is the singular plus `s`.
- **Title:** frontmatter `title` (one nonblank line) wins, else the first H1
  before the first bullet or `##` (wikilinks to display text), the first
  alias, then the humanized stem; an unusable `title` warns
  `album_title_invalid`.
- **Summary:** one plain line; more than 140 characters produces a warning.
- **Grouping:** `status` choices when present, otherwise the first choices
  field, otherwise none. Empty groups are omitted. Missing, unknown, or
  list-mismatched values count as `needs_status`. This auto rule picks the
  title tally, the report's `groups`, and the index headings, unless
  `index_group_by` overrides the headings.
- **`index_group_by`:** an optional top-level key beside `walk_by`,
  `index_by`, and `index_facets` that names the field heading the `[!records]`
  index. The field must be declared `Choices` with `list: false`, the same
  test as `walk_by`. Headings are its choice labels in declared order, so the
  YAML order is the outline order; empty choices are omitted, members keep
  `title_order`, and missing, blank, malformed, list-shaped, or undeclared
  values go last under `⚠️ Needs <field>`. A heading group larger than 12
  still renders `index_by` subgroups. Only the index headings (and the
  default facets below) change: the title tally, `groups`, `needs_status`,
  peeks, the owner catalog chip, record `group` JSON, and the
  `group_before`/`group_after` of `bob albums update` stay on the Grouping
  field. Naming the Grouping field itself leaves the index byte-identical,
  and an album without the key never changes. A blank, multi-line, or
  non-string value warns `album_index_group_by_invalid`, an undeclared field
  `album_index_group_field_unknown`, and a field that is not single-value
  choices `album_index_group_by_not_choices`; each falls back to the
  Grouping field and leaves the index intact. With `walk_by` naming the same
  field, the index order is exactly the `]n` walk order.
- **`index_by`:** a declared field name. Groups larger than 12 with at least
  two distinct normalized values render `index_by` subgroup headings (and
  peeks follow that order). Values fold by case and wikilink display text.
  The heading is the most frequent displayed spelling, preferring a
  plain-text spelling on ties. Missing values sort last under `**—**`.
  Smaller groups stay flat. Peeks are the first three rendered titles.
- **`index_facets`:** an explicit list of at most three field names, or the
  default of other choice fields then `suggest` fields, excluding the heading
  field (the `index_group_by` field when it is in effect, otherwise the
  Grouping field, so `status` becomes an eligible default facet under
  `index_group_by`), capped at three. An explicit list is honored exactly as
  written. Flat rows keep `index_by` as a facet; subgrouped
  rows omit that repeated field unless `index_facets` is explicit. Choices
  show labels, links show display text, lists join with `, `, numbers stay
  plain, and empty values disappear.
- **Records index:** a bob-owned `[!records]` callout whose title is the
  tally (`N <plural>`, `1 <singular>` for a single record, plus nonzero
  group counts, then `⚠️ k need a status`, `needs` when k is 1).
  Rows are `[[<album>/<stem>|<title>]]`; alias text drops `|[]#^`, then
  alias and facets escape active Markdown.
  Empty albums keep a teaching index (`No restaurants yet` / `Name @eat/`).
  First write inserts `## Records` before the first real H2 (or at EOF);
  an authored `## Records` heading with no owned region is adopted, so only
  the callout is added at the end of its section. Later writes patch only
  the callout. `list` and `check` report
  `index.state` as `current`, `missing`, `stale`, or `damaged` and never
  write. Missing and stale indexes warn `album_index_stale` with
  `bob albums sync`; region damage or incomplete inputs error
  `album_index_damaged` and leave the authored region intact. Album-backed
  record writes rebuild this index inside their own transaction (see
  [`bob records`](#bob-records)).
- **Owner catalogs:** each area or project that owns albums, or that already
  has a `## Albums` region, carries a bob-owned `[!albums]` callout of
  stable rows. Direct albums come first, then descendant owners as
  `> - 🧩 [[project]]` groups. A row is
  `> - <emoji> **[[stem|Title]]** — <summary>`; missing emoji or summary is
  omitted. Counts, dates, freshness, peeks, and colors are never written.
  First insertion lands at the end of an existing `## Notebooks` section,
  otherwise before the first real `## Tasks`, otherwise at EOF; an authored
  `## Albums` heading with no owned region is adopted instead of duplicated.
  Later edits stay at the existing callout. Empty subtrees delete the callout and remove
  the heading only when its section is then blank. `list` and `check` report
  `catalogs[].state` as `current`, `missing`, `stale`, or `damaged`. Missing
  and stale warn `album_catalog_stale` with `bob albums sync`; damage errors
  `album_catalog_damaged` and leave the authored region intact. Record
  capture and `records new|update` never rewrite owner notes.
- **`last_record`:** the latest valid `created` date (`YYYY-MM-DD`) among
  folder members, or null. A missing folder is an empty album. Immediate
  Markdown members still count when the schema is broken.

The Obsidian presentation of these cards is [Album cards](reference/plugins/albums.md).

`bob albums` lives in the Vault section of `bob --help`. Bare `bob albums`
defaults to the read-only `list`.

```text
bob albums [list] [-b DIR] [-f human|json] [-r ROOT]
bob albums check [-b DIR] [-f human|json]
bob albums sync [-b DIR] [-d] [-f human|json]
bob albums tidy [STEM] [-a] [-b DIR] [-f human|json]
bob albums create PARENT STEM -n NOUN -s SUMMARY [-b DIR] [-d] [-e EMOJI] [-f human|json] [-l LABEL] [-o] [-t TITLE]
bob albums update STEM [-b DIR] [-d] [-f human|json] [-i FILE]
```

- Every long option has a short alias; options stay alphabetical.
- Root filtering uses the computed root identity (trim, optional `.md`),
  never ownership or directory location, and recomputes `counts`.
- Findings do not make `list` nonzero. `check` exits 1 on any error, 0 for
  warning-only or clean reports, and 2 on usage errors.
- Real I/O failures are reported on stderr and exit 1; they never look like
  an empty successful scan.
- `list` and `check` are read-only: vault bytes, index, and HEAD are
  unchanged.

### Syncing records indexes

`bob albums sync` plans every album index, every record walk line, and every
owner catalog from one coherent snapshot and album inventory. `-d` prints the
unified diffs without taking the maintenance lock, writing files, running Git,
or syncing the vault. A real run takes the shared maintenance lock, pre-syncs
the vault, guards album preimages, record frontmatter, folder membership, and
the owner/ancestor/declaration inputs catalog rows use, writes all healthy
indexes, walk lines, and catalogs, then makes one scoped
`albums: sync (N notes)` commit. It runs a post-sync only when it made that
commit. A current run writes and commits nothing and skips post-sync.

Damaged or invalid albums and damaged owner catalogs remain unchanged. Sync
continues with healthy regions, reports each skipped damaged region, and
exits 1 if an album or catalog is damaged or a runtime operation fails. An
unreadable or incomplete source is never treated as an empty folder. In
`bob nightly`, album sync runs directly after `notebooks` and before
`records`; a failed maintenance step does not prevent that final publish.

JSON uses schema version 1. `albums[]` repeats the list/check entry shape;
`catalogs[]` repeats the owner-catalog report. `counts.damaged` counts unsafe
or invalid indexes, and `counts.walk_lines` counts rewritten walk lines.
`findings[]` flattens album and catalog issues, while `changes[]` includes
path, `kind` (`index`, `catalog`, `walk`), and dry-run diff. `commit`,
`sync`, `warnings[]`, and `io_errors[]` report transaction state; `ok` is
false on damage or runtime failure.

### Tidying album titles

`bob albums tidy [STEM]` moves each root album's title into a bob-owned
frontmatter `title` key so the album card can be its only visible title:

```sh
bob albums tidy
bob albums tidy eat
bob albums tidy -a
```

An album without `title` gains `title: <its current title>` after `type`
(else after `parent`, else first): the first H1 before a bullet or `##`,
else the first alias, else the humanized stem. Every reader already shows
that title, so walk lines, `[!records]` indexes, and `[!albums]` catalogs
do not change. Tidy then removes the first body H1 when it is the first
body content outside fenced code and equals the title exactly, with the
blank lines after it. It keeps and reports a differing H1 (including one
with wikilinks or emphasis), an H1 that is not the first body content, a
later H1, and an H1 inside fenced code. An unusable `title`, duplicate
frontmatter keys, or a missing closing `---` leave the album unchanged and
add a warning. A second run plans nothing. `STEM` is an exact root album
stem; an unknown stem exits 2 before any lock.

Without `-a/--apply`, the run is a dry run: it prints unified diffs and
takes no lock, writes nothing, runs no Git, and does no sync. `-a` takes the
shared maintenance lock, pre-syncs, replans, guards each album by its full
preimage, verifies that each rewritten album keeps its planned title and
plans no further change, makes one scoped `albums: tidy (N notes)` commit,
and post-syncs only when it committed. A no-op writes nothing and skips
post-sync. `bob nightly` never runs album tidy.

JSON uses schema version 1 with `command: "tidy"`, `dry_run`,
`counts {scanned, changed, title_added, h1_removed, h1_kept}`, `changes[]`
(`path`, `title`, `title_added`, `h1_removed`, `h1_kept_reason`, plus
`diff` on dry runs), `kept_h1[]` (`path`, `reason`), `commit`, `sync`,
`warnings[]`, `io_errors[]`, and `error` on failure.

An older bob reads album titles only from the H1. If it runs
`bob albums sync` after tidy removes an H1, it rewrites walk lines and
catalog rows to the humanized stem. Apply tidy only once every host that
runs album sync has this bob; albums created by this bob already have no
H1.

### Tidying empty Notes sections and redundant H1s

An empty `## Notes` section holds only blank lines between its heading and the
next level-1 or level-2 heading (or EOF). Any other content keeps the section:
bullets, prose, a `###` subheading, `%%` or HTML comments, a fence, or a block
id. A redundant H1 is the first non-blank body line outside fenced code when
it is a level-1 heading whose text equals the frontmatter `title` exactly. A
differing H1, an H1 that is not the first body content, and any later H1 are
kept. The H1 probe skips the album's walk line and nav-like `↑` lines, so an
H1 directly below the walk line is still redundant and is stripped, leaving
`---␤␤<walk>␤␤## Notes…`. Tidy never inserts, rewrites, or removes walk
lines, and the blank-body collapse never fires while a walk line exists.
`bob records tidy [TYPE]` removes every empty `## Notes` section and
every redundant H1 from record bodies in one pass:

```sh
bob records tidy -d
bob records tidy eat -d
bob records tidy -f json
```

`TYPE` accepts a noun or album stem, as `bob records check` does; an unknown
`TYPE` exits 2. Without `TYPE`, every album is processed. Only direct Markdown
children of album folders count, and only when their frontmatter type is
`record` or the album's legacy noun type. Notes outside album folders, album
declarations, and non-record notes in the folder are never touched. Unreadable
notes are skipped and reported in `io_errors[]`.

`-d/--dry-run` prints unified diffs without taking the maintenance lock,
writing files, running Git, or syncing the vault. A real run takes the shared
maintenance lock, pre-syncs, guards each rewritten note by its full preimage,
verifies that no rewritten note keeps an empty Notes section or a redundant
H1, makes one scoped `records: tidy (N notes)` commit when any note changed,
and post-syncs only when it committed. A no-op writes and commits nothing and
skips post-sync. A preimage race fails the transaction with nothing written
and exits 1.

JSON uses schema version 1 with `command: "tidy"`,
`counts {scanned, changed, h1_removed, h1_kept}`, `changes[]` (`path`,
`notes_removed`, `h1_removed`, `h1_kept_reason`, plus `diff` on dry runs),
`kept_h1[]` (`path`, `reason` for every H1 left in place),
`commit`, `sync`, `warnings[]`, and `io_errors[]`. `ok` is false on runtime
or guard failure. The human output names both kinds of removal per note and
lists kept H1s with their reasons. `bob nightly` runs the lock-free tidy core
after `albums` and before the final `vault-sync`; a failure never stops that
final publish.

Human `list` groups albums by root with root hues, record and group counts,
freshness (`today` / `Nd` / `Nw`, using `BOB_NOW` when set), and `!N` / `~N`
issue markers. Invalid or unowned albums get a visible group. Human `check`
groups findings by album path and ends with an album, record, error, and
warning summary.

List and check share one JSON envelope (`schema_version: 1`). Albums are
ordered by path. `parent` is the parent stem. `ok` is false when error
findings exist or runtime I/O failed. Each album carries `noun`, `label`,
`plural`, the additive display `singular`, `walk_by` and `index_group_by` (each
the effective field key or `null`), and the `walk` object (`state`, `total`,
`counts: {current, missing, stale, damaged}`, `order: [stem…]`, `groups`,
and empty order/groups when held). Group entries contain the stored choice
`value` (null for the trailing needs group), display `label`, `count`, and the
first record as `head: {stem, path, title}`. Flat walks have no groups. The
top level also carries `capabilities: ["record_values_v1", "record_walk_v1",
"record_walk_groups_v1"]`; clients can use the last capability to enable
grouped-walk controls. See
[JSON outputs](reference/json-outputs.md#album-reports).

Each album's ordered `fields[]` includes its label, kind, flags, typed default
when set, optional numeric `min`/`max`, the 1-based source `line`, and `filled`
record count. Choice entries add `count`, counting each record once even when
its raw value is invalid elsewhere in the schema. `dated_notes`, `legacy`, and
`schema_fingerprint` are `null` when the album schema cannot be parsed;
otherwise `legacy` preserves order and the fingerprint is `sha256:` plus 64
lowercase hexadecimal digits. The fingerprint hashes the parsed schema's
semantic content: noun, label, resolved plural, dated-note flag, ordered legacy
patterns, ordered fields and choices with every attribute, and the effective
`walk_by` key. YAML formatting and source lines do not affect it. An unset
`walk_by` retains the original fingerprint. `bob records show -f json` uses
the same field shape, with `line: null` when an album schema is unavailable.

### Creating an album

`bob albums create PARENT STEM -n NOUN -s SUMMARY` writes a new album note
at the vault root with its starter `<stem>.base` and every affected owner
catalog row, in one guarded write and one scoped `albums: create <stem>.md`
commit. `PARENT` names an area or project of any status (stem or
path-qualified when ambiguous); `STEM` is lower-cased to a vault-root
capture route and names `<stem>.md`, `<stem>.base`, and the future
`<stem>/` record folder. `-n` follows the record noun rule (`album` and
`record` join the reserved built-in note types); `-s` is required and one
line, warning past 140 characters. The title defaults to the humanized stem
and the label to the noun-derived label; `-t`, `-l`, and `-e` take single
lines.

```sh
bob albums create body eat -n restaurant -s 'Places to eat around here.'
```

The new note carries canonical frontmatter (`parent`, `type`, `title`,
`summary`, optional `emoji`, `created`, then `record:` with `noun` and
`label`), no H1, an intro teaching `Name @<stem>/` and linking
`<stem>.base`, and the empty-state records index. `-d` prints the new-file and catalog diffs
without locks, writes, Git, or sync. Validation and refusals exit 2
(unknown or cyclic parents, existing destinations, claimed stems or nouns,
an existing record folder even when empty, or a damaged affected catalog).
Runtime failures exit 1. A damaged catalog refuses the whole creation. `-o`
opens the album in Obsidian only after a committed creation; an opener
failure warns without rolling back. Human success ends with a paste-ready nested
`fields:` example and a `bob albums check` hint; JSON follows the
notebook-style envelope with the created `album` identity, `changed`,
`changes`, `catalogs`, and `findings`. The created album is ready for record
creation; its index and affected owner catalogs are already synchronized.

Destination checks for `<stem>.md` and `<stem>.base` ignore case and include
directories and dangling symlinks.

A broken album refuses only its own route and stays visible; other albums and
notebook catalogs keep working. Each album's failure is isolated per route:
`album_schema_invalid` names the file, 1-based line, and key path, and
discovery failures surface as explicit `record_discovery_incomplete`
diagnostics.
Membership is structural in the immediate album-folder scope: direct children
count even when invalid or when the schema is broken, nested descendants
report `record_outside_album`, and `record_parent_mismatch`,
`record_checkbox`, and `record_outside_album` join the existing field errors.
`bob records list` and `check` report `source: "album"` and album identity;
human success output names the album and owner. A partial vault scan keeps
readable partial results but fails inventory checks; a healthy selected album
is never blocked solely on unrelated failures.

### Updating an album schema

`bob albums update STEM [-i FILE]` applies one ordered JSON envelope to one
root album: it validates ordered schema edits, preserves unrelated bytes,
rebuilds the generated `[!records]` index, replans affected walk lines in the
same transaction, and returns
safe inverse operations for Undo. `STEM` selects one root album note, never a
record noun fallback or an arbitrary path; confinement and symlink safety are
enforced. `-i -` reads stdin, and omitted `-i` has the same meaning. Options
stay alphabetical (`-b`, `-d`, `-f`, `-i`). The Schema Card owns this command;
the command catalog excludes it.

```sh
bob albums update eat -i envelope.json
cat envelope.json | bob albums update eat
bob albums update eat -d -i envelope.json
```

The envelope is one JSON document with `schema_version: 1`, the semantic
`expect` fingerprint (`sha256:…` from `bob albums list`), and the ordered
`ops` array. Unknown versions, keys, operations, duplicate keys, and wrong
types refuse with structured diagnostics. Parsing happens once; labels, JSON,
and YAML are never interpolated into shell commands. An empty `ops` array is
a valid no-op.

```json
{
  "schema_version": 1,
  "expect": "sha256:…",
  "ops": [
    { "op": "choice.add", "field": "status", "label": "Regular", "emoji": "🔁", "position": 2 }
  ]
}
```

To group record walks by a single-value choices field, use
`{ "op": "walk_by.set", "field": "section" }`. Unset the selection with
`{ "op": "walk_by.set", "field": null }`. The field must already be declared
and eligible. Changing the selection or the selected field's choice order
replans only walk lines that differ; the preview lists these file changes and
the returned inverse restores the previous choice and top-level YAML span.

Positions are zero-based. For moves, the selected entry is removed before
indexing the remaining list; 0 prepends and the length appends. Adds default
to append. Out-of-range positions refuse. Stable keys identify targets
throughout a multi-op envelope, so an op can target a key an earlier op
created. Failure at any point refuses the whole envelope. Field keys derive
from labels with `normalize_stem` and enforce the schema identifier rules;
empty, invalid, reserved, or colliding derived keys refuse with a helpful
diagnostic instead of silent suffixing. Choices keep the schema's existing
choice-key rules. Every derived key returns indexed by op position.
`set`/`unset` cannot name the same attribute; absent means unchanged and
`unset` removes its YAML key. False, zero, empty strings, and empty lists stay
distinct from absent and are accepted only when the validator accepts them.
Null is not an implicit unset. Display label and emoji split/combine with the
albums output convention, preserving unchanged combined labels.

| Operation | Inputs and behavior |
| --------- | ------------------- |
| `album.set` | `set` object with label, plural, dated_notes; optional `unset` array restoring the parser's absent/default state. Noun and legacy are not editable here. |
| `walk_by.set` | Required `field`: a declared single-value choices field, or `null` to unset. Changes walk grouping and replans differing walk lines in the same transaction; the inverse restores the prior selection and authored top-level YAML span. |
| `field.add` | label; optional kind (default text), list, suggest, position. Choices-kind additions supply a nonempty ordered `choices` array of label/emoji objects. A new field is optional; a later `field.set` may make it required. |
| `field.set` | field key plus `set` for label, required, suggest, default, min, max, and optional `unset` property-name array. Key, kind, list, and choices are not mutable through this op. |
| `field.move` | field key and position. |
| `field.remove` | field key; allowed only when the complete scan proves filled = 0 and no retained explicit index reference names it. |
| `choice.add` | field key, label, optional emoji and position. The field must already be choices-kind. |
| `choice.set` | field key, stored choice value, and `set` containing label and/or emoji. Omitted components retain their prior value; empty emoji explicitly removes the icon. |
| `choice.move` | field key, stored choice value, and position. |
| `choice.remove` | field key and stored choice value; allowed only with count = 0 and a valid remaining schema. |

Required, bounds, and default changes report newly invalid records without
silently changing them or refusing a valid schema just because existing
records now have findings. Counts include all immediate Markdown members,
including readable invalid records and list values, not only healthy rows;
per-op effects stay distinct from already invalid rows, and multiple edits to
the same field in one ordered envelope count each transition. URL, numeric
bounds, list, and default kinds validate through the shared schema parser. A
scan that cannot prove completeness refuses. Prohibited intent refuses
explicitly with its target and complete-scan usage count plus an
`edit in YAML (y)` hint: field/choice key rename, kind/list-shape change,
merge, noun change, and used removal. Typed errors carry `field`/`choice`,
`key_path` where applicable, and `affected`. A removed field named by
`index_by`, `index_group_by`, or `index_facets` refuses rather than changing
those top-level bytes (the `index_group_by` refusal carries `key_path`
`index_group_by`). The index chooses its group field independently (preferring
`status` choices, otherwise the first choices field) unless `index_group_by`
overrides its headings; `walk_by` explicitly chooses the walk's field and does
not change index order or grouping. There is no `index_group_by` update op:
edit the key in YAML. A selected walk field
cannot be removed until `walk_by` is unset. Reordering its choices replans the
walk; unrelated schema edits do not. A cross-album storage clash names the
conflicting key and album paths.

Dry run previews the entire intended edit, including index and walk diffs, and
performs no record-file writes. It takes no lock, performs no Git or sync
commands, and writes no vault or cache files. A net semantic no-op — an empty
envelope or a batch restoring exactly its initial schema and order — returns
original bytes and no edit before any canonical emission or index repair,
comparing ordered raw schema and presence state rather than emitted bytes;
explicit absent-versus-false/unset stays distinct. Real no-op apply takes no
lock, invokes no sync, and makes no commit, even under a held lock with an
unavailable remote and even for nonempty cancelling batches, and never repairs
a stale or damaged index. Atomic batches may temporarily empty a choices
mapping when later ops make the final schema valid; usage safety still refuses
at the removal, and final schema validation stays mandatory. A changed apply takes the shared vault lock,
pre-syncs, replans fresh with the original `expect` (preflight bytes are never
applied after pre-sync), then installs one album-note edit and makes one
scoped `albums: update <stem>.md (<short op summary>)` commit — a count
summary for multi-op envelopes. One envelope is one gesture and one commit.
The album and each changed walk line are included; owner catalogs and Base
files stay byte-identical. The post-install verifier checks the actual schema,
regenerated index, and any replanned walk; verification or commit failure
rolls back safely. Typed semantic
failure details survive the transaction path.

JSON uses schema version 1 for dry run, apply, no-op, and refusals alike:
`ok`, `command: "update"`, `dry_run`, album `stem`/`path`, `changed`,
before/after fingerprints, ordered per-op results, derived keys, the complete
scan consequence, `group_before`/`group_after`, changed-file diffs,
`inverse_ops`, `commit`, `sync`, `warnings`, and a nullable typed `error`.
Per-op entries carry `index`, `op`, `summary`, `derived_key`, `filled`/`count`,
and `newly_invalid`. Error codes stay stable and carry `op_index`, `field` /
`choice`, `path` / `line` / `key_path`, `affected`, and a source-mode hint
where applicable. Exit 0 for success and no-op (including saved-local sync
warnings), 2 for invalid requests or semantic refusals, 1 for runtime, lock,
or transaction failures. JSON stdout stays one parseable document.

Undo of a removed unused field or choice retains its original stored key even
when it differs from its current label, along with ordering, absent keys,
explicit false, typed defaults and bounds, comments, quoting, BOM, and
LF/CRLF. Optional restoration metadata on inverse add operations carries that
identity: `field.add` accepts `restore_key` and `restore_values` (stored
choice keys in declaration order when they differ from their labels),
`choice.add` accepts `restore_value`. Each only creates an absent key, goes
through collision and schema validation, and cannot rename an existing key.
Required, default, bounds, absent label, and explicit suggest-false return
through ordinary inverse `field.set` operations after the optional field has
been re-added. Normal inverse ops restore semantics and order; narrowly
scoped `restore.spans` entries preserve original quoting, comments, or absent
keys with exact current (`expected`) and prior bytes (or null for an absent
key) under schema key paths below `record`. Restoration of an absent entry is
checked against its server-resolved path, indentation, and expected absence.
Walk-by restorations may also name the original top-level slot with optional
`after`/`before` neighbor keys (a string or null); omitted locators insert at
the frontmatter close, matching earlier envelopes.
The final restored model agrees with the independently validated semantic
inverse, including meaningful order. Paths resolve on the server; no offsets
are trusted. Tampered or stale restoration cannot change noun, kind,
unrelated fields, or invalid schemas. Undo rechecks usage, collisions,
fingerprint, and schema rules. The exact Undo
request replays the returned `inverse_ops` with the returned `after`
fingerprint as its new `expect`:

```json
{ "schema_version": 1, "expect": "<after_fingerprint>", "ops": "<inverse_ops>" }
```

A post-sync failure keeps success and local-commit evidence with
`saved locally; sync failed` guidance; the recovery action is
`bob vault-sync`, never replaying the ops.

## Record walk

Every record note carries one bob-maintained walk line as its first body
line. It links the record to the previous and next records in its album, so
`[n` / `]n` can follow the album ring with no key changes:

An album may declare `walk_by: section` at the frontmatter top level, beside
`index_by` and `index_facets`. The value must name a declared `Choices` field
with `list: false`. Invalid or ineligible values produce a finding and use
flat title order. The generated `[!records]` index keeps its independent group
field and ordering unless `index_group_by` names its heading field. For an
outline album, pair `walk_by: section` with `index_group_by: section` so the
index lists the same sections in the same order as `]n`.

```markdown
---
parent: "[[eat]]"
type: "[[record]]"
title: "Ramen Nagi"
---

[[eat/pho|‹ Pho House]] · [[eat|↑ Eat]] · [[eat/ramen_ya|Ramen Ya ›]]

## Notes
```

A single-record album carries only `[[eat|↑ Eat]]`, with no arms. No count
is ever written into Markdown: `N of M` is painted live by the plugin rail
and the `n` links footer, never stored.

In Obsidian, the [Record Card reference](reference/plugins/record-card.md#record-walk-rail)
shows the rendered rail, and the [navigation hotkeys reference](reference/plugins/bob-navigation-hotkeys.md#bracket-repeat)
documents `[n` / `]n` and the missing-record guard.

- **Order and membership.** Members are the immediate Markdown children of
  the album folder, including readable members whose YAML is invalid. Nested
  files, non-Markdown files, `.base` files, and the album note are excluded.
  Without `walk_by`, order is the album-wide `title_order` the `[!records]`
  index uses (lowercase title, then vault-relative slash path, Unicode scalar
  order). With `walk_by: <field>`, records are grouped by that field's declared
  choice order, then ordered by the same title/path rule within each group.
  Empty choice groups are skipped. Missing, blank, malformed, list-shaped, or
  undeclared values form one trailing `⚠️ Needs <field>` group (`⚠️ Needs a
  status` for `status`). An invalid or ineligible declaration reports a
  finding and falls back to flat title order; it does not damage walk lines.
  Walk grouping is independent of the generated index's grouping and order,
  except that `index_group_by` naming the same field gives the index that
  group and member order.
  Every member appears once. The ring wraps at both ends; `M = 1` has no arms
  and `M = 2` points both arms at the other record.
- **Bytes.** The canonical line for `M >= 2` is
  `[[<album>/<prev>|‹ <Prev>]] · [[<album>|↑ <Album>]] · [[<album>/<next>|<Next> ›]]`.
  Separators are `·` (U+00B7 with one space on each side). Labels drop
  `|[]#^«»`, trim leading `‹`/`<` and trailing `›`/`>` from both ends, then
  escape active Markdown; an empty result falls back to the humanized stem,
  then the bare stem. Nothing is written for `M = 0`.
- **Placement.** The line is inserted as the first body line: closing `---`,
  one blank line, the walk line, then (only when the body has non-blank
  content) one blank line and the original body. After insertion the position
  belongs to Bryan: a recognized line he moved is rewritten where it sits.
- **States.** `current` (exactly one walk line, no nav-like line, content
  equals the canonical line), `stale` (one walk line, content differs,
  rewritten in place), `missing` (no walk line, inserted per placement), and
  `damaged` (reported, never written: `extra_text`, `duplicate_lines`, or
  `no_frontmatter`). A walk line may carry a foreign target immediately
  before the album hub with a `‹` label prefix (for example,
  `[[the_prompt_composer|‹ The Prompt Input Widget]]`), or immediately after
  it with a `›` label suffix. An Obsidian rename or move therefore leaves
  those neighbours `stale`; the next sync, including the plugin's automatic
  rename sync, rewrites their lines in place. Deleting the offending line
  heals a damaged record on the next sync.
- **Owners and radius.** `bob albums sync` is the full repair owner. Capture
  creates and `bob records new` stage only the lines their batch changes (the
  new records plus neighbours whose canonical line differs). A record update
  that changes its effective `walk_by` value stages the field edit and changed
  walk lines in one guarded transaction; unrelated, same-value, and notes-only
  updates do not repair walks. `bob records tidy` never touches walk lines.
  Tidy learns to see past walk lines instead.
- **Churn.** One create rewrites 3 record files, a delete rewrites 2, a
  retitle that keeps its position rewrites 2, a retitle that reorders
  rewrites at most 5, a single membership move between `walk_by` groups
  rewrites at most 5 walk lines, a status or unrelated field edit rewrites 0,
  and an album retitle or whole-album regroup rewrites as many lines as differ
  (up to M).
- **Backfill.** The first real sync after this release backfills every
  record in one scoped commit, from nightly or the plugin watcher. After
  that the steady state is quiet: a second sync is a byte-identical no-op.

`bob albums list|check` report each album's `walk_by`, `walk` object (`state`,
`total`, `counts`, `order`, `groups`) and the `record_walk_groups_v1`
capability, with warnings
`album_walk_stale` (missing or stale lines, hint `bob albums sync`) and
`album_walk_damaged` (damaged lines, count and first path). `bob albums sync`
tags `changes[]` with `kind: "index" | "catalog" | "walk"` and counts
`walk_lines`. Human sync output summarizes walk repairs as one line per
album (`eat — 104 walk lines`).

## Schema

Album schemas live under `record:` in each album note, as shown above. Album
stems are the record folders and capture routes; `record.noun` is the kind.
Field names match `[a-z][a-z0-9_]*`. Reserved frontmatter keys `parent`,
`type`, `title`, and `created` cannot be declared as fields. Labels default to
the field name in title case, with underscores changed to spaces. Choice and
field order follow the YAML mappings.

Fields accept these options:

| Option | Meaning |
| --- | --- |
| `required` | The field must have a value. A valid default satisfies this requirement. |
| `values` | Omit for text; use a nonempty mapping for exact choices; or set to `date`, `link`, `url`, or `number`. |
| `list` | Accept repeated and comma-separated values in order, and write a YAML block list. Defaults for list fields are sequences. |
| `default` | A typed value applied only when the field is absent. An explicit empty field does not receive a default. `status` cannot have a default. |
| `suggest` | Allow existing values in the same folder to be offered as suggestions. |
| `label` | Display label; defaults from the field name. |
| `min`, `max` | Inclusive numeric bounds for `number` fields. |

Property names shared by record types must use the same storage shape: text,
choices, links, and URLs use string properties; dates and numbers have their own
storage kinds. Scalar and list use must also agree. Choice sets may differ
between types.

## Field kinds

| Kind | Accepted values | Written representation |
| --- | --- | --- |
| Text (`values` omitted) | Literal text | Safely quoted YAML string |
| Choices (`values` mapping) | An exact choice key | Bare token when YAML-safe; otherwise quoted to preserve its string identity |
| Date | Real `YYYY-MM-DD`, `today`, or `yesterday` | ISO date |
| Link | A complete `[[wikilink]]` or a bare note name | Quoted wikilink |
| URL | An HTTP or HTTPS URL with a host | URL scalar |
| Number | Finite integer or decimal within configured bounds | Numeric scalar |

Text is literal, including numeric-looking strings, `false`, `%`, `#`, `@`, and
block-anchor wikilinks. Scalar fields accept one value. List fields accept
comma-separated and repeated input, preserving order; commas inside a complete
wikilink remain part of that link. An explicitly empty optional field means
“none” and writes no property. An empty required field is incomplete and does
not get its default reinserted.

## The note Bob writes

Bob writes `parent: "[[<album>]]"`, then `type: "[[record]]"`, `title`, schema
fields in order, and `created`. Empty optional values and legacy keys are
omitted. Strings use typed YAML quoting, including
links and values that YAML might otherwise interpret as booleans or null.
For the [album schema above](#albums):

```markdown
---
parent: "[[eat]]"
type: "[[record]]"
title: "Ramen Nagi"
status: liked
category: "Ramen"
location: "Clark"
created: 2026-11-08T12:41:07-0500
---

[[eat/pho_house|‹ Pho House]] · [[eat|↑ Eat]] · [[eat/ramen_ya|Ramen Ya ›]]

## Notes

- Spicy tuna was great; Kelly loved the salmon.
```

Record notes carry no H1: the frontmatter `title` is the title's one home
(the record sheet renders it). Each album-backed record also has one Bob-owned
walk line as its first body line. The `‹` and `›` links follow the album's
title order; `↑` opens the album. A one-record album has only the `↑` link.
The position and album size render live and are never stored. `created` uses Bob's configured timezone and
`BOB_NOW` when set. `## Notes` exists only when the new note holds ordinary
bullets. With `dated_notes: true`, each first-level note bullet gets today's
date; nested bullets stay nested. A whole-bold first-level container with
nested bullets becomes a `##` group with the authored title preserved. Plain uppercase group bullets remain ordinary content and do not create
groups. A note with no ordinary bullets and
no sections contains only its walk line:

```markdown
---
parent: "[[eat]]"
type: "[[record]]"
title: "Ramen Nagi"
status: liked
category: "Ramen"
location: "Clark"
created: 2026-11-08T12:41:07-0500
---

[[eat|↑ Eat]]
```

## Updating notes

An update validates only the fields it changes. An explicitly empty optional
field removes its key; an empty required field is refused. Repeated values for
a list replace that field's current list in request order. Missing fields are
inserted in schema order, before `created` when there is no earlier schema
field to follow. Notes append under an existing `## Notes` section; otherwise
Bob creates the section at the top of the body, before the first other
`##` section. An empty or whitespace-only body gets a canonical blank line
followed by `## Notes`. Named sections reuse a matching heading instead of creating a
duplicate. When a walk line is present, notes land below it: a walk-only body
becomes walk, blank, `## Notes`, bullets, and updates never stage walk edits.

Bob edits the original YAML spans and preserves every untouched key, comment,
body byte, and line ending. Existing unrelated schema problems are returned as
warnings. Ambiguous or unsupported YAML, including a flow-style root or a
non-mapping root, is refused with the original note unchanged.

## Filename rule

The filename stem comes from the title: normalize Unicode with NFKD, remove
combining marks, turn `&` into the word `and`, drop straight and curly
apostrophes, replace other non-alphanumeric runs with `_`, trim separators,
and lowercase. Unicode letters and digits remain. A title with no
alphanumeric characters needs an explicit stem. Explicit stems use the block
ID alphabet: ASCII letters, digits, and hyphens.

Filename collisions are case-insensitive. Bob refuses duplicates instead of
choosing a suffix. A creation plan includes the vault-relative path and rendered
bytes only when required fields are complete and all supplied values validate;
planning never creates a folder or writes a note.

## Failure isolation

Leftover `records:` keys in host configuration are ignored. Album schema
parsing is isolated by route: a malformed album refuses only its own route,
while neighbouring albums and unrelated config settings remain usable.
Runtime folder read failures stay in the I/O channel.

## Capture

`bob capture` creates a record when an album folder appears as `@dir/` or
`@dir/^stem` at either edge of the parent line:

```text
Ramen Nagi @eat/
- status:: liked
- category:: Ramen
- location:: Clark
- visited:: today
- Spicy tuna was great.
```

That writes `eat/ramen_nagi.md` with the same Markdown `bob records new`
would plan. First-level `key:: value` bullets set fields; other first-level
bullets become notes. `@eat/^ramen-nagi` sets an explicit stem.
`@eat/ramen_nagi` without a caret updates the existing record: parent text
is the first note, only supplied fields change, and unrelated bytes stay
put. `capture-parse` and `bob capture --dry-run` show the same planned
Markdown that commit writes. For a healthy album, `Blaze @eat` refuses with
`album_holds_no_tasks` and teaches `@eat/`.

Capture plans created records' walk lines from the final batch overlay. It
stages each created record and only the pre-existing neighbours whose lines
change in the same guarded transaction; capture record updates do not stage
walk edits.
JSON `walk_files[]` lists each changed pre-existing record with its exact
`before` and `after` Markdown. A damaged neighbour is left untouched and
reported as `record_walk_damaged`.

`--dry-run` prints the exact bytes commit writes, including the staged album
index edit (`album_files` in JSON) and neighbour edits (`walk_files`). A later-item failure,
including an incomplete record after a task, writes nothing. Duplicate paths
refuse with `record_exists` and never add a suffix. See
[Capture](capture.md#record-notes) and
[JSON outputs](reference/json-outputs.md#record-notes).

## `bob records`

The `records` command group manages album-backed record kinds. `TYPE` accepts
the noun or folder stem, such as `restaurant` or `eat`. It is separate from
`bob types`, which manages Task Types. Bare
`bob records` runs the read-only `list` command.

| Command | Purpose |
| --- | --- |
| `bob records check [TYPE]` | Lint record folders, album membership, typed records filed outside their folder, and walk lines (`record_walk_damaged` per damaged record, `record_walk_stale` per album; warnings only, exit 1 on errors). |
| `bob records init TYPE` | Retired hidden command; always refuses and points to `bob albums create`. |
| `bob records list` | Show kinds, albums, folders, note counts, and required fields. |
| `bob records new TYPE TITLE` | Create one record through the same planner used by capture. |
| `bob records show TYPE [STEM]` | Show the schema, choice labels, defaults, and common suggested values; with STEM, read one record. |
| `bob records tidy [TYPE]` | Remove empty `## Notes` sections and redundant H1s from record bodies in one guarded commit. |
| `bob records update TYPE STEM` | Edit one existing record in place through the shared update planner, or apply a guarded JSON envelope with `-i FILE`. |

Every command accepts `-f json`. The human output is intended for a terminal;
JSON output is stable for scripts. `-b/--bob-dir` selects a vault root, defaulting
to `BOB_DIR` or `~/bob`.

Create a note with repeated `-s/--set KEY=VALUE` fields and optional repeated
`-n/--note TEXT` bullets:

```sh
bob records new restaurant 'Ramen Nagi' \
  -s status=to_try -s category=Ramen -s location=Clark \
  -n 'Recommended by Kelly'
```

`-S/--stem STEM` supplies a filename stem for titles that normalize to an empty
stem or need a curated name. Duplicate paths are refused. Missing required fields
and invalid values exit 2; bob prints a ready-to-copy draft for missing fields.
`-d/--dry-run` prints the exact planned Markdown without writing, plus the staged
album index edit and each changed neighbour's new walk line. JSON includes the
new note's final `markdown` and `walk_files[]` (`path`, `before`, `after`) for
pre-existing neighbours. `-o/--open` opens a successfully created note in Obsidian.

The planner applies defaults only to fields that were not supplied, validates all
provided values, and never writes incomplete notes. A completed write goes through
the same staged writer as capture, including its no-replace install guard, plus
the album index edit with its own preimage guard. It plans the final title-order
ring once and installs the new note plus changed neighbours in that transaction;
damaged neighbours remain untouched with a warning.

Update one record with repeated `-s/--set KEY=VALUE` fields and optional repeated
`-n/--note TEXT` bullets. Only supplied fields change; defaults never apply to an
update. An explicitly empty optional field removes its key, while an empty required
field is refused with exit 2. Notes append under the existing `## Notes` section;
otherwise Bob creates the section at the top of the body, before the first
other `##` section. Notes are dated when the record type opts in:

```sh
bob records update restaurant ninja_sushi \
  -s status=liked -n 'Went back, still great'
```

Clear one optional field without touching anything else, or revisit with only a
note:

```sh
bob records update restaurant ninja_sushi -s url=
bob records update restaurant ninja_sushi -n 'Went back for lunch'
```

`-d/--dry-run` prints the planned Markdown without writing or opening Obsidian, and
`-o/--open` opens the note in Obsidian only after a successful committed update.
Usage, missing targets, and supplied-value refusals exit 2. A no-op update succeeds
without rewriting the note. Human and JSON results report changed fields with
before and effective values, notes, and warnings; warnings name `bob records check`
and never block the update.

`new` and `update` rebuild the album's records index in the same
guarded write. The index renders from each record's final fields, so a create
followed by an update in one capture batch counts every postimage exactly once,
and one album file change is staged per affected album. A current index stages
nothing but still guards the album preimage; a no-op update still repairs a
stale index. An album edit that landed after planning refuses the whole write
with a stale-note error and installs nothing. Dry-run shows the planned album
bytes alongside the record Markdown and writes nothing; JSON adds `album_index`
(`path`, `state`, `before`, `after`) only when the index changes. The owner and
unrelated albums stay byte-identical.

### Updating with an envelope

`bob records update TYPE STEM -i FILE` applies one guarded JSON envelope of
field ops instead of `--set`/`--note` (the flags conflict). `-i -` reads
stdin. The envelope carries the semantic `expect` fingerprint from
`bob records show TYPE STEM -f json` plus the ordered `ops` array. Each
field appears at most once per envelope; an empty `ops` array is a valid
no-op.

```json
{
  "schema_version": 1,
  "expect": "sha256:…",
  "ops": [
    { "op": "field.set", "field": "status", "value": "ready" },
    { "op": "field.set", "field": "match", "value": ["ANTHROPIC* CLAUDE", "CLAUDE.AI SUBSCRIP"] },
    { "op": "field.unset", "field": "url" }
  ]
}
```

| Operation | Inputs and behavior |
| --------- | ------------------- |
| `field.set` | Field key plus the typed JSON `value`: strings for text, choice keys, dates, links, and URLs; a number or numeric string for numbers; a string array for list fields. List members are literal and never comma-split. An empty string refuses and points to `field.unset`. |
| `field.unset` | Field key; removes its YAML key. Required fields refuse with `record_required_empty`. |

Values feed the same planner, span writer, and in-batch index rebuild as
`--set`; no second validator exists and defaults never apply. The `expect`
check runs before planning: a mismatch refuses `stale_record` with the
current `record` object, so the Record Card repaints in one call. Dry run,
apply, no-op, and refusal all return one JSON document: `ok`,
`command: "update"`, `dry_run`, `type`, `stem`, `path`, `changed[]`,
`properties[]` (the update shape), `changes[]` (`key`, `label`,
`before_display`, `after_display`), `fingerprint_before` /
`fingerprint_after`, `inverse_ops` (`field.set` with the typed prior value,
or `field.unset`, in reverse order), `group_before` / `group_after`
(`{field, value, label}` or null), one human `consequence` sentence,
`album_index` when it changes, `warnings[]`, and a nullable typed `error`
(`code`, `message`, `field`, `hint`).

Error codes are `stale_record`, `record_invalid_value`,
`record_required_empty`, `record_unknown_field`, `envelope_*` (malformed,
duplicate key or field, unknown key/version/op, wrong type, empty-string
value, unreadable input), and the target-resolution codes. Exit 0 for
success or no-op, 2 for refusals, 1 for runtime or guard failures. Human
output reuses the classic printers; refusals print the message plus hint to
stderr.

Undo replays the returned `inverse_ops` with the returned
`fingerprint_after` as its new `expect`:

```json
{ "schema_version": 1, "expect": "<fingerprint_after>", "ops": "<inverse_ops>" }
```

### Reading one record

`bob records show TYPE` prints that kind's schema, choice labels, defaults,
and the top five suggested values. With an optional `STEM` it resolves one
existing note the same way `bob records update` does and prints a read-only
card of that record.

```sh
bob records show restaurant ninja_sushi
bob records show zettel bead_lifecycle -f json
```

JSON keeps today's schema payload and adds `fields[].values` (the full ranked
suggestions; `top_values` stays capped at five) plus a `record` object:

- `stem`, `path`, `title`, `heading` (the first H1, `null` when the note has none), `created`
- `fingerprint` and `schema_fingerprint`
- `frontmatter_end_line`, `group` (`{field, value, label}` or `null`) from the
  index's own group field
- `properties[]`: schema fields in declaration order with `state`, typed
  `value`, `display`, `raw`, and the 1-based YAML `line` (or `null` when the
  key is absent)
- `identity[]`: `parent`, `type`, `title`, and `created` when present
- `other[]`: remaining keys, each with a `legacy` flag from the album's
  `legacy` patterns
- `findings[]`: this note's `bob records check` diagnostics
- `walk`: `{state, reason, position, total, prev, next, group, prev_group,
  next_group}` from the album ring (`prev`/`next` are `{stem, path, title}` or
  `null`, and `group` is `null` when this album is ungrouped). Group metadata
  is `{field, value, label, index, count, position, size}`; index and position
  are one-based, count is the occupied group count, and size is this group's
  membership. Group stops are `null` with fewer than two occupied groups;
  otherwise each is `{value, label, head, wraps}` for the adjacent group's
  first record. `wraps` is true only when crossing the first/last group edge.
  The entire walk is `null` when the album is held. `state` is `current`, `missing`,
  `stale`, or `damaged`; `reason` names the damage (`extra_text`,
  `duplicate_lines`, `no_frontmatter`) or is `null`. The count is rendered
  live and never stored.

`state` is `set`, `missing` (empty required), `none` (empty optional), or
`invalid` (the stored value fails validation; `raw` keeps it). The record
fingerprint is `sha256:` plus 64 lowercase hex digits over the parsed
frontmatter mapping (canonical JSON with sorted keys and typed scalars kept
distinct) together with the album schema fingerprint. Body bytes, YAML
formatting, and key order do not affect it.

The command never writes, repairs, locks, or commits. A missing stem, a
parent mismatch, or a record outside its album folder exits 2 with a typed
JSON `error` (`code`, `message`): `record_not_found`,
`record_parent_mismatch`, or `record_outside_album`. It never impersonates
an empty record. Human output is a colored card: title, album crumb,
`N of M · ‹ prev · next ›` (or `1 of 1` for singleton rings, omitted when
held), a muted `walk line {state} — bob albums sync` hint when the line is
not current, label/value rows, missing marks, findings, and the other-key
count.

## Adding a record kind

Create an album with [`bob albums create`](#creating-an-album), then add
optional fields under `record.fields` in the album note. Capture records with
`<title> @<stem>/` or use `bob records new`. The hidden `bob records init`
command always exits 2 and explains that `bob albums create` is the supported
setup path.

## Related

- [Capture](capture.md#record-notes)
- [Capture syntax](reference/capture-syntax.md#record-notes)
- [JSON outputs](reference/json-outputs.md#album-reports)
- [JSON outputs](reference/json-outputs.md#record-reports)
- [JSON outputs](reference/json-outputs.md#record-notes)
- [Task types](task-types.md)
- [`bob albums` CLI](reference/cli/albums.md)
- [`bob records` CLI](reference/cli/records.md)
