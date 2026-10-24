# Pomodoro themes

A Pomodoro **theme** is one `+`-separated component of a named ledger entry's
` — NAME` tail. Bob derives a catalog of themes from canonical daily notes,
folds hide and color pins from a per-host journal, and assigns each theme a
stable hue. Clients render what Bob reports; they never scan the vault or
compute a hue of their own.

This is the shared contract for Rust, the Obsidian ledger mirror, Bob Mac
Capture, and the CLI. Rust owns the meaning. JSON grows additively at
`schema_version` 1.

Rules marked **planned (phase …)** are part of this contract and are not
built yet. The `catalog` phase ships PT1–PT5, the journal *reader*, ranking
for `bob themes list -q`, and the read-only list command.

## PT1 — Vocabulary and identity

- **Theme:** one `+`-separated component of a Pomodoro entry's ` — NAME` tail.
  Split with `split_components`. The key is `normalize_component`: whitespace
  collapsed, lowercased. The display name is the most recent authored spelling
  for that key.
- **Pomodoro name:** the whole tail. `— BOB + DECKS` is one name with two
  themes.
- **Not themes:** an unnamed entry, the budget's `(unnamed)` placeholder, a
  Pomodoro home, or an Obsidian appearance theme.
- **Naming in code:** `pomodoro_themes` (Rust), `bob-pomo-theme-*` (CSS),
  `PomodoroTheme*` (Swift), `api.themes` (JS).
