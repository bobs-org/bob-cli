//! Highlight task and status tests.

use crate::support::*;
use std::fs;

#[test]
fn highlights_ref_task_cancelled_dry_run_requires_and_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-cancelled");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    let generated_note = fs::read_to_string(&note).expect("read ref note");
    let cancelled_note = generated_note.replace(
        "- [/] #task #ref [[lib/example.pdf]] #hide ^ref",
        "- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] ^ref",
    );
    write_file(&note, &cancelled_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run sync cancelled task");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("pdf_task: cancelled")
            && dry_run.contains("pdf_task_contribution: status=abandoned")
            && dry_run.contains("note_action: update")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("writes: none"),
        "expected cancelled-task dry-run update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan cancelled task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("notes_update: 1")
            && report.contains("pdf_marker_action: would-update")
            && report.contains("writes: none"),
        "expected cancelled task scan dry-run to preview marker work:\n{}",
        format_output(&output)
    );
    assert!(
        !report.contains("malformed") && !stderr(&output).contains("malformed"),
        "cancelled generated task should not be reported malformed:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after dry run"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "cancelled task should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task write-pdf sync");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("pdf_marker_action: update")
            && report.contains("writes: note,pdf"),
        "expected cancelled-task write-pdf sync to write note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should refresh the source PDF hash"
    );
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: abandoned\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains("- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] ^ref\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_after_write}\n")),
        "note should record post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_before_write}\n")),
        "note should not retain pre-write PDF hash:\n{note_after_write}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat cancelled task sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "cancelled-task write-back should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_cancelled_scan_write_pdfs_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-cancelled-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    let cancelled_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [/] #task", "- [-] #task");
    write_file(&note, &cancelled_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task write-pdfs scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: true")
            && report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("pdf_markers_updated: 1")
            && report.contains("writes: note,pdf"),
        "expected scan --write-pdfs to write abandoned note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: abandoned\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains("- [-] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{note_after_write}"
    );
}

