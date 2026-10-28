---
keyword: Theme Hue Is Config Then Pin Then Exempt Then Unconfigured Then Area Then Hash
summary:
  A theme's hue is configured themes color, then journal pin, then plan.exempt
  mute, then the unconfigured mute, then top-level area stem, then name hash;
  clients render the hue Bob reports.
metadata:
  status: accepted
  decided: 2026-10-28
---

**Applies to.** bob-cli catalog and JSON, Obsidian theme pills, Bob Mac Capture
swatches, terminal `bob themes` output.

**Claim.** Hue is resolved in order: a configured `themes:` color
(`hue_source: config`); else a journal pin (`hue_source: pin`); else a
normalized `plan.exempt` key is muted (`hue: null`, `hue_source: exempt`); else
a key outside the configured vocabulary is muted (`hue: null`,
`hue_source: unconfigured`); else a key equal to a top-level area note's stem
(parent `[[area]]`) wears that area's resolved hue (`hue_source: area`); else
`Hue::ALL[fnv1a32(key) % 8]` (`hue_source: hash`). Rust computes the hue.
Clients render the `hue` Bob reports and never hash or scan areas themselves.
Colors are stable, not unique; pins fix recurring collisions.

**Why.** Config beats pin because the user-owned file is the source of truth:
editing it must always take effect. A config color also beats the exempt mute,
so PLAN can wear blue and still not count toward `plan.max_themes`. Rejected
alternatives:

- **Pin beats config** — editing the config file would silently lose to an old
  journal pin.
- **Hex colors** — every surface already has a light/dark pair and an xterm-256
  code per palette slot; a hex color would render gray on the Mac and need
  derived variants everywhere.
- **A unique color per theme** — the palette is eight slots.

Evidence: `plan:202610/configured_pomodoro_themes.md` PT5 revised;
`docs/pomodoro-themes.md` PT5, PT12.

**Cost.** `bob themes color` refuses with `config_color` on a theme whose
config sets a color. Journal pins shadowed by a config color stay in the
journal untouched and apply again if that config color is removed. Every
client must wait for Bob's `hue` field.

**Reopens when.** The palette grows, area notes stop being the hue source for
matching stems, or a client needs a hue before Bob can answer.

Supersedes [[decisions:theme-hue-is-pin-exempt-area-hash]].
