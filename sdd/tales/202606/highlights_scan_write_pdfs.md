---
create_time: 2026-06-04 13:41:25
status: done
prompt: sdd/prompts/202606/highlights_scan_write_pdfs.md
---
# Plan: Add `bob highlights scan --write-pdfs`

## Context

`bob highlights scan` recursively processes every PDF under the configured Highlights library and already uses the same
per-PDF planning and write execution path as `bob highlights sync <pdf>`.

The current scan behavior hard-codes `SyncOptions.write_pdf = false`, so any PDF whose selected projection needs marker
write-back fails during planning with:

```text
reference note changed but --write-pdf was not supplied; refusing to update the PDF marker
```

This protects PDFs by default, but it makes legitimate bulk marker write-back tedious because the user must run
`bob highlights sync --write-pdf <pdf>` one PDF at a time.

The CLI-rules memory applies because this adds a CLI option: help output should stay clear and option order should
remain alphabetical.

## Requested Behavior

Add a scan-only option:

```bash
bob highlights scan -w
bob highlights scan --write-pdfs
```

Semantics:

- Without `-w|--write-pdfs`, `scan` keeps the current conservative behavior and refuses per-PDF marker write-back.
- With `-w|--write-pdfs`, `scan` allows marker writes for every planned PDF that needs them, using the existing
  `write_pdf_marker` and `execute_pdf_sync` safeguards.
- `--dry-run` remains read-only even when combined with `--write-pdfs`; it should still report planned
  `pdf_marker_action: would-update` and `writes: none`.
- The flag is plural on `scan` because it can write multiple PDFs, while `sync` keeps the existing singular
  `--write-pdf`.
- Scan should make the enabled state visible in its report, likely as `write_pdfs: true|false`, so a bulk-writing run is
  auditable from command output.

## Current Touch Points

- `src/native/highlights_ref/mod.rs`
  - `run_scan()` currently creates `SyncOptions { dry_run, write_pdf: false, prefer: None }`.
  - `scan_library()` plans all PDFs, prints scan entries, preflights dirty/collision state, and writes valid plans.
  - `plan_pdf_sync()` already gates marker write-back on `options.write_pdf` and `options.dry_run`.
  - `execute_pdf_sync()` already checks that the note and PDF are unchanged before writing and refreshes metadata after
    PDF writes.
  - `with_scan_args()` defines scan options in sorted order: `--bob-dir`, `--dry-run`, `--jobs`, `--lib-dir`,
    `--ref-dir`.
  - `write_pdf_arg()` is sync-specific and singular; scan should get a separate plural helper rather than reusing the
    sync option.
- `tests/cli.rs`
  - Existing scan tests cover recursive writes, dry-runs, partial failures, write failures, `--jobs`, and the current
    no-PDF-write refusal path.
  - `highlights_ref_task_checked_dry_run_requires_and_writes_pdf_marker` is the strongest current fixture for scan
    needing PDF marker write-back.
  - Help ordering is tested for `sync`; add analogous scan help coverage or extend existing help tests.
- `README.md` and `docs/highlights-ref-sync.md`
  - Both list `bob highlights scan [--dry-run]`.
  - The docs currently say scan does not enable PDF marker write-back and instruct users to run targeted
    `sync --write-pdf`; this needs to describe the new bulk opt-in workflow while keeping backup cautions.

## Implementation Steps

1. Add the scan CLI option.
   - Introduce a `write_pdfs_arg()` helper with `.long("write-pdfs")`, `.short('w')`, `ArgAction::SetTrue`, and help
     text such as `Allow marker writes back to all PDFs during scan`.
   - Add it to `with_scan_args()` after `ref_dir_arg()` so long-option help stays alphabetically ordered: `--bob-dir`,
     `--dry-run`, `--jobs`, `--lib-dir`, `--ref-dir`, `--write-pdfs`.

