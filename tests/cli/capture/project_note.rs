//! Project note tests.

use super::priority::write_priority_config;
use crate::support::*;
use std::{fs, path::Path};

#[test]
fn capture_project_note_offer_reports_unselected_successors_without_linking() {
    // A sealed `@route:id@` draft creates the project note and reports
    // its open tasks as unselected successor offers. Nothing is linked
    // by default: dry-run JSON matches the real run, and the daily note
    // gains no project links.
    let temp = TempDir::new("bob-cli-capture-project-note-offer");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400) Work\n";
    write_file(&day_file, day_before);
    let input = "Finish the exit packet @cash:goog-exit@\n- Draft the memo\n- Collect paperwork\n- Call the bank :call-bank\n- Private #hide reminder\n";

    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        let output = run_with_stdin(&mut command, input);
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    assert!(!vault.join("cash_goog_exit.md").exists());
    assert_eq!(
        fs::read_to_string(&day_file).expect("dry run writes nothing"),
        day_before
    );

    let json: serde_json::Value = run(false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::json!(false);
    assert_eq!(masked, json, "dry-run JSON must equal real-run JSON");

    assert_eq!(json["kind"], "project_note");
    assert_eq!(json["project_note"]["form"], "offer");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("offer successors");
    assert_eq!(successors.len(), 2, "{json}");
    for (index, row) in successors.iter().enumerate() {
        let offer = &row["offer"];
        assert_eq!(offer["selected"], false, "{row}");
        assert_eq!(offer["group"], "project", "{row}");
        assert_eq!(offer["project"], "cash_goog_exit.md", "{row}");
        assert_eq!(offer["n"], index as u64 + 1, "{row}");
        assert_eq!(offer["block_id_needed"], true, "{row}");
        assert_eq!(row["trigger"], "project_created", "{row}");
        assert_eq!(row["previous_status_symbol"], " ", "{row}");
        assert_eq!(row["status_symbol"], " ", "{row}");
        assert_eq!(row["hide_removed"], false, "{row}");
        assert!(row.get("link").is_none(), "{row}");
        assert_eq!(row["unblocked_by"][0]["block_id"], "prj", "{row}");
        assert!(
            !offer["key"].as_str().unwrap_or_default().is_empty(),
            "{row}"
        );
    }
    assert_ne!(
        successors[0]["offer"]["key"], successors[1]["offer"]["key"],
        "offer keys stay distinct"
    );
    // Only the explicit ` :` bullet links.
    assert_eq!(
        json["project_note"]["task_links"].as_array().map(Vec::len),
        Some(1),
        "{json}"
    );
    assert_eq!(
        json["project_note"]["task_links"][0]["block_id"],
        "call-bank"
    );
    let groups = json["pull_in"]["groups"]
        .as_array()
        .expect("pull-in groups");
    assert_eq!(groups.len(), 1, "{json}");
    assert_eq!(groups[0]["folded"]["hidden"], 1, "{json}");
    assert_eq!(groups[0]["folded"]["already_planned"], 1, "{json}");
    assert_eq!(groups[0]["folded"]["sub_projects"], 0, "{json}");

    let note = fs::read_to_string(vault.join("cash_goog_exit.md"))
        .expect("read new project note");
    assert!(note.contains("- [ ] #task Draft the memo"), "{note}");
    assert!(note.contains("- [ ] #task Collect paperwork"), "{note}");
    assert!(!note.contains("[fresh::"), "{note}");
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    // The explicit ` :call-bank` bullet links; the offers do not.
    assert!(
        day_after.contains("[[cash_goog_exit#^call-bank]]"),
        "{day_after}"
    );
    assert!(!day_after.contains("draft-memo"), "{day_after}");
    assert!(!day_after.contains("collect-paperwork"), "{day_after}");
}

#[test]
fn capture_project_note_offer_all_picks_match_authored_colon_bullets() {
    // Invariant 1: Offer `-p all` writes byte-for-byte the same files as
    // Create with the equivalent authored ` :<id>` bullets.
    let make_vault = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
        write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
        (temp, vault, day_file)
    };
    let (_temp_a, vault_a, day_a) = make_vault("bob-cli-offer-all-a");
    let (_temp_b, vault_b, day_b) = make_vault("bob-cli-offer-all-b");
    let offer_input = "Finish the exit packet @cash:goog-exit@\n- Draft the memo\n- Collect paperwork\n";

    let offer_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_a)
            .arg("-f")
            .arg("json")
            .arg("-p")
            .arg("all")
            .env("BOB_DAY_FILE", &day_a)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        offer_input,
    );
    assert_success(&offer_output);
    let offer_json: serde_json::Value =
        serde_json::from_str(stdout(&offer_output).trim()).expect("offer JSON");
    let successors = offer_json["project_note"]["successors"]
        .as_array()
        .expect("picked successors");
    assert_eq!(successors.len(), 2, "{offer_json}");
    let mut minted = Vec::new();
    for row in successors {
        assert_eq!(row["offer"]["selected"], true, "{row}");
        assert_eq!(row["status_symbol"], "*", "{row}");
        assert!(row.get("link").is_some(), "{row}");
        minted.push(row["block_id"].as_str().expect("minted ID").to_string());
    }

    let authored_input = format!(
        "Finish the exit packet @cash^goog-exit@\n- Draft the memo :{}\n- Collect paperwork :{}\n",
        minted[0], minted[1]
    );
    let authored_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_b)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_b)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        &authored_input,
    );
    assert_success(&authored_output);

    assert_eq!(
        fs::read_to_string(vault_a.join("cash_goog_exit.md"))
            .expect("read offer note"),
        fs::read_to_string(vault_b.join("cash_goog_exit.md"))
            .expect("read authored note"),
        "picked rows render exactly like authored ` :` bullets"
    );
    assert_eq!(
        fs::read_to_string(&day_a).expect("read offer day"),
        fs::read_to_string(&day_b).expect("read authored day"),
        "picked links land exactly like authored ` :` links"
    );
}

#[test]
fn capture_project_note_offer_pick_validation_is_atomic() {
    // Unknown and out-of-range picks, and `--require-pull-in-choice`
    // refusals, write nothing: no project note and no daily-note edit.
    let temp = TempDir::new("bob-cli-capture-project-note-offer-atomic");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400) Work\n";
    write_file(&day_file, day_before);
    let input =
        "Finish the exit packet @cash:goog-exit@\n- Draft the memo\n- Collect paperwork\n";

    let run_flags = |flags: &[&str]| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        for flag in flags {
            command.arg(flag);
        }
        command
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        run_with_stdin(&mut command, input)
    };

    // A numeric pick selects one offer; the key pick selects the same row.
    let numbered = run_flags(&["--dry-run", "-p", "2"]);
    assert_success(&numbered);
    let numbered_json: serde_json::Value =
        serde_json::from_str(stdout(&numbered).trim()).expect("numeric JSON");
    let picked: Vec<&serde_json::Value> = numbered_json["project_note"]
        ["successors"]
        .as_array()
        .expect("successors")
        .iter()
        .filter(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(picked.len(), 1, "{numbered_json}");
    assert_eq!(picked[0]["text"], "Collect paperwork", "{numbered_json}");
    let key = numbered_json["project_note"]["successors"][1]["offer"]["key"]
        .as_str()
        .expect("second offer key")
        .to_string();
    let keyed = run_flags(&["--dry-run", "-p", &key]);
    assert_success(&keyed);
    let keyed_json: serde_json::Value =
        serde_json::from_str(stdout(&keyed).trim()).expect("key JSON");
    assert_eq!(
        keyed_json["project_note"]["successors"][1]["offer"]["selected"], true,
        "{keyed_json}"
    );

    for (flags, code) in [
        (vec!["--dry-run", "-p", "nope"], 2),
        (vec!["--dry-run", "-p", "9"], 2),
        (vec!["--dry-run", "--require-pull-in-choice"], 1),
    ] {
        let output = run_flags(&flags);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
        assert_eq!(json["ok"], false, "{json}");
        assert!(!vault.join("cash_goog_exit.md").exists(), "{json}");
        assert_eq!(
            fs::read_to_string(&day_file).expect("refusal writes nothing"),
            day_before,
            "{json}"
        );
    }
    let required = run_flags(&["--dry-run", "--require-pull-in-choice"]);
    let required_json: serde_json::Value =
        serde_json::from_str(stdout(&required).trim()).expect("choice JSON");
    assert!(
        required_json.get("offers").is_some_and(|offers| {
            offers.as_array().is_some_and(|rows| rows.len() == 2)
        }),
        "choice refusal carries the flat offers payload: {required_json}"
    );

    // A later batch item failure rolls the offer back atomically.
    let rollback = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("-p")
            .arg("all")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Finish the exit packet @cash:goog-exit@\n- Draft the memo\n\nSecond @cash:id+",
    );
    assert_eq!(
        rollback.status.code(),
        Some(2),
        "{}",
        format_output(&rollback)
    );
    assert!(!vault.join("cash_goog_exit.md").exists());
    assert_eq!(
        fs::read_to_string(&day_file).expect("rollback writes nothing"),
        day_before
    );
}

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
        .arg("@cash^goog-exit@")
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
    assert!(json["project_note"].get("move_hint").is_none(), "{json}");
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
        .arg("@cash^second-note@")
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
    assert!(out.contains("task reconcile"), "{out}");
}

