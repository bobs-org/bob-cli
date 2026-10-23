//! Stale Highlights save: restore the replaced marker instead of adopting it.

use crate::support::*;
use std::fs;
use std::path::Path;

const READY_MARKER: &str =
    "- status: ready\n- parent: obsidian\n- title: Stale Save\n";
const WIP_MARKER: &str =
    "- status: wip\n- parent: obsidian\n- title: Stale Save\n";
const STALE_REASON: &str =
    "marker reverted to the copy bob replaced (stale editor save); restoring it";
const STALE_DETAIL: &str = "marker restored after a stale save";

fn scan(vault: &Path, args: &[&str]) -> std::process::Output {
    bob_command()
        .arg("ref")
        .arg("scan")
        .args(args)
        .env("BOB_DIR", vault)
        .output()
        .expect("run bob ref scan")
}

fn yaml_field(contents: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    contents.lines().find_map(|line| {
        let rest = line.strip_prefix(&prefix)?.trim();
        Some(rest.trim_matches('"').to_string())
    })
}

fn set_ref_task_mark(path: &Path, mark: char) {
    let contents = fs::read_to_string(path).expect("read note");
    let mut found = false;
    let mut out = String::new();
    for line in contents.lines() {
        if !found
            && line.contains("#task")
            && line.contains("#ref")
            && let Some(start) = line.find('[')
            && line.len() > start + 2
            && line.as_bytes().get(start + 2) == Some(&b']')
        {
            let mut updated = line.to_string();
            updated.replace_range(start + 1..start + 2, &mark.to_string());
            out.push_str(&updated);
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !contents.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    assert!(found, "missing ref task in {}:\n{contents}", path.display());
    write_file(path, &out);
}

fn write_parent_area(vault: &Path) {
    write_file(
        &vault.join("obsidian.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n## Tasks\n",
    );
}

fn birth_v2(vault: &Path, pdf: &Path, marker: &str) {
    write_parent_area(vault);
    write_highlights_pdf(pdf, marker);
    assert_success(&scan(vault, &[]));
}

fn promote_v2_to_next(vault: &Path, pdf: &Path, parent: &Path, note: &Path) {
    set_ref_task_mark(parent, '*');
    let output = scan(vault, &["--write-pdfs"]);
    assert_success(&output);
    let note_after = fs::read_to_string(note).expect("read note");
    assert!(note_after.contains("status: next\n"), "{note_after}");
    assert!(
        yaml_field(&note_after, "highlights_marker_replaced_hash").is_some(),
        "marker write should store replaced hash:\n{note_after}"
    );
    assert!(
        pdf_marker_contents(pdf).contains("- status: next\n"),
        "{}",
        pdf_marker_contents(pdf)
    );
}

fn parent_has_mark(parent: &str, mark: char) -> bool {
    parent.lines().any(|line| {
        line.contains("#task")
            && line.contains("#ref")
            && line.contains(&format!("[{mark}]"))
    })
}

#[test]
fn v2_stale_save_keeps_next_lane_and_restores_marker() {
    let temp = TempDir::new("bob-cli-stale-save-v2-next");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_save.pdf");
    let note = vault.join("ref/chat/stale_save.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    set_pdf_marker_contents(&pdf, READY_MARKER);

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(STALE_DETAIL),
        "expected restore detail:\n{}",
        format_output(&output)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    let parent_after = fs::read_to_string(&parent).expect("read parent");
    assert!(note_after.contains("status: next\n"), "{note_after}");
    assert!(parent_has_mark(&parent_after, '*'), "{parent_after}");
    assert!(
        pdf_marker_contents(&pdf).contains("- status: next\n"),
        "{}",
        pdf_marker_contents(&pdf)
    );
    assert!(
        yaml_field(&note_after, "highlights_marker_replaced_hash").is_some(),
        "restore keeps replaced hash:\n{note_after}"
    );
}

#[test]
fn v2_stale_save_does_not_reopen_a_finished_task() {
    let temp = TempDir::new("bob-cli-stale-save-v2-read");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_read.pdf");
    let note = vault.join("ref/chat/stale_read.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, WIP_MARKER);
    set_ref_task_mark(&parent, 'x');
    assert_success(&scan(&vault, &["--write-pdfs"]));
    let note_after_close = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_close.contains("status: read\n"),
        "{note_after_close}"
    );
    assert!(
        yaml_field(&note_after_close, "highlights_marker_replaced_hash")
            .is_some(),
        "{note_after_close}"
    );
    set_pdf_marker_contents(&pdf, WIP_MARKER);

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains(STALE_DETAIL),
        "{}",
        format_output(&output)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    let parent_after = fs::read_to_string(&parent).expect("read parent");
    assert!(note_after.contains("status: read\n"), "{note_after}");
    assert!(parent_has_mark(&parent_after, 'x'), "{parent_after}");
    assert!(
        pdf_marker_contents(&pdf).contains("- status: read\n"),
        "{}",
        pdf_marker_contents(&pdf)
    );
}

#[test]
fn v2_gesture_during_stale_save_applies_without_conflict() {
    let temp = TempDir::new("bob-cli-stale-save-v2-gesture");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_gesture.pdf");
    let note = vault.join("ref/chat/stale_gesture.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    set_pdf_marker_contents(&pdf, READY_MARKER);
    set_ref_task_mark(&parent, '/');

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    let combined = format_output(&output);
    assert!(
        !combined.contains("conflict"),
        "stale marker must not conflict with a task gesture:\n{combined}"
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    let parent_after = fs::read_to_string(&parent).expect("read parent");
    assert!(note_after.contains("status: wip\n"), "{note_after}");
    assert!(parent_has_mark(&parent_after, '/'), "{parent_after}");
    assert!(
        pdf_marker_contents(&pdf).contains("- status: wip\n"),
        "{}",
        pdf_marker_contents(&pdf)
    );
}

#[test]
fn genuine_marker_edit_is_still_adopted() {
    let temp = TempDir::new("bob-cli-stale-save-genuine");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/genuine_edit.pdf");
    let note = vault.join("ref/chat/genuine_edit.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    set_pdf_marker_contents(&pdf, WIP_MARKER);

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains(STALE_DETAIL),
        "a genuine edit is not a stale restore:\n{}",
        format_output(&output)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    let parent_after = fs::read_to_string(&parent).expect("read parent");
    assert!(note_after.contains("status: wip\n"), "{note_after}");
    assert!(parent_has_mark(&parent_after, '/'), "{parent_after}");
    assert!(
        pdf_marker_contents(&pdf).contains("- status: wip\n"),
        "{}",
        pdf_marker_contents(&pdf)
    );
}

#[test]
fn missing_replaced_hash_disables_the_guard() {
    let temp = TempDir::new("bob-cli-stale-save-legacy");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/legacy_hash.pdf");
    let note = vault.join("ref/chat/legacy_hash.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    let with_hash = fs::read_to_string(&note).expect("read note");
    let stripped: String = with_hash
        .lines()
        .filter(|line| !line.starts_with("highlights_marker_replaced_hash:"))
        .flat_map(|line| [line, "\n"])
        .collect();
    write_file(&note, &stripped);
    set_pdf_marker_contents(&pdf, READY_MARKER);

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    assert!(
        !stdout(&output).contains(STALE_DETAIL),
        "legacy notes without the field adopt the marker:\n{}",
        format_output(&output)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    let parent_after = fs::read_to_string(&parent).expect("read parent");
    assert!(note_after.contains("status: ready\n"), "{note_after}");
    assert!(parent_has_mark(&parent_after, ' '), "{parent_after}");
}

#[test]
fn replaced_hash_drops_on_a_note_write_that_observes_agreement() {
    let temp = TempDir::new("bob-cli-stale-save-drop");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/drop_hash.pdf");
    let note = vault.join("ref/chat/drop_hash.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    let sidecar = pdf.with_extension("md");
    write_file(
        &sidecar,
        "\
# Stale Save

## Page 1

- status: next
- parent: obsidian

---

> A late highlight.
",
    );

    let output = scan(&vault, &[]);
    assert_success(&output);
    let note_after = fs::read_to_string(&note).expect("read note");
    assert!(
        yaml_field(&note_after, "highlights_marker_replaced_hash").is_none(),
        "agreement should drop replaced hash:\n{note_after}"
    );
    assert!(
        pdf_marker_contents(&pdf).contains("- status: next\n"),
        "drop must not rewrite the marker:\n{}",
        pdf_marker_contents(&pdf)
    );
}

#[test]
fn no_write_pdfs_skips_restore_alone_without_refusal() {
    let temp = TempDir::new("bob-cli-stale-save-no-w");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_nowrite.pdf");
    let note = vault.join("ref/chat/stale_nowrite.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    let note_before = fs::read_to_string(&note).expect("read note");
    set_pdf_marker_contents(&pdf, READY_MARKER);

    let output = scan(&vault, &[]);
    assert_success(&output);
    let combined = format_output(&output);
    assert!(
        !combined.contains("needs_write_pdfs")
            && !combined.contains("--write-pdf"),
        "restore-alone must not refuse without -w:\n{combined}"
    );
    assert!(
        pdf_marker_contents(&pdf).contains("- status: ready\n"),
        "without -w the stale marker stays:\n{}",
        pdf_marker_contents(&pdf)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    assert!(note_after.contains("status: next\n"), "{note_after}");
    assert_eq!(
        yaml_field(&note_after, "highlights_marker_replaced_hash"),
        yaml_field(&note_before, "highlights_marker_replaced_hash")
    );
}

#[test]
fn dry_run_previews_the_stale_restore() {
    let temp = TempDir::new("bob-cli-stale-save-dry");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_dry.pdf");
    let note = vault.join("ref/chat/stale_dry.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    let note_before = fs::read_to_string(&note).expect("read note");
    let pdf_before = fs::read(&pdf).expect("pdf bytes");
    set_pdf_marker_contents(&pdf, READY_MARKER);
    let stale_pdf = fs::read(&pdf).expect("stale pdf");

    let output = scan(&vault, &["--dry-run", "--verbose"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(STALE_REASON)
            && (report.contains("would-update")
                || report.contains(STALE_DETAIL)),
        "dry run should preview the restore:\n{}",
        format_output(&output)
    );
    assert_eq!(fs::read(&pdf).expect("pdf after dry"), stale_pdf);
    assert_eq!(
        fs::read_to_string(&note).expect("note after dry"),
        note_before
    );
    assert_ne!(stale_pdf, pdf_before);
}

#[test]
fn v1_stale_save_keeps_next_lane_and_restores_marker() {
    let temp = TempDir::new("bob-cli-stale-save-v1");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_area_note(&vault.join("obsidian.md"), "Obsidian");
    write_highlights_pdf(&pdf, READY_MARKER);
    seed_v1_ref_note(&note, "- [ ] #task #ref [[lib/example.pdf]] #hide ^ref");
    assert_success(&scan(&vault, &[]));
    set_ref_task_mark(&note, '*');
    assert_success(&scan(&vault, &["--write-pdfs"]));
    let settled = fs::read_to_string(&note).expect("read note");
    assert!(settled.contains("status: next\n"), "{settled}");
    assert!(
        yaml_field(&settled, "highlights_marker_replaced_hash").is_some(),
        "{settled}"
    );
    set_pdf_marker_contents(&pdf, READY_MARKER);

    let output = scan(&vault, &["--write-pdfs"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains(STALE_DETAIL),
        "{}",
        format_output(&output)
    );
    let note_after = fs::read_to_string(&note).expect("read note");
    assert!(note_after.contains("status: next\n"), "{note_after}");
    assert!(
        note_after.contains("- [*] #task #ref [[lib/example.pdf]]"),
        "{note_after}"
    );
    assert!(
        pdf_marker_contents(&pdf).contains("- status: next\n"),
        "{}",
        pdf_marker_contents(&pdf)
    );
}

#[test]
fn json_marker_write_restored_is_true_for_a_stale_save() {
    let temp = TempDir::new("bob-cli-stale-save-json");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/stale_json.pdf");
    let note = vault.join("ref/chat/stale_json.md");
    let parent = vault.join("obsidian.md");
    birth_v2(&vault, &pdf, READY_MARKER);
    promote_v2_to_next(&vault, &pdf, &parent, &note);
    set_pdf_marker_contents(&pdf, READY_MARKER);

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .args(["--write-pdfs", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-23 17:04:00")
        .output()
        .expect("json restore scan");
    assert_eq!(
        output.status.code(),
        Some(0),
        "json restore scan should succeed:\n{}",
        format_output(&output)
    );
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "{}", format_output(&output));
    let document: serde_json::Value =
        serde_json::from_str(text.trim()).expect("json");
    assert_eq!(document["ok"], true);
    assert_eq!(document["schema_version"], 1);
    let notes = document["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(
        notes[0]["marker_write"],
        serde_json::json!({
            "mode": "direct",
            "editor_saved_first": false,
            "status": {"from": "ready", "to": "next"},
            "restored": true,
        })
    );
}
