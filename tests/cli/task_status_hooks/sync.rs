//! Fixture sync, grouping, rank, duplicate prune.

use crate::support::*;
use std::fs;
use std::path::Path;

#[test]
fn task_status_hooks_syncs_fixture_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-task-status-hooks-sync");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let dev = vault.join("dev.md");
    let alpha = vault.join("Projects/Alpha.md");
    write_file(
        &daily,
        include_str!("../../fixtures/task_status_hooks/2026/20260710.md"),
    );
    write_file(
        &dev,
        include_str!("../../fixtures/task_status_hooks/dev.md"),
    );
    write_file(
        &alpha,
        include_str!("../../fixtures/task_status_hooks/Projects/Alpha.md"),
    );
    let original_dev = fs::read(&dev).expect("read fixture before dry-run");
    let original_alpha = fs::read(&alpha).expect("read fixture before dry-run");
    let original_daily = fs::read(&daily).expect("read daily before dry-run");

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run task-status-hooks fixture");
    assert_success(&dry_run);
    assert_eq!(
        fs::read(&dev).expect("read dev after dry-run"),
        original_dev
    );
    assert_eq!(
        fs::read(&alpha).expect("read alpha after dry-run"),
        original_alpha
    );
    assert_eq!(
        fs::read(&daily).expect("read daily after dry-run"),
        original_daily
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("dry-run JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["daily_file"], "2026/20260710.md");
    assert_eq!(json["open_pomodoros"], 2);
    assert_eq!(json["references"], 4);
    assert_eq!(json["dependency_references"], 3);
    assert_eq!(json["scanned_files"], 3);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 3);
    assert!(json["marked_in_progress"].as_array().unwrap().is_empty());
    assert_eq!(json["cleared"].as_array().unwrap().len(), 2);
    assert_eq!(json["kept_next"], 1);
    assert_eq!(json["kept_in_progress"], 1);
    assert_eq!(
        json["embedded_completed_references"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        json["struck_completed_references"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(json["struck_completed_references"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["removed_embed"] == true));
    assert_eq!(
        json["moved_completed_references"].as_array().unwrap().len(),
        0
    );
    assert_eq!(json["marker_added_references"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["marker_removed_references"].as_array().unwrap().len(),
        2
    );
    assert_eq!(json["removed_duplicate_lines"].as_array().unwrap().len(), 1);
    assert_eq!(json["removed_duplicate_lines"][0]["line_number"], 10);
    assert_eq!(
        json["removed_duplicate_lines"][0]["pomodoro"],
        "- [ ] Future session"
    );
    assert_eq!(
        json["removed_duplicate_lines"][0]["line"],
        "    - Finish [[dev#^done|completed work]] with [[dev#^promote]]"
    );
    assert_eq!(
        json["removed_duplicate_lines"][0]["duplicate_tasks"],
        serde_json::json!([
            {"path": "dev.md", "block_id": "done"},
            {"path": "dev.md", "block_id": "promote"}
        ])
    );
    assert_eq!(json["unresolved_references"].as_array().unwrap().len(), 1);
    assert!(json["unresolved_references"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("dependency from dev.md:3"));
    let marked = json["marked_next"].as_array().unwrap();
    assert!(marked.iter().any(|item| {
        item["path"] == "dev.md"
            && item["block_id"] == "promote"
            && item["dependency"] == false
    }));
    assert!(marked.iter().any(|item| {
        item["block_id"] == "dep-one" && item["dependency"] == true
    }));
    assert!(marked.iter().any(|item| {
        item["path"] == "Projects/Alpha.md"
            && item["block_id"] == "dep-two"
            && item["dependency"] == true
    }));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply task-status-hooks fixture");
    assert_success(&applied);
    let report = stdout(&applied);
    assert!(
        report.contains("marked next")
            && report.contains("cleared")
            && report.contains("marked Pomodoro references")
            && report.contains("unmarked Pomodoro references")
            && report.contains("removed duplicate task-link lines")
            && report.contains("(dependency)")
            && report.contains(
                "Summary: 3 marked next, 0 marked in progress, 2 cleared"
            ),
        "unexpected task-status-hooks report:\n{}",
        format_output(&applied)
    );
    let dev_contents = fs::read_to_string(&dev).expect("read updated dev");
    assert!(dev_contents.contains("- [*] #task Promote me ^promote"));
    assert!(dev_contents.contains("- [*] #task Same-file dependency ^dep-one"));
    assert!(dev_contents
        .contains("- [x] #task Completed dependency stays done ^done-dep"));
    assert!(dev_contents
        .contains("- [ ] #task Plain link is not a dependency ^plain"));
    assert!(dev_contents.contains(
        "- [ ] #task Fenced transclusion is not a dependency ^fenced-dep"
    ));
    assert!(dev_contents
        .contains("- [ ] #task Stale dependency clears ^stale-child"));
    assert!(dev_contents.contains("- [*] #task Already next ^already"));
    assert!(dev_contents.contains("- [ ] #task Clear me ^orphan"));
    assert!(dev_contents
        .contains("- [ ] #task Closed reference stays todo ^closed"));
    assert!(dev_contents.contains("- [x] #task Done stays done ^done"));
    assert!(dev_contents
        .contains("- [-] #task Cancelled stays cancelled ^cancelled"));
    assert!(dev_contents.contains("- [!] #task Unknown stays unknown ^unknown"));
    assert!(dev_contents.contains("- [*] Not a Tasks task ^not-a-task"));
    let daily_contents =
        fs::read_to_string(&daily).expect("read updated daily");
    assert!(daily_contents.contains(concat!(
        "  - [[dev#^promote]]\n",
        "  - Work on [[Projects/Alpha#^working]] and [[dev#^already]]\n",
        "  - ~~[[dev#^done|already embedded]]~~\n",
        "- [ ] Future session\n",
        "      - Keep this nested detail with the moved bullet.\n",
    )));
    assert!(daily_contents.contains(concat!(
        "- [x] Closed session (0930-1000)\n",
        "  - 🍅 [[dev#^closed]]\n",
        "  - ~~[[dev#^done|historical completed work]]~~\n",
        "  - ~~[[dev#^done|historical embedded work]]~~\n",
    )));
    let alpha_contents =
        fs::read_to_string(&alpha).expect("read updated alpha");
    assert!(alpha_contents
        .contains("- [*] #task Cross-file recursive dependency ^dep-two"));

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun task-status-hooks fixture");
    assert_success(&second);
    assert!(
        stdout(&second).contains("already in sync, no changes"),
        "expected idempotent no-op:\n{}",
        format_output(&second)
    );

    let canonical_json = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path().join("alias-cache"))
        .output()
        .expect("run canonical task-status-hooks JSON no-op");
    assert_success(&canonical_json);
    for alias in ["task-status-setter", "mark-next-tasks"] {
        let alias_json = bob_command()
            .arg(alias)
            .arg("--format")
            .arg("json")
            .arg("--bob-dir")
            .arg(&vault)
            .env("BOB_DAY_FILE", &daily)
            .env("BOB_CLI_USE_SCRIPT", "1")
            .env("XDG_CACHE_HOME", temp.path().join("alias-cache"))
            .output()
            .unwrap_or_else(|error| {
                panic!("run compatibility alias {alias} JSON no-op: {error}")
            });
        assert_success(&alias_json);
        assert_eq!(stdout(&alias_json), stdout(&canonical_json));
    }
    assert!(
        !temp.path().join("alias-cache/bob-cli/scripts").exists(),
        "task-status compatibility aliases must remain native-only"
    );

    let without_root = fs::read_to_string(&daily)
        .expect("read daily before removing root link")
        .replace("[[dev#^promote]]", "[[dev]]");
    write_file(&daily, &without_root);
    let stale_chain = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("clear stale dependency chain");
    assert_success(&stale_chain);
    let dev_contents = fs::read_to_string(&dev).expect("read cleared chain");
    assert!(dev_contents.contains("- [ ] #task Promote me ^promote"));
    assert!(dev_contents.contains("- [ ] #task Same-file dependency ^dep-one"));
    let alpha_contents =
        fs::read_to_string(&alpha).expect("read cleared alpha");
    assert!(alpha_contents
        .contains("- [ ] #task Cross-file recursive dependency ^dep-two"));
}

#[test]
fn task_status_hooks_groups_area_project_tasks_after_final_statuses() {
    let temp = TempDir::new("bob-cli-task-status-hooks-status-groups");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let project = vault.join("alpha.md");
    let area = vault.join("Areas/Home.md");
    let ordinary = vault.join("notes.md");
    let archived = vault.join("done/Archive.md");
    let generated = vault.join("_generated/Ref.md");
    let template = vault.join("_templates/Task.md");
    let state = temp.path().join("state");
    let lock = temp.path().join("task-status-hooks.lock");

    write_blocked_tasks_settings(&vault);
    write_file(
        &daily,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Today\n\n",
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[alpha#^promote]]\n",
            "  - [[Areas/Home#^already]]\n",
            "  - [[Areas/Home#^working]]\n",
            "\n",
            "## Tasks\n\n",
            "- [x] #task Daily task is not grouped ^daily\n",
        ),
    );
    write_file(
        &project,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Alpha\n\n",
            "## Tasks\n\n",
            "Project context.\n\n",
            "- [ ] #task Keep in intake ^ready\n",
            "- [ ] #task Promote from Pomodoro ^promote\n",
            "- [/] #task Stale work clears ^stale\n",
            "- [ ] #task Future blocked [scheduled:: 2026-07-11] ^future\n",
            "- [x] #task Finished ^done\n",
            "- [-] #task Canceled without block\n",
            "\n",
            "## Notes\n\n",
            "- [x] #task Outside Tasks is not grouped ^outside\n",
        ),
    );
    write_file(
        &area,
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Home\n\n",
            "## Tasks\n\n",
            "- [*] #task Already next ^already\n",
            "- [/] #task Keep working ^working\n",
        ),
    );
    write_file(
        &ordinary,
        concat!(
            "# Ordinary\n\n",
            "## Tasks\n\n",
            "- [x] #task Ordinary done ^ordinary\n",
        ),
    );
    for path in [&archived, &generated, &template] {
        write_file(
            path,
            concat!(
                "---\n",
                "type: [[project]]\n",
                "---\n",
                "# Excluded\n\n",
                "## Tasks\n\n",
                "- [x] #task Excluded done ^excluded\n",
            ),
        );
    }

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("dry-run grouped task-status-hooks");
    assert_success(&dry_run);
    assert!(
        stderr(&dry_run).is_empty(),
        "unexpected dry-run stderr:\n{}",
        format_output(&dry_run)
    );
    assert!(
        !state.join("bob-cli/task-status-hooks").exists(),
        "dry-run must not create recovery state"
    );
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim())
            .expect("grouping dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["marked_next"].as_array().unwrap().len(), 1);
    assert_eq!(dry_json["cleared_in_progress"].as_array().unwrap().len(), 1);
    assert_eq!(dry_json["marked_blocked"].as_array().unwrap().len(), 1);
    assert_eq!(
        dry_json["grouped_task_sections"].as_array().unwrap().len(),
        2
    );
    assert_eq!(dry_json["applied_files"], serde_json::json!([]));
    assert_eq!(dry_json["deferred_files"], serde_json::json!([]));
    assert_eq!(dry_json["recovery_directory"], serde_json::Value::Null);
    let project_group = dry_json["grouped_task_sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["path"] == "alpha.md")
        .expect("project grouping report");
    assert_eq!(project_group["original_heading_line"], 6);
    assert_eq!(
        project_group["heading_ancestry"],
        serde_json::json!(["Alpha", "Tasks"])
    );
    assert_eq!(project_group["open"], 2);
    assert_eq!(project_group["next_and_in_progress"], 1);
    assert_eq!(project_group["blocked"], 1);
    assert_eq!(project_group["done_and_canceled"], 2);
    assert_eq!(project_group["moved_block_count"], 4);
    assert_eq!(
        project_group["moved_blocks"][0],
        serde_json::json!({
            "original_line": 11,
            "destination": "next_and_in_progress"
        })
    );
    assert!(!fs::read_to_string(&project)
        .unwrap()
        .contains("Next & In Progress"));
    assert!(!fs::read_to_string(&area)
        .unwrap()
        .contains("Next & In Progress"));

    let human_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("human dry-run grouped task-status-hooks");
    assert_success(&human_dry_run);
    let human = stdout(&human_dry_run);
    assert!(
        human.contains("would group task sections")
            && human.contains("alpha.md")
            && human.contains("open 2 · next/in progress 1")
            && human.contains("Summary:")
            && !human.contains("already in sync, no changes"),
        "unexpected grouping human dry-run:\n{}",
        format_output(&human_dry_run)
    );

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("apply grouped task-status-hooks");
    assert_success(&applied);
    let applied_json: serde_json::Value =
        serde_json::from_str(stdout(&applied).trim())
            .expect("grouping apply JSON");
    assert_eq!(
        applied_json["applied_files"],
        serde_json::json!(["Areas/Home.md", "alpha.md"])
    );
    let recovery = applied_json["recovery_directory"]
        .as_str()
        .expect("recovery directory");
    assert!(
        Path::new(recovery).join("manifest.json").is_file(),
        "missing recovery manifest in {recovery}"
    );
    let project_after = fs::read_to_string(&project).unwrap();
    assert_text_order(
        &project_after,
        &[
            "## Tasks",
            "<!-- bob:task-status-badges:v1 -->",
            "[`⚪ 2 open`](#Alpha#Tasks)",
            "Project context.",
            "- [ ] #task Keep in intake ^ready",
            "- [ ] #task Stale work clears ^stale",
            "### Next & In Progress",
            "<!-- bob:task-status-group:v1:active -->",
            "- [*] #task Promote from Pomodoro ^promote",
            "### Blocked",
            "- [?] #task Future blocked [scheduled:: 2026-07-11] ^future",
            "### Done & Canceled",
            "- [x] #task Finished ^done",
            "- [-] #task Canceled without block",
            "## Notes",
            "- [x] #task Outside Tasks is not grouped ^outside",
        ],
    );
    let area_after = fs::read_to_string(&area).unwrap();
    assert_text_order(
        &area_after,
        &[
            "## Tasks",
            "<!-- bob:task-status-badges:v1 -->",
            "[`⚪ 0 open`](#Home#Tasks)",
            "### Next & In Progress",
            "- [*] #task Already next ^already",
            "- [/] #task Keep working ^working",
        ],
    );
    for path in [&daily, &ordinary, &archived, &generated, &template] {
        let contents = fs::read_to_string(path).unwrap();
        assert!(
            !contents.contains("bob:task-status-group")
                && !contents.contains("bob:task-status-badges"),
            "{} should not receive generated groups",
            path.display()
        );
    }

    let project_mtime = fs::metadata(&project).unwrap().modified().unwrap();
    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("rerun grouped task-status-hooks");
    assert_success(&second);
    assert!(
        stdout(&second).contains("already in sync, no changes"),
        "expected idempotent no-op:\n{}",
        format_output(&second)
    );
    assert_eq!(
        fs::metadata(&project).unwrap().modified().unwrap(),
        project_mtime
    );

    let capture = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Captured ready @alpha^captured")
        .env("BOB_NOW", "2026-07-10 10:11:12")
        .output()
        .expect("capture into grouped note");
    assert_success(&capture);
    let project_after_capture = fs::read_to_string(&project).unwrap();
    assert_text_order(
        &project_after_capture,
        &[
            "[`⚪ 2 open`](#Alpha#Tasks)",
            "- [ ] #task Stale work clears ^stale",
            "- [ ] #task Captured ready [created::2026-07-10] ^captured",
            "### Next & In Progress",
        ],
    );

    let after_ready_capture = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("rerun after ready capture");
    assert_success(&after_ready_capture);
    let after_ready_capture_report = stdout(&after_ready_capture);
    assert!(
        after_ready_capture_report.contains("open 3 · next/in progress 1"),
        "Ready capture should refresh the open badge:\n{}",
        format_output(&after_ready_capture)
    );
    let project_after_badge_refresh = fs::read_to_string(&project).unwrap();
    assert!(project_after_badge_refresh.contains("[`⚪ 3 open`](#Alpha#Tasks)"));

    let daily_with_captured = fs::read_to_string(&daily).unwrap().replace(
        "  - [[Areas/Home#^working]]\n\n## Tasks",
        "  - [[Areas/Home#^working]]\n  - [[alpha#^captured]]\n\n## Tasks",
    );
    write_file(&daily, &daily_with_captured);
    let promoted_capture = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("promote captured task");
    assert_success(&promoted_capture);
    let project_after_promotion = fs::read_to_string(&project).unwrap();
    assert_text_order(
        &project_after_promotion,
        &[
            "[`⚪ 2 open`](#Alpha#Tasks)",
            "### Next & In Progress",
            "- [*] #task Captured ready [created::2026-07-10] ^captured",
            "- [*] #task Promote from Pomodoro ^promote",
        ],
    );
}

#[test]
fn task_status_hooks_reports_grouping_warnings_without_noop_text() {
    let temp = TempDir::new("bob-cli-task-status-hooks-group-warning");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let area = vault.join("area.md");
    let duplicate_badges = vault.join("badges.md");

    write_file(
        &daily,
        concat!("# Today\n\n", "## Pomodoros\n\n", "- [ ] Current\n"),
    );
    write_file(
        &area,
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "# Area\n\n",
            "###### Tasks\n\n",
            "- [x] #task Cannot have generated child groups ^done\n",
        ),
    );
    let duplicate_badges_original = concat!(
        "---\n",
        "type: [[area]]\n",
        "---\n",
        "# Badges\n\n",
        "## Tasks\n",
        "<!-- bob:task-status-badges:v1 -->\n",
        "[`stale`](#Badges#Tasks)\n",
        "<!-- bob:task-status-badges:v1 -->\n",
        "\n",
        "- [*] #task Next task ^next\n",
    );
    write_file(&duplicate_badges, duplicate_badges_original);

    let json_output = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run grouping warning JSON");
    assert_success(&json_output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("grouping warning JSON");
    assert_eq!(json["grouped_task_sections"], serde_json::json!([]));
    assert_eq!(json["grouping_warnings"].as_array().unwrap().len(), 2);
    assert!(json["grouping_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["path"] == "area.md"
            && warning["code"] == "h6_container"));
    assert!(json["grouping_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["path"] == "badges.md"
            && warning["code"] == "malformed_badge_marker"));
    assert!(
        stderr(&json_output).contains("H6 heading")
            && stderr(&json_output).contains("task-status-badges"),
        "expected stderr warning:\n{}",
        format_output(&json_output)
    );
    assert_eq!(
        fs::read_to_string(&duplicate_badges).unwrap(),
        duplicate_badges_original
    );

    let human_output = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run grouping warning human");
    assert_success(&human_output);
    assert!(
        !stdout(&human_output).contains("already in sync, no changes"),
        "grouping warning should not be reported as a clean no-op:\n{}",
        format_output(&human_output)
    );
    assert!(
        stderr(&human_output).contains("H6 heading")
            && stderr(&human_output).contains("task-status-badges"),
        "expected human stderr warning:\n{}",
        format_output(&human_output)
    );
}

#[test]
fn task_status_hooks_uses_latest_previous_daily_for_scoped_in_progress_tasks() {
    let temp = TempDir::new("bob-cli-task-status-hooks-previous-daily");
    let vault = temp.path().join("vault");
    let current = vault.join("2026/20260721.md");
    let previous = vault.join("2026/20260710.md");
    let older = vault.join("2026/20260701.md");
    let area = vault.join("Areas/Home.md");
    let project = vault.join("Projects/Alpha.md");
    let ordinary = vault.join("notes.md");
    let tasks = vault.join("Tasks.md");
    let current_before = concat!(
        "# Current\n\n",
        "## Pomodoros\n\n",
        "- [ ] Current work (0900-0930)\n",
        "  - [[Projects/Alpha#^today]]\n",
        "  - [[Tasks#^current-root]]\n",
        "  - ![[Tasks#^done|finished]]\n",
    );
    let previous_before = concat!(
        "# Previous\n\n",
        "## Pomodoros\n\n",
        "- [x] Previous work (0900-0930)\n",
        "  - ![[Areas/Home#^previous|area alias]]\n",
        "  - [[Tasks#^previous-root]]\n",
        "  - [[Projects/Alpha#^historical-ready]]\n",
        "  - ~~[[Projects/Alpha#^retired]]~~\n",
        "  - 🍅 🍅 ![[Tasks#^done|historical bytes stay exact]]\n",
        "\n",
        "- [*] #task Historical daily task stays untouched ^daily-task\n",
    );
    let older_before = concat!(
        "# Older\n\n",
        "## Pomodoros\n\n",
        "- [ ] Old work\n",
        "  - [[Projects/Alpha#^older-only]]\n",
    );
    let area_before = concat!(
        "---\r\n",
        "type: \"[[area]]\"\r\n",
        "---\r\n",
        "- [/] #task Previous direct ^previous\r\n",
        "- [/] #task Previous dependency ^previous-dependency\r\n",
        "- [/] #task Current dependency ^current-dependency\r\n",
        "- [/] #task Stale area task ^stale\r\n",
        "- [/] #task Missing block id\r\n",
    );
    let area_after = concat!(
        "---\r\n",
        "type: \"[[area]]\"\r\n",
        "---\r\n",
        "- [/] #task Previous direct ^previous\r\n",
        "- [/] #task Previous dependency ^previous-dependency\r\n",
        "- [/] #task Current dependency ^current-dependency\r\n",
        "- [ ] #task Stale area task ^stale\r\n",
        "- [ ] #task Missing block id\r\n",
    );
    let project_before = concat!(
        "---\n",
        "type: [[project]]\n",
        "---\n",
        "- [/] #task Current direct ^today\n",
        "- [/] #task Older daily only ^older-only\n",
        "- [/] #task Retired historical link ^retired\n",
        "- [ ] #task Historical links do not promote ^historical-ready\n",
    );
    let project_after = concat!(
        "---\n",
        "type: [[project]]\n",
        "---\n",
        "- [/] #task Current direct ^today\n",
        "- [ ] #task Older daily only ^older-only\n",
        "- [ ] #task Retired historical link ^retired\n",
        "- [ ] #task Historical links do not promote ^historical-ready\n",
    );
    let ordinary_before = "- [/] #task Ordinary note stays active ^ordinary\n";
    let tasks_before = concat!(
        "- [ ] #task Current root ^current-root\n",
        "  - ![[Areas/Home#^current-dependency]]\n",
        "- [/] #task Previous root ^previous-root\n",
        "  - ![[Areas/Home#^previous-dependency]]\n",
        "- [x] #task Finished ^done\n",
    );
    write_file(&current, current_before);
    write_file(&previous, previous_before);
    write_file(&older, older_before);
    write_file(&area, area_before);
    write_file(&project, project_before);
    write_file(&ordinary, ordinary_before);
    write_file(&tasks, tasks_before);

    let human_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &current)
        .output()
        .expect("human dry-run rolling daily reconciliation");
    assert_success(&human_dry_run);
    assert!(stdout(&human_dry_run).contains("would clear in progress"));
    assert!(stdout(&human_dry_run).contains("previous 2026/20260710.md"));

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &current)
        .env("BOB_NOW", "2030-01-01 12:00:00")
        .output()
        .expect("dry-run rolling daily reconciliation");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&current).unwrap(), current_before);
    assert_eq!(fs::read_to_string(&previous).unwrap(), previous_before);
    assert_eq!(fs::read_to_string(&older).unwrap(), older_before);
    assert_eq!(fs::read_to_string(&area).unwrap(), area_before);
    assert_eq!(fs::read_to_string(&project).unwrap(), project_before);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("rolling daily dry-run JSON");
    assert_eq!(json["daily_file"], "2026/20260721.md");
    assert_eq!(json["previous_daily_file"], "2026/20260710.md");
    assert_eq!(json["previous_daily_references"], 4);
    assert_eq!(json["recent_activity_references"], 6);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 1);
    assert!(json["cleared"].as_array().unwrap().is_empty());
    assert_eq!(json["cleared_in_progress"].as_array().unwrap().len(), 4);
    assert!(json["cleared_in_progress"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"] == "Areas/Home.md" && item["block_id"] == ""));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &current)
        .env("BOB_NOW", "2030-01-01 12:00:00")
        .output()
        .expect("apply rolling daily reconciliation");
    assert_success(&applied);
    assert!(stdout(&applied).contains("cleared in progress"));
    assert!(stdout(&applied).contains("previous 2026/20260710.md"));
    assert_eq!(fs::read_to_string(&previous).unwrap(), previous_before);
    assert_eq!(fs::read_to_string(&older).unwrap(), older_before);
    assert_eq!(fs::read_to_string(&area).unwrap(), area_after);
    assert_eq!(fs::read_to_string(&project).unwrap(), project_after);
    assert_eq!(fs::read_to_string(&ordinary).unwrap(), ordinary_before);
    assert!(fs::read_to_string(&current)
        .unwrap()
        .contains("  - ~~[[Tasks#^done|finished]]~~\n"));
    assert!(fs::read_to_string(&tasks)
        .unwrap()
        .contains("- [*] #task Current root ^current-root\n"));

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &current)
        .output()
        .expect("rerun rolling daily reconciliation");
    assert_success(&second);
    assert!(stdout(&second).contains("already in sync, no changes"));

    let sectionless = vault.join("2026/20260720.md");
    let sectionless_before = "# A real daily note with no Pomodoros section\n";
    write_file(&sectionless, sectionless_before);
    let empty_previous = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &current)
        .output()
        .expect("run with sectionless previous daily");
    assert_success(&empty_previous);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&empty_previous).trim())
            .expect("sectionless previous JSON");
    assert_eq!(json["previous_daily_file"], "2026/20260720.md");
    assert_eq!(json["previous_daily_references"], 0);
    assert_eq!(
        fs::read_to_string(&sectionless).unwrap(),
        sectionless_before
    );
    let area_contents = fs::read_to_string(&area).unwrap();
    assert!(area_contents.contains("- [ ] #task Previous direct ^previous"));
    assert!(area_contents
        .contains("- [ ] #task Previous dependency ^previous-dependency"));
    assert!(area_contents
        .contains("- [/] #task Current dependency ^current-dependency"));
}

