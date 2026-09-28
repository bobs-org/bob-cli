//! Capture batch and global destination tests.

use crate::support::*;
use std::fs;

#[test]
fn capture_batch_json_is_ordered_and_keeps_legacy_top_level() {
    let temp = TempDir::new("bob-cli-capture-batch-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = concat!(
        "first task @work\n",
        "- child detail\n",
        "\n",
        "second note @notes#Ideas\n",
        "\n",
        "third task @work",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run batch json capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["text"], "first task");
    assert_eq!(json["route"], "work");
    assert_eq!(json["relative_target"], "work.md");
    assert_eq!(json["sub_bullets"], serde_json::json!(["\t- child detail"]));

    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 3);
    assert_eq!(captures[0]["text"], "first task");
    assert_eq!(captures[0]["route"], "work");
    assert_eq!(captures[0]["relative_target"], "work.md");
    assert_eq!(captures[1]["text"], "second note");
    assert_eq!(captures[1]["kind"], "bullet");
    assert_eq!(captures[1]["route"], "notes");
    assert_eq!(captures[1]["relative_target"], "notes.md");
    assert_eq!(captures[2]["text"], "third task");
    assert_eq!(captures[2]["route"], "work");

    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task first task [created::2026-06-15]\n",
            "\t- child detail\n",
            "- [ ] #task third task [created::2026-06-15]\n",
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("notes.md")).expect("read notes route"),
        "- second note [created::2026-06-15]\n"
    );
}

