//! Capture inbox route detection and the write-free choice guard.
use crate::support::*;
use std::{fs, path::Path};

const NOW: &str = "2026-10-24 09:30:00";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("inbox.md"), "# Inbox\n");
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Route me ^route-task\n",
            "- [*] #task Already Next ^already-next\n",
        ),
    );
    write_file(
        &vault.join("gkeep_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task GKeep task ^gkeep-task\n",
        ),
    );
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n",
    );
    let day_file = vault.join("20261024.md");
    write_file(&day_file, "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n");
    (temp, vault, day_file)
}

fn capture(
    vault: &Path,
    day_file: &Path,
    dry_run: bool,
    guard: bool,
    draft: &str,
) -> std::process::Output {
    capture_with_routes(vault, day_file, dry_run, guard, &[], draft)
}

fn capture_with_routes(
    vault: &Path,
    day_file: &Path,
    dry_run: bool,
    guard: bool,
    routes: &[&str],
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("-d");
    }
    if guard {
        command.arg("--require-inbox-route-choice");
    }
    for route in routes {
        command.arg("-i").arg(route);
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn capture_human(
    vault: &Path,
    day_file: &Path,
    routes: &[&str],
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    for route in routes {
        command.arg("-i").arg(route);
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn remove_dry_run(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("dry_run");
            for value in object.values_mut() {
                remove_dry_run(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_dry_run(value);
            }
        }
        _ => {}
    }
}

#[test]
fn ir_preview_report_matches_write_and_hidden_guard_refuses_write_free() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-report");
    let original_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("original inbox");
    let original_day = fs::read_to_string(&day_file).expect("original day");

    // IR1: a Ready inbox task linked by capture becomes a route candidate.
    // IR2: the batch also links an already-Next task and creates a new task;
    // neither adds a route candidate.
    let draft = "@mac_inbox:route-task\n\n@mac_inbox:already-next\n\nNew task @mac_inbox";
    let preview = capture(&vault, &day_file, true, false, draft);
    assert_success(&preview);
    let preview_json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("preview JSON");
    assert_eq!(preview_json["inbox_route"]["supported"], true);
    assert_eq!(preview_json["inbox_route"]["candidate_count"], 1);
    let candidate = &preview_json["inbox_route"]["candidates"][0];
    assert_eq!(candidate["key"], "mac_inbox.md#^route-task");
    assert_eq!(candidate["n"], 1);
    assert_eq!(candidate["note_path"], "mac_inbox.md");
    assert_eq!(candidate["inbox"], "mac_inbox");
    assert_eq!(candidate["previous_status_symbol"], " ");
    assert_eq!(candidate["status_symbol"], "*");
    assert_eq!(candidate["answer"], serde_json::Value::Null);
    assert_eq!(candidate["outcome"], "pending");
    let destinations = preview_json["inbox_route"]["destinations"]
        .as_array()
        .expect("destinations");
    assert!(destinations
        .iter()
        .any(|row| row["relative_path"] == "health.md"));
    assert!(!destinations
        .iter()
        .any(|row| row["relative_path"] == "mac_inbox.md"));
    assert!(!destinations
        .iter()
        .any(|row| row["relative_path"] == "gkeep_inbox.md"));
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("inbox unchanged"),
        original_inbox
    );

    let real = capture(&vault, &day_file, false, false, draft);
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    let mut preview_without_dry_run = preview_json;
    let mut real_without_dry_run = real_json;
    remove_dry_run(&mut preview_without_dry_run);
    remove_dry_run(&mut real_without_dry_run);
    assert_eq!(preview_without_dry_run, real_without_dry_run);

    // Restore the sandbox so the hidden guard is tested against the same
    // planned transition and can prove that neither note is touched.
    write_file(&vault.join("mac_inbox.md"), &original_inbox);
    write_file(&day_file, &original_day);
    // IR6: the client guard refuses before either note is written.
    let guarded = capture(&vault, &day_file, false, true, draft);
    assert_eq!(guarded.status.code(), Some(1));
    let error_json: serde_json::Value =
        serde_json::from_str(stdout(&guarded).trim()).expect("guard JSON");
    assert_eq!(error_json["code"], "inbox_route_choice_required");
    assert_eq!(error_json["inbox_route"]["candidate_count"], 1);
    assert!(error_json["error"]
        .as_str()
        .expect("error message")
        .contains("pass --inbox-route"));
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("guard inbox"),
        original_inbox
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("guard day"),
        original_day
    );
}

