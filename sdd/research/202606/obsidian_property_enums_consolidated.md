---
create_time: 2026-06-09
status: research
topic: Enforcing and suggesting predefined Obsidian property values
---
# Research: Accepted Values for Obsidian Properties

## Question

Some Obsidian note properties have a small, predefined set of accepted values.
What are the practical options for suggesting those values while editing and
enforcing them enough that Bases, Dataview, and automation stay reliable?

## Executive Summary

Obsidian's native Properties feature does not currently provide a true
`select`, `multi-select`, or `enum` property type. Native property suggestions
are useful, but they are learned from existing vault values and are not a
scoped, authoritative allowed-values list.

The prior research files agreed on the important split: "suggest" and
"enforce" are different problems.

- Suggestion means the normal editing path should present a curated picker.
- Enforcement means invalid values must be detected after any bypass path:
  source edits, sync conflicts, imports, plugin edits, or scripts.

For this vault, the recommended solution is layered:

1. Define allowed values by note class/folder, not by property name alone.
2. Use Metadata Menu FileClasses with `Select` or `Multi` fields for the
   day-to-day editing UI.
3. Add a validation backstop: start with a Dataview/DataviewJS compliance
   dashboard because Dataview is already installed, then consider Propsec,
   Forge, or a small `bob` validator if stronger enforcement is worth the
   maintenance.
4. Optionally add Templater or QuickAdd pickers to note-creation flows.

## Verification Notes

I compared the two prior research files:

- `sdd/research/202606/obsidian_property_value_enforcement.md`
- `sdd/research/202606/obsidian_enforce_property_accepted_values.md`

The strongest shared findings hold up: native Obsidian has no enum property
type, Metadata Menu is the best suggestion/editing layer, and some separate
audit or validation layer is needed for real enforcement.

Changes made in this consolidation:

- Kept the vault-specific property counts and the warning that `status` is
  context-sensitive.
- Removed duplicated option descriptions.
- Corrected obsolete generator language: local memory says Bryan has fully
  switched to Obsidian and does not use zorg anymore.
- Avoided overstating Metadata Menu as a hard lock. It constrains Metadata
  Menu editing paths, but raw YAML can still bypass it.
- Added current caution on Forge: it has a direct enum schema and repair/lint
  story, but as of 2026-06-09 it is very new in the community plugin directory.

## Local Vault Context

Observed on 2026-06-09:

- Vault: `/home/bryan/bob`
- Installed community plugins: Dataview, Tasks, Templater, QuickAdd, Task
  Status Cycler, Note Refactor, Vimrc Support, Relative Line Numbers, and
  custom Bob plugins.
- Not installed: Metadata Menu, Meta Bind, Propsec, Forge, Data Entry, Modal
  Forms.
- `.obsidian/types.json` defines native Obsidian property types for some
  properties, but no allowed value lists.

Frontmatter scan of likely enum-like properties:

| Property | Observed frontmatter values |
| --- | --- |
| `status` | `"legacy"` 282, `liked` 84, `wip` 10, `read` 7, `not_liked` 7, `abandoned` 4, `active` 3 |
| `type` | `"[[ref]]"` 304, `restaurant` 91, `daily` 10, `"[[done]]"` 7, `"[[day]]"` 3, `inbox` 2, `monthly` 2, `project` 2, `yearly` 1 |
| `marker_category` | `"project"` 446, `"topic"` 238, `"person"` 187, `"context"` 92, `"status"` 10 |

The counts show small, stable value sets, but also drift:

- `type` mixes wikilink-style values and plain strings.
- Some frontmatter values are quoted while others are not. That may not always
  matter semantically, but it matters for simple text scans and migrations.
- `status` has different meanings by folder. In `eat/`, it means restaurant
  verdict (`liked`, `not_liked`). In `ref/`, Bases already expect reading
  states such as `unread`, `wip`, `read`, `collect_fleeting_notes`,
  `review_fleeting_notes`, `review_lit_notes`, and `abandoned`.

Do not create one global `status` enum. Define status by note class or folder.

## Option Comparison

