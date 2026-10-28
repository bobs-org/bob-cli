---
keyword: Theme Hue Is Pin Then Exempt Then Area Then Hash
aliases:
  - theme color
  - area hues
  - hue_source
summary:
  A theme's hue is journal pin, then plan.exempt mute, then top-level area
  stem, then name hash; clients render the hue Bob reports.
metadata:
  status: superseded
  decided: 2026-10-24
superseded_by:
  - "[[decisions:theme-hue-is-config-pin-exempt-area-hash]]"
---

**Applies to.** bob-cli catalog and JSON, Obsidian theme pills, Bob Mac Capture
swatches, terminal `bob themes` output.

**Claim.** Hue is resolved in order: a journal pin (`hue_source: pin`); else a
normalized `plan.exempt` key is muted (`hue: null`, `hue_source: exempt`); else
a key equal to a top-level area note's stem (parent `[[area]]`) wears that
area's resolved hue (`hue_source: area`); else
`Hue::ALL[fnv1a32(key) % 8]` (`hue_source: hash`). Rust computes the hue.
Clients render the `hue` Bob reports and never hash or scan areas themselves.
Colors are stable, not unique; pins fix recurring collisions.

**Why.** Hash-only identity made JOB and BOB the same magenta, and GTD would
wear an area orange it does not mean. Unique colors cannot fit an unbounded
catalog in an eight-slot palette. Rejected alternatives:

- **Hash only, even for area names** — JOB would not wear aqua.
- **A unique color per theme** — the palette is eight slots.
- **Clients hash independently** — JS, Swift, and Lua would drift.

Evidence: `plan:202610/pomodoro_themes.md` PT5; epic decision `area_hues=area`;
`docs/pomodoro-themes.md`.

**Cost.** Recurring collisions need an explicit pin. Exempt keys such as GTD
are muted even when an area note shares the stem. Every client must wait for
Bob's `hue` field.

**Reopens when.** The palette grows, area notes stop being the hue source for
matching stems, or a client needs a hue before Bob can answer.

Superseded by [[decisions:theme-hue-is-config-pin-exempt-area-hash]], which
retires the pin-first order: a configured `themes:` color now wins over the
journal pin, and themes outside the configured vocabulary mute as
`unconfigured`.