#[test]
fn highlights_ref_task_checked_dry_run_requires_and_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-read");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let checked_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &checked_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task sync");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("pdf_task: checked")
            && dry_run.contains("pdf_task_contribution: status=read")
            && dry_run.contains("note_action: update")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("writes: none"),
        "expected checked-task dry-run update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task scan");

    assert_success(&output);
    let scan = stdout(&output);
    assert!(
        scan.contains("pdf_task_contribution: status=read")
            && scan.contains("write_pdfs: false")
            && scan.contains("pdf_marker_action: would-update")
            && scan.contains("notes_update: 1")
            && scan.contains("writes: none"),
        "expected scan dry-run to preview marker work:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task scan with PDF writes enabled");

    assert_success(&output);
    let scan_write_dry_run = stdout(&output);
    assert!(
        scan_write_dry_run.contains("write_pdfs: true")
            && scan_write_dry_run
                .contains("pdf_task_contribution: status=read")
            && scan_write_dry_run.contains("pdf_marker_action: would-update")
            && scan_write_dry_run.contains("notes_update: 1")
            && scan_write_dry_run.contains("writes: none"),
        "expected scan --dry-run --write-pdfs to stay read-only:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "checked task should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task scan without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "scan should refuse checked-task PDF marker writes:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: false")
            && report.contains("plan_error:")
            && report.contains("--write-pdf")
            && report.contains("plan_failures: 1")
            && report.contains("writes: none"),
        "expected scan planning refusal:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task write-pdfs scan");

    assert_success(&output);
    let scan_write = stdout(&output);
    assert!(
        scan_write.contains("write_pdfs: true")
            && scan_write.contains("pdf_markers_updated: 1")
            && scan_write.contains("writes: note,pdf"),
        "expected scan --write-pdfs to write note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should refresh the source PDF hash"
    );
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: read\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_after_write}\n")),
        "note should record post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_before_write}\n")),
        "note should not retain pre-write PDF hash:\n{note_after_write}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat checked task sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "checked-task write-back should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_ready_scan_reopens_read_ref_to_ready() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-reopen");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    // Start from an already-read ref: the marker, note frontmatter, and the
    // generated ^ref task all reflect the terminal state.
    write_highlights_pdf(&pdf, "- status: read\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", "2026-10-03 09:00:00")
            .output()
            .expect("initial highlights sync"),
    );

    let read_note = fs::read_to_string(&note).expect("read ref note");
    assert!(
        read_note.contains("status: read\n")
            && read_note.contains(
                "- [x] #task #ref [[lib/example.pdf]] #hide [completion:: 2026-10-03] ^ref\n"
            ),
        "expected a read ref with a checked, date-stamped ^ref task:\n{read_note}"
    );
    // The user moves the generated ^ref task to Ready to reopen the ref.
    let reopened_note = read_note.replace("- [x] #task", "- [ ] #task");
    write_file(&note, &reopened_note);
    let marker_before = pdf_marker_contents(&pdf);

    // A dry-run preview surfaces the reopen contribution and the pending marker
    // write without touching anything.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run reopen scan with PDF writes enabled");

    assert_success(&output);
    let scan_dry_run = stdout(&output);
    assert!(
        scan_dry_run.contains("write_pdfs: true")
            && scan_dry_run.contains("pdf_task: ready")
            && scan_dry_run.contains("pdf_task_contribution: status=ready")
            && scan_dry_run.contains("pdf_marker_action: would-update")
            && scan_dry_run.contains("notes_update: 1")
            && scan_dry_run.contains("writes: none"),
        "expected reopen scan dry-run to preview marker work:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), reopened_note);

    // Without --write-pdfs the marker write-back is refused, exactly like the
    // checked/cancelled task closing behavior.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("reopen scan without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "scan should refuse reopen PDF marker writes:\n{}",
        format_output(&output)
    );
    let refusal = stdout(&output);
    assert!(
        refusal.contains("write_pdfs: false")
            && refusal.contains("plan_error:")
            && refusal.contains("--write-pdf")
            && refusal.contains("plan_failures: 1")
            && refusal.contains("writes: none"),
        "expected scan planning refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), reopened_note);

    // With --write-pdfs the ref note, PDF marker, and generated ^ref task all
    // move to Ready.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("reopen write-pdfs scan");

    assert_success(&output);
    let scan_write = stdout(&output);
    assert!(
        scan_write.contains("write_pdfs: true")
            && scan_write.contains("pdf_task_contribution: status=ready")
            && scan_write.contains("pdf_markers_updated: 1")
            && scan_write.contains("writes: note,pdf"),
        "expected reopen scan --write-pdfs to write note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: ready\n"), "{marker}");
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: ready\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write.contains(
            "- [ ] #task #ref [[lib/example.pdf]] #hide [completion:: 2026-10-03] ^ref\n"
        ),
        "reopen must keep the stamped completion date:\n{note_after_write}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat reopen sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "reopen write-back should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_checked_sync_creates_annotation_tasks_before_closing() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-closing-sync");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/closing-order.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/closing-order.md");
    let route_note = vault.join("alice.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Closing Order\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sync creates reference note"),
    );
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    write_file(
        &sidecar,
        "\
## Page 7

- status: wip
- parent: obsidian

---

> Closing highlight.

- #task Final same-note intake.
- #task Ask Alice about closing order @alice
",
    );
    let checked_note =
        fs::read_to_string(&note).expect("read ref note").replace(
            "- [/] #task #ref [[lib/books/closing-order.pdf]]",
            "- [x] #task #ref [[lib/books/closing-order.pdf]]",
        );
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync checked task with new annotation tasks");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task_contribution: status=read")
            && report.contains("annotation_tasks_create: 2")
            && report.contains("annotation_tasks_created: 2")
            && report.contains("routed_task_note_writes: 1"),
        "expected checked-task close to import pending annotation tasks:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read closed ref note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains(
            "- [x] #task #ref [[lib/books/closing-order.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    let same_note_task = find_created_annotation_task(
        &contents,
        "#task Final same-note intake.",
    );
    assert!(
        contents.contains("## Tasks\n\n- [ ]"),
        "same-note task should sit under ## Tasks:\n{contents}"
    );
    assert_annotation_tasks_in_tasks_section(&contents, &[&same_note_task]);
    assert!(
        same_note_task.contains("[[#^h-")
            && same_note_task.contains("[h:: ")
            && same_note_task.contains("[created::"),
        "same-note task should link to its annotation block and carry properties: {same_note_task}"
    );
    let same_note_source_id = annotation_task_source_link_id(&same_note_task);
    assert!(
        highlight_block_ids(&contents).contains(&same_note_source_id),
        "same-note annotation block should exist:\n{contents}"
    );
    let route_contents =
        fs::read_to_string(&route_note).expect("read routed task note");
    let routed_task = find_created_annotation_task(
        &route_contents,
        "#task Ask Alice about closing order",
    );
    assert!(
        !route_contents.contains("## Tasks"),
        "routed notes should not grow a ## Tasks section:\n{route_contents}"
    );
    assert!(
        route_contents.ends_with(&format!("{routed_task}\n"))
            || route_contents.contains(&format!("{routed_task}\n")),
        "routed task should still be appended to the routed note:\n{route_contents}"
    );
    assert!(!routed_task.contains("@alice"), "{routed_task}");
    assert!(
        routed_task.contains("[[ref/books/closing-order#^h-")
            && routed_task.contains("[h:: ")
            && routed_task.contains("[created::"),
        "routed task should link to its annotation block and carry properties: {routed_task}"
    );
    let routed_source_id = annotation_task_source_link_id(&routed_task);
    assert!(
        highlight_block_ids(&contents).contains(&routed_source_id),
        "routed annotation block should exist in the ref note:\n{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat sync after closing status");

    assert_success(&output);
    assert!(
        stdout(&output).contains("annotation_tasks_created: 0")
            && stdout(&output).contains("writes: none"),
        "repeat read sync should not create additional annotation tasks:\n{}",
        format_output(&output)
    );
    let repeat_contents =
        fs::read_to_string(&note).expect("read repeat-synced ref note");
    let repeat_route_contents =
        fs::read_to_string(&route_note).expect("read repeat-routed note");
    assert_eq!(
        created_annotation_task_count(
            &repeat_contents,
            "#task Final same-note intake."
        ),
        1,
        "{repeat_contents}"
    );
    assert_eq!(
        created_annotation_task_count(
            &repeat_route_contents,
            "#task Ask Alice about closing order"
        ),
        1,
        "{repeat_route_contents}"
    );
}

