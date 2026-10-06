---
keyword: Inbox File
aliases:
  - "inbox note"
---

A Bob vault Markdown file whose tasks await triage: `inbox.md` at the vault root, or an
[[area-note]] whose frontmatter `parent` resolves to `inbox.md` (today `mac_inbox.md`
and `gkeep_inbox.md`). Only direct area children count, and the filename is irrelevant:
a [[project-note]] filed under one (`gkeep_gdocs_inbox_dump`) or an untyped child
(`inbox_overflow`) is not an inbox file. Classification reads live frontmatter on every
gesture, and `inbox.md` counts by design though it holds no `#task` lines today. By
default, `bob capture` and Bob Mac Capture file new tasks in `mac_inbox.md`, and
`bob gkeep pull` drains Google Keep into `gkeep_inbox.md`. New tasks carry no `fresh`
stamp, so most surface for triage in the NEW tier of the `]s` walk. On an open task in
an inbox file, every non-closing Ctrl+Shift+P answer and a Ctrl+Shift+Enter toggle on
the task line ask where the task goes just before writing, apply the action in place,
then move the task without following it; the route picker never offers another inbox
file, and `⇧↵` applies the answer without moving. Ctrl+Shift+M also moves it, following
it to the destination, and a closed task leaves through `bob task archive`. The current
inbox files set `ready_cap: off`. bob-plugins and `docs/projects.md` ("Inbox routing")
say "inbox note" (`api.inboxRoute.isInboxNote`); that is not a Google Keep inbox note,
the Keep item `bob gkeep` drains. bob-cli capture's `INBOX_FILE` constant and
`kind: inbox` target mean only `mac_inbox.md`, the default capture route.
