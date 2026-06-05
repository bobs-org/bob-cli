---
create_time: 2026-06-05 10:39:22
status: done
prompt: sdd/prompts/202606/eat_base.md
---
# Eat Base Design Plan

## Goal

Create a single, beautiful `~/bob/eat.base` that turns the 90 migrated restaurant notes under `~/bob/eat/` into a
polished, browsable Obsidian Bases dashboard — sortable, filterable, grouped, and pleasant to look at — **without
modifying any of the migrated notes**. All cleanup of the messy source data happens in the base via formulas, so the
notes stay faithful migration artifacts.

## Context

The prior migration created one note per restaurant under `~/bob/eat/` with this frontmatter schema:

```yaml
parent: "[[eat]]" # always [[eat]]
type: restaurant # always "restaurant"  -> stable Bases filter
title: "Kitchen 27"
status: liked # liked (84) | not_liked (6)
category: "Breakfast" # cuisine/type, free-text from legacy table
location: "Linden" # plain text OR a live wiki-link to a town note
source_group: union_county # union_county | general | breakfast | not_liked
lid: "kitchen_27" # original LID, or null (9 notes)
companions: ["[[kelly]]"]
source_note: "[[eat]]"
source_anchors: [z-251208-0j, ...]
source_dates: ["251208", "250724"] # YYMMDD, stored newest-first
source_lines: [101, 115]
```

Two realities of this data drive the design:

1. **`category` is messy** — 36 distinct free-text spellings across 90 notes, including near-duplicates and typos
   (`Cheesesteaks` / `Cheeseteaks` / `Cheesteaks / Wings`; `Burgers` / `Burgers + Wings` / `Burgers?`; `Bar Food` /
   `Barfood`; `Salad` / `Salads`), plus 6 with no category. Grouping on the raw field would produce 36 noisy one-off
   groups.
2. **`location` is mixed-form** — some values are plain text (`"Westfield"`, `"Clark"`), others are live wiki-links to
   town notes (`"[[nj_towns#^z-241117-0b|cranford]]"`), and casing collides (`Westfield` vs `westfield`).

The vault already establishes a Bases house style in `~/bob/refs.base`: global `filters`, emoji-badge `formulas`,
`properties` with `displayName`, and several purpose-built table `views` with `groupBy` / `order` / `sort`. This design
follows that style and extends it. Bases is enabled as a **core** plugin, so no install step is needed.

## Design Principles

- **Notes stay untouched.** Normalization (cuisine bucketing, town extraction, date formatting) lives entirely in base
  formulas. The migration artifacts remain a faithful, auditable copy of `eat.md`.
- **Tame the mess into clean buckets.** Collapse 36 category spellings into ~25 emoji-tagged cuisine families so
  grouping is meaningful and attractive.
- **Beautiful and useful.** Emoji-led columns, sensible default grouping, and multiple views that answer real questions
  ("what do we love?", "what's near us?", "where did we eat recently?", "what to avoid?").
- **Faithful, not invented.** No ratings, prices, addresses, or visit counts are fabricated — only what the notes
  already contain is surfaced or reformatted.

## Formulas

These computed columns are the heart of the design.

### `name` — clickable restaurant title

`if(title, file.asLink(title), file.asLink())` → renders the display title as a link to the note.

### `verdict` — status badge

`liked` → `💚 Liked`, `not_liked` → `🚫 Not Liked`.

### `cuisine` — normalized, emoji-tagged category (the centerpiece)

A null-guarded, ordered `if(...category.lower().contains(...))` chain that buckets every spelling into one clean family.
Order matters (e.g. cheesesteaks are matched before the generic `wing`/`steak` checks; Italian before Dine-in). The
mapping was validated against all 36 real category values — every note buckets cleanly with no bad fall-through:

| Raw category values (validated)                                     | Bucket             |
| ------------------------------------------------------------------- | ------------------ |
| Cheesesteak, Cheesesteaks, Cheeseteaks, Cheesteaks / Wings          | 🧀 Cheesesteaks    |
| Burgers, Burgers + Dogs, Burgers + Wings, Burgers / Wings, Burgers? | 🍔 Burgers         |
| Pizza                                                               | 🍕 Pizza           |
| Sushi                                                               | 🍣 Sushi           |
| Burritos, Mexican                                                   | 🌮 Mexican         |
| Thai Food                                                           | 🍜 Thai            |
| Chinese Food                                                        | 🥡 Chinese         |
| Korean Fried Chicken                                                | 🍗 Korean          |
| Wings                                                               | 🍗 Wings           |
| Hot Dogs                                                            | 🌭 Hot Dogs        |
| Hoagies                                                             | 🥖 Hoagies         |
| Cuban                                                               | 🥪 Cuban           |
| Dessert                                                             | 🍰 Dessert         |
| Shakes                                                              | 🥤 Shakes          |
| Steakhouse                                                          | 🥩 Steakhouse      |
| Coffee, Bagels / Coffee                                             | ☕ Coffee & Bagels |
| Breakfast                                                           | 🥞 Breakfast       |
| Salad, Salads                                                       | 🥗 Salads          |
| Bar Food, Barfood                                                   | 🍺 Bar Food        |
| Italian / Dine-in                                                   | 🍝 Italian         |
| Dine-in                                                             | 🍽️ Dine-in         |
| Convenience Store                                                   | 🏪 Convenience     |
| Misc, Dinner (any unmatched)                                        | 🍴 \<original\>    |
| (none / null)                                                       | ❓ Uncategorized   |

