# Gcal description vectors

These files are bob-cli-only. They pin the structured description parse
that `bob gcal` uses for Event Task bullets. They are not mirrored into
bob-plugins.

`tests/fixtures/event_buffer/vectors.json` remains the shared
Rust/Obsidian contract for **directive verdicts** (`route`, `untracked`,
`before`, `after`). This directory does not replace it. New directive
cases that the shared file must not grow — a heading that now ends a
line, an Outlook-style `text<https://…>` autolink, a directive inside
`<li>` — live in `outline.json` alongside the outline itself.

## Files

- `outline.json` — description-parser: source tokenize, directive
  classification, directive-line drop, and outline (lead-in, depth cap,
  checkbox strip). No escaping, no vendor-boilerplate filter, no
  rendering.
- `boilerplate.json` — boilerplate-filter: separator, badge-duplicate,
  and vendor-block drops on top of the description-parser behavior, plus
  the one kept join-link bullet. No escaping, no rendering.
- `bullets.json` — reserved for the bullet-renderer phase.

## `outline.json` notation

Each case has `id`, `input`, `lines`, and optionally `directives` and
`oversized`.

A line is:

```json
{
  "depth": 1,
  "marker": "-",
  "inlines": [{ "text": "And retainers!" }]
}
```

## `boilerplate.json` notation

Each case adds an optional `join_link` and `location` (the event's badge
targets, trimmed; absent means the row has none). The filter drops whole
lines only:

- Separators (`-_=*~` runs, `<hr>`, Google's `-::~…::-` wall, Webex's
  do-not-delete sentence) always drop, and double as vendor-region
  fences.
- A fence-bounded region holding a vendor header or vendor join URL drops
  whole, fences included.
- A vendor header line, or a line holding a vendor join URL, starts a run
  through the next blank line or fence. Secondary headers (`Meeting ID:`,
  `Passcode:`, …) only start runs once the description holds a primary
  anchor somewhere, so they stay prose in ordinary agendas.
- A line that only repeats `join_link` or `location` (whole text, or a
  lone link to it) drops.
- When dropped vendor content held a join URL and the event has no
  `join_link`, one `Join … meeting` bullet survives, linking at the kept
  URL (redirect target when the link was wrapped).

Every pure-boilerplate invite whose row already has its join badge yields
zero lines; the without-badge Zoom case yields the one kept bullet. All
IDs, numbers, and passcodes below are fake.

- `depth` is `1` or `2`.
- `marker` is `"-"` for a bullet (including ordinary paragraphs) or
  `"N."` for an ordered item. Plain `N)` markers are stored as `"N."`.
- Inlines:
  - `{ "text": "…" }` — plain text
  - `{ "text": "…", "bold": true }` and/or `"italic": true`
  - `{ "text": "…", "url": "…" }` — a link; the parser keeps redirect
    URLs raw

`directives` uses the same keys as the shared event-buffer vectors:
`route`, `untracked`, `before`, `after`. Omitted keys default to
`null` / `false` / `0`.

The parser treats the whole input as HTML when it contains a known tag
or `<!--`; otherwise it takes the plain path. Known tags are listed in
the description-parser phase of
`plan:202611/gcal_description_bullets.md`.
