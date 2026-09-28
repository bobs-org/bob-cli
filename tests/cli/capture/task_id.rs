//! Capture task id and task sections.

use crate::support::*;
use std::fs;

#[test]
fn capture_task_id_assigns_and_dry_runs_lf_and_crlf_notes() {
    let temp = TempDir::new("bob-cli-capture-task-id-write");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);

    let lf = concat!(
        "# Tasks\n",
        "- [ ] #task Write the report\n",
        "- [ ] #task Keep me ^keep-me\n",
    );
    write_file(&vault.join("file.md"), lf);
    let lf_ref = capture_task_ref("- [ ] #task Write the report");

    let dry = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("file")
        .arg("-t")
        .arg(&lf_ref)
        .arg("-i")
        .arg("report-id")
        .arg("-d")
        .arg("-f")
        .arg("json")
        .output()
        .expect("dry-run capture-task-id");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("json");
    assert_eq!(dry_json["ok"], true);
    assert_eq!(dry_json["schema_version"], 1);
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["block_id"], "report-id");
    assert_eq!(fs::read_to_string(vault.join("file.md")).expect("read"), lf);

    let written = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("file")
        .arg("-t")
        .arg(&lf_ref)
        .arg("-i")
        .arg("report-id")
        .arg("-f")
        .arg("json")
        .output()
        .expect("write capture-task-id");
    assert_success(&written);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&written).trim()).expect("json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["route"], "file");
    assert_eq!(json["relative_target"], "file.md");
    assert_eq!(json["block_id"], "report-id");
    assert_eq!(json["line"], 2);
    assert_eq!(json["task"]["text"], "Write the report");
    assert_eq!(
        json["ref"],
        format!(
            "2:{}",
            &capture_task_ref("- [ ] #task Write the report ^report-id")[2..]
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("file.md")).expect("read"),
        concat!(
            "# Tasks\n",
            "- [ ] #task Write the report ^report-id\n",
            "- [ ] #task Keep me ^keep-me\n",
        )
    );

    let crlf = concat!(
        "# Tasks\r\n",
        "- [ ] #task CRLF report\r\n",
        "- ordinary\r\n",
    );
    write_file(&vault.join("notes.md"), crlf);
    let crlf_ref = capture_task_ref("- [ ] #task CRLF report");
    let crlf_out = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("notes")
        .arg("-t")
        .arg(&crlf_ref)
        .arg("-i")
        .arg("crlf-id")
        .output()
        .expect("CRLF capture-task-id");
    assert_success(&crlf_out);
    assert_eq!(
        fs::read_to_string(vault.join("notes.md")).expect("read"),
        concat!(
            "# Tasks\r\n",
            "- [ ] #task CRLF report ^crlf-id\r\n",
            "- ordinary\r\n",
        )
    );
}

