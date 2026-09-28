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
fn capture_project_note_pomodoro_variants_link_the_prj_task() {
    for (name, selector_args, day_before, expect_name, expect_creates) in [
        (
            "implicit",
            vec!["@cash:impl1+", "Body implicit"],
            "## Pomodoros\n- [ ] (1330-1400 [t:: 30m]) Work\n",
            None,
            false,
        ),
        (
            "named-open",
            vec!["@cash:open1+#bugs", "Body open"],
            "## Pomodoros\n- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n- [ ] () — BUGS\n",
            Some("BUGS"),
            false,
        ),
        (
            "named-future",
            vec!["@cash:future1+#newthing", "Body future"],
            "## Pomodoros\n- [ ] (1330-1400) Work\n",
            Some("NEWTHING"),
            true,
        ),
    ] {
        let temp = TempDir::new(&format!("bob-cli-capture-project-note-{name}"));
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
        write_file(&day_file, day_before);

        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        for arg in &selector_args {
            command.arg(arg);
        }
        let output = command
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0")
            .output()
            .expect("run pomodoro project-note capture");

        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["kind"], "project_note", "{json}");
        assert_eq!(json["block_id"], "prj", "{json}");
        assert_eq!(json["day_file"], day_file.display().to_string(), "{json}");
        assert!(
            json["block_link"]
                .as_str()
                .is_some_and(|link| link.ends_with("#^prj]]")),
            "{json}"
        );
        assert!(
            json["pomodoro_link_placement"] == "inserted"
                || json["pomodoro_link_placement"] == "appended",
            "{json}"
        );
        assert_eq!(json["creates_pomodoro"], expect_creates, "{json}");
        match expect_name {
            Some(name) => assert_eq!(json["pomodoro_name"], name, "{json}"),
            None => assert!(json.get("pomodoro_name").is_none(), "{json}"),
        }
        if name == "implicit" {
            assert_eq!(
                json["task_line"], "- [*] #task #prj Body implicit #hide ^prj",
                "{json}"
            );
        }
        let day_after = fs::read_to_string(&day_file).expect("read daily note");
        assert!(
            day_after.contains(json["block_link"].as_str().unwrap()),
            "{day_after}"
        );
    }
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

    let ledger = TempDir::new("bob-cli-project-note-ledger");
    let vault = ledger.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (1330-1400) Work\n  - [[cash_dup#^prj]]\n",
    );
    let _ = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:dup+")
        .arg("Body")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output();
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (1330-1400) Work\n  - [[cash_dup#^prj]]\n",
    );
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
        .expect("run duplicate ledger capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(
            |error| error.contains("Pomodoro ledger already contains")
        ),
        "{json}"
    );
    assert!(!vault.join("cash_dup.md").exists());

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
