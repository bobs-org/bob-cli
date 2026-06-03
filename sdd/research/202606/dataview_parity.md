---
create_time: 2026-06-03
status: research
topic: bob dataview parity with Obsidian Dataview
---
# Research: `bob dataview` Parity with Obsidian Dataview

## Short Answer

`bob dataview` has strong parity for the most important case: DQL block
queries executed through the default `--engine obsidian` path. That path calls
the real Dataview plugin inside a running Obsidian desktop process, so DQL
parsing, source resolution, expressions, functions, data commands, and
Dataview result construction are handled by Dataview itself.

It does not have full parity with everything Obsidian Dataview can do. The
main gaps are:

- no CLI surface for inline DQL expressions;
- no CLI surface for DataviewJS blocks or inline DataviewJS;
- no live/interactive Obsidian rendering, especially interactive task views;
- no Markdown output for `CALENDAR`, because Dataview's plugin API does not
  export calendar queries to Markdown;
- `paths` output is a Bob convenience projection, not native Dataview output,
  and some transformed/grouped results have no clean source-note identity;
- `--engine native` is only a narrow local frontmatter subset;
- `--engine dynomark` is an external Dataview-like engine, not Dataview.

The biggest risk is therefore terminology. If "bob dataview parity" means
"full DQL through Obsidian", we are mostly there. If it means "every Dataview
query mode and headless exact behavior", we are not close, and exact headless
parity is probably not a good goal.

## Local State Checked

Observed in this workspace on 2026-06-03:

- `~/bob` is the Obsidian vault, per SASE Obsidian memory.
- Dataview is enabled in `~/bob/.obsidian/community-plugins.json`.
- Local Dataview plugin version is `0.5.68`.
- `bob dataview --help` exposes:
  - `--source`;
  - `--query`;
  - `--query-file`;
  - `--format paths|json|markdown`;
  - `--engine obsidian|dynomark|native`;
  - `--origin`;
  - `--vault`;
  - `--bob-dir`;
  - `--strict-paths`.
- Focused verification passed with `cargo test dataview -- --nocapture`.

## Current Implementation

### Default `obsidian` Engine

The default path is the high-fidelity path. `src/native/dataview.rs` generates
JavaScript and runs:

```text
obsidian [vault=<NAME_OR_ID>] eval code=<generated JavaScript>
```

Inside Obsidian, the generated script finds the Dataview API from
`app.plugins.plugins.dataview?.api`, `window.DataviewAPI`, or
`globalThis.DataviewAPI`; waits briefly for Dataview readiness; and then calls
Dataview's plugin API:

- `api.pagePaths(source)` for `--source`;
- `api.tryQuery(query, origin, { forceId: true })` for structured DQL;
- `api.tryQueryMarkdown(query, origin)` for rendered Markdown.

This means DQL query language parity is delegated to the installed Dataview
plugin. That includes Dataview's own parsing, expression evaluation, source
resolution, functions, data types, implicit metadata, inline fields, tasks, and
query commands.

The command then translates the engine response into one of Bob's output
contracts:

- `paths`: one vault-relative Markdown path per line;
- `json`: stable Bob wrapper containing `engine`, `query_kind`, `format`,
  extracted `paths`, raw-ish Dataview `result`, and `warnings`;
- `markdown`: Dataview-rendered Markdown for DQL.

The exact Dataview result is available under `.result` in JSON output, so when
`paths` cannot represent a query faithfully, `--format json` is the escape
hatch.

### `native` Engine

The native engine is intentionally tiny. It reads Markdown files locally and
parses only top-of-file frontmatter scalars. It supports:

- `LIST`;
- limited `TABLE field, parent.field`;
- optional quoted folder sources like `FROM "ref"`;
- `WHERE` expressions built from:
  - field truthiness;
  - `field = true|false`;
  - `field = "string"`;
  - `field = [[wikilink]]`;
  - `AND`;
  - `OR`;
  - parentheses;
  - chained parent/wikilink traversal.

It does not parse Dataview inline fields, list/task metadata, nested YAML as
objects, YAML lists, dates, durations, numbers, tags, implicit file metadata, or
most of DQL.

