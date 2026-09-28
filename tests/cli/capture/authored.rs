//! Authored bullet tests.

use super::priority::write_priority_config;
use crate::support::*;
use std::fs;

#[test]
fn capture_authored_bullets_render_task_with_children_in_order() {
    let temp = TempDir::new("bob-cli-capture-authored-bullets");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@work Prepare the launch review\n- Confirm the rollout owner\n- Attach the final checklist")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task Prepare the launch review [created::2026-06-15]\n",
            "\t- Confirm the rollout owner\n",
            "\t- Attach the final checklist\n",
        )
    );

    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!([
            "\t- Confirm the rollout owner",
            "\t- Attach the final checklist"
        ])
    );
    assert_eq!(
        json["task_line"],
        "- [ ] #task Prepare the launch review [created::2026-06-15]"
    );

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work second parent\n- only child")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run authored-bullet human capture");
    assert_success(&human);
    assert!(
        stdout(&human).contains("\t- only child"),
        "human output should print the authored child:\n{}",
        format_output(&human)
    );
}

#[test]
fn capture_authored_nested_bullets_render_under_their_owners() {
    let temp = TempDir::new("bob-cli-capture-authored-nested");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@work Prepare the launch review\n- Confirm the rollout owner\n  - Send the owner the final date\n- Attach the final checklist\n  - Verify the links")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run nested authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task Prepare the launch review [created::2026-06-15]\n",
            "\t- Confirm the rollout owner\n",
            "\t\t- Send the owner the final date\n",
            "\t- Attach the final checklist\n",
            "\t\t- Verify the links\n",
        )
    );

    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!([
            "\t- Confirm the rollout owner",
            "\t\t- Send the owner the final date",
            "\t- Attach the final checklist",
            "\t\t- Verify the links"
        ])
    );
}

#[test]
fn capture_authored_bullets_accept_dash_star_and_plus_markers() {
    let temp = TempDir::new("bob-cli-capture-authored-markers");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent line\n- dash child\n* star child\n+ plus child")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run mixed-marker authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task parent line [created::2026-06-15]\n",
            "\t- dash child\n",
            "\t- star child\n",
            "\t- plus child\n",
        )
    );
}

#[test]
fn capture_authored_bullets_preserve_inline_markdown_and_checkbox_text() {
    let temp = TempDir::new("bob-cli-capture-authored-markdown");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent\n- **bold** and `code`\n- [ ] looks like a checkbox")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run markdown-preserving authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task parent [created::2026-06-15]\n",
            "\t- **bold** and `code`\n",
            "\t- [ ] looks like a checkbox\n",
        )
    );
}

#[test]
fn capture_authored_bullets_skip_placeholder_lines() {
    let temp = TempDir::new("bob-cli-capture-authored-placeholders");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent\n- real child\n- \n-\t\n")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run placeholder-tolerant authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        concat!(
            "- [ ] #task parent [created::2026-06-15]\n",
            "\t- real child\n",
        )
    );
}

#[test]
fn capture_authored_bullets_reject_indented_or_nonbullet_lines() {
    let cases = [
        ("one-space", "parent\n - nested item"),
        ("too-deep", "parent\n   - nested item"),
        ("prose", "parent\n- real child\ncontinuation prose"),
    ];

    for (name, text) in cases {
        let temp =
            TempDir::new(&format!("bob-cli-capture-authored-invalid-{name}"));
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg(format!("@work {text}"))
            .env("BOB_NOW", "2026-06-15")
            .output()
            .expect("run invalid authored-bullet capture");

        assert_eq!(output.status.code(), Some(2), "{name}");
        assert!(
            stderr(&output).contains("must be a column-zero bullet"),
            "{name}: {}",
            format_output(&output)
        );
        assert!(!vault.join("work.md").exists(), "{name}");
    }
}