#[test]
fn capture_project_note_retired_colon_forms_fail_with_teaching_errors() {
    for (name, marker, teaching) in [
        ("implicit", "@cash:impl1+", "@cash^impl1@"),
        ("named", "@cash:open1+#bugs", "@cash^open1@#bugs"),
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
                error.contains("is retired") && error.contains(teaching)
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
        .arg("@cash^goog-exit@#bugs")
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

    let input = "Parent @cash^auth1@\n- Call Morgan Stanley\n- FUTURE WORK\n  - nested detail\n";
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
        "Parent @cash^tasks1@\n- TASKS\n  - merged note\n- Real task\n";
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
fn capture_project_note_schedule_and_priority_write_inline_date_and_log() {
    let temp = TempDir::new("bob-cli-capture-project-note-schedule");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");

    let scheduled = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^sched1@")
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
    assert!(
        contents.contains("[scheduled:: 2026-09-22] #hide ^prj"),
        "{contents}"
    );
    assert!(
        !contents.contains("\nscheduled:"),
        "capture must not write frontmatter scheduled:\n{contents}"
    );

    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let prioritized = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^prio1@")
        .arg("Body")
        .arg("$4")
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
        prio_contents.contains(&format!(
            "[priority::lowest] [scheduled:: {}] #hide ^prj",
            prio_json["scheduled"].as_str().unwrap()
        )),
        "{prio_contents}"
    );
    assert!(
        prio_contents.contains("🗓️ **SCHEDULE LOG**"),
        "{prio_contents}"
    );
    assert!(
        !prio_contents.contains("\nscheduled:"),
        "priority capture must not write frontmatter scheduled:\n{prio_contents}"
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
        .arg("@cash^x1@")
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
        .arg("@cash^x1@")
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
        .arg("@cash^x1@")
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
        .arg("@cash^x1@")
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
        .arg("@cash^x1@")
        .arg("Body")
        .arg("%c")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run clip capture");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert!(
        json["error"].as_str().is_some_and(|error| error
            .contains("project-note capture cannot be combined with %c")),
        "{json}"
    );
    assert!(!vault.join("cash_x1.md").exists());

    let forced_clip = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip")
        .arg("--")
        .arg("@cash^x1@")
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
        .arg("@cash^lit1@ Body")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .output()
        .expect("run forced route capture");
    assert_success(&output);
    assert!(!vault.join("cash_lit1.md").exists());
    let routed = fs::read_to_string(vault.join("cash.md")).expect("read cash");
    assert!(routed.contains("@cash^lit1@"), "{routed}");
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
        .arg("@cash^dry1@")
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
    let input = "First @cash^first1@\n\nSecond @cash_first1^second1@\n";
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
    let failing = "First @cash^ok1@\n\nSecond @cash^ok1@\n";
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

    let input = "Finish the Google exit packet! @cash^goog-exit@#admin\n\
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
    assert_eq!(
        contents,
        "---\n\
         parent: \"[[cash]]\"\n\
         template: \"[[new_project]]\"\n\
         type: \"[[project]]\"\n\
         status: wip\n\
         created: 2026-09-20T14:31:07+0000\n\
         ---\n\
         \n\
         - [ ] #task #prj Finish the Google exit packet! #hide ^prj\n\
         \n\
         ## Tasks\n\
         \n\
         - [*] #task Draft the resignation memo [created::2026-09-20] ^draft-memo\n\
         \t- keep it short\n\
         - [*] #task Call Morgan Stanley about the 401k [created::2026-09-20] ^call-ms\n\
         - [ ] #task Collect the equity paperwork [created::2026-09-20] ^equity-docs\n\
         \n\
         ## Future Work\n\
         \n\
         - Revisit the severance terms\n",
        "{contents}"
    );

    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(
        day_after,
        "## Pomodoros\n\
         - [ ] (1330-1400) Work\n\
         - [ ] () — ADMIN\n\
         \x20 - [[cash_goog_exit#^draft-memo]]\n\
         \x20 - [[cash_goog_exit#^call-ms]]\n",
        "{day_after}"
    );
    assert_eq!(day_after.matches("— ADMIN").count(), 1, "{day_after}");
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
        "Body @cash^human1@#admin\n- Draft the memo :draft-memo\n",
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
        "Body @cash^impl1@\n- Draft the memo :draft-memo\n- File it ^file-it\n";
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
    let input = "Body @cash^exist1@#admin\n- Draft the memo :draft-memo\n";
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
    let input = "Body @cash^drylink1@\n- Draft the memo :draft-memo\n";
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
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(
        dry_json["project_note"]["task_links"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{dry_json}"
    );
    assert!(!vault.join("cash_drylink1.md").exists());
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);

    // A dry run reports the same JSON as a real run, except for `dry_run`.
    let real_output = run_with_stdin(
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
    assert_success(&real_output);
    let mut real_json: serde_json::Value =
        serde_json::from_str(stdout(&real_output).trim())
            .expect("real-run JSON");
    assert_eq!(real_json["dry_run"], false);
    let mut dry_without_flag = dry_json.clone();
    dry_without_flag["dry_run"] = serde_json::json!(false);
    // `target`/`day_file` embed the temp vault path, which is identical
    // here because both runs share the vault; compare everything else.
    real_json["dry_run"] = serde_json::json!(false);
    assert_eq!(dry_without_flag, real_json, "dry-run JSON must match real");
    assert!(vault.join("cash_drylink1.md").exists());

    // A batch appends the second item's links after the first item's.
    let batch = TempDir::new("bob-cli-capture-project-note-link-batch");
    let vault = batch.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let input = "First @cash^batch1@\n- Alpha task :alpha\n\nSecond @cash^batch2@\n- Beta task :beta\n";
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
    let input = "Body @cash^dup2@\n- Draft the memo :dup\n";
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
    let input = "Body @cash^noday1@\n- Draft the memo :draft-memo\n";
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
    let input = "Body @cash^caretn1@\n- Draft the memo ^draft-memo\n";
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
    let input = "Body @cash^caretd1@\n- Draft the memo ^draft-memo\n";
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

    // A scheduled project blocks every open resident. `:` tasks also
    // remain unlinked, while `^` and plain task rows still get the same
    // immediate status/tag write (PDG5, PDT1).
    let scheduled = TempDir::new("bob-cli-capture-project-note-link-sched");
    let vault = scheduled.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    write_blocked_tasks_settings(&vault);
    let input = concat!(
        "Body @cash^schedlink1@ s:2\n",
        "- Draft the memo :draft-memo\n",
        "- Review blocker ^review-blocker\n",
        "- Plain follow-up\n",
        "- [x] Already done\n",
    );
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
        json["project_note"]["task_links"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "{json}"
    );
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_schedule"]),
        "{json}"
    );
    let contents = fs::read_to_string(vault.join("cash_schedlink1.md"))
        .expect("read scheduled note");
    assert!(
        contents.contains(
            "- [?] #task Draft the memo [created::2026-09-20] #prj_is_blocked ^draft-memo"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [?] #task Review blocker [created::2026-09-20] #prj_is_blocked ^review-blocker"
        ),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [?] #task Plain follow-up [created::2026-09-20] #prj_is_blocked"
        ),
        "{contents}"
    );
    assert!(
        contents.contains("- [x] #task Already done [created::2026-09-20]"),
        "closed resident must keep its state: {contents}"
    );
    assert!(
        !contents
            .contains("Already done [created::2026-09-20] #prj_is_blocked"),
        "closed resident is not tagged: {contents}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        !day_after.contains("[[cash_schedlink1#^draft-memo]]"),
        "{day_after}"
    );
}

#[test]
fn capture_project_note_task_links_report_the_resolved_pomodoro_name() {
    let temp = TempDir::new("bob-cli-capture-project-note-resolved-name");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (1330-1400) Work\n- [ ] () — ADMIN\n",
    );
    let input = "Finish it @cash^y@#adm\n- Draft :dr\n";
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
    assert_eq!(json["pomodoro_name"], "ADMIN", "{json}");
    assert_eq!(json["creates_pomodoro"], false, "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(day_after.matches("— ADMIN").count(), 1, "{day_after}");
    let admin = day_after.find("- [ ] () — ADMIN").expect("ADMIN entry");
    let link = day_after.find("[[cash_y#^dr]]").expect("task link");
    assert!(admin < link, "{day_after}");

    let human = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Finish it @cash^y2@#adm\n- Draft :dr2\n",
    );
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("under ADMIN\n"), "{out}");
    assert!(!out.contains("(created)"), "{out}");
}

