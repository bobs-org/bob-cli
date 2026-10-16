//! Capture priority and three priority helpers.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Output;

pub(crate) fn write_priority_config(path: &Path) {
    write_file(
        path,
        concat!(
            "properties:\n",
            "  - name: scheduled\n",
            "    values: date\n",
            "  - name: dependsOn\n",
            "    values: local_task_id\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
            "      - label: P2\n",
            "        value: medium\n",
            "        min_days: 8\n",
            "        max_days: 30\n",
            "      - label: P3\n",
            "        value: low\n",
            "        min_days: 31\n",
            "        max_days: 90\n",
            "      - label: P4\n",
            "        value: lowest\n",
            "        min_days: 91\n",
            "        max_days: 365\n",
        ),
    );
}

#[test]
fn capture_priority_level_one_rolls_scheduled_date_in_window() {
    let temp = TempDir::new("bob-cli-capture-priority-p1");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("$1")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $1 capture");

    assert_success(&output);
    let task_line =
        "- [?] #task buy milk [created::2026-06-15] [priority::high] [scheduled::2026-06-22]";
    let expected = concat!(
        "- [?] #task buy milk [created::2026-06-15] [priority::high] [scheduled::2026-06-22]\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-06-22* — 🎲 P0 → P1 · in **7** (2–7) days\n",
    );
    assert!(
        stdout(&output).contains(task_line),
        "unexpected capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        expected
    );

    let scheduled = priority_scheduled_offset_days(&output, "2026-06-15");
    assert_eq!(scheduled, 7, "seeded P1 offset should match the log");
    assert!(
        (2..=7).contains(&scheduled),
        "expected P1 offset within [2, 7], got {scheduled}"
    );
}

#[test]
fn capture_priority_level_four_rolls_scheduled_date_in_window() {
    let temp = TempDir::new("bob-cli-capture-priority-p4");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("someday")
        .arg("idea")
        .arg("$4")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $4 capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("[priority::lowest]"),
        "unexpected capture output:\n{out}"
    );
    let scheduled = priority_scheduled_offset_days(&output, "2026-06-15");
    assert_eq!(scheduled, 331, "seeded P4 offset should match the log");
    assert!(
        (91..=365).contains(&scheduled),
        "expected P4 offset within [91, 365], got {scheduled}"
    );

    let expected = concat!(
        "- [?] #task someday idea [created::2026-06-15] [priority::lowest] [scheduled::2027-05-12]\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2027-05-12* — 🎲 P0 → P4 · in **331** (91–365) days\n",
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        expected
    );
}

#[test]
fn capture_priority_with_explicit_schedule_skips_roll() {
    let temp = TempDir::new("bob-cli-capture-priority-explicit-schedule");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("$2")
        .arg("s:1")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run $2 s:1 capture");

    assert_success(&output);
    let expected =
        "- [?] #task buy milk [created::2026-06-15] [priority::medium] [scheduled::2026-06-16]";
    assert!(
        stdout(&output).contains(expected),
        "unexpected capture output:\n{}",
        format_output(&output)
    );
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert_eq!(inbox, format!("{expected}\n"));
    assert!(
        !inbox.contains("SCHEDULE LOG") && !inbox.contains('\u{1F3B2}'),
        "explicit s:<N> must not roll a date, so no schedule log entry: {inbox}"
    );
}

#[test]
fn capture_priority_out_of_range_is_usage_error_without_writes() {
    let temp = TempDir::new("bob-cli-capture-priority-out-of-range");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    for level in ["$9", "$0"] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("buy")
            .arg("milk")
            .arg(level)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .unwrap_or_else(|error| panic!("run {level} capture: {error}"));

        assert_eq!(
            output.status.code(),
            Some(2),
            "{level}: {}",
            format_output(&output)
        );
        let err = stderr(&output);
        assert!(
            err.contains("$ or $1 through $4")
                && err.contains("P1, P2, P3, P4"),
            "{level}: unexpected stderr:\n{err}"
        );
        assert_eq!(
            fs::read_dir(&vault).expect("read vault").count(),
            0,
            "{level}: expected no files written"
        );
    }
}

