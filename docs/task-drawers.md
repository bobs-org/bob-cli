---
title: Task drawers
description: Bold task drawer headers, Obsidian trays and typing, and the title round-trip.
---

# Task drawers

This page defines the bob-cli and Obsidian plugin reader contract. The four
plugin readers share one JavaScript twin and the Rust-owned vectors below. A
task drawer groups ordinary notes below a direct child of a `#task` row.
Drawers hold notes; they do not hold requirements or define task status.

## Recognized headers

A task drawer is a whole-bold title on a direct list-item child of a real task:

```markdown
- [ ] #task Prepare the release
  - **API Design**
    - Review the endpoint
  - **Notes**
    - Check the release notes
```

The header must be outside fences and quotes, have no checkbox, and not be a
managed task row. Its title is one line, nonempty, contains no `*`, has no
whitespace directly inside the delimiters, contains at least one Unicode
letter or digit, and does not end in `:`. Internal colons and punctuation are
allowed (`Q1: Goals`, `C++ Notes`, and `Q&A`). Text after the closing
delimiters, a block ID, tag, or field makes the line ordinary content.
`__Title__` is the visible bold-note escape. Reserved structural names such as
`Tasks`, `Projects`, and `Notebooks` remain notes.

A whole-bold list item deeper than a task's direct children is ordinary note
content. Managed task logs, Cancel Log, Depends-On, BLOCKS, LINKS / LINKED
FROM, and SUB-PROJECTS are checked before drawer recognition.

Plain uppercase bullets are ordinary content in every position. They do not
become drawers or sections, even when they have nested notes. `capture-parse`
adds the nonblocking `caps_drawer_retired` warning when a plain bullet with
nested notes matches a known drawer key in the catalog; the warning suggests
writing the title as `- **Title**`. Unknown plain titles remain ordinary
content without a warning.

## Identity and Notes

The displayed title is always the authored title. Bob derives identity with
`drawer_key`: collapse whitespace, then apply Unicode uppercase. Thus `Notes`
and `NOTES` share `NOTES`, while `Straße` maps to `STRASSE` and `Café` maps to
`CAFÉ`. If one task contains two headers with the same key, lookup reports an
ambiguity and names both rows.

The notes home is matched by the `NOTES` key. Every bob-cli writer creates
`- **Notes**`; a hand-authored `**Notes**` header is reused without adding a
second header or rewriting its spelling. On a `^prj` task, the existing `##
Notes` section remains the notes home.

## Capture selectors and create titles

A quoted `#"TITLE"` or `--drawer TITLE` selector preserves the authored
capitalization. An unquoted slug uses the catalog's preferred explicit
spelling (a `bold` or `section` marker on that key) when one exists, otherwise
the notebook create-title rule: hyphens become spaces, a word that already
contains an uppercase letter keeps its typed capitals (`#API-keys` → `API
Keys`), and every other word takes first-character Title Case (`#git` →
`Git`).

Existing drawers keep their stored title. Creation requires a valid bold
header: `#"NOTES:"` cannot create a drawer. Completion replacements are the
same text a commit would receive. A glued `#slug` replacement is used only
when that slug parses back to the row's title; otherwise the menu quotes
`#"TITLE"`. A bullet replacement that cannot express its target is refused.

## Reader and title contract

Readers in bob-cli, task-status-cycler, bob-navigation-hotkeys,
block-id-prompt, and bob-ledger-tools recognize whole-bold task drawer
headers. Bob's writers create `- **Title**` for task drawers and `- **Notes**`
for the notes drawer. Capture also accepts a whole-bold first-level container
header, such as `- **API Design**`, with nested `  - …` notes. In a new
project note it writes `## API Design`; in a `%n` notebook creation seed it
writes `## API Design`; and in a record capture it creates a matching
`## API Design` group. Task drawer titles and project section titles preserve
the spelling inside the bold markers. Bullet drawer completion inserts the
whole-bold header and opens a nested bullet when none follows.

A bold childless container header is refused with `empty_drawer_bullet`; nest
notes beneath it as `  - …`. In an existing route-only notebook list, bold
text remains an ordinary note; `- #section` continues to select a notebook
section. Bold section interpretation is enabled for notebooks only while
parsing `%n` creation seeds.

## Title mapping and source ranges

The pure `heading_for_drawer` and `drawer_for_heading` mapping preserves a
bold title verbatim between a drawer and an eligible `##` section. An empty
`##` section maps to an empty drawer; that mapping does not change the
nonempty capture-creation rule. Invalid and reserved titles return named
refusals.

## Rename