#[test]
fn capture_project_note_reports_created_pomodoro_block() {
    // Two ` :<id>` tasks linked into one created Pomodoro are
    // auto-detected: no planner ref is pushed for project notes.
    let temp = TempDir::new("bob-cli-capture-project-note-blocks");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400) Work\n";
    write_file(&day_file, day_before);
    let input =
        "Finish packet! @cash^goog-exit@#admin\n- Draft memo :draft-memo\n- Call bank :call-ms\n";

    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        let output = run_with_stdin(&mut command, input);
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(
        fs::read_to_string(&day_file).expect("dry run writes nothing"),
        day_before
    );
    let json: serde_json::Value = run(false);
    assert_eq!(json["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::json!(false);
    assert_eq!(masked, json, "dry-run JSON must equal real-run JSON");

    assert_eq!(json["kind"], "project_note");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "ADMIN",
                "status": "queued",
                "created": true,
                "roles": ["changed"],
                "lines": [
                    {
                        "text": "- [ ] () — ADMIN",
                        "depth": 0,
                        "change": "added",
                    },
                    {
                        "text": "  - [[cash_goog_exit#^draft-memo]]",
                        "depth": 1,
                        "change": "added",
                    },
                    {
                        "text": "  - [[cash_goog_exit#^call-ms]]",
                        "depth": 1,
                        "change": "added",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read linked day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (1330-1400) Work\n",
            "- [ ] () — ADMIN\n",
            "  - [[cash_goog_exit#^draft-memo]]\n",
            "  - [[cash_goog_exit#^call-ms]]\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_project_note_moves_tasks_into_draft_slots_and_stamps_them() {
    let temp = TempDir::new("bob-cli-project-note-move-slots");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let source_before = concat!(
        "---\n",
        "type: \"[[area]]\"\n",
        "---\n",
        "# Cash\n",
        "- [ ] #task Alpha ^a\n",
        "- [ ] #task Beta [keeps:: 2] ^b\n",
        "\t- Beta's complete child\n",
    );
    write_file(&source, source_before);
    let input = concat!(
        "Finish the 🚀 exit packet @cash^x@\n",
        "- New task\n",
        "- ^b\n",
        "- ^a\n",
        "- Another task\n",
    );
    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        let output = run_with_stdin(&mut command, input);
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("project-note move JSON")
    };
    let dry = run(true);
    assert!(!vault.join("cash_x.md").exists());
    assert_eq!(
        fs::read_to_string(&source).expect("dry run source"),
        source_before
    );
    let result = run(false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::json!(false);
    assert_eq!(masked, result, "dry-run and commit JSON agree");

    let destination =
        fs::read_to_string(vault.join("cash_x.md")).expect("read destination");
    let new = destination.find("#task New task").expect("new task");
    let beta = destination.find("#task Beta").expect("moved beta");
    let alpha = destination.find("#task Alpha").expect("moved alpha");
    let another = destination.find("#task Another task").expect("last task");
    assert!(
        new < beta && beta < alpha && alpha < another,
        "{destination}"
    );
    assert!(
        destination.contains("\t- Beta's complete child"),
        "{destination}"
    );
    assert!(
        destination.contains("[fresh:: 2026-09-20]"),
        "{destination}"
    );
    assert!(!destination.contains("[keeps::"), "{destination}");
    assert!(
        !destination.contains("REPLACE WITH TASK DESCRIPTION"),
        "{destination}"
    );
    assert!(!fs::read_to_string(&source).unwrap().contains("^a"));
    assert!(!fs::read_to_string(&source).unwrap().contains("^b"));

    let project = &result["project_note"];
    assert_eq!(project["tasks"], 4, "{result}");
    assert_eq!(project["move_summary"]["moved"], 2, "{result}");
    assert_eq!(project["moved_tasks"][0]["block_id"], "b", "{result}");
    assert_eq!(project["moved_tasks"][1]["block_id"], "a", "{result}");
    assert_eq!(project["moved_tasks"][0]["source"], "cash.md", "{result}");
    assert_eq!(project["moved_tasks"][0]["fresh"], "2026-09-20", "{result}");
    assert_eq!(
        project["moved_tasks"][0]["status_name"], "Ready",
        "{result}"
    );
    assert_eq!(
        project["moved_tasks"][0]["draft_range"],
        serde_json::json!([
            input.find("^b").unwrap(),
            input.find("^b").unwrap() + 2
        ]),
        "{result}"
    );
}

#[test]
fn capture_project_note_move_only_draft_suppresses_placeholder() {
    let temp = TempDir::new("bob-cli-project-note-move-only");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task A ^a\n- [ ] #task B ^b\n",
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- ^a\n- ^b\n",
        false,
        None,
    );
    assert_success(&output);
    let note = fs::read_to_string(vault.join("cash_x.md")).expect("read note");
    assert!(note.contains("#task A"), "{note}");
    assert!(note.contains("#task B"), "{note}");
    assert!(!note.contains("REPLACE WITH TASK DESCRIPTION"), "{note}");
}

#[test]
fn capture_project_note_move_human_output_reports_moves_and_repairs() {
    let temp = TempDir::new("bob-cli-project-note-move-human-output");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let source_before =
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Move this task ^a\n";
    write_file(&source, source_before);
    write_file(&vault.join("other.md"), "See [[cash#^a]].\n");
    let input = "Project @cash^x@\n- ^a\n";

    let dry = project_note_move_human_capture(&vault, input, true);
    assert_success(&dry);
    let dry_text = stdout(&dry);
    assert!(
        dry_text.contains("1 task · 1 moved from cash.md · sections —"),
        "{dry_text}"
    );
    assert!(
        dry_text.contains("would move  [ ] Move this task  cash.md:4 → ^a"),
        "{dry_text}"
    );
    assert!(
        dry_text.contains("would repair  1 link in 1 note"),
        "{dry_text}"
    );
    assert!(dry_text.contains("bob projects sync"), "{dry_text}");
    assert!(!vault.join("cash_x.md").exists());
    assert_eq!(fs::read_to_string(&source).unwrap(), source_before);

    let committed = project_note_move_human_capture(&vault, input, false);
    assert_success(&committed);
    let committed_text = stdout(&committed);
    assert!(
        committed_text.contains("moved  [ ] Move this task  cash.md:4 → ^a"),
        "{committed_text}"
    );
    assert!(
        committed_text.contains("repaired  1 link in 1 note"),
        "{committed_text}"
    );
}

#[test]
fn capture_project_note_move_hint_counts_only_eligible_open_roots() {
    let temp = TempDir::new("bob-cli-project-note-move-hint");
    let vault = temp.path().join("vault");
    let mut parent = String::from("---\ntype: \"[[area]]\"\n---\n\n## Tasks\n");
    parent
        .push_str("- [ ] #task Parent ^parent\n  - [ ] #task Nested ^nested\n");
    for index in 0..11 {
        parent.push_str(&format!("- [ ] #task Open {index} ^open-{index}\n"));
    }
    parent.push_str(concat!(
        "- [ ] #task Hidden #hide ^hidden\n",
        "- [ ] #task ID-less\n",
        "- [x] #task Closed ^closed\n",
        "- [ ] #task Project #prj ^prj\n",
        "- [~] #task Unsupported ^unsupported\n",
        "- [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::2026-09-20]\n",
    ));
    write_file(&vault.join("cash.md"), &parent);

    let output =
        project_note_move_capture(&vault, "Project @cash^x@\n", true, None);
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["project_note"]["move_hint"],
        serde_json::json!({"parent": "cash.md", "open_count": 14}),
        "{json}"
    );

    let human =
        project_note_move_human_capture(&vault, "Project @cash^y@\n", true);
    assert_success(&human);
    assert!(
        stdout(&human).contains(
            "hint: '- ^' moves one of cash.md's 14 open tasks into this project"
        ),
        "{}",
        stdout(&human)
    );
}

#[test]
fn capture_project_note_move_repairs_and_reports_a_followed_work_link() {
    let temp = TempDir::new("bob-cli-project-note-move-followed-link");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [*] #task Work item ^a\n",
    );
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**1330-1400**) — WORK\n  - [[cash#^a]]\n",
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- ^a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    assert_eq!(
        json["project_note"]["moved_tasks"][0]["planned"],
        serde_json::json!({"entry_name": "WORK", "entry_line": 2}),
        "{json}"
    );
    assert_eq!(
        json["project_note"]["moved_tasks"][0]["detail"], "WORK link follows",
        "{json}"
    );
    let day_after = fs::read_to_string(&day).expect("read day note");
    assert!(day_after.contains("[[cash_x#^a]]"), "{day_after}");
    assert!(!day_after.contains("[[cash#^a]]"), "{day_after}");
}

#[test]
fn capture_project_note_moves_return_typed_eligibility_and_collision_refusals()
{
    let cases = [
        (
            "closed",
            "- [x] #task Paid ^a\n",
            "move_task_closed",
            "only open tasks move",
        ),
        (
            "nested",
            "- [ ] #task Parent ^parent\n  - [ ] #task Child ^a\n",
            "move_task_nested",
            "nested under `^parent`",
        ),
        (
            "missing",
            "- [ ] #task Other ^b\n",
            "move_task_missing",
            "no open task",
        ),
        (
            "ambiguous",
            "- [ ] #task First ^a\n- [ ] #task Second ^a\n",
            "move_task_ambiguous",
            "2 tasks ending `^a`",
        ),
    ];
    for (name, tasks, code, message) in cases {
        let temp = TempDir::new(&format!("bob-cli-move-eligibility-{name}"));
        let vault = temp.path().join("vault");
        let source = vault.join("cash.md");
        let before = format!("---\ntype: \"[[area]]\"\n---\n{tasks}");
        write_file(&source, &before);
        let output = project_note_move_capture(
            &vault,
            "Project @cash^x@\n- ^a\n",
            true,
            None,
        );
        assert!(
            !output.status.success(),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("error JSON");
        assert_eq!(json["code"], code, "{name}: {json}");
        assert!(json["error"].as_str().unwrap().contains(message), "{json}");
        assert_eq!(fs::read_to_string(&source).unwrap(), before, "{name}");
        assert!(!vault.join("cash_x.md").exists(), "{name}");
    }

    let temp = TempDir::new("bob-cli-move-descendant-collision");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let source_before = concat!(
        "---\ntype: \"[[area]]\"\n---\n",
        "- [ ] #task Call Morgan Stanley ^call-ms\n",
        "  - [ ] #task Notes ^notes\n",
    );
    write_file(&source, source_before);
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- ^call-ms\n- New notes ^notes\n",
        true,
        None,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(json["code"], "move_task_id_collision", "{json}");
    assert!(json["error"].as_str().unwrap().contains("^notes"), "{json}");
    assert_eq!(fs::read_to_string(&source).unwrap(), source_before);
    assert!(!vault.join("cash_x.md").exists());
}

#[test]
fn capture_project_note_future_schedule_refuses_ready_and_accepts_blocked_move()
{
    let ready = TempDir::new("bob-cli-move-future-ready");
    let ready_vault = ready.path().join("vault");
    let ready_source = ready_vault.join("cash.md");
    let ready_before = "---\ntype: \"[[area]]\"\n---\n- [ ] #task Ready ^a\n";
    write_file(&ready_source, ready_before);
    let refused = project_note_move_capture(
        &ready_vault,
        "Project @cash^x@ s:3\n- ^a\n",
        true,
        None,
    );
    assert!(!refused.status.success(), "{}", format_output(&refused));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&refused).trim()).expect("error JSON");
    assert_eq!(json["code"], "move_task_blocked_birth", "{json}");
    assert_eq!(fs::read_to_string(&ready_source).unwrap(), ready_before);
    assert!(!ready_vault.join("cash_x.md").exists());

    let blocked = TempDir::new("bob-cli-move-future-blocked");
    let blocked_vault = blocked.path().join("vault");
    let blocked_source = blocked_vault.join("cash.md");
    write_file(
        &blocked_source,
        "---\ntype: \"[[area]]\"\n---\n- [?] #task Blocked ^b\n",
    );
    let accepted = project_note_move_capture(
        &blocked_vault,
        "Project @cash^later@ s:3\n- ^b\n",
        false,
        None,
    );
    assert_success(&accepted);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&accepted).trim()).expect("move JSON");
    assert_eq!(
        json["project_note"]["move_summary"]["needs_reconcile"],
        true
    );
    let note = fs::read_to_string(blocked_vault.join("cash_later.md"))
        .expect("read future project");
    assert!(note.contains("- [?] #task Blocked"), "{note}");
}

