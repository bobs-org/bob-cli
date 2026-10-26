//! Holds scans when an archived annotated PDF is restored to its live path.

use crate::support::*;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const HOLD_MESSAGE: &str = "Highlights wrote the replaced capture back";
const NOW: &str = "2026-10-26 12:00:00";

fn run_scan(
    vault: &Path,
    home: &Path,
    state: &Path,
    args: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", vault)
        .env("HOME", home)
        .env("BOB_NOW", NOW)
        .env("BOB_REF_JOBS_KICK", "off")
        .env("XDG_STATE_HOME", state);
    command.args(args);
    command.output().expect("run ref scan")
}

fn run_sync(
    vault: &Path,
    home: &Path,
    state: &Path,
    pdf: &Path,
    args: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("sync")
        .arg(pdf)
        .env("BOB_DIR", vault)
        .env("HOME", home)
        .env("BOB_NOW", NOW)
        .env("BOB_REF_JOBS_KICK", "off")
        .env("XDG_STATE_HOME", state);
    command.args(args);
    command.output().expect("run ref sync")
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).expect("scan JSON")
}

/// Add validated archive notes and PDF companions, then link them from the
/// live note's `earlier:` lineage. Each tuple is `(date, archived bytes)`.
fn add_archives(
    vault: &Path,
    note: &Path,
    live_stem: &str,
    archives: &[(&str, &[u8])],
) {
    let note_rel = note
        .strip_prefix(vault)
        .expect("live note is in vault")
        .to_string_lossy()
        .replace('\\', "/");
    let live_identity = note_rel.strip_suffix(".md").expect("note extension");
    let mut lineage = String::from("earlier:\n");
    for (date, bytes) in archives {
        let archive_stem = format!("{live_stem}__{date}");
        let archive_rel = format!("ref_history/papers/{archive_stem}");
        let archive_pdf_rel = format!("{archive_rel}__annotated.pdf");
        let archive_pdf = vault.join(&archive_pdf_rel);
        fs::create_dir_all(archive_pdf.parent().expect("archive parent"))
            .expect("create archive parent");
        fs::write(&archive_pdf, bytes).expect("write archived PDF");
        let hash = hex::encode(Sha256::digest(bytes));
        write_file(
            &vault.join(format!("{archive_rel}.md")),
            &format!(
                "---\nparent: \"[[{live_identity}]]\"\nannotated_pdf: \"[[{archive_pdf_rel}]]\"\nannotated_pdf_sha256: \"{hash}\"\n---\n\n# Archived capture\n"
            ),
        );
        lineage.push_str(&format!(
            "  - \"[[{archive_rel}|replaced {date} · 1 highlight]]\"\n"
        ));
    }

    let current = fs::read_to_string(note).expect("read live note");
    let (front, body) = current.split_once("\n---\n").expect("frontmatter");
    let updated = format!("{front}\n{lineage}---\n{body}");
    write_file(note, &updated);
}

fn set_note_status(note: &Path, from: &str, to: &str) {
    let current = fs::read_to_string(note).expect("read note");
    let updated = current.replacen(
        &format!("status: {from}"),
        &format!("status: {to}"),
        1,
    );
    assert_ne!(current, updated, "note status should change");
    write_file(note, &updated);
}

fn seed_v1_note(note: &Path, pdf_rel: &str) {
    write_file(
        note,
        &format!(
            "---\ntitle: Legacy reference\nstatus: wip\nparent: obsidian\n---\n\n- [/] #task #ref [[{pdf_rel}]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n"
        ),
    );
}