`bob task rename-drawer OLD NEW` matches only bold task drawers by their
Unicode case-folded drawer key and writes NEW verbatim. Task drawers are
written as `- **NEW**`; eligible project sections are written as `## NEW`.
NEW follows the capture drawer-title rules, including the reserved-name,
no-asterisk, and no-trailing-colon checks.

When OLD and NEW have the same key, Bob respells each matching header in
place. Duplicate identities stay separate during case-only respelling. With
different keys, the dry run shows any merge with an existing NEW header or
repeated OLD headers before `--apply` can merge their bodies at the earliest
position. Section links are repaired; block links do not change. A second run
reports no changes.

`Drawer.header_range` and `Drawer.title_range` are half-open UTF-8 byte
offsets into the supplied Markdown source. The header range covers the
complete line without its line ending. The title range excludes indentation,
list markers, `**`, and the `## ` prefix. LF and CRLF both use byte offsets.

## `bob capture-drawers` JSON

Each drawer object and each recent-spelling object has an additive `marker`:
`bold` or `section`. The `title` remains verbatim. Existing fields and
`schema_version: 1` stay unchanged. The drawer catalog is version 4 and keeps
marker provenance separately for each spelling.

## Obsidian rendering

Bob Ledger Tools paints a recognized drawer header in Live Preview and reading
view. A quiet tray occupies the existing list-marker slot (~0.95em), tinted
with the owner task's type hue mixed toward muted ink. The authored title stays
on the line at weight 600; nested notes keep normal weight. Attributes are
source-backed: `data-bob-drawer`, `data-bob-drawer-key`,
`data-bob-drawer-count` (direct notes only; fences do not count),
`data-bob-drawer-empty`, and `data-bob-drawer-owner-state`, plus the owner's
type hue and state. Folded drawers append a muted tabular `· n`. Empty drawers
use a hollow dashed tray. `[x]` and `[-]` owners desaturate the glyph and
title. Native Live Preview still reveals `**` on the caret line. **Toggle task
drawer marks** (default on) removes only those attributes and styles and never
rewrites Markdown.

## Obsidian typing

On an empty non-checkbox bullet directly under a real `#task`, typing `*`
writes `- **|**` (shown as `- ****` with the caret in the middle). One Undo
returns the literal `- *`. `autoPairMarkdown` on and off produce the same
bytes because the assist inserts its own star. A `*` at the end of a valid
title, immediately before the closing `**`, steps over both stars. Enter there
opens a nested note bullet. Backspace inside empty `****` deletes all four
stars and keeps the list marker. The assist does not fire in fences, quotes,
frontmatter, on a checkbox, or deeper than a task's direct children.

**Toggle Drawer** wraps or unwraps that direct child, preserving title bytes
and descendants. Checkbox lines, managed lines, reserved names, invalid or
trailing-colon titles, and deeper positions show an actionable notice and
write nothing.

## Writers

Every bob-cli writer and the plugin twins emit `- **Title**` for a new task
drawer and `- **Notes**` for the notes home. An existing bold header is reused
without respelling. Plugin conversion uses `heading_for_drawer` /
`drawer_for_heading`: bold titles round-trip byte for byte onto `## Title` and
back to `- **Title**`. Eligible empty H2 sections and explicitly authored
empty drawers are preserved. Invalid and reserved headings refuse before any
write.

## Mac Capture

Bob Mac Capture is a thin client: Bob owns drawer grammar, completion, preview
data, and writes, and the app decodes and presents them. Preview
`task_blocks[].lines[]` drawer headers carry an additive
`drawer: {title, key, marker, empty}` object with `marker` one of `bold`,
`legacy_caps`, or `section`; per-item drawer summaries carry the same verbatim
`title` plus `key`, `created`, and additive `marker`. Missing, malformed, or
unknown drawer fields decode away and leave the existing verbatim preview.
Transfer and merge-in destinations keep the drawer title verbatim and never
uppercase it. The app renders an annotated header as an outline tray plus the
verbatim title (muted when `empty`), and paints draft titles semibold in the
section color with dimmed `**` delimiters. Typing `*` where Bob admits a drawer
expands to `- **|**` through `expand_drawer_marker` as one undoable edit;
Backspace on an empty `****` removes all four stars. At a drawer title end,
Shift-Return, Option-Return, or Ctrl-J nests a note after the closing pair, a
typed `*` steps over it, and Return keeps its capture meaning. Capture fixtures
in the Mac repository are generated from clean bob-cli checkouts (preview
fixtures at grammar 31, rewrite and skeleton fixtures at grammar 33) and
record source revision, version, and inputs.
