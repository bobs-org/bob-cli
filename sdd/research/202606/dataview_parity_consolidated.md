---
create_time: 2026-06-03
status: research
topic: Consolidated research on bob dataview parity with Obsidian Dataview
---
# Research: `bob dataview` Parity with Obsidian Dataview

## Answer

`bob dataview` has strong parity for the highest-value path, but not full
parity with every Obsidian Dataview feature.

The default `--engine obsidian` path runs inside a live desktop Obsidian process
and calls the installed Dataview plugin API. For DQL source expressions and DQL
block queries, that means parsing, source resolution, expression evaluation,
functions, data commands, implicit metadata, tasks, and result construction are
handled by Dataview itself.

The gaps are elsewhere:

- the CLI supports source expressions and DQL queries, but not inline DQL,
  DataviewJS blocks, or inline DataviewJS as first-class input modes;
- `paths` output is a Bob projection over Dataview results, not a native
  Dataview output format, and some queries do not have a clean source-note
  identity;
- `markdown` output uses Dataview's Markdown export API, not Obsidian's live DOM
  renderer, and Dataview cannot export `CALENDAR` queries to Markdown;
- interactive task behavior is not representable on stdout;
- the headless engines are not the installed Obsidian plugin runtime: `native`
  now covers Bob's supported source-expression and DQL shell surface broadly,
  while `dynomark` remains a partial external Dataview-like fallback until the
  final engine cutover removes it.

So the precise conclusion is: **Bob has exact DQL evaluation when the Obsidian
engine can reach a running desktop Obsidian app; Bob has practical native
headless parity for its supported shell contract, but not full Dataview UI/JS or
installed-plugin parity.**

## Verified Current State

Checked in this workspace on 2026-06-03:

- `~/bob` is the Obsidian vault.
- `~/bob/.obsidian/community-plugins.json` enables `dataview`.
- `~/bob/.obsidian/plugins/dataview/manifest.json` reports Dataview `0.5.68`.
- `bob dataview --help` exposes `--source`, `--query`, `--query-file`,
  `--format paths|json|markdown`, `--engine obsidian|dynomark|native`,
  `--origin`, `--vault`, `--bob-dir`, and `--strict-paths`.
- `docs/dataview.md` says `bob dataview` does not run `ob sync` or
  `ob sync-status`; vault freshness is owned by the external background or cron
  sync path.
- Native Phase 8 hardening added real-vault smoke coverage and cached native
  indexing regexes after a simple `~/bob` source query timed out in debug due
  to per-line regex compilation.

The implementation is in `src/native/dataview.rs`. The default engine generates
JavaScript and runs:

```text
obsidian [vault=<NAME_OR_ID>] eval code=<generated JavaScript>
```

The generated code locates `app.plugins.plugins.dataview?.api`,
`window.DataviewAPI`, or `globalThis.DataviewAPI`, waits briefly for the
Dataview index, then calls:

- `api.pagePaths(source)` for `--source`;
- `api.tryQuery(query, origin, { forceId: true })` for structured DQL;
- `api.tryQueryMarkdown(query, origin)` for Markdown output.

The command's output contracts are Bob-specific:

- `paths`: one vault-relative Markdown path per line, best effort unless
  `--strict-paths` is used;
- `json`: a stable wrapper containing `engine`, `query_kind`, `format`,
  extracted `paths`, the Dataview or engine `result`, and `warnings`;
- `markdown`: Dataview-rendered Markdown for DQL through Obsidian, or native
  Markdown export for native `LIST`, `TABLE`, and `TASK`.

## Obsidian Dataview Surface

Dataview has four user query modes:

1. DQL code blocks.
2. Inline DQL expressions.
3. DataviewJS code blocks.
4. Inline DataviewJS expressions.

DQL itself has four query types:

- `LIST`
- `TABLE`
- `TASK`
- `CALENDAR`

DQL queries can use data commands such as `FROM`, `WHERE`, `SORT`, `GROUP BY`,
`FLATTEN`, and `LIMIT`. Sources include tags, folders, specific files, incoming
links, outgoing links, and boolean combinations. Dataview also indexes
frontmatter, inline fields, implicit `file.*` metadata, tasks, lists, tags,
links, and typed values such as numbers, dates, durations, arrays, objects,
links, booleans, and nulls.

The default Bob Obsidian engine covers DQL block-style queries by delegating to
Dataview. It does not expose inline DQL or DataviewJS as separate CLI modes.

