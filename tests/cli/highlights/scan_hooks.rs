//! Pre-scan hook and legacy hook-key tests.

use crate::support::*;
use std::fs;

#[test]
fn highlights_ref_scan_runs_configured_pre_scan_before_xlib_intake() {
    let temp = TempDir::new("bob-cli-highlights-ref-pre-scan");
    let vault = temp.path().join("vault");
    let seed_pdf = temp.path().join("seed.pdf");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let pre_scan_log = temp.path().join("pre-scan.log");
    let destination_pdf = vault.join("lib/chat/from-hook.pdf");
    let note = vault.join("ref/chat/from-hook.md");
    fs::create_dir_all(&vault).expect("create vault");
    write_highlights_pdf(
        &seed_pdf,
        "- status: wip\n- parent: obsidian\n- title: From Hook\n",
    );
    write_executable(
        &script,
        "#!/bin/sh\n\
         set -eu\n\
         printf 'pre-scan stdout from %s\\n' \"$PWD\"\n\
         printf 'pre-scan stderr\\n' >&2\n\
         mkdir -p xlib/chat\n\
         cp \"$SOURCE_PDF\" xlib/chat/from-hook.pdf\n\
         printf '%s\\n' \"$PWD\" > \"$PRE_SCAN_LOG\"\n",
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("SOURCE_PDF", &seed_pdf)
        .env("PRE_SCAN_LOG", &pre_scan_log)
        .output()
        .expect("scan with pre-scan hook");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pre_scan_hook: run")
            && report.contains("pre-scan stdout from")
            && report.contains("intake: moved xlib/chat/from-hook.pdf -> lib/chat/from-hook.pdf")
            && report.contains("notes_created: 1"),
        "expected pre-scan hook to populate xlib before intake:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("pre-scan stderr"),
        "pre-scan stderr should be inherited:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&pre_scan_log).expect("read pre-scan pwd log"),
        format!("{}\n", path_str(&vault)),
        "pre-scan hook must run from BOB_DIR"
    );
    assert!(destination_pdf.is_file(), "pre-scan PDF was not intaked");
    assert!(
        !vault.join("xlib/chat/from-hook.pdf").exists(),
        "pre-scan PDF should be moved out of xlib"
    );
    assert!(note.is_file(), "scan should write the note after intake");
}

#[test]
fn highlights_ref_scan_dry_run_reports_env_pre_scan_without_executing() {
    let temp = TempDir::new("bob-cli-highlights-ref-pre-scan-dry-run");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/dry-run.pdf");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    let command = shell_single_quote(path_str(&script));
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Dry Run\n",
    );
    write_file(
        &config,
        "highlights:\n  pre_scan_hook: should-not-use-file-config\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_HIGHLIGHTS_PRE_SCAN_HOOK", &command)
        .output()
        .expect("dry-run scan with pre-scan hook");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(&format!("pre_scan_hook: would-run {command}")),
        "dry-run should report the env override hook:\n{}",
        format_output(&output)
    );
    assert!(
        !report.contains("should-not-use-file-config"),
        "env override should replace file config:\n{report}"
    );
    assert!(
        !sentinel.exists(),
        "dry-run must not execute the pre-scan hook"
    );
    assert!(
        !vault.join("ref/chat/dry-run.md").exists(),
        "dry-run must not write the reference note"
    );
}

#[test]
fn highlights_ref_scan_empty_env_disables_configured_pre_scan() {
    let temp = TempDir::new("bob-cli-highlights-ref-pre-scan-empty-env");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/disabled.pdf");
    let note = vault.join("ref/chat/disabled.md");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Disabled Hook\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 29\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_HIGHLIGHTS_PRE_SCAN_HOOK", "")
        .output()
        .expect("scan with empty pre-scan override");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pre_scan_hook:"),
        "empty env override should disable the configured hook:\n{report}"
    );
    assert!(
        !sentinel.exists(),
        "disabled pre-scan hook must not execute"
    );
    assert!(note.is_file(), "scan should still process existing PDFs");
}

