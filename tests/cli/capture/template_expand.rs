use crate::support::*;

#[test]
fn capture_expand_prints_template_text_and_json_summary() {
    let draft = "@@template:\nApply {0} to {1}\n\n\"Two Sigma\" job\n";
    let human = bob_command()
        .args(["capture", "-E"])
        .arg(draft)
        .output()
        .expect("run capture expansion");
    assert_success(&human);
    assert_eq!(stdout(&human), "Apply Two Sigma to job\n");
    assert!(stderr(&human).is_empty());

    let json = bob_command()
        .args(["capture", "--expand", "-f", "json"])
        .arg(draft)
        .output()
        .expect("run capture expansion json");
    assert_success(&json);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("expansion JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["expanded"], "Apply Two Sigma to job\n");
    assert_eq!(value["template"]["line"], 1);
    assert_eq!(value["template"]["field_count"], 2);
    assert_eq!(value["template"]["row_count"], 1);
}

#[test]
fn capture_expand_is_identity_for_plain_drafts() {
    let draft = "plain draft\r\nwithout a final newline";
    let output = bob_command()
        .args(["capture", "-E"])
        .arg(draft)
        .output()
        .expect("run identity expansion");
    assert_success(&output);
    assert_eq!(stdout(&output), draft);

    let json = bob_command()
        .args(["capture", "-E", "-f", "json"])
        .arg(draft)
        .output()
        .expect("run identity expansion json");
    assert_success(&json);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("expansion JSON");
    assert_eq!(value["expanded"], draft);
    assert!(value.get("template").is_none());
}

#[test]
fn capture_expand_ignores_capture_options_and_never_needs_a_vault() {
    let output = bob_command()
        .args([
            "capture",
            "-E",
            "--bob-dir",
            "/this/vault/does/not/exist",
            "--route",
            "invalid route ignored",
            "--clip",
            "--dry-run",
        ])
        .arg("@@template:\nTask {0}\n\nvalue\n")
        .output()
        .expect("run expansion with unrelated capture options");
    assert_success(&output);
    assert_eq!(stdout(&output), "Task value\n");
    assert!(stderr(&output).is_empty());
}

#[test]
fn capture_expand_reports_language_errors_as_usage_errors() {
    let output = bob_command()
        .args(["capture", "-E", "-f", "json"])
        .arg("@@template:\nTask {0} {2}\n\nA B C\n")
        .output()
        .expect("run invalid expansion");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(value["ok"], false);
    assert!(value["error"].as_str().unwrap().contains("not {1}"));
}
