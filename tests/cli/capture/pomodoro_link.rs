//! Linked-task and named-pomodoro execution.

use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoro_linked_task_updates_both_notes_and_reports_json() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-json");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] Untimed planning\n",
            "- [ ] (**1330-1400** [t:: 30m]) Current work\n",
            "  - existing context\n",
            "## Later\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@Dev:foobar")
        .arg("Some")
        .arg("foobar")
        .arg("task.")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-linked capture");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "foobar");
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert_eq!(json["block_link"], "[[dev#^foobar]]");
    assert_eq!(json["placement"], "inserted");
    assert_eq!(json["pomodoro_link_placement"], "inserted");
    assert_eq!(
        json["task_line"],
        "- [*] #task Some foobar task. [created::2026-07-10] ^foobar"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [*] #task Some foobar task. [created::2026-07-10] ^foobar\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read daily note"),
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] Untimed planning\n",
            "- [ ] (**1330-1400** [t:: 30m]) Current work\n",
            "  - existing context\n",
            "  - [[dev#^foobar]]\n",
            "## Later\n",
        )
    );
}

#[test]
fn bare_named_link_suffix_writes_the_same_result_as_hash_before_equals() {
    let mut results = Vec::new();
    let mut snapshots = Vec::new();
    for (index, token) in ["@cash:work=focus@5", "@cash:work#focus=5"]
        .into_iter()
        .enumerate()
    {
        let temp =
            TempDir::new(&format!("bob-cli-capture-link-suffix-{index}"));
        let vault = temp.path().join("vault");
        let target = vault.join("cash.md");
        let day_file = vault.join("day.md");
        write_file(
            &target,
            "# Cash\n## Tasks\n- [ ] #task Work [id:: cash__work] ^work\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () — CURRENT\n  - context\n",
        );

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg(token)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:00:00")
            .output()
            .expect("run Pomodoro link");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["kind"], "pomodoro_link", "{json}");
        assert_eq!(json["pomodoro_name"], "FOCUS", "{json}");
        results.push(json);
        snapshots.push((
            fs::read_to_string(target).expect("read linked task"),
            fs::read_to_string(day_file).expect("read Pomodoro ledger"),
        ));
    }

    assert_eq!(snapshots[0], snapshots[1]);
    for json in &mut results {
        if let Some(object) = json.as_object_mut() {
            object.remove("input");
            object.remove("day_file");
            object.remove("target");
            if let Some(open_notes) =
                object.get_mut("open_notes").and_then(|v| v.as_array_mut())
            {
                for note in open_notes {
                    if let Some(note) = note.as_object_mut() {
                        note.remove("path");
                    }
                }
            }
        }
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn link_suffix_timing_errors_leave_notes_byte_identical() {
    for invalid in [
        "@cash:work=3#focus@2",
        "@cash:work=focus@",
        "^cash:work=3#focus@2",
        "^cash:work=focus@",
    ] {
        let temp = TempDir::new("bob-cli-capture-link-timing-no-write");
        let vault = temp.path().join("vault");
        let target = vault.join("cash.md");
        let day_file = vault.join("day.md");
        let target_before =
            "# Cash\n## Tasks\n- [ ] #task Work [id:: cash__work] ^work\n";
        let day_before = "## Pomodoros\n- [ ] () — CURRENT\n  - context\n";
        write_file(&target, target_before);
        write_file(&day_file, day_before);

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg(invalid)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:00:00")
            .output()
            .expect("run invalid link timing");
        assert!(
            !output.status.success(),
            "{invalid}: {}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read untouched target"),
            target_before,
            "{invalid}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("read untouched day"),
            day_before,
            "{invalid}"
        );
    }
    // A batch with an earlier valid item still writes nothing when a later
    // link timing fails.
    let temp = TempDir::new("bob-cli-capture-link-timing-batch-no-write");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    let target_before =
        "# Cash\n## Tasks\n- [ ] #task Work [id:: cash__work] ^work\n";
    let day_before = "## Pomodoros\n- [ ] () — CURRENT\n  - context\n";
    write_file(&target, target_before);
    write_file(&day_file, day_before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:work=focus@5")
        .arg("")
        .arg("@cash:work=3#focus@2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:00:00")
        .output()
        .expect("run batch with invalid link timing");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(&target).expect("read untouched target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read untouched day"),
        day_before
    );
}

