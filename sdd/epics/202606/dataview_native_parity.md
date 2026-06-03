---
create_time: 2026-06-03 16:28:01
status: wip
prompt: sdd/prompts/202606/dataview_native_parity.md
---
# Native Dataview Parity Plan

## Objective

Implement enough native `bob dataview` functionality that agents can run the same query surface currently handled by the
Obsidian engine without requiring a running Obsidian app. After native parity is in place, remove the dynomark
integration.

The target is the current `bob dataview` Obsidian-engine contract:

- `--source <SOURCE>` using Dataview source-expression semantics, with `paths` and `json` output.
- `--query <DQL>` and `--query-file <PATH>` for DQL block queries.
- DQL `paths`, `json`, and `markdown` output where the Obsidian Dataview API supports Markdown export.
- `--origin` behavior for `this`, relative links, incoming/outgoing link resolution, and inline expression context.
- `--strict-paths` behavior and the existing Bob JSON wrapper.

This does not expand the CLI to DataviewJS blocks, inline DQL, inline DataviewJS, live DOM rendering, or interactive
task checking. Those are not part of the Obsidian engine surface currently exposed by `bob dataview`.

## Current State

The current default `obsidian` engine calls the live Dataview plugin API:

- `api.pagePaths(source)` for `--source`.
- `api.tryQuery(query, origin, { forceId: true })` for structured DQL JSON.
- `api.tryQueryMarkdown(query, origin)` for DQL Markdown output.

The current `native` engine is much smaller: scalar frontmatter only, `LIST`, limited `TABLE`, one quoted folder source,
field truthiness, equality, `AND`/`OR`, parentheses, and wikilink parent traversal.

The current `dynomark` engine is a partial external fallback. It should remain untouched until the native engine can
replace its practical value, then be deleted in the final cleanup phase.

## Design Direction

Refactor `src/native/dataview.rs` into a small Dataview-native subsystem rather than extending the existing scalar
parser in place. A practical split is:

- `cli`: argument validation, engine dispatch, Bob output wrapper.
- `value`: Dataview value types, comparison, truthiness, JSON/plain rendering.
- `index`: vault scan, page metadata, link graph, aliases, tasks/lists.
- `parse`: source, DQL, and expression parsers.
- `eval`: source resolution, expression evaluation, DQL command pipeline.
- `render`: Markdown export for list/table/task DQL results.
- `oracle_tests`: fixtures and optional live Obsidian comparisons.

Prefer small, focused dependencies over hand-rolling everything:

- Add YAML parsing for frontmatter instead of maintaining a scalar subset.
- Add regex support for tag/link/inline-field scanning and string functions.
- Use existing `chrono` where it is sufficient for date handling; add a duration/date helper crate only if Dataview
  duration semantics become too awkward.
- Keep parsing code in Rust and under test; do not embed a Node/Obsidian runtime for native mode.

## Phases

### Phase 1: Parity Contract and Fixture Vault

Goal: make parity measurable before expanding native behavior.

Scope:

- Add a deterministic fixture vault for Dataview parity tests.
- Cover frontmatter scalars, arrays, objects, aliases, inline fields, tags, links, folders, daily-note filenames, tasks,
  nested tasks, task metadata, dates, durations, and missing/null values.
- Add native golden tests for the current Bob wrapper shape.
- Add an optional live parity harness gated by an environment variable, so a developer with running Obsidian can compare
  `--engine obsidian` and `--engine native` against the same fixture queries.
- Capture goldens for:
  - `--source` tag/folder/file/link/source-algebra queries.
  - `LIST`, `TABLE`, `TASK`, and `CALENDAR` JSON.
  - `paths` extraction, including grouped/flattened cases that warn.
  - Markdown export for `LIST`, `TABLE`, and `TASK`.
  - Calendar Markdown failure.
  - `--origin` and `this`.

Done when:

- The fixture and tests are committed.
- Existing tests still pass.
- Later phases can add native functionality by changing expected failures into passing cases.

Estimate: 1-2 days.

### Phase 2: Native Dataview Index and Value Model

Goal: replace the frontmatter-only native model with enough indexed vault data to evaluate Dataview DQL.

Scope:

- Introduce a `DataviewValue` model for null, booleans, numbers, strings, dates/datetimes, durations, links, arrays, and
  objects.
- Parse YAML frontmatter into Dataview values, including arrays and objects.
- Parse page-level inline fields and duplicate fields.
- Build implicit `file.*` metadata:
  - `name`, `folder`, `path`, `ext`, `link`, `size`, `ctime`, `cday`, `mtime`, `mday`, `tags`, `etags`, `inlinks`,
    `outlinks`, `aliases`, `tasks`, `lists`, `frontmatter`, `day`, and `starred`.
- Build task/list objects with inherited page fields and task implicit fields: `status`, `checked`, `completed`,
  `fullyCompleted`, `text`, `visual`, `line`, `lineCount`, `path`, `section`, `tags`, `outlinks`, `link`, `children`,
  `task`, `annotated`, `parent`, `blockId`, plus date shorthand fields.
