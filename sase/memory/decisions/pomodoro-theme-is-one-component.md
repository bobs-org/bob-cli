---
keyword: A Pomodoro Theme Is One Name Component
aliases:
  - theme identity
  - plus-separated theme
  - pomodoro theme key
summary:
  A theme is one + component of a Pomodoro's — NAME tail, keyed by
  normalize_component, with the catalog derived from canonical daily notes.
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, the Bob vault.

**Claim.** A Pomodoro **theme** is one `+`-separated component of a named ledger
entry's ` — NAME` tail. Split with `split_components`. The key is
`normalize_component` (whitespace collapsed, lowercased). The display name is the
most recent authored spelling for that key. `— BOB + DECKS` is one Pomodoro name
with two themes. An unnamed entry, the budget's `(unnamed)` placeholder, a
Pomodoro home, and an Obsidian appearance theme are not themes. The catalog is
derived from canonical daily notes `YYYY/YYYYMMDD.md` through
`capture_pomodoros::scan`, plus the hide/color journal. `bob themes` lists it.

**Why.** Treating the whole ` — NAME` as one theme would hide composites such as
`BOB + DECKS` from per-component recency, color, and hide. Scanning task text or
homes would invent history the ledger never recorded. Rejected alternatives:

- **Whole-name identity** — `BOB + DECKS` would be a third theme beside BOB and
  DECKS, and hide/color could not target one component.
- **Slug-only keys** — `SASE V18` and `SASE-V18` would collide or split against
  the plan budget's existing `normalize_component`.
- **Inferring themes from tasks or homes** — the ledger would stop being the
  source of truth.

Evidence: `plan:202610/pomodoro_themes.md` PT1–PT2; `docs/pomodoro-themes.md`.

**Cost.** Every consumer splits names the same way. A rename rewrites one
component across tails, not a whole name. Clients must not scan the vault for
identity.

**Reopens when.** The plan budget and the catalog disagree on what a component
is, or unnamed entries need a remembered identity.
