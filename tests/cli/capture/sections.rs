//! Forced section, task section, sections and tasks.

use crate::support::*;
use sha2::Digest;
use sha2::Sha256;
use std::fs;

#[test]
fn capture_forced_section_inserts_exact_bullet() {
    let temp = TempDir::new("bob-cli-capture-forced-section");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("foo.md"),
        "# Foo\n## Ideas\nnotes\n## Idea\nnotes\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("foo")
        .arg("--section")
        .arg("Idea")
        .arg("--")
        .arg("Some")
        .arg("bullet")
        .arg("@bar")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run forced-section bullet capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  foo.md"),
        "unexpected forced-section capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("read foo"),
        "# Foo\n## Ideas\nnotes\n## Idea\n\n- Some bullet @bar [created::2026-06-15]\nnotes\n"
    );
}

#[test]
fn capture_forced_section_requires_route() {
    let temp = TempDir::new("bob-cli-capture-forced-section-route");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--section")
        .arg("Ideas")
        .arg("--")
        .arg("Some")
        .arg("bullet")
        .output()
        .expect("run forced-section without route");

    assert_eq!(
        output.status.code(),
        Some(2),
        "--section without --route should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--section requires --route"),
        "expected section route usage error:\n{}",
        format_output(&output)
    );
}

#[test]
fn capture_task_section_inserts_into_first_middle_and_last_sections() {
    let original = concat!(
        "- [ ] #task Parent [created::2026-07-01] ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t\t- existing req\n",
        "\t- FUTURE WORK\n",
        "\t\t- later\n",
        "\t- NOTES\n",
        "Tail\n",
    );
    let cases = [
        (
            "first",
            "Postgres 17 minimum @cash+parent#requirements",
            concat!(
                "- [ ] #task Parent [created::2026-07-01] ^parent\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing req\n",
                "\t\t- Postgres 17 minimum\n",
                "\t- FUTURE WORK\n",
                "\t\t- later\n",
                "\t- NOTES\n",
                "Tail\n",
            ),
            "REQUIREMENTS",
            "- Postgres 17 minimum",
        ),
        (
            "middle",
            "follow up @cash+parent#future-work",
            concat!(
                "- [ ] #task Parent [created::2026-07-01] ^parent\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing req\n",
                "\t- FUTURE WORK\n",
                "\t\t- later\n",
                "\t\t- follow up\n",
                "\t- NOTES\n",
                "Tail\n",
            ),
            "FUTURE WORK",
            "- follow up",
        ),
        (
            "last-empty",
            "jot this @cash+parent#notes",
            concat!(
                "- [ ] #task Parent [created::2026-07-01] ^parent\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing req\n",
                "\t- FUTURE WORK\n",
                "\t\t- later\n",
                "\t- NOTES\n",
                "\t\t- jot this\n",
                "Tail\n",
            ),
            "NOTES",
            "- jot this",
        ),
    ];

    for (name, draft, expected, section, task_line) in cases {
        let temp = TempDir::new(&format!("bob-cli-task-section-{name}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("cash.md"), original);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg(draft)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run task-section capture");
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
        assert_eq!(json["parent_section"], section, "{name}");
        assert_eq!(json["parent_text"], "Parent", "{name}");
        assert_eq!(json["block_id"], "parent", "{name}");
        assert_eq!(json["task_line"], task_line, "{name}");
        assert_eq!(json["placement"], "inserted", "{name}");
    }
}

#[test]
fn capture_task_section_prefix_vs_whole_slug_and_multiword() {
    let original = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- FUTURE WORKFLOW\n",
        "\t- FUTURE WORK\n",
    );
    let cases = [
        (
            "prefix",
            "first prefix @cash+parent#future",
            "FUTURE WORKFLOW",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- FUTURE WORKFLOW\n",
                "\t\t- first prefix\n",
                "\t- FUTURE WORK\n",
            ),
        ),
        (
            "whole-slug",
            "exact slug @cash+parent#future-work",
            "FUTURE WORK",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- FUTURE WORKFLOW\n",
                "\t- FUTURE WORK\n",
                "\t\t- exact slug\n",
            ),
        ),
        (
            "multiword-slug",
            "by slug @cash+parent#future-workflow",
            "FUTURE WORKFLOW",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- FUTURE WORKFLOW\n",
                "\t\t- by slug\n",
                "\t- FUTURE WORK\n",
            ),
        ),
    ];

    for (name, draft, section, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-task-section-slug-{name}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("cash.md"), original);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg(draft)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run slug selection capture");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["parent_section"], section, "{name}");
        assert_eq!(
            fs::read_to_string(vault.join("cash.md")).expect("read note"),
            expected,
            "{name}: {}",
            format_output(&output)
        );
    }
}

