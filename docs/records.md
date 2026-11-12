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
through the effective vault (`--bob-dir`/`-b` wins over `BOB_DIR`); an album
claiming a noun or directory owns that kind entirely, even when invalid.
Transitional config `records:` serves only kinds no album claims, and every
such use warns `records_config_deprecated`. `bob records init` is unchanged.

This contract covers album reports, schema loading, value validation, filename
planning, and the Markdown Bob writes. Capture creates notes through the same
planner; exact token forms live in [Capture syntax](reference/capture-syntax.md#record-notes)
and the workflow in [Capture](capture.md#record-notes). Record types are
collections (`records:` or an album's `record:` block). They are not Task Types
(`#fix`, `bob types`).

## Albums

An album is a vault-root Markdown file with `type: "[[album]]"` (any form
`frontmatter_type_names` accepts). Identity is the file stem; that stem is the
record folder (`eat.md` owns `eat/`). Albums must live at the vault root.

```markdown
---
parent: "[[body]]"
type: "[[album]]"
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

# Eat
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
  noun in title case; `plural` lowercases that label unless it starts with
  an acronym, then appends `s`.
- **Summary:** one plain line, at most 140 characters.
- **Grouping:** `status` choices when present, otherwise the first choices
  field, otherwise none. Empty groups are omitted. Missing, unknown, or
  list-mismatched values count as `needs_status`.
- **`index_by`:** a declared field name. Groups larger than 12 with at least
  two distinct normalized values render `index_by` subgroup headings (and
  peeks follow that order). Values fold by case and wikilink display text.
  The heading is the most frequent displayed spelling, preferring a
  plain-text spelling on ties. Missing values sort last under `**—**`.
  Smaller groups stay flat. Peeks are the first three rendered titles.
- **`index_facets`:** an explicit list of at most three field names, or the
  default of other choice fields then `suggest` fields, excluding the group
  field, capped at three. Flat rows keep `index_by` as a facet; subgrouped
  rows omit that repeated field unless `index_facets` is explicit. Choices
  show labels, links show display text, lists join with `, `, numbers stay
  plain, and empty values disappear.
- **Records index:** a bob-owned `[!records]` callout whose title is the
  tally (`N <plural>` plus nonzero group counts, then `⚠️ k need a status`).
  Rows are `[[<album>/<stem>|<title>]]`; alias text drops `|[]#^`, then
  alias and facets escape active Markdown.
  Empty albums keep a teaching index (`No restaurants yet` / `Name @eat/`).
  First write inserts `## Records` before the first real H2 (or at EOF);
  later writes patch only the callout. `list` and `check` report
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
  otherwise before the first real `## Tasks`, otherwise at EOF. Later edits
  stay at the existing callout. Empty subtrees delete the callout and remove
  the heading only when its section is then blank. `list` and `check` report
  `catalogs[].state` as `current`, `missing`, `stale`, or `damaged`. Missing
  and stale warn `album_catalog_stale` with `bob albums sync`; damage errors
  `album_catalog_damaged` and leave the authored region intact. Record
  capture and `records new|update` never rewrite owner notes.
- **`last_record`:** the latest valid `created` date (`YYYY-MM-DD`) among
  folder members, or null. A missing folder is an empty album. Immediate
  Markdown members still count when the schema is broken.

`bob albums` lives in the Vault section of `bob --help`. Bare `bob albums`
defaults to the read-only `list`. `bob albums create` ships in a later phase.

```text
bob albums [list] [-b DIR] [-f human|json] [-r ROOT]
bob albums check [-b DIR] [-f human|json]
bob albums sync [-b DIR] [-d] [-f human|json]
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

`bob albums sync` plans every album index and every owner catalog from one
coherent snapshot and album inventory. `-d` prints the unified diffs without
taking the maintenance lock, writing files, running Git, or syncing the vault.
A real run takes the shared maintenance lock, pre-syncs the vault, guards
album preimages, record frontmatter, folder membership, and the
owner/ancestor/declaration inputs catalog rows use, writes all healthy
indexes and catalogs, then makes one scoped `albums: sync (N notes)` commit.
It runs a post-sync only when it made that commit. A current run writes and
commits nothing and skips post-sync.

Damaged or invalid albums and damaged owner catalogs remain unchanged. Sync
continues with healthy regions, reports each skipped damaged region, and
exits 1 if an album or catalog is damaged or a runtime operation fails. An
unreadable or incomplete source is never treated as an empty folder. In
`bob nightly`, album sync runs directly after `notebooks` and before the
final `vault-sync`; a failed maintenance step does not prevent that final
publish.

JSON uses schema version 1. `albums[]` repeats the list/check entry shape;
`catalogs[]` repeats the owner-catalog report. `counts.damaged` counts unsafe
or invalid indexes. `findings[]` flattens album and catalog issues, while
`changes[]` includes path and dry-run diff. `commit`, `sync`, `warnings[]`,
and `io_errors[]` report transaction state; `ok` is false on damage or runtime
failure.

Human `list` groups albums by root with root hues, record and group counts,
freshness (`today` / `Nd` / `Nw`, using `BOB_NOW` when set), and `!N` / `~N`
issue markers. Invalid or unowned albums get a visible group. Human `check`
groups findings by album path and ends with an album, record, error, and
warning summary.

List and check share one JSON envelope (`schema_version: 1`). Albums are
ordered by path. `parent` is the parent stem. `ok` is false when error
findings exist or runtime I/O failed. See
[JSON outputs](reference/json-outputs.md#album-reports).

A broken album refuses only its own route and stays visible; other albums,
unclaimed config kinds, and notebook catalogs keep working. Each album's
failure is isolated per route: `album_schema_invalid` names the file, 1-based
line, and key path, and discovery failures surface as explicit
`record_discovery_incomplete` diagnostics without falling back to config.
Membership is structural in the immediate album-folder scope: direct children
count even when invalid or when the schema is broken, nested descendants
report `record_outside_album`, and `record_parent_mismatch`,
`record_checkbox`, and `record_outside_album` join the existing field errors.
`bob records list` and `check` add `source: "album" | "config"` and album
identity; human success output names the album and owner. A partial vault scan
keeps readable partial results but fails inventory checks; a healthy selected
album is never blocked solely on unrelated failures.

## Schema

The `records:` block is optional. If it is absent, Bob has no record types. An
empty mapping is valid. A present malformed block disables record features and
reports a typed configuration diagnostic; other config consumers continue to
load their own sections.

```yaml
records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: 🍽️
    parent: "[[food]]" # optional; otherwise use [[eat]] when eat.md exists
    dated_notes: true
    fields:
      status:
        required: true
        values: {to_try: 🔖 To try, liked: 💚 Liked, not_liked: 🚫 Not liked}
      category: {required: true, suggest: true}
      location: {required: true, suggest: true}
      companions: {values: link, list: true, default: ["[[kelly]]"]}
      visited: {values: date}
      url: {values: url}
    legacy: [lid, source_*]
```

Each type has a lowercase id matching `[a-z][a-z0-9_]*`, a required `dir`, a
required display `label`, and an ordered `fields` mapping. `emoji`, `parent`,
`dated_notes`, and `legacy` are optional. The reserved ids are `project`,
`area`, `ref`, `day`, `done`, `mail`, `article`, `type`, and `notebook`.

Directories are unique, safe, relative single path segments. Dot-folders,
`.bob`, `done`, and `ref` are reserved. The directory and collection note do
not need to exist when the configuration is loaded. An explicit `parent` is
written as supplied; otherwise Bob uses `"[[<dir>]]"` only when `<dir>.md`
exists.

Field names match `[a-z][a-z0-9_]*`. The reserved frontmatter keys `parent`,
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

Bob writes the optional parent, then `type`, `title`, schema fields in order,
and `created`. Empty optional values and legacy keys are omitted. Strings use
typed YAML quoting, including links and values that YAML might otherwise
interpret as booleans or null.

```markdown
---
parent: "[[eat]]"
type: "[[restaurant]]"
title: "Ramen Nagi"
status: liked
category: "Ramen"
location: "Clark"
companions:
  - "[[kelly]]"
visited: 2026-11-08
created: 2026-11-08T12:41:07-0500
---

# Ramen Nagi

## Notes

- 2026-11-08: Spicy tuna was great; Kelly loved the salmon.
```

`created` uses Bob's configured timezone and `BOB_NOW` when set. `## Notes` is
always present, including when empty. With `dated_notes: true`, each first-level
note bullet gets today's date; nested bullets stay nested. An all-caps first-
level bullet with nested bullets becomes a title-cased section.

## Updating notes

An update validates only the fields it changes. An explicitly empty optional
field removes its key; an empty required field is refused. Repeated values for
a list replace that field's current list in request order. Missing fields are
inserted in schema order, before `created` when there is no earlier schema
field to follow. Notes append under the existing `## Notes` section, and named
sections reuse a matching heading instead of creating a duplicate.

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

Records configuration is parsed independently of shared configuration. A bad
`records:` block produces a typed diagnostic with the config path and, when
available, the record type and field. It disables record features only; task
capture, planning, hooks, and unrelated config sections keep their existing
behavior. Loading the schema does not scan the vault or require type and
collection notes to exist.

A broken album is the same isolation at vault scope: its claim still
suppresses a matching config kind, neighbouring albums stay usable, and a
schema typo never presents as a clean empty report or a global notebook
catalog failure. Runtime folder read failures stay in the I/O channel.

## Capture

`bob capture` creates a record when a configured folder appears as `@dir/` or
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
Markdown that commit writes. `Blaze @eat` stays a task and warns
`record_collection_route`, teaching `@eat/`.

`--dry-run` prints the exact bytes commit writes, including the staged album
index edit (`album_files` in JSON). A later-item failure,
including an incomplete record after a task, writes nothing. Duplicate paths
refuse with `record_exists` and never add a suffix. See
[Capture](capture.md#record-notes) and
[JSON outputs](reference/json-outputs.md#record-notes).

## `bob records`

The `records` command group manages notes whose frontmatter `type` matches a
configured record type. It is separate from `bob types`, which manages Task
Types. Bare `bob records` runs the read-only `list` command.

| Command | Purpose |
| --- | --- |
| `bob records check [TYPE]` | Lint configured folders and find typed records filed outside them. Exits 1 when it finds errors. |
| `bob records init TYPE` | Scaffold the type note, collection note, and starter Base. Existing files are kept. |
| `bob records list` | Show each configured type, its folder, note count, and required fields. |
| `bob records new TYPE TITLE` | Create one record through the same planner used by capture. |
| `bob records show TYPE` | Show the schema, choice labels, defaults, and common suggested values. |
| `bob records update TYPE STEM` | Edit one existing record in place through the shared update planner. |

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
album index edit, and `-o/--open` opens a successfully created note in Obsidian.

The planner applies defaults only to fields that were not supplied, validates all
provided values, and never writes incomplete notes. A completed write goes through
the same staged writer as capture, including its no-replace install guard, plus
the album index edit with its own preimage guard.

Update one record with repeated `-s/--set KEY=VALUE` fields and optional repeated
`-n/--note TEXT` bullets. Only supplied fields change; defaults never apply to an
update. An explicitly empty optional field removes its key, while an empty required
field is refused with exit 2. Notes append under the existing `## Notes` section,
dated when the record type opts in:

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

Album-backed `new` and `update` rebuild the album's records index in the same
guarded write. The index renders from each record's final fields, so a create
followed by an update in one capture batch counts every postimage exactly once,
and one album file change is staged per affected album. A current index stages
nothing but still guards the album preimage; a no-op update still repairs a
stale index. An album edit that landed after planning refuses the whole write
with a stale-note error and installs nothing. Dry-run shows the planned album
bytes alongside the record Markdown and writes nothing; JSON adds `album_index`
(`path`, `state`, `before`, `after`) only when the index changes. Config-backed
kinds stage no album write, and the owner and unrelated albums stay
byte-identical.

## Adding a record type

Declare a type under `records:` in `config.yml` before running
`bob records init TYPE` to scaffold it. For the restaurant schema above:

```sh
bob records init restaurant -p food
```

`--parent` is required when the collection note (`<dir>.md`) is missing, so
bob never guesses the parent. If that note already exists, `init` keeps it
and `--parent` is unused. `-d/--dry-run` prints the planned files without
writing.

Init creates these pieces when they are missing:

- the type note `<type>.md`, with `parent: "[[type]]"` and a
  `bob records show` pointer
- the collection note `<dir>.md`, whose parent comes from `-p/--parent`,
  embedding the Base view
- a starter `<dir>.base` that filters `file.inFolder(dir)` and
  `note.type == link(type)`, with a table view grouped by status labels

It never edits an existing file: those pieces are reported as `keep`. It
does not create an empty `<dir>/` folder, because git cannot sync one. The
records planner creates the folder when it writes the first record. Writes
use the same no-replace install as capture.

After the scaffold, create records with capture (`<title> @<dir>/`) or
`bob records new`.

## Related

- [Capture](capture.md#record-notes)
- [Capture syntax](reference/capture-syntax.md#record-notes)
- [JSON outputs](reference/json-outputs.md#album-reports)
- [JSON outputs](reference/json-outputs.md#record-notes)
- [Task types](task-types.md)
- [`bob albums` CLI](reference/cli/albums.md)
- [`bob records` CLI](reference/cli/records.md)
