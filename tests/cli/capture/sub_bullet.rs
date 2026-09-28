//! Capture sub-bullet tests.

use super::priority::write_priority_config;
use crate::support::*;
use sha2::Digest;
use sha2::Sha256;
use std::fs;

#[test]
fn capture_sub_bullet_inserts_with_parent_indentation_and_reports_json() {
    let cases = [
        (
            "tab",
            concat!(
                "## Tasks\n",
                "- [*] #task Parent [created::2026-07-01] ^parent\n",
                "\t- first child\n",
                "\t\t- grandchild\n",
                "Tail\n",
            ),
            concat!(
                "## Tasks\n",
                "- [*] #task Parent [created::2026-07-01] ^parent\n",
                "\t- first child\n",
                "\t\t- grandchild\n",
                "\t- new note\n",
                "Tail\n",
            ),
        ),
        (
            "spaces",
            concat!(
                "## Tasks\n",
                "- [/] #task Parent ^parent\n",
                "  - first child\n",
                "Tail\n",
            ),
            concat!(
                "## Tasks\n",
                "- [/] #task Parent ^parent\n",
                "  - first child\n",
                "  - new note\n",
                "Tail\n",
            ),
        ),
        (
            "indented-parent",
            concat!(
                "- [ ] #task Root\n",
                "  - [ ] #task Nested ^parent\n",
                "    - existing\n",
                "Tail\n",
            ),
            concat!(
                "- [ ] #task Root\n",
                "  - [ ] #task Nested ^parent\n",
                "    - existing\n",
                "    - new note\n",
                "Tail\n",
            ),
        ),
    ];

    for (name, original, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-sub-bullet-{name}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("cash.md"), original);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("@cash+parent")
            .arg("new note")
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run sub-bullet capture");
        assert_success(&output);
        assert_eq!(
            fs::read_to_string(vault.join("cash.md")).expect("read note"),
            expected,
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["kind"], "sub_bullet", "{name}");
        assert_eq!(json["block_id"], "parent", "{name}");
        assert_eq!(
            json["parent_text"],
            if name == "indented-parent" {
                "Nested"
            } else {
                "Parent"
            }
        );
        assert!(json["parent_line"].as_u64().is_some(), "{name}: {json}");
        assert!(
            json.get("parent_section").is_none(),
            "{name}: plain sub-bullet must omit parent_section: {json}"
        );
        assert_eq!(json["created"], "2026-07-31");
        assert_eq!(json["task_line"], "- new note");
    }
}

#[test]
fn capture_sub_bullet_uses_dominant_indent_preserves_crlf_and_dry_run() {
    let temp = TempDir::new("bob-cli-sub-bullet-indent-crlf");
    let vault = temp.path().join("vault");
    let note = vault.join("cash.md");
    let original = concat!(
        "## Tasks\r\n",
        "- [ ] #task Parent ^parent\r\n",
        "- ordinary\r\n",
        "\t- tabbed elsewhere\r\n",
    );
    write_file(&note, original);

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("new note @cash+parent")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("dry-run sub-bullet");
    assert_success(&dry_run);
    assert!(stdout(&dry_run).contains("would capture"));
    assert_eq!(fs::read_to_string(&note).expect("read dry note"), original);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("new note @cash+parent")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("CRLF sub-bullet");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "## Tasks\r\n",
            "- [ ] #task Parent ^parent\r\n",
            "\t- new note\r\n",
            "- ordinary\r\n",
            "\t- tabbed elsewhere\r\n",
        )
    );
}

#[test]
fn capture_sub_bullet_task_option_keeps_at_tokens_literal() {
    let temp = TempDir::new("bob-cli-sub-bullet-task-option");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^parent\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task")
        .arg("parent")
        .arg("--")
        .arg("mention @other literally")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("--task sub-bullet capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- mention @other literally\n",
        )
    );
}