#[test]
fn pomodoro_link_guard_consumes_an_aliased_live_link_by_identity() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-consume-alias");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\nstatus: wip\n---\n",
            "- [?] #task Ship it #prj #hide ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^x]]\n",
            "- [?] #task Grind ^grind\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^y]]\n",
        ),
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Ex prerequisite ^x\n- [ ] #task Why prerequisite ^y\n",
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - [[proj#^grind|stale alias]]\n",
            "  - keep context\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@proj:grind")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("guarded Pomodoro link capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "link_guard", "{json}");
    assert_eq!(json["status_symbol"], "?", "{json}");
    assert_eq!(json["status_changed"], false, "{json}");
    assert_eq!(
        json["link_guard"]["drop_refused"], "project_blocked",
        "{json}"
    );
    assert_eq!(
        json["consumed_links"].as_array().unwrap().len(),
        1,
        "{json}"
    );
    assert_eq!(
        json["consumed_links"][0]["link"], "[[proj#^grind|stale alias]]",
        "{json}"
    );
    assert_eq!(json["consumed_links"][0]["action"], "removed", "{json}");
    assert_eq!(
        json["consumed_links"][0]["pomodoro"]["removed"], false,
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        !day_after.contains("[[proj#^grind|stale alias]]"),
        "{day_after}"
    );
    assert!(day_after.contains("keep context"), "{day_after}");
    assert!(fs::read_to_string(vault.join("proj.md"))
        .unwrap()
        .contains("- [?] #task Grind ^grind"));
}

#[test]
fn capture_pomodoro_link_drops_dependencies_when_already_linked() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-guard-removed");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(
        &vault.join("prep.md"),
        "- [ ] #task Prep [id:: prep__prep] ^prep\n",
    );
    write_file(
        &target,
        concat!(
            "- [?] #task Blocked [dependsOn:: prep__prep] [id:: dev__blocked] ^blocked\n",
            "  - ⛓️ **DEPENDS ON:** [[prep#^prep]]\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[dev#^blocked]]\n",
        ),
    );

    let json_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@dev:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run guarded JSON Pomodoro link capture with cleanup");
    assert_success(&json_output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["status_symbol"], "*", "{json}");
    assert_eq!(json["status_changed"], true, "{json}");
    assert_eq!(json["pomodoro_link_action"], "already_current", "{json}");
    assert_eq!(json["dropped_dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(json["day_file"], day_file.display().to_string(), "{json}");
    assert!(json.get("link_guard").is_none(), "{json}");
    assert!(json.get("removed_pomodoro_links").is_none(), "{json}");
    let target_after =
        fs::read_to_string(&target).expect("read unblocked task");
    assert!(!target_after.contains("DEPENDS ON"), "{target_after}");
    assert!(
        target_after.contains("- [*] #task Blocked"),
        "{target_after}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(
        day_after.matches("[[dev#^blocked]]").count(),
        1,
        "{day_after}"
    );

    // Restore the pre-drop task so the human-output path reports the same
    // successful drop while the Pomodoro already contains the link.
    write_file(
        &target,
        concat!(
            "- [?] #task Blocked [dependsOn:: prep__prep] [id:: dev__blocked] ^blocked\n",
            "  - ⛓️ **DEPENDS ON:** [[prep#^prep]]\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] Current (0900-0930)\n",
            "  - [[dev#^blocked]]\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@dev:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run guarded human Pomodoro link capture");
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("✓ link"), "{human}");
    assert!(
        human.contains("dropped dependency on [[prep#^prep]]"),
        "{human}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(
        day_after.matches("[[dev#^blocked]]").count(),
        1,
        "{day_after}"
    );
}

#[test]
fn capture_pomodoro_link_drops_dependencies_without_existing_link() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-guard-empty");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(
        &vault.join("prep.md"),
        "- [ ] #task Prep [id:: prep__prep] ^prep\n",
    );
    write_file(
        &target,
        concat!(
            "- [?] #task Blocked [dependsOn:: prep__prep] [id:: dev__blocked] ^blocked\n",
            "  - ⛓️ **DEPENDS ON:** [[prep#^prep]]\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] Current (0900-0930)\n");

    let json_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@dev:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run JSON Pomodoro link capture");
    assert_success(&json_output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["status_symbol"], "*", "{json}");
    assert_eq!(json["status_changed"], true, "{json}");
    assert_eq!(json["dropped_dependencies"].as_array().unwrap().len(), 1);
    assert!(json.get("link_guard").is_none(), "{json}");
    assert!(json.get("removed_pomodoro_links").is_none(), "{json}");
    let target_after =
        fs::read_to_string(&target).expect("read unblocked task");
    assert!(!target_after.contains("DEPENDS ON"), "{target_after}");
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(
        day_after.matches("[[dev#^blocked]]").count(),
        1,
        "{day_after}"
    );

    // Restore the original task and empty ledger to exercise human output.
    write_file(
        &target,
        concat!(
            "- [?] #task Blocked [dependsOn:: prep__prep] [id:: dev__blocked] ^blocked\n",
            "  - ⛓️ **DEPENDS ON:** [[prep#^prep]]\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] Current (0900-0930)\n");
    let human_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@dev:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run human Pomodoro link capture");
    assert_success(&human_output);
    let human = stdout(&human_output);
    assert!(human.contains("✓ link"), "{human}");
    assert!(
        human.contains("dropped dependency on [[prep#^prep]]"),
        "{human}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read daily note");
    assert_eq!(
        day_after.matches("[[dev#^blocked]]").count(),
        1,
        "{day_after}"
    );
}

#[test]
fn capture_pomodoro_link_uses_default_day_file_and_untimed_fallback() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-default-day");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260710.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] Completed (1200-1230)\n- [ ] Next open\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Plan")
        .arg("work")
        .arg("@!Dev:next-work")
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run default-day Pomodoro capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("captured  dev.md"), "{out}");
    assert!(out.contains("linked"), "{out}");
    assert!(out.contains("[[dev#^next-work]]"), "{out}");
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read new route"),
        "- [*] #task Plan work [created::2026-07-10] ^next-work\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read default day"),
        concat!(
            "## Pomodoros\n",
            "- [x] Completed (1200-1230)\n",
            "- [ ] Next open\n",
            "  - [[dev#^next-work]]\n",
        )
    );
}

