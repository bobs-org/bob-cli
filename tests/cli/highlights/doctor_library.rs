//! `bob ref doctor` library health and coverage rows (phase `doctor`).

use crate::support::*;
use std::fs;

fn commit_vault(vault: &std::path::Path) {
    git_in(vault, ["init", "-q"]);
    configure_test_git_identity(vault);
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", "initial vault"]);
}

fn run_doctor(vault: &std::path::Path) -> std::process::Output {
    bob_command()
        .arg("ref")
        .arg("doctor")
        .arg("--no-hooks")
        .env("BOB_DIR", vault)
        .output()
        .expect("run bob ref doctor")
}

fn base_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("lib")).expect("create lib");
    fs::create_dir_all(vault.join("ref")).expect("create ref");
    fs::create_dir_all(vault.join("xlib")).expect("create xlib");
    vault
}

fn write_queued_note(vault: &std::path::Path, name: &str) {
    write_file(
        &vault.join(format!("ref/papers/{name}.md")),
        "---\nstatus: ready\ntitle: Queued Note\n---\n\n# Queued Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
}

#[test]
fn doctor_library_rows_report_ok_on_clean_vault() {
    let temp = TempDir::new("bob-cli-doctor-library-ok");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/finished.md"),
        "---\nstatus: read\ntitle: Finished Paper\nsource_url: https://example.com/papers/finished\nsource_pdf: \"[[lib/papers/finished.pdf]]\"\n---\n\n# Finished Paper\n\n- [x] #task #ref [[lib/papers/finished.pdf]] #hide ^ref\n",
    );
    write_queued_note(&vault, "queued");
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "library: ok (2 notes · 1 finished · 0 started · 1 queued · 0 dropped · 0 unknown)"
        ),
        "{report}"
    );
    assert!(
        report.contains("library diagnostics: ok (no diagnostics)"),
        "{report}"
    );
    assert!(report.contains("ref tasks:"), "{report}");
    assert!(report.contains("parents:"), "{report}");
    assert!(
        report.contains("identity: ok (no superseded notes)"),
        "{report}"
    );
    assert!(
        report.contains("annotations: ok (no leaked marker mirrors)"),
        "{report}"
    );
    assert!(
        report.contains(
            "coverage: ok (no unindexed zorg-era reading records outside ref/)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_library_diagnostics_warn_lists_codes_and_paths() {
    let temp = TempDir::new("bob-cli-doctor-library-diagnostics");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/two_trackers.md"),
        "---\nstatus: ready\ntitle: Two Trackers\n---\n\n# Two Trackers\n\n- [ ] #task #ref #hide ^ref\n\n- [*] #task #ref #hide ^ref\n",
    );
    write_queued_note(&vault, "queued");
    commit_vault(&vault);

    let output = run_doctor(&vault);
    // Diagnostics are warnings, never failures.
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "library diagnostics: warn (1 note: 1 multiple_ref_trackers · e.g. ref/papers/two_trackers.md)"
        ),
        "{report}"
    );
    assert!(report.contains("ref tasks:"), "{report}");
    assert!(report.contains("parents:"), "{report}");
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_identity_ok_reports_superseded_legacy_note() {
    let temp = TempDir::new("bob-cli-doctor-library-superseded");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/fresh_capture.md"),
        "---\nstatus: ready\ntitle: Fresh Capture\nsource_url: https://example.com/papers/shared\nsource_pdf: \"[[lib/papers/shared.pdf]]\"\n---\n\n# Fresh Capture\n\n- [ ] #task #ref [[lib/papers/shared.pdf]] #hide ^ref\n",
    );
    write_file(
        &vault.join("ref/ai/legacy_capture.md"),
        "---\nstatus: legacy\nlegacy_status: unread\ntitle: Legacy Capture\nurl: https://example.com/papers/shared\n---\n\nMigrated body.\n",
    );
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "identity: ok (1 legacy note superseded by a newer capture)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_identity_warn_on_identity_shared_by_two_pdfs() {
    let temp = TempDir::new("bob-cli-doctor-library-identity");
    let vault = base_vault(&temp);
    for name in ["pdf_a", "pdf_b"] {
        write_file(
            &vault.join(format!("ref/papers/{name}.md")),
            "---\nstatus: read\ntitle: Dup PDF\nsource_url: https://example.com/papers/dup\nsource_pdf: \"[[lib/papers/dup.pdf]]\"\n---\n\n# Dup PDF\n\n- [x] #task #ref [[lib/papers/dup.pdf]] #hide ^ref\n",
        );
    }
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "identity: warn (1 identity key shared by more than one PDF-backed note: https://example.com/papers/dup)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_annotations_warn_on_leaked_marker_mirror() {
    let temp = TempDir::new("bob-cli-doctor-library-annotations");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/mirror.md"),
        "---\nstatus: ready\ntitle: Mirror Note\n---\n\n# Mirror Note\n\n- [ ] #task #ref #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Page 1\n\n> [!note] - status: ready\n> - parent: reading_list\n^h-222222222222\n\n<!-- highlights:end -->\n",
    );
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "annotations: warn (1 note still renders a leaked marker mirror; the next bob ref scan removes it)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_coverage_uses_provenance_mirroring() {
    let temp = TempDir::new("bob-cli-doctor-library-provenance");
    let vault = base_vault(&temp);
    write_queued_note(&vault, "queued");
    write_file(
        &vault.join("hub.md"),
        "# Hub records\n\n- 260101#0a [[read]] ID::pair_one ^z-250425-0f\n  * status:: READ\n\n- 260101#0b [[read]] ID::pair_two ^z-250425-0f\n  * status:: READ\n\n- 260101#0c [[read]] ID::big_book ^z-260101-0a\n  * status:: BOOK\n\n- 260101#0d [[read]] LID::chapter_a First ^z-260101-0b\n  | BOOK: [[hub#^z-260101-0a|big_book]]\n  * status:: READ\n\n- 260101#0e [[read]] LID::chapter_b Second ^z-260101-0c\n  | BOOK: [[hub#^z-260101-0a|big_book]]\n  * status:: UNREAD\n",
    );
    // Mirrors the first member of the shared-block pair only: the
    // `source_id` tells the pair apart.
    write_file(
        &vault.join("ref/ai/pair_one.md"),
        "---\nstatus: legacy\nlegacy_status: read\ntitle: Pair One\nsource_block: ^z-250425-0f\nsource_path: hub.md\nsource_id: pair_one\n---\n\nMigrated body.\n",
    );
    // The book note folds both chapters through `source_blocks` and
    // derives `started` from a finished and a queued chapter.
    write_file(
        &vault.join("ref/ai/big_book.md"),
        "---\nstatus: legacy\nlegacy_status: book\ntitle: Big Book\nsource_block: ^z-260101-0a\nsource_path: hub.md\nsource_id: big_book\nsource_blocks:\n  - \"^z-260101-0b\"\n  - \"^z-260101-0c\"\nlegacy_chapter_statuses:\n  - \"read\"\n  - \"unread\"\n---\n\nMigrated body.\n",
    );
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "coverage: warn (~1 zorg-era reading record outside ref/ is not indexed: hub.md 1)"
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "library: ok (3 notes · 1 finished · 1 started · 1 queued · 0 dropped · 0 unknown)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_coverage_warn_counts_unmirrored_zorg_records() {
    let temp = TempDir::new("bob-cli-doctor-library-coverage");
    let vault = base_vault(&temp);
    write_queued_note(&vault, "queued");
    write_file(
        &vault.join("old_ref.md"),
        "# Old reading records\n\n- 250419#09 [[read]] ID::awesome ^z-250419-09\n  * status:: READ\n  * url:: https://example.com/awesome\n\n- 250420#0a [[read]] ID::other ^z-250420-0a\n  * status:: UNREAD\n",
    );
    write_file(
        &vault.join("other.md"),
        "# More records\n\n- 250421#0b [[read]] ID::third ^z-250421-0b\n  * status:: BOOK\n",
    );
    // A ref note carrying the first record's block id and file mirrors it,
    // so only the other two records count.
    write_file(
        &vault.join("ref/ai/legacy_awesome.md"),
        "---\nstatus: legacy\nlegacy_status: read\ntitle: Legacy Awesome\nsource_block: ^z-250419-09\nsource_path: old_ref.md\nurl: https://example.com/awesome\n---\n\nMigrated body.\n",
    );
    commit_vault(&vault);

    let output = run_doctor(&vault);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "coverage: warn (~2 zorg-era reading records outside ref/ are not indexed: old_ref.md 1, other.md 1)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}