#[test]
fn ir_gkeep_inbox_child_is_classified_from_its_live_parent() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-gkeep");
    let output =
        capture(&vault, &day_file, true, false, "@gkeep_inbox:gkeep-task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    // IR3: a second direct area child of inbox.md is an inbox too.
    assert_eq!(json["inbox_route"]["candidate_count"], 1);
    assert_eq!(
        json["inbox_route"]["candidates"][0]["note_path"],
        "gkeep_inbox.md"
    );
}

#[test]
fn ir_pull_in_choice_refusal_precedes_inbox_route_choice_refusal() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-guard-order");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Finish prerequisite [id:: prerequisite] ^prerequisite\n",
    );
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [?] #task Recovered inbox task [dependsOn:: prerequisite] [id:: dependent] ^dependent\n",
            "- [ ] #task Route me ^route-task\n",
        ),
    );
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox before");
    let before_dev =
        fs::read_to_string(vault.join("dev.md")).expect("dev before");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--require-inbox-route-choice")
        .arg("--require-pull-in-choice")
        .arg("--")
        .arg("!dev:prerequisite")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    // IR7: a recovery would also change an inbox task, but the pull-in card
    // remains the first required choice in the submit sequence.
    assert_eq!(json["code"], "pull_in_choice_required");
    assert!(json.get("inbox_route").is_none());
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox after"),
        before_inbox
    );
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("dev after"),
        before_dev
    );
}

#[test]
fn inbox_route_choice_guard_stays_hidden_from_help() {
    let output = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("capture help");
    assert_success(&output);
    assert!(!stdout(&output).contains("require-inbox-route-choice"));
}

fn route_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("inbox.md"), "# Inbox\n");
    let day_file = vault.join("20261024.md");
    (temp, vault, day_file)
}

fn write_inbox_task(vault: &Path, line: &str) {
    write_file(
        &vault.join("mac_inbox.md"),
        &format!(
            "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n{line}\n"
        ),
    );
}

fn write_area(vault: &Path, name: &str) {
    write_file(&vault.join(name), "---\ntype: [[area]]\n---\n## Tasks\n");
}

#[test]
fn ir_routed_link_moves_with_repair_stamp_follow_and_open_notes() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-apply");
    write_inbox_task(
        &vault,
        "- [ ] #task Route me [fresh:: 2026-10-20] [keeps:: 2] [unblocked:: 2026-10-23] ^route-task",
    );
    write_area(&vault, "health.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA1: a linked Ready inbox task moves to the answered area.
    let preview = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=health"],
        "@mac_inbox:route-task",
    );
    assert_success(&preview);
    let preview_json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("preview JSON");
    let route = &preview_json["inbox_route"];
    assert_eq!(route["routed_count"], 1);
    assert_eq!(route["kept_count"], 0);
    let candidate = &route["candidates"][0];
    assert_eq!(candidate["outcome"], "moved");
    assert_eq!(
        candidate["answer"],
        serde_json::json!({"action": "move", "destination": "health.md"})
    );
    let moved = &candidate["moved"];
    assert_eq!(moved["destination"], "health.md");
    assert_eq!(moved["block_id"], "route-task");
    assert_eq!(moved["previous_block_id"], "route-task");
    assert_eq!(moved["fresh"], "2026-10-24");
    assert_eq!(
        route["repaired_files"],
        serde_json::json!([{
            "path": "20261024.md",
            "link_count": 1,
            "dependency_token_count": 0
        }])
    );
    assert_eq!(route["renamed_block_ids"], serde_json::json!([]));
    assert_eq!(route["needs_reconcile"], false);
    let open = &preview_json["open_notes"][0];
    assert_eq!(open["relative_path"], "health.md");
    assert_eq!(open["line"], moved["line"]);
    assert_eq!(open["block_id"], "route-task");

    let real = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health"],
        "@mac_inbox:route-task",
    );
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    // IRA2: dry-run JSON equals real-run JSON with the same flags.
    let mut preview_without_dry_run = preview_json;
    let mut real_without_dry_run = real_json;
    remove_dry_run(&mut preview_without_dry_run);
    remove_dry_run(&mut real_without_dry_run);
    assert_eq!(preview_without_dry_run, real_without_dry_run);

    // The move carries the fresh stamp and clears keeps/unblocked; the
    // daily Task Link follows the task to its new home.
    let health =
        fs::read_to_string(vault.join("health.md")).expect("health note");
    assert!(
        health
            .contains("- [*] #task Route me [fresh:: 2026-10-24] ^route-task"),
        "{health}"
    );
    assert!(!health.contains("keeps::"), "{health}");
    assert!(!health.contains("unblocked::"), "{health}");
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox note");
    assert!(!inbox.contains("route-task"), "{inbox}");
    let day = fs::read_to_string(&day_file).expect("day file");
    assert!(day.contains("[[health#^route-task]]"), "{day}");
    assert!(!day.contains("mac_inbox#^route-task"), "{day}");

    // Human output names the route and the repaired link.
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );
    let human = capture_human(
        &vault,
        &day_file,
        &["1=health"],
        "@mac_inbox:route-task",
    );
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("routed  Route me  mac_inbox → health"),
        "{out}"
    );
    assert!(out.contains("1 link repaired"), "{out}");
}

