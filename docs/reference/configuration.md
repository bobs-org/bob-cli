---
title: Bob configuration and environment
description: Configuration file precedence, supported YAML settings, environment variables, and runtime requirements for bob.
---

# Bob configuration and environment

This page covers configuration read by the Rust `bob` CLI. Obsidian plugin
settings have separate owners and are described in the [Obsidian plugins
reference](plugins/index.md); Bob CLI does not validate plugin settings.

## Configuration file

The CLI reads one YAML file. A nonempty `BOB_CONFIG_FILE` selects the exact
file. Otherwise, a nonempty `XDG_CONFIG_HOME` selects
`$XDG_CONFIG_HOME/bob/config.yml`; the final default is
`$HOME/.config/bob/config.yml` (shown as `~/.config/bob/config.yml`). Empty
environment values count as unset. Bob expands a leading `~` or `~/` in the
selected file path and in `XDG_CONFIG_HOME`; it does not expand `~someone`.

```yaml
timezone: America/New_York

properties:
  - name: scheduled
    values: date
  - name: dependsOn
    values: local_task_id
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 2
        max_days: 7

plan:
  max_themes: 3
  max_links: 10
  link_unblocked: true
  default_type: CHORE

types:
  PLAN: blue
  BUILD: yellow
```

Missing configuration files use defaults for optional sections. Priority
windows are required by `$<N>` priority rolls and `bob task reroll`; commands
that need them report a setup error when the `priority` property is absent.

## Supported keys

### `timezone`

| Key | Type and rules | Default or effect |
| --- | --- | --- |
| `timezone` | IANA timezone name, such as `America/New_York` | Unset uses the host's system timezone. When set, Bob uses this timezone for its current date and time, daily-note selection, and date stamps. Invalid names are configuration errors. |

A `BOB_NOW` or `DATE` date/time keeps precedence over the configured
timezone. A `DATE` command, such as `DATE=date`, runs with `TZ` set to the
configured timezone. The key makes Bob's clock consistent across hosts without
requiring each host's system timezone to match. See the
[plan contract](../plan.md).

### `properties` and priority windows

`properties` is a sequence of property descriptions. The CLI locates the entry
whose `values` scalar is exactly `priority`; other entries describe names and
value kinds consumed by Bob integrations.

| Key | Type and rules | Default or effect |
| --- | --- | --- |
| `properties` | Sequence of mappings | No priority fallback; priority rolls require an entry with `values: priority`. |
| `name` | String | Property name, commonly `priority`; required in practice for useful metadata. |
| `values` | Scalar | Set to `priority` on the priority entry. Other values, such as `date` and `local_task_id`, declare the property kind. |
| `schedules` | String | The priority entry must use `scheduled`. |
| `levels` | Nonempty sequence | Ordered priority levels. |
| `levels[].label` | Nonempty string; labels must be unique ignoring case | Display label such as `P1`. |
| `levels[].value` | Nonempty scalar string, number, or boolean; values must be unique | Value written in the task's priority property. It cannot contain brackets, `::`, or a newline. |
| `levels[].min_days`, `max_days` | Nonnegative integers; minimum cannot exceed maximum | Inclusive offset window used to roll a scheduled date. |

