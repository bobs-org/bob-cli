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
    if cfg!(target_os = "macos") {
        assert!(
            report.contains("pdf_write_coordination: on")
                || report.contains("pdf_write_coordination: off"),
            "{report}"
        );
    } else {
        assert!(
            report.contains(
                "pdf_write_coordination: n/a (no OS file coordination)"
            ),
            "{report}"
        );
    }
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

#[test]
fn doctor_reference_ids_ok_when_ids_match_stems() {
    let temp = TempDir::new("bob-cli-doctor-reference-ids-ok");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/matched.md"),
        "---\nstatus: ready\ntitle: Matched Note\nid: matched\n---\n\n# Matched Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
    write_file(
        &vault.join("ref/blogs/other.md"),
        "---\nstatus: ready\ntitle: Other Note\nid: other\n---\n\n# Other Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
    commit_vault(&vault);

    let report = stdout(&run_doctor(&vault));
    assert!(
        report
            .contains("reference ids: ok (2 notes with ids, all match stems)"),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_reference_ids_warn_on_mismatch_and_case_insensitive_duplicate() {
    let temp = TempDir::new("bob-cli-doctor-reference-ids-warn");
    let vault = base_vault(&temp);
    write_file(
        &vault.join("ref/papers/mismatch.md"),
        "---\nstatus: ready\ntitle: Mismatch Note\nid: wrong-id\n---\n\n# Mismatch Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
    write_file(
        &vault.join("ref/blogs/a.md"),
        "---\nstatus: ready\ntitle: A Note\nid: DuP\n---\n\n# A Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
    write_file(
        &vault.join("ref/blogs/b.md"),
        "---\nstatus: ready\ntitle: B Note\nid: dup\n---\n\n# B Note\n\n- [ ] #task #ref #hide ^ref\n",
    );
    commit_vault(&vault);

    let output = run_doctor(&vault);
    // Identity problems are warnings, never failures.
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "reference ids: warn (1 duplicate id \"DuP\" across 2 notes · 3 id/stem mismatches · e.g. ref/blogs/a.md, ref/blogs/b.md, ref/papers/mismatch.md — repair with bob ref rename <REF> <NAME>)"
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "library diagnostics: warn (3 notes: 2 duplicate_ref_id, 3 id_stem_mismatch · e.g. ref/blogs/a.md, ref/blogs/b.md, ref/papers/mismatch.md)"
        ),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn doctor_reference_ids_repair_through_rename_clears_the_row() {
    const NOW: &str = "2026-10-25 12:00:00";
    let temp = TempDir::new("bob-cli-doctor-reference-ids-repair");
    let (vault, _, _) = init_vault_sync_pair(&temp);
    for directory in ["lib/papers", "ref", "xlib"] {
        fs::create_dir_all(vault.join(directory)).expect("create vault dir");
    }
    write_area_note(&vault.join("sase.md"), "SASE");
    // Simulate legacy bad data: a synced reference whose recorded id no
    // longer matches its stem. A marker-bearing PDF plus `bob ref sync`
    // needs no pandoc, so this also runs on CI hosts without it.
    let pdf = vault.join("lib/papers/src.pdf");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: sase\n- title: Src\n- id: stale-id\n",
    );
    let sync = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob ref sync");
    assert_success(&sync);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "synced reference"]);
    let before = stdout(&run_doctor(&vault));
    assert!(
        before.contains(
            "reference ids: warn (1 id/stem mismatch · e.g. ref/papers/src.md — repair with bob ref rename <REF> <NAME>)"
        ),
        "{before}"
    );

    // The prescribed repair: a same-stem rename re-keys the id.
    let rename = bob_command()
        .arg("ref")
        .args(["rename", "src", "src"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob ref rename");
    assert_success(&rename);
    assert!(
        stdout(&rename).contains("id: stale-id → src"),
        "{}",
        format_output(&rename)
    );

    let after = stdout(&run_doctor(&vault));
    assert!(
        after
            .contains("reference ids: ok (1 note with an id, all match stems)"),
        "{after}"
    );
    assert!(after.contains("result: ok"), "{after}");
}

fn queue_xlib_pdf(vault: &std::path::Path, name: &str, age_secs: u64) {
    let path = vault.join(format!("xlib/chat/{name}.pdf"));
    write_file(&path, "%PDF-1.4 queued\n");
    let modified =
        std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(modified))
        .expect("set queued PDF mtime");
}

#[test]
fn doctor_xlib_rows_stay_plain_when_nothing_is_queued() {
    let temp = TempDir::new("bob-cli-doctor-xlib-empty");
    let vault = base_vault(&temp);
    write_queued_note(&vault, "tracked");
    commit_vault(&vault);

    let report = stdout(&run_doctor(&vault));
    assert!(report.contains("xlib_pending: 0\n"), "{report}");
    assert!(report.contains("intake_last: none recorded\n"), "{report}");
    assert!(!report.contains("xlib intake:"), "{report}");
}

#[test]
fn doctor_xlib_row_shows_the_oldest_age_without_warning_when_fresh() {
    let temp = TempDir::new("bob-cli-doctor-xlib-fresh");
    let vault = base_vault(&temp);
    queue_xlib_pdf(&vault, "fresh", 120);
    commit_vault(&vault);

    let report = stdout(&run_doctor(&vault));
    assert!(
        report.contains("xlib_pending: 1 · oldest 2 min\n"),
        "{report}"
    );
    assert!(!report.contains("xlib intake:"), "{report}");
}

#[test]
fn doctor_warns_when_a_queued_pdf_is_over_an_hour_old() {
    let temp = TempDir::new("bob-cli-doctor-xlib-stale");
    let vault = base_vault(&temp);
    queue_xlib_pdf(&vault, "fresh", 120);
    queue_xlib_pdf(&vault, "stale", 5 * 3600);
    commit_vault(&vault);

    let report = stdout(&run_doctor(&vault));
    assert!(
        report.contains("xlib_pending: 2 · oldest 5 h\n"),
        "{report}"
    );
    assert!(
        report.contains(
            "xlib intake: 2 queued, oldest 5 h — run bob ref scan -i (the bob-ref-intake timer drains it on athena and apollo)"
        ),
        "{report}"
    );
}

#[test]
fn doctor_intake_last_row_reads_the_recorded_status() {
    let temp = TempDir::new("bob-cli-doctor-intake-last");
    let vault = base_vault(&temp);
    write_queued_note(&vault, "tracked");
    commit_vault(&vault);
    let state = temp.path().join("state");
    let ago = |secs: i64| {
        (chrono::Local::now() - chrono::Duration::seconds(secs))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    };
    write_file(
        &state.join("bob-cli/ref/intake-status.json"),
        &format!(
            "{{\"schema_version\":1,\"last_success\":{{\"at\":\"{}\",\"command\":\"bob ref create\",\"filed\":1}},\"last_failure\":{{\"at\":\"{}\",\"command\":\"bob ref scan -i\",\"reason\":\"scan_busy\",\"message\":\"another scan\"}}}}",
            ago(7200),
            ago(180)
        ),
    );

    let output = bob_command()
        .args(["ref", "doctor", "--no-hooks"])
        .env("BOB_DIR", &vault)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("run bob ref doctor");
    let report = stdout(&output);
    assert!(
        report.contains(
            "intake_last: failure 3 min ago (scan_busy: another scan) · ok 2 h ago (bob ref create, 1 filed)\n"
        ),
        "{report}"
    );
}

#[test]
fn doctor_intake_last_row_keeps_a_multiline_message_on_one_line() {
    let temp = TempDir::new("bob-cli-doctor-intake-last-multiline");
    let vault = base_vault(&temp);
    write_queued_note(&vault, "tracked");
    commit_vault(&vault);
    let state = temp.path().join("state");
    let ago = (chrono::Local::now() - chrono::Duration::seconds(180))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let long = "x".repeat(130);
    write_file(
        &state.join("bob-cli/ref/intake-status.json"),
        &format!(
            "{{\"schema_version\":1,\"last_success\":null,\"last_failure\":{{\"at\":\"{ago}\",\"command\":\"bob ref scan -i\",\"reason\":\"pre_sync_failed\",\"message\":\"\\n  {long}\\nremote: origin refused\\n\"}}}}"
        ),
    );

    let output = bob_command()
        .args(["ref", "doctor", "--no-hooks"])
        .env("BOB_DIR", &vault)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("run bob ref doctor");
    let report = stdout(&output);
    let row = report
        .lines()
        .find(|line| line.starts_with("intake_last:"))
        .unwrap_or_else(|| panic!("missing intake_last row:\n{report}"));
    assert!(
        !row.contains('\n') && row.contains('…'),
        "intake_last must stay one truncated line: {row:?}\n{report}"
    );
    assert!(
        row.contains("pre_sync_failed:") && row.contains(&"x".repeat(120)),
        "{row}"
    );
    assert!(
        !row.contains("remote: origin refused"),
        "later lines must not spill: {row}"
    );
}
