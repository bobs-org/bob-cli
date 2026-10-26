//! Non-TTY color, progress, and exit receipts (`CLICOLOR_FORCE`,
//! `BOB_PROGRESS`, `BOB_RUN_RECEIPT`).

use crate::support::*;
use serde_json::Value;
use std::fs;

#[test]
fn clicolor_force_emits_ansi_on_piped_stdout() {
    let output = bob_command()
        .arg("--help")
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "1")
        .output()
        .expect("run bob --help");
    assert_success(&output);
    assert!(
        output.stdout.contains(&0x1b),
        "CLICOLOR_FORCE=1 should color piped help:\n{}",
        stdout(&output)
    );
}

#[test]
fn no_color_wins_over_clicolor_force() {
    let output = bob_command()
        .arg("--help")
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .output()
        .expect("run bob --help");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn clicolor_force_zero_emits_no_ansi_on_piped_stdout() {
    let output = bob_command()
        .arg("--help")
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "0")
        .output()
        .expect("run bob --help");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn bob_progress_emits_a_line_on_piped_stderr() {
    let temp = TempDir::new("bob-cli-run-env-progress");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
    let curl = temp.path().join("curl.sh");
    write_executable(&curl, "#!/bin/sh\nexit 1\n");

    let mut command = bob_command();
    command
        .args(["ref", "create", "-P", "sase", "https://example.com/hello"])
        .env("BOB_DIR", &vault)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env_remove("NO_COLOR");

    let quiet = command.output().expect("create without progress");
    assert!(
        !stderr(&quiet).contains("fetching example.com"),
        "piped stderr stays quiet without BOB_PROGRESS:\n{}",
        format_output(&quiet)
    );

    command.env("BOB_PROGRESS", "1");
    let output = command.output().expect("create with progress");
    let err = stderr(&output);
    assert!(
        err.contains("fetching example.com…") && !err.contains('\r'),
        "BOB_PROGRESS=1 should print a line-oriented fetch:\n{}",
        format_output(&output)
    );
}

fn receipt_command(
    receipt: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    bob_command()
        .args(args)
        .env("BOB_RUN_RECEIPT", receipt)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|error| panic!("run bob {}: {error}", args.join(" ")))
}

fn assert_receipt(path: &std::path::Path, exit_code: i32) {
    let parsed: Value = serde_json::from_str(
        &fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read receipt: {error}")),
    )
    .expect("receipt json");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["exit_code"], exit_code);
    let finished = parsed["finished_at"].as_str().expect("finished_at");
    assert!(
        finished.contains('T')
            && (finished.ends_with('Z') || finished.contains('+')),
        "finished_at should be RFC3339: {finished}"
    );
}

#[test]
fn run_receipt_records_success_usage_and_failure() {
    let temp = TempDir::new("bob-cli-run-env-receipt");
    let ok_path = temp.path().join("ok.rc");
    let usage_path = temp.path().join("usage.rc");
    let fail_path = temp.path().join("fail.rc");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");

    let ok = receipt_command(&ok_path, &["--help"]);
    assert_eq!(ok.status.code(), Some(0), "{}", format_output(&ok));
    assert_receipt(&ok_path, 0);

    let usage = receipt_command(&usage_path, &["__catalog"]);
    assert_eq!(usage.status.code(), Some(2), "{}", format_output(&usage));
    assert_receipt(&usage_path, 2);

    let fail = bob_command()
        .args(["ref", "show", "missing-ref"])
        .env("BOB_DIR", &vault)
        .env("BOB_RUN_RECEIPT", &fail_path)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob ref show");
    assert_eq!(fail.status.code(), Some(1), "{}", format_output(&fail));
    assert_receipt(&fail_path, 1);
}

#[test]
fn unwritable_run_receipt_warns_and_keeps_the_exit_code() {
    let temp = TempDir::new("bob-cli-run-env-receipt-unwritable");
    let receipt = temp.path().join("missing").join("receipt.rc");
    let output = receipt_command(&receipt, &["--help"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains("warning:") && err.contains("BOB_RUN_RECEIPT"),
        "failed receipt write should warn:\n{}",
        format_output(&output)
    );
    assert!(
        !receipt.exists(),
        "unwritable receipt path must stay absent"
    );
}