#[test]
fn capture_priority_missing_config_file_is_io_error_without_writes() {
    let temp = TempDir::new("bob-cli-capture-priority-missing-config");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("does-not-exist/config.yml");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("$2")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run $2 capture with missing config");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains(&config.display().to_string()),
        "unexpected stderr:\n{err}"
    );
    assert!(err.contains("chezmoi apply"), "unexpected stderr:\n{err}");
    assert_eq!(fs::read_dir(&vault).expect("read vault").count(), 0);
}

#[test]
fn capture_priority_json_includes_priority_fields_only_when_set() {
    let temp = TempDir::new("bob-cli-capture-priority-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .arg("$3")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $3 json capture");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["priority"], "low");
    assert_eq!(json["priority_label"], "P3");
    assert_eq!(json["scheduled"], "2026-07-21");
    assert_eq!(
        date_offset_days("2026-06-15", json["scheduled"].as_str().unwrap()),
        36,
        "seeded P3 offset should match the log"
    );
    assert_eq!(
        json["schedule_log"]["reason"],
        "🎲 P0 → P3 · in **36** (31–90) days"
    );
    assert_eq!(
        json["schedule_log"]["lines"],
        serde_json::json!([
            "\t- 🗓️ **SCHEDULE LOG**",
            "\t\t- *2026-07-21* — 🎲 P0 → P3 · in **36** (31–90) days",
        ])
    );

    let unset_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("no")
        .arg("priority")
        .arg("here")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture without $<N>");
    assert_success(&unset_output);
    let unset_json: serde_json::Value = serde_json::from_str(
        stdout(&unset_output).trim(),
    )
    .unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON: {error}\n{}",
            format_output(&unset_output)
        )
    });
    assert!(unset_json.get("priority").is_none(), "{unset_json}");
    assert!(unset_json.get("priority_label").is_none(), "{unset_json}");
    assert!(unset_json.get("schedule_log").is_none(), "{unset_json}");
}

#[test]
fn capture_priority_renders_before_scheduled_and_before_pomodoro_block_id() {
    let temp = TempDir::new("bob-cli-capture-priority-pomodoro");
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
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@Dev:foobar")
        .arg("Some")
        .arg("foobar")
        .arg("task.")
        .arg("$2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run Pomodoro-linked priority capture");

    assert_success(&output);
    assert_eq!(
        priority_scheduled_offset_days(&output, "2026-07-10"),
        11,
        "seeded P2 offset should match the log"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read routed note"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [?] #task Some foobar task. [created::2026-07-10] [priority::medium] [scheduled::2026-07-21] ^foobar\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-07-21* — 🎲 P0 → P2 · in **11** (8–30) days\n",
        )
    );
}

#[test]
fn capture_priority_schedule_log_uses_the_target_notes_indent_unit() {
    let temp = TempDir::new("bob-cli-capture-priority-two-space-indent");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("spaced.md"),
        "# Spaced\n- [ ] #task Existing\n  - existing child\n",
    );
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["buy", "milk", "$2", "@spaced"])
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $2 capture routed into a two-space-indented note");

    assert_success(&output);
    assert_eq!(
        priority_scheduled_offset_days(&output, "2026-07-10"),
        11,
        "seeded P2 offset should match the log"
    );
    assert!(
        fs::read_to_string(vault.join("spaced.md"))
            .expect("read spaced note")
            .contains(concat!(
                "- [?] #task buy milk [created::2026-07-10] [priority::medium] [scheduled::2026-07-21]\n",
                "  - 🗓️ **SCHEDULE LOG**
",
                "    - *2026-07-21* — 🎲 P0 → P2 · in **11** (8–30) days\n",
            )),
        "expected the SCHEDULE LOG block indented with the note's two-space unit"
    );
}

#[test]
fn capture_priority_with_clip_orders_clip_children_before_schedule_log() {
    let temp = TempDir::new("bob-cli-capture-priority-clip-order");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let clipboard = temp.path().join("clipboard");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'clipped note\n'\n");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["buy", "milk", "$1", "%"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $1 capture with a clipboard child");

    assert_success(&output);
    assert_eq!(
        priority_scheduled_offset_days(&output, "2026-06-15"),
        7,
        "seeded P1 offset should match the log"
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [?] #task buy milk [created::2026-06-15] [priority::high] [scheduled::2026-06-22]\n",
            "\t- clipped note\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-06-22* — 🎲 P0 → P1 · in **7** (2–7) days\n",
        ),
        "clip children must come before the schedule log"
    );
}

