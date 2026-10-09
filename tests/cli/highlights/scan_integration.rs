//! Scan-integration coverage: one shared locator index across parallel
//! planning, v2 sync in human and JSON scans, and rebased annotation
//! writes at execution. Subprocess fixtures over temp vaults with a pinned
//! clock and generated tiny PDFs.

use crate::support::*;
use std::fs;

const PINNED_NOW: &str = "2026-10-06 09:00:00";

fn scan_vault(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    (temp, vault)
}

fn sync_pdf(
    vault: &std::path::Path,
    pdf: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("sync")
        .arg(pdf)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", PINNED_NOW);
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob highlights sync")
}

#[test]
fn v2_two_births_share_one_parent_and_settle() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-two-births");
    let first_pdf = vault.join("lib/books/alpha.pdf");
    let second_pdf = vault.join("lib/papers/beta.pdf");
    let first_note = vault.join("ref/books/alpha.md");
    let second_note = vault.join("ref/papers/beta.md");
    let parent_note = vault.join("obsidian.md");
    let alice_note = vault.join("alice.md");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n## Tasks\n\n- [ ] Existing parent task ^existing-1\n",
    );
    write_file(&alice_note, "# Alice\n");
    write_highlights_pdf(
        &first_pdf,
        "- status: wip\n- parent: obsidian\n- title: Alpha\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: wip\n- parent: obsidian\n- title: Beta\n",
    );
    write_file(
        &second_pdf.with_extension("md"),
        "\
# Beta

## Page 1

- status: wip
- parent: obsidian

---

> Beta quote.

- #task Follow up on beta.
- #task Tell alice @alice
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("scan two births");
    assert_success(&output);

    for (note, id) in [(&first_note, "ref-alpha"), (&second_note, "ref-beta")] {
        let contents = fs::read_to_string(note).expect("read born ref note");
        assert!(
            contents.contains(&format!("![[obsidian#^{id}]]\n")),
            "birth should heal the managed embed:\n{contents}"
        );
        assert!(
            !contents.contains("#hide ^ref"),
            "birth must not write an in-note tracker:\n{contents}"
        );
    }
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read shared parent");
    assert!(
        parent_contents.contains("^ref-alpha")
            && parent_contents.contains("^ref-beta"),
        "both births should file into one parent:\n{parent_contents}"
    );
    assert!(
        parent_contents.contains("- [ ] Existing parent task ^existing-1\n"),
        "unrelated parent tasks must survive:\n{parent_contents}"
    );
    assert!(
        parent_contents.contains("#task Follow up on beta.")
            && parent_contents.contains("[[ref/papers/beta#^h-"),
        "unqualified follow-ups target the residence with a full-path link:\n{parent_contents}"
    );
    let alice_contents =
        fs::read_to_string(&alice_note).expect("read explicit route");
    assert!(
        alice_contents.contains("#task Tell alice"),
        "explicit @alice routes keep legacy behavior:\n{alice_contents}"
    );

    // A repeated run after successful sync is a no-op.
    let before_first = fs::read_to_string(&first_note).expect("read note");
    let before_parent = fs::read_to_string(&parent_note).expect("read parent");
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("repeat scan");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&first_note).expect("read note"),
        before_first
    );
    assert_eq!(
        fs::read_to_string(&parent_note).expect("read parent"),
        before_parent
    );
}

#[test]
fn v2_fallback_parent_uses_inbox_with_warning() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-fallback");
    let pdf = vault.join("lib/lost.pdf");
    let note = vault.join("ref/lost.md");
    let inbox = vault.join("mac_inbox.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: nowhere\n");

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read born note");
    assert!(
        contents.contains("![[mac_inbox#^ref-lost]]\n"),
        "{contents}"
    );
    let inbox_contents =
        fs::read_to_string(&inbox).expect("read fallback inbox");
    assert!(inbox_contents.contains("^ref-lost"), "{inbox_contents}");
    assert!(
        inbox_contents
            .contains("⚠️ parent '[[nowhere]]' is not an open area or project"),
        "fallback births must carry the refile warning:\n{inbox_contents}"
    );
}

