# bob dataview

`bob dataview` runs Dataview source expressions and DQL queries from the shell.
The default engine is `native`, which evaluates queries against the local
Markdown vault without a running desktop Obsidian app. Use `--engine obsidian`
when you need exact behavior from the live Dataview plugin in an open Obsidian
vault.

Stdout is reserved for query results only. Paths, JSON, and rendered Markdown
can be piped into scripts without sync logs, engine warnings, or diagnostics
mixed in.

## Examples

Run a source query and print one vault-relative Markdown path per matching note:

```bash
bob dataview --source '#project and -"archive"'
```

Run DQL and print source note paths. `paths` is the default format, including
for `TABLE` queries:

```bash
bob dataview --query 'LIST FROM #waiting'
bob dataview --strict-paths --query 'TABLE file.link, status FROM #project'
```

Use JSON when a script needs metadata and the structured Dataview result:

```bash
bob dataview --format json --query 'TABLE status, due FROM #project'
bob dataview --format json --query-file ~/queries/projects.dql | jq '.paths'
```

Render a visible Dataview table. Markdown output requires DQL:

```bash
bob dataview --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
```

Read a query from a file, or from stdin with `-`:

```bash
bob dataview --query-file ~/queries/waiting.dql
printf 'LIST FROM #waiting\n' | bob dataview --query-file -
```

## Options

`-b, --bob-dir <PATH>` sets the Bob vault root. It defaults to `BOB_DIR` or `~/bob`.
The path is validated when it is supplied explicitly or when the native engine
is used.

`-e, --engine <native|obsidian>` selects the query engine. The default is `native`,
the local headless implementation of Bob's supported source expression and DQL
surface. `obsidian` evaluates through the live Dataview plugin and is useful as
an oracle or fallback when installed-plugin behavior matters.

`-f, --format <paths|json|markdown>` selects the output format. `paths` is the
default and prints matching source note paths for DQL `LIST` and `TABLE`
queries. Use `json` for structured rows and `markdown` for rendered DQL `LIST`,
`TABLE`, and `TASK` output.

`-o, --origin <VAULT_RELATIVE_PATH>` sets the origin note for Dataview `this` and
relative links. It must be vault-relative; absolute paths and `..` traversal are
rejected.

`-q, --query <DQL>` runs an inline Dataview DQL query.

`-Q, --query-file <PATH>` reads Dataview DQL from a file. Use `-` to read the
query from stdin.

`-s, --source <SOURCE>` runs a Dataview source expression with `pagePaths()`.

`-S, --strict-paths` makes `paths` output fail if note paths cannot be derived
cleanly from every DQL row. Without it, best-effort path extraction warnings go
to stderr and the command prints the paths it can derive.

`-v, --vault <NAME_OR_ID>` forwards an Obsidian vault name or ID to the
Obsidian CLI. It can only be used with `--engine obsidian`. If omitted in Obsidian mode,
`BOB_DATAVIEW_VAULT` is used when set.

Exactly one of `-s|--source`, `-q|--query`, and `-Q|--query-file` is required.

`bob dataview` does not run `ob sync` or `ob sync-status`. Vault freshness is
owned by the external background or cron sync path.

## JSON Output

JSON output is a stable object for scripts. It includes:

- `engine`: `native` or `obsidian`
- `query_kind`: `source` or `dql`
- `format`: `json`
- `paths`: extracted vault-relative note paths
- `result`: structured DQL data for DQL queries
- `warnings`: path extraction or compatibility warnings

## Manual Smoke Test

For local smoke tests, adjust tags or folders to values that exist in the vault.

```bash
bob dataview --source '#project'
bob dataview --query 'LIST FROM #project'
bob dataview --format json --query 'TABLE file.path FROM #project'
bob dataview --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
printf 'LIST FROM #project\n' >/tmp/bob-dataview-smoke.dql
bob dataview --query-file /tmp/bob-dataview-smoke.dql
```

If the smoke test needs recently synced state, let the external background or
cron sync path finish first. `bob dataview` only reads the current vault state.

The fixture parity harness compares supported native fixture queries against the
live Obsidian engine. It is intentionally gated because it requires desktop
Obsidian to be running with the fixture vault open:

```bash
BOB_DATAVIEW_PARITY_LIVE=1 \
BOB_DATAVIEW_PARITY_VAULT=<opened-fixture-vault-name-or-id> \
cargo test --test dataview_parity dataview_live_obsidian_parity_harness_compares_supported_native_cases -- --nocapture
```

For real-vault native smoke tests, use read-only queries against `~/bob`. These
cover the supported local surface without requiring Obsidian:

```bash
bob dataview --strict-paths --query '
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
bob dataview --source '#project'
bob dataview --source '"prj"'
bob dataview --strict-paths --query 'LIST FROM "prj" LIMIT 5'
bob dataview --strict-paths --query 'TABLE file.mday, parent FROM "prj" LIMIT 5'
bob dataview --strict-paths --query 'TASK LIMIT 3'
bob dataview --strict-paths --query 'CALENDAR file.day WHERE file.day LIMIT 5'
bob dataview --format markdown --query 'LIST FROM "prj" LIMIT 3'
bob dataview --format markdown --query 'TABLE file.mday, parent FROM "prj" LIMIT 3'
bob dataview --format markdown --query 'TASK LIMIT 3'
```

Native indexing may warn about ambiguous bare wikilinks when multiple notes
share the same stem or alias. Those warnings are diagnostics about vault links;
they do not make otherwise successful read-only smoke queries fail.

## Native Dataview queries

Native mode runs against the local Markdown index. It does not call Obsidian or
the live Dataview plugin.

```bash
bob dataview --strict-paths --query '
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

Native mode remains scoped to the shell contract exposed by `bob dataview`. It
does not implement DataviewJS, inline DQL modes, Obsidian DOM rendering,
interactive task checking, or every plugin setting. Quoted native sources
resolve an exact note path first and otherwise act as folder sources; use a
more specific folder path when a vault contains both `Name.md` and `Name/...`.

## Live Obsidian engine

Use `--engine obsidian` when a query needs the exact installed Dataview plugin.
The target vault must already be open in desktop Obsidian, and the Dataview
community plugin must be enabled.

```bash
bob dataview --engine obsidian --source '#project'
bob dataview --engine obsidian --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
```

Set `BOB_DATAVIEW_OBSIDIAN_COMMAND` to use a specific Obsidian CLI executable.
Set `BOB_DATAVIEW_VAULT` or pass `--vault` to choose the vault name or ID
forwarded to `obsidian eval`.
