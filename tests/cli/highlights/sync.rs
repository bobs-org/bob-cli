//! Marker frontmatter sync and dirty-note guards.

use crate::support::*;
use std::fs;
use std::path::Path;

#[test]
fn highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note() {
    let temp = TempDir::new("bob-cli-highlights-ref-create");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/systems-performance.pdf");
    let note = vault.join("ref/systems-performance.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n- id: systems-performance\n- topics: [linux, performance]\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated ref note");
    assert!(contents.contains("status: wip\n"), "{contents}");
    assert!(
        contents.contains("parent: \"[[obsidian]]\"\n"),
        "{contents}"
    );
    assert!(contents.contains("type: \"[[ref]]\"\n"), "{contents}");
    assert!(
        contents.contains("title: \"Systems Performance\"\n"),
        "{contents}"
    );
    assert!(
        contents.contains("topics: [linux, performance]\n"),
        "{contents}"
    );
    assert!(contents.contains("id: systems-performance\n"), "{contents}");
    assert!(
        !contents.contains("highlights_marker_fields"),
        "id should not require an unknown-field opt-in:\n{contents}"
    );
    assert!(
        contents.contains("source_pdf: lib/systems-performance.pdf\n"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [/] #task #ref [[lib/systems-performance.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    assert!(
        !contents.contains("ref_type:"),
        "top-level library PDFs should not derive ref_type:\n{contents}"
    );
    assert!(contents.contains("highlights_marker_hash: "), "{contents}");
    assert!(contents.contains("highlights_marker_base: "), "{contents}");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat bob highlights sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "bare marker parent should be idempotent:\n{}",
        format_output(&output)
    );
    let repeat_contents =
        fs::read_to_string(&note).expect("read repeat-synced ref note");
    assert!(
        repeat_contents.contains("id: systems-performance\n"),
        "{repeat_contents}"
    );
    assert!(
        !repeat_contents.contains("highlights_marker_fields"),
        "id should remain standard after repeat sync:\n{repeat_contents}"
    );

    let edited = fs::read_to_string(&note)
        .expect("read generated ref note")
        .replace("parent: \"[[obsidian]]\"\n", "parent: obsidian\n");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync bare frontmatter parent");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read normalized ref note");
    assert!(
        contents.contains("parent: \"[[obsidian]]\"\n"),
        "{contents}"
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_sync_preserves_legacy_research_frontmatter() {
    let temp = TempDir::new("bob-cli-highlights-ref-legacy-research");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/artifact-reference-rendering.pdf");
    let note = vault.join("ref/artifact-reference-rendering.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Artifact Reference Rendering\n- research: 202608/artifact_reference_rendering.md\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync with legacy research");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated ref note");
    assert!(
        contents.contains("research: 202608/artifact_reference_rendering.md\n"),
        "{contents}"
    );
    assert!(!contents.contains("\nid: "), "{contents}");
    assert!(
        !contents.contains("highlights_marker_fields"),
        "legacy research should not require an unknown-field opt-in:\n{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat bob highlights sync with legacy research");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "legacy research should be idempotent:\n{}",
        format_output(&output)
    );
    assert!(
        pdf_marker_contents(&pdf)
            .contains("- research: 202608/artifact_reference_rendering.md\n"),
        "legacy research must remain in the PDF marker"
    );
}

#[test]
fn highlights_ref_sync_dry_run_reads_literal_marker_newlines() {
    let temp = TempDir::new("bob-cli-highlights-ref-literal-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/obsidian-docs.pdf");
    let note = vault.join("ref/obsidian-docs.md");
    write_highlights_pdf(&pdf, "");
    set_pdf_marker_literal_contents(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Obsidian Docs\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run sync literal marker");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("sync_source: marker")
            && report.contains("note_action: create")
            && report.contains("writes: none"),
        "literal marker should parse as a complete marker list:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "dry-run must not create ref note");
}

#[test]
fn highlights_ref_sync_rejects_missing_marker_status_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-status.pdf");
    let note = vault.join("ref/missing-status.md");
    write_highlights_pdf(&pdf, "- parent: obsidian\n- title: Missing Status\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing marker status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: status"),
        "expected missing status error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker error"
    );
}

#[test]
fn highlights_ref_sync_rejects_unsupported_marker_status_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-unsupported-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/unsupported-status.pdf");
    let note = vault.join("ref/unsupported-status.md");
    write_highlights_pdf(&pdf, "- status: queued\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "unsupported marker status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("marker has unsupported status \"queued\""),
        "expected unsupported status error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker status error"
    );
}

#[test]
fn highlights_ref_sync_rejects_missing_marker_parent_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-parent.pdf");
    let note = vault.join("ref/missing-parent.md");
    write_highlights_pdf(&pdf, "- status: wip\n- title: Missing Parent\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing marker parent should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: parent"),
        "expected missing parent error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker error"
    );
}

#[test]
fn highlights_ref_sync_rejects_malformed_and_duplicate_marker_lists() {
    let cases = [
        (
            "malformed-marker",
            "status: wip\n- parent: obsidian\n",
            "invalid marker item on line 1",
        ),
        (
            "duplicate-marker-key",
            "- status: wip\n- parent: obsidian\n- Status: read\n",
            "duplicate marker key on line 3",
        ),
        (
            "managed-type-marker-key",
            "- status: wip\n- parent: obsidian\n- type: [[book]]\n",
            "'type' is command-managed",
        ),
        (
            "managed-ref-type-marker-key",
            "- status: wip\n- parent: obsidian\n- ref_type: books\n",
            "'ref_type' is command-managed",
        ),
    ];

    for (name, marker, expected_error) in cases {
        let temp = TempDir::new(&format!("bob-cli-highlights-ref-{name}"));
        let vault = temp.path().join("vault");
        let pdf = vault.join(format!("lib/{name}.pdf"));
        let note = vault.join(format!("ref/{name}.md"));
        write_highlights_pdf(&pdf, marker);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob highlights sync for {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "invalid marker should fail for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(expected_error),
            "expected marker validation error for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            !note.exists(),
            "sync must not create a note for invalid marker case {name}"
        );
    }
}

#[test]
fn highlights_ref_sync_refuses_dirty_target_note_before_writing() {
    let temp = TempDir::new("bob-cli-highlights-ref-dirty-note");
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
    let dirty_note = fs::read_to_string(&note)
        .expect("read note")
        .replace("## Highlights\n\n", "Local edit.\n\n## Highlights\n\n")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &dirty_note);
    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty target note");

    assert_eq!(
        output.status.code(),
        Some(1),
        "dirty target note should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("refusing to modify dirty vault files")
            && stderr(&output).contains("ref/example.md"),
        "expected dirty target report:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after refusal"),
        dirty_note
    );
}

#[test]
fn highlights_ref_sync_allows_dirty_tracked_frontmatter_writeback() {
    let temp = TempDir::new("bob-cli-highlights-ref-dirty-frontmatter");
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
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: read")
        .replace("- [/] #task", "- [x] #task");
    write_file(&note, &edited);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty frontmatter");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains("status: read\n"), "{contents}");
}

fn set_pdf_marker_literal_contents(path: &Path, marker_contents: &str) {
    let mut doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    doc.get_object_mut(marker_id)
        .expect("get marker object")
        .as_dict_mut()
        .expect("marker is dictionary")
        .set("Contents", pdf_literal_string(marker_contents));
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

fn pdf_literal_string(contents: &str) -> lopdf::Object {
    lopdf::Object::String(
        contents.as_bytes().to_vec(),
        lopdf::StringFormat::Literal,
    )
}
