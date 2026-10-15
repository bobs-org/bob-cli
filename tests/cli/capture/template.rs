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
fn template_url_listen_marker_is_not_clipboard_capture() {
    let temp = TempDir::new("bob-cli-capture-template-listen");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = capture(&temp, &vault)
        .args(["--dry-run", "--no-clip", "-f", "json"])
        .arg("@@template:\n{0}\n\nhttps://example.com/one %\n")
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
}
