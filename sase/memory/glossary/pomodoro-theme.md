---
keyword: Pomodoro Theme
aliases:
  - theme
---

One `+`-separated component of a [[pomodoro]]'s ` — NAME` suffix, keyed by the plan
budget's component normalization and displayed in the most recently authored spelling.
Pomodoro names are written in ALL CAPS: `— BOB + DECKS` is one name with two themes.
`plan.max_themes` counts distinct non-exempt components across today's open entries;
components in `plan.exempt` (default `[GTD]`) do not count. The Stats `theme`
dimension splits a named session into the same components. The catalog is derived
from canonical daily notes plus `.bob/themes/<host>.jsonl`; `bob themes` lists,
hides/unhides from Recent suggestions, pins/unpins colors, and renames components
through one previewed Bob transaction. Hiding removes a theme from history
suggestions only. Hue priority is pin, exempt mute, matching top-level area, then
stable hash. Each theme wears a tint or rounded swatch, never an area's ● dot. Not a
Pomodoro home, an Obsidian appearance theme, or `(unnamed)`. See
`docs/pomodoro-themes.md`.