#[test]
fn capture_priority_sub_bullet_nests_schedule_log_under_the_child() {
    let temp = TempDir::new("bob-cli-capture-priority-sub-bullet");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^parent\n");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task")
        .arg("parent")
        .arg("--")
        .arg("buy milk $2")
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run sub-bullet capture with $2");

    assert_success(&output);
    assert_eq!(
        priority_scheduled_offset_days(&output, "2026-07-10"),
        11,
        "seeded P2 offset should match the log"
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- buy milk [priority::medium] [scheduled::2026-07-21]\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-07-21* — 🎲 P0 → P2 · in **11** (8–30) days\n",
        ),
        "the schedule log must nest under the captured child, and the parent line stays untouched"
    );
}

#[test]
fn capture_priority_dry_run_prints_schedule_log_without_writing() {
    let temp = TempDir::new("bob-cli-capture-priority-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("buy")
        .arg("milk")
        .arg("$2")
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run $2 --dry-run capture");

    assert_success(&output);
    assert_eq!(
        priority_scheduled_offset_days(&output, "2026-07-10"),
        11,
        "seeded P2 offset should match the log"
    );
    let out = stdout(&output);
    assert!(
        out.contains("🗓️ **SCHEDULE LOG**")
            && out.contains("🎲 P0 → P2 · in **11** (8–30) days"),
        "unexpected dry-run output:\n{out}"
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "dry-run must not create the inbox"
    );
}

#[test]
fn capture_without_priority_token_tolerates_missing_config_file() {
    let temp = TempDir::new("bob-cli-capture-priority-lazy-load");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("does-not-exist/config.yml");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture without $<N> and missing config");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task buy milk [created::2026-06-15]\n"
    );
}

#[test]
fn capture_bare_dollar_matches_priority_level_one() {
    let temp = TempDir::new("bob-cli-capture-priority-bare-dollar");
    let vault_dollar = temp.path().join("vault-dollar");
    let vault_one = temp.path().join("vault-one");
    fs::create_dir_all(&vault_dollar).expect("create vault");
    fs::create_dir_all(&vault_one).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault_dollar)
        .args(["foo", "bar", "baz", "@bob", "$"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1");
    let dollar = command.output().expect("run bare $ capture");
    assert_success(&dollar);

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault_one)
        .args(["foo", "bar", "baz", "@bob", "$1"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1");
    let one = command.output().expect("run $1 capture");
    assert_success(&one);

    let dollar_note =
        fs::read_to_string(vault_dollar.join("bob.md")).expect("read $ note");
    let one_note =
        fs::read_to_string(vault_one.join("bob.md")).expect("read $1 note");
    assert_eq!(dollar_note, one_note);
    assert!(
        dollar_note.contains("[priority::high]"),
        "unexpected $ capture:\n{dollar_note}"
    );
    assert!(
        dollar_note.contains("SCHEDULE LOG"),
        "unexpected $ capture:\n{dollar_note}"
    );
}

#[test]
fn capture_mid_line_dollar_amount_stays_task_text() {
    let temp = TempDir::new("bob-cli-capture-priority-mid-line");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["Pay", "$3", "to", "Bob"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run mid-line $3 capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task Pay $3 to Bob [created::2026-06-15]\n"
    );
}

#[test]
fn capture_retired_priority_marker_is_a_usage_error() {
    let temp = TempDir::new("bob-cli-capture-priority-retired");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    for args in [
        vec!["foo", "p:2"],
        vec!["foo", "p:2", "@bob"],
        vec!["foo", "@bob", "p:2", "s:1"],
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .unwrap_or_else(|error| panic!("run {args:?}: {error}"));
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            format_output(&output)
        );
        let err = stderr(&output);
        assert!(
            err.contains("`p:2` is retired") && err.contains("write `$2`"),
            "{args:?}: unexpected stderr:\n{err}"
        );
        assert_eq!(
            fs::read_dir(&vault).expect("read vault").count(),
            0,
            "{args:?}: expected no files written"
        );
    }

    let child = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("foo\n- child p:2")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run child-line p:2 capture");
    assert_eq!(child.status.code(), Some(2), "{}", format_output(&child));
    assert!(stderr(&child).contains("`p:2` is retired"));

    let template = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@@template:\nfoo p:{0}\n\n2\n")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run template p:2 capture");
    assert_eq!(
        template.status.code(),
        Some(2),
        "{}",
        format_output(&template)
    );
    assert!(stderr(&template).contains("`p:2` is retired"));

    let literal = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["foo", "p:x"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run p:x capture");
    assert_success(&literal);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task foo p:x [created::2026-06-15]\n"
    );
}