- Implement Obsidian-style link normalization well enough for vault-relative paths, bare note names, aliases, subpaths,
  block links, embeds, and ambiguous target warnings.
- Keep hidden directories out of the page scan, but allow reading relevant Dataview/bookmark settings from `.obsidian`
  if present.

Done when:

- Index unit tests pass against the fixture vault.
- Existing native parent-chain behavior still passes.
- JSON serialization of links and dates matches the current Obsidian protocol `plain()` behavior used by Bob.

Estimate: 4-7 days.

### Phase 3: Source Expressions and DQL Parser

Goal: parse the query language surface used by the Obsidian engine without evaluating every expression yet.

Scope:

- Implement source-expression parsing/evaluation for:
  - Empty source/all pages.
  - Tags and subtags.
  - Quoted folders.
  - Quoted single files.
  - Incoming links via `[[note]]`.
  - Outgoing links via `outgoing([[note]])`.
  - Boolean source algebra: `and`, `or`, unary `-`, and parentheses.
- Allow `--engine native --source ...` and produce the same Bob `paths`/`json` wrapper as the Obsidian engine.
- Implement DQL parser support for:
  - Query types `LIST`, `TABLE`, `TASK`, `CALENDAR`.
  - `WITHOUT ID`.
  - `AS` aliases.
  - Query fields/expressions.
  - Ordered data commands: `FROM`, `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`, `LIMIT`.
- Preserve high-quality usage and parse errors.

Done when:

- Native source-expression tests pass.
- DQL parser tests pass for representative valid/invalid queries.
- No DQL functionality regresses from the current native subset.

Estimate: 4-6 days.

### Phase 4: Expression Engine Core

Goal: evaluate Dataview expressions used in query fields and data commands.

Scope:

- Implement literals: null, booleans, numbers, strings, dates, durations, links, arrays, and objects.
- Implement field access and swizzling over arrays, including `this`, `this.file`, page fields, task fields, and
  `rows.*`.
- Implement operators:
  - Boolean `AND`, `OR`, `!`.
  - Equality and ordering: `=`, `!=`, `<`, `<=`, `>`, `>=`.
  - Arithmetic and string/list behavior for `+`, `-`, `*`, `/`, and related Dataview coercions.
  - Unary negation.
- Implement Dataview truthiness and comparison ordering.
- Implement lambdas for functions such as `filter`, `map`, `any`, `all`, `minby`, and `maxby`.
- Make missing fields evaluate to null rather than hard failures.

Done when:

- Expression unit tests cover type coercion, nulls, dates, durations, links, arrays, objects, lambdas, and swizzling.
- Existing native filters continue to pass.

Estimate: 1-2 weeks.

### Phase 5: Dataview Function Library

Goal: implement the documented DQL function set needed for native parity.

Scope:

- Constructors: `object`, `list`/`array`, `date`, `dur`, `number`, `string`, `link`, `embed`, `elink`, `typeof`.
- Numeric functions: `round`, `trunc`, `floor`, `ceil`, `min`, `max`, `sum`, `product`, `reduce`, `average`, `minby`,
  `maxby`.
- Container/string functions: `contains`, `icontains`, `econtains`, `containsword`, `extract`, `sort`, `reverse`,
  `length`, `nonnull`, `firstvalue`, `all`, `any`, `none`, `join`, `filter`, `unique`, `map`, `flat`, `slice`.
- String functions: `regextest`, `regexmatch`, `regexreplace`, `replace`, `lower`, `upper`, `split`, `startswith`,
  `endswith`, `padleft`, `padright`, `substring`, `truncate`.
- Utility functions: `default`, `display`, `choice`, `hash`, `striptime`, `dateformat`, `durationformat`,
  `currencyformat`, `localtime`, `meta`.
- Function vectorization over arrays where Dataview applies it.

Done when:

- Function tests cover the documented examples that are practical to reproduce.
- Query tests using functions pass in `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`, `LIST`, `TABLE`, `TASK`, and `CALENDAR`
  contexts.

Estimate: 1-2 weeks.

### Phase 6: Native DQL Execution and Result Shapes

Goal: execute parsed DQL against the native index and emit Dataview-like result objects compatible with Bob's existing
path extraction and JSON output.

Scope:

- Implement page-level execution for `LIST`, `TABLE`, and `CALENDAR`.
- Implement task-level execution for `TASK`.
- Apply data commands in Dataview order and allow repeated commands: `FROM`, `WHERE`, `SORT`, `GROUP BY`, `FLATTEN`,
  `LIMIT`.
- Match `api.tryQuery(..., { forceId: true })` semantics for JSON:
  - `LIST` includes source identity even when the query says `WITHOUT ID`.
  - `TABLE` includes source identity even when the query says `WITHOUT ID`.
  - `TASK` returns task rows/groupings with source path/link fields.
  - `CALENDAR` returns date/link/value rows.
- Preserve Bob's `paths` and `--strict-paths` behavior.
- Make grouped and flattened result path extraction warn or succeed according to the same identity rules used by the
  Obsidian engine.

