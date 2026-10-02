//! Capture `task_blocks` contract tests: batch-level parent-task blocks
//! for sub-bullet captures.

use crate::support::*;
use std::fs;

fn write_status_settings(vault: &std::path::Path) {
    write_toggle_task_settings(vault);
}

#[test]
fn capture_task_blocks_dry_run_reports_the_parent_block() {
    let temp = TempDir::new("bob-cli-task-blocks-dry-run");
    let vault = temp.path().join("vault");
    write_status_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [/] #task Port capture to PIW sase-core [created:: 2026-09-30] ^capture\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- 2026-10-01 moved\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("Should reuse as much of PIW sase-core code as possible! @sase+capture")
        .env("BOB_NOW", "2026-10-02")
        .output()
        .expect("dry-run task-blocks capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["dry_run"], true);
    let blocks = json["task_blocks"].as_array().expect("task_blocks array");
    assert_eq!(blocks.len(), 1);
    let block = &blocks[0];
    assert_eq!(block["relative_target"], "sase.md");
    assert_eq!(block["route"], "sase");
    assert_eq!(block["line"], 1);
    assert_eq!(block["block_id"], "capture");
    assert_eq!(block["text"], "Port capture to PIW sase-core");
    assert_eq!(block["status_symbol"], "/");
    assert_eq!(block["status_name"], "In Progress");
    assert_eq!(block["created"], false);
    assert_eq!(block["roles"], serde_json::json!(["sub_bullet"]));
    let lines = block["lines"].as_array().expect("lines array");
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0]["change"], "unchanged");
    assert_eq!(lines[0]["depth"], 0);
    assert_eq!(lines[3]["change"], "added");
    assert_eq!(lines[3]["depth"], 1);
    assert!(
        lines[3]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Should reuse")),
        "added row carries the new bullet: {block}"
    );
    assert!(
        lines.iter().all(|row| row.get("before").is_none()),
        "no changed rows means no before texts: {block}"
    );
    // Per-item parent fields stay as they are.
    assert_eq!(json["parent_text"], "Port capture to PIW sase-core");
    assert_eq!(json["parent_status_name"], "In Progress");
}

#[test]
fn capture_task_blocks_real_run_equals_dry_run_except_dry_run() {
    let temp = TempDir::new("bob-cli-task-blocks-real-run");
    let dry_vault = temp.path().join("dry");
    let real_vault = temp.path().join("real");
    for vault in [&dry_vault, &real_vault] {
        write_status_settings(vault);
        write_file(
            &vault.join("sase.md"),
            concat!("- [/] #task Parent ^capture\n", "\t- keep\n",),
        );
    }

    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&dry_vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("new note @sase+capture")
        .env("BOB_NOW", "2026-10-02")
        .output()
        .expect("dry-run");
    assert_success(&dry);
    let real = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&real_vault)
        .arg("-f")
        .arg("json")
        .arg("new note @sase+capture")
        .env("BOB_NOW", "2026-10-02")
        .output()
        .expect("real run");
    assert_success(&real);
    let mut dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry JSON");
    let mut real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    // Targets name the vault directory, so normalize before comparing.
    for value in [&mut dry_json, &mut real_json] {
        value["target"] = serde_json::Value::Null;
        value["relative_target"] = serde_json::Value::Null;
        if let Some(blocks) = value["task_blocks"].as_array_mut() {
            for block in blocks {
                block["relative_target"] = serde_json::Value::Null;
            }
        }
        if let Some(captures) = value["captures"].as_array_mut() {
            for item in captures {
                item["target"] = serde_json::Value::Null;
                item["relative_target"] = serde_json::Value::Null;
            }
        }
    }
    dry_json["dry_run"] = serde_json::json!(false);
    assert_eq!(dry_json, real_json, "dry-run equals real-run");
    assert_eq!(
        fs::read_to_string(real_vault.join("sase.md")).expect("read note"),
        concat!(
            "- [/] #task Parent ^capture\n",
            "\t- keep\n",
            "\t- new note\n",
        )
    );
    // The dry-run vault is untouched.
    assert_eq!(
        fs::read_to_string(dry_vault.join("sase.md")).expect("read dry note"),
        concat!("- [/] #task Parent ^capture\n", "\t- keep\n",)
    );
}

#[test]
fn capture_task_blocks_omitted_without_sub_bullets() {
    let temp = TempDir::new("bob-cli-task-blocks-omitted");
    let vault = temp.path().join("vault");
    write_status_settings(&vault);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("@sase Plain task")
        .env("BOB_NOW", "2026-10-02")
        .output()
        .expect("plain task capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "task");
    assert!(
        json.get("task_blocks").is_none(),
        "non-sub-bullet captures omit the key: {json}"
    );
}

#[test]
fn capture_task_blocks_human_output_is_unchanged() {
    let temp = TempDir::new("bob-cli-task-blocks-human");
    let vault = temp.path().join("vault");
    write_status_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        "- [/] #task Parent ^capture\n\t- keep\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("new note @sase+capture")
        .env("BOB_NOW", "2026-10-02")
        .output()
        .expect("human dry-run");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(human.contains("would capture"), "{human}");
    assert!(human.contains("sase.md"), "{human}");
    assert!(human.contains("- new note"), "{human}");
    assert!(
        !human.contains("task_blocks"),
        "human output never mentions the JSON key: {human}"
    );
}

#[test]
fn capture_task_blocks_and_pomodoro_blocks_report_independently() {
    let temp = TempDir::new("bob-cli-task-blocks-pomodoro");
    let vault = temp.path().join("vault");
    write_status_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Parent ^capture\n\t- keep\n",
    );
    let day_file = vault.join("2026/20261002.md");
    fs::create_dir_all(day_file.parent().expect("day parent"))
        .expect("create day parent");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "\t- [[sase#^capture]]\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("@sase+capture\n\nnote @sase+capture")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-10-02 10:00:00")
        .output()
        .expect("toggle plus sub-bullet batch");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let task_blocks =
        json["task_blocks"].as_array().expect("task_blocks array");
    assert_eq!(task_blocks.len(), 1);
    assert_eq!(task_blocks[0]["block_id"], "capture");
    assert_eq!(task_blocks[0]["lines"][0]["change"], "changed");
    assert!(
        task_blocks[0]["lines"][0]["before"]
            .as_str()
            .is_some_and(|before| before.contains("- [ ] #task Parent")),
        "toggled task line keeps its before text: {}",
        task_blocks[0]
    );
    assert!(
        json["pomodoro_blocks"]
            .as_array()
            .is_some_and(|blocks| !blocks.is_empty()),
        "the same batch still reports pomodoro blocks: {json}"
    );
}