#[test]
fn capture_task_section_managed_log_geometry() {
    let nested_log = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t\t- *2026-08-01* — scheduled\n",
        "\t- FUTURE WORK\n",
    );
    let temp = TempDir::new("bob-cli-task-section-nested-log");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), nested_log);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("new note @cash+parent#requirements")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run nested-log section capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- new note\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-08-01* — scheduled\n",
            "\t- FUTURE WORK\n",
        )
    );

    let sibling_log = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t\t- existing\n",
        "\t- FUTURE WORK\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01* — scheduled\n",
    );
    let temp = TempDir::new("bob-cli-task-section-sibling-log");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), sibling_log);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("new note @cash+parent#future-work")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run sibling-log section capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
            "\t\t- new note\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — scheduled\n",
        )
    );
}

#[test]
fn capture_task_section_indent_units_crlf_and_dry_run() {
    let cases = [
        (
            "tab",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing\n",
                "\t- FUTURE WORK\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing\n",
                "\t\t- new note\n",
                "\t- FUTURE WORK\n",
            ),
        ),
        (
            "two-space",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "  - REQUIREMENTS\n",
                "    - existing\n",
                "  - FUTURE WORK\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "  - REQUIREMENTS\n",
                "    - existing\n",
                "    - new note\n",
                "  - FUTURE WORK\n",
            ),
        ),
        (
            "four-space",
            concat!(
                "- [ ] #task Parent ^parent\n",
                "    - REQUIREMENTS\n",
                "        - existing\n",
                "    - FUTURE WORK\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\n",
                "    - REQUIREMENTS\n",
                "        - existing\n",
                "        - new note\n",
                "    - FUTURE WORK\n",
            ),
        ),
        (
            "crlf",
            concat!(
                "- [ ] #task Parent ^parent\r\n",
                "\t- REQUIREMENTS\r\n",
                "\t\t- existing\r\n",
                "\t- FUTURE WORK\r\n",
            ),
            concat!(
                "- [ ] #task Parent ^parent\r\n",
                "\t- REQUIREMENTS\r\n",
                "\t\t- existing\r\n",
                "\t\t- new note\r\n",
                "\t- FUTURE WORK\r\n",
            ),
        ),
    ];

    for (name, original, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-task-section-indent-{name}"));
        let vault = temp.path().join("vault");
        let note = vault.join("cash.md");
        write_file(&note, original);

        let dry_run = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--dry-run")
            .arg("-f")
            .arg("json")
            .arg("new note @cash+parent#requirements")
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("dry-run task-section");
        assert_success(&dry_run);
        let dry_json: serde_json::Value =
            serde_json::from_str(stdout(&dry_run).trim()).expect("dry JSON");
        assert_eq!(dry_json["dry_run"], true, "{name}");
        assert_eq!(dry_json["parent_section"], "REQUIREMENTS", "{name}");
        assert_eq!(
            fs::read_to_string(&note).expect("read dry note"),
            original,
            "{name} dry-run must not write"
        );

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("new note @cash+parent#requirements")
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run indent task-section");
        assert_success(&output);
        assert_eq!(
            fs::read_to_string(&note).expect("read note"),
            expected,
            "{name}: {}",
            format_output(&output)
        );
    }
}

#[test]
fn capture_forced_task_section_matches_title_exactly() {
    let original = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t- FUTURE WORK\n",
    );
    let temp = TempDir::new("bob-cli-forced-task-section");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), original);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--route")
        .arg("cash")
        .arg("--task")
        .arg("parent")
        .arg("--task-section")
        .arg("REQUIREMENTS")
        .arg("--")
        .arg("mention @other literally")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run --task-section capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["parent_section"], "REQUIREMENTS");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- mention @other literally\n",
            "\t- FUTURE WORK\n",
        )
    );

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("-t")
        .arg("parent")
        .arg("-S")
        .arg("Future Work")
        .arg("--")
        .arg("cased title")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run -S Future Work");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("under"), "{out}");
    assert!(out.contains(" · FUTURE WORK"), "{out}");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- mention @other literally\n",
            "\t- FUTURE WORK\n",
            "\t\t- cased title\n",
        )
    );

    let parent_line = "- [ ] #task Parent ^parent";
    let digest = hex::encode(Sha256::digest(parent_line.as_bytes()));
    let ref_temp = TempDir::new("bob-cli-forced-task-section-ref");
    let vault = ref_temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        &format!("{parent_line}\n\t- NOTES\n"),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task-ref")
        .arg(format!("1:{}", &digest[..8]))
        .arg("--task-section")
        .arg("NOTES")
        .arg("--")
        .arg("from ref")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run --task-ref --task-section");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- NOTES\n",
            "\t\t- from ref\n",
        )
    );
}

