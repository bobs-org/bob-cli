//! Execution tests for `@@template:` drafts.

use crate::support::*;
use std::fs;

const NOW: &str = "2026-06-15 10:11:12";

fn capture(temp: &TempDir, vault: &std::path::Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .env("BOB_NOW", NOW)
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

#[test]
fn template_executes_rows_and_adds_row_metadata() {
    let temp = TempDir::new("bob-cli-capture-template-execution");
    let dry_vault = temp.path().join("dry-vault");
    fs::create_dir_all(&dry_vault).expect("create dry-run vault");
    let draft = concat!(
        "@@template:\n",
        "Apply to {0}! @job^apply-{1}\n",
        "- **URL**: {2}\n",
        "\n",
        "Foobar foo https://foobar.example/job\n",
        "\"Two Sigma\" two-sigma https://twosigma.example/job\n",
    );

    let dry_run = capture(&temp, &dry_vault)
        .args(["--dry-run", "-f", "json"])
        .arg(draft)
        .output()
        .expect("run dry-run template capture");
    assert_success(&dry_run);
    let result = json(&dry_run);
    assert_eq!(result["template"]["line"], 1);
    assert_eq!(result["template"]["field_count"], 3);
    assert_eq!(result["template"]["row_count"], 2);
    assert_eq!(
        result["template_row"],
        serde_json::json!({"row": 1, "line": 5})
    );
    assert_eq!(result["text"], "Apply to Foobar!");
    assert_eq!(result["block_id"], "apply-foo");
    assert_eq!(result["captures"][0]["template_row"]["row"], 1);
    assert_eq!(
        result["captures"][1]["template_row"],
        serde_json::json!({"row": 2, "line": 6})
    );
    assert_eq!(result["captures"][1]["text"], "Apply to Two Sigma!");
    assert_eq!(
        result["captures"][1]["sub_bullets"],
        serde_json::json!(["\t- **URL**: https://twosigma.example/job"])
    );
    assert!(!dry_vault.join("job.md").exists());

    let real_vault = temp.path().join("real-vault");
    fs::create_dir_all(&real_vault).expect("create real vault");
    let real = capture(&temp, &real_vault)
        .args(["-f", "json"])
        .arg(draft)
        .output()
        .expect("run real template capture");
    assert_success(&real);
    assert_eq!(
        fs::read_to_string(real_vault.join("job.md")).expect("read job note"),
        concat!(
            "- [ ] #task Apply to Foobar! [created::2026-06-15] ^apply-foo\n",
            "\t- **URL**: https://foobar.example/job\n",
            "- [ ] #task Apply to Two Sigma! [created::2026-06-15] ^apply-two-sigma\n",
            "\t- **URL**: https://twosigma.example/job\n",
        )
    );
}

#[test]
fn template_human_summary_names_the_source_rows() {
    let temp = TempDir::new("bob-cli-capture-template-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .arg("--dry-run")
        .arg("@@template:\nTask {0}\n\nfirst\nsecond\n")
        .output()
        .expect("run human template capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("template  2 rows · lines 4–5"),
        "unexpected output:\n{}",
        format_output(&output)
    );
    assert!(stdout(&output).contains("would capture  1/2"));
    assert!(stdout(&output).contains("would capture  2/2"));
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn a_late_duplicate_id_refuses_the_whole_template_with_row_context() {
    let temp = TempDir::new("bob-cli-capture-template-atomic");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let original = "- [ ] #task existing ^duplicate\n";
    fs::write(vault.join("work.md"), original).expect("write existing task");
    let draft = concat!(
        "@@template:\n",
        "Task {0} @work^{1}\n",
        "\n",
        "first one\n",
        "second two\n",
        "third three\n",
        "fourth four\n",
        "fifth duplicate\n",
    );
    let output = capture(&temp, &vault)
        .arg(draft)
        .output()
        .expect("run invalid template capture");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("template row 5 (line 8)")
            && stderr(&output).contains("block ID ^duplicate already exists"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work note"),
        original,
        "earlier staged rows must not be written"
    );
}

#[test]
fn duplicate_ids_across_template_rows_refuse_with_the_later_row() {
    let temp = TempDir::new("bob-cli-capture-template-duplicate-rows");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .arg("@@template:\nTask {0} @work^{1}\n\nfirst same-id\nsecond same-id\n")
        .output()
        .expect("run duplicate row capture");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("template row 2 (line 5)")
            && stderr(&output).contains("block ID ^same-id already exists"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("work.md").exists());
}

#[test]
fn parser_diagnostics_keep_the_template_source_line() {
    let temp = TempDir::new("bob-cli-capture-template-source-line");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .arg("-f")
        .arg("json")
        .arg("@@template:\nTask {0}\n  - nested {1}\n\nparent child\n")
        .output()
        .expect("run malformed template item");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let error = json(&output);
    assert!(
        error["error"].as_str().unwrap().contains("capture line 3"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn template_clipboard_markers_refuse_even_with_no_clip() {
    for no_clip in [false, true] {
        let temp = TempDir::new("bob-cli-capture-template-clipboard");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let mut command = capture(&temp, &vault);
        command.args(["-f", "json"]);
        if no_clip {
            command.arg("--no-clip");
        }
        let output = command
            .arg("@@template:\nTask {0} %\n\nvalue\n")
            .output()
            .expect("run clipboard-marker refusal");
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        let error = json(&output);
        assert_eq!(error["code"], "template_clipboard");
        assert!(error["error"]
            .as_str()
            .unwrap()
            .contains("template row 1 (line 4)"));
        assert!(!vault.join("mac_inbox.md").exists());
    }
}

#[test]
fn template_clip_flag_and_session_operators_have_stable_codes() {
    let temp = TempDir::new("bob-cli-capture-template-refusals");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let forced_clip = capture(&temp, &vault)
        .args(["--clip", "-f", "json"])
        .arg("@@template:\nTask {0}\n\nvalue\n")
        .output()
        .expect("run forced clip refusal");
    assert_eq!(forced_clip.status.code(), Some(2));
    assert_eq!(json(&forced_clip)["code"], "template_clipboard");

    for session_form in ["=", "=x", "+5", "++2", "--3", "@work:task=3"] {
        let session = capture(&temp, &vault)
            .args(["-f", "json"])
            .arg(format!("@@template:\n{{0}}\n\n{session_form}\n"))
            .output()
            .expect("run session refusal");
        assert_eq!(
            session.status.code(),
            Some(2),
            "{session_form}: {}",
            format_output(&session)
        );
        assert_eq!(
            json(&session)["code"],
            "template_session_operator",
            "{session_form}"
        );
        assert!(json(&session)["error"]
            .as_str()
            .unwrap()
            .contains("template row 1 (line 4)"));
    }
}

#[test]
fn template_session_suffixes_refuse_in_execution() {
    let temp = TempDir::new("bob-cli-capture-template-session-suffix");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    for draft in [
        "@@template:\nTask {0} @job:{1}=3\n\na b\n",
        "@@template:\n^job:{0}=x\n\na\n",
        "@@template:\n^job:{0}=3\n\na\n",
        "@@template:\n{0}\n\n^job:a=x\n",
    ] {
        let output = capture(&temp, &vault)
            .args(["--dry-run", "-f", "json"])
            .arg(draft)
            .output()
            .expect("run session-suffix refusal");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{draft:?}: {}",
            format_output(&output)
        );
        let error = json(&output);
        assert_eq!(error["code"], "template_session_operator", "{draft:?}");
        assert!(
            error["error"].as_str().unwrap().contains("template row 1"),
            "{draft:?}: {}",
            error["error"]
        );
    }
}

#[test]
fn template_execution_reports_first_issue_in_draft_order() {
    let temp = TempDir::new("bob-cli-capture-template-draft-order");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let probe = "@@template:\n{0}{1}\n\n= \"\"\na\n";
    let output = capture(&temp, &vault)
        .args(["--dry-run", "-f", "json"])
        .arg(probe)
        .output()
        .expect("run draft-order probe");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let error = json(&output);
    assert_eq!(error["code"], "template_session_operator");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .starts_with("template row 1 (line 4): a template can't drive"),
        "{}",
        error["error"]
    );

    let arity_only = "@@template:\n{0} {1}\n\nonly-one\n";
    let output = capture(&temp, &vault)
        .args(["--dry-run", "-f", "json"])
        .arg(arity_only)
        .output()
        .expect("run arity-only draft");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let error = json(&output);
    assert_eq!(error["code"], "template_row_arity");
}

fn shelf_path(temp: &TempDir) -> std::path::PathBuf {
    temp.path()
        .join("state")
        .join("bob-cli")
        .join("capture")
        .join("templates.json")
}

fn read_shelf(path: &std::path::Path) -> serde_json::Value {
    let bytes = fs::read(path).expect("read shelf");
    serde_json::from_slice(&bytes).expect("parse shelf")
}

#[test]
fn real_template_capture_saves_and_reports_statuses() {
    let temp = TempDir::new("bob-cli-capture-template-shelf");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let draft = "@@template:\nApply to {0}\n\nAcme\n";

    let first = capture(&temp, &vault)
        .args(["-f", "json"])
        .arg(draft)
        .output()
        .expect("run first template capture");
    assert_success(&first);
    let value = json(&first);
    assert_eq!(value["template"]["saved"]["status"], "new");
    let id = value["template"]["saved"]["id"]
        .as_str()
        .expect("saved id")
        .to_string();
    assert_eq!(id.len(), 12);
    let shelf = read_shelf(&shelf_path(&temp));
    assert_eq!(shelf["schema_version"], 1);
    assert_eq!(shelf["templates"].as_array().unwrap().len(), 1);
    assert_eq!(shelf["templates"][0]["id"], id.as_str());
    assert_eq!(shelf["templates"][0]["use_count"], 1);

    let second = capture(&temp, &vault)
        .args(["-f", "json"])
        .arg(draft)
        .output()
        .expect("run second template capture");
    assert_success(&second);
    let value = json(&second);
    assert_eq!(value["template"]["saved"]["status"], "updated");
    assert_eq!(value["template"]["saved"]["id"], id.as_str());
    assert_eq!(
        read_shelf(&shelf_path(&temp))["templates"][0]["use_count"],
        2
    );

    let edited = "@@template:\nApply to {0}!\n\nAcme\n";
    let replaced = capture(&temp, &vault)
        .args(["-f", "json", "--template-origin"])
        .arg(&id)
        .arg(edited)
        .output()
        .expect("run origin replace capture");
    assert_success(&replaced);
    let value = json(&replaced);
    assert_eq!(value["template"]["saved"]["status"], "replaced");
    let new_id = value["template"]["saved"]["id"]
        .as_str()
        .expect("replaced id")
        .to_string();
    assert_ne!(new_id, id);
    let shelf = read_shelf(&shelf_path(&temp));
    assert_eq!(shelf["templates"].as_array().unwrap().len(), 1);
    assert_eq!(shelf["templates"][0]["id"], new_id.as_str());

    let missing_origin = capture(&temp, &vault)
        .args(["-f", "json", "--template-origin"])
        .arg("deadbeefcafe")
        .arg(draft)
        .output()
        .expect("run missing-origin capture");
    assert_success(&missing_origin);
    let value = json(&missing_origin);
    assert!(
        value["template"]["saved"]["status"] == "new"
            || value["template"]["saved"]["status"] == "updated",
        "{}",
        format_output(&missing_origin)
    );
}

#[test]
fn template_human_output_reports_saved() {
    let temp = TempDir::new("bob-cli-capture-template-shelf-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .arg("@@template:\nTask {0}\n\nfirst\n")
        .output()
        .expect("run human template capture");
    assert_success(&output);
    assert!(
        stdout(&output).contains("· saved"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn non_saving_runs_leave_the_shelf_absent() {
    let temp = TempDir::new("bob-cli-capture-template-shelf-absent");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let draft = "@@template:\nTask {0}\n\nfirst\n";
    let shelf = shelf_path(&temp);

    let dry = capture(&temp, &vault)
        .args(["--dry-run", "-f", "json"])
        .arg(draft)
        .output()
        .expect("dry run");
    assert_success(&dry);
    assert!(json(&dry)["template"].get("saved").is_none());
    assert!(json(&dry)["template"].get("not_saved").is_none());
    assert!(!shelf.exists());

    let expand = capture(&temp, &vault)
        .arg("-E")
        .arg(draft)
        .output()
        .expect("expand");
    assert_success(&expand);
    assert!(!shelf.exists());

    let failed = capture(&temp, &vault)
        .arg("@@template:\nTask {0} {2}\n\nA B C\n")
        .output()
        .expect("failed capture");
    assert_ne!(failed.status.code(), Some(0));
    assert!(!shelf.exists());

    let headerless = capture(&temp, &vault)
        .args(["-f", "json"])
        .arg("just a task")
        .output()
        .expect("headerless capture");
    assert_success(&headerless);
    let value = json(&headerless);
    assert!(value.get("template").is_none());
    assert!(!shelf.exists());

    let parse = {
        let mut command = bob_command();
        command
            .arg("capture-parse")
            .arg("-b")
            .arg(&vault)
            .env("BOB_NOW", NOW)
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .args(["-f", "json"])
            .arg(draft)
            .output()
            .expect("capture-parse")
    };
    assert_success(&parse);
    assert!(!shelf.exists());

    let complete = {
        let mut command = bob_command();
        command
            .arg("capture-complete")
            .arg("-b")
            .arg(&vault)
            .env("BOB_NOW", NOW)
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .args(["-f", "json"])
            .arg(draft)
            .output()
            .expect("capture-complete")
    };
    assert!(!shelf.exists());
    drop(complete);
}

#[test]
fn one_row_and_many_rows_save_the_same_shelf_entry() {
    let temp = TempDir::new("bob-cli-capture-template-shelf-rows");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    for draft in [
        "@@template:\nApply to {0}\n\none\n",
        "@@template:\nApply to {0}\n\none\ntwo\nthree\n",
    ] {
        let output = capture(&temp, &vault)
            .args(["-f", "json"])
            .arg(draft)
            .output()
            .expect("run row capture");
        assert_success(&output);
    }
    let shelf = read_shelf(&shelf_path(&temp));
    assert_eq!(shelf["templates"].as_array().unwrap().len(), 1);
}

#[test]
fn unwritable_shelf_keeps_exit_zero_with_not_saved() {
    let temp = TempDir::new("bob-cli-capture-template-shelf-unwritable");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let blocker = temp.path().join("blocker");
    fs::write(&blocker, "not a dir").expect("write blocker");
    let output = capture(&temp, &vault)
        .env("BOB_CAPTURE_TEMPLATES_FILE", blocker.join("templates.json"))
        .args(["-f", "json"])
        .arg("@@template:\nTask {0}\n\nfirst\n")
        .output()
        .expect("run unwritable capture");
    assert_success(&output);
    let value = json(&output);
    assert!(value["template"]["not_saved"].is_string());
    assert!(value["template"].get("saved").is_none());
    assert!(
        stderr(&output).contains("template not saved"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn bad_template_origin_is_a_usage_error() {
    let temp = TempDir::new("bob-cli-capture-template-origin-bad");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .args(["--template-origin", "not an id!"])
        .arg("@@template:\nTask {0}\n\nfirst\n")
        .output()
        .expect("run bad origin");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn expand_with_bad_template_origin_is_a_usage_error() {
    let temp = TempDir::new("bob-cli-capture-template-origin-expand-bad");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .args(["-E", "--template-origin", "bad!"])
        .arg("@@template:\nTask {0}\n\nfirst\n")
        .output()
        .expect("run bad origin expand");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(!shelf_path(&temp).exists());
}

#[test]
fn human_replaced_names_the_checked_out_origin() {
    let temp = TempDir::new("bob-cli-capture-template-human-replaced");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let first = capture(&temp, &vault)
        .args(["-f", "json"])
        .arg("@@template:\nApply to {0}\n\nAcme\n")
        .output()
        .expect("run first capture");
    assert_success(&first);
    let origin = json(&first)["template"]["saved"]["id"]
        .as_str()
        .expect("origin id")
        .to_string();
    let replaced = capture(&temp, &vault)
        .args(["--template-origin"])
        .arg(&origin)
        .arg("@@template:\nApply to {0}!\n\nAcme\n")
        .output()
        .expect("run replacing capture");
    assert_success(&replaced);
    let expected = format!("· saved, replacing {origin}");
    assert!(
        stdout(&replaced).contains(&expected),
        "{}",
        format_output(&replaced)
    );
}

#[test]
fn plain_captures_stay_byte_identical_without_a_header() {
    let temp = TempDir::new("bob-cli-capture-template-plain-identical");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .args(["-f", "json"])
        .arg("buy milk @groceries")
        .output()
        .expect("run plain capture");
    assert_success(&output);
    let value = json(&output);
    assert!(value.get("template").is_none());
    assert!(!shelf_path(&temp).exists());
    let human = capture(&temp, &vault)
        .arg("buy more milk @groceries")
        .output()
        .expect("run plain human capture");
    assert_success(&human);
    assert!(
        !stdout(&human).contains("· saved")
            && !stdout(&human).contains("not saved"),
        "{}",
        format_output(&human)
    );
}

#[test]
fn template_one_placeholder_one_line_dry_run_matches_three_lines() {
    let temp = TempDir::new("bob-cli-capture-template-one-line");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let one_line = "@@template:\nTask {0}\n\nbar baz bam\n";
    let three_line = "@@template:\nTask {0}\n\nbar\nbaz\nbam\n";

    let one = capture(&temp, &vault)
        .args(["--dry-run", "-f", "json"])
        .arg(one_line)
        .output()
        .expect("run one-line dry-run");
    assert_success(&one);
    let one_value = json(&one);
    assert_eq!(one_value["template"]["row_count"], 3);
    assert_eq!(
        one_value["captures"][0]["template_row"],
        serde_json::json!({"row": 1, "line": 4})
    );
    assert_eq!(
        one_value["captures"][1]["template_row"],
        serde_json::json!({"row": 2, "line": 4})
    );
    assert_eq!(
        one_value["captures"][2]["template_row"],
        serde_json::json!({"row": 3, "line": 4})
    );

    let three = capture(&temp, &vault)
        .args(["--dry-run", "-f", "json"])
        .arg(three_line)
        .output()
        .expect("run three-line dry-run");
    assert_success(&three);
    let three_value = json(&three);
    for index in 0..3 {
        assert_eq!(
            one_value["captures"][index]["text"],
            three_value["captures"][index]["text"],
            "body differs at index {index}"
        );
        assert_eq!(
            one_value["captures"][index]["task_line"],
            three_value["captures"][index]["task_line"],
            "task line differs at index {index}"
        );
    }
}

#[test]
fn template_url_listen_marker_is_not_clipboard_capture() {
    let temp = TempDir::new("bob-cli-capture-template-listen");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .args(["--dry-run", "--no-clip", "-f", "json"])
        .arg("@@template:\n{0}\n\n\"https://example.com/one %\"\n")
        .output()
        .expect("run URL listen template");
    assert_success(&output);
    let value = json(&output);
    assert_eq!(value["kind"], "ref");
    assert_eq!(value["ref"]["listen"], true);
    assert_eq!(
        value["template_row"],
        serde_json::json!({"row": 1, "line": 4})
    );

    let unquoted = capture(&temp, &vault)
        .args(["--dry-run", "--no-clip", "-f", "json"])
        .arg("@@template:\n{0}\n\nhttps://example.com/one %\n")
        .output()
        .expect("run unquoted URL listen template");
    assert_eq!(
        unquoted.status.code(),
        Some(2),
        "{}",
        format_output(&unquoted)
    );
    let error = json(&unquoted);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("template row 2 (line 4)"),
        "{}",
        error["error"]
    );
}
