---
title: Task drawers
description: The dual-read contract for task drawer headers in Bob vault notes.
---

# Task drawers

This page defines the bob-cli reader contract during the dual-read window. A
task drawer groups ordinary notes below a direct child of a `#task` row. Drawers
hold notes; they do not hold requirements or define task status.

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
outside fences and quotes, have no checkbox, and not be a managed task row or a
reserved name. Its body must be exactly one title: `**title**` for a bold drawer,
or a title accepted by the preserved legacy ALL-CAPS grammar. Children are
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

The notes home is matched by the `NOTES` key. During this phase the writer title
remains `NOTES`; a hand-authored `**Notes**` header is reused without adding a
second header or rewriting its spelling. On a `^prj` task, the existing `##
Notes` section remains the notes home.

## Dual-read window and outside text

Readers accept bold and legacy headers. Writers, capture drafts, completion
rows, and header creation stay on the legacy contract in this phase. In
particular, this page does not add `- **Title**` as capture draft syntax.

Imported or pasted child text that would become either kind of drawer is
written with double underscores, such as `__Agenda__`. It renders bold while
remaining a note. Google Calendar's rendered-span hash includes this body, so an
untouched Google span picks up the escape on its next sync.

## Title mapping and source ranges

The pure `heading_for_drawer` and `drawer_for_heading` mapping preserves a bold
title verbatim between a drawer and an eligible `##` section. Legacy drawer
titles keep today's Title Case mapping. An empty `##` section maps to a valid
empty drawer; that mapping does not change the nonempty capture-creation rule.
Invalid and reserved titles return named refusals.

`Drawer.header_range` and `Drawer.title_range` are half-open UTF-8 byte offsets
into the supplied Markdown source. The header range covers the complete line
without its line ending. The title range excludes indentation, list markers,
`**`, and the `## ` prefix. LF and CRLF both use byte offsets.

## `bob capture-drawers` JSON

Each drawer object and each recent-spelling object has an additive `marker`:
`bold`, `legacy_caps`, or `section`. The `title` remains verbatim. Existing
fields and `schema_version: 1` stay unchanged. The drawer catalog is version 3
and keeps marker provenance separately for each spelling.
