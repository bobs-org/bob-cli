---
keyword: Capture Alias
aliases:
  - "sigil alias"
---

A keystroke substitution in Bob Mac Capture's editor: an easy-to-reach key that the app
swaps for the [[capture]] sigil it stands for, only where that sigil would act at the
caret by opening its picker or filling a close slot. bob decides every one
([[decisions:mac-capture-is-a-thin-client]]). The app sends the post-key draft and caret
to `bob capture-rewrite --cursor`, which answers with rule `expand_sigil_alias` (summary
`Typed ^ for .`), and the app applies that one edit natively, so Undo restores the typed
key. The aliases:

- `.` → `^` right after `@route` (`@errands.` → `@errands^`); `;` → `:` there, and as a
  whole item (Task Link picker). In a [[project-note-capture]], a first-level `- .` or
  `- ;` opens the Move picker and `- ^.` gives `- ^^`.
- `.` → `@` after body text and a space or tab, wherever a bare `@` opens route
  completion (`Buy milk .` → `Buy milk @`).
- `.` → `&` and `;` → `*` after text or a marker and a space or tab, where a bare `&` /
  `*` opens the Depends On / Blocks picker and no `^`, `@`, or `:` alias applies
  (`@home .` → `@home &`, `Buy milk ;` → `Buy milk *`; `Buy milk .` stays `@`).
- `,` → `+` wherever the Parent Task picker would open (`@cash,` → `@cash+`); glued to a
  complete `@route+id` target, it inserts ` +` for the next pick.
- `\` or `/` → `#` wherever a `#` menu would open (task type, section, Pomodoro name,
  wikilink heading, and the rest). Never at a line start, glued to a word, or after
  another `\`/`/`; `/` stays literal where it is already field text (`=/`, `[[folder/`,
  `@@` routes).
- `-` → `~` inside an `x=` past token (`x=2-` → `x=2~`); a spaced `-` stays literal.
- `=` → `~` after a space or tab where the Links To relation picker would open, while
  `~=` / `~~=` flip to `~~` / `~` ([[decisions:relation-equals-alias-and-flips]]).
- `x` / `s` → `!` / `*` (complete / park) when the lowercase letter ends a close
  selection: `xx` → `x!`, `+2 xs` → `+2 x*`, `2xx` → `2x!`, `@r:id=xx` → `@r:id=x!`.

Open pickers honour bob's `action_continuation_aliases`: `,` acts as `+` in the lone-`+`
Parent Task picker, and `.` acts as `^` in the `- ^` Move picker. In the relation picker
and label card, `=` flips direction like `~`. The only app-local alias is a blank
draft's `\` → `/`, which opens Go To. Each alias can be escaped: by Undo; by a fast
second `=` or the spaced `.`/`;` fallbacks (350 ms), which type one literal (`===`
gives `==`; `..` and `;;` give one `.` or `;`); by a second `\`/`/` at any speed; or
by Space or Escape on the still-empty picker the alias opened. Aliases that change
meaning (`x`/`s`, and `-` in `x=`) hold later edits and
Return until bob answers. On a timeout the key stays literal and Return is dropped ("Bob
did not answer; press Return again"). An older bob leaves every alias literal.

Aliases exist only while typing. `bob capture`, `capture-parse`, paste, and piped stdin
keep the text, so a pasted `Buy milk .` captures a dot and `xx` closes nothing. Template
drafts and `#"…"` exact-name literals are never rewritten. Rewrites in the same lane
that edit an existing marker are not aliases: the block-ID separator toggle
(`@route:id^` → `@route^id`), the glued `~`/`=` relation direction flip, the `|` label
attach, and `@@` absorption. None of these is a capture alias either: the Mac's "Capture
alias" hotkey (Settings › Hotkey's global `Command-\`), a frontmatter or
`project_name_aliases` route alias, a wikilink `|alias`, a deprecated tag spelling
(`#offclock`), the bare owner alias `^route:id`, Tab's `--` → `—` snippet, or the
close-list comma (`x1` then `2` → `x1,2`). See bob-cli `docs/capture.md`
(`bob capture-rewrite`, "Sigil alias") and Bob Mac Capture's `docs/keyboard.md` and
`docs/capture-panel.md`.
