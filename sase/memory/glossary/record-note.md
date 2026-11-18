---
keyword: Record Note
---

A durable item in an [[album-note]]'s folder, with frontmatter `type: "[[record]]"`
and `parent` linking to its album. Its schema-defined fields hold facts about the item;
optional `## Notes` bullets hold observations, with dates when the album enables dated
notes. The album's generated index catalogs its record notes. Album sync maintains one
Bob-owned walk line on each record's first body line, linking it to the previous and
next records in the album's title order; the position count is rendered live and never
stored. Capture can create or update one with `@album/` or `@album/<stem>`;
`bob records new|update` writes the same record-note format and `bob records check`
validates it. Record notes are not notebooks,
references, or tasks. See `docs/records.md`.