#[test]
fn highlights_ref_task_checked_scan_creates_annotation_tasks_before_closing() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-closing-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/scan-closing.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/scan-closing.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Scan Closing\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sync creates scan reference note"),
    );
    write_file(
        &sidecar,
        "\
## Page 3

- status: wip
- parent: obsidian

---

> Scan closing highlight.

- #task Import during scan close.
",
    );
    let checked_note = fs::read_to_string(&note)
        .expect("read scan ref note")
        .replace(
            "- [/] #task #ref [[lib/books/scan-closing.pdf]]",
            "- [x] #task #ref [[lib/books/scan-closing.pdf]]",
        );
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan checked task with pending annotation task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: true")
            && report.contains("pdf_task_contribution: status=read")
            && report.contains("annotation_tasks_create: 1")
            && report.contains("pdf_markers_would_update: 1")
            && report.contains("writes: none"),
        "scan dry-run should report the final intake pass:\n{}",
        format_output(&output)
    );
    assert!(
        pdf_marker_contents(&pdf).contains("- status: wip\n"),
        "dry-run should leave the PDF marker wip"
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write scan checked task with pending annotation task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("annotation_tasks_created: 1")
            && report.contains("pdf_markers_updated: 1")
            && report.contains("writes: note,pdf"),
        "scan --write-pdfs should write annotation tasks and close marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read scan-closed note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains(
            "- [x] #task #ref [[lib/books/scan-closing.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    let created_task = find_created_annotation_task(
        &contents,
        "#task Import during scan close.",
    );
    let source_id = annotation_task_source_link_id(&created_task);
    assert!(
        created_task.contains("[[#^h-")
            && created_task.contains("[h:: ")
            && highlight_block_ids(&contents).contains(&source_id),
        "scan-created task should link to an annotation block and carry h property:\n{contents}"
    );
}

#[test]
fn highlights_ref_task_checked_dirty_tracked_note_is_allowed() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-dirty");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial sync"]);
    let checked_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty checked task");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read synced note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{contents}"
    );
}

#[test]
fn highlights_ref_task_checked_competing_status_edits_fail() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-task-conflict-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        let mut edited = fs::read_to_string(&note)
            .expect("read ref note")
            .replace("- [/] #task", "- [x] #task");
        if source == "frontmatter" {
            edited = edited.replace("status: wip", "status: abandoned");
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: abandoned\n- parent: obsidian\n",
            );
        }
        write_file(&note, &edited);
        let note_before = fs::read_to_string(&note).expect("read note before");
        let marker_before = pdf_marker_contents(&pdf);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync checked task conflict for {source}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "checked task conflict should fail for {source}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("checked PDF task conflicts")
                && stderr(&output)
                    .contains(&format!("{source} status=\"abandoned\"")),
            "expected checked-task conflict report for {source}:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&note).expect("read note after conflict"),
            note_before
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
    }
}