#[test]
fn capture_pomodoro_dry_run_validates_and_changes_neither_note() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-dry-run");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let target_before = "## Tasks\n- [ ] #task Existing\n";
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (1330-1400 [t:: 30m]) Work\n",
        "  - context\n",
        "  - [[cash#^blocked]]\n",
    );
    write_file(&target, target_before);
    write_file(&day_file, day_before);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("Preview")
        .arg("s:1")
        .arg("@dev:preview")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run dry-run Pomodoro capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["scheduled"], "2026-07-11");
    // PDG4: a new `:` task with s:N (N ≥ 1) is Blocked by its own
    // future schedule, so it is created unlinked with a report.
    assert!(json.get("block_link").is_none(), "{json}");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["local_schedule"]),
        "{json}"
    );
    assert!(
        json["task_line"]
            .as_str()
            .is_some_and(|line| line.starts_with("- [?] ")),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read untouched route"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read untouched day"),
        day_before
    );
}

#[test]
fn capture_pomodoro_preflight_failures_leave_both_notes_untouched() {
    let cases = [
        (
            "missing-section",
            "## Notes\n- [ ] (1330-1400) Outside\n",
            "- [ ] #task Existing\n",
            "no Pomodoros section",
        ),
        (
            "no-open",
            "## Pomodoros\n- [x] (1330-1400) Done\n",
            "- [ ] #task Existing\n",
            "no eligible open Pomodoro",
        ),
        (
            "ambiguous",
            "## Pomodoros\n- [ ] (1300-1330) One\n- [ ] (**1330-1400**) Two\n",
            "- [ ] #task Existing\n",
            "multiple open timed Pomodoros",
        ),
        (
            "duplicate-id",
            "## Pomodoros\n- [ ] (1330-1400) Work\n",
            "- [ ] #task Existing ^dup\n",
            "block ID ^dup already exists",
        ),
    ];

    for (name, day_before, target_before, expected_error) in cases {
        let temp = TempDir::new(&format!("bob-cli-capture-pomodoro-{name}"));
        let vault = temp.path().join("vault");
        let target = vault.join("dev.md");
        let day_file = vault.join("day.md");
        write_file(&target, target_before);
        write_file(&day_file, day_before);

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("Do")
            .arg("work")
            .arg(if name == "duplicate-id" {
                "@dev:dup"
            } else {
                "@dev:new-id"
            })
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run failing Pomodoro capture");

        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
        assert_eq!(json["ok"], false, "{name}");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|error| error.contains(expected_error)),
            "{name}: {json}"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read untouched target"),
            target_before,
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("read untouched day"),
            day_before,
            "{name}"
        );
    }
}

