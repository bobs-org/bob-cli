//! Remaining highlight refs: doctor, marker, tombstone.

use crate::support::*;
use std::ffi::OsString;
use std::fs;

#[test]
fn highlights_ref_short_options_are_accepted() {
    let temp = TempDir::new("bob-cli-highlights-ref-short-options");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/example.pdf");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-j")
        .arg("1")
        .arg("-l")
        .arg("lib")
        .arg("-r")
        .arg("ref")
        .arg("-v")
        .arg("-w")
        .arg("-x")
        .arg("xlib")
        .output()
        .expect("run bob highlights scan with short options");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("pdf_count: 1"), "{report}");
    assert!(report.contains("writes: none"), "{report}");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-l")
        .arg("lib")
        .arg("-p")
        .arg("marker")
        .arg("-r")
        .arg("ref")
        .arg("-w")
        .output()
        .expect("run bob highlights sync with short options");

    assert_success(&output);
    assert!(stdout(&output).contains("writes: none"));
}

#[test]
fn highlights_ref_marker_uses_first_page_text_annotation() {
    let temp = TempDir::new("bob-cli-highlights-ref-first-page-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/two-page.pdf");
    write_highlights_pdf_pages(
        &pdf,
        &[
            &["- status: wip\n- parent: obsidian\n- title: Page One\n"],
            &["- status: read\n- parent: ignored\n- title: Page Two\n"],
        ],
    );

    let output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("inspect first-page marker");

    assert_success(&output);
    let marker = stdout(&output);
    assert!(marker.contains("marker_page: 1"), "{marker}");
    assert!(marker.contains("marker_note: 1"), "{marker}");
    assert!(marker.contains("title: Page One"), "{marker}");
    assert!(
        !marker.contains("Page Two"),
        "later page note must not be selected:\n{marker}"
    );
}

#[test]
fn highlights_ref_rejects_wikilink_marker_parent_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-linked-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/linked-parent.pdf");
    let note = vault.join("ref/linked-parent.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: [[obsidian]]\n- title: Linked Parent\n",
    );

    let sync = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");
    assert_eq!(
        sync.status.code(),
        Some(1),
        "linked marker parent should fail sync:\n{}",
        format_output(&sync)
    );
    assert!(
        stderr(&sync).contains("wikilinks are not supported"),
        "expected linked parent error:\n{}",
        format_output(&sync)
    );
    assert!(!note.exists(), "sync must not write a note on marker error");

    let marker = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights marker");
    assert_eq!(
        marker.status.code(),
        Some(1),
        "linked marker parent should fail marker inspection:\n{}",
        format_output(&marker)
    );
    assert!(
        stderr(&marker).contains("wikilinks are not supported"),
        "expected linked parent error:\n{}",
        format_output(&marker)
    );

    let scan = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights scan");
    assert_eq!(
        scan.status.code(),
        Some(1),
        "linked marker parent should fail scan planning:\n{}",
        format_output(&scan)
    );
    let report = stdout(&scan);
    assert!(
        report.contains("plan_error:")
            && report.contains("wikilinks are not supported"),
        "expected scan linked parent error:\n{}",
        format_output(&scan)
    );
    assert!(
        stderr(&scan).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&scan)
    );
    assert!(!note.exists(), "scan must not write a note on marker error");
}

