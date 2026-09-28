//! Sidecar render and annotation-task routing.

use crate::support::*;
use chrono::Local;
use sha2::Digest;
use sha2::Sha256;
use std::ffi::OsStr;
use std::fs;

#[test]
fn highlights_ref_sync_renders_sidecar_highlights_and_notes() {
    let temp = TempDir::new("bob-cli-highlights-ref-sidecar-create");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/systems-performance.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/systems-performance.md");
    let old_flat_note = vault.join("ref/systems-performance.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n",
    );
    write_file(
        &sidecar,
        "\
# Systems Performance

## Page 12

Note: marker note mirrored from the PDF

---

> Latency is not throughput.

Comment: Compare this with SLO notes.

---

Note: Keep a standalone observation after the marker.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync sidecar highlights");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(contents.contains("ref_type: books\n"), "{contents}");
    assert!(
        contents.contains("source_pdf: lib/books/systems-performance.pdf\n"),
        "{contents}"
    );
    assert!(
        contents
            .contains("highlights_sidecar: lib/books/systems-performance.md\n"),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 2\n"), "{contents}");
    assert!(contents.contains("highlights_synced_at: "), "{contents}");
    assert!(contents.contains("# Systems Performance\n"), "{contents}");
    assert!(
        contents.contains("- [/] #task #ref [[lib/books/systems-performance.pdf]] #hide ^ref\n"),
        "{contents}"
    );
    assert!(
        contents.contains("## Highlights\n\n<!-- highlights:begin -->\n"),
        "{contents}"
    );
    assert!(!contents.contains("## Summary\n"), "{contents}");
    assert!(!contents.contains("## My Notes\n"), "{contents}");
    assert!(contents.contains("### Page 12\n"), "{contents}");
    assert!(
        contents.contains("> [!quote] Latency is not throughput.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > [!note] Comment Compare this with SLO notes.\n"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "> [!note] Keep a standalone observation after the marker.\n"
        ),
        "{contents}"
    );
    assert!(
        !contents.contains("marker note mirrored"),
        "first standalone sidecar note should be excluded:\n{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 2, "{contents}");
    assert!(
        !old_flat_note.exists(),
        "nested sync must not create the old flat reference note"
    );

    let stale_ref_type =
        contents.replace("ref_type: books\n", "ref_type: stale\n");
    write_file(&note, &stale_ref_type);
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync stale ref_type");

    assert_success(&output);
    let refreshed = fs::read_to_string(&note).expect("read refreshed note");
    assert!(refreshed.contains("ref_type: books\n"), "{refreshed}");
    assert!(!refreshed.contains("ref_type: stale\n"), "{refreshed}");
}

#[test]
fn highlights_ref_sync_renders_textbundle_image_selections() {
    let temp = TempDir::new("bob-cli-highlights-ref-image-textbundle");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/figures.pdf");
    let textbundle = pdf.with_extension("textbundle");
    let sidecar = textbundle.join("text.md");
    let source_asset = textbundle.join("assets/figure.png");
    let note = vault.join("ref/books/figures.md");
    let note_assets_dir = vault.join("ref/books/figures.assets");
    let image_bytes = b"synthetic png bytes";

    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Figures\n",
    );
    write_file(
        &sidecar,
        "\
# Figures

## Page 12

Note: marker note mirrored from the PDF

---

![Latency figure](assets/figure.png)

Comment: Compare this figure with p.14.
",
    );
    fs::create_dir_all(source_asset.parent().expect("asset parent"))
        .expect("create source asset parent");
    fs::write(&source_asset, image_bytes).expect("write source image asset");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg("--dry-run")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run image textbundle sync");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("images: 1")
            && dry_run.contains("image_assets: 1")
            && dry_run.contains("writes: none"),
        "expected dry-run image report:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "dry-run must not create note");
    assert!(
        !note_assets_dir.exists(),
        "dry-run must not create note assets dir"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync image textbundle");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("images: 1")
            && report.contains("image_assets_written: 1")
            && report.contains("writes: note"),
        "expected write image report:\n{}",
        format_output(&output)
    );
    let contents = fs::read_to_string(&note).expect("read image note");
    assert!(
        contents.contains(
            "highlights_sidecar: lib/books/figures.textbundle/text.md\n"
        ),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 1\n"), "{contents}");
    assert!(
        contents.contains("> [!quote] Image ![[ref/books/figures.assets/h-"),
        "{contents}"
    );
    assert!(
        contents
            .contains("> > [!note] Comment Compare this figure with p.14.\n"),
        "{contents}"
    );

    let assets = fs::read_dir(&note_assets_dir)
        .expect("read note assets dir")
        .map(|entry| entry.expect("asset entry").path())
        .collect::<Vec<_>>();
    assert_eq!(assets.len(), 1, "expected one copied image asset");
    let copied_asset = &assets[0];
    let file_name = copied_asset
        .file_name()
        .and_then(OsStr::to_str)
        .expect("asset file name");
    assert!(
        file_name.starts_with("h-") && file_name.ends_with(".png"),
        "unexpected asset filename: {file_name}"
    );
    assert_eq!(
        fs::read(copied_asset).expect("read copied image asset"),
        image_bytes
    );
    assert!(
        contents
            .contains(&format!("![[ref/books/figures.assets/{file_name}]]")),
        "{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat image textbundle sync");

    assert_success(&output);
    let repeated = stdout(&output);
    assert!(
        repeated.contains("image_assets_written: 0")
            && repeated.contains("image_assets_skipped: 1")
            && repeated.contains("writes: none"),
        "expected idempotent image report:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_sync_supports_linked_sidecar_style() {
    let temp = TempDir::new("bob-cli-highlights-ref-linked-sidecar");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/highlights-ref-sync.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/highlights-ref-sync.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Highlights Reference Note Sync\n",
    );
    write_file(
        &sidecar,
        "\
# Highlights Reference Note Sync

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

***

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> It only writes the PDF marker when frontmatter is the selected
source and --write-pdf is supplied.

- Support sase tool call replay?

***

#### [Page 6](highlights://highlights-ref-sync#page=6)

##### 2026-06-03:

> Comment: Compare this with SLO notes.

Some note...

***
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync linked sidecar highlights");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(
        contents
            .contains("highlights_sidecar: lib/books/highlights-ref-sync.md\n"),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 2\n"), "{contents}");
    assert!(contents.contains("### Page 2\n"), "{contents}");
    assert!(contents.contains("### Page 6\n"), "{contents}");
    assert!(
        contents.contains(
            "> [!quote] It only writes the PDF marker when frontmatter is the selected source and --write-pdf is supplied.\n"
        ),
        "{contents}"
    );
    assert!(
        contents
            .contains("> > [!note] Comment Support sase tool call replay?\n"),
        "{contents}"
    );
    assert!(
        !contents.contains("[!note] Comment - Support sase tool call replay?"),
        "linked bullet comment marker should be stripped:\n{contents}"
    );
    assert!(
        contents.contains("> [!quote] Comment: Compare this with SLO notes.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > [!note] Comment Some note...\n"),
        "{contents}"
    );
    assert!(
        !contents.contains("> [!quote] Highlights Reference Note Sync\n"),
        "linked marker mirror title should not render as a highlight:\n{contents}"
    );
    assert!(
        !contents.contains("[!note] Comment - status: wip"),
        "linked marker mirror fields should not render as a comment:\n{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 2, "{contents}");
}

#[test]
fn highlights_ref_sync_beautifies_linked_sidecar_rendering() {
    let temp = TempDir::new("bob-cli-highlights-ref-beautify");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/beautify.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/beautify.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Beautify\n",
    );
    write_file(
        &sidecar,
        "\
# Beautify

#### [Page 2](highlights://beautify#page=2)

##### 2026-06-10:

> Confusing latency and through-
put leads to mis-sized capa-
city plans with \u{fb01}les.

- Compare this with SLO notes.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync beautified linked sidecar");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(
        contents.contains(
            "> [!quote] Confusing latency and throughput leads to mis-sized capacity plans with files.\n>\n> > [!note] Comment Compare this with SLO notes.\n"
        ),
        "{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 1, "{contents}");
}

#[test]
fn highlights_ref_sync_creates_tasks_from_pdf_note_task_bullets() {
    let temp = TempDir::new("bob-cli-highlights-ref-note-tasks");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let created = Local::now().format("%Y-%m-%d").to_string();
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> Highlighted claim.

- #task Reconcile with chapter 3.
- Keep this bullet as a comment.

---

Note:
- #task Ask about the standalone note.
* #task Capture the second standalone task.
- Untagged standalone bullet.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync sidecar task bullets");

    assert_success(&output);
    let mut contents = fs::read_to_string(&note).expect("read generated note");

    // The generated PDF reading-status task line is unchanged.
    assert!(
        contents.contains(
            "- [/] #task #ref [[lib/books/task-notes.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );

    let find_created_task = |contents: &str, prose: &str| -> String {
        contents
            .lines()
            .find(|line| line.starts_with("- [ ]") && line.contains(prose))
            .unwrap_or_else(|| {
                panic!("missing created task for {prose}:\n{contents}")
            })
            .to_string()
    };
    let source_link_id = |line: &str| -> String {
        let start = line.find("[[").expect("source link present") + 2;
        let rest = &line[start..];
        let end = rest.find("]]").expect("source link terminator");
        let inside = &rest[..end];
        let target =
            inside.split_once('|').map_or(inside, |(target, _)| target);
        target
            .rsplit_once("#^")
            .unwrap_or_else(|| panic!("source link has no block id: {line}"))
            .1
            .to_string()
    };

    let reconcile_line =
        find_created_task(&contents, "#task Reconcile with chapter 3.");
    let ask_line =
        find_created_task(&contents, "#task Ask about the standalone note.");
    let capture_line = find_created_task(
        &contents,
        "#task Capture the second standalone task.",
    );

    assert!(
        contents.contains("## Tasks\n\n- [ ]"),
        "first annotation task should sit one blank line below ## Tasks:\n{contents}"
    );
    assert_annotation_tasks_in_tasks_section(
        &contents,
        &[&reconcile_line, &ask_line, &capture_line],
    );

    // Each created task carries a same-note annotation backlink, the short
    // durable processed marker, and a created date.
    for line in [&reconcile_line, &ask_line, &capture_line] {
        assert!(
            line.contains("[[#^h-"),
            "missing annotation source link: {line}"
        );
        assert!(
            !line.contains("[highlight_task:: "),
            "legacy processed marker should not be rendered: {line}"
        );
        assert!(
            line.contains("[h:: "),
            "short processed marker should be rendered: {line}"
        );
        assert!(
            line.contains(&format!("[created::{created}]")),
            "missing created date: {line}"
        );
    }

    // The link resolves to annotation-level h-... blocks. The two standalone
    // note tasks share the note block; the comment task points at the highlight
    // block.
    let block_ids = highlight_block_ids(&contents);
    let reconcile_id = source_link_id(&reconcile_line);
    let ask_id = source_link_id(&ask_line);
    let capture_id = source_link_id(&capture_line);
    assert_eq!(block_ids.len(), 2, "{contents}");
    assert!(block_ids.contains(&reconcile_id), "{contents}");
    assert!(block_ids.contains(&ask_id), "{contents}");
    assert!(block_ids.contains(&capture_id), "{contents}");
    assert_eq!(
        ask_id, capture_id,
        "standalone tasks share annotation block"
    );
    assert_ne!(reconcile_id, ask_id, "comment and note tasks differ");

    assert!(
        contents
            .contains("> > [!note] Comment #task Reconcile with chapter 3.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > Keep this bullet as a comment.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> [!note] - #task Ask about the standalone note.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> * #task Capture the second standalone task.\n"),
        "{contents}"
    );
    assert!(
        !contents.contains(" ^ht-"),
        "managed highlight blocks should not render task-specific anchors:\n{contents}"
    );
    assert!(
        contents.contains("> - Untagged standalone bullet.\n"),
        "{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync sidecar task bullets");
    assert_success(&output);
    contents = fs::read_to_string(&note).expect("read resynced note");
    assert_eq!(
        contents
            .lines()
            .filter(|line| line.starts_with("- [ ] #task Reconcile"))
            .count(),
        1,
        "{contents}"
    );
    assert_eq!(contents.matches("## Tasks").count(), 1, "{contents}");
    assert_annotation_tasks_in_tasks_section(
        &contents,
        &[
            &find_created_task(&contents, "#task Reconcile with chapter 3."),
            &find_created_task(
                &contents,
                "#task Ask about the standalone note.",
            ),
            &find_created_task(
                &contents,
                "#task Capture the second standalone task.",
            ),
        ],
    );

    // Complete the comment task and cancel a standalone task, keeping their
    // links; a later sync preserves them verbatim and never duplicates them.
    let reconcile_line =
        find_created_task(&contents, "#task Reconcile with chapter 3.");
    let ask_line =
        find_created_task(&contents, "#task Ask about the standalone note.");
    let reconcile_completed = format!(
        "{} [completion::2026-06-08]",
        reconcile_line.replacen("- [ ]", "- [x]", 1)
    );
    let ask_cancelled = format!(
        "{} [cancelled::2026-06-08]",
        ask_line.replacen("- [ ]", "- [-]", 1)
    );
    let edited = contents
        .replace(&reconcile_line, &reconcile_completed)
        .replace(&ask_line, &ask_cancelled);
    write_file(&note, &edited);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync completed sidecar task bullets");
    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains(&reconcile_completed), "{updated}");
    assert!(updated.contains(&ask_cancelled), "{updated}");
    assert_eq!(
        updated
            .lines()
            .filter(|line| line.starts_with("- [")
                && line.contains("#task Reconcile with chapter 3."))
            .count(),
        1,
        "{updated}"
    );
    assert_eq!(
        updated
            .lines()
            .filter(|line| line.starts_with("- [")
                && line.contains("#task Ask about the standalone note."))
            .count(),
        1,
        "{updated}"
    );
    assert_eq!(updated.matches("## Tasks").count(), 1, "{updated}");
    let tasks_heading = updated.find("## Tasks\n").expect("## Tasks");
    let highlights = updated.find("## Highlights\n").expect("## Highlights");
    for line in [&reconcile_completed, &ask_cancelled] {
        let pos = updated.find(line).unwrap_or_else(|| {
            panic!("missing preserved task {line}:\n{updated}")
        });
        assert!(
            pos > tasks_heading && pos < highlights,
            "preserved task should remain under ## Tasks: {line}\n{updated}"
        );
    }
}

#[test]
fn highlights_ref_sync_skips_legacy_highlight_task_property() {
    let temp = TempDir::new("bob-cli-highlights-ref-legacy-task-property");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/legacy-task.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/legacy-task.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Legacy Task\n",
    );
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note mirrored from the PDF

---

> Highlighted claim.

- #task Legacy follow-up
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("initial sync sidecar legacy task");
    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    let source_block_id = highlight_block_ids(&contents)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing source block id:\n{contents}"));
    let legacy_id = legacy_highlight_task_id(
        "ref/books/legacy-task.md",
        &source_block_id,
        "#task Legacy follow-up",
    );
    let created_line = contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Legacy follow-up"))
        .unwrap_or_else(|| panic!("missing created task:\n{contents}"));
    let legacy_line = format!(
        "- [x] #task Edited legacy follow-up [[ref/books/legacy-task#^{source_block_id}|🔖]] [highlight_task:: {legacy_id}] [created::2026-06-01] [completion::2026-06-02]"
    );
    write_file(&note, &contents.replace(created_line, &legacy_line));

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync legacy task property");
    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains(&legacy_line), "{updated}");
    assert_eq!(
        updated
            .lines()
            .filter(|line| {
                line.starts_with("- [")
                    && line.contains("#task Legacy follow-up")
            })
            .count(),
        0,
        "{updated}"
    );
    assert_eq!(
        updated
            .lines()
            .filter(|line| {
                line.starts_with("- [")
                    && line.contains("#task Edited legacy follow-up")
            })
            .count(),
        1,
        "{updated}"
    );
}

#[test]
fn highlights_ref_sync_routes_annotation_tasks_to_existing_root_note() {
    let temp = TempDir::new("bob-cli-highlights-ref-routed-task");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let route_note = vault.join("alice.md");
    let done_note = vault.join("done/alice_done.md");
    let created = Local::now().format("%Y-%m-%d").to_string();
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> Routed claim.

- #task Follow up with Alice @alice
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync routed sidecar task");

    assert_success(&output);
    let ref_contents = fs::read_to_string(&note).expect("read ref note");
    assert!(
        !ref_contents
            .lines()
            .any(|line| line.starts_with("- [ ] #task Follow up with Alice")),
        "routed task should not be inserted into the reference note:\n{ref_contents}"
    );
    assert!(
        ref_contents.contains("> > [!note] Comment #task Follow up with Alice @alice\n")
            && !ref_contents.contains(" ^ht-"),
        "managed source should render without task-specific anchors:\n{ref_contents}"
    );
    let mut route_contents =
        fs::read_to_string(&route_note).expect("read routed note");
    let routed_line = route_contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Follow up with Alice"))
        .unwrap_or_else(|| panic!("missing routed task:\n{route_contents}"))
        .to_string();
    assert!(!routed_line.contains("@alice"), "{routed_line}");
    assert!(
        routed_line.contains("[[ref/books/task-notes#^h-") && routed_line.contains("|🔖]]"),
        "routed task should link back to the annotation ref note block:\n{routed_line}"
    );
    assert!(
        !routed_line.contains("[highlight_task:: ")
            && routed_line.contains("[h:: ")
            && routed_line.contains(&format!("[created::{created}]")),
        "routed task should carry h marker and created date:\n{routed_line}"
    );

    let completed_line = format!(
        "{} [completion::2026-06-08]",
        routed_line.replacen("- [ ]", "- [x]", 1)
    );
    write_file(
        &route_note,
        &route_contents.replace(&routed_line, &completed_line),
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("resync completed routed task"),
    );
    route_contents =
        fs::read_to_string(&route_note).expect("read rerouted note");
    assert_eq!(
        route_contents
            .lines()
            .filter(|line| line.contains("#task Follow up with Alice"))
            .count(),
        1,
        "{route_contents}"
    );

    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    let edited_archived_line = completed_line
        .replace("#task Follow up with Alice", "#task Followed up with Alice");
    write_file(&done_note, &format!("{edited_archived_line}\n"));
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("resync archived routed task"),
    );
    route_contents = fs::read_to_string(&route_note).expect("read route note");
    assert!(
        !route_contents.contains("#task Follow up with Alice"),
        "archived routed task with edited prose should not be recreated:\n{route_contents}"
    );
}

#[test]
fn highlights_ref_sync_skips_annotation_tasks_for_non_wip_statuses() {
    for status in ["ready", "next", "read", "abandoned", "legacy"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-non-wip-task-{status}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/books/task-notes.pdf");
        let sidecar = pdf.with_extension("md");
        let note = vault.join("ref/books/task-notes.md");
        write_highlights_pdf(
            &pdf,
            &format!(
                "- status: {status}\n- parent: obsidian\n- title: Task Notes\n"
            ),
        );
        write_file(
            &sidecar,
            "\
## Page 4

Note: marker note mirrored from the PDF

---

> Claim.

- #task Should not be created.
",
        );

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| panic!("sync {status}: {error}"));

        assert_success(&output);
        let contents = fs::read_to_string(&note)
            .unwrap_or_else(|error| panic!("read note for {status}: {error}"));
        assert!(
            !contents
                .lines()
                .any(|line| line
                    .starts_with("- [ ] #task Should not be created.")),
            "{status} PDFs should not create annotation tasks:\n{contents}"
        );
    }
}

#[test]
fn highlights_ref_sync_skips_vault_scan_when_no_annotation_candidates() {
    // Pins the "skip the vault-wide processed-task scan when no plan carries
    // annotation-task candidates" optimization. The sidecar has no `#task`
    // bullets, so there are zero candidates and the processed-task index is
    // never needed. We drop an unreadable (invalid UTF-8) `.md` file into the
    // vault: if a future refactor reintroduced the unconditional scan, the walk
    // would read this file and abort the command, failing this test.
    let temp = TempDir::new("bob-cli-highlights-ref-no-candidate-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let unreadable = vault.join("unreadable.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> A claim with no task bullet.
",
    );
    // Invalid UTF-8 bytes make `fs::read_to_string` fail if this file is ever
    // walked by the processed-task index builder.
    fs::write(&unreadable, [0xff, 0xfe, 0x00, 0x9f])
        .expect("write invalid utf-8 sibling note");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync wip pdf without annotation tasks");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read ref note");
    assert!(
        !contents.lines().any(|line| line.contains("#task A claim")),
        "no annotation task should be created:\n{contents}"
    );
}

#[test]
fn highlights_ref_sync_missing_routed_target_fails_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-route");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note mirrored from the PDF

---

> Routed claim.

- #task Follow up with Alice @alice
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync missing routed task target");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing routed target should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output)
            .contains("routed annotation task target does not exist")
            && stderr(&output).contains("alice.md")
            && stderr(&output).contains("create a root-level note"),
        "expected missing route target error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "failed routed planning should not write the reference note"
    );
}