#[test]
fn task_status_hooks_propagates_strongest_rank_and_reports_in_progress_promotions(
) {
    let temp = TempDir::new("bob-cli-task-status-hooks-ranked-dependencies");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260714.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "# Daily\n\n",
        "## Pomodoros\n\n",
        "- [ ] Current (0900-0930)\n",
        "  - [[tasks#^root-next]]\n",
        "  - [[tasks#^root-working]]\n",
    );
    let tasks_before = concat!(
        "- [ ] #task Next root ^root-next\n",
        "  - ![[#^next-ready]]\n",
        "  - ![[#^stronger]]\n",
        "- [ ] #task Next child ^next-ready\n",
        "- [/] #task Stronger intermediate ^stronger\n",
        "  - ![[#^stronger-child]]\n",
        "- [ ] #task Stronger descendant ^stronger-child\n",
        "- [/] #task Working root ^root-working\n",
        "  - ![[#^working-ready]]\n",
        "  - ![[#^working-next]]\n",
        "  - ![[#^done]]\n",
        "  - ![[#^cancelled]]\n",
        "  - ![[#^custom]]\n",
        "- [ ] #task Working ready child ^working-ready\n",
        "- [*] #task Working next child ^working-next\n",
        "- [x] #task Done child ^done\n",
        "- [-] #task Cancelled child ^cancelled\n",
        "- [!] #task Custom child ^custom\n",
        "- [*] #task Unreachable next ^orphan\n",
    );
    write_file(&daily, daily_before);
    write_file(&tasks, tasks_before);

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run ranked dependency propagation");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("ranked propagation dry-run JSON");
    assert_eq!(json["dependency_references"], 8);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 2);
    assert_eq!(json["marked_in_progress"].as_array().unwrap().len(), 3);
    assert_eq!(json["cleared"].as_array().unwrap().len(), 1);
    assert!(json["marked_in_progress"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["block_id"] == "stronger-child"
            && item["dependency"] == true));
    assert!(json["marked_in_progress"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["block_id"] == "working-next"
            && item["dependency"] == true));

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply ranked dependency propagation");
    assert_success(&applied);
    let report = stdout(&applied);
    assert!(
        report.contains("marked in progress")
            && report.contains("[ ] or [*] -> [/]")
            && report.contains(
                "Summary: 2 marked next, 3 marked in progress, 1 cleared"
            ),
        "unexpected ranked propagation report:\n{}",
        format_output(&applied)
    );
    let contents = fs::read_to_string(&tasks).unwrap();
    for expected in [
        "- [*] #task Next root ^root-next",
        "- [*] #task Next child ^next-ready",
        "- [/] #task Stronger intermediate ^stronger",
        "- [/] #task Stronger descendant ^stronger-child",
        "- [/] #task Working root ^root-working",
        "- [/] #task Working ready child ^working-ready",
        "- [/] #task Working next child ^working-next",
        "- [x] #task Done child ^done",
        "- [-] #task Cancelled child ^cancelled",
        "- [!] #task Custom child ^custom",
        "- [ ] #task Unreachable next ^orphan",
    ] {
        assert!(
            contents.contains(expected),
            "missing {expected}:\n{contents}"
        );
    }

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun ranked dependency propagation");
    assert_success(&second);
    assert!(stdout(&second).contains("already in sync, no changes"));

    write_file(
        &daily,
        "# Daily\n\n## Pomodoros\n\n- [ ] Current (0900-0930)\n",
    );
    let without_active_path = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("clear unreachable next while preserving in-progress tasks");
    assert_success(&without_active_path);
    let contents = fs::read_to_string(&tasks).unwrap();
    assert!(contents.contains("- [ ] #task Next root ^root-next"));
    assert!(contents.contains("- [ ] #task Next child ^next-ready"));
    assert!(
        contents.contains("- [/] #task Stronger descendant ^stronger-child")
    );
    assert!(contents.contains("- [/] #task Working next child ^working-next"));
}