#[test]
fn highlights_ref_dry_run_and_inspection_do_not_modify_vault_files() {
    let temp = TempDir::new("bob-cli-highlights-ref-dry-run");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    let pdf_before = fs::read(&pdf).expect("read PDF before");
    let marker_before = pdf_marker_contents(&pdf);
    let cases = vec![
        vec![
            OsString::from("highlights"),
            OsString::from("scan"),
            OsString::from("--dry-run"),
        ],
        vec![
            OsString::from("highlights"),
            OsString::from("sync"),
            OsString::from(path_str(&pdf)),
            OsString::from("--dry-run"),
        ],
        vec![OsString::from("highlights"), OsString::from("doctor")],
        vec![
            OsString::from("highlights"),
            OsString::from("marker"),
            OsString::from(path_str(&pdf)),
        ],
    ];

    for args in cases {
        let output = bob_command()
            .args(&args)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_success(&output);
        assert!(
            stdout(&output).contains("writes: none"),
            "expected no-write report:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read(&pdf).expect("read PDF after"),
            pdf_before,
            "highlights inspection command modified the PDF"
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
        assert!(
            !note.exists(),
            "dry-run/inspection must not create ref note"
        );
    }
}

#[test]
fn highlights_ref_doctor_reports_configured_pre_scan_executable() {
    let temp = TempDir::new("bob-cli-highlights-ref-pre-scan-doctor");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    fs::create_dir_all(vault.join("lib")).expect("create lib");
    fs::create_dir_all(vault.join("ref")).expect("create ref");
    fs::create_dir_all(vault.join("xlib")).expect("create xlib");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    write_executable(&script, "#!/bin/sh\nexit 0\n");
    write_file(
        &config,
        &format!("highlights:\n  pre_scan_hook: {}\n", path_str(&script)),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("doctor with pre-scan hook");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pre_scan_hook: ok")
            && report.contains(path_str(&script))
            && report.contains("result: ok"),
        "doctor should report the configured executable hook:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_doctor_no_hooks_reports_skipped() {
    let temp = TempDir::new("bob-cli-highlights-ref-doctor-no-hooks");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    fs::create_dir_all(vault.join("lib")).expect("create lib");
    fs::create_dir_all(vault.join("ref")).expect("create ref");
    fs::create_dir_all(vault.join("xlib")).expect("create xlib");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 29\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("doctor with --no-hooks");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pre_scan_hook: skipped (--no-hooks)")
            && report.contains("result: ok"),
        "doctor --no-hooks should skip the hook and stay ok:\n{}",
        format_output(&output)
    );
    assert!(
        !sentinel.exists(),
        "--no-hooks doctor must not execute the hook"
    );
}

#[test]
fn highlights_ref_doctor_checks_vault_git_without_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-doctor");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    let pandoc_stub = stub_bin.join("pandoc");
    write_executable(&pandoc_stub, "#!/bin/sh\nexit 0\n");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 1

Note: marker note
",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);

    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("BOB_PANDOC_COMMAND", &pandoc_stub)
        .output()
        .expect("run highlights doctor");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("vault_path: ok"), "{report}");
    assert!(report.contains("library_dir: ok"), "{report}");
    assert!(report.contains("ref_dir: ok"), "{report}");
    assert!(report.contains("xlib_dir: fail"), "{report}");
    assert!(report.contains("xlib_pending: 0"), "{report}");
    assert!(report.contains("sidecars_found: 1"), "{report}");
    assert!(report.contains("pdf_markers_readable: 1"), "{report}");
    assert!(report.contains("git: ok (clean worktree)"), "{report}");
    assert!(report.contains("pandoc: available"), "{report}");
    assert!(report.contains("writes: none"), "{report}");
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn highlights_ref_doctor_warns_on_orphan_companion_audio() {
    let temp = TempDir::new("bob-cli-highlights-ref-doctor-orphan-audio");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let orphan = vault.join("xlib/chat/orphan.mp3");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    let pandoc_stub = stub_bin.join("pandoc");
    write_executable(&pandoc_stub, "#!/bin/sh\nexit 0\n");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 1

Note: marker note
",
    );
    write_file(&orphan, "fake-mp3");
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);

    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("BOB_PANDOC_COMMAND", &pandoc_stub)
        .output()
        .expect("run highlights doctor with orphan audio");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("xlib_orphan_audio: 1"), "{report}");
    assert!(
        report.contains(
            "orphan companion audio has no PDF in xlib or lib: xlib/chat/orphan.mp3"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
    assert!(orphan.is_file(), "doctor must leave orphan audio in xlib");
}

#[test]
fn highlights_ref_marker_edit_updates_frontmatter() {
    let temp = TempDir::new("bob-cli-highlights-ref-marker-edit");
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

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let checked_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &checked_note);
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync marker edit");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read updated ref note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{contents}"
    );
}

