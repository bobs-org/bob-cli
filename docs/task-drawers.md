---
title: Task drawers
description: The dual-read contract for task drawer headers in Bob vault notes.
---

# Task drawers

This page defines the bob-cli and Obsidian plugin reader contract during the
dual-read window. The four plugin readers share one JavaScript twin and the
Rust-owned vectors below. A task drawer groups ordinary notes below a direct
child of a `#task` row. Drawers hold notes; they do not hold requirements or
define task status.

## Recognized headers

Bob reads both the existing ALL-CAPS form and the new whole-bold form:

```markdown
- [ ] #task Prepare the release
  - **API Design**
    - Review the endpoint
  - NOTES
    - Check the release notes
```

A drawer header must be a direct list-item child of a real task. It must be
outside fences and quotes, have no checkbox, and not be a managed task row. A
bold title must also avoid reserved names. Legacy ALL-CAPS titles keep their
existing grammar and remain readable, including unadorned Schedule, Work, and
Rotten Log titles; the rendered managed log rows are excluded. Children are
optional for recognition, so a hand-written empty header can be filled. Capture
creation still requires content.

Bold titles are one line, nonempty, contain no `*`, have no whitespace directly
inside the delimiters, contain at least one Unicode letter or digit, and do not
end in `:`. Internal colons and punctuation are allowed (`Q1: Goals`,
`C++ Notes`, and `Q&A`). Text after the closing delimiters, a block ID, tag, or field
makes the line a note. `__Title__` is the visible bold-note escape. Reserved
structural names such as `Tasks`, `Projects`, and `Notebooks` remain notes.

A whole-bold list item deeper than a task's direct children is ordinary note
content. Managed task logs, Cancel Log, Depends-On, BLOCKS, LINKS / LINKED FROM,
and SUB-PROJECTS are checked before drawer recognition.

## Identity and Notes

The displayed title is always the authored title. Bob derives identity with
`drawer_key`: collapse whitespace, then apply Unicode uppercase. Thus `Notes`
and `NOTES` share `NOTES`, while `Straße` maps to `STRASSE` and `Café` maps to
`CAFÉ`. If one task contains two headers with the same key, lookup reports an
ambiguity and names both rows.

The notes home is matched by the `NOTES` key. Every bob-cli writer creates
`- **Notes**`; a hand-authored `**Notes**` header and a legacy `NOTES` header
are each reused without adding a second header or rewriting their spelling.
On a `^prj` task, the existing `## Notes` section remains the notes home.

## Dual-read window and outside text

Readers in bob-cli, task-status-cycler, bob-navigation-hotkeys, block-id-prompt,
and bob-ledger-tools accept bold and legacy headers. Every bob-cli writer
creates bold headers: `- **Title**` for a task drawer and `- **Notes**` for
the notes drawer. Capture also accepts a whole-bold first-level container
header, such as `- **API Design**`, with nested `  - …` notes. In a new
project note it writes `## API Design`; in a `%n` notebook creation seed it
writes `## API Design`; and in a record capture it creates a matching
`## API Design` group. Task drawer titles and project section titles preserve
the spelling inside the bold markers. Bullet drawer completion inserts the
whole bold header and opens a nested bullet when none follows.

An ALL-CAPS first-level bullet with nested notes in one of these container
positions is retired capture syntax. Bob refuses it with `caps_drawer_retired`
and suggests the equivalent bold header. A childless ALL-CAPS bullet remains
ordinary note or task text. A bold childless container header is refused with
`empty_drawer_bullet`; nest notes beneath it as `  - …`. Use `__Title__` when
you need visible bold text that stays an ordinary note. In an existing
route-only notebook list, bold text remains an ordinary note; `- #section`
continues to select a notebook section. Bold section interpretation is
enabled for notebooks only while parsing `%n` creation seeds.

Imported or pasted child text that would become a drawer is written with
double underscores, such as `__Agenda__`. It renders bold while remaining a
note. Google Calendar's rendered-span hash includes this body, so an untouched
Google span picks up the escape on its next sync.

## Title mapping and source ranges

The pure `heading_for_drawer` and `drawer_for_heading` mapping preserves a bold
title verbatim between a drawer and an eligible `##` section. Legacy drawer
titles keep today's Title Case mapping. An empty `##` section maps to a valid
empty drawer; that mapping does not change the nonempty capture-creation rule.
Invalid and reserved titles return named refusals.

## Rename and migration inventory

`bob task rename-drawer OLD NEW` matches OLD by its Unicode case-folded drawer
key and writes NEW verbatim. Task drawers are written as `- **NEW**`; eligible
project sections are written as `## NEW`. NEW follows the capture drawer-title
rules, including the reserved-name, no-asterisk, and no-trailing-colon checks.

When OLD and NEW have the same key, Bob respells each matching header in place.
Legacy task headers become bold, bold task headers stay bold, and project
headings keep their separate bodies. Duplicate identities stay separate during
case-only respelling. With different keys, the dry run shows any merge with an
existing NEW header or repeated OLD headers before `--apply` can merge their
bodies at the earliest position. Section links are repaired; block links do not
change. A second run reports no changes.

`bob task rename-drawer -l, --legacy` runs a read-only migration inventory and
takes no OLD or NEW. It lists every legacy ALL-CAPS task drawer, every empty
bold task drawer, bold trailing-colon leads for information, and duplicate
drawer or project-section identities. The inventory includes open and closed
tasks and archived rows under `done/`; `--format json` reports the same items
with `schema_version: 1`. It does not lock, sync, write, or commit.

`Drawer.header_range` and `Drawer.title_range` are half-open UTF-8 byte offsets
into the supplied Markdown source. The header range covers the complete line
without its line ending. The title range excludes indentation, list markers,
`**`, and the `## ` prefix. LF and CRLF both use byte offsets.

## `bob capture-drawers` JSON

Each drawer object and each recent-spelling object has an additive `marker`:
`bold`, `legacy_caps`, or `section`. The `title` remains verbatim. Existing
fields and `schema_version: 1` stay unchanged. The drawer catalog is version 3
and keeps marker provenance separately for each spelling.
