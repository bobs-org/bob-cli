---
title: Capture Sigils Say Where, What, and How
summary: "'@' says where, glued '#' names a place and free '#' a type, '%' says how."
requested: project
aka:
  - capture sigils say where what how
  - where what how grammar
---

**Applies to.** bob-cli and Bob Mac Capture.

**Claim.** `@` says where a capture goes, `#` names a place or a type, and `%` says
how the draft expands. `@note`, `@note#Heading`, `@note+id`, `@note+id#drawer`, and
`@@note` route the item. A `#` glued to a destination token names a place inside it: a
note heading, a [[task-drawer]], or a Pomodoro name. A `#` standing alone at an item
edge names a [[task-type]]: `#fix` types a new task or retypes the selected existing
task in the same transaction. `%c`, `%n`, `%t`, and `%l` at an item edge declare clip,
Pomodoro-note, template, and listen directives. Commit matching is exact and creation
is on demand; the drawer menu's prefix assistance never decides what exists.

**Why.** One sigil per question keeps the grammar teachable and the errors local: a
misplaced sigil fails through the ordinary grammar error for the sigil actually typed,
with copy that teaches only the current grammar. Exact matching at commit time (never
from menu state) plus quoted-literal and `--drawer` creation overrides removed the old
prefix-matching surprises and the attached `@route+id#type` ambiguity, where one suffix
meant two things. Rejected alternatives were keeping attached `#type` on `+id` and
`^id`, prefix drawer matching, a separate drawer sigil, and menu-decided creation.

**Cost.** Every retired attached form (`@route+id#type`, `@route^id#type`) and the old
`-S/--task-section` spelling fail as ordinary grammar errors; the `capture-parse` and
`capture-complete` wire renames (`drawer`, `drawer_text`, `invalid_drawer`,
`ambiguous_drawer`, `reserved_drawer`, `drawer_near_duplicate`,
`drawer_near_catalog`) and the `grammar: 3` bump are breaking changes carried on the
release train.

**Reopens when.** Bryan changes what a glued versus free `#` means, adds a sigil, or
restores prefix matching or attached typing.

**Evidence.** `plan:202610/capture_drawers_and_directives.md` §§2–3 and
`plan:202610/capture_task_drawers.md`; bob-cli `cli-drawers` implementation
(`src/native/capture_drawers.rs`, `capture_language/` grammar revision 3,
`bob capture-drawers` with hidden `capture-task-sections` alias). See also
[[glossary:task-drawer]] and [[glossary:task-type]].
