---
keyword: Capture Panel
---

Bob Mac Capture's capture window: the pre-warmed, non-activating `NSPanel` that the
global hotkey (Control-Shift-Command-I by default, plus `Command-\`) or the menu-bar
item's Capture row opens over any app. It stacks a one-line editor that grows with
the draft, an auxiliary region, and footer actions that always stay visible (Capture,
Preview, Discard, Stash, and the `⌃↩ Open` chip). The auxiliary region shows today's
Pomodoro agenda while the draft is empty
([[decisions:idle-capture-shows-ledger-agenda]]), then completion and picker cards,
bob's live preview (`bob capture --dry-run`), and errors as you write. Every show
puts the input line at one fixed eye line; height follows content, and width stays
resizable. It is a thin client ([[decisions:mac-capture-is-a-thin-client]]): spans,
completions, rewrites, previews, and picker rows all come from `bob capture` and
`bob capture-*` JSON, and submitting sends the whole draft, one item or a
[[bulk-capture]], as exactly one [[capture]]. The panel hides only after bob reports
success; a failure leaves the full draft open beside an actionable error. Escape,
`Ctrl+[`, and Ctrl+Tab hide it with the draft and caret retained; Discard clears the
draft; Control-C, Quit, and Restart stash a nonempty draft that Control-S restores.
It also works without capturing: Go To (`/` tasks, `//` notes) and event note mode
(`Command-E`) open inside it over the retained draft. Only one of it and the
[[ref-panel]] is visible at a time, and Ctrl+Tab switches between them. The app's
menus and docs call it just "Capture"; in this glossary a capture is one
`bob capture` run, so say "capture panel" for the window. Not the Obsidian
[[command-mode]] sheet, the app's menu-bar item or Settings window, or the
Hammerspoon [[mac-menu-bar-pomodoro-indicator]]. See Bob Mac Capture's
`docs/capture-panel.md`.
