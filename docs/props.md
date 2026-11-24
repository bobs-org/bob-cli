---
title: Note properties
description: Ordered frontmatter inventory, ownership roles, and the guarded bob props show and update contract.
---

# Note properties

Every Markdown note's YAML frontmatter is an ordered property list. `bob props`
is the terminal surface for reading that list and for editing it through a
guarded, reversible protocol that preserves unrelated bytes and enforces one
writer per key. The [`bob props` CLI reference](reference/cli/props.md) indexes
the commands. This page specifies the behavior contract: note resolution, the
show report, the semantic fingerprint, the ownership roles, the property and setting update
operations with their byte rules, undo, exit codes, and the capability probe.

## Commands

- `bob props show NOTE [-b DIR] [-f text|json]`
- `bob props update NOTE -i FILE|- [-b DIR] [-d] [-f text|json]`

`props` lives in the Vault section, between `periods` and `query`. Its members
are `show`, then `update`. Bare `bob props` prints the group help; there is no
default. Every public option has a short alias (`-b`, `-d`, `-f`, `-h`, `-i`)
and help options are sorted alphabetically.

Examples:

```sh
bob props show notes/groceries.md
bob props show eat/ninja -f json
bob props show 2026/20261122 -f json
bob props update notes/groceries.md -i update.json -f json
bob props update eat/ninja -i - -d -f json < preview.json
bob props update 2026/20261122 -i undo.json
```

## Note resolution

NOTE is a vault-relative Markdown path with an optional `.md` suffix
(`notes/groceries`, `notes/groceries.md`), or an absolute Markdown path
confined to the vault. Resolution rules:

- Resolve to one existing file and retain a normalized vault-relative report
  path (`notes/groceries.md`, with `/` separators).
- Reject traversal (`..` escaping the vault), paths outside the vault,
  non-Markdown targets, and symlink escapes with a typed refusal (exit 2).
- Never interpret a stem as a global search and never create a missing note.
- Confinement checks repeat on apply, after the write lock is held.

## Show report

`show` and dry-run previews are read-only: no locks, writes, sync, Git, or
cache creation. A valid `show` of damaged YAML still returns its recovery
report with exit 0; `update` refuses that document.

`show -f json` returns:

```json
{
  "schema_version": 1,
  "ok": true,
  "path": "notes/groceries.md",
  "note_kind": "note",
  "capabilities": ["props_v1"],
  "fingerprint": "sha256:…",
  "frontmatter": {"state": "ok", "start_line": 1, "end_line": 5},
  "properties": [
    {
      "key": "count",
      "line": 3,
      "end_line": 3,
      "value": 3,
      "kind": "number",
      "style": {"flow": false},
      "role": "loose",
      "owner": "props",
      "editor": null,
      "reason": null,
      "duplicate": false,
      "opaque": false
    }
  ]
}
```

Properties retain source order, including integer-like keys. Lines are 1-based.
Unknown positions, and a fingerprint unavailable because the YAML is
invalid/duplicate, are JSON null — never a fabricated successful mapping. An
absent block has no properties and the canonical empty-mapping fingerprint.
Invalid/unclosed input carries actionable line diagnostics. Duplicate root keys
remain visible as separate source entries where their values can be read
safely. Values that cannot be represented safely are opaque, with a diagnostic
rather than invented values.

Text `show` is a colored aligned key/value/kind/role table with the path, note
kind, fingerprint, frontmatter state, and readable diagnostics.

## Kinds, styles, and null conventions

Kinds are `text`, `number`, `checkbox`, `date`, `datetime`, `list`, `tags`,
`aliases`, `map`, `null`, and `opaque`. `tags` and `aliases` are key-aware
list kinds. `style` reports quoting (`quoted`), flow style (`flow`), and the
datetime separator/offset spelling (`datetime_separator` is `T`, a space, or
null; `datetime_offset` is `Z`, `+0000`, `-0500`, `-05:00`, or null) using the
documented fields.

Null conventions: keep null, empty string, empty list, `false`, zero, and
absence distinct. Safe semantic no-op sets retain original bytes.

Quoting and flow rules:

- Quote wikilinks; leading `#@%&*!?|>-:` or a backtick; `: ` and ` #` inside
  text; coercible boolean/null/number/date text; and edge whitespace. Quote
  keys when needed; handle Unicode and integer-like names without collapsing
  duplicates.
- Multiline text uses literal block style (`|-` for no terminal newline);
  preserve semantic trailing-newline counts with the appropriate chomping
  indicator rather than silently changing the value.
- New lists use block style; retain a short existing one-line flow list when it
  remains suitable.
- Datetimes preserve the existing space/`T` separator and offset spelling: `Z`,
  `+0000`, `-0500`, `-05:00`, and no offset. Date-only values stay date-only.
  JSON date/datetime inputs are validated strings; relative-date UI
  interpretation is outside this contract.