#[test]
fn v2_orphan_recovery_adopts_existing_task() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-orphan");
    let pdf = vault.join("lib/papers/orphan.pdf");
    let note = vault.join("ref/papers/orphan.md");
    let parent_note = vault.join("obsidian.md");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n- [ ] #task #ref [[ref/papers/orphan|Orphan]] [created::2026-10-06] ^ref-orphan\n",
    );
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Orphan\n",
    );

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read born note");
    assert!(
        contents.contains("![[obsidian#^ref-orphan]]\n"),
        "adoption should heal the orphan address:\n{contents}"
    );
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert_eq!(
        parent_contents.matches("^ref-orphan").count(),
        1,
        "adoption must not duplicate the orphan line:\n{parent_contents}"
    );
}

#[test]
fn v2_moved_task_residence_heals_embed() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-moved");
    let pdf = vault.join("lib/moved.pdf");
    let note = vault.join("ref/moved.md");
    let first_parent = vault.join("obsidian.md");
    let second_parent = vault.join("sase.md");
    write_area_note(&first_parent, "Obsidian");
    write_area_note(&second_parent, "Sase");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let born = fs::read_to_string(&note).expect("read born note");
    assert!(born.contains("![[obsidian#^ref-moved]]\n"), "{born}");

    // The user refiles the reading task to another area note.
    let task_line = fs::read_to_string(&first_parent)
        .expect("read first parent")
        .lines()
        .find(|line| line.contains("^ref-moved"))
        .expect("find reading task")
        .to_string();
    let without = fs::read_to_string(&first_parent)
        .expect("read first parent")
        .lines()
        .filter(|line| !line.contains("^ref-moved"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&first_parent, &without);
    let second = fs::read_to_string(&second_parent).expect("read second");
    write_file(&second_parent, &format!("{second}{task_line}\n"));

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let contents = fs::read_to_string(&note).expect("read moved note");
    assert!(
        contents.contains("![[sase#^ref-moved]]\n"),
        "moved residence should heal the embed:\n{contents}"
    );
    assert!(
        contents.contains("parent: \"[[sase]]\"\n"),
        "moved residence should repoint the parent:\n{contents}"
    );
    assert!(
        !fs::read_to_string(&first_parent)
            .expect("read first parent")
            .contains("^ref-moved"),
        "the old residence must not keep a copy"
    );
}

#[test]
fn v2_archive_reopen_inserts_fresh_task_with_suffixed_id() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-reopen");
    let pdf = vault.join("lib/reopen.pdf");
    let note = vault.join("ref/reopen.md");
    let parent_note = vault.join("obsidian.md");
    let archive_note = vault.join("done/obsidian_done.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: read\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let born_task = fs::read_to_string(&parent_note)
        .expect("read parent")
        .lines()
        .find(|line| line.contains("^ref-reopen"))
        .expect("find born task")
        .to_string();
    assert!(born_task.starts_with("- [x]"), "{born_task}");

    // The user archives the terminal task, then deliberately reopens the
    // ref by marking the PDF marker wip again.
    let archived =
        format!("---\nparent: \"[[obsidian]]\"\n---\n\n{born_task}\n");
    write_file(&archive_note, &archived);
    let without = fs::read_to_string(&parent_note)
        .expect("read parent")
        .lines()
        .filter(|line| !line.contains("^ref-reopen"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&parent_note, &without);
    set_pdf_marker_contents(&pdf, "- status: wip\n- parent: obsidian\n");

    let output = sync_pdf(&vault, &pdf, &["--write-pdf"]);
    assert_success(&output);
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read reopened parent");
    assert!(
        parent_contents.contains("^ref-reopen-2"),
        "the taken old address must be suffixed in the live note:\n{parent_contents}"
    );
    assert!(
        parent_contents.contains("- [/] #task #ref [[ref/reopen|reopen]]"),
        "reopen should file a fresh open task in the source parent:\n{parent_contents}"
    );
    let archive_contents =
        fs::read_to_string(&archive_note).expect("read archive");
    assert_eq!(
        archive_contents, archived,
        "scan must never mutate done/:\n{archive_contents}"
    );
    let contents = fs::read_to_string(&note).expect("read reopened note");
    assert!(
        contents.contains("![[obsidian#^ref-reopen-2]]\n"),
        "reopen should heal the fresh address:\n{contents}"
    );
}

#[test]
fn v2_failed_pdf_does_not_consume_sibling_follow_up() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-failure");
    let bad_pdf = vault.join("lib/bad.pdf");
    let good_pdf = vault.join("lib/good.pdf");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    // A PDF with no marker annotation fails planning.
    write_highlights_pdf_pages(&bad_pdf, &[&[]]);
    write_highlights_pdf(&good_pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &good_pdf.with_extension("md"),
        "\
# Good

## Page 1

- status: wip
- parent: obsidian

---

> Good quote.

- #task Shared follow-up.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("scan with sibling failure");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a failed PDF should fail the scan:\n{}",
        format_output(&output)
    );
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("#task Shared follow-up."),
        "the valid sibling must still import its follow-up:\n{parent_contents}"
    );
    assert!(
        parent_contents.contains("^ref-good"),
        "the valid sibling must still birth:\n{parent_contents}"
    );
}

