//! Projects list tests.

use crate::support::*;
use serde_json::Value;

const PROJECTS_LIST_NOW: &str = "2026-11-13 12:00:00";

#[test]
fn projects_list_scans_project_notes_and_renders_counts() {
    let temp = TempDir::new("bob-cli-projects-list");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Alpha.md"),
        r#"---
type: "[[project]]"
status: wip
---
- [ ] #task Finish Alpha #hide ^prj
- [ ] #task shown one
- [/] #task shown in progress
- [*] #task shown next
- [?] #task dependency blocked
- [ ] #task future scheduled [scheduled::2999-01-01]
- [ ] #task hidden helper #hide
- [x] #task done task
- [-] #task canceled task
"#,
    );
    write_file(
        &vault.join("Beta.md"),
        r#"---
type: [[project]]
status: waiting
---
- [ ] #task Finish Beta [scheduled::2026-06-11] ^prj
- [ ] #task planned #hide
"#,
    );
    write_file(
        &vault.join("Done.md"),
        r#"---
type: [[project]]
status: done
---
- [X] #task Finish Done #hide ^prj
"#,
    );
    write_file(
        &vault.join("Canceled.md"),
        r#"---
type: [[project]]
status: canceled
---
- [-] #task Cancel Canceled #hide ^prj
"#,
    );
    write_file(
        &vault.join("Missing.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task Needs prj
"#,
    );
    write_file(
        &vault.join("Placeholder.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task <short_project_completion_criteria_goes_here> #hide ^prj
"#,
    );
    write_file(
        &vault.join("_templates/Template.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join(".obsidian/Hidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join("done/Archived.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects list");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected stderr:\n{}",
        stderr(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Projects - 3 active - 1 waiting - 1 done - 1 canceled"),
        "unexpected summary:\n{out}"
    );
    assert!(
        out.contains("PROJECT")
            && out.contains("STATUS")
            && out.contains("SHOWN")
            && out.contains("^PRJ"),
        "missing table header:\n{out}"
    );
    assert!(out.contains("Alpha") && out.contains("wip"));
    assert!(
        out.contains("   7      3  open"),
        "unexpected Alpha counts:\n{out}"
    );
    assert!(
        out.contains("Beta") && out.contains("on dash"),
        "missing on-dash Beta row:\n{out}"
    );
    assert!(
        !out.contains("scheduled 2026-06-11"),
        "scheduled field should not render in ^PRJ column:\n{out}"
    );
    assert!(out.contains("Done") && out.contains("done"));
    assert!(out.contains("Canceled") && out.contains("canceled"));
    assert!(out.contains("Missing") && out.contains("missing"));
    assert!(out.contains("Placeholder") && out.contains("placeholder"));
    assert!(
        !out.contains("Template")
            && !out.contains("Hidden")
            && !out.contains("Archived"),
        "excluded directories should not be listed:\n{out}"
    );
    assert_text_order(
        &out,
        &[
            "Alpha",
            "Missing",
            "Placeholder",
            "Beta",
            "Done",
            "Canceled",
        ],
    );
}

#[test]
fn projects_list_reports_prj_errors_without_aborting_scan() {
    let temp = TempDir::new("bob-cli-projects-errors");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Good project #hide ^prj\n",
    );
    write_file(
        &vault.join("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete malformed project ^prj\n",
    );
    write_file(
        &vault.join("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects list with errors");

    assert_eq!(
        output.status.code(),
        Some(1),
        "project scan errors should exit 1:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("Good")
            && out.contains("Malformed")
            && out.contains("Multiple"),
        "list should still render every project row:\n{out}"
    );
    let err = stderr(&output);
    assert!(
        err.contains("Malformed.md:4: malformed ^prj task")
            && err.contains("Multiple.md:5: multiple ^prj tasks"),
        "expected per-file project errors:\n{err}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn projects_list_reports_invalid_and_duplicate_inline_prj_scheduled() {
    let temp = TempDir::new("bob-cli-projects-list-prj-scheduled");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Invalid.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship [scheduled:: 2026-02-30] ^prj\n",
    );
    write_file(
        &vault.join("Duplicate.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship [scheduled:: 2026-07-11] [scheduled:: 2026-07-12] ^prj\n",
    );
    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Ship [scheduled:: 2026-07-11] ^prj\n",
    );

    let output = bob_command()
        .args(["projects", "list", "-b"])
        .arg(&vault)
        .output()
        .expect("list inline ^prj scheduled issues");
    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid inline ^prj scheduled should exit 1:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("Invalid")
            && out.contains("Duplicate")
            && out.contains("Good"),
        "list should still render every project row:\n{out}"
    );
    let err = stderr(&output);
    assert!(
        err.contains(
            "Invalid.md:4: ^prj scheduled is not a valid calendar date: 2026-02-30"
        ) && err.contains(
            "Duplicate.md:4: multiple ^prj scheduled fields found; keep exactly one"
        ),
        "expected inline ^prj scheduled errors:\n{err}"
    );
}

#[test]
fn projects_list_ignores_leftover_rollup_key() {
    let temp = TempDir::new("bob-cli-projects-rollup-retired");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Opted.md"),
        "---\ntype: [[project]]\nrollup: subprojects\n---\n- [ ] #task #prj Finish opted #hide ^prj\n",
    );
    write_file(
        &vault.join("Invalid.md"),
        "---\ntype: [[project]]\nrollup: yes please\n---\n- [ ] #task #prj Finish invalid #hide ^prj\n",
    );

    let output = bob_command()
        .args(["projects", "list", "-b"])
        .arg(&vault)
        .output()
        .expect("run bob projects list with leftover rollup key");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        !out.contains("rollup"),
        "leftover rollup key must not render:\n{out}"
    );
    assert!(
        stderr(&output).is_empty(),
        "leftover rollup key must not warn:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_list_json_reports_projects_catalogs_findings_and_today() {
    let temp = TempDir::new("bob-cli-projects-list-json");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);

    write_file(
        &vault.join("Current.md"),
        "---\ntype: [[area]]\n---\n\n## Projects\n\n> [!projects]\n>\n> - **[[OpenChild]]** — Finish open work\n",
    );
    write_file(&vault.join("Missing.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("Stale.md"),
        "---\ntype: [[area]]\n---\n\n## Projects\n\n> [!projects]\n> - **[[Wrong]]**\n",
    );
    write_file(
        &vault.join("Damaged.md"),
        "---\ntype: [[area]]\n---\n\n## Projects\n\n> [!projects\n",
    );

    write_file(
        &vault.join("OpenChild.md"),
        "---\ntype: [[project]]\nparent: [[Current]]\nstatus: wip\n---\n- [ ] #task #prj Finish open work #hide ^prj\n- [ ] #task Assigned work ^open-task\n",
    );
    write_file(
        &vault.join("ScheduledChild.md"),
        "---\ntype: [[project]]\nparent: [[Missing]]\nstatus: wip\n---\n- [ ] #task #prj Finish later [scheduled:: 2026-11-20] #hide ^prj\n",
    );
    write_file(
        &vault.join("BlockedChild.md"),
        "---\ntype: [[project]]\nparent: [[Stale]]\nstatus: wip\n---\n- [?] #task #prj Wait for prerequisite #hide ^prj\n  - ⛓️ **DEPENDS ON:** [[Prereq#^need]]\n",
    );
    write_file(
        &vault.join("InheritedChild.md"),
        "---\ntype: [[project]]\nparent: [[Stale]]\nstatus: wip\n---\n- [?] #task #prj Inherited blocker #prj_is_blocked #hide ^prj\n",
    );
    write_file(
        &vault.join("WarningChild.md"),
        "---\ntype: [[project]]\nparent: [[Damaged]]\nstatus: wip\n---\n- [ ] #task ordinary task\n",
    );
    write_file(
        &vault.join("DoneChild.md"),
        "---\ntype: [[project]]\nparent: [[Missing]]\nstatus: done\n---\n- [x] #task #prj Completed child [completion:: 2026-11-12] ^prj\n",
    );
    write_file(
        &vault.join("CanceledChild.md"),
        "---\ntype: [[project]]\nparent: [[Missing]]\nstatus: canceled\n---\n- [-] #task #prj Canceled child [cancelled:: 2026-11-11] ^prj\n",
    );
    write_file(
        &vault.join("Prereq.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Prerequisite ^need\n",
    );
    let daily = vault.join("2026/20261113.md");
    write_file(
        &daily,
        "# 2026-11-13\n\n## Pomodoros\n\n- [ ] () — FOCUS\n  - [[OpenChild#^open-task]]\n",
    );

    let output = bob_command()
        .args(["projects", "list", "-f", "json", "-b"])
        .arg(&vault)
        .env("BOB_NOW", PROJECTS_LIST_NOW)
        .env("BOB_DAY_FILE", &daily)
        .output()
        .expect("run bob projects list JSON");
    assert_eq!(
        output.status.code(),
        Some(1),
        "damaged catalog should make list exit 1:\n{}",
        format_output(&output)
    );
    let value: Value = serde_json::from_slice(&output.stdout)
        .expect("projects list JSON parses");
    assert!(
        stderr(&output).contains("Damaged.md")
            || stderr(&output).contains("Damaged"),
        "damaged catalog diagnostic should be reported:\n{}",
        stderr(&output)
    );
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "list");
    assert_eq!(value["today"], "2026-11-13");
    assert_eq!(value["counts"]["projects"], 7);
    assert_eq!(value["counts"]["catalogs"], 4);
    assert_eq!(value["counts"]["errors"], 1);
    assert_eq!(value["counts"]["warnings"], 2);

    let project = |stem: &str| {
        value["projects"]
            .as_array()
            .expect("projects array")
            .iter()
            .find(|project| project["stem"] == stem)
            .unwrap_or_else(|| {
                panic!("missing {stem} project in {}", value["projects"])
            })
    };
    assert_eq!(project("OpenChild")["state"], "open");
    assert_eq!(project("OpenChild")["parent"], "Current.md");
    assert_eq!(project("OpenChild")["tasks"]["open"], 2);
    assert_eq!(project("OpenChild")["tasks"]["shown"], 1);
    assert_eq!(project("OpenChild")["tasks"]["today"], 1);
    assert_eq!(project("OpenChild")["prj"]["line"], 6);
    assert_eq!(project("OpenChild")["prj"]["mark"], " ");
    assert_eq!(project("ScheduledChild")["state"], "scheduled");
    assert_eq!(
        project("ScheduledChild")["blocked"],
        serde_json::json!({"kind": "scheduled", "date": "2026-11-20"})
    );
    assert_eq!(project("BlockedChild")["state"], "blocked");
    assert_eq!(project("BlockedChild")["blocked"]["kind"], "dependency");
    assert_eq!(
        project("BlockedChild")["blocked"]["waits_on"],
        serde_json::json!(["Prereq#^need"])
    );
    assert_eq!(project("InheritedChild")["state"], "blocked");
    assert_eq!(
        project("InheritedChild")["blocked"],
        serde_json::json!({"kind": "inherited"})
    );
    assert_eq!(project("WarningChild")["state"], "warning");
    assert_eq!(project("WarningChild")["criterion"], "no valid `^prj` task");
    assert_eq!(project("DoneChild")["state"], "done");
    assert_eq!(project("DoneChild")["closed_on"], "2026-11-12");
    assert_eq!(project("CanceledChild")["state"], "canceled");
    assert_eq!(project("CanceledChild")["closed_on"], "2026-11-11");

    let catalogs = value["catalogs"].as_array().expect("catalogs array");
    for (path, state) in [
        ("Current.md", "current"),
        ("Missing.md", "missing"),
        ("Stale.md", "stale"),
        ("Damaged.md", "damaged"),
    ] {
        assert_eq!(
            catalogs
                .iter()
                .find(|catalog| catalog["path"] == path)
                .unwrap()["state"],
            state,
            "catalog state for {path}"
        );
    }
}

#[test]
fn projects_list_json_uses_null_today_when_daily_note_is_missing() {
    let temp = TempDir::new("bob-cli-projects-list-json-no-daily");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Open.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task #prj Finish open work ^prj\n",
    );
    let missing_daily = vault.join("2026/20261113.md");

    let output = bob_command()
        .args(["projects", "list", "--format", "json", "-b"])
        .arg(&vault)
        .env("BOB_NOW", PROJECTS_LIST_NOW)
        .env("BOB_DAY_FILE", missing_daily)
        .output()
        .expect("run projects list without a daily note");
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout)
        .expect("projects list JSON parses");
    assert_eq!(value["projects"][0]["tasks"]["today"], Value::Null);
}

#[test]
fn projects_list_human_output_adds_a_catalog_repair_footer() {
    let temp = TempDir::new("bob-cli-projects-list-footer");
    let vault = temp.path().join("vault");
    write_file(&vault.join("Area.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nparent: [[Area]]\nstatus: wip\n---\n- [ ] #task #prj Finish child ^prj\n",
    );

    let output = bob_command()
        .args(["projects", "list", "-b"])
        .arg(&vault)
        .env("BOB_NOW", PROJECTS_LIST_NOW)
        .output()
        .expect("run human projects list with a missing catalog");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("1 Projects catalogs need `bob projects sync`"),
        "expected one concise repair footer:\n{out}"
    );
    assert!(
        out.contains("PROJECT") && out.contains("Child"),
        "human project table should remain present:\n{out}"
    );
}
