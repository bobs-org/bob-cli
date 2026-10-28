# Pomodoro themes

A Pomodoro **theme** is one `+`-separated component of a named ledger entry's
` — NAME` tail. Bob derives a catalog of themes from canonical daily notes,
folds hide and color pins from a per-host journal, and assigns each theme a
stable hue. Clients render what Bob reports; they never scan the vault or
compute a hue of their own.

This is the shared contract for Rust, the Obsidian ledger mirror, Bob Mac
Capture, and the CLI. Rust owns the meaning. JSON grows additively at
`schema_version` 1. PT1–PT13 describe shipped behavior across the catalog,
capture menus, CLI, Obsidian plugins, Mac Capture, and menu-bar indicator.

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

- **Location:** `bob_cli_cache_dir()/themes/catalog-v1.json` and
  `known-keys-v1.json`, each keyed by vault root. Machine-local, never synced,
  disposable.
- **Contents:** a per-file `(mtime_ns, size)` fingerprint and that file's
  theme summary.
- Completion also keeps a disposable per-vault
  `themes/completion-v1-<vault-hash>.json` view containing precomputed past-theme totals and the days on which
  those themes occurred. It checks the source cache fingerprint, date, year-directory
  fingerprints, and sweep age before reuse; corruption, rename, and a fresh
  catalog rebuild invalidate it.
- Today's note is always read live. Completion reuses its current ledger scan
  when it is the canonical daily note.
- A full sweep of past days runs at most once per 60 s, or when a year
  directory's mtime changes.
- Rebuild on a corrupt or missing cache, after a rename, and with `--fresh`.
  The rename preview always rescans from source.
- Writes are atomic (temp file + rename).
- Stores prune entries for vault roots that no longer exist. When the catalog
  prunes a root, it also removes completion views not associated with a
  remaining vault and atomic-write temp files older than ten minutes, keeping
  the machine-local cache bounded by live vaults.
- **Budget:** warm `capture-complete` and `capture-parse` stay within +5 ms
  p95 of today's baseline on apollo against the live vault. `bob themes list`
  uses the same source cache.

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
- **Writers:** bob is the only writer. `bob themes hide`, `unhide`,
  `color`, and `rename` append under `bob_sync.lock` (wait at most 3 s; a busy
  lock is `lock_busy`). `bob themes split` rewrites daily notes under the same
  lock and writes no journal event. Each event is one `O_APPEND` write. The
  directory is created on first use.
- **Allowlist:** the vault tracks `.bob/themes/*.jsonl` (`!/.bob/themes/*.jsonl`
  in `.gitignore`) and unions concurrent appends
  (`.bob/themes/*.jsonl merge=union`). A host that has not synced the
  allowlist keeps its hides local until it does. See
  [Vault Git sync](vault-git-sync.md#pomodoro-theme-journal).

## PT5 — Hue rule

In order:

1. A configured color from the `themes:` block wins
   (`hue_source: "config"`).
2. Else a journal pin wins (`hue_source: "pin"`).
3. Else a key in normalized `plan.exempt` is muted: `hue: null`,
   `hue_source: "exempt"` (PLAN by default).
4. Else, when a `themes:` vocabulary is configured and the key is not in
   it (and is not an inventory label), the theme is muted: `hue: null`,
   `hue_source: "unconfigured"`.
5. Else a key equal to a **top-level area note's stem** (an area whose
   `parent` is `[[area]]`) wears that area's resolved hue
   (`hue_source: "area"`): a frontmatter `color:` slot, else the stem hash.
6. Else `Hue::ALL[fnv1a32(key) % 8]` (`hue_source: "hash"`).

Explicit choices (config, pin) beat mutes, and mutes beat implicit
sources (area, hash). Config beats pin because the user-owned file is the
source of truth: editing it must always take effect. A config color also
beats the exempt mute. Pins shadowed by a config color stay in the journal
untouched and apply again if that config color is removed.

Results: JOB → aqua, DEV → blue, LOVE → yellow, PLAN → muted. Palette: the
eight existing slots (blue, orange, aqua, yellow, magenta, green, violet,
red), with their light/dark hex values and xterm-256 codes. Colors are
stable, not unique; pins fix recurring collisions. Rust computes the hue.
JS mirrors it under shared vectors. Swift and Lua only consume the `hue`
Bob reports.

## PT6 — Visual language

Names wear a tint; places wear a dot.

- **Pill** (Obsidian, Mac): a soft tint of the hue behind normal-colored
  text (16 % light / 24 % dark; hover 24 % / 32 %). Corner radius 4 px,
  horizontal padding about 0.3 em, no border, `box-decoration-break: clone`.
  The hue is never the text color.