#[test]
fn capture_project_note_rejects_a_second_move_from_the_same_origin() {
    let temp = TempDir::new("bob-cli-move-already-moved");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let before = "---\ntype: \"[[area]]\"\n---\n- [ ] #task A ^a\n";
    write_file(&source, before);
    let output = project_note_move_capture(
        &vault,
        "First @cash^x@\n- ^a\n\nSecond @cash^y@\n- ^a\n",
        true,
        None,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(json["code"], "move_task_already_moved", "{json}");
    assert!(json["error"].as_str().unwrap().contains("item 1"), "{json}");
    assert_eq!(fs::read_to_string(&source).unwrap(), before);
    assert!(!vault.join("cash_x.md").exists());
    assert!(!vault.join("cash_y.md").exists());
}

#[test]
fn capture_project_note_reads_a_chained_destination_as_the_next_parent() {
    let temp = TempDir::new("bob-cli-move-staged-chain");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task A ^a\n",
    );
    let output = project_note_move_capture(
        &vault,
        "First @cash^x@\n- ^a\n\nSecond @cash_x^y@\n- ^a\n",
        false,
        None,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("chain JSON");
    assert_eq!(
        json["captures"][1]["project_note"]["moved_tasks"][0]["item_line"], 5,
        "{json}"
    );
    assert_eq!(
        json["captures"][1]["project_note"]["moved_tasks"][0]["draft_range"],
        serde_json::json!([
            "First @cash^x@\n- ^a\n\nSecond @cash_x^y@\n- ^a\n"
                .rfind("^a")
                .unwrap(),
            "First @cash^x@\n- ^a\n\nSecond @cash_x^y@\n- ^a\n"
                .rfind("^a")
                .unwrap()
                + 2
        ]),
        "{json}"
    );
    let original = fs::read_to_string(vault.join("cash.md")).unwrap();
    let middle = fs::read_to_string(vault.join("cash_x.md")).unwrap();
    let final_note = fs::read_to_string(vault.join("cash_x_y.md")).unwrap();
    assert!(!original.contains("^a"), "{original}");
    assert!(!middle.contains("#task A"), "{middle}");
    assert!(final_note.contains("#task A"), "{final_note}");
}

#[test]
fn project_moves_and_target_runs_share_staged_task_snapshots() {
    let source_before = concat!(
        "---\ntype: \"[[area]]\"\n---\n",
        "- [ ] #task Alpha ^a\n",
        "- [ ] #task Beta ^b\n",
    );
    let day_before = "## Pomodoros\n- [ ] Work (0900-0930)\n";

    // A move creates cash_x.md only in the planner's staged snapshot. The
    // following run must resolve both moved tasks there, and dry run must
    // leave every source byte and destination path untouched.
    let after = TempDir::new("bob-cli-project-move-then-target-run");
    let after_vault = after.path().join("vault");
    let after_source = after_vault.join("cash.md");
    let after_day = after_vault.join("day.md");
    write_file(&after_source, source_before);
    write_file(&after_day, day_before);
    let after_input = concat!(
        "Project @cash^x@\n",
        "- ^a\n",
        "- ^b\n",
        "\n",
        "@cash_x+a @cash_x+b Shared note\n",
    );
    let preview = project_note_move_capture(
        &after_vault,
        after_input,
        true,
        Some(&after_day),
    );
    assert_success(&preview);
    assert_eq!(fs::read(&after_source).unwrap(), source_before.as_bytes());
    assert_eq!(fs::read(&after_day).unwrap(), day_before.as_bytes());
    assert!(!after_vault.join("cash_x.md").exists());

    let committed = project_note_move_capture(
        &after_vault,
        after_input,
        false,
        Some(&after_day),
    );
    assert_success(&committed);
    let after_json: serde_json::Value =
        serde_json::from_slice(&committed.stdout)
            .expect("staged target-run JSON");
    assert_eq!(after_json["captures"].as_array().unwrap().len(), 3);
    assert_eq!(after_json["captures"][1]["fanout"]["position"], 1);
    assert_eq!(after_json["captures"][2]["fanout"]["position"], 2);
    let moved_note = fs::read_to_string(after_vault.join("cash_x.md"))
        .expect("read moved project note");
    assert!(moved_note.contains("#task Alpha"), "{moved_note}");
    assert!(moved_note.contains("#task Beta"), "{moved_note}");
    assert_eq!(
        moved_note.matches("- Shared note").count(),
        2,
        "{moved_note}"
    );
    for id in ["a", "b"] {
        assert!(
            after_json["task_blocks"].as_array().unwrap().iter().any(|block| {
                block["relative_target"] == "cash_x.md"
                    && block["block_id"] == id
                    && block["lines"].as_array().unwrap().iter().any(|line| {
                        line["text"].as_str().unwrap_or_default().contains("Shared note")
                    })
            }),
            "task block ^{id} must resolve through the staged project move: {after_json}"
        );
    }

    // A run before the move edits the source snapshot first. The final task
    // block report must follow those same task identities to cash_x.md, and
    // the note text must travel with each moved task.
    let before = TempDir::new("bob-cli-target-run-then-project-move");
    let before_vault = before.path().join("vault");
    let before_source = before_vault.join("cash.md");
    let before_day = before_vault.join("day.md");
    write_file(&before_source, source_before);
    write_file(&before_day, day_before);
    let before_input = concat!(
        "@cash+a @cash+b Shared note\n",
        "\n",
        "Project @cash^x@\n",
        "- ^a\n",
        "- ^b\n",
    );
    let preview = project_note_move_capture(
        &before_vault,
        before_input,
        true,
        Some(&before_day),
    );
    assert_success(&preview);
    assert_eq!(fs::read(&before_source).unwrap(), source_before.as_bytes());
    assert_eq!(fs::read(&before_day).unwrap(), day_before.as_bytes());
    assert!(!before_vault.join("cash_x.md").exists());

    let committed = project_note_move_capture(
        &before_vault,
        before_input,
        false,
        Some(&before_day),
    );
    assert_success(&committed);
    let before_json: serde_json::Value =
        serde_json::from_slice(&committed.stdout)
            .expect("move after target-run JSON");
    assert_eq!(before_json["captures"][0]["fanout"]["position"], 1);
    assert_eq!(before_json["captures"][1]["fanout"]["position"], 2);
    let moved_note = fs::read_to_string(before_vault.join("cash_x.md"))
        .expect("read project note after target-run");
    assert_eq!(
        moved_note.matches("- Shared note").count(),
        2,
        "{moved_note}"
    );
    for id in ["a", "b"] {
        assert!(
            before_json["task_blocks"].as_array().unwrap().iter().any(|block| {
                block["relative_target"] == "cash_x.md"
                    && block["block_id"] == id
                    && block["lines"].as_array().unwrap().iter().any(|line| {
                        line["text"].as_str().unwrap_or_default().contains("Shared note")
                    })
            }),
            "task block ^{id} must be remapped to the moved note: {before_json}"
        );
    }

    // A later invalid run member rolls back the earlier move and the valid
    // first member's staged note write as one batch.
    let failed = TempDir::new("bob-cli-project-move-target-run-rollback");
    let failed_vault = failed.path().join("vault");
    let failed_source = failed_vault.join("cash.md");
    let failed_day = failed_vault.join("day.md");
    write_file(&failed_source, source_before);
    write_file(&failed_day, day_before);
    let failing_input = concat!(
        "Project @cash^x@\n",
        "- ^a\n",
        "- ^b\n",
        "\n",
        "@cash_x+a @cash_x+missing Shared note\n",
    );
    let rejected = project_note_move_capture(
        &failed_vault,
        failing_input,
        false,
        Some(&failed_day),
    );
    assert!(!rejected.status.success(), "{}", format_output(&rejected));
    assert_eq!(fs::read(&failed_source).unwrap(), source_before.as_bytes());
    assert_eq!(fs::read(&failed_day).unwrap(), day_before.as_bytes());
    assert!(!failed_vault.join("cash_x.md").exists());
}

#[test]
fn capture_project_note_child_line_seal_keeps_move_ranges_in_original_text() {
    let temp = TempDir::new("bob-cli-move-child-line-seal-range");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task A ^a\n",
    );
    let input = "Finish the 🚀 packet\n- Parent line @cash^x@\n- ^a\n";
    let output = project_note_move_capture(&vault, input, false, None);
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    assert_eq!(
        json["project_note"]["moved_tasks"][0]["item_line"], 3,
        "{json}"
    );
    let start = input.find("^a").expect("move token");
    assert_eq!(
        json["project_note"]["moved_tasks"][0]["draft_range"],
        serde_json::json!([start, start + 2]),
        "{json}"
    );
}

#[test]
fn capture_project_note_repairs_dependencies_for_stayed_and_moved_tasks() {
    let temp = TempDir::new("bob-cli-move-dependency-links");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [ ] #task Move [id:: cash__a] [dependsOn:: cash__b] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task Stay [id:: cash__c] [dependsOn:: cash__a] ^c\n",
            "  - ⛓️ **DEPENDS ON:** [[#^a]]\n",
            "- [ ] #task Blocker [id:: cash__b] ^b\n",
        ),
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- ^a\n",
        false,
        None,
    );
    assert_success(&output);
    let source = fs::read_to_string(vault.join("cash.md")).expect("source");
    let destination =
        fs::read_to_string(vault.join("cash_x.md")).expect("destination");
    assert!(source.contains("[[cash_x#^a]]"), "{source}");
    assert!(destination.contains("[[cash#^b]]"), "{destination}");
    assert!(source.contains("[dependsOn:: cash_x__a]"), "{source}");
    assert!(
        destination.contains("[dependsOn:: cash__b]"),
        "{destination}"
    );
}

#[test]
fn capture_project_note_offer_moves_are_offered_with_moved_from() {
    // MV5 (offers part): `@cash:x@` + `- ^a` + `- New` offers both rows;
    // the moved row carries `moved_from`.
    let temp = TempDir::new("bob-cli-move-offer-mv5");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Moved ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash:x@\n- ^a\n- New task\n",
        true,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("offer JSON");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    assert_eq!(successors.len(), 2, "{json}");
    // Draft order: moved `^a` (source 0) then new task (source 1).
    assert_eq!(successors[0]["block_id"], "a", "{json}");
    assert_eq!(successors[0]["offer"]["moved_from"], "cash.md", "{json}");
    assert!(
        successors[1]["offer"].get("moved_from").is_none()
            || successors[1]["offer"]["moved_from"].is_null(),
        "{json}"
    );
    // Keys: moved uses `<new>#^<id>`, new ID-less uses digest.
    assert_eq!(successors[0]["offer"]["key"], "cash_x.md#^a", "{json}");
}

#[test]
fn capture_project_note_colon_move_links_next_under_named_pomodoro() {
    // MV4: `@cash^x@#admin` + `- :a` (Ready) moves, becomes Next, links
    // under ADMIN.
    let temp = TempDir::new("bob-cli-move-colon-mv4");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Ready task ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] () — ADMIN\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@#admin\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "a", "{json}");
    assert_eq!(moved["sigil"], ":", "{json}");
    assert_eq!(moved["status_symbol"], "*", "{json}");
    assert_eq!(moved["status_name"], "Next", "{json}");
    assert!(moved.get("link").is_some(), "{json}");
    assert_eq!(moved["link"]["block_link"], "[[cash_x#^a]]", "{json}");
    assert!(moved["not_linked"].is_null(), "{json}");
    assert_eq!(
        moved["detail"], "becomes Next · links under ADMIN",
        "{json}"
    );
    assert_eq!(
        json["project_note"]["task_links"][0]["block_id"], "a",
        "{json}"
    );
    assert_eq!(json["pomodoro_name"], "ADMIN", "{json}");
    let note = fs::read_to_string(vault.join("cash_x.md")).expect("new note");
    assert!(note.contains("- [*] #task Ready task"), "{note}");
    assert!(!fs::read_to_string(vault.join("cash.md"))
        .unwrap()
        .contains("^a"));
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(day_after.contains("[[cash_x#^a]]"), "{day_after}");
}

#[test]
fn capture_project_note_colon_move_drops_local_prerequisite_and_links() {
    // MV16: `- :a` blocked only by its own open prerequisite drops it,
    // becomes Next, and links.
    let temp = TempDir::new("bob-cli-move-colon-mv16");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [?] #task Blocked [dependsOn:: cash__b] [id:: cash__a] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task Prereq [id:: cash__b] ^b\n",
        ),
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "a", "{json}");
    assert_eq!(moved["status_symbol"], "*", "{json}");
    assert!(moved.get("link").is_some(), "{json}");
    assert!(moved["not_linked"].is_null(), "{json}");
    assert_eq!(
        json["dropped_dependencies"].as_array().unwrap().len(),
        1,
        "{json}"
    );
    assert_eq!(
        moved["detail"],
        "drops 1 prerequisite · becomes Next · links into today's plan",
        "{json}"
    );
    let note = fs::read_to_string(vault.join("cash_x.md")).expect("new note");
    assert!(note.contains("- [*] #task Blocked"), "{note}");
    assert!(!note.contains("DEPENDS ON"), "{note}");
}