#[test]
fn capture_sub_bullet_lands_before_direct_managed_logs() {
    let cases = [
        (
            "before-schedule",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- keep me\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- keep me\n",
                "\t- new note\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
        ),
        (
            "before-work",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- keep me\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- keep me\n",
                "\t- new note\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
        ),
        (
            "both-work-first",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- new note\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
        ),
        (
            "both-schedule-first",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- new note\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
                "\t- 🛠️ **WORK LOG**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
        ),
        (
            "emoji-less-and-legacy-markers",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t* **SCHEDULE LOG:**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
                "\t+ **Work log:**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- new note\n",
                "\t* **SCHEDULE LOG:**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
                "\t+ **Work log:**\n",
                "\t\t- *2026-08-15* — keep this work\n",
            ),
        ),
        (
            "ordered-marker",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t1. 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- new note\n",
                "\t1. 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-08-01* — keep this entry\n",
            ),
        ),
        (
            "lookalikes-append",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- **SCHEDULE LOG** trailing notes\n",
                "\t- **schedule log**\n",
                "\t- **Work Log:**\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- **SCHEDULE LOG** trailing notes\n",
                "\t- **schedule log**\n",
                "\t- **Work Log:**\n",
                "\t- new note\n",
            ),
        ),
        (
            "nested-log-ignored",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- child\n",
                "\t\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t\t- *2026-08-01* — nested\n",
                "\t- other\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- child\n",
                "\t\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t\t- *2026-08-01* — nested\n",
                "\t- other\n",
                "\t- new note\n",
            ),
        ),
        (
            "no-log-appends",
            concat!("- [ ] #task Parent ^parent\n", "\t- existing\n",),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- existing\n",
                "\t- new note\n",
            ),
        ),
        (
            "two-space",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "  - 🗓️ **SCHEDULE LOG**
",
                "    - *2026-08-01* — keep this entry\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "  - new note\n",
                "  - 🗓️ **SCHEDULE LOG**
",
                "    - *2026-08-01* — keep this entry\n",
            ),
        ),
        (
            "indented-parent",
            concat!(
                "- [ ] #task Root\n",
                "  - [ ] #task Nested ^parent\n",
                "    - 🗓️ **SCHEDULE LOG**\n",
                "      - *2026-08-01* — keep this entry\n",
            ),
            concat!(
                "- [ ] #task Root\n",
                "  - [ ] #task Nested ^parent\n",
                "    - new note\n",
                "    - 🗓️ **SCHEDULE LOG**\n",
                "      - *2026-08-01* — keep this entry\n",
            ),
        ),
        (
            "crlf",
            concat!(
                "- [ ] #task Parent ^parent\r\n",
                "\t- 🗓️ **SCHEDULE LOG**\r\n",
                "\t\t- *2026-08-01* — keep this entry\r\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\r\n",
                "\t- new note\r\n",
                "\t- 🗓️ **SCHEDULE LOG**\r\n",
                "\t\t- *2026-08-01* — keep this entry\r\n",
            ),
        ),
        (
            "eof-without-newline",
            concat!("- [ ] #task Parent ^parent\n", "\t- 🗓️ **SCHEDULE LOG**",),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- new note\n",
                "\t- 🗓️ **SCHEDULE LOG**",
            ),
        ),
    ];

    for (name, original, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-sub-bullet-log-{name}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("cash.md"), original);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("@cash+parent")
            .arg("new note")
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run sub-bullet capture before managed logs");
        assert_success(&output);
        assert_eq!(
            fs::read_to_string(vault.join("cash.md")).expect("read note"),
            expected,
            "{name}: {}",
            format_output(&output)
        );
    }
}

#[test]
fn capture_sub_bullet_selectors_and_batch_keep_order_above_logs() {
    let original = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — keep this entry\n",
    );
    let expected_one = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- new note\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — keep this entry\n",
    );

    let temp = TempDir::new("bob-cli-sub-bullet-log-task-option");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), original);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task")
        .arg("parent")
        .arg("--")
        .arg("new note")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run --task capture before managed log");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        expected_one
    );

    let parent = "- [?] #task Parent without ID";
    let digest = hex::encode(Sha256::digest(parent.as_bytes()));
    let temp = TempDir::new("bob-cli-sub-bullet-log-task-ref");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        &format!(
            "{parent}\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- *2026-08-01* — keep\n"
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task-ref")
        .arg(format!("1:{}", &digest[..8]))
        .arg("--")
        .arg("new note")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run --task-ref capture before managed log");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        format!("{parent}\n\t- new note\n\t- 🗓️ **SCHEDULE LOG**\n\t\t- *2026-08-01* — keep\n"),
    );

    let temp = TempDir::new("bob-cli-sub-bullet-log-batch");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), original);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("first note @cash+parent\n\nsecond note @cash+parent")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run batch capture before managed log");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- first note\n",
            "\t- second note\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep this entry\n",
        )
    );
}

#[test]
fn capture_sub_bullet_inserts_complete_subtree_before_parent_logs() {
    let original = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — keep this entry\n",
        "\t- 🛠️ **WORK LOG**\n",
        "\t\t- *2026-08-15* — keep this work\n",
    );
    let temp = TempDir::new("bob-cli-sub-bullet-log-subtree");
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
        .arg("@cash+parent buy milk p:2 %\n- authored child")
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("run subtree capture before parent logs");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["block_id"], "parent");
    assert_eq!(json["parent_text"], "Parent");
    assert_eq!(
        json["task_line"],
        "- buy milk [priority::medium] [scheduled::2026-07-21]"
    );
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- authored child"])
    );
    assert_eq!(json["placement"], "inserted");
    assert!(json["schedule_log"]["lines"].as_array().is_some());
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- buy milk [priority::medium] [scheduled::2026-07-21]\n",
            "\t\t- authored child\n",
            "\t\t- clip child\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-07-21* — 🎲 P0 → P2 · in **11** (8–30) days\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep this entry\n",
            "\t- 🛠️ **WORK LOG**\n",
            "\t\t- *2026-08-15* — keep this work\n",
        ),
        "{}",
        format_output(&output)
    );
}

