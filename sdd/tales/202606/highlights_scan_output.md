---
create_time: 2026-06-15 08:39:27
status: done
prompt: sdd/prompts/202606/highlights_scan_output.md
---
# Plan: Beautiful, concise `bob highlights scan` output

## Goal

Redesign the terminal output of `bob highlights scan` so it is **concise, scannable, and beautiful** — modeled on the
existing `bob projects sync` output (one aligned, colored line per item of interest, followed by a single `·`-separated
summary line).

Today `scan` dumps a verbose key/value report: a multi-line config header, ~12 indented `key: value` lines **per PDF**
(even for PDFs where nothing changed), and a ~12-line summary block. For a 3-PDF library that is ~50 lines of
mostly-noise. We want the common case to be a handful of lines that immediately answer "what changed and what's about to
be written?".

This is purely a presentation change. No sync logic, planning, or write behavior changes.

## Reference: what `projects sync` does well (the model)

`src/native/projects.rs` (`print_sync_report` + `SyncEvent::render`, ~line 2132) produces:

```
  ok   Deep Learning      status: wip -> done      ^prj task checked
  ok   Webserver          added [p::2] to ^prj     unprioritized open tasks exist
3 projects · 1 status updated · 2 ^prj edited · 0 warnings
```

Key properties we will copy:

- **Only emits a line for things that changed.** Unchanged projects produce no line.
- **Aligned columns** via a cyan, right-padded name column (`pad_right` + `display_width`).
- **A `Styler`** that auto-detects TTY + `NO_COLOR`, so color is on for humans and off when piped/redirected (this is
  why tests, which capture non-TTY stdout, see plain text).
- **One terse `·`-separated summary line** at the end.

## Current state (before)

`bob highlights scan --dry-run` over 3 PDFs (one create, one update+marker, one unchanged):

```
Highlights reference sync
operation: scan
bob_dir: /home/bryan/bob
lib_dir: /home/bryan/bob/lib
ref_dir: /home/bryan/bob/ref
managed_body_begin: <!-- highlights:begin -->
managed_body_end: <!-- highlights:end -->
pipeline_fields_excluded_from_marker_sync: source_pdf,source_pdf_sha256,...,pipeline_version
dry_run: true
write_pdfs: false
ob_sync: not-run
pdf_count: 3
pdf: /home/bryan/bob/lib/Deep Learning.pdf
  note: /home/bryan/bob/ref/Deep Learning.md
  sidecar: none
  sync_source: auto-merge
  sync_reason: frontmatter changed
  pdf_task: missing
  note_action: create
  pdf_marker_action: would-update
  highlights_count: 12
  annotation_tasks_create: 1
  annotation_tasks_skip: 0
  routed_task_note_writes: 0
pdf: /home/bryan/bob/lib/Attention Is All You Need.pdf
  ... (12 more lines)
pdf: /home/bryan/bob/lib/Old Paper.pdf
  ... (12 more lines, even though nothing changed)
summary:
  notes_create: 1
  notes_update: 1
  notes_unchanged: 1
  annotation_tasks_create: 1
  annotation_tasks_skip: 0
  routed_task_note_writes: 0
  pdf_markers_would_update: 1
  pdfs_planned: 3
  plan_failures: 0
  scan_failures: 0
writes: none
```

Relevant code (all in `src/native/highlights_ref/mod.rs`):

- `scan_library` (~586) drives output: calls `print_config_report` (3862), then `print_scan_plan_entry` (1268) per PDF,
  then `print_scan_plan_summary` (1322) / `print_scan_write_summary` (1368).
- `print_config_report` is **shared** with `sync`, `marker`, and `doctor` — it must stay intact; `scan` will simply stop
  calling it.

## Target design (after)

### Default (concise) mode — dry run

```
Scanning 3 PDFs in lib · dry-run

  [dry-run] ok  Attention Is All You Need  would update note + marker  8 highlights · auto-merge (frontmatter changed)
  [dry-run] ok  Deep Learning              would create note           12 highlights · +1 task
  3 pdfs · 1 created · 1 updated · 1 unchanged · 1 marker · 1 task · writes: none
```

`Old Paper` (unchanged: note action `none`, no marker, no tasks) produces **no line** — it is only reflected in the
`1 unchanged` count.

### Default (concise) mode — write run

```
Scanning 3 PDFs in lib

  ok  Attention Is All You Need  updated note + marker  8 highlights
  ok  Deep Learning              created note           12 highlights · +1 task
  3 pdfs · 1 created · 1 updated · 1 unchanged · 1 marker · 1 task · writes: note,pdf
```