fn seed_area(vault: &Path) {
    write_file(
        &vault.join("obsidian.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n",
    );
}

#[test]
fn human_scan_holds_v1_and_v2_before_import_and_continues_other_pdfs() {
    let temp = TempDir::new("bob-cli-replaced-capture-human");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    let v2_pdf = vault.join("lib/papers/current.pdf");
    let v2_note = vault.join("ref/papers/current.md");
    let v1_pdf = vault.join("lib/papers/legacy.pdf");
    let v1_note = vault.join("ref/papers/legacy.md");
    seed_area(&vault);
    write_highlights_pdf(
        &v2_pdf,
        "- status: wip\n- parent: obsidian\n- title: Current Capture\n",
    );
    write_highlights_pdf(
        &v1_pdf,
        "- status: wip\n- parent: obsidian\n- title: Legacy Capture\n",
    );
    seed_v1_note(&v1_note, "lib/papers/legacy.pdf");

    assert_success(&run_scan(&vault, &home, &state, &[]));
    assert!(v2_note.is_file(), "initial scan creates v2 note");

    let v2_old_pdf = fs::read(&v2_pdf).expect("read original v2 PDF");
    let v1_old_pdf = fs::read(&v1_pdf).expect("read original v1 PDF");
    set_note_status(&v2_note, "wip", "next");
    set_note_status(&v1_note, "wip", "next");

    let screenshot = home.join("tmp/screenshots/20261026_120000.png");
    fs::create_dir_all(screenshot.parent().expect("screenshot parent"))
        .expect("create screenshot directory");
    fs::write(&screenshot, b"image-note-source-bytes")
        .expect("write screenshot");
    write_file(
        &v2_pdf.with_extension("md"),
        "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Current passage.\n\n- #task Follow up on the current passage.\n\n~/tmp/screenshots/20261026_120000.png\n",
    );
    write_file(
        &v1_pdf.with_extension("md"),
        "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Legacy passage.\n\n- #task Follow up on the legacy passage.\n",
    );

    let preview = run_sync(
        &vault,
        &home,
        &state,
        &v2_pdf,
        &["--dry-run", "--write-pdf"],
    );
    assert_success(&preview);
    let preview_text = stdout(&preview);
    assert!(
        preview_text.contains("annotation_tasks_create: 1"),
        "{preview_text}"
    );
    assert!(preview_text.contains("image_notes: 1"), "{preview_text}");

    add_archives(&vault, &v2_note, "current", &[("2026-10-25", &v2_old_pdf)]);
    add_archives(&vault, &v1_note, "legacy", &[("2026-10-25", &v1_old_pdf)]);
    let v2_note_before = fs::read(&v2_note).expect("snapshot v2 note");
    let v1_note_before = fs::read(&v1_note).expect("snapshot v1 note");
    let parent_note = vault.join("obsidian.md");
    let parent_before = fs::read(&parent_note).expect("snapshot reading tasks");

    let unaffected_pdf = vault.join("lib/papers/unaffected.pdf");
    let unaffected_note = vault.join("ref/papers/unaffected.md");
    write_highlights_pdf(
        &unaffected_pdf,
        "- status: ready\n- parent: obsidian\n- title: Unaffected\n",
    );

    let output = run_scan(&vault, &home, &state, &["--write-pdfs"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains(HOLD_MESSAGE), "{report}");
    assert!(report.contains("2 held"), "{report}");
    assert!(
        report.contains("unaffected"),
        "unaffected PDF must finish:\n{report}"
    );
    assert!(unaffected_note.is_file(), "unaffected PDF is still synced");
    assert_eq!(fs::read(&v2_pdf).expect("v2 PDF after scan"), v2_old_pdf);
    assert_eq!(fs::read(&v1_pdf).expect("v1 PDF after scan"), v1_old_pdf);
    assert_eq!(
        fs::read(&v2_note).expect("v2 note after scan"),
        v2_note_before
    );
    assert_eq!(
        fs::read(&v1_note).expect("v1 note after scan"),
        v1_note_before
    );
    let parent_after =
        fs::read_to_string(parent_note).expect("tasks after scan");
    assert!(parent_after.contains("unaffected"), "{parent_after}");
    assert!(
        !parent_after.contains("Follow up on the current passage")
            && !parent_after.contains("Follow up on the legacy passage"),
        "held sidecar tasks must not be imported:\n{parent_after}\nBefore:\n{}",
        String::from_utf8_lossy(&parent_before)
    );
    assert!(
        !vault.join("ref/papers/current.assets").exists(),
        "held image note must not write assets"
    );
}

#[test]
fn json_scan_checks_all_archives_and_keeps_dry_run_holds_as_previews() {
    let temp = TempDir::new("bob-cli-replaced-capture-json");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    let target_pdf = vault.join("lib/papers/target.pdf");
    let same_bytes_pdf = vault.join("lib/papers/same_bytes.pdf");
    let ordinary_pdf = vault.join("lib/papers/ordinary.pdf");
    let unsafe_pdf = vault.join("lib/papers/unsafe.pdf");
    seed_area(&vault);
    for (pdf, title) in [
        (&target_pdf, "Target"),
        (&ordinary_pdf, "Ordinary"),
        (&unsafe_pdf, "Unsafe"),
    ] {
        write_highlights_pdf(
            pdf,
            &format!("- status: wip\n- parent: obsidian\n- title: {title}\n"),
        );
    }
    fs::copy(&target_pdf, &same_bytes_pdf).expect("copy exact PDF bytes");
    assert_success(&run_scan(&vault, &home, &state, &[]));

    let target_note = vault.join("ref/papers/target.md");
    let ordinary_note = vault.join("ref/papers/ordinary.md");
    let unsafe_note = vault.join("ref/papers/unsafe.md");
    let target_current = fs::read(&target_pdf).expect("read target PDF");
    let old_target = b"older historical PDF bytes";
    add_archives(
        &vault,
        &target_note,
        "target",
        &[("2026-10-24", old_target), ("2026-10-25", &target_current)],
    );
    fs::remove_file(
        vault.join("ref_history/papers/target__2026-10-24__annotated.pdf"),
    )
    .expect("remove archived PDF while retaining its recorded digest");
    // Match the older of two valid archives. This proves the guard checks
    // the full lineage rather than only the most recent replacement.
    fs::write(&target_pdf, old_target).expect("restore older archived bytes");
    let ordinary_old = b"old ordinary capture bytes";
    add_archives(
        &vault,
        &ordinary_note,
        "ordinary",
        &[("2026-10-25", ordinary_old)],
    );
    let unsafe_contents =
        fs::read_to_string(&unsafe_note).expect("unsafe note");
    let unsafe_frontmatter = unsafe_contents
        .split_once("\n---\n")
        .expect("unsafe frontmatter")
        .0;
    let unsafe_body = unsafe_contents
        .split_once("\n---\n")
        .expect("unsafe body")
        .1;
    write_file(
        &unsafe_note,
        &format!(
            "{unsafe_frontmatter}\nearlier:\n  - \"[[ref_history/../ref/papers/target__2026-10-25|replaced 2026-10-25 · 1 highlight]]\"\n---\n{unsafe_body}"
        ),
    );

    let dry_state = temp.path().join("dry-state");
    let dry = run_scan(&vault, &home, &dry_state, &["--dry-run", "-f", "json"]);
    assert_success(&dry);
    let dry_json = json(&dry);
    assert_eq!(dry_json["held"], serde_json::json!([]));
    assert_eq!(dry_json["summary"]["held"], 0);
    assert_eq!(dry_json["preview_holds"].as_array().unwrap().len(), 1);
    assert_eq!(dry_json["preview_holds"][0]["pdf"], "lib/papers/target.pdf");
    assert_eq!(dry_json["preview_holds"][0]["reason"], "replaced_capture");
    assert_eq!(dry_json["preview_holds"][0]["message"], HOLD_MESSAGE);
    assert!(
        !dry_state.join("bob-cli/ref/last-scan.json").exists(),
        "dry-run must not write a scan receipt"
    );

    let output = run_scan(&vault, &home, &state, &["-f", "json"]);
    assert_success(&output);
    let document = json(&output);
    assert_eq!(document["preview_holds"], serde_json::Value::Null);
    assert_eq!(document["held"].as_array().unwrap().len(), 1);
    assert_eq!(document["held"][0]["pdf"], "lib/papers/target.pdf");
    assert_eq!(document["held"][0]["reason"], "replaced_capture");
    assert_eq!(document["held"][0]["message"], HOLD_MESSAGE);
    assert_eq!(document["summary"]["held"], 1);
    assert_eq!(
        fs::read(&target_pdf).expect("held PDF stays unchanged"),
        old_target
    );
    let receipt = state.join("bob-cli/ref/last-scan.json");
    assert_eq!(
        fs::read(&receipt).expect("write scan receipt"),
        stdout(&output).as_bytes()
    );
}

#[test]
fn explicit_sync_holds_in_human_and_dry_run_modes() {
    let temp = TempDir::new("bob-cli-replaced-capture-sync");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    let sync_state = temp.path().join("sync-state");
    let pdf = vault.join("lib/papers/explicit.pdf");
    let note = vault.join("ref/papers/explicit.md");
    seed_area(&vault);
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Explicit\n",
    );
    assert_success(&run_scan(&vault, &home, &state, &[]));
    let original = fs::read(&pdf).expect("read PDF");
    add_archives(&vault, &note, "explicit", &[("2026-10-25", &original)]);
    let note_before = fs::read(&note).expect("snapshot note");

    for args in [&["--dry-run"][..], &[][..]] {
        let output = run_sync(&vault, &home, &sync_state, &pdf, args);
        assert_success(&output);
        let report = stdout(&output);
        assert!(report.contains(HOLD_MESSAGE), "{report}");
        assert!(report.contains("pdf_marker_action: held"), "{report}");
        assert_eq!(fs::read(&pdf).expect("PDF remains unchanged"), original);
        assert_eq!(
            fs::read(&note).expect("note remains unchanged"),
            note_before
        );
    }
    assert!(
        !sync_state.join("bob-cli/ref/last-scan.json").exists(),
        "explicit sync never writes a scan receipt"
    );
}