Each level's inclusive day window determines how far a chosen priority rolls
its `scheduled` date. `BOB_PRIORITY_ROLL_SEED` can make a roll repeatable. See
[priority and schedule rolls](../plan.md#config) and
[`bob task reroll`](cli/task.md).

### `capture`

| Key | Type and allowed value | Default | Effect |
| --- | --- | --- | --- |
| `capture.max_saved_templates` | Integer, 0–36 | `36` | Maximum saved `@@template:` blocks; `0` disables saving. |

See [capture templates](../capture.md#saved-templates).

### `plan`

All caps are positive integers. `max_ready_per_note` is limited to 1–999;
the other numeric caps use positive 32-bit integers. The lists must contain
nonempty strings. Invalid `plan` settings cause `bob plan` and `bob ready` to
exit 2; capture and hook surfaces warn and use their established fallback.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `plan.max_themes` | Positive integer | `3` | Maximum distinct non-exempt Task types before the plan reports over budget. |
| `plan.max_links` | Positive integer | `10` | Maximum distinct Task Links in today's open Pomodoros before the plan reports over budget. |
| `plan.max_next` | Positive integer | `15` | NEXT lane count cap. It is reported; it does not refuse work. |
| `plan.max_pending` | Positive integer | `10` | PENDING lane count cap. It is reported; it does not refuse work. |
| `plan.max_ready` | Positive integer | `100` | Dashboard READY backlog cap. |
| `plan.max_ready_per_note` | Integer, 1–999 | `5` | Soft cap for the Ready lane in each area or project note; note frontmatter may override it with `ready_cap`. |
| `plan.strict` | Boolean | `false` | When true, refuse a `#NAME` capture that would add a theme past `max_themes`. |
| `plan.link_unblocked` | Boolean | `true` | Offer recovered dependents as successor choices after a managed close; false recovers without offering links. |
| `plan.exempt` | Sequence of nonempty strings | `[PLAN]` | Themes excluded from theme and link budgets. |
| `plan.inventory_labels` | Sequence of nonempty strings | `[LATER, MISC, NEW FEATURES, SASE]` | Labels that produce an inventory warning when used as open Task types. |
| `plan.default_type` | Single task type name, `off`, or `null` | Unset | Last type suggestion after the task field, reading rule, and note defaults; `off` and `null` disable it. A name outside `types:` still applies and produces a `default_type_not_configured` lint in `bob plan`. |
| `plan.reading_type` | Single task type name or `off` | `READ` | Type suggested for reading tasks; `off` disables the rule. `plan.reading_theme` remains a legacy alias. Setting both is a config error. |

See the [plan configuration contract](../plan.md#config) and
[dependency successor offers](../task-dependencies.md#12-successor-offers).

### `types` (legacy alias: `themes`)

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `types` | Mapping from task type name to palette slot, `auto`, or empty | Unset (no configured types) | Declares the task type vocabulary and each type's color. `themes` remains a legacy alias with the same shape. |

Each key must be a valid single type name (YAML strings only; quote names
YAML reads as numbers or booleans). Each value is a palette slot (`blue`,
`orange`, `aqua`, `yellow`, `magenta`, `green`, `violet`, `red`,
case-insensitive), `auto`, or empty/null. `auto` and null mean configured
with no fixed color: the hue falls through to pin, exempt, area, then hash.
A value of anything else, a duplicate normalized name, or a non-mapping
`types` / `themes` block is invalid. Configuring both keys is also invalid;
the whole vocabulary is ignored as if no types were configured (`bob types list` carries the warning; `bob plan` and
`bob capture` surface it on their own). Colors win over `bob types color`
pins: coloring a configured type refuses with `config_color`.

See [task type suggestions](../task-types.md#pt12-type-vocabulary-and-display).

### `dependencies`

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `dependencies.blocks_lines` | Boolean | `true` | Controls display of generated BLOCKS mirror lines for open dependents. The Depends-On line remains the source of dependency truth. |

See [task dependency links](../task-dependencies.md#14-blocks-lines).

### `relations`

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `relations.linked_from_lines` | Boolean | `true` | Write `🪢 LINKED FROM` mirrors ([task relations](../task-relations.md)). `false` removes every LINKED FROM line on writable tasks, mints nothing, and leaves LINKS alone. |
| `relations.linked_from_mentions` | Boolean | `true` | Write inert `(💬 mentions)` entries on LINKED FROM for block links in a task's own text ([text mentions](../task-relations.md#19-text-mentions)). `false` removes only those entries on writable tasks, deleting a line left empty, and mints nothing for mentions. |
| `relations.cancel_labels` | List of relation labels | `[]` | Committing a link with one of these labels also cancels the source task ([cancel labels](../task-relations.md#17-cancel-labels)). Matched by `relation_label_key` (case, spacing, and a trailing colon ignored). Empty cancels nothing. |
| `relations.complete_labels` | List of relation labels | `[]` | Committing a link with one of these labels also completes (marks Done) the source task ([complete labels](../task-relations.md#18-complete-labels)). Matched like `cancel_labels`. Empty completes nothing. |

An invalid `relations.linked_from_lines` value falls back to `true` and
warns. An invalid `relations.linked_from_mentions` value falls back to
`true` and warns, in the same wording pattern. See
[Task relations](../task-relations.md#12-kill-switch-outputs-and-warnings).

`relations.cancel_labels` fails toward fewer cancels: a missing key,
`null`, or `[]` is a valid empty policy; a non-list value warns and
cancels nothing; an illegal or non-string entry warns and is skipped;
duplicate keys collapse. A `cancel_labels` problem never becomes a
config error and never changes `linked_from_lines`. Hide is not
disable: a hidden or one-off cancel label still cancels when typed.
Chezmoi enables `duplicate of` and `superseded by` (see
[Cancel labels](../task-relations.md#17-cancel-labels)).

`relations.complete_labels` parses the same way, failing toward fewer
completes, and never becomes a config error. A key listed in both
`cancel_labels` and `complete_labels` is dropped from both lists and
warns once:
`relations: “{label}” is in both cancel_labels and complete_labels in <path>; it closes nothing`.
Hide is not disable: a hidden or one-off complete label still
completes when typed. Chezmoi enables `implemented by` and
`verified by` at rollout (see
[Complete labels](../task-relations.md#18-complete-labels)).

### `freshness`

Intervals are integer days from 1 through 365. The two lane intervals also
accept `false` to turn that lane's daily review walk off. `project_interval`
accepts an interval or `null` to inherit the Ready cadence.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `freshness.interval` | Integer, 1–365 | `7` | Ready task confirmation cadence. |
| `freshness.pending_interval` | Integer, 1–365, or `false` | `1` | Review cadence for In Progress (`[/]`) tasks; false disables that lane's walk. |
| `freshness.next_interval` | Integer, 1–365, or `false` | `1` | Review cadence for Next (`[*]`) tasks; false disables that lane's walk. |
| `freshness.project_interval` | Integer, 1–365, or `null` | `null` | Explicit cadence for `^prj` tasks; null inherits the existing Ready chain. |
| `freshness.rotten_daily_budget` | Positive integer or `null` | `null` (off) | Optional daily allowance for Rotten keep decisions; it never hides tasks. |
| `freshness.decay` | Mapping, `true`, `false`, or `null` | Enabled; 3 keeps | Approved-decay settings for repeatedly kept due Ready tasks. |
| `freshness.decay.keeps` | Integer, 0–999 | `3` | Keeps before a decision is offered; `0` offers on each due Ready re-confirmation. |
| `freshness.decay.enter` | Nonempty priority label or `null` | `null` | Priority value to enter on an approved decay decision; null uses the interval-aware entry. |

`decay: false` disables asking while retaining keep counts and display. The
removed `stale_daily_budget` key is accepted only as a deprecated input for a
transition period; use `rotten_daily_budget`. The freshness consumer has its
own parser, so its validation and defaults are independent of the other
sections. See [freshness fields and configuration](../freshness.md#2-fields-overrides-and-config).

### `highlights`

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `highlights.pre_scan_hook` | String command | Unset | Command run by `bob ref scan` before scanning; `BOB_HIGHLIGHTS_PRE_SCAN_HOOK` overrides it. `bob ref scan --no-hooks` skips it. |
| `highlights.audio_link_template` | String template | `obsidian://open?vault={vault}&file={path}` | URI used for the PDF's audio button. `{vault}` and `{path}` are percent-encoded; an empty string disables the button. |
| `highlights.audio_library` | Path | `$XDG_DATA_HOME/sase-listen/library`, else `~/.local/share/sase-listen/library` | SASE Listen library searched for companion audio. |
| `highlights.listen_command` | String command template | Unset | Optional `bob ref create --listen` command. It must be a direct `sase listen render {target} ... -o {audio}` invocation; Bob does not run a shell. |
| `highlights.ingest_on_create` | Boolean | `true` | Whether create-time intake files the PDF and reference note immediately when safe. `BOB_HIGHLIGHTS_INGEST_ON_CREATE` overrides it. |
| `highlights.coordinate_pdf_writes` | Boolean | `false` | When on, wrap each PDF marker write in a bounded macOS `.forMerging` coordination and hold when Highlights is in use or uncertain. Stays off until the Mac acceptance test passes. On Linux the key is accepted and inert for OS coordination. See [PDFs open in Highlights](../highlights-ref-sync.md#pdfs-open-in-highlights). |
| `highlights.table_layout` | `auto`, `records`, or `grid` | `auto` | Global table default for `bob ref create` PDFs: prose tables print as single-column records, other tables stay grids. Per-table `bob-records`/`bob-grid` Divs and frontmatter `bob-tables` override it. See [Tables](../highlights-create.md#tables). |
| `highlights.table_prose_words` | Integer 1–200 | `12` | Word threshold for the auto prose rule: a table is prose when some body-row cell in column 2 or later holds at least this many words. See [Tables](../highlights-create.md#tables). |
| `highlights.url_routing.capture` | Boolean | `true` | Whether eligible bare links in capture queue a reference job. |
| `highlights.url_routing.gkeep` | Boolean | `true` | Whether eligible URL-only Keep notes are clipped during pull. |
| `highlights.url_routing.exclude_hosts` | Sequence of host strings | `[google.com, googleplex.com, youtube.com, youtu.be, github.com, x.com, twitter.com]` | Replaces the built-in exclusions; an empty list excludes none. Entries are normalized and match a host and its subdomains. |
| `highlights.annotation_task_relation` | String relation label | Unset (off) | When set, each task newly created from a Highlights `#task` annotation gains a 🪢 LINKS child pointing at its reference's reading task with this label. An invalid label fails scans that create annotation tasks. See [Relation to the reading task](../highlights-ref-sync.md#relation-to-the-reading-task). |

The old `highlights.pre_scan_command` key is rejected; use
`pre_scan_hook`. Invalid URL-routing config warns and leaves the URL as a task.
See [URL routing](../ref.md#url-routing),
[reference audio](../highlights-ref-sync.md#companion-audio-on-scan),
[PDFs open in Highlights](../highlights-ref-sync.md#pdfs-open-in-highlights), and
[web article capture](../highlights-clip.md#hosts-and-browsers).

### `gkeep`

| Key | Type and rules | Default | Effect |
| --- | --- | --- | --- |
| `gkeep.email` | String containing `@` | Required by commands that connect to Keep | Account identity; `bob gkeep login -e` can override it for that invocation. |
| `gkeep.token_command` | String command | `pass show gkeep/master_token` | Prints the Keep master token. |
| `gkeep.token_store_command` | String command | `pass insert -m -f gkeep/master_token` | Stores the token read by `bob gkeep login`. |
| `gkeep.device_id` | 1–16 hexadecimal digits | Derived from the configured email | Stable Keep device identifier. |
| `gkeep.target` | Vault-relative `.md` path with no parent-directory component | `gkeep_inbox.md` | Destination for imported tasks. |
| `gkeep.timeout_secs` | Positive integer | `300` | Adapter request timeout in seconds. |

The adapter can be replaced with `BOB_GKEEP_ADAPTER`. See the
[Google Keep contract](../gkeep.md#configuration).

### `gcal`

| Key | Type and rules | Default | Effect |
| --- | --- | --- | --- |
| `gcal.account` | String containing `@` | Optional when the isolated `bob-gcal` bucket holds exactly one unique email, else when gog as a whole holds exactly one | Account identity; `bob gcal login -a` can override it for that invocation. Commands prefer the Bob bucket before gog's default accounts. |
| `gcal.writer` | Hostname slug | Unset (this host may write) | The one host label allowed to sync; `doctor` compares it to vault-sync's hostname slug. |
| `gcal.root` | Bare note name | `meet` | Default `[root::]` for Event Tasks. |
| `gcal.calendars` | Sequence of ids or `{id, root?}` maps | `[primary]` | Calendars to sync; `primary` matches the account's primary calendar. |
| `gcal.colors` | Mapping of Google event colour names or ids (`lavender`/`1` through `tomato`/`11`) to roots | `{}` | Event-colour root source, checked after an `@route` token and before the calendar's root. Unknown colour keys are a setup error. |
| `gcal.tentative` | `context` or `task` | `context` | Whether a busy Maybe event becomes a context row or an Event Task. |
| `gcal.timeout_secs` | Integer greater than 0 | `60` | gog request timeout in seconds. |

A missing `gcal:` block uses those defaults. Unknown keys are ignored. A
wrong-typed value is a setup error (exit 2) that names the key and never
breaks other config loaders. `BOB_GCAL_GOG` replaces `gog` on `PATH`. See the
[Google Calendar contract](../gcal.md#configuration).

### `stats`

| Key | Type and allowed values | Default | Effect |
| --- | --- | --- | --- |
| `stats.range` | Range string: `7d`, `4w`, `3m`, `1y`, `Nd/Nw/Nm/Ny`, `all`, or inclusive `YYYY-MM-DD..YYYY-MM-DD` | `4w` | Default report window. |
| `stats.overview` | Sequence of nonempty metric IDs | `[worked, reviewed, done, created, net, review_closed]` | Metric IDs shown in the overview; repeated IDs are deduplicated. |
| `stats.hide` | Sequence of nonempty metric IDs | `[]` | Hides metrics from lists and default selection, without disabling their evaluation. |
| `stats.since` | ISO date `YYYY-MM-DD` | Unset | Global lower bound for inferred metric start dates. |
| `stats.metrics` | Mapping from ID to metric definition | `{}` | Adds or replaces a metric; supported kinds and fields follow the nested table below. |

Each `stats.metrics.<id>` entry uses `label` plus `kind`. The supported kinds
are `pomodoro`, `tasks`, `checklist`, `field`, `journal`, and `formula`.
`pomodoro` entries accept `measure` and `in`; task entries accept `event` and
`task_kind`; checklist entries accept `tags` and `measure`; field entries
accept `field`, `source`, and `unit`; journal entries accept `select` and
`tiers`; formulas accept `expr` and `unit`. Kind-specific requirements and
allowed values are defined in the [stats configuration contract](../stats.md#stats-config).

Metric definitions are checked individually. A malformed definition warns and
does not prevent unrelated metrics from loading. Unknown top-level keys are
ignored by the current section parsers.

## `BOB_*` environment variables

Unset or empty values use the listed fallback unless a row says otherwise.
Path values support a leading `~` only where noted. Command overrides below
are executable names or paths; command strings split on whitespace and do not
interpret shell quotes unless their contract says otherwise.

### Vault, clock, and state

| Variable | Default or unset behavior | Effect |
| --- | --- | --- |
| `BOB_DIR` | `$HOME/bob` | Default vault root when `--bob-dir` is omitted; a leading `~` is expanded. |
| `BOB_CONFIG_FILE` | `XDG_CONFIG_HOME/bob/config.yml`, else `~/.config/bob/config.yml` | Exact config file; empty means unset; leading `~` and `~/` expand. |
| `BOB_DAY_FILE` | `<BOB_DIR>/YYYY/YYYYMMDD.md` for the selected day | Exact daily note path for commands that read the Pomodoro ledger. The configured `timezone` determines the selected day when set. |
| `BOB_NOW` | Current time in configured `timezone`, else the host's system timezone | Valid `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, `YYYY-MM-DD HH:MM:SS`, or `T`-separated timestamp pins Bob's clock. Invalid text is ignored. |
| `BOB_PRIORITY_ROLL_SEED` | Fresh time/process-derived seed | A nonnegative integer seed makes `$<N>` priority rolls reproducible. |
| `BOB_CAPTURE_TEMPLATES_FILE` | `$XDG_STATE_HOME/bob-cli/capture/templates.json`, else `$HOME/.local/state/bob-cli/capture/templates.json` | Exact saved-template shelf file. |
| `BOB_VAULT_SYNC_LOCK_FILE` | `$XDG_RUNTIME_DIR/bob_sync.lock` when that directory exists, otherwise `/tmp/bob_sync.lock` | Shared maintenance lock path for vault sync, nightly maintenance, task reconcile, task reroll, and theme/relation-label journal writes and renames. |
| `BOB_VAULT_SYNC_NOTIFY` | Desktop notification on each quarantined clobber | Exact value `0`, `false`, or `no` suppresses the vault-sync clobber notification (`osascript` on macOS, `notify-send` on Linux). |
| `BOB_VAULT_SYNC_STATE_FILE` | `$XDG_STATE_HOME/bob-cli/vault-sync.json`, else `$HOME/.local/state/bob-cli/vault-sync.json` | Vault-sync status JSON path. |
| `BOB_CLI_USE_SCRIPT` | Native Rust path | Exact value `1` selects an embedded shell compatibility implementation where supported. See [compatibility shims](compatibility.md#compatibility-shims). |
| `BOB_PROGRESS` | Progress only on a TTY stderr | Exact value `1` prints line-oriented progress on stderr even when it is not a TTY. Progress never uses `\r` redraws on a non-TTY. Native sites: `bob ref create` fetch/capture/compose lines and `bob gkeep` spinner labels. Compatibility shims have no progress redraws to adapt. |
| `BOB_RUN_RECEIPT` | No receipt | Path of a JSON file bob writes after it chooses an exit code, including the quiet-EPIPE 0: `{"schema_version":1,"exit_code":N,"finished_at":"<RFC3339>"}`. The write is a temp file in the same directory, then a rename. A failed write prints one stderr warning and leaves the exit code unchanged. Panic and signal paths write no receipt. Every command is covered, including script-backed ones, because the write lives in the top-level runner. |

### Capture and query adapters

| Variable | Default or unset behavior | Effect |
| --- | --- | --- |
| `BOB_CLIPBOARD_CMD` | Platform clipboard discovery | Whitespace-split command that prints live clipboard text; takes precedence over `pbpaste`, Linux clipboard tools, and tmux buffer fallback. |
| `BOB_CLIPBOARD_HISTORY_CMD` | Clipy read-only history on macOS; no automatic provider elsewhere | Command receives the requested total count as its final argument and prints a newest-first JSON array of clipboard strings. |
| `BOB_DATAVIEW_OBSIDIAN_COMMAND` | `obsidian` found on `PATH` | Executable used for `bob query --engine obsidian`. |
| `BOB_DATAVIEW_VAULT` | Obsidian's default vault selection | Vault name or ID forwarded to `obsidian eval`. |

See the [capture command reference](cli/capture.md) and
[Dataview query contract](../dataview.md).

### References and integrations

| Variable | Default or unset behavior | Effect |
| --- | --- | --- |
| `BOB_HIGHLIGHTS_LIB_DIR` | `lib` beneath the vault | Highlights PDF library directory; relative paths resolve under the vault and a leading `~` expands. |
| `BOB_HIGHLIGHTS_REF_DIR` | `ref` beneath the vault | Reference-note directory; relative paths resolve under the vault and a leading `~` expands. |
| `BOB_HIGHLIGHTS_XLIB_DIR` | `xlib` beneath the vault | Intake directory for PDFs awaiting scan; relative paths resolve under the vault and a leading `~` expands. |
| `BOB_HIGHLIGHTS_AUDIO_LIBRARY` | Configured `highlights.audio_library`, then XDG data path, then `~/.local/share/sase-listen/library` | Companion-audio search root; a leading `~` expands. |
| `BOB_HIGHLIGHTS_AUDIO_LINK_TEMPLATE` | `highlights.audio_link_template`, then the default URI template | Overrides the PDF audio-button URI; empty disables the button. |
| `BOB_HIGHLIGHTS_CURL` | `curl` resolved through `PATH` | Executable used for PDF and arXiv downloads. |
| `BOB_HIGHLIGHTS_DIAGRAMS` | Mermaid diagrams render when the adapter is available | Case-insensitive values `off`, `0`, `false`, or `no` skip diagram rendering. |
| `BOB_HIGHLIGHTS_INGEST_ON_CREATE` | `highlights.ingest_on_create`, default `true` | Case-insensitive `0/false/no/off` disables inline intake; `1/true/yes/on` enables it; blank inherits the config value. |
| `BOB_HIGHLIGHTS_KEEP_WORKDIR` | Temporary work files are removed | Set to `1` to retain projected Markdown, TeX, logs, fonts, and staged assets for debugging. |
| `BOB_HIGHLIGHTS_LISTEN_COMMAND` | `highlights.listen_command` | Command template for `bob ref create --listen`; it must be a direct SASE Listen render invocation. |
| `BOB_HIGHLIGHTS_PRE_SCAN_HOOK` | `highlights.pre_scan_hook` | Shell command override run before `bob ref scan`; a present empty value disables the configured hook. |
| `BOB_PANDOC_COMMAND` | `pandoc` resolved through `PATH` | Pandoc executable for Markdown reference rendering. |
| `BOB_XELATEX_COMMAND` | `xelatex` resolved through `PATH` | XeLaTeX executable for PDF rendering. |
| `BOB_WEB_CLIP_ADAPTER` | Bob's pinned adapter launched through `uv` | Replaces the whole web-article adapter invocation. |
| `BOB_WEB_CLIP_TIMEOUT_SECS` | `300` seconds | Overall web-clip adapter timeout; diagram requests use their calculated budget, capped at this default. |
| `BOB_WEB_CLIP_KEEP_WORKDIR` | Temporary work files are removed | Set to `1` to retain the web-clip scratch directory for debugging. |
| `BOB_CHROME` | Discovered Google Chrome, then Playwright's bundled Chromium (newest installed revision first) | Browser executable for web article capture and Mermaid rendering. Bundled Chromium launches by its discovered path even when pinned Playwright's default revision is missing. |
| `BOB_REF_JOBS_KICK` | Eligible capture attempts to start the background worker | Exact values `off`, `0`, or `false` suppress the detached worker kick; queued jobs remain available to `bob ref jobs run`. |
| `BOB_REF_FRONTMOST_BUNDLE_ID` | Unset | Test override for the frontmost app bundle id used by the reading-activity probe. |
| `BOB_REF_HID_IDLE_SECONDS` | Unset | Test override for HID idle time in seconds, used with `BOB_REF_READING_PROBE` or `BOB_REF_FRONTMOST_BUNDLE_ID`. |
| `BOB_REF_INTAKE_LOCK_WAIT_SECONDS` | `60` seconds for maintenance lock and `30` seconds for scan lock | Bounds how long inline intake waits for vault maintenance locks. |
| `BOB_REF_PDF_SCAN_BUDGET_SECONDS` | `30` | Internal override for the coordinated-write scan-wide cap. |
| `BOB_REF_PDF_WRITE_BUDGET_SECONDS` | `10` | Internal override for the per-PDF coordinated-write budget. |
| `BOB_REF_READING_PROBE` | Unset | Internal override: `reading` or `away` forces the Highlights reading-activity probe. |
| `BOB_REF_READING_WINDOW_SECONDS` | `30` | Internal override for how recently HID input must be for an `editor_active` hold. |
| `BOB_GKEEP_ADAPTER` | `uv` runs Bob's pinned Keep adapter | Replaces the whole Keep adapter invocation. |
| `BOB_GCAL_ASSUME_TTY` | Unset (real TTY detection) | Test hook: `1` or `true` treats stdin as a TTY for login prompts and logout confirm. |
| `BOB_GCAL_BROWSER` | `open` on macOS, `xdg-open` elsewhere | Non-empty value selects browser sign-in and names the browser opener executable. |
| `BOB_GCAL_CONSENT_SECS` | 600 seconds | Positive integer override for the interactive Google consent window. |
| `BOB_GCAL_GOG` | `gog` resolved through `PATH` | Executable used by `bob gcal`; tests point this at a fake. Must be gog ≥ v0.44.0. |

See [reference jobs](../ref-jobs.md), [reference intake](../highlights-ref-sync.md),
[web article capture](../highlights-clip.md#hosts-and-browsers), and the
[Keep adapter contract](../gkeep.md#configuration), and the
[Google Calendar contract](../gcal.md#environment).

### Plugins and shell completion

| Variable | Default or unset behavior | Effect |
| --- | --- | --- |
| `BOB_PLUGINS_DIR` | `~/projects/github/bobs-org/bob-plugins` | Plugins repository root; a leading `~` expands. |
| `BOB_PLUGIN_BACKUPS_DIR` | `~/.local/state/bob-cli/plugin-backups` | Backup directory for overwritten plugin files; a leading `~` expands. |
| `BOB_COMPLETE_DEBUG` | No completion debug file | Appends hidden shell-completion request and response diagnostics to the exact path supplied. |

See [`bob completion`](cli/completion.md), [plugin deployment](../plugins.md),
the [Obsidian plugins reference](plugins/index.md), and the
[plugin deployment guide](../develop/plugins/deployment.md).

To install plugins, name the checkout you mean. Bare `bob plugins sync`
resolves `--repo`, then `BOB_PLUGINS_DIR`, then
`~/projects/github/bobs-org/bob-plugins`; a bare sync from inside a different
bob-plugins checkout refuses before any pull or copy. From the checkout you
want to deploy, preview then copy with that same `--repo`:

```bash
bob plugins list --repo "$PWD" --no-pull
bob plugins sync --repo "$PWD" --dry-run --no-pull
```

`--repo "$PWD"` requires the current directory to be that bob-plugins
checkout. Enable the copied plugins in Obsidian afterward. Do not run a live
deploy from this documentation.

## Other runtime environment

| Variable | Use |
| --- | --- |
| `HOME` | Home-directory fallback for the default vault, config, and XDG paths. |
| `XDG_CONFIG_HOME` | Base for `bob/config.yml`; defaults to `$HOME/.config`. |
| `XDG_STATE_HOME` | Base for Bob's mutable state; defaults to `$HOME/.local/state`. |
| `XDG_CACHE_HOME` | Base for Bob's local cache; defaults to `$HOME/.cache`. |
| `XDG_DATA_HOME` | Base for SASE Listen audio discovery and shell-completion data; defaults to `$HOME/.local/share`. |
| `XDG_RUNTIME_DIR` | Runtime location for the shared vault-sync lock when it exists; otherwise Bob uses `/tmp`. |
| `DATE` | Optional clock fallback after `BOB_NOW`: an ISO date/time, or a command whose output accepts Bob's date-time format. A command runs in the configured `timezone` when one is set. |
| `TZ` | Time zone used by the operating system's local clock and calendar formatting. |
| `NO_COLOR` | Any set value disables ANSI colors in terminal output. Wins over `CLICOLOR_FORCE`. |
| `CLICOLOR_FORCE` | A non-empty value other than `0` enables ANSI color even when stdout is not a TTY. Native help and `Styler::detect` honor it; `NO_COLOR` still wins. Compatibility shims (`BOB_CLI_USE_SCRIPT=1`) print plain text and have no separate color path. |
| `COLUMNS` | Positive integer width for `bob plugins list` and `bob gkeep`; otherwise those tables use 100 columns. |
| `PATH` | Discovery for `git`, `curl`, `pandoc`, `xelatex`, `uv`, browser tools, and configured command names. |
| `TMPDIR` | Temporary-directory base for reference rendering scratch files. |
| `WAYLAND_DISPLAY`, `DISPLAY`, `TMUX` | Detect Wayland, X11, or a display-less tmux session when choosing a Linux clipboard source. |
| `SHELL`, `BASH_COMPLETION_USER_DIR`, `ZSH`, `ZSH_CUSTOM`, `XDG_DATA_HOME` | Shell completion installation target discovery. |
| `GIT_SSH_COMMAND` | Passed through to Git; Bob's SSH wrapper is used only when this is unset. |

The principal external requirements are `git` for vault sync and several
maintenance commands; `obsidian` plus the Dataview plugin for the Obsidian
query engine; `curl` for PDF and arXiv URLs; `uv` and a browser for web
articles; `pandoc` and `xelatex` for Markdown-to-PDF rendering; `pass` for
the default Keep token store; and `uv` plus the pinned adapter packages for
Google Keep. Clipboard tools are platform-specific. Native execution does
not require Bash or Perl; the compatibility fallback may. See the
[runtime dependency list](compatibility.md#runtime-dependencies) and the
individual command references for command-specific requirements.

## Mac Capture hotkeys

Bob CLI owns no hotkey. When the Mac menu-bar app reports a hotkey conflict,
follow
[Resolve a Mac Capture hotkey conflict](../guides/resolve-mac-capture-hotkey-conflict.md).
The live chords are in
[Mac Capture settings](mac-capture/settings.md#hotkey)
and [Keyboard](mac-capture/keyboard.md).

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Requirements and compatibility](compatibility.md)
- [Configuration contracts for plan, freshness, and stats](../plan.md#config), [freshness](../freshness.md#2-fields-overrides-and-config), and [stats](../stats.md#stats-config)
- [Capture](../capture.md) and [reference library](../ref.md)
- [Resolve a Mac Capture hotkey conflict](../guides/resolve-mac-capture-hotkey-conflict.md)