| Option | Curated suggestions | Scoped by note type | Detects invalid existing values | Fit |
| --- | --- | --- | --- | --- |
| Native Properties/Bases | Partial; based on existing values | No | No | Baseline only |
| Templater | Yes, at creation time | Yes, per template | No | Already installed |
| QuickAdd | Yes, in capture/macro flows | Yes, per choice/script | No | Already installed |
| Metadata Menu | Yes, strongest editing UX | Yes, via FileClasses | Indirect only | Best suggestion layer |
| Meta Bind | Yes, inline controls | Yes, per control/template | No | Good for special notes |
| Modal Forms | Yes, form fields | Yes, per form | No | Good for capture workflows |
| Data Entry | Yes, JSON Schema forms | Yes, per schema | Mostly form-scoped | Old/WIP; lower fit |
| Dataview audit | No | Yes, by query | Yes | Best installed backstop |
| Propsec | No | Yes, path/tag/property filters | Yes | Lightweight in-Obsidian validator |
| Forge | No direct picker | Yes, schemas | Yes, plus repair workflows | Powerful but new |
| Custom `bob` validator | No | Yes, code/config | Yes | Most controllable automation |

## Findings

### Native Obsidian

Native Properties support Text, List, Number, Checkbox, Date, Date & time, and
Tags. A property type is assigned by property name across the whole vault. That
helps with basic hygiene, but it does not express "this value must be one of
these choices."

The Obsidian forum has multiple feature requests for enum/select properties,
predefined multi-select values, customizable suggestion lists, and Bases value
suggestions scoped to the active Base or folder. Those requests are still the
right mental model for the missing native feature.

Use native properties and Bases as the storage/query surface. Do not rely on
them as the enum enforcement mechanism.

### Templater and QuickAdd

Templater is already installed and supports `tp.system.suggester(...)` and
`tp.system.multi_suggester(...)`. QuickAdd is also installed and its API
supports fixed dropdown inputs and searchable suggesters.

This is enough to make new notes start compliant:

```js
await tp.system.suggester(
  ["Unread", "Working", "Read", "Abandoned"],
  ["unread", "wip", "read", "abandoned"]
)
```

The limitation is scope. Creation-time prompts do not help when editing
existing notes, and they do not catch drift introduced later.

### Metadata Menu

Metadata Menu is the strongest suggestion layer for this specific problem.
It can define managed frontmatter fields globally or through FileClasses.
Relevant field types include:

- `Select`: one value from a list.
- `Multi`: multiple values from a list.
- `Cycle`: cycle through a list.

For `Select`, `Multi`, and `Cycle`, values can come from a settings list, a
note path where each line is an option, or a JavaScript function with Dataview
access. FileClasses can be mapped by frontmatter, tag, folder path, bookmark
group, query, or global fallback, and child FileClasses can override inherited
fields.

Why it fits:

- `restaurant.status` can be `liked | not_liked`.
- `ref.status` can be a different list.
- The same property name can be constrained differently by folder or class.
- The plugin can insert missing fields for notes mapped to a FileClass.

Limits:

- It is not a hard security boundary. Raw source edits can still write invalid
  values.
- It adds another schema/configuration layer.
- It should be piloted on one folder before broad rollout.

### Meta Bind

Meta Bind can render dropdowns and inline controls inside a note and bind them
to frontmatter. It is useful for dashboard-style notes or templates where the
control should appear in the note body.

It is weaker as the primary schema system because options live in controls or
control templates, not as a vault-wide metadata model, and it does not audit
existing notes.

### Forms

Modal Forms can present forms with select-list fields and can be called from
Templater, QuickAdd, or JavaScript. This is useful when structured capture is
the desired workflow.

Data Entry uses JSON Schema and JSON Forms to edit metadata through forms, but
the community plugin page still describes work-in-progress behavior and shows a
last update from years ago. It is not the first choice for this vault.

Forms are best for capture, not routine property maintenance.

### Dataview Audit

Dataview is already installed. It cannot stop invalid values at edit time, but
it can show every existing note that violates a rule. That makes it the lowest
friction enforcement backstop.

Simple DQL can hard-code allowed values:

```dataview
TABLE status
FROM "eat"
WHERE status AND !contains(list("liked", "not_liked"), status)
```

For a single source of truth, use DataviewJS to read the same line-based
allowed-values notes that Metadata Menu uses. A custom `bob` validator can also
read those same files.

### Propsec

Propsec is an in-Obsidian validation plugin for frontmatter. It targets notes by
folder path, tag, and property conditions, then reports violations in the status
bar/sidebar. It is read-only: it does not modify notes.

It supports string regex constraints, array constraints, required/warn flags,
custom types, union types, uniqueness, cross-field constraints, and conditional
validation. It does not appear to have a first-class enum type in the public
docs, so enums would likely be expressed as regex patterns.

Use it if you want lightweight in-Obsidian validation warnings without repair
operations.

### Forge