#[test]
fn highlights_ref_frontmatter_edit_updates_marker_when_pdf_writes_enabled() {
    let temp = TempDir::new("bob-cli-highlights-ref-frontmatter-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: The Log is the Agent\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: read")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run frontmatter edit");

    assert_success(&output);
    assert!(
        stdout(&output).contains("pdf_marker_action: would-update"),
        "expected dry-run PDF update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter edit without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "frontmatter-to-marker sync should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter edit");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian\n"), "{marker}");
    assert!(
        marker.contains("- title: The Log is the Agent\n"),
        "{marker}"
    );
    assert!(!marker.contains("- type:"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should change the source PDF hash"
    );

    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains(&format!("source_pdf_sha256: {pdf_hash_after_write}\n")),
        "reference note should record the post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write.contains(&format!("source_pdf_sha256: {pdf_hash_before_write}\n")),
        "reference note should not keep the pre-write PDF hash:\n{note_after_write}"
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync after PDF write-back");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "frontmatter PDF write-back should settle in one run:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read settled note"),
        note_after_write
    );
}

#[test]
fn highlights_ref_deprecated_done_status_migrates_to_read_with_pdf_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-done-migration");
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

    set_pdf_marker_contents(&pdf, "- status: done\n- parent: obsidian\n");
    let old_done_note = fs::read_to_string(&note)
        .expect("read generated ref note")
        .replace("status: wip", "status: done")
        .replace("\\\"status\\\":\\\"wip\\\"", "\\\"status\\\":\\\"done\\\"")
        .replace("- [/] #task", "- [x] #task");
    assert!(
        old_done_note.contains("\\\"status\\\":\\\"done\\\""),
        "test must simulate old stored base status:\n{old_done_note}"
    );
    write_file(&note, &old_done_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run deprecated done migration");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("status_normalization: done->read")
            && dry_run.contains("marker,frontmatter,base")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("note_action: update")
            && dry_run.contains("writes: none"),
        "expected done->read migration dry-run report:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after dry-run"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("deprecated done sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "deprecated marker normalization should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after refusal"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan deprecated done migration");

    assert_success(&output);
    let scan_dry_run = stdout(&output);
    assert!(
        scan_dry_run.contains("status_normalization: done->read")
            && scan_dry_run.contains("pdf_marker_action: would-update")
            && scan_dry_run.contains("writes: none"),
        "expected scan dry-run migration report:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("writing scan deprecated done migration");

    assert_eq!(
        output.status.code(),
        Some(1),
        "writing scan should refuse deprecated marker normalization:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("plan_error:")
            && stdout(&output).contains("--write-pdf")
            && stdout(&output).contains("writes: none"),
        "expected scan planning refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after scan refusal"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write deprecated done migration");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(!marker.contains("- status: done\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(pdf_hash_before_write, pdf_hash_after_write);
    let migrated_note = fs::read_to_string(&note).expect("read migrated note");
    assert!(migrated_note.contains("status: read\n"), "{migrated_note}");
    assert!(
        migrated_note
            .contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{migrated_note}"
    );
    assert!(
        !migrated_note.contains("status: done")
            && !migrated_note.contains("\\\"status\\\":\\\"done\\\""),
        "{migrated_note}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat deprecated done migration sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "done migration should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_non_overlapping_edits_auto_merge_and_settle() {
    let temp = TempDir::new("bob-cli-highlights-ref-auto-merge");
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

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace(
            "parent: \"[[obsidian]]\"\n",
            "parent: \"[[obsidian]]\"\ntitle: \"Frontmatter Title\"\n",
        )
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &edited);
    let note_before = fs::read_to_string(&note).expect("read note before");
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("sync_source: auto-merge")
            && stdout(&output).contains("pdf_marker_action: would-update")
            && stdout(&output).contains("writes: none"),
        "expected dry-run auto-merge report:\n{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), note_before);
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("sync_source: auto-merge"),
        "expected auto-merge report:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian\n"), "{marker}");
    assert!(marker.contains("- title: Frontmatter Title\n"), "{marker}");
    let note_after = fs::read_to_string(&note).expect("read merged note");
    assert!(note_after.contains("status: read\n"), "{note_after}");
    assert!(
        note_after.contains("title: \"Frontmatter Title\"\n"),
        "{note_after}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync after auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "auto-merge should settle in one write:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_frontmatter_missing_parent_fails_before_pdf_writeback() {
    let temp =
        TempDir::new("bob-cli-highlights-ref-frontmatter-missing-parent");
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
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("parent: \"[[obsidian]]\"\n", "");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter missing parent");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing frontmatter parent should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: parent"),
        "expected missing parent error:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_frontmatter_unsupported_status_fails_before_pdf_writeback() {
    let temp = TempDir::new("bob-cli-highlights-ref-frontmatter-bad-status");
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
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: complete")
        .replace("- [/] #task #ref [[lib/example.pdf]] #hide ^ref\n", "");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter unsupported status");

    assert_eq!(
        output.status.code(),
        Some(1),
        "unsupported frontmatter status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output)
            .contains("frontmatter has unsupported status \"complete\""),
        "expected unsupported frontmatter status error:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_conflicting_edits_fail_and_prefer_frontmatter_resolves() {
    let temp = TempDir::new("bob-cli-highlights-ref-conflict");
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

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let frontmatter_side = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: abandoned")
        .replace("- [/] #task", "- [-] #task");
    write_file(&note, &frontmatter_side);
    let note_before = fs::read_to_string(&note).expect("read note before");
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync conflicting edits");

    assert_eq!(
        output.status.code(),
        Some(1),
        "conflicting edits should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("marker/frontmatter conflict"),
        "expected conflict report:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("status: marker=\"read\"")
            && stderr(&output).contains("frontmatter=\"abandoned\"")
            && stderr(&output).contains("base=\"wip\""),
        "expected field-level conflict report:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after conflict"),
        note_before
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .arg("--prefer")
        .arg("frontmatter")
        .env("BOB_DIR", &vault)
        .output()
        .expect("resolve conflict with frontmatter");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
}

#[test]
fn highlights_ref_comment_edit_keeps_stable_block_id() {
    let temp = TempDir::new("bob-cli-highlights-ref-comment-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note

---

> Stable quoted text.

Comment: first comment
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sidecar sync"),
    );
    let initial = fs::read_to_string(&note).expect("read initial note");
    let initial_ids = highlight_block_ids(&initial);
    assert_eq!(initial_ids.len(), 1, "{initial}");

    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note

---

> Stable quoted text.

Comment: revised comment
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync comment edit");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert_eq!(highlight_block_ids(&updated), initial_ids, "{updated}");
    assert!(
        updated.contains("[!note] Comment revised comment"),
        "{updated}"
    );
    assert!(
        !updated.contains("[!note] Comment first comment"),
        "{updated}"
    );
}

#[test]
fn highlights_ref_deleted_highlight_is_tombstoned() {
    let temp = TempDir::new("bob-cli-highlights-ref-tombstone");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 9

Note: marker note

---

> First quote.

---

> Deleted quote.
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial tombstone sync"),
    );
    let initial = fs::read_to_string(&note).expect("read initial note");
    let initial_ids = highlight_block_ids(&initial);
    assert_eq!(initial_ids.len(), 2, "{initial}");
    let deleted_id = initial_ids[1].clone();

    write_file(
        &sidecar,
        "\
## Page 9

Note: marker note

---

> First quote.
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync deleted highlight");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains("highlights_count: 1\n"), "{updated}");
    assert!(updated.contains("### Removed highlights\n"), "{updated}");
    assert!(
        updated.contains(&format!("^{deleted_id}\n")),
        "deleted block id should remain as a tombstone:\n{updated}"
    );
    assert!(
        updated.contains("> [!warning] Removed highlight This annotation is no longer present in the Highlights sidecar.\n"),
        "{updated}"
    );
    assert!(
        !updated.contains("> [!quote] Deleted quote.\n"),
        "{updated}"
    );
}