#[test]
fn task_status_hooks_prunes_duplicate_lines_before_dependency_sync() {
    let temp = TempDir::new("bob-cli-task-status-hooks-prune-duplicates");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260713.md");
    let tasks = vault.join("tasks.md");
    let daily_before = concat!(
        "# Daily\n\n",
        "## Pomodoros\n\n",
        "- [ ] First owner (0900-0930)\n",
        "  - [[tasks#^alpha]]\n",
        "- [ ] Later owner\n",
        "  - authored [[tasks#^alpha|duplicate]] and ![[tasks#^beta]]\n",
        "    - retained child\n\n",
        "## Tasks\n\n",
        "- [*] #task Daily stale task ^daily-stale\n",
    );
    let tasks_before = concat!(
        "- [ ] #task Alpha ^alpha\n",
        "- [*] #task Beta ^beta\n",
        "  - ![[#^beta-dep]]\n",
        "- [*] #task Beta dependency ^beta-dep\n",
    );
    write_file(&daily, daily_before);
    write_file(&tasks, tasks_before);

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run duplicate Pomodoro cleanup");
    assert_success(&dry_run);
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("duplicate cleanup dry-run JSON");
    assert_eq!(json["references"], 2);
    assert_eq!(json["dependency_references"], 0);
    assert_eq!(json["marked_next"].as_array().unwrap().len(), 1);
    assert_eq!(json["cleared"].as_array().unwrap().len(), 3);
    assert_eq!(json["removed_duplicate_lines"].as_array().unwrap().len(), 1);
    assert_eq!(json["removed_duplicate_lines"][0]["line_number"], 8);
    assert_eq!(
        json["removed_duplicate_lines"][0]["duplicate_tasks"][0],
        serde_json::json!({"path": "tasks.md", "block_id": "alpha"})
    );

    let human_dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("human dry-run duplicate Pomodoro cleanup");
    assert_success(&human_dry_run);
    assert!(
        stdout(&human_dry_run)
            .contains("would remove duplicate task-link lines"),
        "unexpected duplicate cleanup dry-run report:\n{}",
        format_output(&human_dry_run)
    );
    assert_eq!(fs::read_to_string(&daily).unwrap(), daily_before);
    assert_eq!(fs::read_to_string(&tasks).unwrap(), tasks_before);

    let applied = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("apply duplicate Pomodoro cleanup");
    assert_success(&applied);
    assert!(
        stdout(&applied).contains("removed duplicate task-link lines")
            && stdout(&applied).contains("1 duplicate-line removals"),
        "unexpected duplicate cleanup report:\n{}",
        format_output(&applied)
    );
    assert_eq!(
        fs::read_to_string(&daily).unwrap(),
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] First owner (0900-0930)\n",
            "  - [[tasks#^alpha]]\n",
            "- [ ] Later owner\n",
            "    - retained child\n\n",
            "## Tasks\n\n",
            "- [ ] #task Daily stale task ^daily-stale\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        concat!(
            "- [*] #task Alpha ^alpha\n",
            "- [ ] #task Beta ^beta\n",
            "  - ![[#^beta-dep]]\n",
            "- [ ] #task Beta dependency ^beta-dep\n",
        )
    );

    let second = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("rerun duplicate Pomodoro cleanup");
    assert_success(&second);
    assert!(
        stdout(&second).contains("already in sync, no changes"),
        "expected duplicate cleanup rerun to be a no-op:\n{}",
        format_output(&second)
    );
}

