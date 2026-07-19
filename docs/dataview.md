# bob query

`bob query` runs Dataview source expressions, Dataview DQL, and Obsidian Tasks
queries from the shell. The default engine is `native`, which evaluates queries
against the local Markdown vault without a running desktop Obsidian app. Use
`--engine obsidian` when you need exact behavior from the live Dataview plugin
in an open Obsidian vault. Tasks inputs are native-only; an env-gated test
harness provides the live Tasks renderer oracle without making DOM scraping a
public query engine.

Stdout is reserved for query results only. Paths, JSON, and rendered Markdown
can be piped into scripts without sync logs, engine warnings, or diagnostics
mixed in.

## Examples

Run a source query and print one vault-relative Markdown path per matching note:

```bash
bob query --source '#project and -"archive"'
```

Run DQL and print source note paths. `paths` is the default format, including
for `TABLE` queries:

```bash
bob query --query 'LIST FROM #waiting'
bob query --strict-paths --query 'TABLE file.link, status FROM #project'
```

Use JSON when a script needs metadata and the structured Dataview result:

```bash
bob query --format json --query 'TABLE status, due FROM #project'
bob query --format json --query-file ~/queries/projects.dql | jq '.paths'
```

Render a visible Dataview table. Markdown output requires DQL:

```bash
bob query --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
```

Read a query from a file, or from stdin with `-`:

```bash
bob query --query-file ~/queries/waiting.dql
printf 'LIST FROM #waiting\n' | bob query --query-file -
```

Run Tasks v8 queries using the vault's plugin settings, custom statuses, global
filter/query, presets, task format, dependencies, and JavaScript `by function`
instructions. An empty query returns every task allowed by the global filter:

```bash
bob query --tasks ''
bob query --tasks 'status.type is TODO' --origin dash.md
bob query --format json --tasks-file queries/all.tasks
printf '# all globally-filtered tasks\n' | bob query --tasks-file -
```

Run every fenced `tasks` block in a note with that note's `TQ_*` Query File
Defaults and `query.file.*` context:

```bash
bob query --tasks-note dash.md
bob query --format json --tasks-note dash.md
bob query --format markdown --tasks-note dash.md
```

## Options

`-b, --bob-dir <PATH>` sets the Bob vault root. It defaults to `BOB_DIR` or `~/bob`.
The path is validated when it is supplied explicitly or when the native engine
is used.

`-e, --engine <native|obsidian>` selects the query engine. The default is
`native`, the local headless implementation of Bob's supported Dataview and
Tasks surface. `obsidian` evaluates Dataview through the live plugin and is
useful as an oracle or fallback when installed-plugin behavior matters. Tasks
inputs reject the Obsidian engine because its DOM-rendering mechanism has a
smaller, less stable output contract; use the live parity harness below as the
Tasks oracle.

`-f, --format <paths|json|markdown>` selects the output format. `paths` is the
default and prints matching source note paths for Dataview results or unique
note paths containing matched Tasks results. Use `json` for structured output
and `markdown` for rendered Dataview DQL or static Tasks output. For
`--tasks-note`, paths output labels each block before its unique paths, JSON
contains a `blocks` array, and Markdown gives each block its own heading.

`-o, --origin <VAULT_RELATIVE_PATH>` sets the origin note for Dataview `this`
and relative links. For `--tasks` and `--tasks-file`, it supplies
`query.file.*`, placeholder values, and the note's `TQ_*` Query File Defaults.
It must be vault-relative; absolute paths and `..` traversal are rejected. It
cannot be combined with `--tasks-note`, whose note supplies its own origin.

`-q, --query <DQL>` runs an inline Dataview DQL query.

`-Q, --query-file <PATH>` reads Dataview DQL from a file. Use `-` to read the
query from stdin.

`-s, --source <SOURCE>` runs a Dataview source expression with `pagePaths()`.

`-S, --strict-paths` makes `paths` output fail if note paths cannot be derived
cleanly from every DQL row. Without it, best-effort path extraction warnings go
to stderr and the command prints the paths it can derive.

`-t, --tasks <QUERY>` runs an inline Obsidian Tasks query. Newline-separated
instructions are accepted. Empty or comment-only input returns every task
allowed by the configured global filter.

`-T, --tasks-file <PATH>` reads an Obsidian Tasks query from a file. Use `-` to
read the query from stdin.