#[test]
fn ir_completed_task_routes_home_without_stamp() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-close");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA3: completions route too, and closed tasks are never stamped.
    let output = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=health"],
        "!mac_inbox:route-task",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let candidate = &json["inbox_route"]["candidates"][0];
    assert_eq!(candidate["outcome"], "moved");
    assert_eq!(candidate["moved"]["fresh"], serde_json::Value::Null);

    let real = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health"],
        "!mac_inbox:route-task",
    );
    assert_success(&real);
    let health =
        fs::read_to_string(vault.join("health.md")).expect("health note");
    assert!(health.contains("- [x] #task Route me"), "{health}");
    assert!(!health.contains("fresh::"), "{health}");
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox note");
    assert!(!inbox.contains("route-task"), "{inbox}");
}

#[test]
fn ir_idless_recovered_dependent_moves_by_line() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-idless");
    write_inbox_task(
        &vault,
        "- [?] #task Idless dependent [dependsOn:: dev__prereq]",
    );
    write_area(&vault, "health.md");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Prereq [id:: dev__prereq] ^prereq\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    let preview = capture(&vault, &day_file, true, false, "!dev:prereq");
    assert_success(&preview);
    let preview_json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("preview JSON");
    // IRA4: the recovery is a candidate with an id-less opaque key.
    assert_eq!(preview_json["inbox_route"]["candidate_count"], 1);
    let key = preview_json["inbox_route"]["candidates"][0]["key"]
        .as_str()
        .expect("candidate key")
        .to_string();
    assert!(key.starts_with("mac_inbox.md@"), "{key}");
    assert_eq!(preview_json["inbox_route"]["candidates"][0]["block_id"], "");

    // The key answer moves the task by exact line; the route stamp sets
    // fresh and clears the recovery's unblocked marker.
    let answer = format!("{key}=health");
    let real = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &[&answer],
        "!dev:prereq",
    );
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    let candidate = &real_json["inbox_route"]["candidates"][0];
    assert_eq!(candidate["outcome"], "moved");
    assert_eq!(candidate["moved"]["fresh"], "2026-10-24");
    let health =
        fs::read_to_string(vault.join("health.md")).expect("health note");
    assert!(
        health.contains(
            "- [ ] #task Idless dependent [fresh:: 2026-10-24] [dependsOn:: dev__prereq]"
        ),
        "{health}"
    );
    assert!(!health.contains("unblocked::"), "{health}");
    assert!(!health.contains('^'), "{health}");
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox note");
    assert!(!inbox.contains("Idless dependent"), "{inbox}");
}