#[test]
fn capture_pomodoro_missing_daily_note_does_not_create_target() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-missing-day");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let missing_day = vault.join("missing-day.md");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Do")
        .arg("work")
        .arg("@dev:new-id")
        .env("BOB_DAY_FILE", &missing_day)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run capture with missing day");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(stderr(&output).contains("daily note does not exist"));
    assert!(!vault.join("dev.md").exists());
}

#[test]
fn capture_malformed_pomodoro_marker_is_usage_error_without_writes() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-malformed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Do")
        .arg("work")
        .arg("@dev:")
        .output()
        .expect("run malformed Pomodoro capture");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(stderr(&output).contains("block ID must be non-empty"));
    assert_eq!(fs::read_dir(&vault).expect("read vault").count(), 0);
}

#[test]
fn capture_named_pomodoro_updates_both_notes_and_skips_current() {
    let temp = TempDir::new("bob-cli-capture-named-pomodoro");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — BUGS\n",
            "## Later\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@Dev:foobar#bugs")
        .arg("Some")
        .arg("foobar")
        .arg("task.")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run named Pomodoro capture");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["block_id"], "foobar");
    assert_eq!(json["block_link"], "[[dev#^foobar]]");
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [*] #task Some foobar task. [created::2026-07-10] ^foobar\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read daily note"),
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — BUGS\n",
            "  - [[dev#^foobar]]\n",
            "## Later\n",
        )
    );
}

#[test]
fn capture_named_pomodoro_dry_run_and_failures_leave_notes_untouched() {
    let temp = TempDir::new("bob-cli-capture-named-pomodoro-dry-run");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let target_before = "## Tasks\n- [ ] #task Existing\n";
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
        "- [ ] () — BUGS\n",
    );
    write_file(&target, target_before);
    write_file(&day_file, day_before);

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@dev:preview#new-name")
        .arg("Preview")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run named dry-run");
    assert_success(&dry_run);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["block_link"], "[[dev#^preview]]");
    assert_eq!(
        fs::read_to_string(&target).expect("untouched route"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("untouched day"),
        day_before
    );

    for (name, marker, created_name) in [
        ("unknown", "@dev:new-id#zzzz", "ZZZZ"),
        ("completed", "@dev:new-id#done", "DONE"),
    ] {
        let case_dir = TempDir::new(&format!("bob-cli-capture-named-{name}"));
        let vault = case_dir.path().join("vault");
        let target = vault.join("dev.md");
        let day_file = vault.join("day.md");
        write_file(&target, target_before);
        write_file(
            &day_file,
            concat!(
                "## Pomodoros\n",
                "- [x] () — DONE\n",
                "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
                "- [ ] () — BUGS\n",
            ),
        );
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(marker)
            .arg("Do work")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run named capture with new future Pomodoro");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("success JSON");
        assert_eq!(json["ok"], true, "{name}");
        assert_eq!(json["block_link"], "[[dev#^new-id]]", "{name}");
        assert_eq!(
            fs::read_to_string(&target).expect("untouched target"),
            concat!(
                "## Tasks\n",
                "- [ ] #task Existing\n",
                "- [*] #task Do work [created::2026-07-10] ^new-id\n",
            ),
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("untouched day"),
            format!(
                concat!(
                    "## Pomodoros\n",
                    "- [x] () — DONE\n",
                    "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
                    "- [ ] () — {}\n",
                    "  - [[dev#^new-id]]\n",
                    "- [ ] () — BUGS\n",
                ),
                created_name
            ),
            "{name}"
        );
    }

    let ambiguous_dir =
        TempDir::new("bob-cli-capture-named-pomodoro-ambiguous-current");
    let vault = ambiguous_dir.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let target_before = "## Tasks\n- [ ] #task Existing\n";
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
        "- [ ] (**1400-1430** [t:: 30m]) — OTHER\n",
    );
    write_file(&target, target_before);
    write_file(&day_file, day_before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@dev:new-id#zzzz")
        .arg("Do work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run ambiguous-current named capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"].as_str().is_some_and(
            |error| error.contains("multiple open timed Pomodoros")
        ),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("untouched target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("untouched day"),
        day_before
    );
}

