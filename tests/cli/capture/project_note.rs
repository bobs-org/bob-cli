//! Project note tests.

use super::priority::write_priority_config;
use crate::support::*;
use std::fs;

#[test]
fn capture_project_note_creates_plain_note_with_json_and_human_output() {
    let temp = TempDir::new("bob-cli-capture-project-note-plain");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n# Cash\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^goog-exit+")
        .arg("Finish the Google exit packet!")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run plain project-note capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "project_note");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["route_label"], "cash_goog_exit.md");
    assert_eq!(json["relative_target"], "cash_goog_exit.md");
    assert_eq!(
        json["target"],
        vault.join("cash_goog_exit.md").display().to_string()
    );
    assert_eq!(json["placement"], "created");
    assert_eq!(json["block_id"], "prj");
    assert_eq!(
        json["task_line"],
        "- [ ] #task #prj Finish the Google exit packet! #hide ^prj"
    );
    assert_eq!(json["project_note"]["basename"], "cash_goog_exit.md");
    assert_eq!(json["project_note"]["parent_route"], "cash");
    assert_eq!(json["project_note"]["parent_link"], "[[cash]]");
    assert_eq!(json["project_note"]["tasks"], 1);
    assert!(json["project_note"]["sections"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json.get("sub_bullets").is_none(), "{json}");
    assert!(json.get("day_file").is_none(), "{json}");

    let contents = fs::read_to_string(vault.join("cash_goog_exit.md"))
        .expect("read new project note");
    assert!(contents.contains("parent: \"[[cash]]\""), "{contents}");
    assert!(
        contents.contains("template: \"[[new_project]]\""),
        "{contents}"
    );
    assert!(contents.contains("type: \"[[project]]\""), "{contents}");
    assert!(contents.contains("status: wip"), "{contents}");
    assert!(
        contents.contains("created: 2026-09-20T14:31:07"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [ ] #task #prj Finish the Google exit packet! #hide ^prj"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::2026-09-20]"
        ),
        "{contents}"
    );

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@cash^second-note+")
        .arg("Second body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run human project-note capture");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("captured") && out.contains("cash_second_note.md"),
        "{out}"
    );
    assert!(out.contains("[[cash]]"), "{out}");
    assert!(out.contains("^prj"), "{out}");
    assert!(out.contains("projects sync"), "{out}");
    assert!(out.contains("task-status-hooks"), "{out}");
}

#[test]
fn capture_project_note_retired_colon_forms_fail_with_teaching_errors() {
    for (name, marker, teaching) in [
        ("implicit", "@cash:impl1+", "@cash^impl1+"),
        ("named", "@cash:open1+#bugs", "@cash^open1+#bugs"),
    ] {
        let temp =
            TempDir::new(&format!("bob-cli-capture-project-note-{name}"));
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
        let day_before = "## Pomodoros\n- [ ] (1330-1400 [t:: 30m]) Work\n";
        write_file(&day_file, day_before);

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg(marker)
            .arg("Body")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0")
            .output()
            .expect("run retired project-note capture");

        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
        assert!(
            json["error"].as_str().is_some_and(|error| {
                error.contains("is retired")
                    && error.contains(teaching)
                    && error.contains(" :<id>")
            }),
            "{json}"
        );
        // Nothing is written: no project note and no daily-note edit.
        assert!(!vault.join("cash_impl1.md").exists());
        assert!(!vault.join("cash_open1.md").exists());
        assert_eq!(
            fs::read_to_string(&day_file).expect("read daily note"),
            day_before
        );
    }
}

#[test]
fn capture_project_note_named_caret_form_rejects_the_unused_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-project-note-unused");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^goog-exit+#bugs")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run named project-note capture");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(|error| error
            .contains("`#bugs` picks the Pomodoro for ` :<id>` task links")),
        "{json}"
    );
    assert!(!vault.join("cash_goog_exit.md").exists());
}