#[test]
fn capture_authored_bullets_reject_orphaned_nested_lines() {
    let temp = TempDir::new("bob-cli-capture-authored-orphan-nested");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent\n  - orphan")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run orphaned nested authored-bullet capture");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("nested bullet")
            && stderr(&output).contains("no preceding first-level"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("work.md").exists());
}

#[test]
fn capture_authored_bullets_reject_item_emptied_by_markers() {
    let temp = TempDir::new("bob-cli-capture-authored-empty-after-markers");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("parent text\n- s:1")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run empty-after-markers authored-bullet capture");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "bob capture: capture item 1 starting on line 1: capture line 2 has no text left after its capture markers were removed"
    );
    assert!(!vault.join("mac_inbox.md").exists());
}

#[test]
fn capture_authored_bullet_marker_on_child_line_configures_whole_capture() {
    let temp = TempDir::new("bob-cli-capture-authored-child-marker");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Prepare the launch review\n- Confirm the rollout owner\n- Attach the final checklist @work s:3")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run child-line-marker authored-bullet capture");

    assert_success(&output);
    let content =
        fs::read_to_string(vault.join("work.md")).expect("read work route");
    assert!(
        content.starts_with(
            "- [?] #task Prepare the launch review [created::2026-06-15] [scheduled::2026-06-18]"
        ),
        "unexpected content:\n{content}"
    );
    assert!(
        content.contains("\t- Confirm the rollout owner\n"),
        "unexpected content:\n{content}"
    );
    assert!(
        content.contains("\t- Attach the final checklist\n"),
        "child line should drop its item-wide markers:\n{content}"
    );
    assert!(
        !content.contains('@'),
        "the route marker must not leak into rendered text:\n{content}"
    );
}

#[test]
fn capture_authored_bullets_duplicate_schedule_marker_across_lines_is_usage_error(
) {
    let temp = TempDir::new("bob-cli-capture-authored-duplicate-schedule");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent s:1\n- child s:2")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run duplicate-schedule authored-bullet capture");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("schedule marker")
            && stderr(&output).contains("only one line"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("work.md").exists());
}

#[test]
fn capture_authored_bullets_duplicate_route_marker_across_lines_is_usage_error()
{
    let temp = TempDir::new("bob-cli-capture-authored-duplicate-route");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@work parent line\n- child @home")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run duplicate-route authored-bullet capture");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("route/mode marker")
            && stderr(&output).contains("only one line"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("work.md").exists());
    assert!(!vault.join("home.md").exists());
}

#[test]
fn capture_authored_bullets_bullet_capture_uses_section_prefix_with_children() {
    let temp = TempDir::new("bob-cli-capture-authored-bullet-section");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("notes.md"),
        "# Ideas\n- existing idea\n\n# Other\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@notes#Ideas jot an idea\n- with a detail")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run bullet authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("notes.md")).expect("read notes"),
        concat!(
            "# Ideas\n",
            "- existing idea\n",
            "- jot an idea [created::2026-06-15]\n",
            "\t- with a detail\n",
            "\n# Other\n",
        )
    );
}

#[test]
fn capture_authored_bullets_pomodoro_linked_capture_includes_children() {
    let temp = TempDir::new("bob-cli-capture-authored-pomodoro");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**1330-1400** [t:: 30m]) Current work\n",
            "## Later\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@Dev:foobar Some foobar task.\n- first detail\n- second detail")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-linked authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [*] #task Some foobar task. [created::2026-07-10] ^foobar\n",
            "\t- first detail\n",
            "\t- second detail\n",
        )
    );
}

#[test]
fn capture_authored_bullets_sub_bullet_nests_children_two_levels() {
    let temp = TempDir::new("bob-cli-capture-authored-sub-bullet");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "## Tasks\n- [*] #task Parent [created::2026-07-01] ^parent\nTail\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@cash+parent new note\n- nested detail")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run sub-bullet authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "## Tasks\n",
            "- [*] #task Parent [created::2026-07-01] ^parent\n",
            "\t- new note\n",
            "\t\t- nested detail\n",
            "Tail\n",
        )
    );
}