Resulting group sizes: Breakfast 18, Coffee & Bagels 11, Burgers 7, Mexican 6, Uncategorized 6, Misc 5, Cheesesteaks 4,
Pizza 4, Salads 4, Sushi 4, Bar Food 3, Dine-in 3, and a long tail of singletons.

### `town` — clean town name for grouping

Strips the `[[town#^anchor|alias]]` wrapper down to the alias, normalizes casing, and merges duplicates:
`location.replace(/\[\[[^\]|]*\|/, "").replace("]]", "").replace("[[", "").lower().title().replace("Nyc", "NYC")`. So
`[[nj_towns#...|cranford]]` and a plain `Cranford` both group under **Cranford**, `westfield`/`Westfield` merge, and
`NYC` is preserved (not lower-cased to "Nyc"). Used only for grouping/sorting; the table still **displays the raw
`location`** so live town wiki-links stay clickable.

### `latest_code` + `last_tried` — most recent visit date

`latest_code = source_dates.sort().reverse().slice(0, 1).join("")` picks the newest `YYMMDD` code; `last_tried` parses
it into a real date and formats it: `date("20" + ...).format("MMM YYYY")` → e.g. **Dec 2025**. `latest_code` (a
fixed-width string that sorts chronologically) is used as the real sort key for date-ordered views, so months sort
correctly rather than alphabetically.

### `source` — back-link to the legacy table

`link("eat", "📄 eat.md")` for one-click audit back to the original source note.

## Views

A focused set of views, each answering a real question. `🍽️ All Restaurants` is first so the base opens on a strong,
robust table.

1. **🍽️ All Restaurants** (table) — everything, grouped by `cuisine`, with a per-group **Count** summary. Columns:
   Restaurant · Verdict · Location · Last Tried · LID. The default landing view.
2. **💚 Favorites** (table) — `status == "liked"`, grouped by `cuisine`, sorted most-recently-tried first within each
   group. The "where should we go?" view.
3. **🗺️ By Town** (table) — grouped by `town`, with Count summaries. Columns: Restaurant · Cuisine · Verdict · Last
   Tried. The "what's good near us?" view.
4. **🕐 Recently Tried** (table) — no grouping, sorted by `latest_code` descending (newest first), limit 30. A timeline
   of recent meals.
5. **🚫 Not For Us** (table) — `status == "not_liked"`, the six-row avoid list.
6. **🖼️ Gallery** (cards) — the same data as cards, grouped by `cuisine`, for a visual browse. _(Cards is a real Bases
   view type in current Obsidian; it is placed last so that even if a given Obsidian build lacks it, the five table
   views — the core experience — are unaffected.)_

## Proposed `~/bob/eat.base`