#[test]
fn capture_project_note_renders_authored_tasks_sections_and_tasks_merge() {
    let temp = TempDir::new("bob-cli-capture-project-note-authored");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");

    let input = "Parent @cash^auth1+\n- Call Morgan Stanley\n- FUTURE WORK\n  - nested detail\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["project_note"]["tasks"], 1, "{json}");
    assert_eq!(
        json["project_note"]["sections"],
        serde_json::json!(["Future Work"]),
        "{json}"
    );
    let contents = fs::read_to_string(vault.join("cash_auth1.md"))
        .expect("read authored note");
    assert!(
        contents
            .contains("- [ ] #task Call Morgan Stanley [created::2026-09-20]"),
        "{contents}"
    );
    assert!(contents.contains("## Future Work"), "{contents}");
    assert!(contents.contains("- nested detail"), "{contents}");

    let tasks_input =
        "Parent @cash^tasks1+\n- TASKS\n  - merged note\n- Real task\n";
    let tasks_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        tasks_input,
    );
    assert_success(&tasks_output);
    let tasks_json: serde_json::Value =
        serde_json::from_str(stdout(&tasks_output).trim())
            .expect("tasks merge JSON");
    assert!(
        tasks_json["project_note"]["sections"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{tasks_json}"
    );
    let tasks_contents = fs::read_to_string(vault.join("cash_tasks1.md"))
        .expect("read tasks merge note");
    assert_eq!(
        tasks_contents.matches("## Tasks").count(),
        1,
        "{tasks_contents}"
    );
    assert!(tasks_contents.contains("- merged note"), "{tasks_contents}");
    assert!(
        tasks_contents.contains("- [ ] #task Real task"),
        "{tasks_contents}"
    );
}

#[test]
fn capture_project_note_schedule_and_priority_write_frontmatter_and_log() {
    let temp = TempDir::new("bob-cli-capture-project-note-schedule");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");

    let scheduled = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^sched1+")
        .arg("Body")
        .arg("s:2")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run scheduled project-note capture");
    assert_success(&scheduled);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&scheduled).trim())
            .expect("scheduled JSON");
    assert_eq!(json["scheduled"], "2026-09-22", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().starts_with("- [?] "),
        "{json}"
    );
    let contents = fs::read_to_string(vault.join("cash_sched1.md"))
        .expect("read scheduled note");
    assert!(contents.contains("scheduled: 2026-09-22"), "{contents}");

    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let prioritized = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^prio1+")
        .arg("Body")
        .arg("p:4")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run prioritized project-note capture");
    assert_success(&prioritized);
    let prio_json: serde_json::Value =
        serde_json::from_str(stdout(&prioritized).trim())
            .expect("priority JSON");
    assert_eq!(prio_json["priority"], "lowest", "{prio_json}");
    assert!(prio_json["scheduled"].as_str().is_some(), "{prio_json}");
    assert!(prio_json["schedule_log"].is_object(), "{prio_json}");
    let prio_contents = fs::read_to_string(vault.join("cash_prio1.md"))
        .expect("read priority note");
    assert!(
        prio_contents.contains("[priority::lowest] #hide ^prj"),
        "{prio_contents}"
    );
    assert!(
        prio_contents.contains("🗓️ **SCHEDULE LOG**"),
        "{prio_contents}"
    );
    assert!(
        prio_contents.contains(&format!(
            "scheduled: {}",
            prio_json["scheduled"].as_str().unwrap()
        )),
        "{prio_contents}"
    );
}

