//! Other highlights scan tests.

use crate::support::*;
use std::fs;
use std::process::Command;

#[test]
fn highlights_ref_scan_treats_later_page_note_as_missing_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-page-two-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/page-two-marker.pdf");
    let note = vault.join("ref/books/page-two-marker.md");
    write_highlights_pdf_pages(
        &pdf,
        &[
            &[],
            &["- status: wip\n- parent: obsidian\n- title: Page Two\n"],
        ],
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "later page marker-like note should fail:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&pdf))
            && report.contains("plan_error:")
            && report.contains(
                "no standalone /Text note annotations found on page 1"
            ),
        "expected page-1 missing marker error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "scan must not write a note on marker error");
}

#[test]
fn highlights_ref_scan_recurses_dry_runs_and_writes_multiple_pdfs() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/systems-performance.pdf");
    let second_pdf = vault.join("lib/papers/rust-book.PDF");
    let first_note = vault.join("ref/books/systems-performance.md");
    let second_note = vault.join("ref/papers/rust-book.md");
    write_highlights_pdf(
        &first_pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: ready\n- parent: obsidian\n- title: Rust Book\n",
    );
    write_file(
        &first_pdf.with_extension("md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> First quote.
",
    );
    write_file(
        &second_pdf.with_extension("md"),
        "\
## Page 2

- status: wip
- parent: obsidian

---

> Second quote.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run highlights scan");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(dry_run.contains("pdf_count: 2"), "{dry_run}");
    assert!(
        dry_run.contains(path_str(&first_note))
            && dry_run.contains(path_str(&second_note)),
        "{dry_run}"
    );
    assert!(dry_run.contains("notes_create: 2"), "{dry_run}");
    assert!(dry_run.contains("writes: none"), "{dry_run}");
    assert!(!first_note.exists(), "dry-run must not create first note");
    assert!(!second_note.exists(), "dry-run must not create second note");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write highlights scan");

    assert_success(&output);
    let written = stdout(&output);
    assert!(written.contains("notes_created: 2"), "{written}");
    assert!(written.contains("writes: note"), "{written}");
    let first_contents =
        fs::read_to_string(&first_note).expect("read first note");
    let second_contents =
        fs::read_to_string(&second_note).expect("read second note");
    assert!(
        first_contents.contains("ref_type: books\n")
            && first_contents.contains("> [!quote] First quote.\n"),
        "{first_contents}"
    );
    assert!(
        first_contents.contains("## Highlights\n"),
        "{first_contents}"
    );
    assert!(!first_contents.contains("## Summary\n"), "{first_contents}");
    assert!(
        !first_contents.contains("## My Notes\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n")
            && second_contents.contains("> [!quote] Second quote.\n"),
        "{second_contents}"
    );
    assert!(
        second_contents.contains("## Highlights\n"),
        "{second_contents}"
    );
    assert!(
        !second_contents.contains("## Summary\n"),
        "{second_contents}"
    );
    assert!(
        !second_contents.contains("## My Notes\n"),
        "{second_contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat highlights scan");

    assert_success(&output);
    let repeated = stdout(&output);
    assert!(repeated.contains("notes_unchanged: 2"), "{repeated}");
    assert!(repeated.contains("writes: none"), "{repeated}");
}

#[test]
fn highlights_ref_scan_intakes_xlib_pdf_and_writes_note_in_same_run() {
    let temp = TempDir::new("bob-cli-highlights-ref-xlib-intake");
    let vault = temp.path().join("vault");
    let source_pdf = vault.join("xlib/chat/intake.pdf");
    let destination_pdf = vault.join("lib/chat/intake.pdf");
    let note = vault.join("ref/chat/intake.md");
    write_highlights_pdf(
        &source_pdf,
        "- status: wip\n- parent: obsidian\n- title: Intake PDF\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan xlib intake");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("intake_moves: 1")
            && report.contains(
                "intake: moved xlib/chat/intake.pdf -> lib/chat/intake.pdf"
            )
            && report.contains("notes_created: 1"),
        "expected intake and note write in same scan:\n{report}"
    );
    assert!(!source_pdf.exists(), "scan must move the PDF out of xlib");
    assert!(destination_pdf.is_file(), "scan must move the PDF into lib");
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(
        contents.contains("source_pdf: lib/chat/intake.pdf\n"),
        "{contents}"
    );
    assert!(contents.contains("title: \"Intake PDF\"\n"), "{contents}");
}