#[test]
fn capture_project_note_colon_move_future_schedule_stays_unlinked() {
    // Colon MV9: `s:3` + `- :b` ([?]) moves and stays unlinked with
    // `project_schedule`.
    let temp = TempDir::new("bob-cli-move-colon-mv9");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [?] #task Blocked ^b\n",
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^later@ s:3\n- :b\n",
        false,
        None,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "b", "{json}");
    assert_eq!(moved["sigil"], ":", "{json}");
    assert!(moved["link"].is_null(), "{json}");
    assert_eq!(moved["not_linked"], "project_schedule", "{json}");
    assert_eq!(
        moved["detail"], "stays unlinked until the project starts Sep 23",
        "{json}"
    );
    let note =
        fs::read_to_string(vault.join("cash_later.md")).expect("new note");
    assert!(note.contains("- [?] #task Blocked"), "{note}");
}

#[test]
fn capture_project_note_mixed_new_and_colon_move_preserves_draft_order() {
    // Mixed ordering: new `:` task and colon move link in draft order.
    let temp = TempDir::new("bob-cli-move-colon-mixed");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Moved ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- New task :new\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let links = json["project_note"]["task_links"]
        .as_array()
        .expect("links");
    assert_eq!(links.len(), 2, "{json}");
    assert_eq!(links[0]["block_id"], "new", "{json}");
    assert_eq!(links[1]["block_id"], "a", "{json}");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "a", "{json}");
    assert!(moved.get("link").is_some(), "{json}");
    let note = fs::read_to_string(vault.join("cash_x.md")).expect("new note");
    let new_pos = note.find("#task New task").expect("new");
    let moved_pos = note.find("#task Moved").expect("moved");
    assert!(new_pos < moved_pos, "{note}");
}

#[test]
fn capture_project_note_offer_all_picks_equal_explicit_colon_moves() {
    // MV5 (selection part): Offer `-p all` on `- ^a` + `- New` writes
    // byte-identical files to explicit `- :a` + `- New :new`.
    let make_vault = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        let day = vault.join("day.md");
        write_file(
            &vault.join("cash.md"),
            "---\ntype: \"[[area]]\"\n---\n- [ ] #task Moved ^a\n",
        );
        write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
        (temp, vault, day)
    };
    let (_temp_a, vault_a, day_a) = make_vault("bob-cli-move-offer-all-a");
    let (_temp_b, vault_b, day_b) = make_vault("bob-cli-move-offer-all-b");
    let offer_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_a)
            .arg("-f")
            .arg("json")
            .arg("-p")
            .arg("all")
            .env("BOB_DAY_FILE", &day_a)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Project @cash:x@\n- ^a\n- New\n",
    );
    assert_success(&offer_output);
    let offer_json: serde_json::Value =
        serde_json::from_str(stdout(&offer_output).trim()).expect("offer JSON");
    let successors = offer_json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    assert_eq!(successors.len(), 2, "{offer_json}");
    for row in successors {
        assert_eq!(row["offer"]["selected"], true, "{row}");
        assert_eq!(row["status_symbol"], "*", "{row}");
        assert!(row.get("link").is_some(), "{row}");
    }
    assert_eq!(successors[0]["block_id"], "a", "{offer_json}");
    assert_eq!(
        successors[0]["offer"]["moved_from"], "cash.md",
        "{offer_json}"
    );
    // "New" mints the fallback `task` ID (no usable word for a suggestion).
    assert_eq!(successors[1]["block_id"], "task", "{offer_json}");
    let explicit_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_b)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_b)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Project @cash^x@\n- :a\n- New :task\n",
    );
    assert_success(&explicit_output);
    assert_eq!(
        fs::read_to_string(vault_a.join("cash_x.md")).expect("offer note"),
        fs::read_to_string(vault_b.join("cash_x.md")).expect("explicit note"),
        "picked moved+new rows render exactly like authored `:` bullets"
    );
    assert_eq!(
        fs::read_to_string(&day_a).expect("offer day"),
        fs::read_to_string(&day_b).expect("explicit day"),
        "picked moved+new links land exactly like authored `:` links"
    );
    assert_eq!(
        fs::read_to_string(vault_a.join("cash.md")).expect("offer parent"),
        fs::read_to_string(vault_b.join("cash.md")).expect("explicit parent"),
        "moved parent blocks match"
    );
}

#[test]
fn capture_project_note_offer_none_leaves_moves_unlinked() {
    // `-p none`: moves happen, new tasks are created, but nothing links.
    let temp = TempDir::new("bob-cli-move-offer-none");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Moved ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("-p")
            .arg("none")
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Project @cash:x@\n- ^a\n- New\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("offer JSON");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    assert_eq!(successors.len(), 2, "{json}");
    for row in successors {
        assert_eq!(row["offer"]["selected"], false, "{row}");
        assert!(row.get("link").is_none() || row["link"].is_null(), "{row}");
    }
    assert!(
        json["project_note"]["task_links"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{json}"
    );
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "a", "{json}");
    assert!(moved["link"].is_null(), "{json}");
    let note = fs::read_to_string(vault.join("cash_x.md")).expect("new note");
    assert!(note.contains("#task Moved"), "{note}");
    assert!(note.contains("#task New"), "{note}");
}

#[test]
fn capture_project_note_offer_subset_picks_only_first_offer() {
    // `-p 1` selects only the moved offer (draft order a then New).
    let temp = TempDir::new("bob-cli-move-offer-subset");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Moved ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("-p")
            .arg("1")
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0"),
        "Project @cash:x@\n- ^a\n- New\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("offer JSON");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    assert_eq!(successors.len(), 2, "{json}");
    assert_eq!(successors[0]["offer"]["selected"], true, "{json}");
    assert_eq!(successors[1]["offer"]["selected"], false, "{json}");
    assert!(
        successors[0].get("link").is_some() && !successors[0]["link"].is_null(),
        "{json}"
    );
    assert!(
        successors[1].get("link").is_none() || successors[1]["link"].is_null(),
        "{json}"
    );
    let links = json["project_note"]["task_links"]
        .as_array()
        .expect("links");
    assert_eq!(links.len(), 1, "{json}");
    assert_eq!(links[0]["block_id"], "a", "{json}");
}

#[test]
fn capture_project_note_colon_move_next_stays_next_and_links() {
    // Next `[*]` colon move stays Next and links (no `becomes` prefix).
    let temp = TempDir::new("bob-cli-move-colon-next");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [*] #task Next task ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] () — ADMIN\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@#admin\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["status_symbol"], "*", "{json}");
    assert!(moved.get("link").is_some(), "{json}");
    assert_eq!(moved["detail"], "links under ADMIN", "{json}");
}

#[test]
fn capture_project_note_colon_move_keeps_in_progress_lane() {
    let temp = TempDir::new("bob-cli-move-colon-in-progress");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [/] #task Active task ^a\n",
    );
    write_file(&day, "## Pomodoros\n- [ ] () — ADMIN\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@#admin\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["status_symbol"], "/", "{json}");
    assert_eq!(moved["status_name"], "In Progress", "{json}");
    assert_eq!(moved["detail"], "links under ADMIN", "{json}");
}

#[test]
fn capture_project_note_caret_move_reports_blocked_schedule() {
    let temp = TempDir::new("bob-cli-move-caret-blocked-schedule");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [?] #task Waiting [scheduled:: 2026-09-23] ^a\n",
        ),
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- ^a\n",
        false,
        None,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["status_symbol"], "?", "{json}");
    assert_eq!(moved["detail"], "stays Blocked until Sep 23", "{json}");
}

#[test]
fn capture_project_note_colon_move_future_ready_is_refused() {
    // Colon MV9 (Ready): `s:3` + `- :a` (Ready) refuses with
    // `move_task_blocked_birth`; vault byte-identical.
    let temp = TempDir::new("bob-cli-move-colon-future-ready");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Ready ^a\n",
    );
    let before = fs::read_to_string(vault.join("cash.md")).unwrap();
    let output = project_note_move_capture(
        &vault,
        "Project @cash^later@ s:3\n- :a\n",
        false,
        None,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("refusal JSON")["code"],
        "move_task_blocked_birth"
    );
    assert_eq!(fs::read_to_string(vault.join("cash.md")).unwrap(), before);
    assert!(!vault.join("cash_later.md").exists());
}

fn project_note_move_capture(
    vault: &Path,
    input: &str,
    dry_run: bool,
    day_file: Option<&Path>,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    if let Some(day_file) = day_file {
        command.env("BOB_DAY_FILE", day_file);
    }
    command
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    run_with_stdin(&mut command, input)
}

fn project_note_move_human_capture(
    vault: &Path,
    input: &str,
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    run_with_stdin(&mut command, input)
}

#[test]
fn capture_move_landing_repairs_tracker_forwards_moved_roots() {
    // Defect 1: a shared-note touch plus a project move must not panic;
    // the tracked root follows to the destination with roles preserved.
    // Dry-run JSON agrees with real JSON after normalizing dry_run.
    let temp = TempDir::new("bob-cli-move-landing-tracker");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [ ] #task Alpha ^a\n",
            "- [ ] #task Beta ^b\n",
        ),
    );
    write_file(&day, "## Pomodoros\n- [ ] Work (0900-0930)\n");
    // Two valid sub-bullet touches (one per task) then a project move of
    // ^a. The tracker must forward ^a to the destination instead of
    // tripping the lost-headline guard.
    let input = concat!(
        "Touch one @cash+a\n",
        "\n",
        "Touch two @cash+b\n",
        "\n",
        "Project @cash^x@\n",
        "- ^a\n",
    );
    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        let output = run_with_stdin(&mut command, input);
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture JSON")
    };
    let day_before = fs::read_to_string(&day).expect("read day");
    let dry = run(true);
    assert!(!vault.join("cash_x.md").exists(), "{dry}");
    assert_eq!(
        fs::read_to_string(&day).expect("dry writes nothing"),
        day_before
    );
    let json = run(false);
    fn mask_dry_run(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    if key == "dry_run" {
                        *val = serde_json::json!(false);
                    } else {
                        mask_dry_run(val);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    mask_dry_run(item);
                }
            }
            _ => {}
        }
    }
    let mut masked = dry.clone();
    mask_dry_run(&mut masked);
    let mut expected = json.clone();
    mask_dry_run(&mut expected);
    assert_eq!(masked, expected, "dry-run must equal real-run JSON");
    // Multi-item batch output nests the project-note capture in captures[].
    let project_json = if json.get("project_note").is_some() {
        json.clone()
    } else {
        json["captures"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("project_note").is_some())
                    .cloned()
            })
            .expect("project-note capture in batch")
    };
    let moved = &project_json["project_note"]["moved_tasks"];
    assert_eq!(moved.as_array().unwrap().len(), 1, "{json}");
    assert_eq!(moved[0]["block_id"], "a", "{json}");
    assert_eq!(moved[0]["source"], "cash.md", "{json}");
    let dest = fs::read_to_string(vault.join("cash_x.md")).expect("dest");
    assert!(dest.contains("#task Alpha"), "{dest}");
    let source = fs::read_to_string(vault.join("cash.md")).expect("source");
    assert!(!source.contains("^a"), "{source}");
    assert!(source.contains("^b"), "{source}");
}