#[test]
fn capture_batch_dry_run_reports_all_items_without_writing() {
    let temp = TempDir::new("bob-cli-capture-batch-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("one @work\n\ntwo @notes")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run dry-run batch capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["captures"].as_array().expect("captures").len(), 2);
    assert_eq!(json["captures"][0]["text"], "one");
    assert_eq!(json["captures"][1]["text"], "two");
    assert!(!vault.join("work.md").exists());
    assert!(!vault.join("notes.md").exists());
}

#[test]
fn capture_batch_human_output_numbers_items() {
    let temp = TempDir::new("bob-cli-capture-batch-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("one @work\n\ntwo @notes")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run human batch capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would capture  1/2  work.md")
            && out.contains("would capture  2/2  notes.md"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_batch_duplicate_block_id_failure_leaves_no_partial_write() {
    let temp = TempDir::new("bob-cli-capture-batch-duplicate-id");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("first @work^dup\n\nsecond @work^dup")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run duplicate block-id batch capture");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("capture item 2 starting on line 3")
            && stderr(&output).contains("block ID ^dup already exists"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("work.md").exists());
}

#[test]
fn capture_global_destination_routes_unmarked_items_and_keeps_overrides() {
    let temp = TempDir::new("bob-cli-capture-global-destination");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = concat!(
        "@@foo\n",
        "First task\n",
        "\n",
        "Second task @bar\n",
        "\n",
        "Third task\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run global destination capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["text"], "First task");
    assert_eq!(json["route"], "foo");
    assert_eq!(json["global_destination"]["mode"], "task");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert!(json["global_destination"].get("block_id").is_none());
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 3);
    assert_eq!(captures[0]["route"], "foo");
    assert_eq!(captures[1]["route"], "bar");
    assert_eq!(captures[2]["route"], "foo");
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("foo"),
        concat!(
            "- [ ] #task First task [created::2026-06-15]\n",
            "- [ ] #task Third task [created::2026-06-15]\n",
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("bar.md")).expect("bar"),
        "- [ ] #task Second task [created::2026-06-15]\n"
    );
}

#[test]
fn capture_global_destination_can_be_declared_at_the_end_of_any_item() {
    let temp = TempDir::new("bob-cli-capture-global-trailing");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = concat!("First task\n", "\n", "Second task s:2 @@foo\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run trailing global destination capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert_eq!(json["captures"][0]["route"], "foo");
    assert_eq!(json["captures"][1]["route"], "foo");
    assert_eq!(json["captures"][1]["text"], "Second task");
    assert_eq!(json["captures"][1]["scheduled"], "2026-06-17");
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("foo"),
        concat!(
            "- [ ] #task First task [created::2026-06-15]\n",
            "- [?] #task Second task [created::2026-06-15] [scheduled::2026-06-17]\n",
        )
    );
}

#[test]
fn capture_global_destination_on_authored_child_line_applies_draft_wide() {
    let temp = TempDir::new("bob-cli-capture-global-child-line");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = concat!("Parent\n", "- child @@foo\n", "\n", "Other\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run child-line global destination capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert_eq!(json["captures"][0]["route"], "foo");
    assert_eq!(json["captures"][0]["sub_bullets"][0], "\t- child");
    assert!(!json["captures"][0]["sub_bullets"][0]
        .as_str()
        .unwrap()
        .contains("@@"));
    assert_eq!(json["captures"][1]["route"], "foo");
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("foo"),
        concat!(
            "- [ ] #task Parent [created::2026-06-15]\n",
            "\t- child\n",
            "- [ ] #task Other [created::2026-06-15]\n",
        )
    );
}

#[test]
fn capture_global_sub_bullet_inserts_ordered_siblings_and_keeps_authored_children(
) {
    let temp = TempDir::new("bob-cli-capture-global-sub-bullet");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("foo.md"),
        concat!(
            "## Tasks\n",
            "- [ ] #task Parent [created::2026-06-01] ^a-id\n",
            "\t- existing\n",
        ),
    );
    write_file(
        &vault.join("bar.md"),
        concat!(
            "## Tasks\n",
            "- [ ] #task Other [created::2026-06-01] ^b-id\n",
        ),
    );

    let draft = concat!(
        "@@foo+a-id\n",
        "First note\n",
        "- authored detail\n",
        "\n",
        "Second note\n",
        "\n",
        "Independent task @bar\n",
        "\n",
        "Different parent @bar+b-id\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run global sub-bullet capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["global_destination"]["mode"], "sub_bullet");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert_eq!(json["global_destination"]["block_id"], "a-id");
    assert_eq!(json["captures"].as_array().expect("captures").len(), 4);
    assert_eq!(json["captures"][0]["kind"], "sub_bullet");
    assert_eq!(json["captures"][1]["kind"], "sub_bullet");
    assert_eq!(json["captures"][2]["kind"], "task");
    assert_eq!(json["captures"][2]["route"], "bar");
    assert_eq!(json["captures"][3]["kind"], "sub_bullet");
    assert_eq!(json["captures"][3]["route"], "bar");
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("foo"),
        concat!(
            "## Tasks\n",
            "- [ ] #task Parent [created::2026-06-01] ^a-id\n",
            "\t- existing\n",
            "\t- First note\n",
            "\t\t- authored detail\n",
            "\t- Second note\n",
        )
    );
    let bar = fs::read_to_string(vault.join("bar.md")).expect("bar");
    assert!(bar.contains("- [ ] #task Independent task [created::2026-06-15]"));
    assert!(
        bar.contains("\t- Different parent")
            || bar.contains("  - Different parent"),
        "{bar}"
    );
}

#[test]
fn capture_global_destination_conflict_and_declaration_only_are_usage_errors() {
    let temp = TempDir::new("bob-cli-capture-global-errors");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let conflict = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("bar")
        .arg("@@foo\nTask")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run competing destination capture");
    assert_eq!(
        conflict.status.code(),
        Some(2),
        "{}",
        format_output(&conflict)
    );
    assert!(
        stderr(&conflict)
            .contains("competing document-wide destination controls")
            && stderr(&conflict).contains("--route"),
        "{}",
        format_output(&conflict)
    );
    assert!(!vault.join("foo.md").exists());
    assert!(!vault.join("bar.md").exists());

    let declaration_only = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@@foo")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run declaration-only capture");
    assert_eq!(
        declaration_only.status.code(),
        Some(2),
        "{}",
        format_output(&declaration_only)
    );
    assert!(
        stderr(&declaration_only).contains("add a capture item"),
        "{}",
        format_output(&declaration_only)
    );
}

#[test]
fn capture_global_batch_failure_leaves_every_fixture_unchanged() {
    let temp = TempDir::new("bob-cli-capture-global-rollback");
    let vault = temp.path().join("vault");
    let foo =
        concat!("## Tasks\n", "- [ ] #task Keep [created::2026-06-01]\n",);
    write_file(&vault.join("foo.md"), foo);
    let bar = "- [ ] #task Existing [created::2026-06-01]\n";
    write_file(&vault.join("bar.md"), bar);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@@foo\nFirst\n\nSecond @bar\n\nThird @foo^dup\n\nFourth @foo^dup")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run failing global batch");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(vault.join("foo.md")).expect("foo"), foo);
    assert_eq!(fs::read_to_string(vault.join("bar.md")).expect("bar"), bar);
}

#[test]
fn capture_global_destination_human_and_stdin_and_dry_run() {
    let temp = TempDir::new("bob-cli-capture-global-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = "@@foo\n\nFirst task\n\nSecond task";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--dry-run")
            .env("BOB_NOW", "2026-06-15"),
        draft,
    );
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("global  foo.md")
            && out.contains("would capture  1/2  foo.md")
            && out.contains("would capture  2/2  foo.md"),
        "{out}"
    );
    assert!(!vault.join("foo.md").exists());
    assert_stdout_has_no_ansi(&output);
}