## Gap Inventory

| Area | Current behavior | Remaining boundary | Practical severity |
| --- | --- | --- | --- |
| DQL source expressions | Exact via Obsidian; native supports tags, folders/files, incoming/outgoing links, boolean algebra, and parentheses | Native quoted sources resolve an exact note before folder matching when both exist | Low |
| DQL `LIST` / `TABLE` | Exact via Obsidian; native supports computed expressions, `WITHOUT ID`, identity-aware paths, JSON, and Markdown | Formatting can differ from installed Dataview/plugin settings | Low |
| DQL `TASK` | Exact structured result via Obsidian; native indexes task/list objects and exports JSON/Markdown | CLI cannot expose interactive task checking | Medium |
| DQL `CALENDAR` | Exact structured result via Obsidian; native emits JSON/path-capable calendar rows | Markdown export fails cleanly because Dataview cannot render calendar queries to Markdown | Low |
| Data commands | Exact via Obsidian; native supports `FROM`, `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`, and `LIMIT` | Some advanced Dataview coercions may still differ | Medium |
| Expressions/functions | Exact via Obsidian; native supports the Bob parity matrix of typed literals, operators, swizzling, lambdas, and common functions | Not a guarantee of every Dataview plugin edge case | Medium |
| Metadata index | Exact via Obsidian; native indexes YAML, inline fields, tags, links, tasks/lists, and implicit `file.*` metadata | File timestamps/bookmark state can differ from a live Obsidian cache | Low |
| `this` / origin | Obsidian forwards `--origin`; native supports `this` against indexed origin notes | Dynomark still warns and ignores origin | Low until dynomark removal |
| Inline DQL | Not exposed | No `--expression` or inline-result mode | Medium if requested |
| DataviewJS | Not exposed | No `--js` / `--js-file`; no safe stdout-oriented JS contract | High if requested |
| Live rendering | Not attempted | No DOM renderer, CSS fidelity, lifecycle, live reload, or interactive task state | Usually acceptable |
| Path output | Bob extracts paths from source/list/table/task/calendar-shaped results | Grouped rows and rows without source identity warn or fail under `--strict-paths` | Medium |
| Exact headless Dataview | Native is practical Bob-shell parity, not the installed plugin runtime | Full upstream parity would require ongoing reimplementation of Dataview and Obsidian metadata cache behavior | XL / not recommended |

## Native Engine Detail

The native engine should be described as a Bob-owned headless Dataview
implementation for the CLI's supported shell contract, not as a full Obsidian
Dataview clone.

It now supports:

- source expressions for all pages, tags/subtags, quoted files/folders,
  incoming links, outgoing links, boolean source algebra, and parentheses;
- `LIST`, `TABLE`, `TASK`, and `CALENDAR` DQL;
- ordered data commands: `FROM`, `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`, and
  `LIMIT`;
- typed values for nulls, booleans, numbers, strings, dates/datetimes,
  durations, links, arrays, and objects;
- YAML frontmatter, page inline fields, tags, aliases, outlinks/inlinks,
  tasks/lists, and implicit `file.*` metadata;
- expression evaluation for field access, swizzling, arithmetic/comparison,
  booleans, `this`, lambdas, and the practical Dataview function set used in
  the parity matrix;
- native `paths`/`json` output plus Markdown export for `LIST`, `TABLE`, and
  `TASK`.

It intentionally does not expose DataviewJS, inline DQL modes, live DOM
rendering, CSS/view semantics, or interactive task state. Its Markdown output
targets stable shell output, not pixel-perfect live Dataview rendering.

## Dynomark Detail

The dynomark engine is now an obsolete partial fallback retained only until the
final engine cutover. Upstream describes it as a Markdown query language engine
similar to Dataview and "barebones for now." It is not the Obsidian Dataview
plugin runtime and is no longer the preferred headless path now that the native
engine covers the supported Bob shell surface.

Bob's integration is correctly conservative:

- explicit opt-in with `--engine dynomark`;
- DQL only, no `--source`;
- `paths` and `json`, no Dataview-rendered Markdown;
- compatibility warning in JSON/stderr;
- no Obsidian `--origin` semantics.

Dynomark should not be treated as parity. Phase 9 is expected to remove it after
native parity hardening is accepted.

## Work to Fill the Gaps

### Small, high-value work