#[test]
fn ir_block_id_collision_renames_on_route() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-rename");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Resident ^route-task\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA5: a destination-side collision renames the moved block.
    let output = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=health"],
        "@mac_inbox:route-task",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let candidate = &json["inbox_route"]["candidates"][0];
    assert_eq!(candidate["outcome"], "moved");
    let moved = &candidate["moved"];
    assert_eq!(moved["previous_block_id"], "route-task");
    let new_id = moved["block_id"].as_str().expect("new block id");
    assert_ne!(new_id, "route-task");
    assert_eq!(
        json["inbox_route"]["renamed_block_ids"],
        serde_json::json!([{
            "path": "health.md",
            "old_block_id": "route-task",
            "new_block_id": new_id,
        }])
    );
    assert_eq!(
        json["inbox_route"]["repaired_files"],
        serde_json::json!([{
            "path": "20261024.md",
            "link_count": 1,
            "dependency_token_count": 0
        }])
    );

    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    let human = capture_human(
        &vault,
        &day_file,
        &["1=health"],
        "@mac_inbox:route-task",
    );
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains(&format!("^route-task renamed ^{new_id}")),
        "{out}"
    );
    let day = fs::read_to_string(&day_file).expect("day file");
    assert!(day.contains(&format!("[[health#^{new_id}]]")), "{day}");
}

#[test]
fn ir_two_candidates_move_to_two_destinations_in_one_bulk() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-bulk");
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n- [ ] #task Route me ^route-task\n- [ ] #task Second ^second\n",
    );
    write_area(&vault, "health.md");
    write_area(&vault, "other.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA6: one bulk draft routes two candidates to two destinations;
    // the second group plans against the first group's rewritten source.
    let output = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health", "2=other"],
        "@mac_inbox:route-task\n\n@mac_inbox:second",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let candidates = json["inbox_route"]["candidates"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["outcome"], "moved");
    assert_eq!(candidates[0]["moved"]["destination"], "health.md");
    assert_eq!(candidates[1]["outcome"], "moved");
    assert_eq!(candidates[1]["moved"]["destination"], "other.md");
    assert_eq!(json["inbox_route"]["routed_count"], 2);
    let health =
        fs::read_to_string(vault.join("health.md")).expect("health note");
    assert!(health.contains("^route-task"), "{health}");
    assert!(!health.contains("^second"), "{health}");
    let other = fs::read_to_string(vault.join("other.md")).expect("other");
    assert!(other.contains("^second"), "{other}");
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox note");
    assert!(!inbox.contains("route-task"), "{inbox}");
    assert!(!inbox.contains("Second"), "{inbox}");
}

#[test]
fn ir_keep_answers_keep_the_task_and_satisfy_the_guard() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-keep");
    write_area(&vault, "health.md");

    // IRA7: `keep` and `N=` both keep; kept candidates satisfy the guard.
    // Each answer runs against a fresh link so the candidate recurs. The
    // link transition still commits; only the move is skipped.
    for routes in [&["keep"][..], &["1="][..]] {
        write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
        write_file(
            &vault.join("20261024.md"),
            "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
        );
        let output = capture_with_routes(
            &vault,
            &day_file,
            false,
            true,
            routes,
            "@mac_inbox:route-task",
        );
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("keep JSON");
        let candidate = &json["inbox_route"]["candidates"][0];
        assert_eq!(candidate["outcome"], "kept");
        assert_eq!(candidate["answer"], serde_json::json!({"action": "keep"}));
        assert_eq!(json["inbox_route"]["kept_count"], 1);
        let inbox =
            fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
        assert!(inbox.contains("- [*] #task Route me"), "{inbox}");
        let day = fs::read_to_string(&day_file).expect("day");
        assert!(day.contains("[[mac_inbox#^route-task]]"), "{day}");
        let health =
            fs::read_to_string(vault.join("health.md")).expect("health");
        assert!(!health.contains("route-task"), "{health}");
    }

    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );
    let human =
        capture_human(&vault, &day_file, &["keep"], "@mac_inbox:route-task");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("kept  Route me · still in mac_inbox"), "{out}");
}

#[test]
fn ir_conflicting_duplicate_answers_are_usage_errors() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-conflict");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_area(&vault, "other.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA8: two destinations for one candidate fail without writing.
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox before");
    let output = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health", "1=other"],
        "@mac_inbox:route-task",
    );
    assert_eq!(output.status.code(), Some(2));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("conflict JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error")
            .contains("conflicting"),
        "{}",
        json["error"]
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox after"),
        before_inbox
    );
}

