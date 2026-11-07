//! `bob clipboard`: read-only counted-history diagnostics.

use crate::support::*;

fn history_rig(
    name: &str,
    history_json: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let live = temp.path().join("live-clipboard");
    let history = temp.path().join("history-command");
    write_executable(
        &live,
        "#!/bin/sh\nprintf '%s\\n' 'synthetic-live-value'\n",
    );
    write_executable(
        &history,
        &format!("#!/bin/sh\nprintf '%s\\n' '{history_json}'\n"),
    );
    (temp, live, history)
}

fn clipboard_command(
    live: &std::path::Path,
    history: Option<&std::path::Path>,
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("clipboard")
        .env("BOB_CLIPBOARD_CMD", live)
        .env_remove("BOB_CLIPBOARD_HISTORY_CMD");
    if let Some(history) = history {
        command.env("BOB_CLIPBOARD_HISTORY_CMD", history);
    }
    command
}

fn command_provider(report: &serde_json::Value) -> &serde_json::Value {
    // On macOS the diagnostic `providers` list leads with the Alfred and
    // Clipy rows and the selected command provider comes last; elsewhere
    // it is the only row. Select by name so the assertions below do not
    // depend on host or platform row order.
    report["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|row| row["provider"] == "command")
        .expect("command provider row")
}

#[test]
fn healthy_command_provider_reports_only_metadata() {
    let (_temp, live, history) = history_rig(
        "bob-cli-clipboard-healthy",
        r#"["synthetic-live-value","older-one","older-two"]"#,
    );
    let output = clipboard_command(&live, Some(&history))
        .args(["-f", "json"])
        .output()
        .expect("run bob clipboard");

    assert_success(&output);
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("status JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["would_succeed"], true);
    assert_eq!(report["selected_provider"], "command");
    assert_eq!(report["selection"]["source"], "environment");
    let command = command_provider(&report);
    assert_eq!(command["selected"], true);
    assert_eq!(command["head_agrees_with_live"], true);
    assert_no_clipboard_content(&output);
}

#[test]
fn stale_command_provider_refuses_with_stable_code_and_redacts_content() {
    let (_temp, live, history) = history_rig(
        "bob-cli-clipboard-stale",
        r#"["stale-history-value","older-one","older-two"]"#,
    );
    let output = clipboard_command(&live, Some(&history))
        .args(["-f", "json"])
        .output()
        .expect("run bob clipboard");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("status JSON");
    assert_eq!(report["would_succeed"], false);
    assert_eq!(report["refusal"]["code"], "clipboard_history_mismatch");
    assert_eq!(command_provider(&report)["head_agrees_with_live"], false);
    assert_no_clipboard_content(&output);
}

#[test]
fn off_selection_refuses_with_stable_code() {
    let temp = TempDir::new("bob-cli-clipboard-off");
    let live = temp.path().join("live-clipboard");
    let config = temp.path().join("config.yml");
    write_executable(
        &live,
        "#!/bin/sh\nprintf '%s\\n' 'synthetic-live-value'\n",
    );
    write_file(&config, "capture:\n  clipboard_history: off\n");
    let output = bob_command()
        .args(["clipboard", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_CLIPBOARD_CMD", &live)
        .env_remove("BOB_CLIPBOARD_HISTORY_CMD")
        .output()
        .expect("run bob clipboard");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("status JSON");
    assert_eq!(report["selection"]["value"], "off");
    assert_eq!(report["selection"]["source"], "config");
    assert_eq!(report["refusal"]["code"], "clipboard_history_unavailable");
    assert_no_clipboard_content(&output);
}

#[test]
fn help_and_usage_are_listed_under_integrations() {
    let root = bob_command()
        .arg("--help")
        .output()
        .expect("run bob --help");
    assert_success(&root);
    let root_help = stdout(&root);
    assert!(root_help.contains("Integrations:"));
    assert_text_order(
        &root_help,
        &["Integrations:", "clipboard", "gcal", "gkeep"],
    );

    let help = bob_command()
        .args(["clipboard", "--help"])
        .output()
        .expect("run bob clipboard --help");
    assert_success(&help);
    let help_text = stdout(&help);
    assert!(help_text.contains("never prints clipboard or history contents"));
    assert_text_order(
        &help_text,
        &["-c, --count", "-f, --format", "-h, --help"],
    );

    let usage = bob_command()
        .args(["clipboard", "-c", "1"])
        .output()
        .expect("run bob clipboard with invalid count");
    assert_eq!(usage.status.code(), Some(2));
}

fn assert_no_clipboard_content(output: &std::process::Output) {
    let all = format!("{}{}", stdout(output), stderr(output));
    for secret in [
        "synthetic-live-value",
        "stale-history-value",
        "older-one",
        "older-two",
    ] {
        assert!(!all.contains(secret), "clipboard value leaked: {all}");
    }
}