- **Muted pill:** exempt themes such as PLAN. Hover background, muted text.
- **Unconfigured pill:** a theme outside the configured vocabulary wears a
  1 px dashed outline in the platform warning color (Obsidian
  `--text-warning`, macOS `systemOrange`) at about 70 % opacity, over the
  muted pill, with a "not in your themes" tooltip/AX suffix. Like the
  typing rule, the mark appears only once the name is committed or the
  cursor leaves it.
- **Swatch:** in dense rows, an 8 pt rounded square (radius 2) before the
  name. The Day strip theme legend uses the same rounded square at the
  legend-dot footprint. Area homes keep their ● circle. The Day strip
  shades smaller same-hue themes; every other surface uses the exact hue.
- **Composites:** each component gets its own pill; the `+` stays neutral.
- **CLI:** an xterm-256 `■` before a default-color name when color is
  enabled (TTY and no `NO_COLOR`). An exempt theme gets `□`; an
  unconfigured theme gets `◌` (dim). `bob themes list` already draws these
  swatches; `bob plan`, capture destinations, and the Stats theme dimension
  use the same squares, and tmux uses its native format color before each
  named component. Under `NO_COLOR` the glyph and the
  `not configured` text still carry the meaning.
- **Typing:** a name the catalog does not know stays neutral while you type
  it; it takes its hue once it is committed or the cursor leaves the line.
- Color is never the only signal. The name text, Today/Recent labels, and
  `hidden`, `pinned`, and `exempt` badges carry the meaning under `NO_COLOR`.

## PT7 — Menu composition

Implemented in both capture-complete Pomodoro-name contexts and the Obsidian
Pomodoro-name pickers through `api.themes`.

Filter first, then group:

1. **A:** Today rows that match the query, in today's order.
2. **B:** Recent exact and prefix matches. Sort exact first, then
   `last_used` descending, `days_used` descending, then name.
3. **C:** The create row, only when the query is creatable and no A or B row
   is an exact match.
4. **D:** Recent substring matches, in B's order.
5. **E:** Today's query-independent utility rows.

Recent rows exclude hidden themes and any theme that is a component of a
named Today row in the same menu. Only rows the menu actually shows count. A
theme used today only by a done or cancelled Pomodoro is not a Today row in a
link or move picker, so it stays in Recent. In the start context it shows as
an "again" row. When a `themes:` vocabulary is configured, Recent rows come
only from that set: never-used configured themes rank after used ones, in
config order, with no `last_used`/`days_used`, and `similar_to` draws only
from visible configured themes. An empty query shows at most 5 Recent
rows, or 12 when themes are configured so the whole vocabulary can show; a
non-empty query shows at most 12. Matching is ASCII
case-insensitive on slugs. A partial quoted literal matches on the
normalized key prefix.

`bob themes list -q` ranks B then D over the catalog. Capture completion
adds Today rows, applies the caps, and folds an exact Recent match into the
create action. Recent rows carry `origin: "history"`, `last_used`, and
`days_used`; they create a Pomodoro, report the plan-theme preview, and omit
`ref` and `line`. Every named row carries component `themes`.

A create row carries `hidden: true` when its exact key is hidden, and
`similar_to` for at most two visible known names, most recent first. A query
needs at least three characters. Similarity means equality ignoring spaces
and punctuation, a singular/plural pair (`s` or `es`), or edit distance one
when both keys have at least five characters. This badge never changes
ranking or Return. When themes are configured, an unconfigured create row's
`themes` refs carry `configured: false`. Zsh descriptions show
`recent · Oct 21 · 3 days` for used Recent rows, `your theme · never used`
for never-used configured Recent rows, and `new · not in your themes` on
unconfigured create rows.

Fetching, filtering, and accepting suggestions never touch the journal.

## PT8 — Lossless acceptance

A history or “again” row emits its plain slug only when the slug round-trips to the
displayed name and does not prefix-select a different today entry.
Otherwise it emits the exact-name literal `#"NAME"`. Compare by key
equality, never by slug or prefix. In a bare start (`=name`), a literal
replacement includes the leading `#`; in a `#` field it replaces just the
name. Partial straight or curly quotes match normalized key prefixes, and
acceptance replaces the whole partial literal. Bob emits straight quotes.
A prefix-shadowed “again” name also uses a literal: with SASE V18 open,
accepting completed SASE creates SASE instead of starting SASE V18.

## PT9 — Hide