#[test]
fn ir_stale_route_key_lists_current_candidates() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-stale");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA9: an unknown answer fails with the current candidate list and
    // writes nothing.
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox before");
    let output = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["9=health"],
        "@mac_inbox:route-task",
    );
    assert_eq!(output.status.code(), Some(2));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("stale JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "inbox_route_stale");
    let error = json["error"].as_str().expect("error message");
    assert!(error.contains("pick: 9"), "{error}");
    assert!(
        error.contains("1 mac_inbox.md#^route-task Route me"),
        "{error}"
    );
    assert_eq!(json["inbox_route"]["candidate_count"], 1);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox after"),
        before_inbox
    );
}

#[test]
fn ir_route_refusals_name_the_destination_and_the_rule() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-refused");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_file(
        &vault.join("notasks.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\nNo tasks section here.\n",
    );
    write_file(
        &vault.join("blocked.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n## Tasks\n- [?] #task Ship it ^prj\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox before");

    // IRA10: an inbox destination refuses with its own message.
    let refused_inbox = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=mac_inbox"],
        "@mac_inbox:route-task",
    );
    assert_eq!(refused_inbox.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&refused_inbox).trim())
            .expect("refused JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "inbox_route_refused");
    assert_eq!(json["refused"]["key"], "mac_inbox.md#^route-task");
    assert_eq!(json["refused"]["n"], 1);
    assert_eq!(json["refused"]["destination"], "mac_inbox.md");
    assert!(
        json["refused"]["message"]
            .as_str()
            .expect("message")
            .contains("is an inbox note"),
        "{}",
        json["refused"]
    );

    // IRA11: a project without `## Tasks` refuses with the engine rule.
    let refused_tasks = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=notasks"],
        "@mac_inbox:route-task",
    );
    assert_eq!(refused_tasks.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&refused_tasks).trim())
            .expect("refused JSON");
    assert_eq!(json["code"], "inbox_route_refused");
    assert!(
        json["refused"]["message"]
            .as_str()
            .expect("message")
            .contains("has no ## Tasks section"),
        "{}",
        json["refused"]
    );

    // IRA12: a Blocked `^prj` refuses a Next task with the engine rule.
    // (`?` needs the Blocked status settings, which the shared fixture
    // lacks, so this vault carries its own project note only.)
    let refused_blocked = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=blocked"],
        "@mac_inbox:route-task",
    );
    assert_eq!(refused_blocked.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&refused_blocked).trim())
            .expect("refused JSON");
    assert_eq!(json["code"], "inbox_route_refused");
    assert!(
        json["refused"]["message"]
            .as_str()
            .expect("message")
            .contains("only [?] tasks can move there"),
        "{}",
        json["refused"]
    );

    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox after"),
        before_inbox
    );
}

#[test]
fn ir_pull_in_and_route_answers_combine_in_one_batch() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-pullin");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Prereq [id:: dev__prereq] ^prereq\n- [?] #task Dev dependent [dependsOn:: dev__prereq] [id:: dev__dep] ^dep\n",
    );
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n- [?] #task Inbox dependent [dependsOn:: dev__prereq] [id:: mac_inbox__dep] ^dep\n",
    );
    write_area(&vault, "health.md");
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA13: a picked successor offer and a route answer apply together.
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-d")
        .arg("-p")
        .arg("all")
        .arg("-i")
        .arg("1=health")
        .arg("--")
        .arg("!dev:prereq")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert!(json["pull_in"]["picked_count"].as_u64().unwrap_or(0) > 0);
    let candidates = json["inbox_route"]["candidates"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["outcome"], "moved");
    assert_eq!(candidates[0]["moved"]["destination"], "health.md");
}