#[test]
fn capture_sub_bullet_errors_are_actionable_in_human_and_json_modes() {
    struct ErrorCase<'a> {
        name: &'a str,
        note: Option<&'a str>,
        args: Vec<String>,
        exit: i32,
        expected: &'a str,
    }

    let duplicate = "- [ ] #task One ^dup\n- [x] #task Two ^dup\n";
    let repeated = "- [ ] #task Same\n- [ ] #task Same\n";
    let repeated_digest = hex::encode(Sha256::digest(b"- [ ] #task Same"));
    let cases = vec![
        ErrorCase {
            name: "missing-id",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@cash+missing".into()],
            exit: 1,
            expected: "no task with block ID ^missing in cash.md",
        },
        ErrorCase {
            name: "suggestion",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@cash+paren".into()],
            exit: 1,
            expected: "did you mean ^parent?",
        },
        ErrorCase {
            name: "not-task",
            note: Some("ordinary paragraph ^parent\n"),
            args: vec!["body".into(), "@cash+parent".into()],
            exit: 1,
            expected: "^parent in cash.md is not a task (line 1:",
        },
        ErrorCase {
            name: "duplicate",
            note: Some(duplicate),
            args: vec!["body".into(), "@cash+dup".into()],
            exit: 1,
            expected: "block ID ^dup appears 2 times",
        },
        ErrorCase {
            name: "stale-ref",
            note: Some("- [ ] #task Parent\n"),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task-ref".into(),
                "1:deadbeef".into(),
                "body".into(),
            ],
            exit: 1,
            expected: "selected task is no longer in cash.md",
        },
        ErrorCase {
            name: "ambiguous-ref",
            note: Some(repeated),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task-ref".into(),
                format!("99:{}", &repeated_digest[..8]),
                "body".into(),
            ],
            exit: 1,
            expected: "selected task matches more than one line in cash.md",
        },
        ErrorCase {
            name: "missing-note",
            note: None,
            args: vec!["body".into(), "@cash+parent".into()],
            exit: 1,
            expected: "note does not exist:",
        },
        ErrorCase {
            name: "empty-marker-id",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@cash+".into()],
            exit: 2,
            expected: "sub-bullet capture requires a block ID",
        },
        ErrorCase {
            name: "marker-no-route",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@+parent".into()],
            exit: 2,
            expected: "markers must use @<route>+<block-id>",
        },
        ErrorCase {
            name: "invalid-route-char",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@bad.route+parent".into()],
            exit: 2,
            expected: "sub-bullet capture route must contain only A-Z, a-z, 0-9, '_' or '-'",
        },
        ErrorCase {
            name: "invalid-block-id-char",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["body".into(), "@cash+bad.id".into()],
            exit: 2,
            expected: "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
        },
        ErrorCase {
            name: "task-no-route",
            note: Some("- [ ] #task Parent ^parent\n"),
            args: vec!["--task".into(), "parent".into(), "body".into()],
            exit: 2,
            expected: "--task requires --route",
        },
        ErrorCase {
            name: "malformed-ref",
            note: Some("- [ ] #task Parent\n"),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task-ref".into(),
                "bad".into(),
                "body".into(),
            ],
            exit: 2,
            expected: "--task-ref must use <line>:<digest>",
        },
    ];

    for case in cases {
        for json in [false, true] {
            let temp = TempDir::new(&format!(
                "bob-cli-sub-bullet-error-{}-{}",
                case.name,
                if json { "json" } else { "human" }
            ));
            let vault = temp.path().join("vault");
            fs::create_dir_all(&vault).expect("create vault");
            if let Some(note) = case.note {
                write_file(&vault.join("cash.md"), note);
            }
            let mut command = bob_command();
            command.arg("capture").arg("-b").arg(&vault);
            if json {
                command.arg("-f").arg("json");
            }
            command.args(&case.args);
            let output = command.output().expect("run failing capture");
            assert_eq!(
                output.status.code(),
                Some(case.exit),
                "{} / {json}: {}",
                case.name,
                format_output(&output)
            );
            let error_text = if json {
                let value: serde_json::Value =
                    serde_json::from_str(stdout(&output).trim())
                        .expect("JSON error object");
                assert_eq!(value["ok"], false);
                value["error"].as_str().unwrap_or_default().to_string()
            } else {
                stderr(&output)
            };
            assert!(
                error_text.contains(case.expected),
                "{} / {json}: expected {:?} in {:?}",
                case.name,
                case.expected,
                error_text
            );
        }
    }
}