Hiding removes a theme from **Recent** rows only. Today's rows, typing the
name, history, Stats, and colors are unaffected. Using a hidden name never
unhides it. `bob themes hide NAME…` and `bob themes unhide NAME…` append
journal events; `bob themes list -H` lists hidden themes. Ctrl+D in a
Pomodoro-name menu hides a Recent row in place; the ghost remains selectable
and Ctrl+D again restores it. Exact hidden create rows can be unhidden, while
Today and ordinary create rows consume Ctrl+D without writing.

## PT10 — Global rename

```text
bob themes rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline]
```

One previewed, verified, single-commit rewrite of the theme component in
` — NAME` tails of canonical daily notes (past, today, and future, open and
closed) plus `_templates/daily.md`, area and project `theme:` defaults, and
`[theme::]` / `(theme::)` task fields (including `done/`), modeled on
`bob ref rename`.

- **Scope:** Pomodoro tails, note defaults on area/project notes, and task
  theme fields. Prose, fenced examples, `.bob/`, and `_conflicts/` are
  never touched.
- **Byte preservation:** separators keep their `+` spelling. A resulting
  duplicate collapses (`FIX + FIXES` → `FIX`). Quote style on `theme:`
  scalars is preserved. Task fields keep their bracket or paren form and
  take NEW's spelling; with `--merge`, fields simply take NEW. Every other
  byte is preserved.
- **Counts:** the preview (human and `-f json`) reports `sessions`,
  `defaults`, and `task_themes` separately and lists the default-bearing
  notes. JSON is additive; `entries` remains the session count.
- **NEW** must be a valid single theme. A NEW containing `+` is `invalid_name`.
- **Vault selection:** before taking a lock, an online write requires
  `-b/--bob-dir` and `BOB_DIR` (default `~/bob`) to resolve to the same
  directory. A mismatch or unresolved path refuses with `vault_mismatch`.
  Set `BOB_DIR` to the selected vault or use `--offline`. Previews and
  offline writes do not require this match; symlink aliases work.
- **Flow:** take `bob_sync.lock`, then pre-sync unless `--offline`, then
  replan from the synced files. Snapshot each target and compare again right
  before writing (a change aborts with `stale_note`). Write each file
  atomically, then append `theme.rename`. Verify by rescanning: OLD is absent
  from tails, defaults, and task fields, entry counts are unchanged, only
  name tails differ on session files, and every changed non-daily line
  differs only in the theme value span. On failure, restore
  the before-images. Make one commit, `bob themes rename: FIXES → FIX`, then
  post-sync. Undo is `git revert`.
- **Refusals:** `unknown_theme` (OLD never used), `invalid_name` (NEW
  contains `+` or is blank), `unchanged` (OLD and NEW share a key),
  `theme_exists` (NEW already used without `--merge`), `config_reference`
  (OLD is in `plan.exempt`, `plan.inventory_labels`, `plan.reading_theme`,
  a Stats metric filter, or the `themes:` vocabulary, and NEW is not — edit
  the chezmoi config to add NEW first; a configured OLD reads "OLD is a
  configured theme; add NEW under themes: first"; a `plan.reading_theme`
  OLD reads "OLD is plan.reading_theme; set plan.reading_theme to NEW
  first"), plus
  `lock_busy`, `dirty_paths` (offline with dirty targets), `not_a_worktree`,
  `vault_mismatch`, `sync_failed`, `verify_failed`, and `stale_note`.
- **Color and state:** the color follows the new name, and the preview says
  so. Hidden state and pins move with the rename per PT4.
- **Unconfigured NEW:** a NEW outside the vocabulary adds a `warnings`
  entry (`theme_not_configured` message) to the preview and result; human
  output marks it `◌ NEW not configured`. A `--merge` of two configured
  themes notes that OLD stays in `themes:` until you remove it.
- **Reappearance:** if OLD's spelling reappears after a rename event (an
  offline host, muscle memory), `bob themes list` reports it in
  `renamed_reappeared`. It only reports; it never rewrites on its own. The
  latest rename *from* each key is the candidate, using the fold's
  `(ts, host, path, line)` writer order. A later event in that order that
  re-adopts the key clears the report: a `theme.rename` whose `to` is the
  key, or a `theme.color` (pin or unpin) or `theme.unhide` naming the key.
  A later `theme.hide` does not re-adopt a name.

Runbook: `git log --oneline -3`, confirm the `bob themes rename: OLD → NEW`
commit, then `git revert <sha>` and `bob vault-sync` to restore every byte.

## PT11 — Split by entry

