//! Task block-id marker and retired marker.

use crate::support::*;
use std::fs;

#[test]
fn capture_task_block_id_marker_writes_ordinary_task_and_ignores_daily_note() {
    let temp = TempDir::new("bob-cli-capture-task-block-id-existing");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, "## Notes\n- not a Pomodoro ledger\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["Do", "work", "@Dev^id-only"])
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run ID-only capture");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "task");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "id-only");
    assert_eq!(
        json["task_line"],
        "- [ ] #task Do work [created::2026-07-10] ^id-only"
    );
    assert!(json.get("day_file").is_none(), "{json}");
    assert!(json.get("block_link").is_none(), "{json}");
    assert!(json.get("pomodoro_link_placement").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [ ] #task Do work [created::2026-07-10] ^id-only\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read untouched daily note"),
        "## Notes\n- not a Pomodoro ledger\n"
    );

    let scheduled_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["Do", "later", "s:0", "@Dev^scheduled-id"])
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run scheduled ID-only capture");

    assert_success(&scheduled_output);
    let scheduled_json: serde_json::Value =
        serde_json::from_str(stdout(&scheduled_output).trim())
            .expect("scheduled capture JSON");
    assert_eq!(scheduled_json["scheduled"], "2026-07-10");
    assert_eq!(scheduled_json["block_id"], "scheduled-id");
    assert_eq!(
        scheduled_json["task_line"],
        "- [?] #task Do later [created::2026-07-10] [scheduled::2026-07-10] ^scheduled-id"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [ ] #task Do work [created::2026-07-10] ^id-only\n",
            "- [?] #task Do later [created::2026-07-10] [scheduled::2026-07-10] ^scheduled-id\n",
        )
    );
}

#[test]
fn capture_task_block_id_marker_creates_missing_routed_note() {
    let temp = TempDir::new("bob-cli-capture-task-block-id-create");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let missing_day = vault.join("missing-day.md");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["New", "task", "@Projects^launch-id"])
        .env("BOB_DAY_FILE", &missing_day)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run ID-only capture into missing route");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("projects.md")).expect("read route"),
        "- [ ] #task New task [created::2026-07-10] ^launch-id\n"
    );
    assert!(
        !missing_day.exists(),
        "ID-only capture must not create or require the daily note"
    );
}

#[test]
fn capture_task_block_id_dry_run_and_duplicate_preflight_do_not_write() {
    let temp = TempDir::new("bob-cli-capture-task-block-id-preflight");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let before = "# Dev\n## Tasks\n- [ ] #task Existing ^dup\n";
    write_file(&target, before);

    let preview = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .args(["Preview", "@dev^new-id"])
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run ID-only dry-run");

    assert_success(&preview);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["block_id"], "new-id");
    assert_eq!(fs::read_to_string(&target).expect("read target"), before);

    let duplicate = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .args(["Duplicate", "@dev^dup"])
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run duplicate ID-only dry-run");

    assert_eq!(
        duplicate.status.code(),
        Some(1),
        "{}",
        format_output(&duplicate)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&duplicate).trim()).expect("failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("block ID ^dup already exists")),
        "{json}"
    );
    assert_eq!(fs::read_to_string(&target).expect("read target"), before);
}

#[test]
fn capture_malformed_task_block_id_marker_is_usage_error_without_writes() {
    let temp = TempDir::new("bob-cli-capture-task-block-id-malformed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["Do", "work", "@dev^bad.id"])
        .output()
        .expect("run malformed ID-only capture");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(stderr(&output).contains("task block-ID capture block ID"));
    assert_eq!(fs::read_dir(&vault).expect("read vault").count(), 0);
}

#[test]
fn capture_retired_double_colon_marker_is_usage_error_without_writes() {
    let temp = TempDir::new("bob-cli-capture-retired-double-colon");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["Follow", "up", "@file::new-id"])
        .output()
        .expect("run retired double-colon capture");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stderr(&output)
            .contains("'@<route>::<block-id>' is no longer accepted")
            && stderr(&output).contains("@<route>^<block-id>"),
        "{}",
        format_output(&output)
    );
    assert_eq!(fs::read_dir(&vault).expect("read vault").count(), 0);
}
