//! CLI holds for `highlights.coordinate_pdf_writes` and `-a, --attended`.
use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

fn write_coordinate_config(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("config.yml");
    write_file(&path, "highlights:\n  coordinate_pdf_writes: true\n");
    path
}

fn settle_v1_ready_for_marker_write(vault: &Path) -> (PathBuf, PathBuf) {
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_area_note(&vault.join("obsidian.md"), "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    seed_v1_ref_note(&note, "- [/] #task #ref [[lib/example.pdf]] #hide ^ref");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", vault)
            .output()
            .expect("settle v1 note"),
    );
    let generated = fs::read_to_string(&note).expect("read settled note");
    let updated = generated.replace("- [/] #task", "- [x] #task");
    assert_ne!(updated, generated);
    write_file(&note, &updated);
    (pdf, note)
}

#[test]
fn unattended_scan_holds_editor_active_when_the_probe_says_reading() {
    let temp = TempDir::new("bob-cli-marker-write-editor-active");
    let vault = temp.path().join("vault");
    let config = write_coordinate_config(&temp);
    let (pdf, note) = settle_v1_ready_for_marker_write(&vault);
    let pdf_before = fs::read(&pdf).expect("pdf before");
    let note_before = fs::read_to_string(&note).expect("note before");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "1")
        .output()
        .expect("unattended hold scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("held")
            && report.contains(
                "waiting while you read in Highlights · retrying next scan"
            )
            && report.contains("1 held"),
        "expected concise editor_active hold:\n{}",
        format_output(&output)
    );
    assert_eq!(fs::read(&pdf).expect("pdf after"), pdf_before);
    assert_eq!(fs::read_to_string(&note).expect("note after"), note_before);
}

#[test]
fn verbose_unattended_scan_names_the_reading_hold() {
    let temp = TempDir::new("bob-cli-marker-write-verbose-hold");
    let vault = temp.path().join("vault");
    let config = write_coordinate_config(&temp);
    let (_pdf, _note) = settle_v1_ready_for_marker_write(&vault);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "4")
        .output()
        .expect("verbose hold scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_write_coordination: on")
            && report.contains(
                "reading_hold: holding (Highlights frontmost, last input 4 s ago)"
            )
            && report.contains(
                "pdf_marker_action: held (waiting while you read in Highlights; retrying next scan)"
            )
            && report.contains("pdf_markers_held: 1"),
        "expected verbose editor_active hold:\n{}",
        format_output(&output)
    );
}

#[test]
fn json_unattended_scan_counts_a_hold_as_neither_note_nor_failure() {
    let temp = TempDir::new("bob-cli-marker-write-json-hold");
    let vault = temp.path().join("vault");
    let config = write_coordinate_config(&temp);
    let (pdf, note) = settle_v1_ready_for_marker_write(&vault);
    let pdf_before = fs::read(&pdf).expect("pdf before");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .arg("--format")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "1")
        .output()
        .expect("json hold scan");

    assert_success(&output);
    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse scan JSON");
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["pdfs"], 1);
    assert_eq!(document["summary"]["created"], 0);
    assert_eq!(document["summary"]["updated"], 0);
    assert_eq!(document["summary"]["unchanged"], 0);
    assert_eq!(document["summary"]["markers"], 0);
    assert_eq!(document["summary"]["failures"], 0);
    assert!(document["summary"].get("held").is_none());
    assert_eq!(document["notes"].as_array().map(Vec::len), Some(0));
    assert_eq!(document["failures"].as_array().map(Vec::len), Some(0));
    assert_eq!(fs::read(&pdf).expect("pdf after"), pdf_before);
    assert!(
        fs::read_to_string(&note)
            .expect("note after")
            .contains("- [x] #task"),
        "held scan must leave the checkbox edit on the note"
    );
}

#[test]
fn attended_scan_bypasses_the_reading_hold() {
    let temp = TempDir::new("bob-cli-marker-write-attended");
    let vault = temp.path().join("vault");
    let config = write_coordinate_config(&temp);
    let (pdf, note) = settle_v1_ready_for_marker_write(&vault);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("-a")
        .arg("--write-pdfs")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "1")
        .output()
        .expect("attended scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_write_coordination: on")
            && report.contains("reading_hold: attended")
            && !report.contains("pdf_markers_held:")
            && report.contains("pdf_markers_updated: 1"),
        "expected attended scan to write:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(
        fs::read_to_string(&note)
            .expect("note after")
            .contains("status: read\n"),
        "{}",
        fs::read_to_string(&note).expect("note after")
    );
}

#[test]
fn ref_sync_bypasses_the_reading_hold() {
    let temp = TempDir::new("bob-cli-marker-write-sync-exempt");
    let vault = temp.path().join("vault");
    let config = write_coordinate_config(&temp);
    let (pdf, note) = settle_v1_ready_for_marker_write(&vault);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "1")
        .output()
        .expect("sync write");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_marker_action: updated")
            && report.contains("writes: note,pdf")
            && !report.contains("held"),
        "expected ref sync to write through a reading probe:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(fs::read_to_string(&note)
        .expect("note after")
        .contains("status: read\n"));
}

#[test]
fn key_off_scan_ignores_the_reading_probe_and_omits_coordination_header() {
    let temp = TempDir::new("bob-cli-marker-write-key-off");
    let vault = temp.path().join("vault");
    let (pdf, _note) = settle_v1_ready_for_marker_write(&vault);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_REF_READING_PROBE", "reading")
        .env("BOB_REF_HID_IDLE_SECONDS", "1")
        .output()
        .expect("key-off scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pdf_write_coordination:")
            && !report.contains("reading_hold:")
            && !report.contains("pdf_markers_held:")
            && report.contains("pdf_markers_updated: 1"),
        "key-off verbose output must stay uncoordinated:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
}