#[test]
fn capture_forced_task_section_option_errors() {
    struct Case<'a> {
        name: &'a str,
        note: Option<&'a str>,
        args: Vec<String>,
        exit: i32,
        expected: &'a str,
    }

    let note = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t- FUTURE WORK\n",
    );
    let cases = [
        Case {
            name: "slug-is-not-exact",
            note: Some(note),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task".into(),
                "parent".into(),
                "--task-section".into(),
                "future-work".into(),
                "body".into(),
            ],
            exit: 1,
            expected: "did you mean FUTURE WORK?",
        },
        Case {
            name: "requires-route",
            note: Some(note),
            args: vec![
                "--task-section".into(),
                "REQUIREMENTS".into(),
                "body".into(),
            ],
            exit: 2,
            expected: "--task-section requires --route",
        },
        Case {
            name: "requires-task",
            note: Some(note),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task-section".into(),
                "REQUIREMENTS".into(),
                "body".into(),
            ],
            exit: 2,
            expected: "--task-section requires --task or --task-ref",
        },
        Case {
            name: "empty",
            note: Some(note),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--task".into(),
                "parent".into(),
                "--task-section".into(),
                String::new(),
                "body".into(),
            ],
            exit: 2,
            expected: "--task-section must not be empty",
        },
        Case {
            name: "conflicts-with-section",
            note: Some(note),
            args: vec![
                "--route".into(),
                "cash".into(),
                "--section".into(),
                "Ideas".into(),
                "--task-section".into(),
                "REQUIREMENTS".into(),
                "body".into(),
            ],
            exit: 2,
            expected: "cannot be used with",
        },
    ];

    for case in cases {
        let temp = TempDir::new(&format!(
            "bob-cli-forced-task-section-error-{}",
            case.name
        ));
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        if let Some(note) = case.note {
            write_file(&vault.join("cash.md"), note);
        }
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&case.args)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run forced task-section error");
        assert_eq!(
            output.status.code(),
            Some(case.exit),
            "{}: {}",
            case.name,
            format_output(&output)
        );
        let error_text = format!("{}{}", stdout(&output), stderr(&output));
        assert!(
            error_text.contains(case.expected),
            "{}: expected {:?} in {:?}",
            case.name,
            case.expected,
            error_text
        );
        if let Some(note) = case.note {
            assert_eq!(
                fs::read_to_string(vault.join("cash.md")).expect("read note"),
                note,
                "{} must leave the note unchanged",
                case.name
            );
        }
    }
}

#[test]
fn capture_task_section_errors_leave_the_note_unchanged() {
    struct Case<'a> {
        name: &'a str,
        note: Option<&'a str>,
        args: Vec<String>,
        exit: i32,
        expected: &'a str,
    }

    let with_sections = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- REQUIREMENTS\n",
        "\t- FUTURE WORK\n",
    );
    let no_sections = "- [ ] #task Parent ^parent\n\t- ordinary child\n";
    let duplicate = "- [ ] #task One ^dup\n- [x] #task Two ^dup\n";
    let cases = [
        Case {
            name: "no-match",
            note: Some(with_sections),
            args: vec!["body".into(), "@cash+parent#absent".into()],
            exit: 1,
            expected:
                "no task section matching 'absent' under ^parent in cash.md",
        },
        Case {
            name: "lists-titles",
            note: Some(with_sections),
            args: vec!["body".into(), "@cash+parent#zzz".into()],
            exit: 1,
            expected: "have: REQUIREMENTS, FUTURE WORK",
        },
        Case {
            name: "close-match",
            note: Some(with_sections),
            args: vec!["body".into(), "@cash+parent#requirments".into()],
            exit: 1,
            expected: "did you mean REQUIREMENTS?",
        },
        Case {
            name: "no-sections",
            note: Some(no_sections),
            args: vec!["body".into(), "@cash+parent#requirements".into()],
            exit: 1,
            expected: "task ^parent in cash.md has no task sections",
        },
        Case {
            name: "missing-task-wins",
            note: Some(with_sections),
            args: vec!["body".into(), "@cash+missing#requirements".into()],
            exit: 1,
            expected: "no task with block ID ^missing in cash.md",
        },
        Case {
            name: "duplicate-wins",
            note: Some(duplicate),
            args: vec!["body".into(), "@cash+dup#requirements".into()],
            exit: 1,
            expected: "block ID ^dup appears 2 times",
        },
        Case {
            name: "non-task-wins",
            note: Some("ordinary paragraph ^parent\n"),
            args: vec!["body".into(), "@cash+parent#requirements".into()],
            exit: 1,
            expected: "^parent in cash.md is not a task (line 1:",
        },
    ];

    for case in cases {
        for json in [false, true] {
            let temp = TempDir::new(&format!(
                "bob-cli-task-section-error-{}-{}",
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
            let output = command.output().expect("run failing section capture");
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
            if let Some(note) = case.note {
                assert_eq!(
                    fs::read_to_string(vault.join("cash.md"))
                        .expect("read note"),
                    note,
                    "{} / {json} must leave the note unchanged",
                    case.name
                );
            }
        }
    }
}

#[test]
fn capture_sections_json_lists_sections_in_order() {
    let temp = TempDir::new("bob-cli-capture-sections-json");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "## Ignored\n",
            "---\n",
            "# Cash\n",
            "```md\n",
            "## Ignored\n",
            "```\n",
            "## Tasks\n",
            "### Ideas\n",
            "###### Log\n",
        ),
    );

    let output = bob_command()
        .arg("capture-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("Cash")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-sections json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "capture-sections json should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["route"], "cash");
    assert_eq!(json["count"], 3);
    assert_eq!(json["sections"][0]["title"], "Cash");
    assert_eq!(json["sections"][0]["level"], 1);
    assert_eq!(json["sections"][1]["title"], "Ideas");
    assert_eq!(json["sections"][1]["level"], 3);
    assert_eq!(json["sections"][2]["title"], "Log");
    assert_eq!(json["sections"][2]["level"], 6);
}