2. Wire scan behavior.
   - Change `run_scan()` to set `write_pdf: matches.get_flag("write-pdfs")`.
   - Keep `prefer: None`; bulk scan should not introduce conflict resolution behavior unless requested separately.
   - In `scan_library()`, print `write_pdfs: {options.write_pdf}` near `dry_run` so reports show whether bulk PDF
     write-back was enabled.

3. Preserve existing safety behavior.
   - Do not change `plan_pdf_sync()` refusal semantics except through the new option value.
   - Do not change `execute_pdf_sync()`; it already performs the PDF unchanged check immediately before marker write and
     records refreshed PDF hash metadata after writes.
   - Keep partial-failure behavior: if some PDFs fail planning or writing, valid planned PDFs may still be written and
     the command exits non-zero with the existing summary.

4. Update tests.
   - Add or extend a scan help test to assert `--write-pdfs` appears after `--ref-dir` and that `-w` is advertised.
   - Extend the checked-task/frontmatter marker-write scenario so:
     - `bob highlights scan` without the new flag still fails and leaves the PDF marker unchanged.
     - `bob highlights scan --write-pdfs` succeeds, updates the PDF marker, updates note metadata as needed, and reports
       `pdf_markers_updated: 1` plus `writes: note,pdf` or the exact existing summary produced by `execute_pdf_sync`.
   - Add a dry-run assertion for `bob highlights scan --dry-run --write-pdfs` if the existing test shape makes that
     cheap, proving `--dry-run` still wins.
   - Keep existing multi-PDF scan tests passing; they should now show `write_pdfs: false` in scan output if that report
     line is added.

5. Update documentation.
   - Update command summaries in `README.md` and `docs/highlights-ref-sync.md` to show:
     `bob highlights scan [--dry-run] [--write-pdfs]`.
   - Replace the old "scan does not enable PDF marker write-back" instructions with a workflow that recommends:
     `scan --dry-run`, review `pdf_markers_would_update`, back up PDFs, then run `scan --write-pdfs`.
   - Keep the caution that scheduled automation should remain dry-run or note-only unless the user intentionally wants
     bulk PDF writes.
   - Update troubleshooting text so `--write-pdf` refusals mention either targeted `sync --write-pdf` or bulk
     `scan --write-pdfs`.

6. Verify.
   - Run `cargo fmt --check`; if formatting is needed, run `cargo fmt` and re-run the check.
   - Run targeted tests:
     - `cargo test highlights_ref_sync_help_lists_options_alphabetically`
     - `cargo test highlights_ref_scan`
     - `cargo test highlights_ref_task_checked_dry_run_requires_and_writes_pdf_marker`
   - If targeted tests expose broader command-help fallout, run `cargo test --test cli highlights_ref`.

## Risks and Edge Cases

- Bulk PDF marker write-back is intentionally higher blast radius than targeted `sync --write-pdf`; the default must
  remain no PDF writes.
- `scan --write-pdfs` with mixed valid and invalid PDFs may write valid PDFs before returning a partial-failure exit
  code. That matches current scan note-write behavior, but the docs should make it explicit for PDF writes too.
- `--write-pdfs` should not imply conflict resolution. PDFs with marker/frontmatter conflicts should still fail unless
  resolved with targeted `sync --prefer ...`.
- `--dry-run --write-pdfs` should never write; tests should guard this because users may run it as the final preview
  before the real bulk write.

## Acceptance Criteria

- `bob highlights scan --help` documents `-w, --write-pdfs` in sorted option order.
- `bob highlights scan` without the flag behaves as it does today when marker write-back is needed.
- `bob highlights scan --write-pdfs` writes needed PDF markers across planned PDFs and reports PDF marker updates in the
  existing scan summary.
- `bob highlights scan --dry-run --write-pdfs` writes nothing.
- README/docs describe the new bulk workflow and retain backup/scheduled-automation cautions.
- Focused highlights tests pass.