- **User-facing words:** **theme**, **Recent**, **Hide**/**Unhide**, **Pin
  color**, and **Rename theme**. Hiding changes suggestions only.

## PT2 — Catalog

- **Source:** every canonical daily note, `YYYY/YYYYMMDD.md` and nothing else.
  Not `_templates/`, `done/`, `_conflicts/`, or `*_done.md` / `*_poms.md`
  siblings. Read column-0 entries in `## Pomodoros`, open and closed, through
  `capture_pomodoros::scan`. Fenced lines are skipped.
- **Per-theme fields:** `key`, `name`, `first_used`, `last_used`, `days_used`,
  and `entries`. Only days up to and including today count. Future-dated notes
  feed rename, not recency.
- **`today`:** `open`, `closed`, or absent, for the theme's presence in
  today's ledger. Open wins when both occur.
- History is derived only from ledger entries. Never infer it from task text
  or homes.

## PT3 — Cache

- **Location:** `bob_cli_cache_dir()/themes/catalog-v1.json`, keyed by vault
  root. Machine-local, never synced, disposable.
- **Contents:** a per-file `(mtime_ns, size)` fingerprint and that file's
  theme summary.
- Today's note is always read live.
- A full sweep of past days runs at most once per 60 s, or when a year
  directory's mtime changes.
- Rebuild on a corrupt or missing cache, after a rename, and with `--fresh`.
  The rename preview always rescans from source. **planned (phase rename)**
- Writes are atomic (temp file + rename).
- **Budget:** warm `capture-complete` and `capture-parse` stay within +5 ms
  p95 of today's baseline on apollo against the live vault. **planned
  (phase suggest / theme-json)** for those hot paths; `bob themes list`
  uses the same cache.

## PT4 — Preferences journal

- **Location:** `.bob/themes/<host>.jsonl`, one append-only file per host,
  inside the vault so it syncs.
- **`host`:** the same host label vault-sync uses in its commit subjects.
- **Events (`v: 1`):** `theme.hide`, `theme.unhide`, `theme.color` (`hue` is a
  slot name, or `null` to unpin), and `theme.rename`. `via` names the client:
  `bob-cli`, `bob-mac-capture`, or a plugin id.
- **Fold:** read every `*.jsonl` file; skip malformed lines with a bounded
  warning; sort by `(ts instant, host, line number)`; last writer wins per key
  for the hidden state and for the pin. A rename moves the source key's
  explicit state to the target unless the target already has explicit state.
  Events after the rename on the old key apply to the old key.
- **Writers:** bob is the only writer. **planned (phase prefs)** for hide,
  unhide, and color; **planned (phase rename)** for rename. This phase ships
  the reader and the fold.

## PT5 — Hue rule

In order:

1. A journal pin wins (`hue_source: "pin"`).
2. Else a key in normalized `plan.exempt` is muted: `hue: null`,
   `hue_source: "exempt"` (GTD by default).
3. Else a key equal to a **top-level area note's stem** (an area whose
   `parent` is `[[area]]`) wears that area's resolved hue
   (`hue_source: "area"`): a frontmatter `color:` slot, else the stem hash.
4. Else `Hue::ALL[fnv1a32(key) % 8]` (`hue_source: "hash"`).

Results: JOB → aqua, DEV → blue, LOVE → yellow, GTD → muted. Palette: the
eight existing slots (blue, orange, aqua, yellow, magenta, green, violet,
red), with their light/dark hex values and xterm-256 codes. Colors are
stable, not unique; pins fix recurring collisions. Rust computes the hue.
JS mirrors it under shared vectors. Swift and Lua only consume the `hue`
Bob reports.

## PT6 — Visual language

**planned (phase theme-json / mac-colors / obsidian-colors)**

Names wear a tint; places wear a dot.

- **Pill** (Obsidian, Mac): a soft tint of the hue behind normal-colored
  text (16 % light / 24 % dark; hover 24 % / 32 %). Corner radius 4 px,
  horizontal padding about 0.3 em, no border, `box-decoration-break: clone`.
  The hue is never the text color.
- **Muted pill:** exempt themes such as GTD. Hover background, muted text.
- **Swatch:** in dense rows, an 8 pt rounded square (radius 2) before the
  name. Area homes keep their ● circle.
- **Composites:** each component gets its own pill; the `+` stays neutral.
- **CLI:** an xterm-256 `■` before a default-color name when color is
  enabled (TTY and no `NO_COLOR`). An exempt theme gets `□`. `bob themes
  list` already draws these swatches.
- **Typing:** a name the catalog does not know stays neutral while you type
  it. **planned (phase theme-json / mac-colors)**
- Color is never the only signal. The name text, Today/Recent labels, and
  `hidden`, `pinned`, and `exempt` badges carry the meaning under `NO_COLOR`.

## PT7 — Menu composition

**planned (phase suggest)** for capture-complete and Obsidian pickers.

Filter first, then group:

1. **A:** Today rows that match the query, in today's order.
2. **B:** Recent exact and prefix matches. Sort exact first, then
   `last_used` descending, `days_used` descending, then name.
3. **C:** The create row, only when the query is creatable and no A or B row
   is an exact match.
4. **D:** Recent substring matches, in B's order.
5. **E:** Today's query-independent utility rows.

Recent rows exclude hidden themes and any theme that is a component of a
named Today row in the same menu. An empty query shows at most 5 Recent
rows; a non-empty query shows at most 12. Matching is ASCII
case-insensitive on slugs. A partial quoted literal matches on the
normalized key prefix.

`bob themes list -q` already ranks B then D over the catalog. Menu caps,
today-row composition, and `similar_to` are **planned (phase suggest)**.

Fetching, filtering, and accepting suggestions never touch the journal.

## PT8 — Lossless acceptance

**planned (phase lossless)**

A history row emits its plain slug only when the slug round-trips to the
displayed name and does not prefix-select a different today entry.
Otherwise it emits the exact-name literal `#"NAME"`. Compare by key
equality, never by slug or prefix.

## PT9 — Hide

**planned (phase prefs / mac-menus / obsidian-pickers)**

Hiding removes a theme from **Recent** rows only. Today's rows, typing the
name, history, Stats, and colors are unaffected. Using a hidden name never
unhides it. Ctrl+D in a Pomodoro-name menu hides a Recent row in place as a
ghost, with undo. `bob themes list -H` already shows hidden themes; the
hide/unhide writers are planned.

## PT10 — Global rename

**planned (phase rename)**

```text
bob themes rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline]
```

One previewed, verified, single-commit rewrite of the theme component in
` — NAME` tails of canonical daily notes plus `_templates/daily.md`, modeled
on `bob ref rename`. Undo is `git revert`.

## JSON contract

Additive; `schema_version` stays 1.

**`ThemeRef`:** `{key, name, hue, hue_source}`, plus `known: false` only for
names the catalog does not know. Where offsets matter, it also carries
`start` / `end` byte offsets into the draft. **planned (phase theme-json)**
for capture-parse and agenda attachments; `theme_refs_for_name` already
builds this shape.

**`bob themes list -f json`:**

```json
{
  "schema_version": 1,
  "today": "2026-10-24",
  "window_days": 90,
  "counts": { "known": 81, "listed": 40, "today": 4, "hidden": 2 },
  "themes": [
    {
      "key": "bob",
      "name": "BOB",
      "hue": "magenta",
      "hue_source": "hash",
      "pinned": false,
      "hidden": false,
      "exempt": false,
      "first_used": "2026-08-26",
      "last_used": "2026-10-24",
      "days_used": 26,
      "entries": 49,
      "today": "open"
    }
  ],
  "hints": [],
  "renamed_reappeared": [],
  "warnings": []
}
```

`hints` is empty until **planned (phase prefs)** fills `theme_hue_collision`.
`renamed_reappeared` is empty until **planned (phase rename)**.

**Writers** (`hide`, `unhide`, `color`, `rename`) emit `{schema_version, ok,
...}` and `{ok:false, code, message}` on refusal. **planned (phase prefs /
rename)**

## `bob themes` CLI

The group is `bob themes`, a top-level catalog noun in Daily workflow after
`stats`. Bare `bob themes` is `list`. Members, alphabetical in help:

- `color NAME HUE|auto` **planned (phase prefs)**
- `hide NAME…` **planned (phase prefs)**
- `list [-a/--all] [-F/--fresh] [-f/--format human|json] [-H/--hidden] [-n/--limit N] [-q/--query Q]`
- `rename OLD NEW …` **planned (phase rename)**
- `unhide NAME…` **planned (phase prefs)**

List defaults: visible themes used in the last 90 days or today, newest
first. `-a` shows every theme and flags hidden ones. `-H` shows only hidden
themes. `-q` ranks per PT7 groups B then D.

Human list:

```text
THEMES  81 known · 4 today · 2 hidden · last 90 days
  ■ BOB        today     26 days   magenta
  ■ JOB        Oct 23    24 days   aqua · area
  ■ BLOG       Oct 23    14 days   red · pinned
  □ GTD        today     51 days   muted · exempt
```

## Shared vectors

`tests/fixtures/pomodoro_themes/vectors.json` is the single source. Sections:
`components`, `hue` (including the `areas` input), `fold`, and `rank`.
`menu`, `similar`, and `literal` are **planned (phase suggest)**. bob-plugins
copies the file byte-for-byte **planned (phase obsidian-colors)**.

## Related

- [Plan budget](plan.md#themes)
- [Daily Stats](stats.md)
- [Pomodoro homes](pomodoro-homes.md)
- [Vault Git sync](vault-git-sync.md)
- [Capture](capture.md)