```text
bob themes split OLD (-m/--map FILE | -t/--template) [-b/--bob-dir DIR] [-d/--dry-run]
                     [-f/--format human|json] [-o/--offline]
```

Rewrite selected Pomodoro theme components in ` — NAME` tails of canonical
daily notes (past, today, and future, open and closed). `_templates/daily.md`
is out of scope. Exactly one of `-m` and `-t` is required. `-m -` reads the
map from stdin. OLD must be a known theme.

- **Template (`-t`):** read-only; never takes the lock. Lists every entry
  where some `+` component normalizes to OLD's key, ordered by date then
  line. Human output is TSV the map parser accepts unchanged: `#` comment
  lines naming the columns and the fill-in rule, then
  `DATE<TAB>REF<TAB>THEME<TAB>NOTE`. DATE is `YYYY-MM-DD`. REF is
  `LINE:DIGEST`. THEME is prefilled with OLD's display name (an untouched
  row is a keep). NOTE is `[x]`/`[ ]`, the time range or `()`, the full
  current name, and the Task Link targets from the entry's child lines,
  joined with `, `. Plain-text children with no link appear as their first
  60 characters. `-f json` emits `{schema_version: 1, ok: true,
  mode: "template", theme: {key, name}, entries: [{date, path, ref, line,
  state, time_range, name, links, notes}]}`.
- **Map (`-m FILE`):** rows are `DATE<TAB>REF<TAB>THEME[<TAB>NOTE]`. Blank
  lines and lines starting with `#` are skipped; NOTE is ignored. Refuse
  with `invalid_map` (a usage error), citing the map line number, when a
  row has fewer than three columns, the date is malformed or has no
  canonical daily note, the ref is malformed, or two rows resolve to the
  same entry. A blank THEME or one containing `+` is `invalid_name`.
  Resolve each row with `PomodoroScan::by_ref`. `Stale` or `Ambiguous`, or
  an entry with no OLD component, is `stale_entry` (hint: regenerate with
  `-t`). A row whose THEME key equals OLD's key is a keep. Otherwise
  replace the OLD component with THEME's trimmed spelling; separators keep
  their `+` spelling and a resulting duplicate collapses. OLD entries with
  no row are left alone and counted as `unmapped`. If every row is a keep,
  refuse with `unchanged`. THEME may not exist yet (`new` in the preview).
  Targets in normalized `plan.exempt` are `exempt`. An unconfigured THEME
  adds a `warnings` entry and human output marks it `◌ not configured`.
- **Writing:** `-d` prints the plan and changes nothing. A write runs
  through the shared vault-rewrite transaction (`command_name` `bob themes
  split`, worktree subject "theme splits"). Split writes **no journal
  event**. OLD keeps its hidden state and pin, and every target keeps its
  own. Note defaults and `[theme::]` / `(theme::)` task fields are left
  unchanged. When OLD still appears in either, the preview and result add a
  `split_leaves_task_themes` warning with those leftover counts. The commit
  subject is `bob themes split: OLD → T1, T2, …` with the
  changed target display names in alphabetical order. Verify: reread
  contents equal the planned after-image, line count unchanged, the set of
  changed lines equals the planned set, every changed line differs only in
  its name tail, each planned line now carries its planned name, and the
  scanned entry count is unchanged. Undo is `git revert`.
- **Refusals:** `unknown_theme`, `invalid_name`, `unchanged`, `invalid_map`,
  `stale_entry`, plus `lock_busy`, `dirty_paths`, `not_a_worktree`,
  `vault_mismatch`, `sync_failed`, `verify_failed`, and `stale_note`.

Runbook: `bob themes split JOB -t > job.tsv`, edit THEME, preview with
`bob themes split JOB -m job.tsv -d`, apply with `bob themes split JOB -m
job.tsv`. Undo with `git revert <sha>` and `bob vault-sync`.

## PT12 — Configured themes

A `themes:` block in the config file declares the Pomodoro theme vocabulary
and each theme's palette color. Bob reads the file and never writes it.

```yaml
themes:
  PLAN: blue
  BUILD: yellow
  READ: green
  CHORE: magenta
  WRITE: violet
  FIX: red
```

- **Shape.** A mapping from theme name to palette slot (`blue`, `orange`,
  `aqua`, `yellow`, `magenta`, `green`, `violet`, `red`; case-insensitive,
  trimmed), `auto`, or empty/null. `auto` and null both mean "configured,
  but bob picks the color" (pin, then exempt, then area, then hash). No hex
  values: every surface already has a light/dark pair and an xterm-256
  code for each slot.
