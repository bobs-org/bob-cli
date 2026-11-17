---
keyword: Ref Panel
aliases:
  - "Bob Refs"
  - "refs panel"
---

The reference-library window of the `bob-mac-capture` menu-bar app, named **Bob
Refs** in its menus and docs: a near-full-screen, non-activating glass panel that
quick-opens any PDF-backed [[reference-note]] while Highlights stays frontmost,
hiding again when it loses key focus. Control-Shift-Command-O or `Command-]` toggles
it from any app; while Highlights is frontmost, `Command-O` opens it in place of
Highlights' Open dialog, and `Command-T` and `Command-Y` open it on Highlights' open
tabs or on the focused PDF's Copy card. A search field filters two-line rows (kind
tile, title, reading state, a NEW dot, and a Now pill while the row's
[[reference-task]] has a [[task-link]] under one of today's open ledger entries)
grouped under pinned headers such as Just scanned, NEW, NOW, Reading, Next, and
Ready; `Command-0` regroups them by date added and `Command-T` narrows them to
Highlights' open tabs. An inspector column beside the list fills from `bob ref show`.
Return opens the PDF in Highlights (the note when the PDF is missing) and changes
nothing in the vault; other keys open the note or the reading task in Obsidian,
reveal the PDF in Finder, or raise in-panel cards: Copy (IDs, links, code blocks,
highlights; [[decisions:code-copy-reads-render-time-catalog]]), Rename
(`bob ref rename`; [[decisions:reference-renames-keep-identity]]), and Link to Today
(`bob ref today`). `Command-S` runs `bob ref scan -a -w` and reports what it filed
([[decisions:open-pdf-writes-are-coordinated]]). Escape or `Ctrl+[` backs out of a
card or banner, then hides the panel with its view remembered for the next show. Like
the [[capture-panel]], it is a thin client
([[decisions:mac-capture-is-a-thin-client]]): rows come from `bob ref list` and
`bob plan`, and it never parses notes or writes the vault itself. Its NEW section,
Seen marks, and open history are app-local state, never the `]s` walk's NEW tier.
Only one of the two panels is visible at a time; Ctrl+Tab switches between them, and
each comes back as it was left. Not Highlights, not the reference's Obsidian note,
and not `bob ref` CLI output. See `docs/refs.md` and `docs/refs-inspector.md` in the
`bob-mac-capture` repo.