#[test]
fn highlights_ref_task_cancelled_competing_status_edits_fail() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-task-cancelled-conflict-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        let mut edited = fs::read_to_string(&note)
            .expect("read ref note")
            .replace("- [/] #task", "- [-] #task");
        if source == "frontmatter" {
            edited = edited.replace("status: wip", "status: read");
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: read\n- parent: obsidian\n",
            );
        }
        write_file(&note, &edited);
        let note_before = fs::read_to_string(&note).expect("read note before");
        let marker_before = pdf_marker_contents(&pdf);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync cancelled task conflict for {source}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "cancelled task conflict should fail for {source}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("cancelled PDF task conflicts")
                && stderr(&output)
                    .contains(&format!("{source} status=\"read\"")),
            "expected cancelled-task conflict report for {source}:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&note).expect("read note after conflict"),
            note_before
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
    }
}

#[test]
fn highlights_ref_status_abandoned_rewrites_generated_task_to_cancelled() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-abandoned-task-render-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        if source == "frontmatter" {
            let edited = fs::read_to_string(&note)
                .expect("read ref note")
                .replace("status: wip", "status: abandoned")
                .replace("- [/] #task", "- [-] #task");
            write_file(&note, &edited);
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: abandoned\n- parent: obsidian\n",
            );
            let cancelled_note = fs::read_to_string(&note)
                .expect("read ref note")
                .replace("- [/] #task", "- [-] #task");
            write_file(&note, &cancelled_note);
        }

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync abandoned status render for {source}: {error}")
            });

        assert_success(&output);
        let marker = pdf_marker_contents(&pdf);
        assert!(marker.contains("- status: abandoned\n"), "{marker}");
        let contents = fs::read_to_string(&note).expect("read synced note");
        assert!(contents.contains("status: abandoned\n"), "{contents}");
        assert!(
            contents
                .contains("- [-] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
            "{contents}"
        );
    }
}

#[test]
fn highlights_ref_blocked_task_syncs_as_status_neutral_overlay() {
    let temp = TempDir::new("bob-cli-highlights-ref-blocked");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: next\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    let synced = fs::read_to_string(&note).expect("read ref note");
    assert!(synced.contains("status: next\n"), "{synced}");
    let blocked_line = "- [?] #task #ref [[lib/example.pdf]] [dependsOn:: dep] [fresh:: 2026-10-08] #hide ^ref";
    let child = "\t- ⛓️ **DEPENDS ON:** [[other#^dep]]";
    let blocked_note = synced.replacen(
        "- [*] #task #ref [[lib/example.pdf]] #hide ^ref",
        &format!("{blocked_line}\n{child}"),
        1,
    );
    assert!(blocked_note.contains(child), "{blocked_note}");
    write_file(&note, &blocked_note);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("blocked writing scan");
    assert_success(&output);
    let after = fs::read_to_string(&note).expect("read blocked note");
    assert!(after.contains("status: next\n"), "{after}");
    assert!(after.contains(blocked_line), "{after}");
    assert!(after.contains(child), "{after}");
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat blocked scan");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after repeat"),
        after
    );

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let note_before_conflict = after.clone();
    let marker_before_conflict = pdf_marker_contents(&pdf);
    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("blocked conflict scan");
    assert_eq!(
        output.status.code(),
        Some(1),
        "blocked marker conflict should fail:\n{}",
        format_output(&output)
    );
    assert!(
        format_output(&output).contains("blocked PDF task conflicts"),
        "expected blocked conflict:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after conflict"),
        note_before_conflict
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before_conflict);
    set_pdf_marker_contents(&pdf, "- status: next\n- parent: obsidian\n");

    let unblocked = after.replacen("- [?]", "- [ ]", 1);
    write_file(&note, &unblocked);
    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("blocked recovery scan");
    assert_success(&output);
    let recovered = fs::read_to_string(&note).expect("read recovered note");
    assert!(recovered.contains("status: ready\n"), "{recovered}");
    assert!(pdf_marker_contents(&pdf).contains("- status: ready\n"));
}

fn find_created_annotation_task(contents: &str, prose: &str) -> String {
    contents
        .lines()
        .find(|line| line.starts_with("- [ ]") && line.contains(prose))
        .unwrap_or_else(|| {
            panic!("missing created annotation task for {prose}:\n{contents}")
        })
        .to_string()
}

fn created_annotation_task_count(contents: &str, prose: &str) -> usize {
    contents
        .lines()
        .filter(|line| line.starts_with("- [") && line.contains(prose))
        .count()
}

fn annotation_task_source_link_id(line: &str) -> String {
    let start = line.find("[[").expect("source link present") + 2;
    let rest = &line[start..];
    let end = rest.find("]]").expect("source link terminator");
    let inside = &rest[..end];
    let target = inside.split_once('|').map_or(inside, |(target, _)| target);
    target
        .rsplit_once("#^")
        .unwrap_or_else(|| panic!("source link has no block id: {line}"))
        .1
        .to_string()
}