1. **Keep the contract explicit in docs/help**: Obsidian engine is exact for
   installed-plugin DQL evaluation; native is practical headless parity for
   Bob's supported shell contract; Bob output formats are Bob-specific. The
   parity language should stay precise as features are added.
   - Estimate: 0.5 day when docs drift.

2. **Maintain the parity smoke suite**: keep the deterministic fixture vault,
   fake-Obsidian protocol goldens, generated native source smoke test, gated
   live fixture harness, and documented `~/bob` native smoke checklist current
   as the engine changes.
   - Estimate: 0.5-1 day when behavior changes.

3. **Polish path extraction only where source identity really exists**: add live
   examples for grouped/flattened/task/calendar results, keep `--strict-paths`
   strict, and consider a raw result-only JSON mode if scripts dislike the Bob
   wrapper.
   - Estimate: 1-3 days.

4. **Add inline DQL expression mode if useful**: Dataview's plugin API exposes
   `evaluate`, `tryEvaluate`, and `evaluateInline`. A CLI mode such as
   `bob dataview --expression 'this.file.name' --origin Home.md --format json`
   would cover inline DQL-shaped shell use without pretending to render a note.
   - Estimate: 1-2 days.

### Medium work

5. **Add data-only DataviewJS only for a concrete use case**: a mode such as
   `--js` / `--js-file` could run inside Obsidian and serialize returned data.
   This needs security language because DataviewJS has access to the Obsidian
   plugin environment and can read or mutate files. DOM-rendering APIs such as
   `dv.table()` and `dv.taskList()` should be rejected or out of scope for this
   mode.
   - Estimate: 3-6 days for a data-returning subset.

6. **Tighten native edge cases only when real queries need them**: examples
   include exact Dataview coercion edge cases, installed-plugin display
   settings, or ambiguous file-vs-folder source behavior. Avoid broad upstream
   parity work without a concrete Bob workflow.
   - Estimate: varies by edge case.

### Large or not recommended

7. **Full DataviewJS rendering**: requires real Obsidian DOM containers,
   component lifecycle, async render completion, CSS/view semantics, and
   `dv.view()` behavior. This is possible to spike but brittle for a CLI.
   - Estimate: 2-6 weeks plus ongoing fragility.

8. **Full native Dataview parity**: requires a mature parser, source resolver,
   expression evaluator, type system, coercion/comparison semantics, function
   library, metadata index, task/list model, link graph, renderer/export
   semantics, compatibility tests, and upstream tracking.
   - Estimate: months plus ongoing maintenance. Not recommended.

## Recommended Solution

Do **not** pursue full native or dynomark parity with Obsidian Dataview. Keep
`--engine obsidian` as the canonical exact-DQL path, because it already uses the
installed Dataview plugin.

The pragmatic path is:

1. Preserve precise docs: exact DQL through Obsidian, practical native headless
   parity for Bob's shell contract, Bob-specific path/json/markdown output.
2. Keep the fixture/live parity smoke suite so regressions in the Obsidian
   engine and path extraction are visible.
3. Add inline DQL expression support only if a real shell workflow needs it.
4. Keep native compatibility demand-driven rather than chasing every upstream
   Dataview edge case.
5. Consider data-only DataviewJS later, but avoid DOM/render capture unless it
   becomes a hard requirement.

This keeps the valuable behavior and avoids turning `bob-cli` into a perpetual
upstream Dataview reimplementation. The practical gaps worth closing are
incremental and workflow-driven; the theoretical gap of exact headless
installed-plugin parity is too large for the likely return.

## Sources

Local:

- `src/native/dataview.rs`
- `docs/dataview.md`
- `README.md`
- `tests/cli.rs`
- `sdd/research/202606/dataview_cli_commandline.md`

External:

- Dataview query modes:
  https://blacksmithgu.github.io/obsidian-dataview/queries/dql-js-inline/
- Dataview query types:
  https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/
- Dataview data commands:
  https://blacksmithgu.github.io/obsidian-dataview/queries/data-commands/
- Dataview sources:
  https://blacksmithgu.github.io/obsidian-dataview/reference/sources/
- Dataview metadata and types:
  https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-pages/
  https://blacksmithgu.github.io/obsidian-dataview/annotation/types-of-metadata/
- Dataview plugin API:
  https://raw.githubusercontent.com/blacksmithgu/obsidian-dataview/master/src/api/plugin-api.ts
- Obsidian CLI:
  https://obsidian.md/help/cli
- dynomark:
  https://github.com/k-lar/dynomark