#[test]
fn capture_task_id_recovers_a_shifted_line_and_rejects_write_free_failures() {
    let temp = TempDir::new("bob-cli-capture-task-id-errors");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    let original = concat!(
        "Plain heading ^plain-id\n",
        "- [ ] #task Same\n",
        "- [ ] #task Same\n",
        "- [ ] #task Ready ^ready-id\n",
        "- [x] #task Done task\n",
        "- [ ] #task Open missing\n",
    );
    write_file(&vault.join("file.md"), original);
    let missing_ref = capture_task_ref("- [ ] #task Open missing");
    let ready_ref = capture_task_ref("- [ ] #task Ready ^ready-id");
    let done_ref = capture_task_ref("- [x] #task Done task");
    let same_digest = &capture_task_ref("- [ ] #task Same")[2..];

    write_file(
        &vault.join("shifted.md"),
        "Intro\n- [ ] #task Open missing\n",
    );
    let shifted = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("shifted")
        .arg("-t")
        .arg(&missing_ref)
        .arg("-i")
        .arg("new-id")
        .arg("-f")
        .arg("json")
        .output()
        .expect("shifted capture-task-id");
    assert_success(&shifted);
    let shifted_json: serde_json::Value =
        serde_json::from_str(stdout(&shifted).trim()).expect("json");
    assert_eq!(shifted_json["line"], 2);
    assert_eq!(
        fs::read_to_string(vault.join("shifted.md")).expect("read"),
        "Intro\n- [ ] #task Open missing ^new-id\n"
    );

    let ambiguous_ref = format!("99:{same_digest}");
    let cases: &[(&str, i32, &str, &[&str])] = &[
        (
            "invalid-id",
            2,
            "--block-id must be",
            &["-r", "file", "-t", &missing_ref, "-i", "bad.id"],
        ),
        (
            "duplicate-task-id",
            1,
            "already exists",
            &["-r", "file", "-t", &missing_ref, "-i", "ready-id"],
        ),
        (
            "duplicate-anchor",
            1,
            "already exists",
            &["-r", "file", "-t", &missing_ref, "-i", "plain-id"],
        ),
        (
            "already-identified",
            1,
            "already has block ID ^ready-id",
            &["-r", "file", "-t", &ready_ref, "-i", "fresh-id"],
        ),
        (
            "terminal",
            1,
            "no longer open",
            &["-r", "file", "-t", &done_ref, "-i", "fresh-id"],
        ),
        (
            "stale",
            1,
            "no longer in file.md",
            &["-r", "file", "-t", "99:deadbeef", "-i", "fresh-id"],
        ),
        (
            "ambiguous",
            1,
            "matches more than one line",
            &["-r", "file", "-t", &ambiguous_ref, "-i", "fresh-id"],
        ),
        (
            "missing-note",
            1,
            "does not exist",
            &["-r", "missing", "-t", &missing_ref, "-i", "fresh-id"],
        ),
        (
            "invalid-ref",
            2,
            "--task-ref must use",
            &["-r", "file", "-t", "nope", "-i", "fresh-id"],
        ),
        (
            "invalid-route",
            2,
            "--route must contain",
            &["-r", "../bad", "-t", &missing_ref, "-i", "fresh-id"],
        ),
    ];

    for (name, exit, expected, extra) in cases {
        let output = bob_command()
            .arg("capture-task-id")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .args(*extra)
            .output()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            output.status.code(),
            Some(*exit),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|_| {
                panic!("{name}: {}", format_output(&output))
            });
        assert_eq!(json["ok"], false, "{name}");
        assert!(
            json["error"].as_str().expect("error").contains(expected),
            "{name}: {json}"
        );
        assert_eq!(
            fs::read_to_string(vault.join("file.md")).expect("read"),
            original,
            "{name} mutated the note"
        );
    }
}