#[test]
fn capture_project_note_rejections_leave_no_partial_write() {
    let missing_parent = TempDir::new("bob-cli-project-note-missing-parent");
    let vault = missing_parent.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^x1+")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run missing parent capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("note does not exist")
                && error.contains("bob capture-targets")),
        "{json}"
    );

    let not_area = TempDir::new("bob-cli-project-note-not-area");
    let vault = not_area.path().join("vault");
    write_file(&vault.join("cash.md"), "# plain\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^x1+")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run non-area capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("not an area or project")),
        "{json}"
    );

    let terminal = TempDir::new("bob-cli-project-note-terminal");
    let vault = terminal.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[project]]\"\nstatus: done\n---\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^x1+")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run terminal parent capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("is a done project")),
        "{json}"
    );

    let collision = TempDir::new("bob-cli-project-note-collision");
    let vault = collision.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&vault.join("cash_x1.md"), "existing\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^x1+")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run collision capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("project note already exists")),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash_x1.md")).expect("read collision"),
        "existing\n"
    );

    let clip = TempDir::new("bob-cli-project-note-clip");
    let vault = clip.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^x1+")
        .arg("Body")
        .arg("%")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run clip capture");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(|error| error.contains(
            "project-note capture cannot be combined with % clipboard markers"
        )),
        "{json}"
    );
    assert!(!vault.join("cash_x1.md").exists());

    let forced_clip = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip")
        .arg("--")
        .arg("@cash^x1+")
        .arg("Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run forced clip capture");
    assert_eq!(
        forced_clip.status.code(),
        Some(2),
        "{}",
        format_output(&forced_clip)
    );
    assert!(
        stderr(&forced_clip)
            .contains("project-note capture cannot be combined with --clip"),
        "{}",
        format_output(&forced_clip)
    );

    // The retired `:` form never reaches the ledger: it fails before any
    // write, even when the ledger already mentions the `^prj` link.
    let ledger = TempDir::new("bob-cli-project-note-ledger");
    let vault = ledger.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before =
        "## Pomodoros\n- [ ] (1330-1400) Work\n  - [[cash_dup#^prj]]\n";
    write_file(&day_file, day_before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:dup+")
        .arg("Body")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run retired ledger capture");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("is retired")),
        "{json}"
    );
    assert!(!vault.join("cash_dup.md").exists());
    assert_eq!(
        fs::read_to_string(&day_file).expect("read daily note"),
        day_before
    );

    let forced = TempDir::new("bob-cli-project-note-forced-literal");
    let vault = forced.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--")
        .arg("@cash^lit1+ Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run forced route capture");
    assert_success(&output);
    assert!(!vault.join("cash_lit1.md").exists());
    let routed = fs::read_to_string(vault.join("cash.md")).expect("read cash");
    assert!(routed.contains("@cash^lit1+"), "{routed}");
}

#[test]
fn capture_project_note_dry_run_and_batch_parenting_and_rollback() {
    let temp = TempDir::new("bob-cli-capture-project-note-dry-run");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let preview = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("@cash^dry1+")
        .arg("Preview body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run dry-run capture");
    assert_success(&preview);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["kind"], "project_note");
    assert!(!vault.join("cash_dry1.md").exists());

    let batch = TempDir::new("bob-cli-capture-project-note-batch");
    let vault = batch.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let input = "First @cash^first1+\n\nSecond @cash_first1^second1+\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    assert_eq!(json["captures"].as_array().unwrap().len(), 2, "{json}");
    assert!(vault.join("cash_first1.md").exists());
    assert!(vault.join("cash_first1_second1.md").exists());
    let child = fs::read_to_string(vault.join("cash_first1_second1.md"))
        .expect("read child note");
    assert!(child.contains("parent: \"[[cash_first1]]\""), "{child}");

    let rollback = TempDir::new("bob-cli-capture-project-note-rollback");
    let vault = rollback.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let failing = "First @cash^ok1+\n\nSecond @cash^ok1+\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        failing,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        !vault.join("cash_ok1.md").exists(),
        "no partial note should survive a failing batch"
    );
}

