---
title: Albums and record notes
description: Vault album notes, record schemas, capture creation, and how Bob plans record-note files.
---

# Albums and record notes

A record note represents one durable thing, such as a restaurant, podcast, show,
or movie. An **album** is a vault-root note that owns one record type: its stem
is the folder, `record.noun` names the Obsidian `type` link, and `record.fields`
is the schema. Config `records:` remains a fallback for capture and
`bob records` until a later phase migrates those consumers.

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
  project. The chain must reach a Root Parent and must not cycle.
- **Summary:** one plain line, at most 140 characters.
- **Grouping:** `status` choices when present, otherwise the first choices
  field, otherwise none. Empty groups are omitted. Missing, unknown, or
  list-mismatched values count as `needs_status`.
- **`index_by`:** a declared field name. Groups larger than 12 with at least
  two distinct values peek by that field's display (wikilink phrase, then
  title), otherwise by title then path. Peeks are the first three titles.
- **`index_facets`:** an explicit list of at most three field names, or the
  default of other choice fields then `suggest` fields, excluding the group
  field and `index_by`, capped at three.
- **`last_record`:** the latest valid `created` date (`YYYY-MM-DD`) among
  folder members, or null.

`bob albums` lives in the Vault section of `bob --help`. Bare `bob albums`
defaults to the read-only `list`. `bob albums create` ships in a later phase.

```text
bob albums [list] [-b DIR] [-f human|json] [-r ROOT]
bob albums check [-b DIR] [-f human|json]
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

Human `list` groups albums by root with root hues, record and group counts,
freshness (`today` / `Nd` / `Nw`, using `BOB_NOW` when set), and `!N` / `~N`
issue markers. Invalid or unowned albums get a visible group. Human `check`
groups findings by album path and ends with an album, record, error, and
warning summary.

List and check share one JSON envelope (`schema_version: 1`). Albums are
ordered by path. `parent` is the parent stem. `ok` is false when error
findings exist. See [JSON outputs](reference/json-outputs.md#album-reports).

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

`--dry-run` prints the exact bytes commit writes. A later-item failure,
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
`-d/--dry-run` prints the exact planned Markdown without writing, and `-o/--open`
opens a successfully created note in Obsidian.

The planner applies defaults only to fields that were not supplied, validates all
provided values, and never writes incomplete notes. A completed write goes through
the same staged writer as capture, including its no-replace install guard.

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