## Semantic fingerprint

The fingerprint is `sha256:` plus the SHA-256 digest of canonical JSON for the
parsed mapping. It excludes body bytes, formatting, key order, and album
schema. Never replace it with a byte hash. Canonicalization normalizes valid
property names consistently (integer-like names stay distinct) and uses a
deterministic typed canonical representation for YAML shapes the scalar helper
cannot express; candidate verification compares parsed YAML as well as public
JSON so JSON conversion cannot hide loss. Fingerprint invariance holds across
body, spacing, comment, and order changes; semantic changes alter the digest.

## Ownership roles

`roles` is the authority for both `show` and every update operation. Kinds
resolve from canonical subjects, canonical records and albums through the
existing rules, bank from the bank hub rule, and area/project/notebook/
reference from the existing predicates; otherwise the kind is `note`. The wire
kind names are `day`, `week`, `quarter`, `year`, `all_time`, `record`,
`album`, `bank`, `area`, `project`, `notebook`, `reference`, and `note`.

| Role     | Keys/context                                                                                                            | Owner or action           |
| -------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| identity | `type`; record/album `parent`; album `title`                                                                            | locked with reason        |
| derived  | `*_digest`, `*_drafted`, `review_*`, `zorg_*`, `generated_from_zorg*`; day/period `parent`; reference `source_markdown`; `done_tasks`; project `status`/`scheduled` | writer name and reason    |
| card     | record schema fields                                                                                                    | `record`, editor `==`     |
| card     | day/period `intentions`, `summary`, `reflection`                                                                        | `intentions`, editor `==` |
| card     | album `record`                                                                                                          | `album`, editor `==`      |
| card     | bank hub `bank`                                                                                                         | `bank`, editor `==`       |
| loose    | area/project settings, retired `task_count`/`open_task_count`, and remaining keys including `created` and `template`                                                                      | editable here             |

Audit sources for the derived list: intention writers under
`intentions/` and `periods/`, record and album writers under `records/` and
`albums/`, bank writers under `bank/`, reference importers, note-creation
defaults, and the frontmatter predicates in `projects`, `notebooks`
snapshots, and the album registry. A creation-time default alone does not
make a field derived; `created` and `template` stay loose. Where note/schema discovery
fails, the note does not silently downgrade a potentially owned field to
loose: `show` carries the diagnostic and `update` refuses dependent edits
until ownership resolves.

Retired `task_count` and `open_task_count` keys stay loose so the property
editor and cleanup script can delete them. Live project counts come from
`bob projects list`.

Protected keys are refused for `set`, `unset`, `rename`, `set_yaml`, and
`restore`. Rename checks both the original and the prospective name; new-key
insertion checks the prospective role. Position references may name protected
neighbors without mutating their values. Refusals are `prop_owned` with the
`==` hint, `prop_derived`, and `prop_identity` with the teaching reason.
Roles are recomputed from fresh note/schema context under the write lock, not
from client-supplied metadata. Retired count keys are deletable loose properties
with `legacy: true`. Project `scheduled` is also marked legacy, but stays
derived. `legacy` is a boolean on every property row; contract keys additionally
carry a `setting` id (`theme` maps to `task_type`, and both clock keys map
to `clock`). Project `status` and `scheduled` name owner `projects`;
`done_tasks` names owner `task archive`.

## Area and project settings

Area and project contract keys stay `loose`, so the Property Editor (`+`)
can edit them through the same guarded writer. They carry domain validation
for generic value edits, YAML fragments, rename destinations, and typed
setting gestures. Validation examines the addressed value, so unrelated
legacy values do not prevent another property from being edited.

`bank.md` is a hybrid: its `note_kind` remains `bank`, its `bank` key stays
card-owned with editor `==`, and its other keys use the area contract.

Settings have this stable order (bank uses the area order):

- Area: Summary, Lives under, Hue, Default type, Clock, Ready cap, Review,
  Capture names.
- Project: Lives under, Hue, Default type, Clock, Stats root, Ready cap,
  Review, Capture names.

| Setting id | Keys | Valid values |
| --- | --- | --- |
| `summary` (area) | `summary` | One line of text; over 140 characters warns. |
| `parent` | `parent` | One scalar wikilink to an existing home. Areas allow the `area` root or another area; projects allow an area or a non-terminal project. Required. |
| `color` | `color` | blue, orange, aqua, yellow, magenta, green, violet, red, grey/gray. Grey is an explicit mute. |
| `task_type` | `task_type`, legacy `theme` | One valid type name or `off` (Stop parent defaults). Unconfigured names warn. |
| `clock` | `offclock`, `offclock_types` | Boolean `offclock`, or a nonempty type name/list for `offclock_types`. Typed clock gestures use the modes below. |
| `root` (project) | `root` | `true` or absence; setting it on a top-level project is disabled. |
| `ready_cap` | `ready_cap` | 1–999 or `off`. Existing YAML `false` still reads as off; new writes use `off`. |
| `task_refresh` | `task_refresh` | 1–365 days. |
| `project_name_aliases` | `project_name_aliases` | A list of nonblank, distinct strings; duplicates are case-insensitive. Cross-note collisions and stem shadows warn. |