#[test]
fn highlights_ref_scan_fails_when_pre_scan_hook_fails() {
    let temp = TempDir::new("bob-cli-highlights-ref-pre-scan-fail");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &script,
        "#!/bin/sh\n\
         printf 'pre-scan stdout before failure\\n'\n\
         printf 'pre-scan stderr before failure\\n' >&2\n\
         exit 17\n",
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("scan with failing pre-scan hook");

    assert_eq!(
        output.status.code(),
        Some(1),
        "failing pre-scan hook should abort scan:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("pre-scan stdout before failure"),
        "pre-scan stdout should be inherited:\n{}",
        format_output(&output)
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("pre-scan stderr before failure")
            && diagnostic.contains("pre-scan hook failed with exit 17"),
        "expected failing pre-scan diagnostics:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("lib").exists(),
        "scan should abort before intake or library inspection"
    );
}

#[test]
fn highlights_ref_scan_no_hooks_before_subcommand_skips_configured_hook() {
    let temp = TempDir::new("bob-cli-highlights-ref-no-hooks-before");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/existing.pdf");
    let note = vault.join("ref/chat/existing.md");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Existing\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 29\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("scan with --no-hooks before subcommand");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pre_scan_hook:"),
        "--no-hooks must skip the hook without reporting it:\n{report}"
    );
    assert!(
        !sentinel.exists(),
        "--no-hooks must not execute the configured hook"
    );
    assert!(note.is_file(), "scan should still write notes for lib PDFs");
}

#[test]
fn highlights_ref_scan_no_hooks_after_subcommand_skips_configured_hook() {
    let temp = TempDir::new("bob-cli-highlights-ref-no-hooks-after");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/existing.pdf");
    let note = vault.join("ref/chat/existing.md");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Existing\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 29\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--no-hooks")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("scan with --no-hooks after subcommand");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pre_scan_hook:"),
        "--no-hooks must skip the hook without reporting it:\n{report}"
    );
    assert!(
        !sentinel.exists(),
        "--no-hooks must not execute the configured hook"
    );
    assert!(note.is_file(), "scan should still write notes for lib PDFs");
}

#[test]
fn highlights_ref_scan_no_hooks_overrides_env_hook() {
    let temp = TempDir::new("bob-cli-highlights-ref-no-hooks-env");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/existing.pdf");
    let note = vault.join("ref/chat/existing.md");
    let script = temp.path().join("pre-scan");
    let sentinel = temp.path().join("pre-scan-ran");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Existing\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 29\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--no-hooks")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_HIGHLIGHTS_PRE_SCAN_HOOK", path_str(&script))
        .output()
        .expect("scan with --no-hooks overriding env hook");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pre_scan_hook:"),
        "--no-hooks must override the env hook:\n{report}"
    );
    assert!(!sentinel.exists(), "env hook must not execute");
    assert!(note.is_file(), "scan should still write notes for lib PDFs");
}

#[test]
fn highlights_ref_scan_hook_child_sees_in_hook_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-hook-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/chat/existing.pdf");
    let config = temp.path().join("config.yml");
    let script = temp.path().join("pre-scan");
    let marker_log = temp.path().join("hook-marker.log");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Existing\n",
    );
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf '%s' \"${{BOB_HIGHLIGHTS_IN_PRE_SCAN_HOOK:-empty}}\" > {}\n",
            shell_single_quote(path_str(&marker_log))
        ),
    );
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("scan with marker-logging hook");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&marker_log).expect("read hook marker log"),
        "1",
        "hook child must see BOB_HIGHLIGHTS_IN_PRE_SCAN_HOOK=1"
    );
}

#[test]
fn highlights_ref_scan_rejects_legacy_pre_scan_command_key() {
    let temp = TempDir::new("bob-cli-highlights-ref-legacy-key");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&config, "highlights:\n  pre_scan_command: bob_xlib_pull\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("scan with legacy config key");

    assert_eq!(
        output.status.code(),
        Some(1),
        "legacy config key must fail scan:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("pre_scan_hook"),
        "legacy rejection must name the new spelling:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_scan_rejects_legacy_pre_scan_env() {
    let temp = TempDir::new("bob-cli-highlights-ref-legacy-env");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("BOB_HIGHLIGHTS_PRE_SCAN_COMMAND", "true")
        .output()
        .expect("scan with legacy env var");

    assert_eq!(
        output.status.code(),
        Some(1),
        "legacy env var must fail scan:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("BOB_HIGHLIGHTS_PRE_SCAN_HOOK"),
        "legacy env rejection must name the new spelling:\n{}",
        format_output(&output)
    );
}
