---
create_time: 2026-06-03
status: research
topic: Running Obsidian Dataview queries from the command line
---
# Research: Running Dataview Queries from the Command Line

## Question

Bob wants to run [Dataview](https://blacksmithgu.github.io/obsidian-dataview/) queries
from the command line and get back **all notes that match a particular query** (e.g. for
scripting, automation, or feeding results to other tools/agents). Is this possible, and
what is the best way to implement it?

## Current Vault State (observed)

From `/home/bryan/bob/.obsidian/community-plugins.json` and `.obsidian/plugins/`:

- `dataview` **is** installed and enabled.
- The following are also enabled: `obsidian-tasks-plugin`, `templater-obsidian`,
  `quickadd`, `task-status-cycler`, `mrj-jump-to-link`, `bob-navigation-hotkeys`,
  `bob-ledger-tools`, `block-id-prompt`, `obsidian-relative-line-numbers`,
  `note-refactor-obsidian`.
- The **Local REST API** plugin (`coddingtonbear/obsidian-local-rest-api`) is **NOT yet
  installed**. It is the key dependency for the REST API option below, but not for the
  Obsidian CLI `eval` option.
- Dataview's plugin manifest reports version `0.5.68`.
- `ob`, `obsidian`, `node`, and `npm` are installed locally. `ob` is useful for Obsidian
  Sync freshness, but it is not a Dataview runtime.

So Bob's notes already carry Dataview metadata (frontmatter + `key:: value` inline fields),
and the query language is already in daily use inside Obsidian.

## The Core Constraint

**Dataview has no official, native CLI.** This is a long-standing, explicitly-acknowledged
gap, not an oversight we can quickly route around:

- The maintainer has said CLI extraction is "on my priority list" but it is **not
  implemented** (request open since 2021, still requested as of mid-2025).
- The blocker is architectural: Dataview's index is built on **Obsidian's APIs** — most
  importantly Obsidian's `CachedMetadata` database. Core functions like `parsePage` expect
  items handed to them *by Obsidian*. Dataview cannot currently build its index from raw
  markdown on disk without the Obsidian app running.
- Dataview publishes TypeScript typings to npm (`obsidian-dataview`), but these are for
  **plugin development inside Obsidian**, not for standalone Node use.

Consequently every viable option falls into one of three buckets:

1. **Use the *real* Dataview engine via Obsidian CLI `eval`** — drive a running Obsidian
   app directly, without installing another community plugin.
2. **Use the *real* Dataview engine via an HTTP plugin** — drive a running Obsidian app
   through Local REST API.
3. **Use a *reimplementation* of DQL** that reads markdown directly off disk — no Obsidian
   needed, but only a subset of DQL is supported (lower fidelity, fully headless).

## Options

### Option A — Obsidian CLI `eval` + Dataview API (real Dataview engine)

The current Obsidian CLI can run JavaScript inside the desktop Obsidian app:

```bash
obsidian eval code="app.vault.getFiles().length"
```

It can target a vault with `vault=<name-or-id>` or, when run from inside a vault folder,
use that vault by default. Official docs say the app must be running; if it is not running,
the CLI launches it.

That gives Bob a direct route to Dataview's own plugin API:

```javascript
const api = app.plugins.plugins.dataview?.api ?? window.DataviewAPI;
```

Useful API surfaces:

- `api.pagePaths(source)` for Dataview **source** expressions such as `#tag`, `"folder"`,
  `[[note]]`, and boolean combinations of sources.
- `api.tryQuery(query, originFile, { forceId: true })` for full DQL execution.
- `api.tryQueryMarkdown(query, originFile)` for rendered markdown output.

Important limitation: `pagePaths()` accepts a Dataview source expression, not a full DQL
query. Full filtering such as:

```dataview
LIST
FROM #project
WHERE status = "active"
SORT file.mtime DESC
```

must go through `tryQuery()` or `tryQueryMarkdown()`.

Recommended `bob-cli` command shape:

```bash
bob dataview --format paths --query 'LIST FROM #project WHERE status = "active"'
bob dataview --format markdown --query 'TABLE file.mtime FROM #project SORT file.mtime DESC'
bob dataview --format json --query 'TASK FROM #project WHERE !completed'
```

Suggested options:

- `--format markdown|json|paths`: output mode; default `paths` if the primary goal is note
  matching.
- `--origin <path>`: Dataview origin file for `this` and relative links.
- `--query <DQL>`: query text.
- `--query-file <path>`: read query text from a file to avoid shell quoting.
- `--sync`: optionally run `ob sync --path <vault>` before querying.
- `--vault <name-or-id>`: pass through to Obsidian CLI vault targeting.

Implementation notes:

- Build the JavaScript snippet in Rust and pass it as an `obsidian` argument; do not depend
  on shell interpolation for query quoting.
- Wait for `api.index.initialized`, or subscribe once to
  `app.metadataCache.on("dataview:index-ready", ...)`, before querying on a cold start.
- For `--format markdown`, call `tryQueryMarkdown()`.
- For `--format json`, call `tryQuery()` and serialize the result.
- For `--format paths`, normalize the result to note paths. This is straightforward for
  ordinary `LIST`/`TABLE` rows, `TASK` rows via `task.path`, and `CALENDAR` links, but
  grouped queries, `WITHOUT ID`, and intentionally transformed rows may not have a
  one-note-per-row meaning.

- **Pros:** exact Dataview semantics; no additional community plugin; works with the
  existing enabled Dataview plugin; fits `bob-cli` as a thin native wrapper.
- **Cons:** requires desktop Obsidian CLI/app availability; cold starts need index-ready
  handling; path-only output needs documented limits for grouped/transformed DQL.
- **Net:** best first implementation for `bob-cli` when exact DQL behavior matters and a
  desktop Obsidian session is available.

### Option B — Local REST API plugin + `curl` (real Dataview engine)

`coddingtonbear/obsidian-local-rest-api` exposes a secure REST API over the running vault.
Its `POST /search/` endpoint accepts a **Dataview DQL `TABLE` query** directly when you set
the content type:

```
Content-Type: application/vnd.olrapi.dataview.dql+txt
```

Because the query runs inside Obsidian, it uses the **actual Dataview index** — exact DQL
semantics, all implicit fields (`file.name`, `file.path`, `file.mtime`, `file.tags`, …),
inline fields, and resolved links all behave exactly as they do in the app.

Example (HTTPS on the default port, self-signed cert ⇒ `-k`):

```bash
curl -sk -X POST \
  -H "Authorization: Bearer $OBSIDIAN_API_KEY" \
  -H "Content-Type: application/vnd.olrapi.dataview.dql+txt" \
  --data 'TABLE file.mtime AS modified, status FROM #project WHERE status = "active" SORT file.mtime DESC' \
  https://127.0.0.1:27124/search/
```

The response is JSON: one entry per matching file, with the file path plus the evaluated
column values — i.e. exactly "all notes that match the query," machine-readable and ready
to pipe into `jq`, scripts, or an agent.

- **Pros:** real Dataview semantics (highest fidelity); JSON out of the box; trivial to
  wrap in a shell function/alias; no new language runtime — just `curl`; the API key lives
  in the plugin settings.
- **Cons:** Obsidian **must be running** with the plugin enabled; query type is limited to
  `TABLE` over this endpoint (no `LIST`/`TASK`/`CALENDAR` directly — though `TABLE` plus
  `file.link`/columns covers most "which notes match" needs); HTTPS uses a self-signed cert.
- **Setup:** install + enable the Local REST API plugin, copy its API key, keep Dataview
  enabled. (Dataview is already enabled in Bob's vault; only Local REST API is missing.)

### Option C — `dnvriend/obsidian-search-tool` (ergonomic CLI wrapper over Option B)

A purpose-built **CLI** that talks to the same Local REST API and is explicitly designed to
be "agent-friendly." It wraps the DQL-`TABLE` endpoint (and a JsonLogic mode) with nicer
ergonomics and multiple output formats (JSON for automation, Markdown, or pretty tables).

```bash
obsidian-search-tool search 'TABLE file.name FROM #project'
obsidian-search-tool search 'TABLE file.name, author WHERE author SORT file.mtime DESC'
```

- **Requires:** Obsidian running; Local REST API plugin **and** Dataview enabled;
  `OBSIDIAN_API_KEY` env var; Python 3.14+ with `uv`.
- **Supports:** `TABLE` queries with `FROM` (tags/folders/files/links), `WHERE`, `SORT`
  (multi-field), `LIMIT`; functions `date()`, `dur()`, `contains()`; comparison/logical
  operators; the common implicit `file.*` fields.
- **Does NOT support:** `GROUP BY`, `FLATTEN`, `LIST`, `TASK`, `CALENDAR`.
- **Net:** same fidelity/availability trade-off as Option B (it *is* Option B under the
  hood) but saves us writing the curl/JSON plumbing — at the cost of a Python+uv dependency.
  Good if we want a ready-made, documented CLI rather than a shell wrapper we maintain.

### Option D — `k-lar/dynomark` (standalone DQL reimplementation, no Obsidian)

A **standalone Go binary** that reimplements a Dataview-like query language and reads
markdown **directly off disk** — *no Obsidian instance required*. This is the best fit
whenever queries must run headless (cron jobs, CI, a server, or any context where launching
Obsidian is impractical).

```bash
dynomark 'TASK FROM "examples/test.md" WHERE NOT CHECKED'
dynomark 'TABLE file.cday AS "Date", title FROM todos/'
dynomark 'PARAGRAPH FROM examples/ WHERE [author] IS "Shakespeare"'
```

- **Supports:** `LIST`, `TASK`, `PARAGRAPH`, `ORDEREDLIST`, `UNORDEREDLIST`, `FENCEDCODE`,
  `TABLE` (+ `TABLE NO ID`); `WHERE` with `AND`/`OR`, `CONTAINS`, `IS`; `SORT ASC/DESC`;
  `GROUP BY` (with max-group limits); `LIMIT`; `AS` aliases. Dataview-style `key: value`
  metadata plus ~10 built-in file fields (path, name, size, created/modified timestamps).
- **Does NOT (yet) match real Dataview:** it's a *partial* implementation. Advanced
  operators, regex, rich date arithmetic, functions, and nested queries are not documented
  as supported. Inline `key:: value` vs frontmatter coverage and edge-case semantics will
  diverge from the genuine engine — queries must be validated against expected output.
- **Maturity:** ~v0.2.0 (Nov 2024), early-stage but functional; editor integrations exist
  (Neovim/VS Code/Emacs); prebuilt binaries for Linux/macOS/Windows, or `make && sudo make
  install` with Go ≥ 1.22.5.
- **Pros:** truly headless; fast; single binary; reads the vault as plain files.
- **Cons:** not byte-for-byte Dataview-compatible — fidelity is the price of independence.

### Option E — In-Obsidian export plugins (adjacent, not a true CLI)

Plugins like `udus122/dataview-publisher` (and similar "dataview serializer" tools) run a
Dataview query **inside** Obsidian and write the rendered results back into a markdown file,
keeping it up to date. Useful if the real goal is "materialize query results into a note,"
but they run inside the app on Obsidian's schedule — they are **not** a command-line
interface. Mentioned for completeness; not recommended for CLI/scripting use.

### Option F — `intellectronica/mdbasequery` (different query language)

A standalone CLI/library that queries Markdown-frontmatter "bases" and is **Obsidian
*Bases*-compatible** (the newer native query feature), running on Node 20+/Bun/Deno. It is
**not** Dataview DQL — different syntax and semantics — and it only sees frontmatter, not
Dataview inline `key:: value` fields. Worth knowing about given Obsidian's industry-wide
drift from Dataview toward Bases/Datacore, but it does not satisfy "run *Dataview* queries"
today. Listed as a forward-looking alternative, not a match.

## Recommendation

Pick by whether Obsidian can be running at query time and whether we want an extra plugin:

1. **Best `bob-cli` default: Option A — Obsidian CLI `eval` + Dataview API.** It uses the
   actual Dataview engine, does not require installing Local REST API, and can support
   `LIST`, `TABLE`, `TASK`, `CALENDAR`, markdown output, JSON output, and path output from
   one native `bob dataview` wrapper.

2. **HTTP/API workflow: Option B or C.** If we want a persistent HTTP endpoint with API-key
   authentication, install Local REST API and either call it with `curl` or use
   `obsidian-search-tool`. This is a good automation surface when Obsidian is already
   running, but the documented Dataview endpoint is `TABLE`-oriented and requires another
   community plugin.

3. **Headless / automation (Obsidian not running): Option D — `dynomark`, or a small
   Rust-only query subset.** Accept that this is not exact Dataview. Validate each query
   against expected output and pin usage to the subset the tool or Bob implementation
   supports.

**Not recommended:** waiting for native Dataview CLI support (no timeline), or relying on
the npm `obsidian-dataview` typings to build our own headless indexer. Dataview's real index
depends on Obsidian's `App`, `Vault`, `MetadataCache`, IndexedDB/local storage, and plugin
lifecycle, so a standalone Node CLI would effectively recreate a large part of Obsidian.

## Open Questions / Follow-ups

- Which DQL query *types* does Bob actually need from the CLI? If it's purely "list the
  notes matching X," `TABLE`/`LIST` cover it. If `TASK`, `GROUP BY`, or `FLATTEN` are
  required, Option A has the best chance of preserving exact Dataview semantics; the REST
  endpoint is `TABLE`-only and `dynomark`'s coverage must be checked per-feature.
- Is the use case interactive (Obsidian usually open) or automated (headless)? That choice
  is what selects Option A/B/C vs Option D.
- Longer term: given the ecosystem shift toward **Bases/Datacore**, is it worth tracking
  `mdbasequery` (Option F) as Bob's metadata strategy evolves?

## Sources

- [Dataview — Extracting data from CLI (Discussion #471)](https://github.com/blacksmithgu/obsidian-dataview/discussions/471)
- [Dataview — Accessing the API/database outside Obsidian (Discussion #1811)](https://github.com/blacksmithgu/obsidian-dataview/discussions/1811)
- [Export Dataview query results to CSV from command line (Obsidian Forum)](https://forum.obsidian.md/t/export-dataview-query-results-to-csv-from-command-line/48046)
- [k-lar/dynomark (standalone DQL CLI, Go)](https://github.com/k-lar/dynomark)
- [coddingtonbear/obsidian-local-rest-api](https://github.com/coddingtonbear/obsidian-local-rest-api)
- [Local REST API — interactive API docs](https://coddingtonbear.github.io/obsidian-local-rest-api/)
- [dnvriend/obsidian-search-tool (CLI over Local REST API)](https://github.com/dnvriend/obsidian-search-tool)
- [udus122/dataview-publisher (in-Obsidian export)](https://github.com/udus122/dataview-publisher)
- [intellectronica/mdbasequery (Obsidian Bases-compatible CLI)](https://github.com/intellectronica/mdbasequery)
- [Obsidian CLI help](https://obsidian.md/help/cli)
- [Obsidian Headless help](https://obsidian.md/help/headless)
- [obsidianmd/obsidian-headless](https://github.com/obsidianmd/obsidian-headless)
- [Dataview JavaScript API overview](https://blacksmithgu.github.io/obsidian-dataview/api/intro/)
- [Dataview codeblock/API reference](https://blacksmithgu.github.io/obsidian-dataview/api/code-reference/)
- [Dataview docs — Structure of a Query](https://blacksmithgu.github.io/obsidian-dataview/queries/structure/)
- [Dataview docs — Data Commands](https://blacksmithgu.github.io/obsidian-dataview/queries/data-commands/)
- [Dataview docs — Sources](https://blacksmithgu.github.io/obsidian-dataview/reference/sources/)
- [Dataview `FullIndex` source](https://github.com/blacksmithgu/obsidian-dataview/blob/master/src/data-index/index.ts)
- [Dataview plugin API source](https://github.com/blacksmithgu/obsidian-dataview/blob/master/src/api/plugin-api.ts)