Clearing a setting removes its local key(s). Ready cap and Review use Bob's
configuration by residence; they never inherit from a parent. Other inherited
policies resume their existing parent/default resolution. Clearing `parent`,
or renaming it away, refuses with `prop_required`. Parent changes also refuse
self-links, cycles/descendants, missing or ambiguous targets, wrong kinds,
terminal projects, and `done/` or `_conflicts/` targets. Bob resolves links and
reloads the destination and ancestry under the write lock.

`prop.restore` deliberately skips domain validation to recover exact legacy
values during undo. Ownership, structural safety, and byte verification still
apply. Move likewise preserves existing value bytes. Restore is an explicit
raw replay operation, not a value-edit validation route.

### Typed setting operations

These operations share the schema-version-1 envelope with `prop.*`:

- `setting.set {setting, value}` sets a typed setting.
- `setting.clear {setting}` removes every key belonging to that setting;
  clearing an already absent setting is a no-op.

Unknown setting ids and invalid value shapes refuse. A setting unavailable on
the note's kind refuses with `prop_setting_disabled`. `setting.set task_type`
writes the canonical `task_type` and removes legacy `theme` in the same batch;
clear removes both. Hue saves lowercase names and normalizes `gray` to `grey`.
Parent saves one quoted scalar link, with a path qualification when needed.
Stats root sets `true`; clear removes it.

Clock values are strict objects:

| `setting.set clock` value | Exact resulting policy |
| --- | --- |
| `{"mode":"inherit"}` | Remove both clock keys. |
| `{"mode":"on"}` | Write `offclock: false`, remove `offclock_types`. |
| `{"mode":"off"}` | Write `offclock: true`, remove `offclock_types`. |
| `{"mode":"types","types":["MEET","CALL"]}` | Write a canonical nonempty block list in `offclock_types`, remove `offclock`. |

`setting.clear clock` is equivalent to Inherit. Type names canonicalize and
deduplicate through Bob's clock parser. Saving explicitly cleans shadowed
pairs; reading never does. Unknown clock fields, missing types, scalar types
in this object, and empty lists refuse.

For example, one atomic write changes the clock and the cap:

```json
{
  "schema_version": 1,
  "expect": "sha256:…",
  "ops": [
    {"op": "setting.set", "setting": "clock", "value": {"mode": "types", "types": ["MEET"]}},
    {"op": "setting.set", "setting": "ready_cap", "value": "off"}
  ]
}
```

Bob expands settings into its ordinary span edits. Unrelated frontmatter and
body bytes stay intact, and `inverse_ops` contains ordinary `prop.restore`
operations that recover the exact original bytes, including shadowed keys,
legacy values, comments, order, and line endings. Error `op_index` refers to
the original submitted operation, including when one setting expands into
several edits.

Dry-run and apply include `warnings[]`, using the shared diagnostic shape
(`code`, `message`, `hint`, `key`, `op_index`, and optional `line`). Current
warnings include `area_summary_long`, `note_theme_unconfigured`,
`type_config_unavailable`, and `project_name_aliases_conflict`. Text output
prints warnings too. Warnings allow the write and do not mutate other notes.

Refusals are `prop_bad_value` (with allowed values in the hint),
`area_summary_multiline`, `prop_parent_kind`, `prop_parent_cycle`,
`prop_parent_missing` (also ambiguous resolution or an incomplete parent
snapshot), `prop_required`, and `prop_setting_disabled`. Every refused batch
leaves the note unchanged.

## Update envelope and operations

`bob props update` takes a strict JSON envelope on `-i FILE` (`-` reads
stdin):

```json
{
  "schema_version": 1,
  "expect": "sha256:…",
  "ops": [{"op": "prop.set", "key": "count", "value": 4}]
}
```

Parsing is strict: recursive duplicate object keys, unknown fields or ops,
wrong versions or types, a missing `expect`, and invalid positioning
combinations are refusals. Empty `ops` is a valid no-op. Ordered ops apply to
an in-memory candidate and install only after the complete batch succeeds.

- `prop.set {key, value, kind?, before?|after?}`: replace an existing value or
  insert a new entry; placement is for new entries, defaulting to the end.
