---
origin: requested
scope: project
keyword: q-and-ctrl-bracket-mirror-esc
summary: >-
  Every Bob panel that Esc closes or backs out of also does so with q and Ctrl+[,
  through Esc's own dispatch and regardless of focus; q types only in the panel's
  text controls, Ctrl+[ never does, and no panel binds q to anything else.
aliases:
  - q closes
  - ctrl+[ closes
  - close key parity
  - esc q ctrl-bracket
metadata:
  status: accepted
decided: 2026-11-17
---

# q And Ctrl+[ Mirror Esc In Every Panel

**Applies to.** Every panel Esc dismisses: Obsidian modals, cards, pickers, and
in-card panels across bob-plugins, and Bob Mac Capture panels.

**Claim.** Every Bob panel that Esc closes or backs out of also does so with
`q` and `Ctrl+[`, through Esc's own dispatch and regardless of focus. Both
keys are aliases of Esc's current layer: they back out exactly one layer and
then close, never a parallel implementation that could drift. The `q` text
exception: `q` types only in the panel's text controls (`input`, `select`,
`textarea`, `[contenteditable]`, the Emoji Slot search, and the Emoji Slot
well); focus outside the panel, for example the editor behind the modal, is
not a text context, so `q` closes and is consumed. `Ctrl+[` means Ctrl alone
and never types in fields. `q` is reserved: no hint badge, hotkey, or row key
may claim it.

**Why.** Bryan requested: "Also, add a new decision memory that states that
all panels that support the `<esc>` keymap to close that panel should also
support the `q` and `<ctrl+[>` keymaps." Vim habits agree: `q` closes help,
quickfix, and pager windows, and `Ctrl+[` is Esc. Repeated per-panel fixes
show opt-in drifts.

Rejected alternatives:

- **Per-panel opt-in.** Each new panel re-decides close keys and drifts.
- **`q` closing outright even from a stage.** It loses Esc's layering.
- **`q` closing inside text fields.** Typing would close the panel.
- **`q` as a hint badge.** The key would change meaning by mode.

Evidence:
bob-plugins `c9d4572` (Copy card),
bob-plugins `31e5c0e` (Task Card `Ctrl+[`), 
bob-plugins `f72bd4e` (Category card keys through the modal scope),
bead `bob-cli-a1` (image zoom closes on Esc, q, `Ctrl+[`), 
bob's reserved bank hotkey `q` (`src/native/bank/config.rs` `RESERVED_HOTKEY`),
and `plan:202611/schema_card_q_ctrl_bracket_close.md` (this change).

**Cost.** `q` can't be a panel hotkey or hint badge, every panel needs a
keymap-scope route, and text-control detection must be kept current.

**Reopens when.** A panel needs `q` as a content key, `Ctrl+[` should type in
fields, or a panel's Esc must do something other than back out.