#[test]
fn highlights_ref_scan_dry_run_previews_xlib_intake_without_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-xlib-dry-run");
    let vault = temp.path().join("vault");
    let source_pdf = vault.join("xlib/chat/preview.pdf");
    let destination_pdf = vault.join("lib/chat/preview.pdf");
    let note = vault.join("ref/chat/preview.md");
    write_highlights_pdf(
        &source_pdf,
        "- status: wip\n- parent: obsidian\n- title: Preview PDF\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run xlib intake");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("intake_moves: 1")
            && report.contains("intake: would-move xlib/chat/preview.pdf -> lib/chat/preview.pdf")
            && report.contains(path_str(&note))
            && report.contains("notes_create: 1")
            && report.contains("writes: none"),
        "expected dry-run to preview intake and reference note:\n{report}"
    );
    assert!(source_pdf.is_file(), "dry-run must leave PDF in xlib");
    assert!(
        !destination_pdf.exists(),
        "dry-run must not move PDF into lib"
    );
    assert!(!note.exists(), "dry-run must not create note");
}

#[test]
fn highlights_ref_scan_intakes_xlib_sidecars_with_pdfs() {
    let temp = TempDir::new("bob-cli-highlights-ref-xlib-sidecars");
    let vault = temp.path().join("vault");
    let markdown_pdf = vault.join("xlib/chat/markdown.pdf");
    let markdown_sidecar = markdown_pdf.with_extension("md");
    let markdown_note = vault.join("ref/chat/markdown.md");
    let markdown_destination = vault.join("lib/chat/markdown.pdf");
    let bundle_pdf = vault.join("xlib/chat/bundle.pdf");
    let bundle = bundle_pdf.with_extension("textbundle");
    let bundle_sidecar = bundle.join("text.md");
    let bundle_note = vault.join("ref/chat/bundle.md");
    let bundle_destination = vault.join("lib/chat/bundle.pdf");

    write_highlights_pdf(
        &markdown_pdf,
        "- status: wip\n- parent: obsidian\n- title: Markdown Sidecar\n",
    );
    write_file(
        &markdown_sidecar,
        "\
## Page 1

- status: wip
- parent: obsidian

---

> Markdown sidecar quote.
",
    );
    write_highlights_pdf(
        &bundle_pdf,
        "- status: wip\n- parent: obsidian\n- title: Bundle Sidecar\n",
    );
    write_file(
        &bundle_sidecar,
        "\
## Page 2

- status: wip
- parent: obsidian

---

> TextBundle sidecar quote.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan xlib sidecars");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("intake_moves: 2"), "{report}");
    assert!(markdown_destination.is_file(), "markdown PDF moved");
    assert!(
        markdown_destination.with_extension("md").is_file(),
        "markdown sidecar moved"
    );
    assert!(
        !markdown_pdf.exists() && !markdown_sidecar.exists(),
        "markdown intake files should be gone"
    );
    assert!(bundle_destination.is_file(), "bundle PDF moved");
    assert!(
        bundle_destination
            .with_extension("textbundle")
            .join("text.md")
            .is_file(),
        "textbundle sidecar moved"
    );
    assert!(
        !bundle_pdf.exists() && !bundle.exists(),
        "bundle intake files should be gone"
    );

    let markdown_contents =
        fs::read_to_string(&markdown_note).expect("read markdown note");
    assert!(
        markdown_contents
            .contains("highlights_sidecar: lib/chat/markdown.md\n")
            && markdown_contents
                .contains("> [!quote] Markdown sidecar quote.\n"),
        "{markdown_contents}"
    );
    let bundle_contents =
        fs::read_to_string(&bundle_note).expect("read bundle note");
    assert!(
        bundle_contents.contains(
            "highlights_sidecar: lib/chat/bundle.textbundle/text.md\n"
        ) && bundle_contents.contains("> [!quote] TextBundle sidecar quote.\n"),
        "{bundle_contents}"
    );
}

#[test]
fn highlights_ref_scan_refuses_xlib_intake_conflict_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-xlib-conflict");
    let vault = temp.path().join("vault");
    let source_pdf = vault.join("xlib/chat/conflict.pdf");
    let destination_pdf = vault.join("lib/chat/conflict.pdf");
    let note = vault.join("ref/chat/conflict.md");
    write_highlights_pdf(
        &source_pdf,
        "- status: wip\n- parent: obsidian\n- title: Intake Conflict\n",
    );
    write_highlights_pdf(
        &destination_pdf,
        "- status: ready\n- parent: obsidian\n- title: Archived Conflict\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan xlib conflict");

    assert_eq!(
        output.status.code(),
        Some(1),
        "conflicting intake should fail:\n{}",
        format_output(&output)
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("xlib intake collision(s) detected before writes")
            && diagnostic.contains("xlib/chat/conflict.pdf")
            && diagnostic.contains("lib/chat/conflict.pdf"),
        "{diagnostic}"
    );
    assert!(
        source_pdf.is_file(),
        "conflict must leave xlib PDF in place"
    );
    assert!(
        destination_pdf.is_file(),
        "conflict must leave library PDF in place"
    );
    assert!(!note.exists(), "conflict must not write a reference note");
}