### `dynomark` Engine

The dynomark engine shells out to:

```text
dynomark --query <DQL> --metadata
```

from the vault root and extracts paths from emitted metadata. This is a useful
headless fallback, but it is not Obsidian Dataview. Bob correctly warns that it
is partial compatibility, rejects `--source`, and rejects `--format markdown`.

## Obsidian Dataview Surface Area

Dataview itself has four query modes:

1. DQL code blocks.
2. Inline DQL expressions.
3. DataviewJS code blocks.
4. Inline DataviewJS expressions.

DQL blocks have four query types:

- `LIST`;
- `TABLE`;
- `TASK`;
- `CALENDAR`.

They can use data commands:

- `FROM`;
- `WHERE`;
- `SORT`;
- `GROUP BY`;
- `FLATTEN`;
- `LIMIT`.

They can use Dataview expressions, functions, operators, lambdas, index
expressions, Dataview data types, and implicit metadata.

The default Bob Obsidian engine covers the DQL block mode because it delegates
to Dataview. Bob does not currently expose the other three modes as first-class
CLI inputs.

## Gap Matrix

| Feature area | Current `obsidian` engine | Current `native` engine | Current `dynomark` engine | Gap severity |
| --- | --- | --- | --- | --- |
| Source expressions | Yes via `pagePaths()` | No | No | Low for default, high headless |
| DQL `LIST` | Yes | Narrow subset | Partial, external | Low for default |
| DQL `TABLE` | Yes | Narrow projection subset | Partial, external | Low for default |
| DQL `TASK` | Yes, JSON/paths best effort | No | Partial, external | Medium |
| DQL `CALENDAR` | Yes for JSON/paths, no Markdown | No | No | Medium |
| `FROM` tag/folder/link/source combinations | Yes | Quoted folder only | Different syntax/subset | Medium headless |
| `WHERE`, comparisons, boolean logic | Yes | Tiny subset | Partial | High headless |
| `SORT`, `GROUP BY`, `FLATTEN`, `LIMIT` | Yes | No | Partial | High headless |
| Expressions/functions/lambdas/indexing | Yes | Almost none | Partial | High headless |
| Dataview data types | Yes | Bool/null/string/link-ish only | Partial | High headless |
| Frontmatter fields | Yes | Scalar only | Partial | Medium headless |
| Inline fields | Yes through Dataview index | No | Partial/unclear | High headless |
| Implicit file metadata | Yes | No | Some dynomark-defined metadata | High headless |
| Tasks and list metadata | Yes | No | Partial | High headless |
| `this` / relative link origin | Yes via `--origin` | No real `this` | No | Medium |
| Inline DQL | No CLI mode | No | No | Medium |
| DataviewJS blocks | No CLI mode | No | No | High if needed |
| Inline DataviewJS | No CLI mode | No | No | Medium/high |
| Live Dataview rendering | No, one-shot CLI | No | No | Usually acceptable |
| Interactive task checkboxes | No | No | No | Low/medium |
| Markdown rendering | Yes for list/table/task, not calendar | No | No | Medium |
| Exact headless behavior | No, requires Obsidian app | No | No | Not feasible today |

## Important Nuances

### `paths` Is Not Native Dataview Output

Obsidian Dataview renders views or returns structured query results. Bob's
default `paths` format is a convenience for scripts that need matching source
notes.

That projection is straightforward for source expressions and simple page-level
queries. It gets ambiguous for:

- `WITHOUT ID`;
- `GROUP BY` aggregate rows;
- heavy `FLATTEN` transformations;
- computed rows that no longer carry a source note;
- calendar rows with odd values;
- task groupings where the desired identity is task-level, not note-level.

The implementation already handles many cases by forcing IDs when calling
Dataview, extracting path-like identities, warning in best-effort mode, and
failing under `--strict-paths`. That is the right shape. Some queries simply do
not have a meaningful one-row-to-one-note answer.

### `markdown` Is Not Full Obsidian Rendering