#[test]
fn capture_named_pomodoro_batch_reuses_new_placeholder() {
    let temp = TempDir::new("bob-cli-capture-named-pomodoro-batch");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "## Tasks\n");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
        ),
    );

    let input =
        "First @dev:first#after-tui-fix\n\nSecond @dev:second#after-tui-fix\n";
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00"),
        input,
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["block_link"], "[[dev#^first]]");
    assert_eq!(captures[1]["block_link"], "[[dev#^second]]");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read daily note"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — AFTER-TUI-FIX\n",
            "  - [[dev#^first]]\n",
            "  - [[dev#^second]]\n",
        )
    );
}

#[test]
fn capture_pomodoro_task_reports_running_entry_block() {
    // A Pomodoro task capture links into the running entry: the planner
    // pushes the destination `linked` ref.
    let temp = TempDir::new("bob-cli-capture-pomodoro-task-blocks");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# Sase\n## Tasks\n");
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930** [t:: 30m]) — RUN\n",
        "  - context\n",
    );
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["Write outline @sase:outline"],
    );
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["block_link"], "[[sase#^outline]]");
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 2,
                "name": "RUN",
                "time_range": "0900-0930",
                "status": "running",
                "created": false,
                "roles": ["linked"],
                "lines": [
                    {
                        "text": "- [ ] (**0900-0930** [t:: 30m]) — RUN",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - context",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "  - [[sase#^outline]]",
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
            "- [ ] (**0900-0930** [t:: 30m]) — RUN\n",
            "  - context\n",
            "  - [[sase#^outline]]\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_pomodoro_link_reports_move_destination_first() {
    // A queued link moved to `#gtd` reports the destination first and
    // then the source with its removed line.
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-move");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "- [ ] #task Do X ^x\n");
    let day_before = "## Pomodoros\n\
         - [ ] (**0900-0930** [t:: 30m]) — RUN\n\
         - [ ] () — QUEUE\n\
         \t- [[sase#^x]]\n\
         - [ ] () — GTD\n";
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["^sase:x#gtd"],
    );
    assert_eq!(json["kind"], "pomodoro_link");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 4,
                "name": "GTD",
                "status": "queued",
                "created": false,
                "roles": ["linked"],
                "lines": [
                    {
                        "text": "- [ ] () — GTD",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- [[sase#^x]]",
                        "depth": 1,
                        "change": "added",
                    },
                ],
            },
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "QUEUE",
                "status": "queued",
                "created": false,
                "roles": ["unlinked"],
                "lines": [
                    {
                        "text": "- [ ] () — QUEUE",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- [[sase#^x]]",
                        "depth": 1,
                        "change": "removed",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read moved day");
    assert_eq!(
        day_after,
        "## Pomodoros\n\
         - [ ] (**0900-0930** [t:: 30m]) — RUN\n\
         - [ ] () — QUEUE\n\
         - [ ] () — GTD\n\
         \t- [[sase#^x]]\n"
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

#[test]
fn capture_pomodoro_link_reports_created_named_destination() {
    // A link that creates its named Pomodoro reports a created block
    // whose lines are all added.
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-created");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &vault.join("dev.md"),
        "# Dev\n## Tasks\n- [ ] #task Existing\n",
    );
    let day_before = "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — RUN\n";
    write_file(&day_file, day_before);

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-07-10 10:00:00",
        &["@Dev:brand#new-name New task body."],
    );
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "NEW-NAME",
                "status": "queued",
                "created": true,
                "roles": ["linked"],
                "lines": [
                    {
                        "text": "- [ ] () — NEW-NAME",
                        "depth": 0,
                        "change": "added",
                    },
                    {
                        "text": "  - [[dev#^brand]]",
                        "depth": 1,
                        "change": "added",
                    },
                ],
            },
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read created day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — RUN\n",
            "- [ ] () — NEW-NAME\n",
            "  - [[dev#^brand]]\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(day_before, &day_after, &json);
}

// PDG18/20/21 (§13.15): a safe own-dependency blocker is removed before
// the ordinary solo link, in the same staged capture.
#[test]
fn capture_pomodoro_link_drops_own_dependency_and_links_atomically() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-drop");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let target_before = concat!(
        "- [?] #task Blocked work [id:: cash__blocked] [dependsOn:: job__prep] [scheduled:: 2026-07-20] ^blocked\n",
        "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
    );
    write_file(&target, target_before);
    write_file(&vault.join("job.md"), "- [ ] #task Prep work ^prep\n");
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - context\n",
        "- [ ] () — LATER\n",
        "  - [[cash#^blocked]]\n",
    );
    write_file(&day_file, day_before);

    let dry_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("@cash:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("dry-run dependency drop and link");
    assert_success(&dry_output);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_output).trim())
            .expect("dry-run link JSON");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:blocked")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("link dependency-blocked task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("link JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(
        json["previous_task_line"],
        target_before.lines().next().unwrap()
    );
    assert_eq!(json["previous_status_symbol"], "?");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["removed_scheduled"], "2026-07-20");
    assert!(json.get("link_guard").is_none(), "{json}");
    assert_eq!(json["dropped_dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["dropped_dependencies"][0],
        serde_json::json!({
            "note_path": "job.md",
            "block_id": "prep",
            "link": "[[job#^prep]]",
            "text": "Prep work",
            "status_symbol": " ",
            "status_name": "Ready",
        }),
        "{json}"
    );
    assert_eq!(
        json["task_blocks"][0]["roles"],
        serde_json::json!(["dependency_dropped"]),
        "{json}"
    );
    let mut normalized_dry = dry_json;
    normalized_dry["dry_run"] = serde_json::json!(false);
    assert_eq!(
        normalized_dry, json,
        "dry run plans the same dependency drop"
    );
    let target_after = fs::read_to_string(&target).expect("read target");
    assert!(
        target_after.contains("- [*] #task Blocked work"),
        "{target_after}"
    );
    assert!(
        target_after.contains("[fresh:: 2026-07-10]"),
        "{target_after}"
    );
    assert!(!target_after.contains("dependsOn::"), "{target_after}");
    assert!(!target_after.contains("DEPENDS ON"), "{target_after}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(day_after.contains("[[cash#^blocked]]"), "{day_after}");
    assert!(day_after.contains("context"), "{day_after}");
    assert_eq!(
        fs::read_to_string(vault.join("job.md")).expect("read prereq"),
        "- [ ] #task Prep work ^prep\n"
    );
}

#[test]
fn dependency_drop_reasons_report_schedule_and_managed_line_refusals() {
    let cases = [
        (
            "schedule",
            concat!(
                "- [?] #task Blocked [id:: cash__blocked] [dependsOn:: job__prep] [scheduled:: 2026-07-20] [scheduled:: 2026-07-21] ^blocked\n",
                "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
            ),
            "schedule",
            "several [scheduled::] fields still block it",
        ),
        (
            "duplicate-lines",
            concat!(
                "- [?] #task Blocked [id:: cash__blocked] [dependsOn:: job__prep] ^blocked\n",
                "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
                "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
            ),
            "dependency_line",
            "repair its Depends-On line first",
        ),
    ];
    for (name, target_before, code, phrase) in cases {
        let temp =
            TempDir::new(&format!("bob-cli-capture-drop-refused-{name}"));
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        let target = vault.join("cash.md");
        write_file(&target, target_before);
        write_file(
            &vault.join("job.md"),
            "- [ ] #task Prep [id:: job__prep] ^prep\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n  - context\n",
        );

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("@cash:blocked")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("capture refused dependency drop");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["kind"], "link_guard", "{json}");
        assert_eq!(json["link_guard"]["drop_refused"], code, "{json}");
        assert!(
            json["link_guard"]["detail"]
                .as_str()
                .unwrap_or_default()
                .contains(phrase),
            "{json}"
        );
        assert_eq!(fs::read_to_string(target).unwrap(), target_before);
    }
}

#[test]
fn failed_start_rolls_back_a_staged_dependency_drop_for_real_and_dry_runs() {
    let temp = TempDir::new("bob-cli-capture-drop-start-rollback");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let target_before = concat!(
        "- [?] #task Blocked [id:: cash__blocked] [dependsOn:: job__prep] ^blocked\n",
        "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
    );
    let day_before = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
        "  - context\n",
    );
    write_file(&target, target_before);
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    write_file(&day_file, day_before);

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
            .arg("@cash:blocked=5")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("capture start should fail while a timed Pomodoro is open")
    };
    let dry = run(true);
    let real = run(false);
    assert!(!dry.status.success(), "{}", format_output(&dry));
    assert!(!real.status.success(), "{}", format_output(&real));
    assert!(
        format_output(&dry).contains("finish the current Pomodoro first"),
        "{}",
        format_output(&dry)
    );
    assert_eq!(stdout(&dry), stdout(&real));
    assert_eq!(stderr(&dry), stderr(&real));
    assert_eq!(fs::read_to_string(&target).unwrap(), target_before);
    assert_eq!(fs::read_to_string(&day_file).unwrap(), day_before);
}

#[test]
fn dependency_drop_supports_named_pomodoro_start() {
    let temp = TempDir::new("bob-cli-capture-drop-named-start");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [?] #task Work [id:: cash__work] [dependsOn:: job__prep] ^work\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
        ),
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — CURRENT\n  - context\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:work#focus=5")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:00:00")
        .output()
        .expect("drop prerequisites and start named Pomodoro");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["dropped_dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(json["pomodoro_name"], "FOCUS", "{json}");
    assert!(json.get("pomodoro_start").is_some(), "{json}");
    let day = fs::read_to_string(day_file).expect("read started session");
    assert!(day.contains("FOCUS"), "{day}");
    assert!(day.contains("[[cash#^work]]"), "{day}");
}

#[test]
fn dependency_drop_human_output_names_the_removed_link() {
    let temp = TempDir::new("bob-cli-capture-drop-human-output");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [?] #task Work [id:: cash__work] [dependsOn:: job__prep] ^work\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
        ),
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — CURRENT\n  - context\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@cash:work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:00:00")
        .output()
        .expect("capture dependency drop with human output");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("⛓️ dropped dependency on [[job#^prep]]"),
        "{human}"
    );
}

