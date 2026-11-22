---
keyword: Record Note
---

A durable item in an [[album-note]]'s folder, with frontmatter `type: "[[record]]"`
and `parent` linking to its album. Its schema-defined fields hold facts about the item;
optional `## Notes` bullets hold observations, with dates when the album enables dated
notes. The album's generated index catalogs its record notes. Album sync maintains one
Bob-owned walk line on each record's first body line; the walk follows the album's title order, or with `walk_by` set, the field's choice groups in declared order and then title order, so `[n` / `]n` finish a group before entering the next and `[N` / `]N` jump between group heads; the position count is rendered live and never
stored. Capture can create or update one with `@album/` or `@album/<stem>`;
`bob records new|update` writes the same record-note format and `bob records check`
validates it. Record notes are not notebooks,
references, or tasks. See `docs/records.md`.