- **"Configured" means** the block is present, valid, and has at least one
  entry. Missing, `null`, or `{}` means no configured themes, which is
  exactly today's behavior.
- **Validation** is strict, and a failure drops the whole block, so behavior
  is as if no themes were configured: `themes` must be a mapping; each key
  must be a YAML string (a number or boolean key fails with "quote theme
  names YAML reads as numbers or booleans"); each name must be a valid
  single theme (not blank, no `+`); two names with the same normalized key
  fail, naming both spellings; a value that is not a slot, `auto`, or null
  fails, listing the valid colors. Every message names the config path and
  the offending `themes.NAME`.
- **Where an invalid block is reported.** Never fatal: `bob themes list`
  carries it as a `warnings` entry (yellow in human output), `bob plan`
  reports the `themes_config_invalid` lint, and `bob capture` prints a
  plain warning string. `bob themes list` never exits 2 over it.
- **Identity and display.** A configured theme's key is its normalized
  name. The history catalog itself does not change: it is still derived
  only from daily notes plus the journal. Reports combine the catalog with
  the configured set. A configured theme with no history displays its
  config spelling; otherwise the most recently authored spelling wins.
- **Configured flag and `known`.** When themes are configured, every
  `ThemeRef` and every `themes list` row carries
  `configured: true|false`. When none are configured, the field is absent.
  `known` is also true for configured themes with no history. Inventory
  labels are storage, not themes: they never warn, are never muted as
  unconfigured, and report `configured: true`. `(unnamed)` is not a theme.
- **Warning code.** One code everywhere: `theme_not_configured`
  (`SNACK isn't one of your configured themes`, plus `; did you mean
  CHORE?` when the similarity rule matches a configured theme). Warnings
  never block anything; there is no strict mode. Surfaces: theme-name
  menus scope Recent rows to configured themes (never-used configured
  themes rank after used ones in config order, the empty-query cap rises
  from 5 to 12, `similar_to` draws only from visible configured themes,
  create rows warn), `bob capture` (structured top-level warnings),
  `capture-pomodoro-name` (Name-it), Obsidian pickers that write names,
  `bob themes rename`/`split`, `bob plan` (`theme_not_configured` lint),
  pills and swatches everywhere, and `bob themes list` hints.
- **`bob themes list`.** The default view always includes visible configured
  themes, even never-used ones (`first_used: null`, `last_used: null`,
  `days_used: 0`, `entries: 0`, no `today`; human output shows `never`
  and `0 days`, sorted last). The header reads
  `6 configured · 76 known · …`. Human meta labels read `blue · config`
  for config colors, today's labels (`aqua`, `red · pinned`,
  `muted · exempt`, `aqua · area`) for configured `auto` themes, and
  `muted · not configured` with the `◌` swatch for the rest. JSON adds
  `counts.configured`, top-level `configured_themes` (`[{key, name,
  color}]`, color a slot or null, in config order), per-row `configured`,
  and `hue_source` values `config` and `unconfigured`.
  `theme_hue_collision` skips config-colored and muted themes.
  `theme_not_configured` hints cover visible unconfigured, non-inventory
  themes used today or in the last 7 days, one JSON object per theme
  (`{code, theme: {key, name}, last_used, days_used, similar_to?}`). Human
  output prints at most 3 of those hint lines, then
  `… and N more · bob themes -a`.
- **`bob themes color`.** Pinning a theme whose config sets a color refuses
  with `config_color` (exit 1, no journal write):
  `BUILD's color comes from themes: in ~/.config/bob/config.yml (yellow)`,
  hint `edit themes.BUILD there, or set it to auto to pin colors with bob
  themes color`. Configured `auto` themes pin normally.

## PT13 — Task theme suggestions

Rust suggests one Pomodoro theme per task and remembers deliberate
exceptions as task fields. `bob themes suggest` explains the suggestion;
nothing else in this phase writes. Capture pickers and commits, listing
rows, and other clients follow this same contract in later phases; this
section does not claim they ship yet.

### Note defaults

An area or project note declares its default with scalar frontmatter:

```yaml
theme: WRITE
```

Quotes are honored. An absent or blank value inherits from further up the
residence chain. ASCII-case-insensitive `off` stops the walk, but only
when declared by an area or project. Lists (flow and multiline),
composites, mappings, and bad names are invalid: skipped, with one
`theme_default_invalid` warning per visited declaring note. A `theme:`
value on any other note kind is ignored, while that note's scalar
`parent:` links still carry the walk. Daily notes have no default walk.
The walk reuses the R1 residence chain with its 16-note and cycle
limits.