#[test]
fn capture_project_note_task_links_write_the_worked_example() {
    let temp = TempDir::new("bob-cli-capture-project-note-links");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");

    let input = "Finish the Google exit packet! @cash^goog-exit+#admin\n\
         - Draft the resignation memo :draft-memo\n  - keep it short\n\
         - Call Morgan Stanley about the 401k :call-ms\n\
         - Collect the equity paperwork ^equity-docs\n\
         - FUTURE WORK\n  - Revisit the severance terms\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "project_note");
    assert_eq!(json["block_id"], "prj");
    assert!(json.get("block_link").is_none(), "{json}");
    assert_eq!(
        json["project_note"]["task_links"],
        serde_json::json!([
            {
                "block_id": "draft-memo",
                "block_link": "[[cash_goog_exit#^draft-memo]]",
                "text": "Draft the resignation memo",
                "task_line": "- [*] #task Draft the resignation memo [created::2026-09-20] ^draft-memo",
            },
            {
                "block_id": "call-ms",
                "block_link": "[[cash_goog_exit#^call-ms]]",
                "text": "Call Morgan Stanley about the 401k",
                "task_line": "- [*] #task Call Morgan Stanley about the 401k [created::2026-09-20] ^call-ms",
            },
        ]),
        "{json}"
    );
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert_eq!(json["pomodoro_name"], "ADMIN");
    assert_eq!(json["creates_pomodoro"], true);
    assert!(json["pomodoro_link_placement"].is_string(), "{json}");

    let contents = fs::read_to_string(vault.join("cash_goog_exit.md"))
        .expect("read new project note");
    assert!(
        contents.contains(
            "- [*] #task Draft the resignation memo [created::2026-09-20] ^draft-memo\n\t- keep it short\n"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [*] #task Call Morgan Stanley about the 401k [created::2026-09-20] ^call-ms\n"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [ ] #task Collect the equity paperwork [created::2026-09-20] ^equity-docs\n"
        ),
        "{contents}"
    );
    assert!(
        contents
            .contains("\n## Future Work\n\n- Revisit the severance terms\n"),
        "{contents}"
    );

    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert!(day_after.contains("- [ ] () — ADMIN"), "{day_after}");
    let admin = day_after.find("- [ ] () — ADMIN").expect("ADMIN entry");
    let first = day_after
        .find("[[cash_goog_exit#^draft-memo]]")
        .expect("first link");
    let second = day_after
        .find("[[cash_goog_exit#^call-ms]]")
        .expect("second link");
    assert!(admin < first && first < second, "{day_after}");
    assert!(!day_after.contains("^equity-docs"), "{day_after}");

    let human_temp = TempDir::new("bob-cli-capture-project-note-human");
    let human_vault = human_temp.path().join("vault");
    let human_day = human_vault.join("day.md");
    write_file(
        &human_vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n",
    );
    write_file(&human_day, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let human = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&human_vault)
            .env("BOB_DAY_FILE", &human_day)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Body @cash^human1+#admin\n- Draft the memo :draft-memo\n",
    );
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("linked") && out.contains("day.md"), "{out}");
    assert!(out.contains("under ADMIN (created)"), "{out}");
    assert!(out.contains("[[cash_human1#^draft-memo]]"), "{out}");
    assert!(out.contains("projects sync"), "{out}");
}

#[test]
fn capture_project_note_task_links_use_the_implicit_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-project-note-implicit");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");

    let input =
        "Body @cash^impl1+\n- Draft the memo :draft-memo\n- File it ^file-it\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["project_note"]["task_links"].as_array().unwrap().len(),
        1,
        "{json}"
    );
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert!(json.get("pomodoro_name").is_none(), "{json}");
    assert_eq!(json["creates_pomodoro"], false);
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert!(
        day_after.contains("[[cash_impl1#^draft-memo]]"),
        "{day_after}"
    );
    assert!(!day_after.contains("() — "), "{day_after}");

    // An existing named Pomodoro is reused instead of created.
    let named = TempDir::new("bob-cli-capture-project-note-existing");
    let vault = named.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (1330-1400) Work\n- [ ] () — ADMIN\n",
    );
    let input = "Body @cash^exist1+#admin\n- Draft the memo :draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["pomodoro_name"], "ADMIN");
    assert_eq!(json["creates_pomodoro"], false);
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(day_after.matches("— ADMIN").count(), 1, "{day_after}");
    assert!(
        day_after.contains("[[cash_exist1#^draft-memo]]"),
        "{day_after}"
    );
}