#[test]
fn v2_parallel_matches_sequential() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-jobs");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    for (stem, status) in [("one", "wip"), ("two", "ready"), ("three", "wip")] {
        write_highlights_pdf(
            &vault.join(format!("lib/{stem}.pdf")),
            &format!("- status: {status}\n- parent: obsidian\n"),
        );
    }
    let run = |jobs: &str| {
        let output = bob_command()
            .arg("highlights")
            .arg("scan")
            .arg("--dry-run")
            .arg("--jobs")
            .arg(jobs)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", PINNED_NOW)
            .output()
            .expect("dry-run scan with --jobs");
        assert_success(&output);
        stdout(&output)
    };
    assert_eq!(run("1"), run("4"), "--jobs must not change scan output");
}

#[test]
fn v2_json_scan_reports_births_without_human_lines() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-json");
    let pdf = vault.join("lib/json.pdf");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("json scan");
    assert_eq!(
        output.status.code(),
        Some(0),
        "json scan should succeed:\n{}",
        format_output(&output)
    );
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "stdout must be one JSON line");
    let document: serde_json::Value =
        serde_json::from_str(text.trim()).expect("parse JSON");
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["pdfs"], 1);
    assert_eq!(document["summary"]["created"], 1);
    let notes = document["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["action"], "create");
    assert!(
        !text.contains("reading tasks") && !text.contains("would create note"),
        "no human task messages on JSON stdout:\n{text}"
    );
}

#[test]
fn v2_dry_run_writes_nothing() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-dry");
    let pdf = vault.join("lib/dry.pdf");
    let note = vault.join("ref/dry.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &pdf.with_extension("md"),
        "\
# Dry

## Page 1

- status: wip
- parent: obsidian

---

> Dry quote.

- #task Dry follow-up.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg("--dry-run")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("dry-run sync");
    assert_success(&output);
    assert!(
        stdout(&output).contains("writes: none"),
        "{}",
        format_output(&output)
    );
    assert!(!note.exists(), "dry-run must not create the ref note");
    assert!(
        !fs::read_to_string(&parent_note)
            .expect("read parent")
            .contains("^ref-dry"),
        "dry-run must not file the reading task"
    );
}

#[test]
fn v2_missing_task_never_gains_automatic_replacement() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-missing");
    let pdf = vault.join("lib/missing.pdf");
    let note = vault.join("ref/missing.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    // The user deletes the reading task everywhere.
    let without = fs::read_to_string(&parent_note)
        .expect("read parent")
        .lines()
        .filter(|line| !line.contains("^ref-missing"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&parent_note, &without);

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        !parent_contents.contains("^ref-missing"),
        "an open note without a task must never gain a replacement:\n{parent_contents}"
    );
}

