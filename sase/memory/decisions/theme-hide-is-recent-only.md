---
keyword: Hiding A Theme Removes It From Recent Only
aliases:
  - theme hide
  - unhide
  - hidden recent
summary:
  Hiding a theme removes it from Recent suggestion rows only; today, Stats,
  colors, and typing the name stay unchanged, and using it never unhides it.
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli `bob themes`, capture-complete Recent rows, Obsidian
and Mac Pomodoro-name menus.

**Claim.** Hiding a theme removes it from **Recent** rows only. Today's ledger
rows, typing the name, history, Stats, and colors are unaffected. Using a hidden
name never unhides it. `bob themes list` omits hidden themes by default; `-H`
lists only them; `-a` lists every known theme and flags hidden ones.

**Why.** Deleting a theme from the catalog would rewrite history. Auto-unhiding
on use would fight the hide gesture the next morning. Muting colors on hide
would mix a suggestion preference with the hue rule. Rejected alternatives:

- **Delete from the catalog** — Stats and old daily notes would lose the name.
- **Unhide on next use** — a hidden name typed to start a Pomodoro would bounce
  back into Recent.
- **Hide also mutes color** — color would stop meaning identity.

Evidence: `plan:202610/pomodoro_themes.md` PT9; `docs/pomodoro-themes.md`.

**Cost.** Hidden themes still occupy Stats and still wear a hue. Menus must
filter Recent after ranking, not drop the catalog row.

**Reopens when.** Hide should also suppress Stats or color, or using a hidden
name should be an unhide gesture.