#[test]
fn capture_project_note_task_links_dry_run_batch_and_rollback() {
    // Dry run reports the same JSON but writes nothing.
    let temp = TempDir::new("bob-cli-capture-project-note-link-dry");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400) Work\n";
    write_file(&day_file, day_before);
    let input = "Body @cash^drylink1+\n- Draft the memo :draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-d")
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(
        json["project_note"]["task_links"].as_array().unwrap().len(),
        1,
        "{json}"
    );
    assert!(!vault.join("cash_drylink1.md").exists());
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);

    // A batch appends the second item's links after the first item's.
    let batch = TempDir::new("bob-cli-capture-project-note-link-batch");
    let vault = batch.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let input = "First @cash^batch1+\n- Alpha task :alpha\n\nSecond @cash^batch2+\n- Beta task :beta\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    let first = day_after
        .find("[[cash_batch1#^alpha]]")
        .expect("first batch link");
    let second = day_after
        .find("[[cash_batch2#^beta]]")
        .expect("second batch link");
    assert!(first < second, "{day_after}");

    // A duplicate ledger link rolls the whole batch back.
    let rollback = TempDir::new("bob-cli-capture-project-note-link-dup");
    let vault = rollback.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before =
        "## Pomodoros\n- [ ] (1330-1400) Work\n  - [[cash_dup2#^dup]]\n";
    write_file(&day_file, day_before);
    let input = "Body @cash^dup2+\n- Draft the memo :dup\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(|error| error
            .contains("Pomodoro ledger already contains [[cash_dup2#^dup]]")),
        "{json}"
    );
    assert!(!vault.join("cash_dup2.md").exists());
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);
}

#[test]
fn capture_project_note_task_links_need_the_day_file_only_for_links() {
    // A ` :` task without a daily note fails before writing anything.
    let missing = TempDir::new("bob-cli-capture-project-note-link-noday");
    let vault = missing.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let input = "Body @cash^noday1+\n- Draft the memo :draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(
            |error| error.contains("Bob daily note does not exist")
        ),
        "{json}"
    );
    assert!(!vault.join("cash_noday1.md").exists());

    // `^`-only tasks never read the daily note, even when it is missing.
    let caret = TempDir::new("bob-cli-capture-project-note-caret-noday");
    let vault = caret.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let input = "Body @cash^caretn1+\n- Draft the memo ^draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert!(
        json["project_note"]["task_links"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{json}"
    );
    assert!(json.get("day_file").is_none(), "{json}");

    // `^`-only tasks leave an existing daily note untouched.
    let untouched = TempDir::new("bob-cli-capture-project-note-caret-day");
    let vault = untouched.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400) Work\n";
    write_file(&day_file, day_before);
    let input = "Body @cash^caretd1+\n- Draft the memo ^draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);

    // A scheduled project still links its `:` tasks as `[?]`.
    let scheduled = TempDir::new("bob-cli-capture-project-note-link-sched");
    let vault = scheduled.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let input = "Body @cash^schedlink1+ s:2\n- Draft the memo :draft-memo\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("scheduled JSON");
    assert_eq!(json["scheduled"], "2026-09-22", "{json}");
    assert!(
        json["project_note"]["task_links"][0]["task_line"]
            .as_str()
            .unwrap()
            .starts_with("- [?] "),
        "{json}"
    );
    let contents = fs::read_to_string(vault.join("cash_schedlink1.md"))
        .expect("read scheduled note");
    assert!(
        contents.contains(
            "- [?] #task Draft the memo [created::2026-09-20] ^draft-memo"
        ),
        "{contents}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("[[cash_schedlink1#^draft-memo]]"),
        "{day_after}"
    );
}