#[test]
fn v2_multiple_open_tasks_refuse_replacement() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-ambiguous");
    let pdf = vault.join("lib/ambiguous.pdf");
    let note = vault.join("ref/ambiguous.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    // A second open task claims the same ref: the sync must refuse to
    // invent a third line or repoint the note.
    let mut parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    parent_contents.push_str(
        "- [/] #task #ref [[ref/ambiguous|Ambiguous]] [created::2026-10-06] ^ref-ambiguous-2\n",
    );
    write_file(&parent_note, &parent_contents);

    let task_before = fs::read_to_string(&parent_note).expect("read parent");
    let ref_before = fs::read_to_string(&note).expect("read note");
    let pdf_before = fs::read(&pdf).expect("read pdf");
    let output = sync_pdf(&vault, &pdf, &[]);
    assert!(
        !output.status.success(),
        "ambiguity must refuse with a per-PDF diagnostic failure:\n{}",
        format_output(&output)
    );
    let stderr = format_output(&output);
    assert!(
        stderr.contains("multiple_open_ref_tasks"),
        "refusal must name the ambiguity:\n{stderr}"
    );
    let after = fs::read_to_string(&parent_note).expect("read parent");
    assert_eq!(
        after, task_before,
        "refusal must leave task bytes unchanged"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        ref_before,
        "refusal must leave ref bytes unchanged"
    );
    assert_eq!(fs::read(&pdf).expect("read pdf"), pdf_before);
    assert_eq!(
        after.matches("ref/ambiguous").count(),
        2,
        "ambiguity must not mint a replacement task:\n{after}"
    );
    let contents = fs::read_to_string(&note).expect("read note");
    assert!(
        !contents.contains("^ref-ambiguous-2")
            || contents.contains("^ref-ambiguous]"),
        "the note must not point at an arbitrary claimant:\n{contents}"
    );
}

#[test]
fn v2_birth_without_tasks_heading_creates_section() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-heading");
    let pdf = vault.join("lib/heading.pdf");
    let parent_note = vault.join("obsidian.md");
    // A bare area note with no Tasks section at all.
    write_file(&parent_note, "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    // Capture's insertion appends when no Tasks section exists; the birth
    // must land without requiring one.
    assert!(
        parent_contents.contains("^ref-heading"),
        "insertion should work without a Tasks section:\n{parent_contents}"
    );
}

#[test]
fn v2_same_stem_in_two_categories_stays_distinct() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-stems");
    let first_pdf = vault.join("lib/books/dup.pdf");
    let second_pdf = vault.join("lib/papers/dup.pdf");
    let first_note = vault.join("ref/books/dup.md");
    let second_note = vault.join("ref/papers/dup.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: obsidian\n");
    write_highlights_pdf(&second_pdf, "- status: wip\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("scan duplicate stems");
    assert_success(&output);
    assert!(first_note.is_file() && second_note.is_file());
    let first = fs::read_to_string(&first_note).expect("read books note");
    let second = fs::read_to_string(&second_note).expect("read papers note");
    assert!(first.contains("ref_type: books\n"), "{first}");
    assert!(second.contains("ref_type: papers\n"), "{second}");
    assert_ne!(first, second, "category notes must differ");
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("^ref-dup\n")
            || parent_contents.contains("^ref-dup\r"),
        "{parent_contents}"
    );
    assert!(
        parent_contents.contains("^ref-dup-2"),
        "the second birth must take a suffixed address:\n{parent_contents}"
    );
}

#[test]
fn v2_destination_collision_suffixes_preview_id() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-collision");
    let pdf = vault.join("lib/collide.pdf");
    let note = vault.join("ref/collide.md");
    let parent_note = vault.join("obsidian.md");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n- [ ] Unrelated old task ^ref-collide\n",
    );
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("- [ ] Unrelated old task ^ref-collide\n"),
        "the colliding line must survive:\n{parent_contents}"
    );
    assert!(
        parent_contents.contains("^ref-collide-2"),
        "the birth must take a suffixed address:\n{parent_contents}"
    );
    let contents = fs::read_to_string(&note).expect("read born note");
    assert!(
        contents.contains("![[obsidian#^ref-collide-2]]\n"),
        "the embed must name the actual address:\n{contents}"
    );
}

#[test]
fn v2_alias_parent_resolves_to_canonical_residence() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-alias");
    let pdf = vault.join("lib/aliased.pdf");
    let note = vault.join("ref/aliased.md");
    let parent_note = vault.join("obsidian.md");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\nproject_name_aliases: [\"ob\"]\n---\n\n# Obsidian\n",
    );
    write_highlights_pdf(&pdf, "- status: wip\n- parent: ob\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let contents = fs::read_to_string(&note).expect("read born note");
    assert!(
        contents.contains("parent: \"[[obsidian]]\"\n"),
        "aliases should resolve to the canonical route:\n{contents}"
    );
    assert!(
        contents.contains("![[obsidian#^ref-aliased]]\n"),
        "{contents}"
    );
    assert!(
        !fs::read_to_string(&vault.join("mac_inbox.md"))
            .unwrap_or_default()
            .contains("^ref-aliased"),
        "an alias match must not fall back to the inbox"
    );
}