#[test]
fn highlights_ref_scan_dry_run_reports_valid_and_invalid_pdfs() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-mixed-dry-run");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/papers/invalid.pdf");
    let valid_note = vault.join("ref/books/valid.md");
    let invalid_note = vault.join("ref/papers/invalid.md");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid PDF\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Missing Status\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("mixed dry-run highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "mixed dry-run scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(report.contains("pdf_count: 2"), "{report}");
    assert!(
        report.contains(path_str(&valid_note))
            && report.contains("notes_create: 1")
            && report.contains("pdfs_planned: 1"),
        "valid PDF should still be planned:\n{report}"
    );
    assert!(
        report.contains(path_str(&invalid_pdf))
            && report
                .contains("plan_error: missing required marker key: status")
            && report.contains("plan_failures: 1")
            && report.contains("scan_failures: 1")
            && report.contains("writes: none"),
        "invalid PDF should be reported without writes:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!valid_note.exists(), "dry-run must not create valid note");
    assert!(
        !invalid_note.exists(),
        "dry-run must not create invalid note"
    );
}

#[test]
fn highlights_ref_scan_writes_valid_pdfs_despite_invalid_pdf() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-mixed-write");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/papers/invalid.pdf");
    let valid_note = vault.join("ref/books/valid.md");
    let invalid_note = vault.join("ref/papers/invalid.md");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid PDF\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Missing Status\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("mixed write highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "mixed write scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&valid_note))
            && report.contains("notes_created: 1")
            && report.contains("write_successes: 1")
            && report.contains("writes: note"),
        "valid PDF should be written:\n{report}"
    );
    assert!(
        report.contains(path_str(&invalid_pdf))
            && report
                .contains("plan_error: missing required marker key: status")
            && report.contains("plan_failures: 1")
            && report.contains("scan_failures: 1"),
        "invalid PDF should be reported:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    let valid_contents =
        fs::read_to_string(&valid_note).expect("read valid note");
    assert!(
        valid_contents.contains("title: \"Valid PDF\"\n"),
        "{valid_contents}"
    );
    assert!(
        !invalid_note.exists(),
        "scan must not create a note for invalid PDF"
    );
}