#[test]
fn capture_sections_missing_note_returns_empty_json() {
    let temp = TempDir::new("bob-cli-capture-sections-missing");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("cash")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-sections missing");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "missing note should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["route"], "cash");
    assert_eq!(json["count"], 0);
    assert_eq!(
        json["sections"].as_array().expect("sections array").len(),
        0
    );
}

#[test]
fn capture_sections_invalid_or_missing_route_errors_cleanly() {
    let temp = TempDir::new("bob-cli-capture-sections-route-errors");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let missing = bob_command()
        .arg("capture-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-sections missing route");
    assert_eq!(
        missing.status.code(),
        Some(2),
        "missing route should be a usage error:\n{}",
        format_output(&missing)
    );
    assert!(
        stderr(&missing).is_empty(),
        "json usage failure should keep stderr clean:\n{}",
        format_output(&missing)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json error");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("--route")),
        "unexpected missing-route json: {json}"
    );

    let invalid = bob_command()
        .arg("capture-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("../bad")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-sections invalid route");
    assert_eq!(
        invalid.status.code(),
        Some(2),
        "invalid route should be a usage error:\n{}",
        format_output(&invalid)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&invalid).trim()).expect("json error");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("must contain only")),
        "unexpected invalid-route json: {json}"
    );
}

#[test]
fn capture_tasks_json_lists_open_tasks_with_stable_picker_shape() {
    let temp = TempDir::new("bob-cli-capture-tasks-json");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "# Cash\n",
            "## Tasks\n",
            "- [ ] #task Call bank ^bank\n",
            "\t- existing note\n",
            "- [x] #task Finished\n",
            "  - [/] #task Nested active [created:: 2026-07-31]\n",
            "- [-] #task Canceled\n",
            "- [?] #task Waiting ^waiting\n",
        ),
    );

    let output = bob_command()
        .arg("capture-tasks")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("Cash")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-tasks json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "capture-tasks json should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["route"], "cash");
    assert_eq!(json["relative_target"], "cash.md");
    assert_eq!(json["count"], 3);
    assert_eq!(json["tasks"][0]["line"], 3);
    assert_eq!(json["tasks"][0]["block_id"], "bank");
    assert_eq!(json["tasks"][0]["status_type"], "TODO");
    assert_eq!(json["tasks"][0]["text"], "Call bank");
    assert_eq!(json["tasks"][0]["section"], "Tasks");
    assert_eq!(json["tasks"][0]["depth"], 0);
    assert_eq!(json["tasks"][0]["child_count"], 1);
    let task_ref = json["tasks"][0]["ref"].as_str().expect("task ref");
    let (line, digest) = task_ref.split_once(':').expect("ref separator");
    assert_eq!(line, "3");
    assert_eq!(digest.len(), 8);
    assert!(digest
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
    assert_eq!(json["tasks"][1]["line"], 6);
    assert_eq!(json["tasks"][1]["status_name"], "In Progress");
    assert_eq!(json["tasks"][1]["depth"], 1);
    assert_eq!(json["tasks"][2]["status_name"], "Blocked");
}

#[test]
fn capture_tasks_human_output_is_plain_when_piped() {
    let temp = TempDir::new("bob-cli-capture-tasks-human");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "# Cash\n## Tasks\n- [ ] #task Call bank ^bank\n",
    );

    let output = bob_command()
        .arg("capture-tasks")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("cash")
        .output()
        .expect("run bob capture-tasks human");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(human.contains("Capture tasks - cash.md"), "{human}");
    assert!(human.contains("[ ] Call bank"), "{human}");
    assert!(human.contains("^bank"), "{human}");
    assert!(human.contains("1 task - 1 unknown"), "{human}");
}