### Failures (default mode)

Per-PDF failures render inline with a red `error` prefix; the command still exits non-zero with the existing
partial-failure error (printed to stderr unchanged):

```
  ok     Deep Learning   created note   12 highlights
  error  Corrupt Paper   failed to read marker note: invalid annotation object
  2 pdfs · 1 created · 0 updated · 0 unchanged · 0 markers · 0 tasks · 1 failure · writes: note
```

### Line anatomy

`  {prefix}  {name}  {action}  {detail}`

- **prefix** — reuses the `projects sync` vocabulary: green `ok` / `[dry-run] ok` for success, red `error` for a failed
  PDF. Padded to a fixed width so the name column always aligns even when ok/error lines are mixed (a small refinement
  over `projects sync`).
- **name** — the note's display name (file stem, e.g. `Deep Learning`), cyan, right-padded to the widest shown name.
  Full paths move to verbose mode.
- **action** — human verb describing the note + marker change, tense following dry-run like `projects sync` does
  (`would create note`, `created note`, `would update note + marker`, `updated marker`).
- **detail** — dim, compact context joined by `·`: highlight count (`12 highlights`), tasks created (`+1 task`), and the
  sync source **only when it is interesting** (i.e. `auto-merge (<reason>)`; the ordinary `marker` source is omitted to
  reduce noise).

### Summary line

`{N} pdfs · {C} created · {U} updated · {X} unchanged · {M} marker(s) · {T} task(s) · writes: {none|note|pdf|note,pdf}`

`· {F} failure(s)` is appended (red) only when there are failures. The `writes:` token is kept because it is genuinely
useful (did anything get written?) and singular/plural nouns are chosen for polish.

### Header

A single dim line replaces the 8-line config dump: `Scanning {N} PDFs in {lib_dir} · dry-run` (the `· dry-run` suffix
only in dry-run mode). The internal `managed_body_*` markers and `pipeline_fields_excluded_from_marker_sync` are dropped
from `scan` output entirely — they are implementation details with no value to a human reading scan results (they remain
available via `doctor`/`marker`, which still use `print_config_report`).

### Verbose escape hatch: `-v` / `--verbose`

Add a `--verbose` / `-v` flag to `scan`. In verbose mode, `scan` prints **today's exact detailed output** (config
header + per-PDF `key: value` blocks for _every_ PDF including unchanged ones + the detailed `summary:` block +
`writes:` line). This:

- preserves a debugging view for when a sync does something surprising (sync_source, sync_reason, status_normalization,
  pdf_task contribution, routed task writes, etc.), and
- lets the large existing test suite keep its detailed assertions by simply adding `--verbose` (see Test strategy),
  which de-risks the change substantially.

Per `memory/long/cli_rules.md`: `--verbose` gets short alias `-v`, the scan options list stays alphabetically sorted,
and `-h/--help` text is updated to describe it clearly.

## Technical approach

### 1. Extract a shared `Styler` into `src/native/style.rs` (new `pub(crate)` module)

`Styler` (color detection, ANSI `paint`, `cyan/green/yellow/blue/red/dim`, `separator`, `success_prefix`,
`warning_prefix`, success/warning/error labels) currently lives privately in `projects.rs`, with a near-duplicate in
`nightly.rs`. To get `scan` looking _identical_ to `projects sync` without copy-pasting a third `Styler`, move the
generic primitives plus the `pad_right` / `display_width` helpers into a new `src/native/style.rs`, declared
`mod style;` in `src/native.rs`, exposed as `pub(crate)`.

- `projects.rs` switches to `crate::native::style::Styler`. Its only `ProjectStatus`-specific method (`status()`) and
  its project-flavored label helpers stay in `projects.rs` as a small local extension trait (e.g. `ProjectStyleExt`) so
  existing call sites and the exact `projects sync` output are unchanged.
- `nightly.rs` is **left as-is** (out of scope; noted as optional future cleanup) to keep this change focused.
- `highlights_ref` uses the shared `Styler` directly.

This guarantees visual consistency by construction and removes one of the two duplications.

### 2. Add a `ScanEvent` rendering abstraction in `highlights_ref`