`-n, --tasks-note <VAULT_RELATIVE_PATH>` extracts and runs every fenced `tasks`
block in the note. Each block uses the note as its origin and is identified by
its nearest preceding Markdown heading, one-based block index, and zero-based
fence `lineNumber` in JSON (human output displays one-based line numbers).

`-v, --vault <NAME_OR_ID>` forwards an Obsidian vault name or ID to the
Obsidian CLI. It can only be used with `--engine obsidian`. If omitted in Obsidian mode,
`BOB_DATAVIEW_VAULT` is used when set.

Exactly one of `-s|--source`, `-q|--query`, `-Q|--query-file`, `-t|--tasks`,
`-T|--tasks-file`, and `-n|--tasks-note` is required.

`bob query` does not run `ob sync` or `ob sync-status`. Vault freshness is
owned by the external background or cron sync path.

## JSON Output

Dataview JSON output is a stable object for scripts. It includes:

- `engine`: `native` or `obsidian`
- `query_kind`: `source` or `dql`
- `format`: `json`
- `paths`: extracted vault-relative note paths
- `result`: structured DQL data for DQL queries
- `warnings`: path extraction or compatibility warnings

Single-query Tasks JSON uses the same wrapper with `query_kind: "tasks"`. Its
`result` contains the matched task count and a full record for each task:
parsed status, descriptions, dates (including invalid-date state), priority,
recurrence, dependency and block-link metadata, tags, file and heading context,
list hierarchy, blocked/blocking state, and urgency. Line numbers are
zero-based, matching the Tasks plugin scripting API. `settings` contains the
Tasks settings that governed the scan. Metadata is parsed using the configured
Tasks format (`dataview` or `tasksPluginEmoji`).

Dependency equality follows Obsidian Tasks exactly and is vault-wide. Bob uses
`<note-path-with-slashes-as-__>__<block-id>` for `[id::]` and
`[dependsOn::]`, while the navigation block remains file-local. For example,
`projects/Shared.md#^review` is represented as
`projects__Shared__review`. This lets separate notes reuse `^review` without
colliding in blocked/blocking calculations.

Whole-note JSON uses `query_kind: "tasks_note"`, includes `note`, the union of
matched `paths`, and one entry per code block in `blocks`. Each block has
`index`, zero-based `lineNumber`, `heading`, raw `query`, `parsedQuery`, `paths`,
and the same structured `result` object as a single Tasks query.

## Manual Smoke Test

For local smoke tests, adjust tags or folders to values that exist in the vault.

```bash
bob query --source '#project'
bob query --query 'LIST FROM #project'
bob query --format json --query 'TABLE file.path FROM #project'
bob query --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
printf 'LIST FROM #project\n' >/tmp/bob-dataview-smoke.dql
bob query --query-file /tmp/bob-dataview-smoke.dql
bob query --tasks ''
bob query --format json --tasks '' | jq '.result.count'
bob query --tasks 'status.type is TODO' --origin dash.md
bob query --format markdown --tasks-note dash.md
```

If the smoke test needs recently synced state, let the external background or
cron sync path finish first. `bob query` only reads the current vault state.

The fixture parity harness compares supported native fixture queries against the
live Obsidian engine. It is intentionally gated because it requires desktop
Obsidian to be running with the fixture vault open:

```bash
BOB_DATAVIEW_PARITY_LIVE=1 \
BOB_DATAVIEW_PARITY_VAULT=<opened-fixture-vault-name-or-id> \
cargo test --test dataview_parity dataview_live_obsidian_parity_harness_compares_supported_native_cases -- --nocapture
```

The Tasks parity fixture and filterless goldens run without Obsidian:

```bash
cargo test --test tasks_parity
```

The Tasks live oracle renders a fenced block through Obsidian's
`MarkdownRenderer`, waits for the Tasks plugin's asynchronous DOM output, and
scrapes descriptions, status symbols, backlink targets, group headings, and
render errors. It compares a dashboard-style block with the native result. The
test skips cleanly when the desktop command is unavailable; when enabled, the
fixture vault must be open in Obsidian with Tasks enabled:

```bash
BOB_TASKS_PARITY_LIVE=1 \
BOB_TASKS_PARITY_VAULT=<opened-fixture-vault-name-or-id> \
cargo test --test tasks_parity tasks_live_obsidian_parity_harness_renders_and_scrapes_tasks_blocks -- --nocapture
```

