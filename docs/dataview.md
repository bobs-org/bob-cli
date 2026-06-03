# bob dataview

`bob dataview` runs Dataview source expressions and DQL queries from the shell.
The default engine is `obsidian`, which evaluates the query inside a running
desktop Obsidian app. The target vault must already be open, and the Dataview
community plugin must be enabled.

Stdout is reserved for query results only. Paths, JSON, and rendered Markdown
can be piped into scripts without sync logs, engine warnings, or diagnostics
mixed in.

## Examples

Run a source query and print one vault-relative Markdown path per matching note:

```bash
bob dataview --source '#project and -"archive"'
```

Run DQL and print source note paths. `paths` is the default format:

```bash
bob dataview --query 'LIST FROM #waiting'
bob dataview --strict-paths --query 'TABLE file.link, status FROM #project'
```

Use JSON when a script needs metadata and the structured Dataview result:

```bash
bob dataview --format json --query 'TABLE status, due FROM #project'
bob dataview --format json --query-file ~/queries/projects.dql | jq '.paths'
```

Render Dataview Markdown through Obsidian. Markdown output requires DQL:

```bash
bob dataview --format markdown --origin Home.md --query 'TABLE file.link FROM #project'
```

Read a query from a file, or from stdin with `-`:

```bash
bob dataview --query-file ~/queries/waiting.dql
printf 'LIST FROM #waiting\n' | bob dataview --query-file -
```

## Options

`--bob-dir <PATH>` sets the Bob vault root. It defaults to `BOB_DIR` or `~/bob`.
The path is validated when it is supplied explicitly or when the `dynomark` or
`native` engine is used.

`--engine <obsidian|dynomark|native>` selects the query engine. The default is
`obsidian`; `dynomark` is an explicit partial-compatibility headless fallback.
`native` is a local frontmatter/query subset for automation that cannot depend
on a running Obsidian app.

`--format <paths|json|markdown>` selects the output format. `paths` is the
default. `markdown` requires a DQL query and the Obsidian engine.

`--origin <VAULT_RELATIVE_PATH>` sets the origin note for Dataview `this` and
relative links. It must be vault-relative; absolute paths and `..` traversal are
rejected.

`--query <DQL>` runs an inline Dataview DQL query.

`--query-file <PATH>` reads Dataview DQL from a file. Use `-` to read the query
from stdin.

`--source <SOURCE>` runs a Dataview source expression with `pagePaths()`.

`--strict-paths` makes `paths` output fail if note paths cannot be derived
cleanly from every DQL row. Without it, best-effort path extraction warnings go
to stderr and the command prints the paths it can derive.

`--vault <NAME_OR_ID>` forwards an Obsidian vault name or ID to the Obsidian CLI.
If omitted, `BOB_DATAVIEW_VAULT` is used when set.

Exactly one of `--source`, `--query`, and `--query-file` is required.

`bob dataview` does not run `ob sync` or `ob sync-status`. Vault freshness is
owned by the external background or cron sync path.

## JSON Output

JSON output is a stable object for scripts. It includes:

- `engine`: `obsidian`, `dynomark`, or `native`
- `query_kind`: `source` or `dql`
- `format`: `json`
- `paths`: extracted vault-relative note paths
- `result`: structured DQL data for DQL queries
- `warnings`: path extraction or compatibility warnings

## Manual Smoke Test

For live smoke tests, start desktop Obsidian, open the target vault, and confirm
the Dataview plugin is enabled. Adjust tags or folders to values that exist in
the vault.

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

## Headless dynomark

For non-GUI shell or cron workflows, `bob dataview` also supports an explicit
partial-compatibility engine:

```bash
bob dataview --engine dynomark --format paths --query 'LIST FROM "Projects"'
bob dataview --engine dynomark --format json --query-file query.dql
```

Dynomark is a Dataview-like Markdown query engine, not the Obsidian Dataview
plugin runtime. It is useful when desktop Obsidian is unavailable, but its query
language and output are not guaranteed to match Dataview exactly. Validate
queries against the default Obsidian engine before relying on dynomark for
automation. Dynomark does not support Obsidian `--origin` context.

The dynomark engine runs `dynomark --query <DQL> --metadata` from `--bob-dir`.
Set `BOB_DATAVIEW_DYNOMARK_COMMAND` to use a specific executable. It supports
`paths` and `json` output for DQL queries; `--source` and `--format markdown`
remain Obsidian-only.

Set `BOB_DATAVIEW_OBSIDIAN_COMMAND` to use a specific Obsidian CLI executable
for the default engine. Set `BOB_DATAVIEW_VAULT` to choose the default vault
name or ID forwarded to `obsidian eval`.

## Headless native frontmatter queries

Use `--engine native` when the query only needs local Markdown frontmatter and
wikilink parent traversal. It does not call Obsidian, Dataview, or dynomark.

```bash
bob dataview --engine native --strict-paths --query '
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

The native engine supports `LIST` queries with an optional quoted folder source
such as `FROM "ref"`, plus `WHERE` expressions made from field truthiness,
`field = [[wikilink]]`, string/boolean comparisons, `AND`, `OR`, and
parentheses. Chained fields such as `parent.parent` resolve each intermediate
frontmatter value as an Obsidian wikilink or bare note target. It supports
`paths` and `json` output; source expressions and rendered Markdown remain
Obsidian-only.
