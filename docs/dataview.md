# bob query

`bob query` runs Dataview source expressions, Dataview DQL, and Obsidian Tasks
queries from the shell. The default engine is `native`, which evaluates queries
against the local Markdown vault without a running desktop Obsidian app. Use
`--engine obsidian` when you need exact behavior from the live Dataview plugin
in an open Obsidian vault; Tasks inputs are native-only for now.

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

Run the current filterless Tasks query slice. It reads the vault's Tasks plugin
settings, applies the global filter, and returns every matching task. An empty
inline query or an empty/comment-only file is accepted:

```bash
bob query --tasks ''
bob query --format json --tasks-file queries/all.tasks
printf '# all globally-filtered tasks\n' | bob query --tasks-file -
```

The complete Tasks instruction language, Markdown rendering, and whole-note
execution through `--tasks-note` are reserved by the CLI but not implemented in
this initial slice. Unsupported uses fail explicitly instead of silently
returning incomplete results.

## Options

`-b, --bob-dir <PATH>` sets the Bob vault root. It defaults to `BOB_DIR` or `~/bob`.
The path is validated when it is supplied explicitly or when the native engine
is used.

`-e, --engine <native|obsidian>` selects the query engine. The default is
`native`, the local headless implementation of Bob's supported Dataview and
Tasks surface. `obsidian` evaluates Dataview through the live plugin and is
useful as an oracle or fallback when installed-plugin behavior matters. Tasks
inputs currently reject the Obsidian engine.

`-f, --format <paths|json|markdown>` selects the output format. `paths` is the
default and prints matching source note paths for Dataview results or unique
note paths containing matched Tasks results. Use `json` for structured output
and `markdown` for rendered Dataview DQL `LIST`, `TABLE`, and `TASK` output.
Tasks queries currently support only `paths` and `json`.

`-o, --origin <VAULT_RELATIVE_PATH>` sets the origin note for Dataview `this`
and relative links, or the future Tasks `query.file.*` context and Query File
Defaults. It must be vault-relative; absolute paths and `..` traversal are
rejected. It cannot be combined with `--tasks-note`, whose note supplies its own
origin.

`-q, --query <DQL>` runs an inline Dataview DQL query.

`-Q, --query-file <PATH>` reads Dataview DQL from a file. Use `-` to read the
query from stdin.

`-s, --source <SOURCE>` runs a Dataview source expression with `pagePaths()`.

`-S, --strict-paths` makes `paths` output fail if note paths cannot be derived
cleanly from every DQL row. Without it, best-effort path extraction warnings go
to stderr and the command prints the paths it can derive.

`-t, --tasks <QUERY>` runs an inline Obsidian Tasks query. The current native
slice accepts an empty or comment-only query and returns every task allowed by
the configured global filter.

`-T, --tasks-file <PATH>` reads an Obsidian Tasks query from a file. Use `-` to
read the query from stdin. Empty and comment-only input use the filterless
slice.

`-n, --tasks-note <VAULT_RELATIVE_PATH>` reserves whole-note Tasks block
execution. The path is validated now; execution will be enabled when Query File
Defaults, placeholders, and multi-block rendering are implemented.

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

Filterless Tasks JSON uses the same wrapper with `query_kind: "tasks"`. Its
`result` contains the matched task count and a full record for each task:
parsed status, descriptions, dates (including invalid-date state), priority,
recurrence, dependency and block-link metadata, tags, file and heading context,
list hierarchy, blocked/blocking state, and urgency. Line numbers are
zero-based, matching the Tasks plugin scripting API. `settings` contains the
Tasks settings that governed the scan. Metadata is parsed using the configured
Tasks format (`dataview` or `tasksPluginEmoji`).

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

`BOB_TASKS_PARITY_LIVE=1` with `BOB_TASKS_PARITY_VAULT` enables the documented
live-oracle scaffold. A later parity phase will render fenced `tasks` blocks
through Obsidian's `MarkdownRenderer`, wait for the Tasks plugin's asynchronous
DOM output, and scrape matched rows and group headings.

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