#[test]
fn highlights_ref_scan_default_output_is_concise() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-concise");
    let vault = temp.path().join("vault");
    let create_pdf = vault.join("lib/books/create-note.pdf");
    let update_pdf = vault.join("lib/books/update-me.pdf");
    let settled_pdf = vault.join("lib/books/settled.pdf");
    let create_note = vault.join("ref/books/create-note.md");
    let update_note = vault.join("ref/books/update-me.md");
    let settled_note = vault.join("ref/books/settled.md");

    write_highlights_pdf(
        &create_pdf,
        "- status: wip\n- parent: obsidian\n- title: Create Note\n",
    );
    write_file(
        &create_pdf.with_extension("md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> Create highlight.
",
    );
    write_highlights_pdf(
        &update_pdf,
        "- status: wip\n- parent: obsidian\n- title: Update Me\n",
    );
    write_highlights_pdf(
        &settled_pdf,
        "- status: wip\n- parent: obsidian\n- title: Settled\n",
    );

    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&update_pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial update sync"),
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&settled_pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial settled sync"),
    );

    let checked_update = fs::read_to_string(&update_note)
        .expect("read update note")
        .replace("- [/] #task", "- [x] #task");
    write_file(&update_note, &checked_update);
    write_file(
        &update_pdf.with_extension("md"),
        "\
## Page 2

- status: wip
- parent: obsidian

---

> Update highlight.

- #task Import this annotation task.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise dry-run scan");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("Scanning 3 PDFs in lib - dry-run"),
        "{dry_run}"
    );
    assert!(
        dry_run.contains("[dry-run] ok  create note")
            && dry_run.contains("would create note")
            && dry_run.contains("1 highlight"),
        "created PDF should render as one concise line:\n{dry_run}"
    );
    assert!(
        dry_run.contains("[dry-run] ok  update me")
            && dry_run.contains("would update note + marker")
            && dry_run.contains("+1 task"),
        "updated PDF should render marker and task context:\n{dry_run}"
    );
    assert!(
        dry_run.contains(
            "3 pdfs - 1 created - 1 updated - 1 unchanged - 1 marker - 1 task - writes: none"
        ),
        "expected concise dry-run summary:\n{dry_run}"
    );
    assert!(
        !dry_run.contains("settled")
            && !dry_run.contains("pdf_count:")
            && !dry_run.contains("sync_source:"),
        "default scan output should suppress unchanged PDFs and verbose keys:\n{dry_run}"
    );
    assert!(!create_note.exists(), "dry-run must not create note");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise write scan");

    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("Scanning 3 PDFs in lib")
            && !written.contains("dry-run"),
        "{written}"
    );
    assert!(
        written.contains("ok  create note")
            && written.contains("created note")
            && written.contains("ok  update me")
            && written.contains("updated note + marker"),
        "write scan should use past-tense concise actions:\n{written}"
    );
    assert!(
        written.contains(
            "3 pdfs - 1 created - 1 updated - 1 unchanged - 1 marker - 1 task - writes: note,pdf"
        ),
        "expected concise write summary:\n{written}"
    );
    assert!(
        !written.contains("settled") && !written.contains("notes_created:"),
        "write scan should keep detailed keys out of default output:\n{written}"
    );
    assert!(create_note.exists(), "write scan should create note");
    assert!(settled_note.exists(), "settled note should remain present");
}