### Task fields

A task line carries none, one set field, or repair state:

- **Set:** exactly one valid field, `[theme:: X]` or `(theme:: X)`.
- **Repair:** two or more fields mean `duplicate`; one invalid field
  means `invalid`. Repair falls through to inheritance, carries
  `repair: "duplicate" | "invalid"` whenever a suggestion exists, and is
  replaced by the next eligible writer.
- Closed tasks keep their field as history and have no suggestion.

A no-suggestion result still returns warnings separately; no repair
marker is fabricated to carry them.

### Precedence and inheritance

First match wins:

1. **Task field.** Source `task`, label `this task`.
2. **Reading task.** An exact `#ref` token, or a trailing block ID `ref`
   or `ref-*` (rereads included). Ordinary annotation follow-up tasks do
   not qualify. Uses `plan.reading_theme` (default `READ`); `off` skips
   this step. Source `reading`, label `reading task`.
3. **Daily review.** The canonical daily residence plus a
   case-insensitive trailing `gtd` block ID, suggesting the first
   `plan.exempt` name. An empty exempt list skips this step. Source
   `review`, label `daily review`.
4. **Note default.** The first valid default on the residence chain.
   Source `note`, label `from <lowercase declaring stem>`.
5. Otherwise none: `No suggested theme`.

The inherited suggestion runs the same rule with step 1 skipped. The
serialized suggestion holds `theme` (a `ThemeRef`), `source`, and
`label`. Only source `note` adds
`from: {path, stem, kind, hue, chain}`: vault-relative path, chain from
residence through declaring note, and the declaring note's root-area hue.
Only source `task` adds `inherited`, serialized as a suggestion without
a further `inherited` member, or explicit null. Other optional fields
are omitted.

### Known, unknown, and hidden

A theme is known when the catalog knows it or when it appears among
today's live ledger components, even when the catalog cache is stale.
Unknown names stay reportable (`known: false`, `◌` swatch); downstream
menus must never preselect them. Hidden themes remain eligible.
Theme defaults never affect hue, history, or Stats.

### `themes suggest`

`bob themes suggest TARGET [-b/--bob-dir DIR] [-f/--format human|json]`.
`route:block-id` explains an existing task (capture-route validation,
no path traversal, one-based line); a bare route explains a new task's
default. Report closed tasks successfully with no suggestion and one
`closed_task` warning. Exit 0 explains (including no suggestion or a
closed task); 1 is a target lookup or I/O failure; 2 is usage or
config. The vault is never written; byte-identical before and after.

JSON is
`{schema_version: 1, ok, target: {route, block_id, path, line, new_task}, theme_suggestion: <object or null>, warnings}`.
Null block ID and line mark new tasks. Warnings are `closed_task` and
`theme_default_invalid` (with the declaring `path`).

Human output shows the swatch and name plus `suggested · <label>`;
note defaults add the declaring path and chain, and task overrides add
the inherited reason:

```text
■ WRITE  suggested · from sase
  declaring note: sase.md
  chain: sase.md
```

Task overrides render `without it: ■ WRITE from sase_blog` (or the
equivalent inherited reason, or `without it: none (no default here)`).
Warnings render as `warning: <code>` plus ` · <path>` when present.
Color follows NO_COLOR; broken pipes exit silently.

### Placement