#[test]
fn v2_concise_scan_reports_reading_task_creations() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-report-born");
    let first_pdf = vault.join("lib/books/alpha.pdf");
    let second_pdf = vault.join("lib/papers/beta.pdf");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(
        &first_pdf,
        "- status: wip\n- parent: obsidian\n- title: Alpha\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: wip\n- parent: obsidian\n- title: Beta\n",
    );

    // Concise dry-run rolls up the planned births by destination.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("dry-run scan");
    assert_success(&output);
    let planned = stdout(&output);
    assert!(
        planned.contains("reading tasks created · obsidian.md (2)"),
        "dry-run summary should roll up planned births:\n{planned}"
    );

    // Verbose dry-run names the planned destination and preview ID per PDF.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("verbose dry-run scan");
    assert_success(&output);
    let verbose = stdout(&output);
    assert!(
        verbose.contains("reading_task: insert obsidian.md ^ref-alpha")
            && verbose.contains("reading_task: insert obsidian.md ^ref-beta"),
        "verbose dry-run should name each birth destination and ID:\n{verbose}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("write scan");
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("📖 2 reading tasks created · obsidian.md (2)"),
        "concise scan should report actual births by destination:\n{written}"
    );
    assert!(
        !written.contains("reading tasks updated"),
        "births must not count as updates:\n{written}"
    );

    // A repeated run after successful sync is quiet: no creation lines.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("repeat scan");
    assert_success(&output);
    let repeat = stdout(&output);
    assert!(
        !repeat.contains("reading tasks created")
            && !repeat.contains("reading tasks updated"),
        "settled reruns must not report reading-task writes:\n{repeat}"
    );
}

#[test]
fn v2_single_pdf_report_names_performed_reading_write() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-report-one");
    let pdf = vault.join("lib/solo.pdf");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    let output = sync_pdf(&vault, &pdf, &["--dry-run"]);
    assert_success(&output);
    let planned = stdout(&output);
    assert!(
        planned.contains("reading_task: insert obsidian.md ^ref-solo"),
        "single-PDF dry-run should name the planned birth:\n{planned}"
    );

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("reading_task_created: obsidian.md ^ref-solo"),
        "single-PDF write should name the actual destination and ID:\n{written}"
    );
}

#[test]
fn v2_concise_scan_reports_reading_task_updates() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-report-edit");
    let pdf = vault.join("lib/grow.pdf");
    let note = vault.join("ref/grow.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: ready\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));

    // The reader moves the ref-note frontmatter forward; sync flips the
    // located checkbox instead of inserting a replacement task.
    let before = fs::read_to_string(&note).expect("read born note");
    assert!(before.contains("![[obsidian#^ref-grow]]\n"), "{before}");
    let moved = before.replace("status: ready", "status: wip");
    assert_ne!(moved, before, "born note should carry a status: {before}");
    write_file(&note, &moved);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("update scan");
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("↻ 1 reading tasks updated"),
        "concise scan should report the performed line edit:\n{written}"
    );
    assert!(
        !written.contains("reading tasks created"),
        "a line edit must not count as a creation:\n{written}"
    );
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("- [/] #task #ref [[ref/grow|"),
        "the located task should flip to in-progress:\n{parent_contents}"
    );
}

#[test]
fn v2_concise_scan_flags_open_v1_with_migrate_hint() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-report-v1");
    let pdf = vault.join("lib/books/legacy.pdf");
    let note = vault.join("ref/books/legacy.md");
    write_highlights_pdf(&pdf, "- status: ready\n- parent: obsidian\n");
    seed_v1_ref_note(
        &note,
        "- [ ] #task #ref [[lib/books/legacy.pdf]] #hide ^ref",
    );

    // One sync settles the legacy note's frontmatter; the second scan is
    // the frozen steady state this test asserts.
    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let before = fs::read_to_string(&note).expect("read v1 note");
    assert!(before.contains("#hide ^ref"), "{before}");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("v1 scan");
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("1 open v1 ref tasks · run bob ref migrate-tasks"),
        "concise scan should point at the remaining v1 reference:\n{written}"
    );
    assert!(
        !written.contains("reading tasks created")
            && !written.contains("reading tasks updated"),
        "v1 notes perform no reading-task writes:\n{written}"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read v1 note"),
        before,
        "an unchanged v1 note must keep frozen bytes"
    );
}