#[test]
fn highlights_ref_scan_default_output_reports_inline_errors() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-concise-errors");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/books/invalid.pdf");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Invalid\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise scan with invalid PDF");

    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid PDF should make scan return non-zero:\n{}",
        format_output(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let report = stdout(&output);
    assert!(
        report.contains("error")
            && report.contains("invalid  missing required marker key: status"),
        "invalid PDF should be rendered inline:\n{report}"
    );
    assert!(
        report.contains(
            "2 pdfs - 1 created - 0 updated - 0 unchanged - 0 markers - 0 tasks - 1 failure - writes: none"
        ),
        "expected concise partial-failure summary:\n{report}"
    );
    assert!(
        !report.contains("plan_error:") && !report.contains(path_str(&invalid_pdf)),
        "default failure output should avoid verbose keys and full paths:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_scan_continues_after_write_failure() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-write-failure");
    let vault = temp.path().join("vault");
    let fail_pdf = vault.join("lib/books/a-fail.pdf");
    let later_pdf = vault.join("lib/books/b-later.pdf");
    let fail_note = vault.join("ref/books/a-fail.md");
    let later_note = vault.join("ref/books/b-later.md");
    write_highlights_pdf(
        &fail_pdf,
        "- status: wip\n- parent: obsidian\n- title: Fails At Write\n",
    );
    write_highlights_pdf(
        &later_pdf,
        "- status: wip\n- parent: obsidian\n- title: Later Still Writes\n",
    );

    let fail_parent = fail_note.parent().expect("fail note parent");
    let fail_name = fail_note.file_name().expect("fail note filename");
    let output = Command::new("sh")
        .arg("-c")
        .arg(
            "set -eu\n\
             mkdir -p \"$FAIL_PARENT\"\n\
             mkdir \"$FAIL_PARENT/.$FAIL_NAME.$$.tmp\"\n\
             exec \"$BOB_BIN\" highlights scan --verbose\n",
        )
        .env("BOB_BIN", BOB_BIN)
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", TEST_MISSING_CONFIG_FILE)
        .env("FAIL_PARENT", fail_parent)
        .env("FAIL_NAME", fail_name)
        .output()
        .expect("write-failure highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "write failure scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&fail_pdf))
            && report.contains("write_failure:")
            && report.contains("write temporary file"),
        "failed PDF should be reported as a write failure:\n{report}"
    );
    assert!(
        report.contains(path_str(&later_note))
            && report.contains("write_successes: 1")
            && report.contains("write_failures: 1")
            && report.contains("scan_failures: 1")
            && report.contains("writes: note"),
        "later valid PDF should still be written:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!fail_note.exists(), "failed note must not be installed");
    let later_contents =
        fs::read_to_string(&later_note).expect("read later note");
    assert!(
        later_contents.contains("title: \"Later Still Writes\"\n"),
        "{later_contents}"
    );
}

#[test]
fn highlights_ref_scan_jobs_flag_matches_sequential_output() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-jobs");
    let vault = temp.path().join("vault");

    // Spread several PDFs across nested ref types so completion order under
    // parallel planning is unlikely to match the sorted reporting order.
    let specs = [
        ("lib/books/alpha.pdf", "wip", "Alpha", "Alpha quote."),
        ("lib/books/beta.pdf", "ready", "Beta", "Beta quote."),
        ("lib/papers/gamma.pdf", "wip", "Gamma", "Gamma quote."),
        ("lib/papers/delta.pdf", "ready", "Delta", "Delta quote."),
        ("lib/notes/epsilon.pdf", "wip", "Epsilon", "Epsilon quote."),
    ];
    for (rel, status, title, quote) in specs {
        let pdf = vault.join(rel);
        write_highlights_pdf(
            &pdf,
            &format!(
                "- status: {status}\n- parent: obsidian\n- title: {title}\n"
            ),
        );
        write_file(
            &pdf.with_extension("md"),
            &format!(
                "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> {quote}\n"
            ),
        );
    }

    let run_scan = |jobs: &str| {
        let output = bob_command()
            .arg("highlights")
            .arg("scan")
            .arg("--verbose")
            .arg("--dry-run")
            .arg("--jobs")
            .arg(jobs)
            .env("BOB_DIR", &vault)
            .output()
            .expect("dry-run highlights scan with --jobs");
        assert_success(&output);
        stdout(&output)
    };

    // Dry-run scan output carries no timestamps, so order-preserving parallel
    // planning must produce byte-identical output regardless of job count.
    let sequential = run_scan("1");
    let parallel = run_scan("4");
    assert_eq!(sequential, parallel, "--jobs must not change scan output");
    assert!(sequential.contains("pdf_count: 5"), "{sequential}");
    assert!(sequential.contains("notes_create: 5"), "{sequential}");

    // Rejecting --jobs 0 keeps the flag meaningful (1 = sequential floor).
    let zero = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--jobs")
        .arg("0")
        .env("BOB_DIR", &vault)
        .output()
        .expect("reject --jobs 0");
    assert!(!zero.status.success(), "--jobs 0 must be rejected");
}