Done when:

- Native DQL `json` and `paths` tests pass for all fixture queries.
- The optional live parity harness shows matching or explicitly documented equivalent result shapes for the fixture
  corpus.

Estimate: 1-2 weeks.

### Phase 7: Native Markdown Export

Goal: support `--engine native --format markdown` for the same DQL cases handled by `api.tryQueryMarkdown`.

Scope:

- Remove the argument-level rejection of native Markdown.
- Render Markdown for `LIST`, `TABLE`, and `TASK` result values.
- Return a Dataview-style query error for `CALENDAR` Markdown, matching the Obsidian API behavior.
- Read Dataview plugin export/display settings from the vault when available; otherwise use stable defaults.
- Keep Markdown output on stdout only; warnings and diagnostics stay on stderr.
- Preserve the existing rejection of `--source --format markdown`.

Done when:

- Native Markdown tests pass for list/table/task fixtures.
- Calendar Markdown fails cleanly with empty stdout.
- The optional live parity harness confirms acceptable Markdown equivalence.

Estimate: 3-5 days.

### Phase 8: Parity Hardening and Real-Vault Smoke Tests

Goal: close mismatches before deleting dynomark or changing defaults.

Scope:

- Run the full local test suite.
- Run the optional live Obsidian parity harness on the fixture vault.
- Run smoke queries against `~/bob` when available:
  - Existing native parent-chain query.
  - Tag/folder source queries.
  - Representative `LIST`, `TABLE`, `TASK`, `CALENDAR`.
  - Markdown list/table/task export.
- Add targeted tests for any bugs found in real-vault smoke runs.
- Update `docs/dataview.md`, `README.md`, and research notes to describe native parity boundaries accurately.

Done when:

- Local tests pass.
- Live fixture parity has no unexplained mismatches.
- Any remaining differences are documented as intentional non-goals, not hidden engine gaps.

Estimate: 3-5 days.

### Phase 9: Dynomark Removal and Engine Cutover

Goal: remove the obsolete headless fallback after native parity is accepted.

Scope:

- Delete `Engine::Dynomark`, dynomark command execution, dynomark parsing, dynomark warnings, dynomark errors, and
  `BOB_DATAVIEW_DYNOMARK_COMMAND`.
- Remove `dynomark` from `--engine` choices and from all help/docs/README text.
- Delete dynomark tests and replace any remaining coverage with native tests.
- Decide and implement the final default:
  - Recommended: make `native` the default engine because agent machines should not require a running Obsidian app.
  - Keep `--engine obsidian` available as an explicit live-plugin oracle and as a fallback for users who want exact
    installed-plugin behavior.
- Keep `--vault` and `BOB_DATAVIEW_VAULT` documented as Obsidian-engine-only.
- Ensure CLI help remains sorted and clear.

Done when:

- `bob dataview --help` no longer references dynomark.
- No dynomark symbols remain in source, tests, docs, or README.
- `cargo test dataview -- --nocapture` and the full suite pass.

Estimate: 1-2 days.

## Sequencing Notes

- Phases should run in order. Phase 1 creates the acceptance rails; phases 2-7 build the native engine; phase 8
  validates; phase 9 removes dynomark.
- Do not delete dynomark early. It is the current explicit headless fallback and should disappear only after native
  parity has been demonstrated.
- Do not change the default engine until the final phase. This avoids breaking users while native behavior is still
  under construction.
- Each phase should leave the repo in a passing-test state.

## Main Risks

- Exact Dataview behavior is broad. Treat parity as parity with the current Bob CLI surface and the documented DQL API,
  not as an implementation of DataviewJS or live Obsidian rendering.
- File timestamps, Obsidian settings, bookmark state, and link resolution can differ between a running Obsidian process
  and a raw filesystem scan.
- Markdown export may differ in small formatting details if the installed Dataview plugin has custom settings. Read
  settings where possible and test against stable defaults.
- Task hierarchy and child-task filtering semantics are easy to get subtly wrong. Keep task fixtures focused and compare
  against the live engine.
- Dataview function semantics are the largest single implementation risk. Implement with a compatibility test matrix
  rather than ad hoc behavior.

## References Used

- Local implementation: `src/native/dataview.rs`
- Local tests: `tests/cli.rs`
- Local docs: `docs/dataview.md`, `README.md`
- Existing research: `sdd/research/202606/dataview_parity_consolidated.md`
- Dataview query modes: https://blacksmithgu.github.io/obsidian-dataview/queries/dql-js-inline/
- Dataview query types: https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/
- Dataview data commands: https://blacksmithgu.github.io/obsidian-dataview/queries/data-commands/
- Dataview metadata and types: https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-pages/
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
  https://blacksmithgu.github.io/obsidian-dataview/annotation/types-of-metadata/
- Dataview functions: https://blacksmithgu.github.io/obsidian-dataview/reference/functions/
- Dataview plugin API: https://raw.githubusercontent.com/blacksmithgu/obsidian-dataview/master/src/api/plugin-api.ts