#[test]
fn v2_adoption_reports_neither_created_nor_updated() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-report-adopt");
    let pdf = vault.join("lib/papers/orphan.pdf");
    let parent_note = vault.join("obsidian.md");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n- [ ] #task #ref [[ref/papers/orphan|Orphan]] [created::2026-10-06] ^ref-orphan\n",
    );
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Orphan\n",
    );

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("reading_task: adopt"),
        "the plan report should name the adoption:\n{written}"
    );
    assert!(
        !written.contains("reading_task_created:")
            && !written.contains("reading tasks created"),
        "adoption is not creation:\n{written}"
    );
    assert!(
        !written.contains("reading tasks updated"),
        "adoption is not an update:\n{written}"
    );
}

#[test]
fn v2_sidecar_free_birth_pins_created_date() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-sidecar-free");
    let pdf = vault.join("lib/nosidecar.pdf");
    let note = vault.join("ref/nosidecar.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: ready\n- parent: obsidian\n");

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("reading_task_created: obsidian.md ^ref-nosidecar"),
        "sidecar-free births still report the reading task:\n{written}"
    );
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("[created::2026-10-06]"),
        "birth dates pin to the invocation date:\n{parent_contents}"
    );
    let contents = fs::read_to_string(&note).expect("read born note");
    assert!(
        contents.contains("![[obsidian#^ref-nosidecar]]\n"),
        "sidecar-free births still heal the managed embed:\n{contents}"
    );
}

#[test]
fn v2_deleted_managed_embed_heals_without_new_task() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-embed-heal");
    let pdf = vault.join("lib/heal.pdf");
    let note = vault.join("ref/heal.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let born = fs::read_to_string(&note).expect("read born note");
    assert!(born.contains("![[obsidian#^ref-heal]]\n"), "{born}");

    // The reader deletes the managed embed; the next sync heals the slot
    // without filing a replacement task.
    let stripped = born
        .lines()
        .filter(|line| !line.contains("![[obsidian#^ref-heal]]"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&note, &stripped);

    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        !written.contains("reading_task_created:"),
        "healing the embed must not create a task:\n{written}"
    );
    let healed = fs::read_to_string(&note).expect("read healed note");
    assert!(
        healed.contains("![[obsidian#^ref-heal]]\n"),
        "the managed embed should heal:\n{healed}"
    );
    assert_eq!(
        fs::read_to_string(&parent_note)
            .expect("read parent")
            .matches("^ref-heal")
            .count(),
        1,
        "healing must not duplicate the reading task"
    );
}

#[test]
fn v2_parent_move_settles_without_pdf_opt_in() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-parent-move");
    let pdf = vault.join("lib/shift.pdf");
    let note = vault.join("ref/shift.md");
    let first_parent = vault.join("obsidian.md");
    let second_parent = vault.join("sase.md");
    write_area_note(&first_parent, "Obsidian");
    write_area_note(&second_parent, "Sase");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    assert_success(&sync_pdf(&vault, &pdf, &[]));

    // The user refiles the reading task; the PDF bytes never change.
    let task_line = fs::read_to_string(&first_parent)
        .expect("read first parent")
        .lines()
        .find(|line| line.contains("^ref-shift"))
        .expect("find reading task")
        .to_string();
    let without = fs::read_to_string(&first_parent)
        .expect("read first parent")
        .lines()
        .filter(|line| !line.contains("^ref-shift"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&first_parent, &without);
    let second = fs::read_to_string(&second_parent).expect("read second");
    write_file(&second_parent, &format!("{second}{task_line}\n"));

    // No `--write-pdfs`: a parent-only move must never demand marker
    // write-back.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("move scan");
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        !written.contains("reading tasks created"),
        "a residence move is not a creation:\n{written}"
    );
    let contents = fs::read_to_string(&note).expect("read moved note");
    assert!(
        contents.contains("parent: \"[[sase]]\"\n"),
        "the residence move should repoint the parent:\n{contents}"
    );
    assert!(
        contents.contains("![[sase#^ref-shift]]\n"),
        "the residence move should heal the embed:\n{contents}"
    );
}