#[test]
fn capture_move_landing_repairs_exact_live_link_identities() {
    // Defect 2: prose mentions and prefix IDs never count as live links.
    // A prose mention must still create a dedicated ADMIN link.
    let temp = TempDir::new("bob-cli-move-landing-live");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [ ] #task Admin work ^admin\n",
        ),
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] Work (0900-0930)\n",
            "  - Discuss [[cash#^admin]]\n",
        ),
    );
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@#admin\n- :admin\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "admin", "{json}");
    assert!(moved.get("link").is_some(), "{json} prose is inert");
    assert!(moved["not_linked"].is_null(), "{json}");
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(day_after.contains("[[cash_x#^admin]]"), "{day_after}");
}

#[test]
fn capture_move_landing_repairs_hidden_eligibility_coordinates() {
    // Defect 3: hidden final rows are offered (not visible-before-hidden).
    // MovableRoot lines are 1-based; parent_lines is 0-based.
    let temp = TempDir::new("bob-cli-move-landing-hidden");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [ ] #task Visible ^a\n",
            "- [ ] #task Secret #hide ^b\n",
        ),
    );
    // Offer form via :x seal with caret moves: eligible rows are offered,
    // hidden rows fold instead of offering. MovableRoot lines are 1-based.
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Work (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash:x@\n- ^a\n",
        true,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    // Visible ^a immediately before hidden ^b must be offered (not
    // wrongly excluded by the off-by-one).
    assert_eq!(successors.len(), 1, "{json} visible before hidden");
    // Explicit hidden move still moves when named (Create form).
    let output = project_note_move_capture(
        &vault,
        "Project @cash^y@\n- ^b\n",
        false,
        None,
    );
    assert_success(&output);
    let note = fs::read_to_string(vault.join("cash_y.md")).expect("dest");
    assert!(note.contains("^b"), "{note}");
    // Hidden final row is never offered: same Offer draft against a vault
    // where ^a itself is hidden must fold, not offer.
    let temp = TempDir::new("bob-cli-move-landing-hidden-final");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [ ] #task Secret #hide ^a\n",
        ),
    );
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Work (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash:x@\n- ^a\n",
        true,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let successors = json["project_note"]["successors"]
        .as_array()
        .expect("successors");
    assert_eq!(successors.len(), 0, "{json} hidden final folds");
}

#[test]
fn capture_move_landing_repairs_inherited_blocking_at_new_boundary() {
    // Defect 4: a future-scheduled parent blocks the new project; a colon
    // move through the new boundary stays unlinked, never Next+linked.
    let temp = TempDir::new("bob-cli-move-landing-inherited");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    crate::support::write_toggle_task_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "- [?] #task Parent #prj #hide [scheduled:: 2026-09-25] ^prj\n",
            "- [?] #task Alpha #prj_is_blocked ^a\n",
        ),
    );
    write_file(&day, "## Pomodoros\n- [ ] Work (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- :a\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = &json["project_note"]["moved_tasks"][0];
    assert_eq!(moved["block_id"], "a", "{json}");
    assert!(moved["link"].is_null(), "{json} inherited stays unlinked");
    assert!(moved["not_linked"].is_string(), "{json}");
    assert_ne!(moved["status_symbol"], "*", "{json} never Next");
    let dest = fs::read_to_string(vault.join("cash_x.md")).expect("dest");
    assert!(dest.contains("#prj_is_blocked"), "{dest}");
}

#[test]
fn capture_move_landing_repairs_moved_reports_after_child_edits() {
    // Defect 5: dependency-child removal shifting later moved rows must
    // re-resolve every moved root by block ID (c on 14, not stale 15).
    let temp = TempDir::new("bob-cli-move-landing-reresolve");
    let vault = temp.path().join("vault");
    let day = vault.join("day.md");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "- [?] #task Blocked [dependsOn:: cash__b] [id:: cash__a] ^a\n",
            "  - ⛓️ **DEPENDS ON:** [[#^b]]\n",
            "- [ ] #task Prereq [id:: cash__b] ^b\n",
            "- [ ] #task Ready ^c\n",
        ),
    );
    write_file(&day, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let output = project_note_move_capture(
        &vault,
        "Project @cash^x@\n- :a\n- ^c\n",
        false,
        Some(&day),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    let moved = json["project_note"]["moved_tasks"]
        .as_array()
        .expect("moved");
    assert_eq!(moved.len(), 2, "{json}");
    let dest = fs::read_to_string(vault.join("cash_x.md")).expect("dest");
    for task in moved {
        let id = task["block_id"].as_str().unwrap();
        let reported = task["line"].as_u64().unwrap() as usize;
        let actual = dest
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(&format!("^{id}")))
            .map(|(index, _)| index + 1)
            .expect("block in dest");
        assert_eq!(reported, actual, "{json} {dest} stale line for ^{id}");
    }
}

#[test]
fn capture_project_note_choice_report_lists_rows_and_refuses_real() {
    // cli-report (MQ5/MQ6): unanswered dry run succeeds with numbered rows;
    // unanswered real run refuses `move_choice_required` write-free.
    let temp = TempDir::new("bob-cli-choice-report");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let source_before = "---\ntype: \"[[area]]\"\n---\n- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n- [ ] #task Gamma\n";
    write_file(&source, source_before);

    let dry = project_note_move_capture(
        &vault,
        "Finish @cash^x@\n- ^^\n",
        true,
        None,
    );
    assert_success(&dry);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("choice JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["move_choice"]["supported"], true);
    let slot = &json["move_choice"]["slots"][0];
    assert_eq!(slot["answered"], false);
    assert_eq!(slot["open_count"], 3);
    assert_eq!(slot["rows"].as_array().unwrap().len(), 3);
    assert_eq!(slot["rows"][0]["n"], 1);
    assert_eq!(slot["rows"][0]["key"], "cash.md#^a");
    assert_eq!(slot["rows"][2]["mint"], "gamma");
    for field in [
        "section",
        "block_id",
        "mint",
        "scheduled",
        "pomodoro",
        "locked",
        "disabled_reason",
        "answer",
    ] {
        assert!(slot["rows"][2].get(field).is_some(), "missing {field}");
    }
    assert!(slot["rows"][2]["section"].is_null());
    assert!(slot["rows"][2]["block_id"].is_null());
    assert!(slot["rows"][2]["pomodoro"].is_null());
    assert!(slot["rows"][2]["choices"][0]["consequence"]
        .as_str()
        .unwrap()
        .contains("adds ^gamma"));

    let human = project_note_move_human_capture(
        &vault,
        "Finish @cash^x@\n- ^^\n",
        true,
    );
    assert_success(&human);
    let out = String::from_utf8_lossy(&human.stdout).to_string();
    assert!(
        out.contains(
            "- ^^ on line 2 asks which of cash.md's 3 open tasks move in:"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "choose with -m 1,3 (move), -m :2 (move & link), or -m none"
        ),
        "{out}"
    );

    let real = project_note_move_capture(
        &vault,
        "Finish @cash^x@\n- ^^\n",
        false,
        None,
    );
    assert!(!real.status.success());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("required JSON");
    assert_eq!(json["code"], "move_choice_required");
    assert!(json["move_choice"]["slots"].is_array());
    assert_eq!(
        fs::read_to_string(&source).expect("source unchanged"),
        source_before
    );
    assert!(!vault.join("cash_x.md").exists());
}

#[test]
fn capture_project_note_choice_refusals_are_write_free() {
    // MQ10/MQ14: unasked, conflict, stale, refused, usage.
    let temp = TempDir::new("bob-cli-choice-refusals");
    let vault = temp.path().join("vault");
    let source = vault.join("cash.md");
    let before = "---\ntype: \"[[area]]\"\n---\n- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n";
    write_file(&source, before);

    // -m without - ^^.
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-m")
        .arg("1")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    let output = run_with_stdin(&mut command, "Hello @cash\n");
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("unasked JSON");
    assert_eq!(json["code"], "move_choice_unasked");

    // Conflict: none + pick, and duplicate row both sigils.
    for picks in [["none", "1"], ["1", ":1"]] {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run");
        for pick in picks {
            command.arg("-m").arg(pick);
        }
        command
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        let output = run_with_stdin(&mut command, "Finish @cash^x@\n- ^^\n");
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("conflict JSON");
        assert_eq!(json["code"], "move_choice_conflict", "{picks:?} {json}");
    }

    // Usage: -m foo.
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("-m")
        .arg("foo")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    let output = run_with_stdin(&mut command, "Finish @cash^x@\n- ^^\n");
    assert!(!output.status.success());
    // Usage error has no code, but writes nothing.
    let raw = stdout(&output).trim().to_string();
    assert!(raw.contains("--move-in pick"), "{raw}");

    // Stale: out-of-range number.
    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .arg("-m")
            .arg("99")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        run_with_stdin(&mut command, "Finish @cash^x@\n- ^^\n")
    };
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("stale JSON");
    assert_eq!(json["code"], "move_choice_stale");

    // Refused: locked row via explicit move in same draft.
    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .arg("-m")
            .arg("1")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        run_with_stdin(&mut command, "Finish @cash^x@\n- ^a\n- ^^\n")
    };
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("refused JSON");
    assert_eq!(json["code"], "move_choice_refused");

    assert_eq!(fs::read_to_string(&source).expect("unchanged"), before);
    assert!(!vault.join("cash_x.md").exists());
}

#[test]
fn capture_project_note_choice_empty_and_locked() {
    // MQ11/MQ15: empty parent dry ok with warning, real refuses empty.
    let temp = TempDir::new("bob-cli-choice-empty");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("empty.md"),
        "---\ntype: \"[[area]]\"\n---\nNo tasks.\n",
    );
    let dry = project_note_move_capture(
        &vault,
        "Finish @empty^x@\n- ^^\n",
        true,
        None,
    );
    assert_success(&dry);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("empty dry JSON");
    assert_eq!(json["ok"], true);
    assert!(json["warnings"].as_array().unwrap().iter().any(|w| w
        .as_str()
        .unwrap_or_default()
        .contains("no selectable tasks")));

    let real = project_note_move_capture(
        &vault,
        "Finish @empty^x@\n- ^^\n",
        false,
        None,
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("empty real JSON");
    assert_eq!(json["code"], "move_choice_empty");

    // All locked: explicit move claims the only row.
    let temp = TempDir::new("bob-cli-choice-locked");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Alpha ^a\n",
    );
    let dry = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        run_with_stdin(&mut command, "Finish @cash^x@\n- ^a\n- ^^\n")
    };
    assert_success(&dry);
}