Forge is a broader vault-maintenance plugin. Its public plugin page documents
schema validation with explicit enum values, vault linting, frontmatter
normalization, patch operations, repair workflows, dry runs, backups, and
restore support.

It is the strongest plugin-shaped answer to "enforce and repair enum drift",
but it is also new. As of 2026-06-09, the community listing shows it was
created recently and has a small install base. Pilot it narrowly before trusting
repair operations across the vault.

Use it if you want schema validation and repair workflows inside Obsidian.

### Custom `bob` Validator

A small `bob` command would give the most controllable enforcement:

```yaml
rules:
  - name: restaurant status
    paths: ["eat/**/*.md"]
    property: status
    allowed: [liked, not_liked]
  - name: ref status
    paths: ["ref/**/*.md"]
    property: status
    allowed:
      - unread
      - wip
      - read
      - abandoned
      - collect_fleeting_notes
      - review_fleeting_notes
      - review_lit_notes
```

The command should parse Markdown frontmatter with a real YAML parser, validate
values by path/tag/type, report violations, and only support `--fix` for
explicit migrations.

This does not help with in-editor suggestions, but it is the best option for
versioned rules, tests, cron, git hooks, and SASE workflows.

## Recommended Solution

Use Metadata Menu plus an audit/validation backstop.

Start small:

1. Create allowed-values notes, one value per line, for the first scoped enum:
   for example `_meta/enums/restaurant.status.md`.
2. Install Metadata Menu.
3. Create a `restaurant` FileClass mapped to `eat/` or `type: restaurant`.
4. Define `status` as a `Select` sourced from the allowed-values note.
5. Add a Dataview or DataviewJS compliance note that reports restaurant notes
   whose `status` is not `liked` or `not_liked`.
6. After the workflow feels good, repeat for `ref.status`.
7. Defer `type` until deciding whether canonical values should be wikilinks or
   plain strings.

If the Dataview dashboard is too passive, add a validator:

- Use Propsec for read-only in-Obsidian validation warnings.
- Use Forge if enum schemas plus lint/repair workflows are worth piloting.
- Use a `bob` validator if the rules should be versioned, testable, and
  automation-friendly.

Final recommendation: adopt Metadata Menu FileClasses for scoped, curated
property suggestions, backed first by a Dataview/DataviewJS compliance
dashboard and later by Propsec, Forge, or a custom `bob` validator if stronger
enforcement proves necessary. Do not rely on native Obsidian autocomplete alone.

## Sources

- [Obsidian Help: Properties](https://obsidian.md/help/properties)
- [Obsidian Help: Properties view](https://obsidian.md/help/Plugins/Properties%2Bview)
- [Obsidian Help: Bases syntax](https://help.obsidian.md/bases/syntax)
- [Obsidian Forum: Enumerated properties feature request](https://forum.obsidian.md/t/enumerated-properties-unique-select-from-a-prefixed-set-of-values/63900)
- [Obsidian Forum: Predefined values for multi-select properties](https://forum.obsidian.md/t/add-predefined-values-to-multi-select-properties/75414)
- [Obsidian Forum: Customizable property suggestion lists](https://forum.obsidian.md/t/customizable-property-suggestions-lists/105142)
- [Obsidian Forum: Scope Bases property suggestions to filtered folder](https://forum.obsidian.md/t/obsidian-base-plugin-limit-property-value-suggestion-to-filtered-folder/109469)
- [Metadata Menu overview](https://mdelobelle.github.io/metadatamenu/)
- [Metadata Menu fields](https://mdelobelle.github.io/metadatamenu/fields/)
- [Metadata Menu FileClasses](https://mdelobelle.github.io/metadatamenu/fileclasses/)
- [Metadata Menu controls](https://mdelobelle.github.io/metadatamenu/controls/)
- [Templater system module](https://silentvoid13.github.io/Templater/internal-functions/internal-modules/system-module.html)
- [QuickAdd API](https://quickadd.obsidian.guide/docs/QuickAddAPI/)
- [Meta Bind input fields](https://www.moritzjung.dev/obsidian-meta-bind-plugin-docs/guides/inputfields/)
- [Meta Bind select input](https://www.moritzjung.dev/obsidian-meta-bind-plugin-docs/reference/inputfields/select/)
- [Modal Forms plugin](https://community.obsidian.md/plugins/modalforms)
- [Data Entry plugin](https://community.obsidian.md/plugins/data-entry)
- [Dataview query types](https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/)
- [Propsec plugin](https://community.obsidian.md/plugins/propsec)
- [Forge plugin](https://community.obsidian.md/plugins/forge)