#[test]
fn highlights_ref_sync_preserves_manual_sections_and_rejects_missing_markers() {
    let temp = TempDir::new("bob-cli-highlights-ref-manual-body");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 2

Note: marker note

---

> Initial quote.
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial manual-body sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read note")
        .replacen("---\n", "---\nowner: Bryan\n", 1)
        .replace(
            "## Highlights\n\n",
            "## Manual Notes\n\nManual synthesis.\n\n## Highlights\n\n",
        );
    write_file(&note, &edited);
    write_file(
        &sidecar,
        "\
## Page 2

Note: marker note

---

> Initial quote.

Comment: added later
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync with manual content");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains("owner: Bryan\n"), "{updated}");
    assert!(updated.contains("Manual synthesis.\n"), "{updated}");
    assert!(updated.contains("[!note] Comment added later"), "{updated}");

    let unsafe_pdf = vault.join("lib/unsafe.pdf");
    let unsafe_note = vault.join("ref/unsafe.md");
    write_highlights_pdf(&unsafe_pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &unsafe_note,
        "\
---
status: wip
parent: \"[[obsidian]]\"
---

Manual note without generated markers.
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&unsafe_pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync unsafe existing note");

    assert_eq!(
        output.status.code(),
        Some(1),
        "existing note without markers should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing the managed Highlights region"),
        "expected managed-region error:\n{}",
        format_output(&output)
    );
}

fn legacy_highlight_task_id(
    ref_note_path: &str,
    source_block_id: &str,
    identity: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update("v1");
    hasher.update([0]);
    hasher.update(ref_note_path);
    hasher.update([0]);
    hasher.update(source_block_id);
    hasher.update([0]);
    hasher.update(identity);
    hex::encode(hasher.finalize())
}