#[test]
fn dependency_drop_keeps_closed_and_unresolved_links_verbatim() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-link-drop-partial");
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [?] #task Work [id:: cash__work] [dependsOn:: job__prep, done__done, ghost__missing] ^work\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep|Prep alias]] • [[done#^done|Done alias]] • [[ghost#^missing]]\n",
        ),
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    write_file(
        &vault.join("done.md"),
        "- [x] #task Done [id:: done__done] ^done\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n  - context\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash:work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("capture link with historical and unresolved dependency links");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(
        json["dropped_dependencies"][0]["link"], "[[job#^prep|Prep alias]]",
        "{json}"
    );
    let after = fs::read_to_string(&target).expect("read dependent");
    assert!(after.contains("[[done#^done|Done alias]]"), "{after}");
    assert!(after.contains("[[ghost#^missing]]"), "{after}");
    assert!(
        after.contains("dependsOn:: done__done, ghost__missing"),
        "{after}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("done.md")).expect("read closed target"),
        "- [x] #task Done [id:: done__done] ^done\n"
    );
}

// A new `:` task with an open `&` prerequisite lands `[?]` unlinked:
// the prerequisite rises and the result reports `not_linked` (§13.5).
#[test]
fn capture_pomodoro_task_with_open_dependency_is_created_unlinked() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-task-dep-unlinked");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Seed work ^seed\n");
    let day_before = "## Pomodoros\n- [ ] (1330-1400 [t:: 30m]) Work\n";
    write_file(&day_file, day_before);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Water fern")
        .arg("@dev:fern")
        .arg("&cash:seed")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("capture : task with open dep");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("link JSON");
    assert_eq!(json["kind"], "pomodoro_task", "{json}");
    assert_eq!(json["block_id"], "fern", "{json}");
    assert!(json.get("block_link").is_none(), "{json}");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["local_dependency"]),
        "{json}"
    );
    let task_line = json["task_line"].as_str().unwrap_or_default();
    assert!(task_line.starts_with("- [?] "), "{json}");
    assert!(task_line.ends_with("^fern"), "{json}");
    let contents =
        fs::read_to_string(vault.join("dev.md")).expect("read new note");
    assert!(contents.contains(task_line), "{contents}");
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);
    // BW13: the prerequisite's BLOCKS mirror stages in the same batch,
    // even when the `:` task itself stays unlinked.
    assert_eq!(
        json["blocks_line_updates"][0]["added"],
        serde_json::json!(["[[dev#^fern]]"]),
        "{json}",
    );
    assert!(json.get("minted_dependent_block_id").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read prereq"),
        "- [*] #task Seed work [id:: cash__seed] ^seed\n\t- 🧱 **BLOCKS:** [[dev#^fern]]\n"
    );
}