- `prop.unset {key}`: remove that entry; a missing target refuses.
- `prop.rename {key, to}`: replace only the key token, preserving
  value/comment bytes. An existing destination refuses; a same-name rename is
  a no-op.
- `prop.move {key, before|after}`: move the exact full entry and its attached
  leading comments. A self-position is a no-op; a missing neighbor refuses.
- `prop.set_yaml {key, yaml, before?|after?}`: parse a single value fragment,
  reject document/fence/sibling injection, and splice only this key's value.
- `prop.restore {key, raw, before?|after?}`: replay validated exact entry
  bytes or restore absence. It is subject to all ownership and opacity checks.

Raw restoration is concrete: `raw` is the complete entry string including its
attached leading trivia, or null to restore absence. It must describe exactly
the addressed key — never multiple keys, a frontmatter delimiter, or body
text. Inverse operations run in reverse order and carry neighboring keys when
needed to recover placement.

Whole-note refusals: `prop_invalid_yaml` for invalid/unclosed blocks and
`prop_duplicate_key` for duplicates, with a YAML repair hint. Anchors,
aliases, merge keys, and custom tags make the affected key opaque
(`prop_opaque`), not unrelated safe keys; structural detection never mistakes
quoted text or block-scalar contents for YAML syntax. An unsafe construct that
affects sibling semantics refuses the dependent edit as well. Other checks use
`prop_exists`, `prop_missing`, `prop_bad_key`, and `prop_bad_value`. Every
candidate is reparsed, target semantics and untouched values are verified, and
`prop_verify_failed` rejects without writes when verification fails.
Restoration never bypasses this verification. Whole-file equality backs the
operation-plus-inverse golden assertions, not parsed equality alone.

## Undo, restore, and frontmatter framing

`update` output carries `inverse_ops` for dry-run as well as successful apply;
applying those ops against the new fingerprint replays the exact original
bytes, and undo itself returns inverses usable for redo. `prop.restore` with
`raw: null` restores absence.

Absent-frontmatter undo is explicit. An ordinary unset preserves a
pre-existing empty or comment-only frontmatter block. When an operation
created the block, its generated inverse may carry optional strict `framing`
restoration metadata on `prop.restore`, containing `expected` and `prior`
strings for the empty frontmatter prefix. Framing is allowed only after
restoring absence leaves no properties, when the current prefix exactly
matches `expected` and both fragments contain only delimiters/trivia (`prior`
may be empty). This narrowly scoped additive field distinguishes removal of a
newly created block from retention of an authored empty block. Never restore a
saved whole file over the current body.

```sh
# Preview, then apply, then undo with the returned inverse ops.
bob props update notes/groceries.md -i edit.json -d -f json > preview.json
bob props update notes/groceries.md -i edit.json -f json > applied.json
# The inverse envelope replays applied.json's inverse_ops against its
# fingerprint_after; it restores the exact pre-edit bytes.
```

## Source boundaries and byte preservation

Source boundaries distinguish the key token, value, inline comment, attached
leading comments, trailing block-scalar content, and envelope. Moving a key
never steals a previous block scalar's blank lines. Unchanged entries,
delimiters, BOM, line endings, body, and final-newline state retain their
bytes. Move/rename/restore may change bytes while leaving the semantic
fingerprint unchanged; they are not discarded solely because before and after
hashes agree.

## Apply, output, and exit codes

Show and dry-run are read-only. For a changed apply, Bob acquires the bounded
vault lock (contention is `lock_busy`); re-resolves the target and ownership
dependencies; rereads current bytes; compares `expect` (`prop_stale`);
replans against those fresh bytes; preserves intervening body-only and
formatting edits; and installs atomically with temporary-file cleanup on
errors. No Git commit or vault-sync runs. Success is reported only after
install; a refused batch leaves file bytes unchanged; no-op requests do not
write or change mtime.

Update output includes `ok`, `schema_version`, `path`, `dry_run`, `changed`,
`fingerprint_before`, `fingerprint_after`, `inverse_ops`, `changes` as a
frontmatter-only line diff, and the post-image `properties`. Invalid input
reports the shared typed error, including the failing operation where known.

Exit codes: 0 for success (including recovery `show` and no-op updates);
2 for usage, protocol, semantic, and ownership refusals; 1 for vault I/O,
lock contention, and stale guarded writes that fail under the lock.

## Capability probe

`props_v1` appears in the `show` capability array and in the
vault-independent `__catalog` feature handshake, so plugins probe once
without selecting a note:

```sh
bob __catalog --protocol 1 | jq '.features'
# ["clicolor-force", "progress-env", "props_v1", "run-receipt"]
```

`props show` is a read catalog row; `props update` is excluded from command
mode with reason "JSON-input property edit owned by the Property Editor; run
it from a terminal".