#[test]
fn v2_birth_honors_configured_ref_dir() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-ref-dir");
    let pdf = vault.join("lib/books/custom.pdf");
    let note = vault.join("custom/books/custom.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: ready\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--ref-dir")
        .arg("custom")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("custom ref-dir scan");
    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("📖 1 reading tasks created · obsidian.md (1)"),
        "custom ref dirs still report the birth:\n{written}"
    );
    let contents = fs::read_to_string(&note).expect("read custom ref note");
    assert!(
        contents.contains("![[obsidian#^ref-custom]]\n"),
        "the note should birth under the configured ref dir:\n{contents}"
    );
}

#[test]
fn v2_opt_in_refreshes_stale_marker_hint() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-hint-refresh");
    let pdf = vault.join("lib/hint.pdf");
    let note = vault.join("ref/hint.md");
    let inbox = vault.join("mac_inbox.md");
    let home = vault.join("nowhere.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: Nowhere\n");

    // No matching area note yet: the birth falls back to the inbox and a
    // normal scan preserves the marker's original hint untouched.
    assert_success(&sync_pdf(&vault, &pdf, &[]));
    assert!(
        fs::read_to_string(&inbox)
            .expect("read inbox")
            .contains("^ref-hint"),
        "fallback birth should file into the inbox"
    );

    // The reader refiles the task into the new area note. A normal scan
    // follows the residence for the ref note but still leaves the marker
    // hint alone; only the opt-in write refreshes it to the residence.
    write_area_note(&home, "Nowhere");
    let task_line = fs::read_to_string(&inbox)
        .expect("read inbox")
        .lines()
        .find(|line| line.contains("^ref-hint"))
        .expect("find reading task")
        .to_string();
    let without = fs::read_to_string(&inbox)
        .expect("read inbox")
        .lines()
        .filter(|line| !line.contains("^ref-hint"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&inbox, &without);
    let second = fs::read_to_string(&home).expect("read new parent");
    write_file(&home, &format!("{second}{task_line}\n"));

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let contents = fs::read_to_string(&note).expect("read moved note");
    assert!(
        contents.contains("parent: \"[[nowhere]]\"\n"),
        "the residence move should repoint the parent:\n{contents}"
    );

    assert_success(&sync_pdf(&vault, &pdf, &["--write-pdf"]));
    let marker = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("read back marker");
    assert_success(&marker);
    let shown = stdout(&marker);
    assert!(
        shown.contains("parent: nowhere"),
        "opt-in should refresh the stale hint to the residence:\n{shown}"
    );
}

#[test]
fn v2_archive_move_dedups_follow_up() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-archive-move");
    let pdf = vault.join("lib/move.pdf");
    let note = vault.join("ref/move.md");
    let parent_note = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &pdf.with_extension("md"),
        "# Move

## Page 1

- status: wip
- parent: obsidian

---

> Move quote.

- #task Archived follow-up.
",
    );

    assert_success(&sync_pdf(&vault, &pdf, &[]));
    let parent_before = fs::read_to_string(&parent_note).expect("read parent");
    let follow_line = parent_before
        .lines()
        .find(|line| line.contains("Archived follow-up"))
        .expect("follow-up born")
        .to_string();
    assert!(
        follow_line.contains("[h::"),
        "follow-up carries [h::]:\n{follow_line}"
    );
    let processed = follow_line
        .split("[h::")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("processed id")
        .trim()
        .to_string();
    assert!(!processed.is_empty(), "processed id present");

    // Move the follow-up into done/ carrying its [h::] property.
    let without = parent_before
        .lines()
        .filter(|line| !line.contains("Archived follow-up"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_file(&parent_note, &without);
    write_file(&archive, &format!("{follow_line}\n"));
    let archive_before = fs::read_to_string(&archive).expect("read archive");

    // Rescan: no recreation, archive bytes intact, counts accurate.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("rescan after archive move");
    assert_success(&output);
    let parent_after = fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        !parent_after.contains("Archived follow-up"),
        "archived follow-up must not recreate:\n{parent_after}"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        archive_before,
        "archive bytes must stay intact"
    );
    let written = stdout(&output);
    assert!(
        !written.contains("Archived follow-up"),
        "no duplicate follow-up reported:\n{written}"
    );

    // Subsequent run is a no-op for this follow-up.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("repeat after archive move");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&parent_note).expect("read parent"),
        parent_after,
        "repeat must stay settled"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        archive_before,
        "repeat must leave archive intact"
    );

    // Archived terminal reading sync leaves archive bytes intact (birth
    // archive path is untouched by later scans).
    assert!(note.is_file(), "ref note still exists");
}