`--format markdown` uses Dataview's Markdown export API. That is useful for
tables, lists, and tasks, but it is not the same as rendering inside Obsidian's
DOM. Dataview's plugin API returns an error for calendar-to-Markdown export.

Interactive task behavior also cannot be preserved in Markdown stdout. In
Obsidian, checking a Dataview task can update the original file; Bob's CLI output
cannot carry that interaction.

### DataviewJS Is a Different Class of Problem

DataviewJS code blocks can run arbitrary JavaScript with access to the Dataview
API and Obsidian plugin environment. The Dataview API includes render methods
such as `dv.table()`, `dv.list()`, `dv.taskList()`, `dv.execute()`,
`dv.executeJs()`, and `dv.view()`.

Some DataviewJS scripts are data-producing and could be made CLI-friendly. Other
scripts are view-producing and depend on DOM containers, Obsidian component
lifecycle, CSS, async file loading, and custom view files. Those do not map
cleanly to stdout.

### Headless Exact Dataview Is the Wrong Target

The default high-fidelity route needs a running Obsidian app because Dataview is
an Obsidian plugin and relies on Obsidian's app/plugin/runtime/index. Obsidian
Headless is useful for Sync/services workflows, but it is not a community plugin
runtime.

Trying to make Rust `native` or dynomark exactly match Dataview would mean
reimplementing a mature TypeScript plugin: parser, source resolver, expression
engine, function library, type system, metadata index, task/list model, rendering
semantics, and compatibility bugs. That is a large ongoing maintenance burden.

## Steps to Fill Gaps

### 1. Clarify the Contract

Update docs/help language from "exact Dataview runtime" to something more
precise:

- `--engine obsidian` has exact DQL evaluation because it calls Dataview.
- The command supports source expressions and DQL inputs.
- It does not execute inline Dataview syntax or DataviewJS as native Obsidian
  code blocks.
- `native` and `dynomark` are explicit non-parity headless modes.

Estimated work: 0.5 day.

### 2. Add a Parity Smoke Suite

Create a small fixture vault and a manual/live test checklist comparing:

- source expression;
- `LIST`;
- `TABLE`;
- `TASK`;
- `CALENDAR` JSON;
- `SORT`;
- `GROUP BY`;
- `FLATTEN`;
- inline field usage;
- `this` via `--origin`;
- a query where `paths` must warn.

Automated tests should continue using fake `obsidian` binaries. A live suite
can be documented and optionally gated behind an environment variable because it
requires a running desktop Obsidian session.

Estimated work: 1-2 days.

### 3. Add Inline DQL Expression Mode

Dataview's plugin API exposes expression evaluation methods. A new CLI mode
could run one expression in an origin file context:

```text
bob dataview --expression 'this.file.name' --origin Home.md --format json
```

This would cover Obsidian inline DQL use cases that are expression-shaped, not
table/list/task/calendar query-shaped. It should output JSON by default or a
small scalar format if needed.

Estimated work: 1-2 days.

### 4. Consider Data-Only DataviewJS

Add an explicit JavaScript mode only if there is a real use case:

```text
bob dataview --js 'return dv.pages("#project").where(p => p.status === "active").map(p => p.file.path)'
bob dataview --js-file query.dvjs
```

Implementation should run inside Obsidian, provide a limited `dv` object or
plugin-facing API wrapper, await the result, serialize it with the existing
plain-value serializer, and refuse DOM-rendering APIs unless a later phase
supports them.

This mode needs clear security language. DataviewJS can access the Obsidian
plugin environment and can mutate files if the script chooses to.

Estimated work: 3-6 days for data-only JavaScript.

### 5. Do Not Chase Full DataviewJS Rendering Yet

Full DataviewJS rendering would require constructing a container in Obsidian,
calling Dataview renderer methods with a valid component/context, waiting for
async render completion, and serializing DOM or Markdown-like output. `dv.view()`
adds more file loading and CSS behavior.

This is possible to spike, but it is brittle and not obviously useful for shell
automation.

Estimated work: 2-6 weeks, with ongoing fragility.

### 6. Keep Improving `paths` Only Where Source Identity Exists

Path extraction can be improved around known Dataview result shapes, but it
should not pretend every DQL result has source-note identity. Good next steps:

- add live examples for grouped and flattened rows;
- ensure `--strict-paths` catches ambiguous projections;
- document that `--format json` is the correct output for aggregate/table data;
- maybe add a raw result-only JSON mode if scripts dislike Bob's wrapper.

Estimated work: 1-3 days depending on scope.

### 7. Avoid Full Native Reimplementation

A better native engine could parse YAML with a real YAML parser, support inline
fields, tags, file metadata, simple sources, `SORT`, and `LIMIT`. That might be
worth doing for Bob-specific automation.

But full Dataview parity would require:

- Dataview source parser;
- DQL parser;
- expression AST;
- Dataview comparison semantics;
- function library;
- date/duration/link/list/object types;
- metadata index over frontmatter, inline fields, tags, tasks, lists, links;
- task hierarchy and line/block metadata;
- renderer/export semantics;
- compatibility tests against Dataview 0.5.68 and future versions.

Estimated work:

- useful Bob-specific native expansion: 1-3 weeks;
- broad DQL reimplementation: 2-4 months;
- exact parity: ongoing product-level maintenance.

## Work Estimate Summary

| Work item | Size | Estimate |
| --- | --- | --- |
| Clarify docs/help contract | Small | 0.5 day |
| Add parity smoke suite | Small/Medium | 1-2 days |
| Inline DQL expression mode | Small/Medium | 1-2 days |
| Path extraction polish/raw result mode | Small/Medium | 1-3 days |
| Data-only DataviewJS mode | Medium | 3-6 days |
| Better Bob-specific native subset | Medium/Large | 1-3 weeks |
| Full DataviewJS DOM/render capture | Large/XL | 2-6 weeks |
| Full native Dataview parity | XL | months plus ongoing maintenance |
| Exact headless Dataview plugin runtime | Not recommended | no clear feasible path |

## Sources

Local sources:

- `src/native/dataview.rs`
- `docs/dataview.md`
- `tests/cli.rs`
- `sdd/research/202606/dataview_cli_commandline.md`
- `sdd/epics/202606/dataview_mvp.md`

External sources checked:

- Dataview query types:
  https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/
- Dataview query structure and data commands:
  https://blacksmithgu.github.io/obsidian-dataview/queries/structure/
- Dataview DQL, JS, and inline modes:
  https://blacksmithgu.github.io/obsidian-dataview/queries/dql-js-inline/
- Dataview expressions:
  https://blacksmithgu.github.io/obsidian-dataview/reference/expressions/
- Dataview functions:
  https://blacksmithgu.github.io/obsidian-dataview/reference/functions/
- Dataview data types:
  https://blacksmithgu.github.io/obsidian-dataview/annotation/types-of-metadata/
- Dataview JavaScript API overview and codeblock reference:
  https://blacksmithgu.github.io/obsidian-dataview/api/intro/
  https://blacksmithgu.github.io/obsidian-dataview/api/code-reference/
- Dataview plugin API source:
  https://raw.githubusercontent.com/blacksmithgu/obsidian-dataview/master/src/api/plugin-api.ts
- Obsidian community plugin listing:
  https://community.obsidian.md/plugins/dataview
- Obsidian CLI docs:
  https://obsidian.md/help/cli
- dynomark:
  https://github.com/k-lar/dynomark

## Recommended Solution

Do not try to make `--engine native` or `--engine dynomark` fully Dataview
compatible. Keep `--engine obsidian` as the canonical path for real Dataview DQL,
because it already calls the installed Dataview plugin.

The pragmatic next step is:

1. Document the contract precisely: Bob has exact DQL evaluation through
   Obsidian, not full Dataview UI/JS parity and not exact headless parity.
2. Add a small parity smoke suite so regressions are visible.
3. Add inline DQL expression support if shell workflows need it.
4. Add data-only DataviewJS only after a concrete use case appears.
5. Keep the native engine as a Bob-specific frontmatter/query subset, not a
   Dataview clone.

That gets the valuable 80-90% with low maintenance cost and avoids turning
`bob-cli` into a second Dataview implementation.