#[test]
fn highlights_ref_scan_allows_duplicate_basenames_in_different_ref_types() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-ref-types");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/example.pdf");
    let second_pdf = vault.join("lib/papers/example.pdf");
    let first_note = vault.join("ref/books/example.md");
    let second_note = vault.join("ref/papers/example.md");
    let old_flat_note = vault.join("ref/example.md");
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: obsidian\n");
    write_highlights_pdf(&second_pdf, "- status: ready\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run duplicate basename scan");

    assert_success(&output);
    let first_contents =
        fs::read_to_string(&first_note).expect("read books note");
    let second_contents =
        fs::read_to_string(&second_note).expect("read papers note");
    assert!(
        first_contents.contains("ref_type: books\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n"),
        "{second_contents}"
    );
    assert!(
        !old_flat_note.exists(),
        "nested references must not also write the old flat note"
    );
}

#[test]
fn highlights_ref_scan_detects_same_target_collision_before_writing() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-collision");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/example.pdf");
    let second_pdf = vault.join("lib/books/example.PDF");
    let note = vault.join("ref/books/example.md");
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: obsidian\n");
    write_highlights_pdf(&second_pdf, "- status: ready\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run collision scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "same target collision should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("output path collision"),
        "expected collision report:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "scan must not write any note when collisions exist"
    );
}

#[test]
fn highlights_ref_scan_groups_routed_tasks_with_parallel_jobs() {
    let temp = TempDir::new("bob-cli-highlights-ref-routed-scan");
    let vault = temp.path().join("vault");
    let route_note = vault.join("alice.md");
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    for (name, title, task) in [
        ("alpha", "Alpha", "Ask Alice about alpha."),
        ("beta", "Beta", "Ask Alice about beta."),
    ] {
        let pdf = vault.join(format!("lib/books/{name}.pdf"));
        write_highlights_pdf(
            &pdf,
            &format!("- status: wip\n- parent: obsidian\n- title: {title}\n"),
        );
        write_file(
            &pdf.with_extension("md"),
            &format!(
                "\
## Page 1

- status: wip
- parent: obsidian

---

> {title} claim.

- #task {task} @alice
"
            ),
        );
    }

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--jobs")
        .arg("2")
        .env("BOB_DIR", &vault)
        .output()
        .expect("parallel routed scan");

    assert_success(&output);
    let contents = fs::read_to_string(&route_note).expect("read route note");
    assert_eq!(
        contents.matches("#task Ask Alice about alpha.").count(),
        1,
        "{contents}"
    );
    assert_eq!(
        contents.matches("#task Ask Alice about beta.").count(),
        1,
        "{contents}"
    );
    assert_text_order(
        &contents,
        &[
            "#task Ask Alice about alpha.",
            "#task Ask Alice about beta.",
        ],
    );
}

#[test]
fn highlights_ref_scan_round_trips_provenance_marker_fields() {
    let temp = TempDir::new("bob-cli-highlights-ref-provenance-round-trip");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/blogs/clipped.pdf");
    let note = vault.join("ref/blogs/clipped.md");
    write_highlights_pdf(
        &pdf,
        "- status: ready\n- parent: obsidian_ref\n- title: Clipped Article\n- id: clipped_article\n- source_url: https://example.com/article\n- author: Jane Doe\n- published: 2026-04-27\n- captured: 2026-10-01\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan provenance marker");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("notes_created: 1"), "{report}");
    let contents = fs::read_to_string(&note).expect("read generated note");
    for needle in [
        "source_url:",
        "https://example.com/article",
        "author:",
        "Jane Doe",
        "published: 2026-04-27",
        "captured: 2026-10-01",
        "id: clipped_article",
    ] {
        assert!(contents.contains(needle), "{needle}:\n{contents}");
    }

    let second = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("rescan provenance marker");

    assert_success(&second);
    let rerun = stdout(&second);
    assert!(
        rerun.contains("notes_created: 0")
            && rerun.contains("notes_unchanged: 1"),
        "{rerun}"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after rescan"),
        contents,
        "second scan must leave the note unchanged"
    );
}