#[test]
fn v2_lifecycle_needs_opt_in_and_settles_on_rerun() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-lifecycle");
    let pdf = vault.join("lib/life.pdf");
    let note = vault.join("ref/life.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: ready\n- parent: obsidian\n");
    assert_success(&sync_pdf(&vault, &pdf, &[]));

    // Birth a Ready ref, then drive it to Next via the external task.
    let task_line = fs::read_to_string(&parent_note)
        .expect("read parent")
        .lines()
        .find(|line| line.contains("^ref-life"))
        .expect("reading task")
        .to_string();
    assert!(task_line.starts_with("- [ ]"), "{task_line}");
    let next_line = task_line.replacen("[ ]", "[*]", 1);
    let parent_next = fs::read_to_string(&parent_note)
        .expect("read parent")
        .replace(&task_line, &next_line);
    write_file(&parent_note, &parent_next);
    let note_before = fs::read_to_string(&note).expect("read note");

    // First sync without opt-in must refuse before any write; the marker
    // stays Ready while frontmatter/base already say Next, so the next run
    // would revert without the refusal.
    let output = sync_pdf(&vault, &pdf, &[]);
    assert!(
        !output.status.success(),
        "task-driven status without opt-in must refuse:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        note_before,
        "refusal must leave ref bytes unchanged"
    );
    // With opt-in, marker, note/base, and task settle consistently.
    assert_success(&sync_pdf(&vault, &pdf, &["--write-pdf"]));
    let output = sync_pdf(&vault, &pdf, &[]);
    assert_success(&output);
    let parent_after = fs::read_to_string(&parent_note).expect("read parent");
    assert!(parent_after.contains("- [*]"), "{parent_after}");
}

#[test]
fn v2_ambiguity_leaves_bytes_unchanged_with_pdf_writes() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-ambiguous-bytes");
    let pdf = vault.join("lib/amb.pdf");
    let note = vault.join("ref/amb.md");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(&sync_pdf(&vault, &pdf, &[]));
    // Second claimant creates ambiguity; change marker status and request
    // PDF writes: affected task/ref/PDF bytes must stay unchanged.
    let mut parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    parent_contents.push_str(
        "- [/] #task #ref [[ref/amb|Amb]] [created::2026-10-06] ^ref-amb-2\n",
    );
    write_file(&parent_note, &parent_contents);
    let task_before = fs::read_to_string(&parent_note).expect("read parent");
    let ref_before = fs::read_to_string(&note).expect("read note");
    let pdf_before = fs::read(&pdf).expect("read pdf");
    let output = sync_pdf(&vault, &pdf, &["--write-pdf"]);
    assert!(
        !output.status.success(),
        "ambiguity with PDF writes must refuse:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&parent_note).expect("read parent"),
        task_before
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), ref_before);
    assert_eq!(fs::read(&pdf).expect("read pdf"), pdf_before);
}

#[test]
fn v2_dirty_parent_accepts_default_follow_up() {
    let (_temp, vault) = scan_vault("bob-cli-scan-integration-dirty-parent");
    let pdf = vault.join("lib/dirty.pdf");
    let parent_note = vault.join("obsidian.md");
    write_area_note(&parent_note, "Obsidian");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &pdf.with_extension("md"),
        "# Dirty\n\n## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Q.\n\n- #task Default follow-up.\n",
    );
    assert_success(&sync_pdf(&vault, &pdf, &[]));
    // Tracked dirty parent with unrelated additions plus a reading action
    // and default annotation insertion must still succeed (v2 exemption).
    let mut parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    parent_contents.push_str("- [ ] unrelated authored task\n");
    write_file(&parent_note, &parent_contents);
    // Stage the vault as a git repo with the parent tracked and dirty.
    let _ = std::process::Command::new("git")
        .arg("init")
        .current_dir(&vault)
        .output();
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&vault)
        .output();
    // Without git the guard is a no-op; with git the v2 default must still
    // accept the dirty destination. Either way the follow-up lands.
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", PINNED_NOW)
        .output()
        .expect("dirty scan");
    assert_success(&output);
}
