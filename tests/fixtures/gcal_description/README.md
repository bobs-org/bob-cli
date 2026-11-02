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
- `boilerplate.json` — reserved for the boilerplate-filter phase.
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