#[test]
fn highlights_ref_scan_stamps_created_on_category_and_intake_notes() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-created");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/scan-created.pdf");
    let second_pdf = vault.join("lib/papers/scan-created.pdf");
    let intake_pdf = vault.join("xlib/chat/scan-intake.pdf");
    let first_note = vault.join("ref/books/scan-created.md");
    let second_note = vault.join("ref/papers/scan-created.md");
    let intake_note = vault.join("ref/chat/scan-intake.md");
    write_highlights_pdf(
        &first_pdf,
        "- status: ready\n- parent: obsidian\n- title: Scan Created One\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: ready\n- parent: obsidian\n- title: Scan Created Two\n",
    );
    write_highlights_pdf(
        &intake_pdf,
        "- status: ready\n- parent: obsidian\n- title: Scan Intake\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-03 15:42:18")
        .env("TZ", "America/New_York")
        .output()
        .expect("dry-run highlights scan");

    assert_success(&output);
    assert!(
        stdout(&output).contains("writes: none"),
        "dry-run scan must stay read-only:\n{}",
        format_output(&output)
    );
    assert!(!first_note.exists(), "dry-run must not create first note");
    assert!(!second_note.exists(), "dry-run must not create second note");
    assert!(!intake_note.exists(), "dry-run must not create intake note");
    assert!(intake_pdf.is_file(), "dry-run must leave PDF in xlib");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-03 15:42:18")
        .env("TZ", "America/New_York")
        .output()
        .expect("write highlights scan");

    assert_success(&output);
    assert!(
        stdout(&output).contains("notes_created: 3"),
        "scan should create all three notes:\n{}",
        format_output(&output)
    );
    assert!(!intake_pdf.exists(), "scan must move the PDF out of xlib");
    let first_contents =
        fs::read_to_string(&first_note).expect("read first note");
    let second_contents =
        fs::read_to_string(&second_note).expect("read second note");
    let intake_contents =
        fs::read_to_string(&intake_note).expect("read intake note");
    for contents in [&first_contents, &second_contents, &intake_contents] {
        assert_eq!(
            contents.matches("created: ").count(),
            1,
            "each new scan note should carry exactly one created field:\n{contents}"
        );
        assert!(
            contents.contains("created: 2026-10-03T15:42:18-0400\n"),
            "{contents}"
        );
    }
    assert!(
        first_contents.contains("ref_type: books\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n"),
        "{second_contents}"
    );
    assert!(
        intake_contents.contains("ref_type: chat\n"),
        "{intake_contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-04 09:00:00")
        .env("TZ", "America/New_York")
        .output()
        .expect("repeat highlights scan at a later clock");

    assert_success(&output);
    assert!(
        stdout(&output).contains("notes_created: 0"),
        "repeat scan must create nothing:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&first_note).expect("read first note after rescan"),
        first_contents,
        "repeat scan at a later clock must leave notes unchanged"
    );
    assert_eq!(
        fs::read_to_string(&intake_note)
            .expect("read intake note after rescan"),
        intake_contents,
        "repeat scan at a later clock must leave the intake note unchanged"
    );
}

fn assert_note_has_audio_player(contents: &str, vault_audio: &str) {
    assert!(
        contents.contains(&format!("audio: \"[[{vault_audio}]]\"\n")),
        "{contents}"
    );
    assert!(
        contents.contains(&format!("![[{vault_audio}]]\n")),
        "{contents}"
    );
    assert!(
        !contents.contains("highlights_marker_fields:"),
        "audio must not enter marker fields:\n{contents}"
    );
}