#[test]
fn ir_dependency_touch_follows_its_routed_task() {
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-dep");
    write_blocked_tasks_settings(&vault);
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Prereq ^prereq\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );

    // IRA14: a dependency touch's task-block entry follows the routed
    // task to its destination line, and the engine's BLOCKS mirror is
    // repaired to the new home.
    let output = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health"],
        "^mac_inbox:route-task &dev:prereq",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let moved = &json["inbox_route"]["candidates"][0]["moved"];
    assert_eq!(moved["destination"], "health.md");
    let blocks = json["task_blocks"].as_array().expect("task_blocks");
    let followed = blocks
        .iter()
        .find(|block| {
            block["relative_target"] == "health.md"
                && block["block_id"] == "route-task"
        })
        .expect("followed task block");
    assert_eq!(followed["line"], moved["line"]);
    let dev = fs::read_to_string(vault.join("dev.md")).expect("dev note");
    assert!(dev.contains("[[health#^route-task]]"), "{dev}");
    assert!(!dev.contains("mac_inbox#^route-task"), "{dev}");
}

fn journal_lines(vault: &Path) -> Vec<String> {
    let dir = vault.join(".bob/relations");
    let mut lines = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return lines;
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    for path in paths {
        let body = fs::read_to_string(&path).expect("journal file");
        lines.extend(body.lines().map(str::to_string));
    }
    lines
}

#[test]
fn ir_routed_bulk_with_one_off_relation_repairs_once_and_guards_write_nothing()
{
    // Finish tale §3: a status-changing inbox item plus a one-off
    // relation item. A route guard/refusal writes neither notes nor
    // hide events; a routed success repairs the relation links and
    // appends the intended hide event once.
    let (_temp, vault, day_file) = route_vault("bob-cli-inbox-route-rel");
    write_inbox_task(&vault, "- [ ] #task Route me ^route-task");
    write_area(&vault, "health.md");
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Design ^foo\n",
    );
    write_file(
        &vault.join("20261024.md"),
        "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n",
    );
    let draft = "@mac_inbox:route-task\n\nFix trip ~work:foo||oneoff";
    let inbox_before =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    let health_before =
        fs::read_to_string(vault.join("health.md")).expect("health");

    // Guard writes nothing: notes and hide journal stay identical.
    let guard = capture_with_routes(&vault, &day_file, false, true, &[], draft);
    assert!(!guard.status.success());
    let guard_json: serde_json::Value =
        serde_json::from_str(stdout(&guard).trim()).expect("guard JSON");
    assert_eq!(
        guard_json["code"], "inbox_route_choice_required",
        "{guard_json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox"),
        inbox_before
    );
    assert_eq!(
        fs::read_to_string(vault.join("health.md")).expect("health"),
        health_before
    );
    assert!(journal_lines(&vault).is_empty());

    // Refusal writes nothing either.
    let refused = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=mac_inbox"],
        draft,
    );
    assert!(!refused.status.success());
    let refused_json: serde_json::Value =
        serde_json::from_str(stdout(&refused).trim()).expect("refused JSON");
    assert_eq!(
        refused_json["code"], "inbox_route_refused",
        "{refused_json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox"),
        inbox_before
    );
    assert!(journal_lines(&vault).is_empty());

    // Dry-run and real-run agree except for `dry_run`.
    let preview = capture_with_routes(
        &vault,
        &day_file,
        true,
        false,
        &["1=health"],
        draft,
    );
    assert_success(&preview);
    let mut preview_json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("preview JSON");
    remove_dry_run(&mut preview_json);
    let real = capture_with_routes(
        &vault,
        &day_file,
        false,
        false,
        &["1=health"],
        draft,
    );
    assert_success(&real);
    let mut real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    remove_dry_run(&mut real_json);
    assert_eq!(preview_json, real_json, "dry-run matches real-run");
    assert_eq!(
        real_json["inbox_route"]["candidates"][0]["outcome"],
        "moved"
    );
    // The relation hide ran once on commit, and the moved task's fresh
    // stamp and repaired links are in the destination.
    let hides = real_json["captures"][1]["relation_label_hides"]
        .as_array()
        .expect("hides");
    assert!(
        hides
            .iter()
            .any(|hide| hide["key"] == "oneoff" && hide["action"] == "hide"),
        "{real_json}"
    );
    assert_eq!(journal_lines(&vault).len(), 1);
    let health = fs::read_to_string(vault.join("health.md")).expect("health");
    assert!(health.contains("- [*] #task Route me"), "{health}");
    assert!(health.contains("[fresh:: 2026-10-24]"), "{health}");
}