#[test]
fn task_status_hooks_reports_plan_budget_in_json_and_human() {
    let temp = TempDir::new("bob-cli-task-status-hooks-plan-budget");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    write_file(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] () — GOALS\n",
            "  - [[tasks#^one]]\n",
            "  - [[tasks#^two]]\n",
            "- [ ] () — DECKS\n",
            "  - [[tasks#^three]]\n",
        ),
    );
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task One ^one\n",
            "- [ ] #task Two ^two\n",
            "- [ ] #task Three ^three\n",
        ),
    );

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run plan budget JSON");
    assert_success(&dry_run);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("plan budget dry-run JSON");
    assert_eq!(
        json["plan_budget"]["themes"],
        serde_json::json!({"count": 2, "cap": 3, "over": false})
    );
    assert_eq!(
        json["plan_budget"]["links"],
        serde_json::json!({"count": 3, "cap": 10, "over": false})
    );
    assert_eq!(json["plan_budget"]["now"]["count"], 0);
    assert_eq!(json["plan_budget"]["now"]["cap"], 15);
    assert_eq!(json["plan_budget"]["status"], "ok");

    let human = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("dry-run plan budget human output");
    assert_success(&human);
    assert!(
        stdout(&human).contains("plan 2/3 themes · 3/10 links · NOW 0/15"),
        "expected a plan budget stats line:\n{}",
        format_output(&human)
    );
}

#[test]
fn task_status_hooks_nulls_plan_budget_on_invalid_config() {
    let temp = TempDir::new("bob-cli-task-status-hooks-plan-invalid");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    write_file(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] () — GOALS\n",
            "  - [[tasks#^one]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task One ^one\n");
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_themes: 0\n");

    let dry_run = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("dry-run with invalid plan config");
    assert_success(&dry_run);
    let json: serde_json::Value = serde_json::from_str(stdout(&dry_run).trim())
        .expect("invalid-config dry-run JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["plan_budget"], serde_json::Value::Null);
    assert!(
        stderr(&dry_run).contains("invalid plan config"),
        "expected a single invalid-config warning:\n{}",
        format_output(&dry_run)
    );
}