#[test]
fn capture_project_note_choice_name_needs_link() {
    // MQ16: #name needs a : pick with adds_link.
    let temp = TempDir::new("bob-cli-choice-name");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n",
    );
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] (1330-1400) Work\n");

    let dry = project_note_move_capture(
        &vault,
        "Finish @cash^x@#admin\n- ^^\n",
        true,
        Some(&day),
    );
    assert_success(&dry);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("name JSON");
    assert_eq!(json["move_choice"]["slots"][0]["name_needs_link"], true);

    let refused = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .arg("-m")
            .arg("1")
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", "2026-09-20 14:31:07")
            .env("TZ", "UTC0");
        run_with_stdin(&mut command, "Finish @cash^x@#admin\n- ^^\n")
    };
    let json: serde_json::Value =
        serde_json::from_str(stdout(&refused).trim()).expect("unused JSON");
    assert_eq!(json["code"], "unused_project_note_pomodoro");
}

#[test]
fn capture_project_note_choice_unanswered_lists_all_rows() {
    // MQ19: unanswered preview prints every row, even above 20.
    let temp = TempDir::new("bob-cli-choice-many");
    let vault = temp.path().join("vault");
    let mut parent = String::from("---\ntype: \"[[area]]\"\n---\n");
    for i in 1..=25 {
        parent.push_str(&format!("- [ ] #task Task {i:02} ^t{i:02}\n"));
    }
    write_file(&vault.join("cash.md"), &parent);
    let human = project_note_move_human_capture(
        &vault,
        "Finish @cash^x@\n- ^^\n",
        true,
    );
    assert_success(&human);
    let out = String::from_utf8_lossy(&human.stdout).to_string();
    assert!(out.contains("25 open tasks"), "{out}");
    for n in 1..=25 {
        assert!(out.contains(&format!("{n} [ ]")), "{out} missing row {n}");
    }
    assert!(out.contains("choose with -m"), "{out}");
}

fn project_note_choice_capture(
    vault: &Path,
    input: &str,
    dry_run: bool,
    day_file: Option<&Path>,
    move_picks: &[&str],
    pull_picks: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    if let Some(day_file) = day_file {
        command.env("BOB_DAY_FILE", day_file);
    }
    for pick in move_picks {
        command.arg("-m").arg(pick);
    }
    for pick in pull_picks {
        command.arg("-p").arg(pick);
    }
    command
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    run_with_stdin(&mut command, input)
}

fn choice_vault(
    name: &str,
    parent: &str,
    day: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("cash.md"), parent);
    write_file(&day_file, day);
    (temp, vault, day_file)
}

fn choice_parent(rows: &[&str]) -> String {
    let mut parent = String::from("---\ntype: \"[[area]]\"\n---\n");
    for row in rows {
        parent.push_str(row);
        parent.push('\n');
    }
    parent
}

#[test]
fn capture_project_note_choice_answers_match_authored_bullets() {
    // MQ13/MQ18: picks expand to authored moves at the ^^ slot, in parent
    // order; repeated flags union, keys and numbers resolve identically.
    let parent = choice_parent(&[
        "- [ ] #task Alpha ^a",
        "- [ ] #task Beta ^b",
        "- [ ] #task Gamma ^c",
        "- [ ] #task Delta ^d",
    ]);
    let day = "## Pomodoros\n- [ ] (1400-1500) WORK\n";
    let choice_input =
        "Finish packet @cash^x@#work\n- New item\n- ^^\n- Other item\n";
    let authored_input = concat!(
        "Finish packet @cash^x@#work\n",
        "- New item\n",
        "- :a\n",
        "- ^d\n",
        "- Other item\n",
    );

    let (_choice_temp, choice_real_vault, choice_day) =
        choice_vault("bob-cli-choice-equivalence", &parent, day);
    let dry = project_note_choice_capture(
        &choice_real_vault,
        choice_input,
        true,
        Some(&choice_day),
        &["4", ":1"],
        &[],
    );
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry JSON");
    assert!(!choice_real_vault.join("cash_x.md").exists());
    let real = project_note_choice_capture(
        &choice_real_vault,
        choice_input,
        false,
        Some(&choice_day),
        &["4", ":1"],
        &[],
    );
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    let mut normalized_dry = dry_json.clone();
    normalized_dry["dry_run"] = serde_json::json!(false);
    assert_eq!(normalized_dry, real_json, "dry-run JSON equals real JSON");

    let (_authored_temp, authored_vault, authored_day) =
        choice_vault("bob-cli-choice-equivalence-authored", &parent, day);
    let authored = project_note_choice_capture(
        &authored_vault,
        authored_input,
        false,
        Some(&authored_day),
        &[],
        &[],
    );
    assert_success(&authored);
    for relative in ["cash.md", "cash_x.md", "day.md"] {
        assert_eq!(
            fs::read(choice_real_vault.join(relative)).expect("choice bytes"),
            fs::read(authored_vault.join(relative)).expect("authored bytes"),
            "choice and authored runs match {relative}"
        );
    }
    let destination = fs::read_to_string(choice_real_vault.join("cash_x.md"))
        .expect("destination");
    let positions: Vec<usize> = ["New item", "Alpha", "Delta", "Other item"]
        .iter()
        .map(|needle| destination.find(needle).expect("task in destination"))
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{destination}"
    );

    let mut choice_moves = real_json["project_note"]["moved_tasks"].clone();
    let authored_json: serde_json::Value =
        serde_json::from_str(stdout(&authored).trim()).expect("authored JSON");
    let mut authored_moves =
        authored_json["project_note"]["moved_tasks"].clone();
    for moves in [
        choice_moves.as_array_mut().expect("choice moves"),
        authored_moves.as_array_mut().expect("authored moves"),
    ] {
        for moved in moves {
            let row = moved.as_object_mut().expect("moved row");
            row.remove("via");
            row.remove("draft_range");
            // `item_line` points at the authored bullet or the ^^ slot,
            // so source provenance differs even when the move outcome does not.
            row.remove("item_line");
        }
    }
    assert_eq!(choice_moves, authored_moves);
    assert_eq!(real_json["move_choice"]["slots"][0]["moved_count"], 2);
    assert_eq!(real_json["move_choice"]["slots"][0]["linked_count"], 1);

    let (_order_temp, order_vault, order_day) =
        choice_vault("bob-cli-choice-order", &parent, day);
    let order_a = project_note_choice_capture(
        &order_vault,
        "Finish @cash^x@\n- ^^\n",
        true,
        Some(&order_day),
        &["3,1"],
        &[],
    );
    let order_b = project_note_choice_capture(
        &order_vault,
        "Finish @cash^x@\n- ^^\n",
        true,
        Some(&order_day),
        &["1,3"],
        &[],
    );
    let order_a_json: serde_json::Value =
        serde_json::from_str(stdout(&order_a).trim()).expect("order A JSON");
    let order_b_json: serde_json::Value =
        serde_json::from_str(stdout(&order_b).trim()).expect("order B JSON");
    assert_eq!(
        order_a_json, order_b_json,
        "pick order does not change result"
    );

    let (_key_temp, key_vault, key_day) =
        choice_vault("bob-cli-choice-key-number", &parent, day);
    let number = project_note_choice_capture(
        &key_vault,
        "Finish @cash^x@\n- ^^\n",
        true,
        Some(&key_day),
        &[":1"],
        &[],
    );
    let key = project_note_choice_capture(
        &key_vault,
        "Finish @cash^x@\n- ^^\n",
        true,
        Some(&key_day),
        &[":cash.md#^a"],
        &[],
    );
    let number_json: serde_json::Value =
        serde_json::from_str(stdout(&number).trim()).expect("number JSON");
    let key_json: serde_json::Value =
        serde_json::from_str(stdout(&key).trim()).expect("key JSON");
    assert_eq!(
        number_json, key_json,
        "row keys and numbers resolve equally"
    );
}

#[test]
fn capture_project_note_choice_mints_and_guards_idless_rows() {
    // MQ12: suggestions avoid authored IDs, are written with the move, and
    // a collision with a moved descendant refuses the entire capture.
    let parent = choice_parent(&["- [ ] #task Idless"]);
    let (_mint_temp, mint_vault, mint_day) = choice_vault(
        "bob-cli-choice-mint-dry",
        &parent,
        "## Pomodoros\n- [ ] (1400-1500) WORK\n",
    );
    let dry = project_note_choice_capture(
        &mint_vault,
        "Finish @cash^x@\n- Existing ^idless\n- ^^\n",
        true,
        Some(&mint_day),
        &["1"],
        &[],
    );
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("mint dry JSON");
    let mint = dry_json["move_choice"]["slots"][0]["rows"][0]["mint"]
        .as_str()
        .expect("mint suggestion");
    assert_ne!(mint, "idless", "new-note authored ID is reserved");
    assert!(!mint_vault.join("cash_x.md").exists());
    let real = project_note_choice_capture(
        &mint_vault,
        "Finish @cash^x@\n- Existing ^idless\n- ^^\n",
        false,
        Some(&mint_day),
        &["1"],
        &[],
    );
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("mint real JSON");
    assert!(real_json["project_note"]["moved_tasks"][0]["detail"]
        .as_str()
        .unwrap()
        .ends_with(&format!("adds ^{mint}")));
    let destination = fs::read_to_string(mint_vault.join("cash_x.md"))
        .expect("minted destination");
    assert!(destination.contains(&format!("^{mint}")), "{destination}");
    let mut normalized_dry = dry_json;
    normalized_dry["dry_run"] = serde_json::json!(false);
    assert_eq!(normalized_dry, real_json);

    let descendant_parent = choice_parent(&[
        "- [ ] #task Parent ^parent",
        "  - [ ] #task Child ^prj",
    ]);
    let (_collision_temp, collision_vault, collision_day) = choice_vault(
        "bob-cli-choice-mint-descendant-collision",
        &descendant_parent,
        "## Pomodoros\n- [ ] (1400-1500) WORK\n",
    );
    let source_before = fs::read(collision_vault.join("cash.md"))
        .expect("source before collision");
    let collision = project_note_choice_capture(
        &collision_vault,
        "Finish @cash^x@\n- ^^\n",
        false,
        Some(&collision_day),
        &["1"],
        &[],
    );
    assert!(!collision.status.success());
    let collision_json: serde_json::Value =
        serde_json::from_str(stdout(&collision).trim())
            .expect("collision refusal JSON");
    assert_eq!(
        collision_json["code"], "move_task_id_collision",
        "{collision_json}"
    );
    assert_eq!(
        fs::read(collision_vault.join("cash.md")).expect("unchanged source"),
        source_before
    );
    assert!(!collision_vault.join("cash_x.md").exists());
}