```yaml
filters:
  and:
    - file.inFolder("eat")
    - note.type == "restaurant"

formulas:
  name: "if(title, file.asLink(title), file.asLink())"
  verdict: 'if(status == "liked", "💚 Liked", if(status == "not_liked", "🚫 Not Liked", status))'
  cuisine:
    'if(category, if(category.lower().contains("hees"), "🧀 Cheesesteaks", if(category.lower().contains("burger"), "🍔
    Burgers", if(category.lower().contains("pizza"), "🍕 Pizza", if(category.lower().contains("sushi"), "🍣 Sushi",
    if(category.lower().containsAny("burrito", "mexican"), "🌮 Mexican", if(category.lower().contains("thai"), "🍜
    Thai", if(category.lower().contains("chinese"), "🥡 Chinese", if(category.lower().contains("korean"), "🍗 Korean",
    if(category.lower().contains("wing"), "🍗 Wings", if(category.lower().contains("dog"), "🌭 Hot Dogs",
    if(category.lower().contains("hoagie"), "🥖 Hoagies", if(category.lower().contains("cuban"), "🥪 Cuban",
    if(category.lower().contains("dessert"), "🍰 Dessert", if(category.lower().contains("shake"), "🥤 Shakes",
    if(category.lower().contains("steak"), "🥩 Steakhouse", if(category.lower().containsAny("coffee", "bagel"), "☕
    Coffee & Bagels", if(category.lower().contains("breakfast"), "🥞 Breakfast", if(category.lower().contains("salad"),
    "🥗 Salads", if(category.lower().contains("bar"), "🍺 Bar Food", if(category.lower().contains("italian"), "🍝
    Italian", if(category.lower().contains("dine"), "🍽️ Dine-in", if(category.lower().contains("convenience"), "🏪
    Convenience", "🍴 " + category)))))))))))))))))))))), "❓ Uncategorized")'
  town:
    'if(location, location.replace(/\[\[[^\]|]*\|/, "").replace("]]", "").replace("[[",
    "").lower().title().replace("Nyc", "NYC"), "—")'
  latest_code: 'if(source_dates, source_dates.sort().reverse().slice(0, 1).join(""), "")'
  last_tried:
    'if(formula.latest_code, date("20" + formula.latest_code.slice(0, 2) + "-" + formula.latest_code.slice(2, 4) + "-" +
    formula.latest_code.slice(4, 6)).format("MMM YYYY"), "")'
  source: 'link("eat", "📄 eat.md")'

properties:
  formula.name:
    displayName: Restaurant
  formula.verdict:
    displayName: Verdict
  formula.cuisine:
    displayName: Cuisine
  formula.town:
    displayName: Town
  formula.last_tried:
    displayName: Last Tried
  formula.source:
    displayName: Source
  note.location:
    displayName: Location
  note.lid:
    displayName: LID

views:
  - type: table
    name: 🍽️ All Restaurants
    groupBy:
      property: formula.cuisine
      direction: ASC
    order:
      - formula.name
      - formula.verdict
      - note.location
      - formula.last_tried
      - note.lid
    sort:
      - property: formula.name
        direction: ASC
    summaries:
      formula.name: Count

  - type: table
    name: 💚 Favorites
    filters:
      and:
        - status == "liked"
    groupBy:
      property: formula.cuisine
      direction: ASC
    order:
      - formula.name
      - note.location
      - formula.last_tried
    sort:
      - property: formula.latest_code
        direction: DESC
      - property: formula.name
        direction: ASC
    summaries:
      formula.name: Count

  - type: table
    name: 🗺️ By Town
    groupBy:
      property: formula.town
      direction: ASC
    order:
      - formula.name
      - formula.cuisine
      - formula.verdict
      - formula.last_tried
    sort:
      - property: formula.name
        direction: ASC
    summaries:
      formula.name: Count

  - type: table
    name: 🕐 Recently Tried
    order:
      - formula.name
      - formula.cuisine
      - note.location
      - formula.verdict
      - formula.last_tried
    sort:
      - property: formula.latest_code
        direction: DESC
      - property: formula.name
        direction: ASC
    limit: 30

  - type: table
    name: 🚫 Not For Us
    filters:
      and:
        - status == "not_liked"
    order:
      - formula.name
      - note.location
      - formula.cuisine
      - formula.last_tried
    sort:
      - property: formula.name
        direction: ASC

  - type: cards
    name: 🖼️ Gallery
    groupBy:
      property: formula.cuisine
      direction: ASC
    order:
      - formula.name
      - formula.verdict
      - note.location
      - formula.last_tried
    sort:
      - property: formula.name
        direction: ASC
```

## Implementation Steps

1. Write the file above to `~/bob/eat.base`.
2. Sanity-check the YAML parses and the `filters` select exactly the 90 `eat/` notes (and exclude `eat.md` itself).
3. Re-run the offline bucketing check (already green) to confirm the `cuisine` mapping still covers every category
   value, in case the notes changed.
4. Spot-check representative rows against expected formula output:
   - `kitchen_27` → 🥞 Breakfast, 💚 Liked, Linden, Dec 2025.
   - `garlic_rose` → 🍽️ Dine-in, Cranford, Nov 2025.
   - `1_walnut` → ❓ Uncategorized, 🚫 Not Liked, Cranford.
   - An NYC note → Town renders as **NYC**, not "Nyc".
5. Leave `~/bob/eat.md`, `~/bob/maybe_eat.md`, and every note under `~/bob/eat/` untouched; confirm the repo worktree is
   clean (only `eat.base` added in the vault).

## Validation

- `eat.base` is valid YAML and opens in Obsidian without errors.
- The base scopes to exactly 90 restaurant notes.
- `cuisine` shows ~25 clean emoji groups (no raw 36-way spread, no broken fall-through).
- `town` merges casing/wiki-link duplicates and preserves NYC.
- `last_tried` shows human dates and date-ordered views sort chronologically (via `latest_code`).
- No migrated note, `eat.md`, or `maybe_eat.md` is modified.

## Risks & Notes

- **Cards view availability.** `cards` is a current Bases view type, but it is intentionally placed **last** so the five
  table views (the core experience) are unaffected on any Obsidian build that doesn't render it.
- **`summaries: Count`.** Per-group counts are a tasteful enhancement; if an Obsidian build ignores the aggregation, the
  tables still render — only the footer count is affected.
- **Source-data normalization is presentation-only.** If you later want the underlying `category`/`location` cleaned in
  the notes themselves, that's a separate, follow-up pass; this plan deliberately keeps the notes pristine.

## Out of Scope

- Editing, renaming, or re-categorizing any migrated note.
- Touching `~/bob/eat.md` or `~/bob/maybe_eat.md`.
- Enriching with external restaurant metadata (ratings, prices, addresses, photos).
- Building additional bases beyond `eat.base`.
