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
    let day_before = "## Pomodoros\n- [ ] (1330-1400 [t:: 30m]) Work\n";
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
    assert_eq!(json["block_link"], "[[dev#^preview]]");
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