`set_task_theme` removes every theme field in either form plus one
adjacent separating space, then inserts ` [theme:: NAME]` before the
maximal trailing run of inline fields of any key, tags, and a trailing
`^id`. With no run it inserts at the trimmed end. `clear_task_theme`
only removes the fields and the adjacent separator. Both preserve every
other byte: indentation, blockquote prefixes, tabs, trailing whitespace,
CRLF, and absence of a final newline. The checkbox and title are never
consumed as trailing metadata, and overlapping removals from adjacent
fields stay safe. Catalog spelling is used for X; a semantic no-op
(exactly one valid field already holding X's key) never rewrites bytes.

### The write rule and never-writers

A deliberate choice for destination name D with shown suggestion S:

1. Not deliberate: no change.
2. D unnamed, all-exempt, or holding other than exactly one non-exempt
   component: no change. `PLAN + BUILD` selects BUILD; `BUILD + FIX`
   does not write.
3. No suggestion was shown: no change. The explicit Task Card edit
   escape hatch stays separate, where this guard does not apply.
4. Chosen key equals inherited: clear every theme field, no-op if
   absent.
5. Otherwise set catalog-spelled X, no-op when already exact.

Deliberate capture means an explicit `#NAME` or equivalent `=name`;
the picker means a changed committed row, a nonempty query, or
Shift+Enter. Alt+Enter is always link once; untouched preselection is
not deliberate. Guards refuse before any field edit. Theme edits never
affect freshness eligibility.

No existing gesture becomes a writer in this phase. In the full
contract, close, roll, typed `^^`, move, rollover, reconcile, ordinary
successor placement, and ref-today are never writers.

### Preservation

Task move, archive, Highlights reading-status checkbox edits, ref
rename's alias overwrite, task-status-hooks reconcile, and freshness
stamping keep a distinctive authored field (either form, any spacing)
byte-identical: only each operation's intended bytes change. Freshness
stamps land after the theme, still before the fresh/Tasks suffix.
Closed and history fields are preserved, not stripped.

### Rename coverage

`bob themes rename` rewrites matching note defaults and task fields in the
same previewed, verified commit as the session tails. Invalid defaults,
`off`, non-area/project `theme:` lines, fenced examples, and prose stay
untouched. `bob themes split` never rewrites defaults or fields.

### List hints: unknown defaults

`theme_default_unknown` fires once per typed area/project note whose
valid default is unknown to the current catalog:
`{code: "theme_default_unknown", path, name, similar_to}`. Human
output renders
`<path> suggests unknown theme <NAME>[; did you mean <X>?] → edit theme: in that note or add <NAME> under themes: in <config>`.
No hint for absent, blank, `off`, invalid, ignored note kinds, or known
defaults. Collision hints keep their exact bytes and shape; hint order
is deterministic. A `--query`, `--limit`, or hidden filter never hides
this vault-wide diagnostic. The scan follows project discovery
exclusions, never writes, and knownness re-evaluates from the current
catalog and config, so new ledger use or vocabulary changes clear hints
without rewriting defaults.

### The shared menu contract

Menus in every client follow this rule over the suggested theme S.
Exact entries contain only S after removing exempt keys other than S.
Prefer a running exact entry, else the first exact entry in ledger
order, else the first containing entry; keep Today order. Otherwise a
known S may create a top `New S Pomodoro` row when a Pomodoros section
exists, at most one timed open entry exists, and strict theme caps
permit it; remove its Recent duplicate. Otherwise preserve today's
default and report `unknown_theme`, `creation_blocked`, or
`strict_cap`; ref-today additionally uses `already_queued`. A nonempty
query keeps PT7 ranking and first-row behavior while annotating at most
one best matching row. Ctrl+D on suggested creation is consumed without
writing. The later completion phase owns implementation and the
`preselect` vectors.

Pinned copy: `suggested · this task`, `suggested · reading task`,
`suggested · daily review`, `suggested · from <stem>` (root-area dot
where supported); `reading tasks suggest READ`,
`daily review suggests PLAN`, `<stem> suggests WRITE`, and
`no default here`. Consequences: `this task becomes FIX · <stem>
suggests WRITE` and `task theme cleared · <stem> suggests WRITE`;
notices: `Linked under FIX · task theme FIX` and `Linked under WRITE ·
task theme cleared`. Accessible example: `Pomodoro WRITE, suggested,
from sase_blog`. Explicit task fields alone get PT6 pills; inherited
values never become ghost pills. Unknown and repair styling follows the
established pill rules.

The later capture addition is
`task_theme: {action: "set" | "cleared", theme, previous, inherited}`,
omitted on no change. Later listing rows add `suggested` and
`suggestion_label`; suggestions may gain `fallback`, and ref-today
`create`, without a schema bump.

### Dataview caveat

`task.theme` can conflate frontmatter note defaults with task fields,
so it cannot answer what a task itself declares. Use
`bob themes suggest` for the authoritative suggestion.

## JSON contract

Additive; `schema_version` stays 1.

**`ThemeRef`:** `{key, name, hue, hue_source}`, plus `known: false` only for
names the catalog does not know, plus `configured: true|false` when a
`themes:` vocabulary is configured (absent otherwise). Where offsets matter,
it also carries `start` / `end` byte offsets into the draft.
`capture-parse` attaches `themes` to each Pomodoro-name span; offsets
identify component text and exclude quotes and `+`. `capture-pomodoros`
attaches `themes` to every entry. `bob plan` returns `theme_refs` beside
`theme_names` and `themes` on each entry; `bob capture` returns
`added_theme_refs` beside `added_themes`; task reconcile uses the same
plan-budget fields. These arrays preserve source order. `hue_source` is
one of `config`, `pin`, `exempt`, `unconfigured`, `area`, `hash`.

Stats reports include `theme_hues` beside `homes`, keyed by normalized theme
key, for theme-dimension values present in the report. Exempt themes have
`hue: null` and `hue_source: "exempt"`. Unknown names carry `known: false`;
while typing, clients keep them neutral.

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
  "hints": [
    {
      "code": "theme_hue_collision",
      "themes": ["SASE", "BLOG"],
      "hue": "orange",
      "days_together": 6,
      "suggest": { "name": "BLOG", "hue": "red" }
    },
    {
      "code": "theme_not_configured",
      "theme": { "key": "snack", "name": "SNACK" },
      "last_used": "2026-10-27",
      "days_used": 1,
      "similar_to": "CHORE"
    }
  ],
  "renamed_reappeared": [],
  "warnings": []
}
```

`theme_hue_collision` pairs are unpinned, non-exempt themes that share a hue
and met on at least 2 of the last 30 days. The theme with fewer `days_used`
is the one to move. The suggested slot is the least used by the last 30
days' themes, excluding hues of themes that met the moved theme in that
window; ties follow palette order. `renamed_reappeared` lists every rename
source key that still has occurrences after its latest rename-from event
and that a later rename-to, color, or unhide has not re-adopted, as
`{key, name, renamed_to, renamed_to_name}`.

**Writers** (`hide`, `unhide`, `color`, `rename`, `split`) emit
`{schema_version, ok, ...}` on success and `{ok:false, code, message, hint?}`
on refusal, even on a nonzero exit. Refusal codes: `unknown_theme`,
`invalid_hue`, `invalid_name`, `unchanged` (no journal write), `theme_exists`,
`config_reference`, `invalid_map`, `stale_entry`, `lock_busy`, `dirty_paths`,
`not_a_worktree`, `vault_mismatch`, `sync_failed`, `verify_failed`,
`stale_note`, plus `config_color` for `color` on a theme whose color comes
from the `themes:` block. The Obsidian manager previews with `rename -d -f
json` (files, entries, before/after tails, state and color change) and
confirms with `--merge` when needed.

## `bob themes` CLI

The group is `bob themes`, a top-level catalog noun in Daily workflow after
`stats`. Bare `bob themes` is `list`. Members, alphabetical in help:

- `color NAME HUE|auto`
- `hide NAME…`
- `list [-a/--all] [-F/--fresh] [-f/--format human|json] [-H/--hidden] [-n/--limit N] [-q/--query Q]`
- `rename OLD NEW [-d/--dry-run] [-f/--format human|json] [-m/--merge] [-o/--offline]`
- `split OLD (-m/--map FILE | -t/--template) [-d/--dry-run] [-f/--format human|json] [-o/--offline]`
- `suggest TARGET [-b/--bob-dir DIR] [-f/--format human|json]`
- `unhide NAME…`

List defaults: visible themes used in the last 90 days or today, newest
first, plus every visible configured theme even when never used (sorted
last, showing `never` and `0 days`). `-a` shows every theme and flags
hidden ones. `-H` shows only hidden themes. `-q` ranks per PT7 groups B
then D.

Human list:

```text
THEMES  81 known · 4 today · 2 hidden · last 90 days
  ■ BOB        today     26 days   magenta
  ■ JOB        Oct 23    24 days   aqua · area
  ■ BLOG       Oct 23    14 days   red · pinned
  □ PLAN       today     51 days   muted · exempt
  hint  SASE and BLOG share orange and met on 6 days → bob themes color BLOG red
```

With a vocabulary configured, the header reads
`6 configured · 81 known · …`, configured rows show `blue · config`, and
unconfigured rows show `◌ SNACK … muted · not configured`.

`color` on a theme whose config sets a color refuses with `config_color`
and writes nothing to the journal.

## Shared vectors

`tests/fixtures/pomodoro_themes/vectors.json` is the single source. Sections:
`components`, `hue` (including the `areas` and `configured` inputs), `fold`,
`rank`, `similar`, `menu`, `literal`, `suggest`, `task_field`, and
`write_rule`. CLI tests consume the menu and literal sections, including
capture dry-run round trips; Rust tests consume all three new sections,
including mini-vault suggest cases with expected JSON and warnings.
bob-plugins copies the file byte-for-byte and runs the same hue, fold, and
ranking vectors in JavaScript; the suggest vectors are reserved for the
later JavaScript mirror.

## Related

- [Plan budget](plan.md#themes)
- [Daily Stats](stats.md)
- [Pomodoro homes](pomodoro-homes.md)
- [Vault Git sync](vault-git-sync.md)
- [Capture](capture.md)