#[test]
fn capture_authored_bullets_use_two_space_target_indentation() {
    let temp = TempDir::new("bob-cli-capture-authored-two-space");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("spaced.md"),
        "# Spaced\n- [ ] #task Existing\n  - existing child\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@spaced parent\n- child one\n  - detail")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run two-space-indent authored-bullet capture");

    assert_success(&output);
    assert!(
        fs::read_to_string(vault.join("spaced.md"))
            .expect("read spaced note")
            .contains(concat!(
                "- [ ] #task parent [created::2026-06-15]\n",
                "  - child one\n",
                "    - detail\n",
            )),
        "expected two-space child indentation"
    );
}

#[test]
fn capture_authored_bullets_preserve_crlf_line_endings() {
    let temp = TempDir::new("bob-cli-capture-authored-crlf");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dev.md"), "# Dev\r\n- existing\r\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@dev parent line\n- child line")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run CRLF authored-bullet capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev note"),
        concat!(
            "# Dev\r\n",
            "- existing\r\n",
            "- [ ] #task parent line [created::2026-06-15]\r\n",
            "\t- child line\r\n",
        )
    );
}

#[test]
fn capture_authored_bullets_dry_run_reports_children_without_writing() {
    let temp = TempDir::new("bob-cli-capture-authored-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("@work parent\n- child one\n  - child detail\n- child two")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run dry-run authored-bullet capture");

    assert_success(&output);
    assert!(!vault.join("work.md").exists());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!([
            "\t- child one",
            "\t\t- child detail",
            "\t- child two"
        ])
    );
}

#[test]
fn capture_bare_terminal_marker_authored_children_nest_one_level_deeper() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-children");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] Current (0900-0930)\n  - existing child\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Parent #\n- child detail")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with an authored child");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - existing child\n",
            "  - Parent\n",
            "    - child detail\n",
        )
    );
}

#[test]
fn capture_task_section_authored_children_and_composition() {
    let original = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t\t- existing\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — keep this entry\n",
    );
    let temp = TempDir::new("bob-cli-task-section-composition");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let config = temp.path().join("config.yml");
    write_file(&vault.join("cash.md"), original);
    write_executable(&clipboard, "#!/bin/sh\nprintf 'clip child\\n'\n");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+parent#requirements buy milk p:2 %\n- authored child")
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("run composed task-section capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["parent_section"], "REQUIREMENTS");
    assert_eq!(
        json["task_line"],
        "- buy milk [priority::medium] [scheduled::2026-07-21]"
    );
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- authored child"])
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t\t- buy milk [priority::medium] [scheduled::2026-07-21]\n",
            "\t\t\t- authored child\n",
            "\t\t\t- clip child\n",
            "\t\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t\t- *2026-07-21* — 🎲 P0 → P2 · in **11** (8–30) days\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep this entry\n",
        ),
        "{}",
        format_output(&output)
    );

    let scheduled = TempDir::new("bob-cli-task-section-scheduled");
    let vault = scheduled.path().join("vault");
    write_file(&vault.join("cash.md"), original);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("ship it s:2 @cash+parent#requirements")
        .env("BOB_NOW", "2026-07-10")
        .output()
        .expect("run scheduled task-section capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t\t- ship it [scheduled::2026-07-12]\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep this entry\n",
        )
    );

    let clipped = TempDir::new("bob-cli-task-section-clip-flag");
    let vault = clipped.path().join("vault");
    let clipboard = clipped.path().join("clipboard");
    write_file(&vault.join("cash.md"), original);
    write_executable(&clipboard, "#!/bin/sh\nprintf 'forced clip\\n'\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip")
        .arg("--")
        .arg("from clip @cash+parent#requirements")
        .env("BOB_NOW", "2026-07-31")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("run --clip task-section capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t\t- from clip\n",
            "\t\t\t- forced clip\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep this entry\n",
        )
    );
}