### Real-vault Tasks acceptance

The gated real-vault acceptance test first snapshots the vault's Markdown and
installed Tasks settings so background sync cannot change the inputs between
queries. It then independently scans raw task lines without calling the native
Tasks parser or filter engine. It derives the expected WIP, NEXT, and READY
sets using the dashboard's global-filter, status, scheduled-date, `#hide`,
`_templates`, `dash.md` self-exclusion, and dependency-blocking rules. It
compares those sets by path, zero-based line number, and status symbol with both
`--tasks-note dash.md` and the three individual `--tasks ... --origin dash.md`
executions. It also executes every other fenced Tasks block in the snapshot and
verifies that none are skipped.

Pin `BOB_NOW` so scheduled-date behavior remains reproducible:

```bash
BOB_TASKS_REAL_VAULT_PARITY=1 \
BOB_DIR="$HOME/bob" \
BOB_NOW=2026-07-10T12:00:00 \
cargo test --test tasks_real_vault_parity -- --nocapture
```

The 2026-07-10 acceptance run matched all three independently derived sets: 6
WIP, 8 NEXT, and 40 READY tasks. All 13 other Tasks blocks present in the vault
also parsed and executed successfully. The phase design's earlier inventory of
14 non-dashboard blocks had changed by acceptance time.

Desktop Obsidian was unavailable for that run, so live DOM-renderer
confirmation of the real `dash.md` remains a manual acceptance check. Native
Tasks queries intentionally remain read-only and do not render or mutate
interactive toolbar, edit, postpone, recurrence-generation, or completion
actions; Tasks inputs also remain unsupported by the public `--engine
obsidian` surface. These are explicit non-goals, not silent native-query
fallbacks.

For real-vault native smoke tests, use read-only queries against `~/bob`. These
cover the supported local surface without requiring Obsidian:

```bash
bob query --strict-paths --query '
LIST
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
'
bob query --source '#project'
bob query --source '"prj"'
bob query --strict-paths --query 'LIST FROM "prj" LIMIT 5'
bob query --strict-paths --query 'TABLE file.mday, parent FROM "prj" LIMIT 5'
bob query --strict-paths --query 'TASK LIMIT 3'
bob query --strict-paths --query 'CALENDAR file.day WHERE file.day LIMIT 5'
bob query --format markdown --query 'LIST FROM "prj" LIMIT 3'
bob query --format markdown --query 'TABLE file.mday, parent FROM "prj" LIMIT 3'
bob query --format markdown --query 'TASK LIMIT 3'
```

Native indexing may warn about ambiguous bare wikilinks when multiple notes
share the same stem or alias. Those warnings are diagnostics about vault links;
they do not make otherwise successful read-only smoke queries fail.

## Native Dataview queries

Native mode runs against the local Markdown index. It does not call Obsidian or
the live Dataview plugin.

```bash
bob query --strict-paths --query '
LIST
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
'
```

The native engine supports source expressions, `LIST`, `TABLE`, `TASK`, and
`CALENDAR` JSON results, common Dataview expressions/functions, and ordered data
commands such as `FROM`, `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`, and `LIMIT`.

Native `paths` output prints matching source note paths where a DQL result
retains source identity. Native `json` output emits the stable Bob wrapper.
Native `markdown` output renders DQL `LIST`, `TABLE`, and `TASK` results and
fails cleanly for `CALENDAR`, matching Dataview's Markdown export behavior.

Native mode remains scoped to the shell contract exposed by `bob query`. It
does not implement DataviewJS, inline DQL modes, Obsidian DOM rendering,
interactive task checking, or every plugin setting. Quoted native sources
resolve an exact note path first and otherwise act as folder sources; use a
more specific folder path when a vault contains both `Name.md` and `Name/...`.

## Live Obsidian engine

Use `--engine obsidian` when a query needs the exact installed Dataview plugin.
The target vault must already be open in desktop Obsidian, and the Dataview
community plugin must be enabled.

```bash
bob query --engine obsidian --source '#project'
bob query --engine obsidian --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
```

Set `BOB_DATAVIEW_OBSIDIAN_COMMAND` to use a specific Obsidian CLI executable.
Set `BOB_DATAVIEW_VAULT` or pass `--vault` to choose the vault name or ID
forwarded to `obsidian eval`.