#[test]
fn capture_project_note_choice_moves_are_not_successor_offers() {
    // MQ16: choice moves are excluded from successor offers, regardless of
    // sigil; newly authored project tasks remain eligible for -p.
    let parent = choice_parent(&["- [ ] #task Parent ^a"]);
    let day = "## Pomodoros\n- [ ] (1400-1500) WORK\n";
    let input = "Finish @cash:x@\n- New task\n- ^^\n";

    let (_move_temp, move_vault, move_day) =
        choice_vault("bob-cli-choice-offers-move", &parent, day);
    let moved = project_note_choice_capture(
        &move_vault,
        input,
        false,
        Some(&move_day),
        &["1"],
        &[],
    );
    assert_success(&moved);
    let moved_json: serde_json::Value =
        serde_json::from_str(stdout(&moved).trim()).expect("move JSON");
    let offers = moved_json["project_note"]["successors"]
        .as_array()
        .expect("successor offers");
    assert_eq!(offers.len(), 1, "{moved_json}");
    assert_eq!(offers[0]["text"], "New task");
    assert!(
        offers.iter().all(|row| row["text"] != "Parent"),
        "{offers:?}"
    );
    assert_eq!(
        fs::read_to_string(move_day).expect("day without offer selection"),
        day
    );

    let (_offer_temp, offer_vault, offer_day) =
        choice_vault("bob-cli-choice-offers-pick", &parent, day);
    let offer_pick = project_note_choice_capture(
        &offer_vault,
        input,
        false,
        Some(&offer_day),
        &["1"],
        &["all"],
    );
    assert_success(&offer_pick);
    let day_after = fs::read_to_string(offer_day).expect("selected offer day");
    assert!(day_after.contains("[[cash_x#^task]]"), "{day_after}");
    assert!(!day_after.contains("[[cash_x#^a]]"), "{day_after}");

    let (_colon_temp, colon_vault, colon_day) =
        choice_vault("bob-cli-choice-offers-colon", &parent, day);
    let colon_pick = project_note_choice_capture(
        &colon_vault,
        input,
        false,
        Some(&colon_day),
        &[":1"],
        &["all"],
    );
    assert_success(&colon_pick);
    let day_after = fs::read_to_string(colon_day).expect("both links day");
    assert!(day_after.contains("[[cash_x#^a]]"), "{day_after}");
    assert!(day_after.contains("[[cash_x#^task]]"), "{day_after}");
}

#[test]
fn capture_project_note_choice_matches_dated_project_and_name_guards() {
    // MQ14/MQ15: a future project keeps a Blocked pick unlinked until its
    // start, rejects Ready rows, and a : pick satisfies a required #name.
    let day = "## Pomodoros\n- [ ] (1400-1500) WORK\n";
    let blocked_parent = choice_parent(&["- [?] #task Blocked ^blocked"]);
    let (_blocked_temp, blocked_vault, blocked_day) =
        choice_vault("bob-cli-choice-dated-blocked", &blocked_parent, day);
    let blocked = project_note_choice_capture(
        &blocked_vault,
        "Finish @cash^x@ s:3\n- ^^\n",
        false,
        Some(&blocked_day),
        &[":1"],
        &[],
    );
    assert_success(&blocked);
    let blocked_json: serde_json::Value =
        serde_json::from_str(stdout(&blocked).trim()).expect("blocked JSON");
    let moved = &blocked_json["project_note"]["moved_tasks"][0];
    assert!(moved["link"].is_null(), "{blocked_json}");
    assert_eq!(
        moved["detail"],
        "stays unlinked until the project starts Sep 23"
    );
    assert_eq!(
        blocked_json["move_choice"]["slots"][0]["rows"][0]["answer"],
        ":"
    );

    let ready_parent = choice_parent(&[
        "- [ ] #task Ready ^ready",
        "- [?] #task Blocked ^blocked",
    ]);
    let (_ready_temp, ready_vault, ready_day) =
        choice_vault("bob-cli-choice-dated-ready", &ready_parent, day);
    let ready = project_note_choice_capture(
        &ready_vault,
        "Finish @cash^x@ s:3\n- ^^\n",
        false,
        Some(&ready_day),
        &[":1"],
        &[],
    );
    assert!(!ready.status.success());
    let ready_json: serde_json::Value =
        serde_json::from_str(stdout(&ready).trim()).expect("ready refusal");
    assert_eq!(ready_json["code"], "move_choice_refused");
    assert!(!ready_vault.join("cash_x.md").exists());

    let name_parent = choice_parent(&["- [ ] #task Named ^named"]);
    let (_name_temp, name_vault, name_day) =
        choice_vault("bob-cli-choice-name-success", &name_parent, day);
    let named = project_note_choice_capture(
        &name_vault,
        "Finish @cash^x@#work\n- ^^\n",
        false,
        Some(&name_day),
        &[":1"],
        &[],
    );
    assert_success(&named);
    let named_json: serde_json::Value =
        serde_json::from_str(stdout(&named).trim()).expect("name JSON");
    assert_eq!(
        named_json["move_choice"]["slots"][0]["name_needs_link"],
        true
    );
    assert_eq!(
        named_json["project_note"]["task_links"][0]["block_id"],
        "named"
    );
    assert!(fs::read_to_string(name_day)
        .expect("name linked day")
        .contains("[[cash_x#^named]]"));
}

#[test]
fn capture_project_note_choice_prediction_matches_each_execution_guard() {
    // MQ17: run every consequence-table state through ^^ for both sigils;
    // the preview suffix, link count, and answered row mirror execution.
    let day_work = "## Pomodoros\n- [ ] (1400-1500) WORK\n";
    let day_named_link =
        "## Pomodoros\n- [ ] (1400-1500) — WORK\n  - [[cash#^a]]\n";
    let day_unnamed_link = "## Pomodoros\n- [ ] (1400-1500)\n  - [[cash#^a]]\n";
    let parent_blocked_deps = choice_parent(&[
        "- [?] #task Blocked [dependsOn:: cash__b] [id:: cash__a] ^a",
        "  - ⛓️ **DEPENDS ON:** [[#^b]]",
        "- [ ] #task Prerequisite ^b",
    ]);
    let parent_blocked_date =
        choice_parent(&["- [?] #task Scheduled [scheduled:: 2026-09-25] ^a"]);
    let parent_other_reason = choice_parent(&[
        "- [?] #task Waiting [dependsOn:: cash__b] [id:: cash__a] [scheduled:: 2026-09-25] [scheduled:: 2026-09-26] ^a",
        "  - ⛓️ **DEPENDS ON:** [[#^b]]",
        "- [ ] #task Prerequisite ^b",
    ]);
    let parent_blocked_project = choice_parent(&["- [?] #task Blocked ^a"]);
    let parent_idless_ready = choice_parent(&["- [ ] #task Idless"]);
    let parent_idless_blocked = choice_parent(&["- [?] #task Idless"]);
    let parent_ready = choice_parent(&["- [ ] #task Row ^a"]);
    let parent_next = choice_parent(&["- [*] #task Row ^a"]);
    let parent_in_progress = choice_parent(&["- [/] #task Row ^a"]);
    let parent_named = choice_parent(&["- [ ] #task Row ^a"]);
    let parent_unnamed = choice_parent(&["- [ ] #task Row ^a"]);

    let cases: Vec<(&str, String, &str)> = vec![
        ("ready", parent_ready, "day"),
        ("next", parent_next, "day"),
        ("in-progress", parent_in_progress, "day"),
        ("linked-named", parent_named, "named"),
        ("linked-unnamed", parent_unnamed, "unnamed"),
        ("own-prerequisite", parent_blocked_deps, "day"),
        ("own-date", parent_blocked_date, "day"),
        ("other-reason", parent_other_reason, "day"),
        ("future-project", parent_blocked_project, "project"),
        ("idless-ready", parent_idless_ready, "day"),
        ("idless-blocked", parent_idless_blocked, "day"),
    ];

    for (case_name, parent, day_kind) in cases {
        for sigil in ['^', ':'] {
            let day = match day_kind {
                "named" => day_named_link,
                "unnamed" => day_unnamed_link,
                _ => day_work,
            };
            let (_temp, vault, day_file) = choice_vault(
                &format!("bob-cli-choice-matrix-{case_name}-{sigil}"),
                &parent,
                day,
            );
            let draft = if day_kind == "project" {
                "Finish @cash^x@ s:3\n- ^^\n"
            } else {
                "Finish @cash^x@\n- ^^\n"
            };
            let preview = project_note_choice_capture(
                &vault,
                draft,
                true,
                Some(&day_file),
                &[],
                &[],
            );
            assert_success(&preview);
            let preview_json: serde_json::Value =
                serde_json::from_str(stdout(&preview).trim())
                    .expect("preview JSON");
            let row = &preview_json["move_choice"]["slots"][0]["rows"][0];
            let choice = row["choices"]
                .as_array()
                .expect("choice options")
                .iter()
                .find(|option| option["sigil"] == sigil.to_string())
                .unwrap_or_else(|| panic!("{case_name} {sigil}: {row}"));
            let pick = format!("{sigil}1");
            let outcome = project_note_choice_capture(
                &vault,
                draft,
                false,
                Some(&day_file),
                &[&pick],
                &[],
            );
            assert_success(&outcome);
            let outcome_json: serde_json::Value =
                serde_json::from_str(stdout(&outcome).trim())
                    .expect("answered JSON");
            let moved = &outcome_json["project_note"]["moved_tasks"][0];
            let consequence =
                choice["consequence"].as_str().expect("consequence");
            let head = if sigil == '^' {
                "Moves into cash_x.md"
            } else {
                "Moves in"
            };
            let predicted = consequence
                .strip_prefix(head)
                .expect("consequence head")
                .trim_start_matches(" · ");
            let actual = moved["detail"].as_str();
            if predicted.is_empty() {
                assert!(
                    actual.is_none(),
                    "{case_name} {sigil}: {outcome_json}"
                );
            } else if case_name == "linked-named" && sigil == '^' {
                assert_eq!(
                    predicted.strip_prefix("its "),
                    actual,
                    "{outcome_json}"
                );
            } else {
                assert_eq!(
                    Some(predicted),
                    actual,
                    "{case_name} {sigil}: {outcome_json}"
                );
            }

            let minted_id = row["mint"].as_str();
            let moved_id = moved["block_id"].as_str().expect("moved block ID");
            if let Some(mint) = minted_id {
                assert_eq!(
                    moved_id, mint,
                    "{case_name} {sigil}: {outcome_json}"
                );
            }
            let link_added = outcome_json["project_note"]["task_links"]
                .as_array()
                .is_some_and(|links| {
                    links.iter().any(|link| link["block_id"] == moved_id)
                });
            assert_eq!(
                choice["adds_link"], link_added,
                "{case_name} {sigil}: {outcome_json}"
            );
            let slot = &outcome_json["move_choice"]["slots"][0];
            assert_eq!(slot["answered"], true, "{outcome_json}");
            assert_eq!(slot["moved_count"], 1, "{outcome_json}");
            assert_eq!(
                slot["linked_count"].as_u64(),
                Some(u64::from(link_added)),
                "{outcome_json}"
            );
            assert_eq!(
                slot["rows"][0]["answer"],
                sigil.to_string(),
                "{outcome_json}"
            );
        }
    }
}