#[test]
fn capture_task_sections_json_and_human_list_sections() {
    let temp = TempDir::new("bob-cli-capture-task-sections-list");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("foo.md"),
        concat!(
            "# Tasks\n",
            "- [ ] #task Parent ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
            "\t- NOTES\n",
        ),
    );

    let json_out = bob_command()
        .arg("capture-task-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("Foo")
        .arg("-i")
        .arg("bar")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run capture-task-sections json");
    assert_success(&json_out);
    assert!(
        stderr(&json_out).is_empty(),
        "json should keep stderr clean:\n{}",
        format_output(&json_out)
    );
    let raw = stdout(&json_out);
    assert_text_order(
        raw.trim(),
        &[
            "\"ok\"",
            "\"schema_version\"",
            "\"route\"",
            "\"block_id\"",
            "\"ref\"",
            "\"count\"",
            "\"sections\"",
            "\"title\"",
            "\"slug\"",
            "\"line\"",
            "\"child_count\"",
            "\"depth\"",
        ],
    );
    let json: serde_json::Value =
        serde_json::from_str(raw.trim()).expect("json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["route"], "foo");
    assert_eq!(json["block_id"], "bar");
    assert_eq!(json["count"], 3);
    assert_eq!(json["sections"][0]["title"], "REQUIREMENTS");
    assert_eq!(json["sections"][0]["slug"], "requirements");
    assert_eq!(json["sections"][0]["line"], 3);
    assert_eq!(json["sections"][0]["child_count"], 1);
    assert_eq!(json["sections"][0]["depth"], 1);
    assert_eq!(json["sections"][1]["title"], "FUTURE WORK");
    assert_eq!(json["sections"][1]["slug"], "future-work");
    assert_eq!(json["sections"][2]["title"], "NOTES");
    assert_eq!(json["sections"][2]["child_count"], 0);
    let task_ref = json["ref"].as_str().expect("ref");
    let (line, digest) = task_ref.split_once(':').expect("ref separator");
    assert_eq!(line, "2");
    assert_eq!(digest.len(), 8);

    let human = bob_command()
        .arg("capture-task-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("foo")
        .arg("-i")
        .arg("bar")
        .output()
        .expect("run capture-task-sections human");
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let text = stdout(&human);
    assert!(text.contains("Capture task sections - foo.md"), "{text}");
    assert!(text.contains("^bar"), "{text}");
    assert!(text.contains("REQUIREMENTS"), "{text}");
    assert!(text.contains("requirements"), "{text}");
    assert!(text.contains("FUTURE WORK"), "{text}");
    assert!(text.contains("3 sections"), "{text}");
}

#[test]
fn capture_task_sections_empty_and_error_paths() {
    let temp = TempDir::new("bob-cli-capture-task-sections-errors");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    let original = concat!(
        "Plain heading ^plain-id\n",
        "- [ ] #task Ready ^ready-id\n",
        "- [ ] #task Dup ^dup-id\n",
        "- [ ] #task Also dup ^dup-id\n",
        "- [ ] #task Same\n",
        "- [ ] #task Same\n",
        "- [ ] #task Empty ^empty-id\n",
        "- [ ] #task Parent\n",
        "\t- REQUIREMENTS\n",
    );
    write_file(&vault.join("foo.md"), original);
    let parent_ref = capture_task_ref("- [ ] #task Parent");
    let same_digest = &capture_task_ref("- [ ] #task Same")[2..];
    let ambiguous_ref = format!("99:{same_digest}");

    let empty = bob_command()
        .arg("capture-task-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("foo")
        .arg("-i")
        .arg("empty-id")
        .arg("-f")
        .arg("json")
        .output()
        .expect("empty sections");
    assert_success(&empty);
    let empty_json: serde_json::Value =
        serde_json::from_str(stdout(&empty).trim()).expect("json");
    assert_eq!(empty_json["ok"], true);
    assert_eq!(empty_json["count"], 0);
    assert_eq!(
        empty_json["sections"].as_array().expect("sections").len(),
        0
    );

    let by_ref = bob_command()
        .arg("capture-task-sections")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("foo")
        .arg("-t")
        .arg(&parent_ref)
        .arg("-f")
        .arg("json")
        .output()
        .expect("task-ref sections");
    assert_success(&by_ref);
    let ref_json: serde_json::Value =
        serde_json::from_str(stdout(&by_ref).trim()).expect("json");
    assert!(ref_json["block_id"].is_null(), "{ref_json}");
    assert_eq!(ref_json["count"], 1);
    assert_eq!(ref_json["sections"][0]["title"], "REQUIREMENTS");

    let cases: &[(&str, i32, &str, &[&str])] = &[
        (
            "missing-note",
            1,
            "does not exist",
            &["-r", "missing", "-i", "bar"],
        ),
        (
            "missing-task",
            1,
            "no task with block ID ^bar",
            &["-r", "foo", "-i", "bar"],
        ),
        (
            "duplicate-id",
            1,
            "appears 2 times",
            &["-r", "foo", "-i", "dup-id"],
        ),
        (
            "not-a-task",
            1,
            "is not a task",
            &["-r", "foo", "-i", "plain-id"],
        ),
        (
            "stale-ref",
            1,
            "no longer in foo.md",
            &["-r", "foo", "-t", "99:deadbeef"],
        ),
        (
            "ambiguous-ref",
            1,
            "matches more than one line",
            &["-r", "foo", "-t", &ambiguous_ref],
        ),
        (
            "both-selectors",
            2,
            "exactly one of --block-id or --task-ref",
            &["-r", "foo", "-i", "ready-id", "-t", &parent_ref],
        ),
        (
            "neither-selector",
            2,
            "exactly one of --block-id or --task-ref",
            &["-r", "foo"],
        ),
        (
            "invalid-id",
            2,
            "--block-id must be",
            &["-r", "foo", "-i", "bad.id"],
        ),
        (
            "invalid-ref",
            2,
            "--task-ref must use",
            &["-r", "foo", "-t", "nope"],
        ),
        (
            "invalid-route",
            2,
            "--route must contain",
            &["-r", "../bad", "-i", "bar"],
        ),
        ("missing-route", 2, "--route is required", &["-i", "bar"]),
    ];

    for (name, exit, expected, extra) in cases {
        let output = bob_command()
            .arg("capture-task-sections")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .args(*extra)
            .output()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            output.status.code(),
            Some(*exit),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|_| {
                panic!("{name}: {}", format_output(&output))
            });
        assert_eq!(json["ok"], false, "{name}");
        assert!(
            json["error"].as_str().expect("error").contains(expected),
            "{name}: {json}"
        );
        assert_eq!(
            fs::read_to_string(vault.join("foo.md")).expect("read"),
            original,
            "{name} mutated the note"
        );
    }
}

fn capture_task_ref(line: &str) -> String {
    capture_pomodoro_ref(line, 1)
}