Mirror `SyncEvent::render`. Introduce a small enum (e.g. `ScanLine`) with
`Synced { name, action, marker_changed, highlights, tasks, source }` and `Failed { name, error }` variants and a
`render(name_width, dry_run, styler) -> String` method. `scan_library` builds the list of lines from the existing
`PdfSyncPlan` / `SyncWriteReport` / `ScanFailure` data (no new data is needed — every field used already exists),
filtering out unchanged PDFs in default mode.

### 3. Route output by mode in `scan_library`

- Read a new `verbose: bool` (threaded from `run_scan` via the existing `SyncOptions` struct or a small dedicated
  parameter).
- `if verbose` → today's code path (unchanged: `print_config_report` + per-PDF detail + detailed summary). Keep the
  existing `print_scan_*` functions for this.
- `else` → new concise path: dim header, `ScanLine` events for changed/failed PDFs, concise summary line. New private
  helpers (`print_scan_header`, `print_scan_summary_line`).

The summary counts already computed in `print_scan_plan_summary` / `print_scan_write_summary` are reused (extract the
counting into small shared helpers so concise + verbose summaries stay consistent).

### 4. Color correctness

`Styler::detect()` already disables color for non-TTY stdout and honors `NO_COLOR`, so piped output and the test harness
see plain text. No special handling needed.

## Test strategy

All scan output tests live in `tests/cli.rs` (inline `.contains()` assertions; no snapshot framework). Approach:

1. **Preserve detailed-assertion tests via `--verbose`.** The ~10 scan test functions that assert on detailed strings
   (`sync_source: …`, `pdf_marker_action: …`, `annotation_tasks_create(d): …`, `notes_create: …`, `pdfs_planned: …`, the
   many `writes: none`, etc.) get `--verbose` added to their `scan` invocation. Because verbose reproduces today's
   output byte-for-byte, these assertions keep passing with a one-token change.
   `highlights_ref_scan_jobs_flag_matches_sequential_output` continues to compare verbose outputs.
2. **Add new default-mode tests** covering the concise output: created/updated/unchanged suppression, marker update,
   task counts, the dim header, the new summary line (`created`, `updated`, `unchanged`, `writes:`), dry-run vs write
   tense, and an inline `error` line + non-zero exit on a failing PDF.
3. **Update the help test.** `highlights_ref_scan_help_lists_options_alphabetically` must include `-v, --verbose` in the
   expected alphabetically-sorted options.

## Files to change

- `src/native/style.rs` — **new**: shared `pub(crate) Styler` + `pad_right` / `display_width`.
- `src/native.rs` — add `mod style;`.
- `src/native/projects.rs` — use shared `Styler`; keep `ProjectStatus`-specific bits as a local extension trait. No
  output change.
- `src/native/highlights_ref/mod.rs` — add `--verbose/-v` arg; branch `scan_library` output; add `ScanLine` enum +
  `render`; add concise header/summary helpers; keep existing detailed `print_scan_*` functions for verbose. Update scan
  help/`after_help` as needed.
- `tests/cli.rs` — add `--verbose` to existing detailed scan tests; add new concise-mode tests; update the
  alphabetical-options help test.

## Scope / explicitly out of scope

- **Out:** changing `sync`, `doctor`, `marker` output; changing `print_config_report`; migrating `nightly.rs` to the
  shared `Styler`; any JSON/machine-readable mode (no consumer parses scan output today); any change to
  sync/planning/write logic.
- **In:** the default `scan` output redesign, the `--verbose` flag, the shared style module, and the test updates above.

## Design decisions (made on my lead; open to your steer at review)

1. **Concise-by-default + `--verbose` for detail** rather than a hard replacement — keeps a debug view and minimizes
   test/behavior risk. (Alternative: fully replace and rewrite all assertions to concise strings.)
2. **Suppress unchanged PDFs** in default mode (counted only) — this is the single biggest conciseness win and matches
   `projects sync`.
3. **Drop the `managed_body_*` / pipeline-fields header** from scan — pure internal noise for a human; still reachable
   via `doctor`.
4. **Keep the `writes:` token** in the concise summary — genuinely useful and a nice continuity with the current mental
   model.

## Risks

- **Test churn** (~50 assertions). Mitigated by the `--verbose` strategy keeping most assertions intact with a one-token
  edit.
- **`Styler` extraction touching `projects.rs`** (a working command). Mitigated by keeping `projects sync` output
  byte-identical and relying on its existing tests to catch regressions.
- **Alignment width edge cases** (very long PDF names, mixed ok/error prefixes). Mitigated by padding both prefix and
  name columns; reuse `display_width` for correct multibyte widths.

```

```