#[test]
fn highlights_ref_scan_intakes_pdf_and_mp3_and_embeds_player() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-audio");
    let vault = temp.path().join("vault");
    let source_pdf = vault.join("xlib/chat/listen.pdf");
    let source_audio = vault.join("xlib/chat/listen.mp3");
    let destination_pdf = vault.join("lib/chat/listen.pdf");
    let destination_audio = vault.join("lib/chat/listen.mp3");
    let note = vault.join("ref/chat/listen.md");
    write_highlights_pdf(
        &source_pdf,
        "- status: wip\n- parent: obsidian\n- title: Listen PDF\n",
    );
    write_file(&source_audio, "fake-mp3");

    let dry_run = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run audio intake");
    assert_success(&dry_run);
    let dry_report = stdout(&dry_run);
    assert!(
        dry_report.contains("intake_audio_moves: 1")
            && dry_report.contains(
                "intake: would-move xlib/chat/listen.mp3 -> lib/chat/listen.mp3"
            )
            && dry_report.contains("notes_create: 1")
            && dry_report.contains("writes: none"),
        "{dry_report}"
    );
    assert!(source_audio.is_file(), "dry-run must leave audio in xlib");
    assert!(!destination_audio.exists(), "dry-run must not move audio");
    assert!(!note.exists(), "dry-run must not create note");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan audio intake");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("intake_audio_moves: 1")
            && report.contains(
                "intake: moved xlib/chat/listen.mp3 -> lib/chat/listen.mp3"
            )
            && report.contains("notes_created: 1"),
        "{report}"
    );
    assert!(!source_pdf.exists() && !source_audio.exists());
    assert!(destination_pdf.is_file() && destination_audio.is_file());
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert_note_has_audio_player(&contents, "lib/chat/listen.mp3");
    assert!(
        contents.contains(
            "- [/] #task #ref [[lib/chat/listen.pdf]] #hide ^ref\n\n![[lib/chat/listen.mp3]]\n\n## Highlights\n"
        ),
        "{contents}"
    );

    let repeat = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat audio scan");
    assert_success(&repeat);
    assert!(
        stdout(&repeat).contains("notes_unchanged: 1"),
        "{}",
        format_output(&repeat)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after repeat"),
        contents
    );
}

#[test]
fn highlights_ref_scan_late_pairs_audio_onto_existing_note() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-late-pair");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/listen.pdf");
    let note = vault.join("ref/chat/listen.md");
    let audio = vault.join("xlib/chat/listen.mp3");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Listen PDF\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("scan")
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial scan without audio"),
    );
    let before = fs::read_to_string(&note).expect("read note before audio");
    assert!(!before.contains("\naudio:"), "{before}");
    assert!(!before.contains("![[lib/chat/listen.mp3]]"), "{before}");

    write_file(&audio, "fake-mp3");
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("late-pair scan");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("intake_audio_moves: 1")
            && report.contains(
                "intake: moved xlib/chat/listen.mp3 -> lib/chat/listen.mp3"
            )
            && report.contains("notes_updated: 1"),
        "{report}"
    );
    assert!(!audio.exists(), "late-pair must move audio out of xlib");
    assert!(vault.join("lib/chat/listen.mp3").is_file());
    let contents = fs::read_to_string(&note).expect("read late-paired note");
    assert_note_has_audio_player(&contents, "lib/chat/listen.mp3");

    let without_embed = contents.replace("\n![[lib/chat/listen.mp3]]\n", "\n");
    write_file(&note, &without_embed);
    let repeat = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat after user deleted embed");
    assert_success(&repeat);
    let after = fs::read_to_string(&note).expect("read note after deletion");
    assert!(
        after.contains("audio: \"[[lib/chat/listen.mp3]]\"\n"),
        "{after}"
    );
    assert!(
        !after.contains("![[lib/chat/listen.mp3]]"),
        "deleted embed must stay deleted:\n{after}"
    );
}

#[test]
fn highlights_ref_scan_refuses_audio_destination_conflict() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-audio-conflict");
    let vault = temp.path().join("vault");
    let source_pdf = vault.join("xlib/chat/conflict.pdf");
    let source_audio = vault.join("xlib/chat/conflict.mp3");
    let destination_audio = vault.join("lib/chat/conflict.mp3");
    write_highlights_pdf(
        &source_pdf,
        "- status: wip\n- parent: obsidian\n- title: Conflict PDF\n",
    );
    write_file(&source_audio, "new-audio");
    write_file(&destination_audio, "old-audio");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("scan audio conflict");
    assert_eq!(
        output.status.code(),
        Some(1),
        "conflicting audio intake should fail:\n{}",
        format_output(&output)
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("xlib intake collision(s) detected before writes")
            && diagnostic.contains("xlib/chat/conflict.mp3")
            && diagnostic.contains("lib/chat/conflict.mp3"),
        "{diagnostic}"
    );
    assert!(source_audio.is_file(), "conflict must leave xlib audio");
    assert_eq!(
        fs::read_to_string(&destination_audio).expect("read library audio"),
        "old-audio"
    );
}