#[test]
fn capture_escaped_dollar_amount_stays_task_text() {
    let temp = TempDir::new("bob-cli-capture-priority-escape");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);

    let escaped = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["Pay", "Bob", "\\$3"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run escaped $3 capture");
    assert_success(&escaped);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task Pay Bob $3 [created::2026-06-15]\n"
    );

    let mixed_vault = vault.join("mixed");
    fs::create_dir_all(&mixed_vault).expect("create mixed vault");
    let mixed = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&mixed_vault)
        .args(["Pay", "Bob", "\\$3", "$2", "@cash"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("run escaped $3 with $2 capture");
    assert_success(&mixed);
    let note =
        fs::read_to_string(mixed_vault.join("cash.md")).expect("read cash");
    assert!(
        note.contains("#task Pay Bob $3 ")
            && note.contains("[priority::medium]"),
        "unexpected escaped+priority capture:\n{note}"
    );

    let mid_vault = vault.join("mid");
    fs::create_dir_all(&mid_vault).expect("create mid vault");
    let mid = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&mid_vault)
        .args(["cost", "\\$3", "today"])
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run mid-line escaped $3 capture");
    assert_success(&mid);
    assert_eq!(
        fs::read_to_string(mid_vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task cost \\$3 today [created::2026-06-15]\n"
    );
}

#[test]
fn capture_parse_reports_priority_spans_and_retired_markers() {
    let dollar = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("body $ @bob")
        .output()
        .expect("run capture-parse bare $");
    assert_success(&dollar);
    let json: serde_json::Value = serde_json::from_str(stdout(&dollar).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "body");
    assert_eq!(json["spans"][0]["kind"], "priority");
    assert_eq!(json["spans"][0]["start"], 5);
    assert_eq!(json["spans"][0]["end"], 6);

    let two = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("body $2")
        .output()
        .expect("run capture-parse $2");
    assert_success(&two);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&two).trim()).expect("capture-parse JSON");
    assert_eq!(json["spans"][0]["kind"], "priority");
    assert_eq!(json["spans"][0]["start"], 5);
    assert_eq!(json["spans"][0]["end"], 7);

    let retired = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("foo p:2")
        .output()
        .expect("run capture-parse p:2");
    assert_success(&retired);
    let json: serde_json::Value = serde_json::from_str(stdout(&retired).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["diagnostics"][0]["code"], "retired_priority_marker");
    assert_eq!(
        json["diagnostics"][0]["message"],
        "`p:2` is retired; write `$2` for priority level 2 (a bare `$` means `$1`)"
    );
    assert_eq!(json["diagnostics"][0]["range"], serde_json::json!([4, 7]));

    let escaped = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Pay Bob \\$3")
        .output()
        .expect("run capture-parse escaped $3");
    assert_success(&escaped);
    let json: serde_json::Value = serde_json::from_str(stdout(&escaped).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "Pay Bob $3");
    let kinds: Vec<&str> = json["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .filter_map(|span| span["kind"].as_str())
        .collect();
    assert!(!kinds.contains(&"priority"), "{json}");
}

pub(crate) fn priority_scheduled_offset_days(
    output: &Output,
    created: &str,
) -> i64 {
    let out = stdout(output);
    let marker = "[scheduled::";
    let start = out.find(marker).unwrap_or_else(|| {
        panic!("expected a [scheduled::] field:\n{}", format_output(output))
    }) + marker.len();
    let end = out[start..].find(']').expect("closing bracket") + start;
    let scheduled_text = &out[start..end];
    date_offset_days(created, scheduled_text)
}

pub(crate) fn date_offset_days(created: &str, scheduled: &str) -> i64 {
    let scheduled = chrono::NaiveDate::parse_from_str(scheduled, "%Y-%m-%d")
        .unwrap_or_else(|error| {
            panic!("invalid scheduled date {scheduled}: {error}")
        });
    let created = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d")
        .expect("valid created date");
    (scheduled - created).num_days()
}
