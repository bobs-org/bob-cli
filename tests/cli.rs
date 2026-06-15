use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const BOB_NOTIFY_BIN: &str = env!("CARGO_BIN_EXE_bob_notify");
const BOB_POMODORO_BIN: &str = env!("CARGO_BIN_EXE_bob_pomodoro");
const BOB_SYNC_BIN: &str = env!("CARGO_BIN_EXE_bob_sync");
const TMUX_BOB_POMODORO_BIN: &str = env!("CARGO_BIN_EXE_tmux_bob_pomodoro");

struct LegacyHelpCase {
    command: fn() -> Command,
    name: &'static str,
    marker: &'static str,
}

#[test]
fn cache_extraction_writes_expected_files_and_modes() {
    let temp = TempDir::new("bob-cli-cache");
    let output = bob_command()
        .arg("notify")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run bob notify --help");

    assert_success(&output);

    let script_dir = single_script_cache_dir(temp.path());
    let executable_assets = [
        "bob_pomodoro",
        "bob_notify",
        "bob_sync",
        "tmux_bob_pomodoro",
    ];

    for asset in executable_assets {
        let path = script_dir.join(asset);
        assert!(
            path.is_file(),
            "missing extracted asset: {}",
            path.display()
        );
        assert_unix_mode(&path, 0o755);
    }

    let helper = script_dir.join("lib/bob_shell.sh");
    assert!(
        helper.is_file(),
        "missing extracted helper: {}",
        helper.display()
    );
    assert_unix_mode(&helper, 0o644);
}

#[test]
fn move_done_tasks_help_is_native_only() {
    let temp = TempDir::new("bob-cli-move-done-tasks-native-help");
    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob move-done-tasks --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob move-done-tasks"),
        "expected move-done-tasks help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only move-done-tasks should not extract script assets"
    );
}

#[test]
fn bulk_git_commit_help_is_native_only() {
    let temp = TempDir::new("bob-cli-bulk-git-commit-native-help");
    let output = bob_command()
        .arg("bulk-git-commit")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob bulk-git-commit --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob bulk-git-commit"),
        "expected bulk-git-commit help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only bulk-git-commit should not extract script assets"
    );
}

#[test]
fn capture_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-native-help");
    let output = bob_command()
        .arg("capture")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture"),
        "expected capture help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn dataview_help_is_native_only() {
    let temp = TempDir::new("bob-cli-dataview-native-help");
    let output = bob_command()
        .arg("dataview")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob dataview --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob dataview"),
        "expected dataview help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only dataview should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn projects_help_is_native_only() {
    let temp = TempDir::new("bob-cli-projects-native-help");
    let output = bob_command()
        .arg("projects")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob projects --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob projects"),
        "expected projects help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only projects should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_help_is_native_only() {
    let temp = TempDir::new("bob-cli-highlights-ref-native-help");
    let output = bob_command()
        .arg("highlights")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob highlights --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob highlights"),
        "expected highlights help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only highlights should not extract script assets"
    );
}

#[test]
fn highlights_ref_subcommand_help_works() {
    let cases: &[&[&str]] = &[
        &["highlights", "--help"],
        &["highlights", "scan", "--help"],
        &["highlights", "sync", "--help"],
        &["highlights", "doctor", "--help"],
        &["highlights", "marker", "--help"],
    ];

    for args in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains("Usage: bob highlights"),
            "expected highlights usage for {args:?}:\n{}",
            format_output(&output)
        );
        assert!(
            !output.stdout.contains(&0x1b),
            "piped help output must not contain ANSI escape codes:\n{help}"
        );
    }
}

#[test]
fn all_top_level_subcommand_help_is_safe_and_plain() {
    let cases: &[(&[&str], &str)] = &[
        (&["bulk-git-commit", "--help"], "usage: bob bulk-git-commit"),
        (&["capture", "--help"], "bob capture"),
        (&["dataview", "--help"], "bob dataview"),
        (&["highlights", "--help"], "Usage: bob highlights"),
        (&["move-done-tasks", "--help"], "usage: bob move-done-tasks"),
        (&["nightly", "--help"], "usage: bob nightly"),
        (&["notify", "--help"], "Notify me when"),
        (&["pomodoro", "--help"], "usage: bob pomodoro"),
        (&["projects", "--help"], "bob projects"),
        (&["tmux-pomodoro", "--help"], "usage: bob tmux-pomodoro"),
    ];

    for (args, marker) in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains(marker),
            "expected `{marker}` in help for {args:?}:\n{}",
            format_output(&output)
        );
        assert_stdout_has_no_ansi(&output);
    }
}

#[test]
fn public_help_surfaces_do_not_list_long_only_options() {
    let bob_cases: &[(&[&str], &str)] = &[
        (&["--help"], "bob --help"),
        (&["bulk-git-commit", "--help"], "bob bulk-git-commit --help"),
        (&["capture", "--help"], "bob capture --help"),
        (&["dataview", "--help"], "bob dataview --help"),
        (&["highlights", "--help"], "bob highlights --help"),
        (
            &["highlights", "doctor", "--help"],
            "bob highlights doctor --help",
        ),
        (
            &["highlights", "marker", "--help"],
            "bob highlights marker --help",
        ),
        (
            &["highlights", "scan", "--help"],
            "bob highlights scan --help",
        ),
        (
            &["highlights", "sync", "--help"],
            "bob highlights sync --help",
        ),
        (&["move-done-tasks", "--help"], "bob move-done-tasks --help"),
        (&["nightly", "--help"], "bob nightly --help"),
        (&["notify", "--help"], "bob notify --help"),
        (&["pomodoro", "--help"], "bob pomodoro --help"),
        (&["projects", "--help"], "bob projects --help"),
        (&["projects", "list", "--help"], "bob projects list --help"),
        (&["projects", "sync", "--help"], "bob projects sync --help"),
        (&["tmux-pomodoro", "--help"], "bob tmux-pomodoro --help"),
    ];

    for (args, label) in bob_cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run {label}: {error}"));

        assert_success(&output);
        assert_no_long_only_option_lines(label, &stdout(&output));
    }

    let legacy_cases = [
        (bob_pomodoro_command as fn() -> Command, "bob_pomodoro"),
        (bob_notify_command as fn() -> Command, "bob_notify"),
        (bob_sync_command as fn() -> Command, "bob_sync"),
        (
            tmux_bob_pomodoro_command as fn() -> Command,
            "tmux_bob_pomodoro",
        ),
    ];

    for (command, name) in legacy_cases {
        let output = command()
            .arg("--help")
            .output()
            .unwrap_or_else(|error| panic!("run {name} --help: {error}"));

        assert_success(&output);
        assert_no_long_only_option_lines(
            &format!("{name} --help"),
            &stdout(&output),
        );
    }
}

#[test]
fn legacy_binary_help_is_safe_and_plain() {
    let cases = [
        LegacyHelpCase {
            command: bob_pomodoro_command,
            name: "bob_pomodoro",
            marker: "Show the current Pomodoro status",
        },
        LegacyHelpCase {
            command: bob_notify_command,
            name: "bob_notify",
            marker: "Notify me when",
        },
        LegacyHelpCase {
            command: bob_sync_command,
            name: "bob_sync",
            marker: "Stage all Bob vault changes",
        },
        LegacyHelpCase {
            command: tmux_bob_pomodoro_command,
            name: "tmux_bob_pomodoro",
            marker: "Print the current Pomodoro status",
        },
    ];

    for case in cases {
        let output =
            (case.command)()
                .arg("--help")
                .output()
                .unwrap_or_else(|error| {
                    panic!("run {} --help: {error}", case.name)
                });

        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains(case.marker),
            "expected `{}` in {} help:\n{}",
            case.marker,
            case.name,
            format_output(&output)
        );
        assert_stdout_has_no_ansi(&output);
    }
}

#[test]
fn script_fallback_help_is_safe_and_plain() {
    let temp = TempDir::new("bob-cli-script-help");
    let cases: &[(&[&str], &str)] = &[
        (&["notify", "--help"], "Notify me when"),
        (&["pomodoro", "--help"], "Show the current Pomodoro status"),
        (
            &["tmux-pomodoro", "--help"],
            "Print the current Pomodoro status",
        ),
    ];

    for (args, marker) in cases {
        let output = bob_command()
            .args(*args)
            .env("BOB_CLI_USE_SCRIPT", "1")
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .output()
            .unwrap_or_else(|error| {
                panic!("run script fallback bob {args:?}: {error}")
            });

        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains(marker),
            "expected `{marker}` in script help for {args:?}:\n{}",
            format_output(&output)
        );
        assert_stdout_has_no_ansi(&output);
    }
}

#[test]
fn script_fallback_bob_sync_help_exits_before_work() {
    let temp = TempDir::new("bob-cli-script-bob-sync-help");
    let stub_bin = temp.path().join("bin");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("git"),
        "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$STUB_LOG\"\nexit 99\n",
    );
    write_executable(
        &stub_bin.join("ob"),
        "#!/bin/sh\nprintf 'ob %s\\n' \"$*\" >> \"$STUB_LOG\"\nexit 99\n",
    );

    let output = bob_sync_command()
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script fallback bob_sync --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob_sync"),
        "expected bob_sync script help:\n{}",
        format_output(&output)
    );
    assert!(
        !log.exists(),
        "bob_sync --help must not run ob or git:\n{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn nightly_help_exits_before_operational_work() {
    let temp = TempDir::new("bob-cli-nightly-help");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &stub_bin.join("git"),
        "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$STUB_LOG\"\nexit 99\n",
    );
    write_executable(
        &stub_bin.join("ob"),
        "#!/bin/sh\nprintf 'ob %s\\n' \"$*\" >> \"$STUB_LOG\"\nexit 99\n",
    );

    let output = bob_command()
        .arg("nightly")
        .arg("--help")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob nightly"),
        "expected nightly help:\n{}",
        format_output(&output)
    );
    assert!(
        !log.exists(),
        "bob nightly --help must not run ob or git:\n{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run bob capture --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Capture one task into the Bob Obsidian vault"),
        "expected capture long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-r, --route",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_unrouted_appends_to_mac_inbox() {
    let temp = TempDir::new("bob-cli-capture-inbox");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .output()
        .expect("run bob capture inbox");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("captured  mac_inbox.md")
            && out.contains("- [ ] #task buy milk [created::2026-06-15]"),
        "unexpected capture output:\n{out}"
    );
    assert_stdout_has_no_ansi(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task buy milk [created::2026-06-15]\n"
    );
}

#[test]
fn capture_routed_prefix_inserts_and_suffix_creates_file() {
    let temp = TempDir::new("bob-cli-capture-routed");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("groceries.md"),
        "# Groceries\n- [ ] #task existing\n  detail\n\nNext\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@Groceries")
        .arg("pick")
        .arg("apples")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run prefix routed capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  groceries.md"),
        "unexpected prefix capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("groceries.md"))
            .expect("read groceries"),
        "# Groceries\n- [ ] #task existing\n  detail\n- [ ] #task pick apples [created::2026-06-15]\n\nNext\n"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("call")
        .arg("vet")
        .arg("@Errands")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run suffix routed capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  errands.md"),
        "unexpected suffix capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("errands.md")).expect("read errands"),
        "- [ ] #task call vet [created::2026-06-15]\n"
    );
}

#[test]
fn capture_route_override_keeps_at_tokens_literal() {
    let temp = TempDir::new("bob-cli-capture-route-override");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("Work")
        .arg("buy")
        .arg("milk")
        .arg("@groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run forced-route capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        "- [ ] #task buy milk @groceries [created::2026-06-15]\n"
    );
    assert!(
        !vault.join("groceries.md").exists(),
        "--route should bypass auto @route parsing"
    );
}

#[test]
fn capture_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-capture-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("buy")
        .arg("milk")
        .arg("@groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run dry-run capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("[dry-run] ok would capture  groceries.md")
            && out.contains("- [ ] #task buy milk [created::2026-06-15]"),
        "unexpected dry-run output:\n{out}"
    );
    assert!(
        !vault.join("groceries.md").exists(),
        "dry-run must not create routed target"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_json_output_is_machine_readable() {
    let temp = TempDir::new("bob-cli-capture-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .arg("@Groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json capture");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "json capture should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["routed"], true);
    assert_eq!(json["route"], "groceries");
    assert_eq!(json["route_label"], "groceries.md");
    assert_eq!(json["relative_target"], "groceries.md");
    assert_eq!(
        json["target"],
        vault.join("groceries.md").display().to_string()
    );
    assert_eq!(json["text"], "buy milk");
    assert_eq!(
        json["task_line"],
        "- [ ] #task buy milk [created::2026-06-15]"
    );
    assert_eq!(json["created"], "2026-06-15");
    assert_eq!(json["placement"], "created");
}

#[test]
fn capture_reads_one_line_from_stdin_when_text_is_absent() {
    let temp = TempDir::new("bob-cli-capture-stdin");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut child = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin capture");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(b"ping team @work\nignored input\n")
        .expect("write stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait stdin capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        "- [ ] #task ping team [created::2026-06-15]\n"
    );
}

#[test]
fn capture_empty_input_is_usage_error() {
    let temp = TempDir::new("bob-cli-capture-empty");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run empty capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "empty capture should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty()
            && stderr(&output).contains("task text is required"),
        "expected empty-input error:\n{}",
        format_output(&output)
    );
}

#[test]
fn capture_json_failure_prints_error_object() {
    let temp = TempDir::new("bob-cli-capture-json-failure");
    let missing_vault = temp.path().join("missing-vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&missing_vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json capture failure");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing vault should be an IO failure:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "json failure should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("create target")),
        "unexpected json failure object: {json}"
    );
}

#[test]
fn dataview_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("dataview")
        .arg("--help")
        .output()
        .expect("run bob dataview --help");

    assert_success(&output);
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir ",
            "-e, --engine ",
            "-f, --format ",
            "-o, --origin ",
            "-q, --query ",
            "-Q, --query-file ",
            "-s, --source ",
            "-S, --strict-paths",
            "-v, --vault ",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn dataview_short_options_are_accepted() {
    let temp = TempDir::new("bob-cli-dataview-short-options");
    let vault = temp.path().join("vault");
    let query_file = temp.path().join("projects.dql");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");

    write_file(&vault.join("Home.md"), "---\n---\n");
    write_file(&vault.join("Projects/Alpha.md"), "# Alpha\n#project\n");
    write_file(&query_file, "LIST FROM #project");

    let output = bob_command()
        .arg("dataview")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-o")
        .arg("Home.md")
        .arg("-Q")
        .arg(&query_file)
        .output()
        .expect("run bob dataview with short query-file options");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["format"], "json");
    assert_eq!(json["paths"][0], "Projects/Alpha.md");

    let output = bob_command()
        .arg("dataview")
        .arg("-b")
        .arg(&vault)
        .arg("-S")
        .arg("-q")
        .arg("LIST FROM #project")
        .output()
        .expect("run bob dataview with short strict-paths/query options");

    assert_success(&output);
    assert_eq!(stdout(&output), "Projects/Alpha.md\n");

    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/Alpha.md"],"warnings":[]}"##,
    );
    let output = bob_command()
        .arg("dataview")
        .arg("-e")
        .arg("obsidian")
        .arg("-s")
        .arg("#project")
        .arg("-v")
        .arg("Bob")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob dataview with short obsidian options");

    assert_success(&output);
    assert_eq!(stdout(&output), "Projects/Alpha.md\n");
    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert!(log_text.contains("ARG:vault=Bob"), "{log_text}");
}

#[test]
fn dataview_rejects_invalid_argument_combinations() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["dataview", "--source", "#project", "--query", "LIST"],
            "cannot be used with",
        ),
        (
            &["dataview", "--source", "#project", "--format", "markdown"],
            "--format markdown requires a DQL query",
        ),
        (
            &[
                "dataview",
                "--vault",
                "Bob",
                "--query",
                "LIST FROM #project",
            ],
            "--vault can only be used with --engine obsidian",
        ),
        (
            &[
                "dataview",
                "--query",
                "LIST FROM #project",
                "--format",
                "json",
                "--strict-paths",
            ],
            "--strict-paths can only be used with --format paths",
        ),
    ];

    for (args, marker) in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_eq!(
            output.status.code(),
            Some(2),
            "invalid dataview args should fail with usage:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker),
            "expected `{marker}` in dataview validation error:\n{}",
            format_output(&output)
        );
        assert!(
            !stderr(&output).contains("engine execution is not implemented"),
            "validation must fail before execution:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_source_uses_path_command_and_sentinel_protocol() {
    let temp = TempDir::new("bob-cli-dataview-source");
    let stub_bin = temp.path().join("bin");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");

    write_obsidian_success_stub(
        &stub_bin.join("obsidian"),
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/alpha.md","Inbox/waiting.md"],"warnings":["cold start"]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--source")
        .arg("#project")
        .arg("--vault")
        .arg("Bob")
        .env_remove("BOB_DATAVIEW_OBSIDIAN_COMMAND")
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob dataview source query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\nInbox/waiting.md\n",
        "source paths should be printed cleanly:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("warning: cold start"),
        "protocol warnings should go to stderr:\n{}",
        format_output(&output)
    );

    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert_text_order(&log_text, &["ARG:vault=Bob", "ARG:eval", "ARG:code="]);
    assert!(
        log_text.contains(r##""query":{"kind":"source","source":"#project"}"##)
            && log_text.contains("api.pagePaths")
            && log_text.contains("BOB_DATAVIEW_RESULT"),
        "expected generated source-query JavaScript in obsidian argv:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_dql_paths_extracts_and_deduplicates_note_paths() {
    let temp = TempDir::new("bob-cli-dataview-dql-paths");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"path"},"headers":["File","Status"],"values":[[{"type":"link","path":"Projects/alpha.md","display":null,"embed":false},"active"],[{"type":"link","path":"Projects/alpha.md","display":null,"embed":false},"duplicate"],[{"path":"Inbox\\waiting"},"waiting"]]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob dataview dql paths query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\nInbox/waiting.md\n",
        "DQL paths should be printed cleanly:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "unexpected dataview stderr:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_dql_paths_warn_or_fail_for_missing_identities() {
    let temp = TempDir::new("bob-cli-dataview-dql-strict-paths");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"table","idMeaning":{"type":"path"},"headers":["File","Status"],"values":[[],[{"path":"Projects/alpha.md"},"active"]]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run non-strict bob dataview dql paths query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\n",
        "non-strict DQL paths should print best-effort paths:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("warning: DQL table row 1"),
        "non-strict DQL paths should warn about missing identities:\n{}",
        format_output(&output)
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--query")
        .arg("TABLE status FROM #project")
        .arg("--strict-paths")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run strict bob dataview dql paths query");

    assert_eq!(
        output.status.code(),
        Some(1),
        "strict DQL paths should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty(),
        "strict DQL paths must keep stdout clean:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("paths output could not derive clean note paths")
            && err.contains("DQL table row 1")
            && err.contains("--format json"),
        "strict DQL paths should explain how to inspect raw results:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_dql_json_reads_query_file_and_forwards_env_vault() {
    let temp = TempDir::new("bob-cli-dataview-dql-json");
    let obsidian = temp.path().join("obsidian");
    let query_file = temp.path().join("projects.dql");
    let log = temp.path().join("commands.log");
    write_file(&query_file, "LIST FROM #project");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"dql_json","result":{"type":"list","values":[{"path":"Projects/alpha.md"}]},"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--origin")
        .arg("Home.md")
        .arg("--query-file")
        .arg(&query_file)
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env("BOB_DATAVIEW_VAULT", "Bob")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob dataview dql json query");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected dataview stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["engine"], "obsidian");
    assert_eq!(json["query_kind"], "dql");
    assert_eq!(json["format"], "json");
    assert_eq!(json["paths"][0], "Projects/alpha.md");
    assert_eq!(json["result"]["type"], "list");
    assert_eq!(json["result"]["values"][0]["path"], "Projects/alpha.md");

    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert_text_order(&log_text, &["ARG:vault=Bob", "ARG:eval", "ARG:code="]);
    assert!(
        log_text.contains(r##""origin":"Home.md""##)
            && log_text.contains(
                r##""query":{"kind":"dql","query":"LIST FROM #project"}"##
            )
            && log_text.contains(
                "api.tryQuery(request.query.query, origin, { forceId: true })"
            ),
        "expected generated DQL JavaScript in obsidian argv:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_markdown_prints_rendered_markdown() {
    let temp = TempDir::new("bob-cli-dataview-markdown");
    let obsidian = temp.path().join("obsidian");
    let log = temp.path().join("commands.log");
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"markdown","markdown":"| File |\n| --- |\n| Alpha |\n","warnings":[]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("markdown")
        .arg("--query")
        .arg("TABLE file.name FROM #project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("STUB_LOG", &log)
        .output()
        .expect("run bob dataview markdown query");

    assert_success(&output);
    assert_eq!(stdout(&output), "| File |\n| --- |\n| Alpha |\n");
    assert!(
        stderr(&output).is_empty(),
        "unexpected dataview stderr:\n{}",
        format_output(&output)
    );
    let log_text = fs::read_to_string(&log).expect("read obsidian argv log");
    assert!(
        !log_text.contains("ARG:vault=")
            && log_text.contains("api.tryQueryMarkdown"),
        "markdown query should not forward an unset vault:\n{log_text}"
    );
}

#[test]
fn dataview_obsidian_reports_protocol_errors() {
    let cases = [
        (
            "missing-dataview",
            r##"{"status":"error","code":"DATAVIEW_MISSING","message":"Dataview plugin not loaded"}"##,
            "Dataview is disabled, missing, or not ready",
            "Dataview plugin not loaded",
        ),
        (
            "query-error",
            r##"{"status":"error","code":"DATAVIEW_QUERY_ERROR","message":"Expected one of FROM, WHERE"}"##,
            "Dataview query failed",
            "Expected one of FROM, WHERE",
        ),
    ];

    for (name, payload, marker, detail) in cases {
        let temp = TempDir::new(&format!("bob-cli-dataview-{name}"));
        let obsidian = temp.path().join("obsidian");
        let log = temp.path().join("commands.log");
        write_obsidian_success_stub(&obsidian, payload);

        let output = bob_command()
            .arg("dataview")
            .arg("--engine")
            .arg("obsidian")
            .arg("--format")
            .arg("json")
            .arg("--query")
            .arg("LIST FROM #project")
            .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
            .env_remove("BOB_DATAVIEW_VAULT")
            .env("STUB_LOG", &log)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob dataview protocol error {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "protocol error should fail:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "protocol errors must keep stdout clean:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected protocol error report for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_reports_missing_and_malformed_sentinel() {
    let cases = [
        (
            "missing-sentinel",
            "#!/bin/sh\nprintf 'plugin log only\\n'\n",
            "missing Obsidian protocol response",
            "plugin log only",
        ),
        (
            "malformed-sentinel",
            "#!/bin/sh\nprintf 'BOB_DATAVIEW_RESULT\\t{not-json}\\n'\n",
            "malformed Obsidian protocol response",
            "invalid sentinel JSON",
        ),
    ];

    for (name, script, marker, detail) in cases {
        let temp = TempDir::new(&format!("bob-cli-dataview-{name}"));
        let obsidian = temp.path().join("obsidian");
        write_executable(&obsidian, script);

        let output = bob_command()
            .arg("dataview")
            .arg("--engine")
            .arg("obsidian")
            .arg("--format")
            .arg("json")
            .arg("--query")
            .arg("LIST FROM #project")
            .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
            .env_remove("BOB_DATAVIEW_VAULT")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob dataview sentinel case {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "sentinel protocol failure should exit 1:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "sentinel protocol failures must keep stdout clean:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected sentinel protocol error for {name}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_obsidian_reports_missing_command_without_query_blob() {
    let temp = TempDir::new("bob-cli-dataview-missing-command");
    let missing = temp.path().join("missing-obsidian");

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("LIST FROM #project WHERE status = \"active\"")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &missing)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .expect("run bob dataview with missing obsidian");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing obsidian command should fail:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        stdout(&output).is_empty()
            && err.contains("Obsidian command not found")
            && err.contains("BOB_DATAVIEW_OBSIDIAN_COMMAND")
            && !err.contains("LIST FROM #project")
            && !err.contains("code="),
        "missing-command error should be actionable without query/code leak:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_reports_not_running_without_javascript_blob() {
    let temp = TempDir::new("bob-cli-dataview-not-running");
    let obsidian = temp.path().join("obsidian");
    write_executable(
        &obsidian,
        "#!/bin/sh\nprintf 'The CLI is unable to find Obsidian. Please make sure Obsidian is running and try again. %s\\n' \"$*\" >&2\nexit 1\n",
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--engine")
        .arg("obsidian")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("LIST FROM #project WHERE status = \"active\"")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .output()
        .expect("run bob dataview when Obsidian is not running");

    assert_eq!(
        output.status.code(),
        Some(1),
        "not-running obsidian should fail:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        stdout(&output).is_empty()
            && err.contains("Obsidian is not running")
            && err.contains("<generated JavaScript>")
            && !err.contains("LIST FROM #project")
            && !err.contains("forceId"),
        "not-running error should redact generated JavaScript:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_obsidian_query_does_not_run_ob_command() {
    let temp = TempDir::new("bob-cli-dataview-no-sync");
    let vault = temp.path().join("vault");
    let ob = temp.path().join("ob");
    let obsidian = temp.path().join("obsidian");
    let ob_log = temp.path().join("ob.log");
    let obsidian_log = temp.path().join("obsidian.log");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &ob,
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$OB_LOG"
printf 'ob should not run\n' >&2
exit 99
"#,
    );
    write_obsidian_success_stub(
        &obsidian,
        r##"{"status":"ok","kind":"source_paths","paths":["Projects/alpha.md"],"warnings":[]}"##,
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("obsidian")
        .arg("--source")
        .arg("#project")
        .env("BOB_DATAVIEW_OBSIDIAN_COMMAND", &obsidian)
        .env_remove("BOB_DATAVIEW_VAULT")
        .env("OB_COMMAND", &ob)
        .env("OB_LOG", &ob_log)
        .env("STUB_LOG", &obsidian_log)
        .output()
        .expect("run bob dataview query with failing ob stub");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "Projects/alpha.md\n",
        "paths output should stay clean without sync logs:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "dataview should not surface ob output:\n{}",
        format_output(&output)
    );
    assert!(
        !ob_log.exists(),
        "bob dataview must not run OB_COMMAND:\n{}",
        fs::read_to_string(&ob_log).unwrap_or_default()
    );
}

#[test]
fn dataview_rejects_removed_sync_option() {
    let temp = TempDir::new("bob-cli-dataview-sync-rejected");
    let ob = temp.path().join("ob");
    let ob_log = temp.path().join("ob.log");
    write_executable(
        &ob,
        "#!/bin/sh\nprintf 'ob %s\\n' \"$*\" >> \"$OB_LOG\"\nexit 99\n",
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--sync")
        .arg("--source")
        .arg("#project")
        .env("OB_COMMAND", &ob)
        .env("OB_LOG", &ob_log)
        .output()
        .expect("run bob dataview with removed sync flag");

    assert_eq!(
        output.status.code(),
        Some(2),
        "removed --sync flag should be a usage failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty(),
        "usage failure must keep stdout clean:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("unexpected argument") && err.contains("--sync"),
        "removed --sync flag should fail visibly:\n{}",
        format_output(&output)
    );
    assert!(
        !ob_log.exists(),
        "usage rejection must not run OB_COMMAND:\n{}",
        fs::read_to_string(&ob_log).unwrap_or_default()
    );
}

#[test]
fn dataview_rejects_unsafe_origin_and_missing_bob_dir() {
    let temp = TempDir::new("bob-cli-dataview-path-validation");
    let vault = temp.path().join("vault");
    let missing_vault = temp.path().join("missing-vault");
    fs::create_dir_all(&vault).expect("create vault");
    let cases = [
        (
            vec![
                OsString::from("dataview"),
                OsString::from("--bob-dir"),
                vault.clone().into_os_string(),
                OsString::from("--origin"),
                OsString::from("../Secret.md"),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "invalid --origin",
            ".. traversal",
        ),
        (
            vec![
                OsString::from("dataview"),
                OsString::from("--bob-dir"),
                vault.into_os_string(),
                OsString::from("--origin"),
                temp.path().join("absolute.md").into_os_string(),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "invalid --origin",
            "absolute paths are not allowed",
        ),
        (
            vec![
                OsString::from("dataview"),
                OsString::from("--bob-dir"),
                missing_vault.into_os_string(),
                OsString::from("--query"),
                OsString::from("LIST FROM #project"),
            ],
            "--bob-dir must name an existing Bob vault directory",
            "missing-vault",
        ),
    ];

    for (args, marker, detail) in cases {
        let output = bob_command()
            .args(args)
            .output()
            .expect("run invalid dataview path case");

        assert_eq!(
            output.status.code(),
            Some(2),
            "path validation errors should be usage failures:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty()
                && stderr(&output).contains(marker)
                && stderr(&output).contains(detail),
            "expected path validation error:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dataview_native_dql_paths_walks_parent_frontmatter_headlessly() {
    let temp = TempDir::new("bob-cli-dataview-native-parents");
    let vault = temp.path().join("vault");
    write_native_parent_chain_fixture(&vault);
    let query = r#"
LIST
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
"#;

    let output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--strict-paths")
        .arg("--query")
        .arg(query)
        .output()
        .expect("run native dataview parent query");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "ref/papers/direct_ai.md\nref/papers/memory_os.md\n",
        "native parent query should only include source PDFs under ai_ref:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "native engine should keep stderr clean for supported queries:\n{}",
        format_output(&output)
    );
}

#[test]
fn dataview_native_table_paths_match_list_rows_headlessly() {
    let temp = TempDir::new("bob-cli-dataview-native-table-paths");
    let vault = temp.path().join("vault");
    write_native_parent_chain_fixture(&vault);
    let query_tail = r#"
FROM "ref"
WHERE source_pdf
  AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
"#;
    let list_query = format!("LIST\n{query_tail}");
    let table_query =
        format!("TABLE status, parent, source_path\n{query_tail}");

    let list_output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--strict-paths")
        .arg("--query")
        .arg(&list_query)
        .output()
        .expect("run native dataview list parent query");
    let table_output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--strict-paths")
        .arg("--query")
        .arg(&table_query)
        .output()
        .expect("run native dataview table parent query");

    assert_success(&list_output);
    assert_success(&table_output);
    assert_eq!(
        stdout(&table_output),
        stdout(&list_output),
        "native TABLE paths should match equivalent LIST rows:\nLIST:\n{}\nTABLE:\n{}",
        format_output(&list_output),
        format_output(&table_output)
    );
    assert_eq!(
        stdout(&table_output),
        "ref/papers/direct_ai.md\nref/papers/memory_os.md\n"
    );
    assert!(
        stderr(&list_output).is_empty() && stderr(&table_output).is_empty(),
        "native table/list paths should keep stderr clean:\nLIST:\n{}\nTABLE:\n{}",
        format_output(&list_output),
        format_output(&table_output)
    );
}

#[test]
fn dataview_native_table_json_projects_frontmatter_rows() {
    let temp = TempDir::new("bob-cli-dataview-native-table-json");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/alpha.md"),
        "---\nstatus: active\nparent: \"[[memory_ref]]\"\nready: true\n---\n",
    );
    write_file(
        &vault.join("ref/beta.md"),
        "---\nparent: null\nready: false\n---\n",
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--format")
        .arg("json")
        .arg("--query")
        .arg("TABLE status, parent, ready, missing FROM \"ref\"")
        .output()
        .expect("run native dataview table JSON query");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "native JSON TABLE query should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["engine"], "native");
    assert_eq!(json["query_kind"], "dql");
    assert_eq!(json["format"], "json");
    assert_eq!(
        json["paths"],
        serde_json::json!(["ref/alpha.md", "ref/beta.md"])
    );
    assert_eq!(json["result"]["type"], "table");
    assert_eq!(
        json["result"]["idMeaning"],
        serde_json::json!({"type": "path"})
    );
    assert_eq!(
        json["result"]["headers"],
        serde_json::json!(["status", "parent", "ready", "missing"])
    );
    let values = json["result"]["values"]
        .as_array()
        .expect("table values array");
    assert_eq!(values.len(), 2, "expected two table rows:\n{json}");
    assert_eq!(
        values[0][0],
        serde_json::json!({
            "type": "link",
            "path": "ref/alpha.md",
            "display": null,
            "embed": false
        })
    );
    assert_eq!(values[0][1], "active");
    assert_eq!(
        values[0][2],
        serde_json::json!({
            "type": "link",
            "path": "memory_ref.md",
            "display": null,
            "embed": false
        })
    );
    assert_eq!(values[0][3], true);
    assert!(values[0][4].is_null(), "missing field should be null");
    assert_eq!(
        values[1][0],
        serde_json::json!({
            "type": "link",
            "path": "ref/beta.md",
            "display": null,
            "embed": false
        })
    );
    assert!(values[1][1].is_null(), "missing field should be null");
    assert!(values[1][2].is_null(), "frontmatter null should be null");
    assert_eq!(values[1][3], false);
}

#[test]
fn dataview_native_where_false_returns_no_rows() {
    let temp = TempDir::new("bob-cli-dataview-native-false");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/papers/memory_os.md"),
        "---\nsource_pdf: lib/papers/memory-os.pdf\nparent: \"[[ai_ref]]\"\n---\n",
    );

    let output = bob_command()
        .arg("dataview")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--engine")
        .arg("native")
        .arg("--query")
        .arg("LIST FROM \"ref\" WHERE false")
        .output()
        .expect("run native dataview false query");

    assert_success(&output);
    assert!(
        stdout(&output).is_empty() && stderr(&output).is_empty(),
        "WHERE false should not return fallback rows:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_help_lists_subcommands_and_options() {
    let output = bob_command()
        .arg("projects")
        .arg("--help")
        .output()
        .expect("run bob projects --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Manage Bob project notes")
            && help.contains("\n  list ")
            && help.contains("\n  sync "),
        "expected projects help to list subcommands:\n{help}"
    );
    assert_text_order(&help, &["\n  list ", "\n  sync "]);
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("--help")
        .output()
        .expect("run bob projects list --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-b, --bob-dir"),
        "expected bob-dir short and long option in list help:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob projects sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-b, --bob-dir") && help.contains("-d, --dry-run"),
        "expected sync short and long options:\n{help}"
    );
    assert_text_order(&help, &["-b, --bob-dir", "-d, --dry-run"]);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn projects_list_scans_project_notes_and_renders_counts() {
    let temp = TempDir::new("bob-cli-projects-list");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Alpha.md"),
        r#"---
type: "[[project]]"
status: wip
---
- [ ] #task Finish Alpha #hide ^prj
- [ ] #task shown one
- [/] #task shown in progress
- [B] #task shown blocked
- [ ] #task hidden helper #hide
- [x] #task done task
- [-] #task canceled task
"#,
    );
    write_file(
        &vault.join("Beta.md"),
        r#"---
type: [[project]]
status: waiting
---
- [ ] #task Finish Beta [scheduled::2026-06-11] ^prj
- [ ] #task planned #hide
"#,
    );
    write_file(
        &vault.join("Done.md"),
        r#"---
type: [[project]]
status: done
---
- [X] #task Finish Done #hide ^prj
"#,
    );
    write_file(
        &vault.join("Canceled.md"),
        r#"---
type: [[project]]
status: canceled
---
- [-] #task Cancel Canceled #hide ^prj
"#,
    );
    write_file(
        &vault.join("Missing.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task Needs prj
"#,
    );
    write_file(
        &vault.join("Placeholder.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task <short_project_completion_criteria_goes_here> #hide ^prj
"#,
    );
    write_file(
        &vault.join("_templates/Template.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join(".obsidian/Hidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join("done/Archived.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects list");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected stderr:\n{}",
        stderr(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Projects - 3 active - 1 waiting - 1 done - 1 canceled"),
        "unexpected summary:\n{out}"
    );
    assert!(
        out.contains("PROJECT")
            && out.contains("STATUS")
            && out.contains("SHOWN")
            && out.contains("^PRJ"),
        "missing table header:\n{out}"
    );
    assert!(out.contains("Alpha") && out.contains("wip"));
    assert!(
        out.contains("   5      3  open"),
        "unexpected Alpha counts:\n{out}"
    );
    assert!(
        out.contains("Beta") && out.contains("on dash"),
        "missing on-dash Beta row:\n{out}"
    );
    assert!(
        !out.contains("scheduled 2026-06-11"),
        "scheduled field should not render in ^PRJ column:\n{out}"
    );
    assert!(out.contains("Done") && out.contains("done"));
    assert!(out.contains("Canceled") && out.contains("canceled"));
    assert!(out.contains("Missing") && out.contains("missing"));
    assert!(out.contains("Placeholder") && out.contains("placeholder"));
    assert!(
        !out.contains("Template")
            && !out.contains("Hidden")
            && !out.contains("Archived"),
        "excluded directories should not be listed:\n{out}"
    );
    assert_text_order(
        &out,
        &[
            "Alpha",
            "Missing",
            "Placeholder",
            "Beta",
            "Done",
            "Canceled",
        ],
    );
}

#[test]
fn projects_list_reports_prj_errors_without_aborting_scan() {
    let temp = TempDir::new("bob-cli-projects-errors");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Good project #hide ^prj\n",
    );
    write_file(
        &vault.join("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete malformed project ^prj\n",
    );
    write_file(
        &vault.join("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects list with errors");

    assert_eq!(
        output.status.code(),
        Some(1),
        "project scan errors should exit 1:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("Good")
            && out.contains("Malformed")
            && out.contains("Multiple"),
        "list should still render every project row:\n{out}"
    );
    let err = stderr(&output);
    assert!(
        err.contains("Malformed.md:4: malformed ^prj task")
            && err.contains("Multiple.md:5: multiple ^prj tasks"),
        "expected per-file project errors:\n{err}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn projects_sync_updates_status_prj_hide_tag_warns_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-projects-sync");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("DoneFlip.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship done #hide ^prj\n",
    );
    write_file(
        &vault.join("CancelFlip.md"),
        "---\ntype: [[project]]\nstatus: waiting\n---\n- [-] #task Stop work #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingStatus.md"),
        "---\ntype: [[project]]\n---\n- [X] #task Ship missing status #hide ^prj\n",
    );
    write_file(
        &vault.join("Stalled.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish stalled #hide ^prj\n- [/] #task Secondary work #hide\n",
    );
    write_file(
        &vault.join("ZeroOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish zero open #hide ^prj\n- [x] #task Already done\n",
    );
    write_file(
        &vault.join("HasUnprioritized.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish has unprioritized #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("MissingPriority.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish missing priority ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ExistingScheduled.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish already scheduled #hide [scheduled::2026-06-01] ^prj\n",
    );
    write_file(
        &vault.join("TerminalOpen.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Finish drift #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingPrj.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Needs completion task\n",
    );
    write_file(
        &vault.join("Placeholder.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task <short_project_completion_criteria_goes_here> #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let dry_run_snapshot =
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled");
    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-d")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected dry-run stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("[dry-run] ok")
            && out.contains("would set status: waiting -> canceled")
            && out.contains("would remove #hide from ^prj")
            && out.contains("would add #hide to ^prj")
            && out.contains("would remove [scheduled::2026-06-01] from ^prj")
            && out.contains("active project has no ^prj task")
            && out.contains("template placeholder")
            && out.contains(
                "11 projects - 3 status updated - 5 ^prj edited - 3 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled"),
        dry_run_snapshot,
        "dry-run must not edit files"
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected sync stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> done")
            && out.contains("removed #hide from ^prj")
            && out.contains("added #hide to ^prj")
            && out.contains("removed [scheduled::2026-06-01] from ^prj")
            && out.contains(
                "11 projects - 3 status updated - 5 ^prj edited - 3 warnings"
            ),
        "unexpected sync output:\n{out}"
    );

    assert_eq!(
        fs::read_to_string(vault.join("DoneFlip.md")).expect("read done"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Ship done #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("CancelFlip.md")).expect("read cancel"),
        "---\ntype: [[project]]\nstatus: canceled\n---\n- [-] #task Stop work #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("MissingStatus.md"))
            .expect("read missing status"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [X] #task Ship missing status #hide ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Stalled.md")).expect("read stalled"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish stalled ^prj\n- [/] #task Secondary work #hide\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ZeroOpen.md")).expect("read zero"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish zero open ^prj\n- [x] #task Already done\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("HasUnprioritized.md"))
            .expect("read has unprioritized"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish has unprioritized #hide ^prj\n- [ ] #task Needs priority\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("MissingPriority.md"))
            .expect("read missing priority"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish missing priority #hide ^prj\n- [ ] #task Needs priority\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ExistingScheduled.md"))
            .expect("read scheduled"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish already scheduled ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "11 projects - 0 status updated - 0 ^prj edited - 3 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_hides_parent_projects_with_open_subprojects() {
    let temp = TempDir::new("bob-cli-projects-sync-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentKeep.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent keep #hide ^prj\n",
    );
    write_file(
        &vault.join("ParentAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent add ^prj\n",
    );
    write_file(
        &vault.join("ChildKeep.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentKeep]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[projects/ParentAdd#Now|parent]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildAlpha.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentAdd]]\n---\n- [ ] #task Finish alpha #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let parent_add_snapshot =
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent");
    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would add #hide to ^prj  project has open sub-projects")
            && out
                .contains("would add [[ChildKeep]] to ^prj  open sub-project")
            && out.contains("would add [[ChildAdd]] to ^prj  open sub-project")
            && out
                .contains("would add [[ChildAlpha]] to ^prj  open sub-project")
            && out.contains(
                "5 projects - 0 status updated - 4 ^prj edited - 0 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent"),
        parent_add_snapshot,
        "dry-run must not edit parent"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added #hide to ^prj  project has open sub-projects")
            && out.contains("added [[ChildKeep]] to ^prj  open sub-project")
            && out.contains("added [[ChildAdd]] to ^prj  open sub-project")
            && out.contains("added [[ChildAlpha]] to ^prj  open sub-project")
            && out.contains(
                "5 projects - 0 status updated - 4 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentKeep.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent keep #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildKeep]]\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentAdd.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent add #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildAdd]] • [[ChildAlpha]]\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_unhides_parent_when_child_prj_is_checked_same_run() {
    let temp = TempDir::new("bob-cli-projects-sync-checked-subproject");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [x] #task Finish child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> done")
            && out.contains(
                "removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
            )
            && out.contains(
                "updated [[Child]] on ^prj  sub-project completed"
            )
            && out.contains(
                "2 projects - 1 status updated - 2 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[Child]]~~ ✅\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Child.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish child #hide ^prj\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "2 projects - 0 status updated - 0 ^prj edited - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_marks_canceled_subproject_same_run() {
    let temp = TempDir::new("bob-cli-projects-sync-canceled-subproject");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [-] #task Stop child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("status: wip -> canceled")
            && out.contains(
                "removed #hide from ^prj  no non-hidden open tasks or open sub-projects"
            )
            && out
                .contains("updated [[Child]] on ^prj  sub-project canceled")
            && out.contains(
                "2 projects - 1 status updated - 2 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[Child]]~~ ❌\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Child.md")).expect("read child"),
        "---\ntype: [[project]]\nstatus: canceled\nparent: [[Parent]]\n---\n- [-] #task Stop child #hide ^prj\n"
    );
}

#[test]
fn projects_sync_orders_open_then_closed_subprojects_in_one_run() {
    let temp = TempDir::new("bob-cli-projects-sync-mixed-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[DoneChild]] • [[CanceledChild]] • [[ExistingOpen]]\n",
    );
    write_file(
        &vault.join("ExistingOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish existing #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("AddedOpen.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish added #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("DoneChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [x] #task Finish done #hide ^prj\n",
    );
    write_file(
        &vault.join("CanceledChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [-] #task Stop canceled #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added [[AddedOpen]] to ^prj  open sub-project")
            && out.contains(
                "updated [[DoneChild]] on ^prj  sub-project completed"
            )
            && out.contains(
                "updated [[CanceledChild]] on ^prj  sub-project canceled"
            )
            && out.contains(
                "5 projects - 2 status updated - 3 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[AddedOpen]] • [[ExistingOpen]] • ~~[[CanceledChild]]~~ ❌ • ~~[[DoneChild]]~~ ✅\n"
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("rerun bob projects sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "5 projects - 0 status updated - 0 ^prj edited - 0 warnings"
        ),
        "second run should have zero actions:\n{}",
        format_output(&output)
    );
}

#[test]
fn projects_sync_keeps_pruned_closed_entries_gone() {
    let temp = TempDir::new("bob-cli-projects-sync-curated-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** [[DeletedChild]] • [[ReparentedChild]] • ~~[[KeptDone]]~~ ✅\n",
    );
    write_file(
        &vault.join("KeptDone.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish kept #hide ^prj\n",
    );
    write_file(
        &vault.join("PrunedDone.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[Parent]]\n---\n- [x] #task Finish pruned #hide ^prj\n",
    );
    write_file(
        &vault.join("ReparentedChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[OtherParent]]\n---\n- [ ] #task Finish reparented #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "removed [[DeletedChild]] from ^prj  no longer a sub-project"
        ) && out.contains(
            "removed [[ReparentedChild]] from ^prj  no longer a sub-project"
        ) && !out.contains("PrunedDone]]")
            && out.contains(
                "4 projects - 0 status updated - 2 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n\t- 🧩 **Sub-projects:** ~~[[KeptDone]]~~ ✅\n"
    );
}

#[test]
fn projects_sync_treats_children_without_open_prj_as_childless() {
    let temp = TempDir::new("bob-cli-projects-sync-no-open-subprojects");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentMissingChild.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n",
    );
    write_file(
        &vault.join("MissingPrjChild.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentMissingChild]]\n---\n- [ ] #task Needs completion task\n",
    );
    write_file(
        &vault.join("ParentCheckedChild.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n",
    );
    write_file(
        &vault.join("CheckedChild.md"),
        "---\ntype: [[project]]\nstatus: done\nparent: [[ParentCheckedChild]]\n---\n- [x] #task Finish child #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("active project has no ^prj task")
            && out.contains(
                "4 projects - 0 status updated - 2 ^prj edited - 1 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentMissingChild.md"))
            .expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join("ParentCheckedChild.md"))
            .expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent ^prj\n"
    );
}

#[test]
fn projects_sync_preserves_user_sub_bullets_and_inserts_subprojects_line() {
    let temp = TempDir::new("bob-cli-projects-sync-user-sub-bullets");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- Remember to check notes\n\t- [[non_project_note]]\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("added [[Child]] to ^prj  open sub-project")
            && out.contains(
                "2 projects - 0 status updated - 1 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Child]]\n\t- Remember to check notes\n\t- [[non_project_note]]\n"
    );
}

#[test]
fn projects_sync_normalizes_mangled_subprojects_line() {
    let temp = TempDir::new("bob-cli-projects-sync-subproject-line-update");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n  - 🧩 **Sub-projects:** [[ChildBeta]] plus [[ChildAlpha]]\n",
    );
    write_file(
        &vault.join("ChildAlpha.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish alpha #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ChildBeta.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish beta #hide ^prj\n- [ ] #task Needs priority\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("updated sub-projects on ^prj  canonical format")
            && out.contains(
                "3 projects - 0 status updated - 1 ^prj edited - 0 warnings"
            ),
        "unexpected sync output:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Parent.md")).expect("read parent"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildAlpha]] • [[ChildBeta]]\n"
    );
}

#[test]
fn projects_sync_subproject_line_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-projects-sync-subproject-line-dry-run");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("ParentAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish add #hide ^prj\n",
    );
    write_file(
        &vault.join("ChildAdd.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentAdd]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ParentRemove.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish remove #hide ^prj\n\t- 🧩 **Sub-projects:** [[OldChild]]\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("ParentUpdate.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish update #hide ^prj\n\t- 🧩 **Sub-projects:** [[ChildUpdate]] plus notes\n",
    );
    write_file(
        &vault.join("ChildUpdate.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[ParentUpdate]]\n---\n- [ ] #task Finish update child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    let snapshots = [
        (
            "ParentAdd.md",
            fs::read_to_string(vault.join("ParentAdd.md"))
                .expect("read parent add"),
        ),
        (
            "ParentRemove.md",
            fs::read_to_string(vault.join("ParentRemove.md"))
                .expect("read parent remove"),
        ),
        (
            "ParentUpdate.md",
            fs::read_to_string(vault.join("ParentUpdate.md"))
                .expect("read parent update"),
        ),
    ];

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects sync dry-run");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would add [[ChildAdd]] to ^prj  open sub-project")
            && out.contains(
                "would remove [[OldChild]] from ^prj  no longer a sub-project"
            )
            && out.contains(
                "would update sub-projects on ^prj  canonical format"
            )
            && out.contains(
                "5 projects - 0 status updated - 3 ^prj edited - 0 warnings"
            ),
        "unexpected dry-run output:\n{out}"
    );
    for (name, snapshot) in snapshots {
        assert_eq!(
            fs::read_to_string(vault.join(name)).expect("read parent"),
            snapshot,
            "dry-run must not edit {name}"
        );
    }
}

#[test]
fn projects_sync_reports_prj_errors_without_aborting_scan() {
    let temp = TempDir::new("bob-cli-projects-sync-errors");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Good project #hide ^prj\n",
    );
    write_file(
        &vault.join("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete malformed project ^prj\n",
    );
    write_file(
        &vault.join("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects sync with errors");

    assert_eq!(
        output.status.code(),
        Some(1),
        "project sync errors should exit 1:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("3 projects - 1 status updated - 0 ^prj edited - 0 warnings - 2 errors"),
        "unexpected sync summary:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("Malformed.md:4: malformed ^prj task")
            && err.contains("Multiple.md:5: multiple ^prj tasks"),
        "expected per-file project errors:\n{err}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("Good.md")).expect("read good"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [x] #task Good project #hide ^prj\n"
    );
}

#[test]
fn highlights_ref_help_lists_subcommands_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("--help")
        .output()
        .expect("run bob highlights --help");

    assert_success(&output);
    let help = stdout(&output);
    assert_text_order(
        &help,
        &["\n  doctor ", "\n  marker ", "\n  scan ", "\n  sync "],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_sync_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob highlights sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Arguments:") && help.contains("<PDF>"),
        "expected PDF positional argument in Arguments section:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-l, --lib-dir",
            "-p, --prefer",
            "-r, --ref-dir",
            "-w, --write-pdf",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_scan_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--help")
        .output()
        .expect("run bob highlights scan --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-w, --write-pdfs"),
        "expected short and long write-pdfs flag in scan help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-j, --jobs",
            "-l, --lib-dir",
            "-r, --ref-dir",
            "-v, --verbose",
            "-w, --write-pdfs",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_short_options_are_accepted() {
    let temp = TempDir::new("bob-cli-highlights-ref-short-options");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/example.pdf");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-j")
        .arg("1")
        .arg("-l")
        .arg("lib")
        .arg("-r")
        .arg("ref")
        .arg("-v")
        .arg("-w")
        .output()
        .expect("run bob highlights scan with short options");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("pdf_count: 1"), "{report}");
    assert!(report.contains("writes: none"), "{report}");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-l")
        .arg("lib")
        .arg("-p")
        .arg("marker")
        .arg("-r")
        .arg("ref")
        .arg("-w")
        .output()
        .expect("run bob highlights sync with short options");

    assert_success(&output);
    assert!(stdout(&output).contains("writes: none"));
}

#[test]
fn highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note() {
    let temp = TempDir::new("bob-cli-highlights-ref-create");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/systems-performance.pdf");
    let note = vault.join("ref/systems-performance.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n- topics: [linux, performance]\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated ref note");
    assert!(contents.contains("status: wip\n"), "{contents}");
    assert!(
        contents.contains("parent: \"[[obsidian]]\"\n"),
        "{contents}"
    );
    assert!(contents.contains("type: \"[[ref]]\"\n"), "{contents}");
    assert!(
        contents.contains("title: \"Systems Performance\"\n"),
        "{contents}"
    );
    assert!(
        contents.contains("topics: [linux, performance]\n"),
        "{contents}"
    );
    assert!(
        contents.contains("source_pdf: lib/systems-performance.pdf\n"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "- [ ] #task #ref [[lib/systems-performance.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    assert!(
        !contents.contains("ref_type:"),
        "top-level library PDFs should not derive ref_type:\n{contents}"
    );
    assert!(contents.contains("highlights_marker_hash: "), "{contents}");
    assert!(contents.contains("highlights_marker_base: "), "{contents}");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat bob highlights sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "bare marker parent should be idempotent:\n{}",
        format_output(&output)
    );

    let edited = fs::read_to_string(&note)
        .expect("read generated ref note")
        .replace("parent: \"[[obsidian]]\"\n", "parent: obsidian\n");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync bare frontmatter parent");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read normalized ref note");
    assert!(
        contents.contains("parent: \"[[obsidian]]\"\n"),
        "{contents}"
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_sync_dry_run_reads_literal_marker_newlines() {
    let temp = TempDir::new("bob-cli-highlights-ref-literal-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/obsidian-docs.pdf");
    let note = vault.join("ref/obsidian-docs.md");
    write_highlights_pdf(&pdf, "");
    set_pdf_marker_literal_contents(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Obsidian Docs\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run sync literal marker");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("sync_source: marker")
            && report.contains("note_action: create")
            && report.contains("writes: none"),
        "literal marker should parse as a complete marker list:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "dry-run must not create ref note");
}

#[test]
fn highlights_ref_marker_uses_first_page_text_annotation() {
    let temp = TempDir::new("bob-cli-highlights-ref-first-page-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/two-page.pdf");
    write_highlights_pdf_pages(
        &pdf,
        &[
            &["- status: wip\n- parent: obsidian\n- title: Page One\n"],
            &["- status: read\n- parent: ignored\n- title: Page Two\n"],
        ],
    );

    let output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("inspect first-page marker");

    assert_success(&output);
    let marker = stdout(&output);
    assert!(marker.contains("marker_page: 1"), "{marker}");
    assert!(marker.contains("marker_note: 1"), "{marker}");
    assert!(marker.contains("title: Page One"), "{marker}");
    assert!(
        !marker.contains("Page Two"),
        "later page note must not be selected:\n{marker}"
    );
}

#[test]
fn highlights_ref_scan_treats_later_page_note_as_missing_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-page-two-marker");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/page-two-marker.pdf");
    let note = vault.join("ref/books/page-two-marker.md");
    write_highlights_pdf_pages(
        &pdf,
        &[
            &[],
            &["- status: wip\n- parent: obsidian\n- title: Page Two\n"],
        ],
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "later page marker-like note should fail:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&pdf))
            && report.contains("plan_error:")
            && report.contains(
                "no standalone /Text note annotations found on page 1"
            ),
        "expected page-1 missing marker error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "scan must not write a note on marker error");
}

#[test]
fn highlights_ref_sync_rejects_missing_marker_status_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-status.pdf");
    let note = vault.join("ref/missing-status.md");
    write_highlights_pdf(&pdf, "- parent: obsidian\n- title: Missing Status\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing marker status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: status"),
        "expected missing status error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker error"
    );
}

#[test]
fn highlights_ref_sync_rejects_unsupported_marker_status_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-unsupported-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/unsupported-status.pdf");
    let note = vault.join("ref/unsupported-status.md");
    write_highlights_pdf(&pdf, "- status: queued\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "unsupported marker status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("marker has unsupported status \"queued\""),
        "expected unsupported status error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker status error"
    );
}

#[test]
fn highlights_ref_sync_rejects_missing_marker_parent_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-parent.pdf");
    let note = vault.join("ref/missing-parent.md");
    write_highlights_pdf(&pdf, "- status: wip\n- title: Missing Parent\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing marker parent should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: parent"),
        "expected missing parent error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "sync must not create a note on marker error"
    );
}

#[test]
fn highlights_ref_rejects_wikilink_marker_parent_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-linked-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/linked-parent.pdf");
    let note = vault.join("ref/linked-parent.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: [[obsidian]]\n- title: Linked Parent\n",
    );

    let sync = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights sync");
    assert_eq!(
        sync.status.code(),
        Some(1),
        "linked marker parent should fail sync:\n{}",
        format_output(&sync)
    );
    assert!(
        stderr(&sync).contains("wikilinks are not supported"),
        "expected linked parent error:\n{}",
        format_output(&sync)
    );
    assert!(!note.exists(), "sync must not write a note on marker error");

    let marker = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights marker");
    assert_eq!(
        marker.status.code(),
        Some(1),
        "linked marker parent should fail marker inspection:\n{}",
        format_output(&marker)
    );
    assert!(
        stderr(&marker).contains("wikilinks are not supported"),
        "expected linked parent error:\n{}",
        format_output(&marker)
    );

    let scan = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights scan");
    assert_eq!(
        scan.status.code(),
        Some(1),
        "linked marker parent should fail scan planning:\n{}",
        format_output(&scan)
    );
    let report = stdout(&scan);
    assert!(
        report.contains("plan_error:")
            && report.contains("wikilinks are not supported"),
        "expected scan linked parent error:\n{}",
        format_output(&scan)
    );
    assert!(
        stderr(&scan).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&scan)
    );
    assert!(!note.exists(), "scan must not write a note on marker error");
}

#[test]
fn highlights_ref_sync_rejects_malformed_and_duplicate_marker_lists() {
    let cases = [
        (
            "malformed-marker",
            "status: wip\n- parent: obsidian\n",
            "invalid marker item on line 1",
        ),
        (
            "duplicate-marker-key",
            "- status: wip\n- parent: obsidian\n- Status: read\n",
            "duplicate marker key on line 3",
        ),
        (
            "managed-type-marker-key",
            "- status: wip\n- parent: obsidian\n- type: [[book]]\n",
            "'type' is command-managed",
        ),
        (
            "managed-ref-type-marker-key",
            "- status: wip\n- parent: obsidian\n- ref_type: books\n",
            "'ref_type' is command-managed",
        ),
    ];

    for (name, marker, expected_error) in cases {
        let temp = TempDir::new(&format!("bob-cli-highlights-ref-{name}"));
        let vault = temp.path().join("vault");
        let pdf = vault.join(format!("lib/{name}.pdf"));
        let note = vault.join(format!("ref/{name}.md"));
        write_highlights_pdf(&pdf, marker);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob highlights sync for {name}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "invalid marker should fail for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(expected_error),
            "expected marker validation error for {name}:\n{}",
            format_output(&output)
        );
        assert!(
            !note.exists(),
            "sync must not create a note for invalid marker case {name}"
        );
    }
}

#[test]
fn highlights_ref_dry_run_and_inspection_do_not_modify_vault_files() {
    let temp = TempDir::new("bob-cli-highlights-ref-dry-run");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    let pdf_before = fs::read(&pdf).expect("read PDF before");
    let marker_before = pdf_marker_contents(&pdf);
    let cases = vec![
        vec![
            OsString::from("highlights"),
            OsString::from("scan"),
            OsString::from("--dry-run"),
        ],
        vec![
            OsString::from("highlights"),
            OsString::from("sync"),
            OsString::from(path_str(&pdf)),
            OsString::from("--dry-run"),
        ],
        vec![OsString::from("highlights"), OsString::from("doctor")],
        vec![
            OsString::from("highlights"),
            OsString::from("marker"),
            OsString::from(path_str(&pdf)),
        ],
    ];

    for args in cases {
        let output = bob_command()
            .args(&args)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_success(&output);
        assert!(
            stdout(&output).contains("writes: none"),
            "expected no-write report:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read(&pdf).expect("read PDF after"),
            pdf_before,
            "highlights inspection command modified the PDF"
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
        assert!(
            !note.exists(),
            "dry-run/inspection must not create ref note"
        );
    }
}

#[test]
fn highlights_ref_scan_recurses_dry_runs_and_writes_multiple_pdfs() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/systems-performance.pdf");
    let second_pdf = vault.join("lib/papers/rust-book.PDF");
    let first_note = vault.join("ref/books/systems-performance.md");
    let second_note = vault.join("ref/papers/rust-book.md");
    write_highlights_pdf(
        &first_pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: unread\n- parent: obsidian\n- title: Rust Book\n",
    );
    write_file(
        &first_pdf.with_extension("md"),
        "\
## Page 1

Note: marker note

---

> First quote.
",
    );
    write_file(
        &second_pdf.with_extension("md"),
        "\
## Page 2

Note: marker note

---

> Second quote.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run highlights scan");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(dry_run.contains("pdf_count: 2"), "{dry_run}");
    assert!(
        dry_run.contains(path_str(&first_note))
            && dry_run.contains(path_str(&second_note)),
        "{dry_run}"
    );
    assert!(dry_run.contains("notes_create: 2"), "{dry_run}");
    assert!(dry_run.contains("writes: none"), "{dry_run}");
    assert!(!first_note.exists(), "dry-run must not create first note");
    assert!(!second_note.exists(), "dry-run must not create second note");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write highlights scan");

    assert_success(&output);
    let written = stdout(&output);
    assert!(written.contains("notes_created: 2"), "{written}");
    assert!(written.contains("writes: note"), "{written}");
    let first_contents =
        fs::read_to_string(&first_note).expect("read first note");
    let second_contents =
        fs::read_to_string(&second_note).expect("read second note");
    assert!(
        first_contents.contains("ref_type: books\n")
            && first_contents.contains("> [!quote] First quote.\n"),
        "{first_contents}"
    );
    assert!(
        first_contents.contains("## Highlights\n"),
        "{first_contents}"
    );
    assert!(!first_contents.contains("## Summary\n"), "{first_contents}");
    assert!(
        !first_contents.contains("## My Notes\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n")
            && second_contents.contains("> [!quote] Second quote.\n"),
        "{second_contents}"
    );
    assert!(
        second_contents.contains("## Highlights\n"),
        "{second_contents}"
    );
    assert!(
        !second_contents.contains("## Summary\n"),
        "{second_contents}"
    );
    assert!(
        !second_contents.contains("## My Notes\n"),
        "{second_contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat highlights scan");

    assert_success(&output);
    let repeated = stdout(&output);
    assert!(repeated.contains("notes_unchanged: 2"), "{repeated}");
    assert!(repeated.contains("writes: none"), "{repeated}");
}

#[test]
fn highlights_ref_scan_dry_run_reports_valid_and_invalid_pdfs() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-mixed-dry-run");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/papers/invalid.pdf");
    let valid_note = vault.join("ref/books/valid.md");
    let invalid_note = vault.join("ref/papers/invalid.md");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid PDF\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Missing Status\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("mixed dry-run highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "mixed dry-run scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(report.contains("pdf_count: 2"), "{report}");
    assert!(
        report.contains(path_str(&valid_note))
            && report.contains("notes_create: 1")
            && report.contains("pdfs_planned: 1"),
        "valid PDF should still be planned:\n{report}"
    );
    assert!(
        report.contains(path_str(&invalid_pdf))
            && report
                .contains("plan_error: missing required marker key: status")
            && report.contains("plan_failures: 1")
            && report.contains("scan_failures: 1")
            && report.contains("writes: none"),
        "invalid PDF should be reported without writes:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!valid_note.exists(), "dry-run must not create valid note");
    assert!(
        !invalid_note.exists(),
        "dry-run must not create invalid note"
    );
}

#[test]
fn highlights_ref_scan_writes_valid_pdfs_despite_invalid_pdf() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-mixed-write");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/papers/invalid.pdf");
    let valid_note = vault.join("ref/books/valid.md");
    let invalid_note = vault.join("ref/papers/invalid.md");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid PDF\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Missing Status\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("mixed write highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "mixed write scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&valid_note))
            && report.contains("notes_created: 1")
            && report.contains("write_successes: 1")
            && report.contains("writes: note"),
        "valid PDF should be written:\n{report}"
    );
    assert!(
        report.contains(path_str(&invalid_pdf))
            && report
                .contains("plan_error: missing required marker key: status")
            && report.contains("plan_failures: 1")
            && report.contains("scan_failures: 1"),
        "invalid PDF should be reported:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    let valid_contents =
        fs::read_to_string(&valid_note).expect("read valid note");
    assert!(
        valid_contents.contains("title: \"Valid PDF\"\n"),
        "{valid_contents}"
    );
    assert!(
        !invalid_note.exists(),
        "scan must not create a note for invalid PDF"
    );
}

#[test]
fn highlights_ref_scan_default_output_is_concise() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-concise");
    let vault = temp.path().join("vault");
    let create_pdf = vault.join("lib/books/create-note.pdf");
    let update_pdf = vault.join("lib/books/update-me.pdf");
    let settled_pdf = vault.join("lib/books/settled.pdf");
    let create_note = vault.join("ref/books/create-note.md");
    let update_note = vault.join("ref/books/update-me.md");
    let settled_note = vault.join("ref/books/settled.md");

    write_highlights_pdf(
        &create_pdf,
        "- status: wip\n- parent: obsidian\n- title: Create Note\n",
    );
    write_file(
        &create_pdf.with_extension("md"),
        "\
## Page 1

Note: marker note mirrored from the PDF

---

> Create highlight.
",
    );
    write_highlights_pdf(
        &update_pdf,
        "- status: wip\n- parent: obsidian\n- title: Update Me\n",
    );
    write_highlights_pdf(
        &settled_pdf,
        "- status: wip\n- parent: obsidian\n- title: Settled\n",
    );

    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&update_pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial update sync"),
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&settled_pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial settled sync"),
    );

    let checked_update = fs::read_to_string(&update_note)
        .expect("read update note")
        .replace("- [ ] #task", "- [x] #task");
    write_file(&update_note, &checked_update);
    write_file(
        &update_pdf.with_extension("md"),
        "\
## Page 2

Note: marker note mirrored from the PDF

---

> Update highlight.

- #task Import this annotation task.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise dry-run scan");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("Scanning 3 PDFs in lib - dry-run"),
        "{dry_run}"
    );
    assert!(
        dry_run.contains("[dry-run] ok  create note")
            && dry_run.contains("would create note")
            && dry_run.contains("1 highlight"),
        "created PDF should render as one concise line:\n{dry_run}"
    );
    assert!(
        dry_run.contains("[dry-run] ok  update me")
            && dry_run.contains("would update note + marker")
            && dry_run.contains("+1 task"),
        "updated PDF should render marker and task context:\n{dry_run}"
    );
    assert!(
        dry_run.contains(
            "3 pdfs - 1 created - 1 updated - 1 unchanged - 1 marker - 1 task - writes: none"
        ),
        "expected concise dry-run summary:\n{dry_run}"
    );
    assert!(
        !dry_run.contains("settled")
            && !dry_run.contains("pdf_count:")
            && !dry_run.contains("sync_source:"),
        "default scan output should suppress unchanged PDFs and verbose keys:\n{dry_run}"
    );
    assert!(!create_note.exists(), "dry-run must not create note");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise write scan");

    assert_success(&output);
    let written = stdout(&output);
    assert!(
        written.contains("Scanning 3 PDFs in lib")
            && !written.contains("dry-run"),
        "{written}"
    );
    assert!(
        written.contains("ok  create note")
            && written.contains("created note")
            && written.contains("ok  update me")
            && written.contains("updated note + marker"),
        "write scan should use past-tense concise actions:\n{written}"
    );
    assert!(
        written.contains(
            "3 pdfs - 1 created - 1 updated - 1 unchanged - 1 marker - 1 task - writes: note,pdf"
        ),
        "expected concise write summary:\n{written}"
    );
    assert!(
        !written.contains("settled") && !written.contains("notes_created:"),
        "write scan should keep detailed keys out of default output:\n{written}"
    );
    assert!(create_note.exists(), "write scan should create note");
    assert!(settled_note.exists(), "settled note should remain present");
}

#[test]
fn highlights_ref_scan_default_output_reports_inline_errors() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-concise-errors");
    let vault = temp.path().join("vault");
    let valid_pdf = vault.join("lib/books/valid.pdf");
    let invalid_pdf = vault.join("lib/books/invalid.pdf");
    write_highlights_pdf(
        &valid_pdf,
        "- status: wip\n- parent: obsidian\n- title: Valid\n",
    );
    write_highlights_pdf(
        &invalid_pdf,
        "- parent: obsidian\n- title: Invalid\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("concise scan with invalid PDF");

    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid PDF should make scan return non-zero:\n{}",
        format_output(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let report = stdout(&output);
    assert!(
        report.contains("error")
            && report.contains("invalid  missing required marker key: status"),
        "invalid PDF should be rendered inline:\n{report}"
    );
    assert!(
        report.contains(
            "2 pdfs - 1 created - 0 updated - 0 unchanged - 0 markers - 0 tasks - 1 failure - writes: none"
        ),
        "expected concise partial-failure summary:\n{report}"
    );
    assert!(
        !report.contains("plan_error:") && !report.contains(path_str(&invalid_pdf)),
        "default failure output should avoid verbose keys and full paths:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_scan_continues_after_write_failure() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-write-failure");
    let vault = temp.path().join("vault");
    let fail_pdf = vault.join("lib/books/a-fail.pdf");
    let later_pdf = vault.join("lib/books/b-later.pdf");
    let fail_note = vault.join("ref/books/a-fail.md");
    let later_note = vault.join("ref/books/b-later.md");
    write_highlights_pdf(
        &fail_pdf,
        "- status: wip\n- parent: obsidian\n- title: Fails At Write\n",
    );
    write_highlights_pdf(
        &later_pdf,
        "- status: wip\n- parent: obsidian\n- title: Later Still Writes\n",
    );

    let fail_parent = fail_note.parent().expect("fail note parent");
    let fail_name = fail_note.file_name().expect("fail note filename");
    let output = Command::new("sh")
        .arg("-c")
        .arg(
            "set -eu\n\
             mkdir -p \"$FAIL_PARENT\"\n\
             mkdir \"$FAIL_PARENT/.$FAIL_NAME.$$.tmp\"\n\
             exec \"$BOB_BIN\" highlights scan --verbose\n",
        )
        .env("BOB_BIN", BOB_BIN)
        .env("BOB_DIR", &vault)
        .env("FAIL_PARENT", fail_parent)
        .env("FAIL_NAME", fail_name)
        .output()
        .expect("write-failure highlights scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "write failure scan should return non-zero:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(path_str(&fail_pdf))
            && report.contains("write_failure:")
            && report.contains("write temporary file"),
        "failed PDF should be reported as a write failure:\n{report}"
    );
    assert!(
        report.contains(path_str(&later_note))
            && report.contains("write_successes: 1")
            && report.contains("write_failures: 1")
            && report.contains("scan_failures: 1")
            && report.contains("writes: note"),
        "later valid PDF should still be written:\n{report}"
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert!(!fail_note.exists(), "failed note must not be installed");
    let later_contents =
        fs::read_to_string(&later_note).expect("read later note");
    assert!(
        later_contents.contains("title: \"Later Still Writes\"\n"),
        "{later_contents}"
    );
}

#[test]
fn highlights_ref_scan_jobs_flag_matches_sequential_output() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-jobs");
    let vault = temp.path().join("vault");

    // Spread several PDFs across nested ref types so completion order under
    // parallel planning is unlikely to match the sorted reporting order.
    let specs = [
        ("lib/books/alpha.pdf", "wip", "Alpha", "Alpha quote."),
        ("lib/books/beta.pdf", "unread", "Beta", "Beta quote."),
        ("lib/papers/gamma.pdf", "wip", "Gamma", "Gamma quote."),
        ("lib/papers/delta.pdf", "unread", "Delta", "Delta quote."),
        ("lib/notes/epsilon.pdf", "wip", "Epsilon", "Epsilon quote."),
    ];
    for (rel, status, title, quote) in specs {
        let pdf = vault.join(rel);
        write_highlights_pdf(
            &pdf,
            &format!(
                "- status: {status}\n- parent: obsidian\n- title: {title}\n"
            ),
        );
        write_file(
            &pdf.with_extension("md"),
            &format!("## Page 1\n\nNote: marker note\n\n---\n\n> {quote}\n"),
        );
    }

    let run_scan = |jobs: &str| {
        let output = bob_command()
            .arg("highlights")
            .arg("scan")
            .arg("--verbose")
            .arg("--dry-run")
            .arg("--jobs")
            .arg(jobs)
            .env("BOB_DIR", &vault)
            .output()
            .expect("dry-run highlights scan with --jobs");
        assert_success(&output);
        stdout(&output)
    };

    // Dry-run scan output carries no timestamps, so order-preserving parallel
    // planning must produce byte-identical output regardless of job count.
    let sequential = run_scan("1");
    let parallel = run_scan("4");
    assert_eq!(sequential, parallel, "--jobs must not change scan output");
    assert!(sequential.contains("pdf_count: 5"), "{sequential}");
    assert!(sequential.contains("notes_create: 5"), "{sequential}");

    // Rejecting --jobs 0 keeps the flag meaningful (1 = sequential floor).
    let zero = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--jobs")
        .arg("0")
        .env("BOB_DIR", &vault)
        .output()
        .expect("reject --jobs 0");
    assert!(!zero.status.success(), "--jobs 0 must be rejected");
}

#[test]
fn highlights_ref_scan_allows_duplicate_basenames_in_different_ref_types() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-ref-types");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/example.pdf");
    let second_pdf = vault.join("lib/papers/example.pdf");
    let first_note = vault.join("ref/books/example.md");
    let second_note = vault.join("ref/papers/example.md");
    let old_flat_note = vault.join("ref/example.md");
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: obsidian\n");
    write_highlights_pdf(&second_pdf, "- status: unread\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run duplicate basename scan");

    assert_success(&output);
    let first_contents =
        fs::read_to_string(&first_note).expect("read books note");
    let second_contents =
        fs::read_to_string(&second_note).expect("read papers note");
    assert!(
        first_contents.contains("ref_type: books\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n"),
        "{second_contents}"
    );
    assert!(
        !old_flat_note.exists(),
        "nested references must not also write the old flat note"
    );
}

#[test]
fn highlights_ref_scan_detects_same_target_collision_before_writing() {
    let temp = TempDir::new("bob-cli-highlights-ref-scan-collision");
    let vault = temp.path().join("vault");
    let first_pdf = vault.join("lib/books/example.pdf");
    let second_pdf = vault.join("lib/books/example.PDF");
    let note = vault.join("ref/books/example.md");
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: obsidian\n");
    write_highlights_pdf(&second_pdf, "- status: unread\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run collision scan");

    assert_eq!(
        output.status.code(),
        Some(1),
        "same target collision should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("output path collision"),
        "expected collision report:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "scan must not write any note when collisions exist"
    );
}

#[test]
fn highlights_ref_sync_refuses_dirty_target_note_before_writing() {
    let temp = TempDir::new("bob-cli-highlights-ref-dirty-note");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial sync"]);
    let dirty_note = fs::read_to_string(&note)
        .expect("read note")
        .replace("## Highlights\n\n", "Local edit.\n\n## Highlights\n\n");
    write_file(&note, &dirty_note);
    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty target note");

    assert_eq!(
        output.status.code(),
        Some(1),
        "dirty target note should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("refusing to modify dirty vault files")
            && stderr(&output).contains("ref/example.md"),
        "expected dirty target report:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after refusal"),
        dirty_note
    );
}

#[test]
fn highlights_ref_sync_allows_dirty_tracked_frontmatter_writeback() {
    let temp = TempDir::new("bob-cli-highlights-ref-dirty-frontmatter");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial sync"]);
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: read");
    write_file(&note, &edited);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty frontmatter");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains("status: read\n"), "{contents}");
}

#[test]
fn highlights_ref_doctor_checks_vault_git_and_ob_without_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-doctor");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    let ob_stub = stub_bin.join("ob");
    write_executable(&ob_stub, "#!/bin/sh\nexit 0\n");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 1

Note: marker note
",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);

    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("OB_COMMAND", &ob_stub)
        .output()
        .expect("run highlights doctor");

    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("vault_path: ok"), "{report}");
    assert!(report.contains("library_dir: ok"), "{report}");
    assert!(report.contains("ref_dir: ok"), "{report}");
    assert!(report.contains("sidecars_found: 1"), "{report}");
    assert!(report.contains("pdf_markers_readable: 1"), "{report}");
    assert!(report.contains("git: ok (clean worktree)"), "{report}");
    assert!(report.contains("ob: available"), "{report}");
    assert!(report.contains("writes: none"), "{report}");
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn highlights_ref_marker_edit_updates_frontmatter() {
    let temp = TempDir::new("bob-cli-highlights-ref-marker-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync marker edit");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read updated ref note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{contents}"
    );
}

#[test]
fn highlights_ref_frontmatter_edit_updates_marker_when_pdf_writes_enabled() {
    let temp = TempDir::new("bob-cli-highlights-ref-frontmatter-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: The Log is the Agent\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: read");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run frontmatter edit");

    assert_success(&output);
    assert!(
        stdout(&output).contains("pdf_marker_action: would-update"),
        "expected dry-run PDF update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter edit without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "frontmatter-to-marker sync should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter edit");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian\n"), "{marker}");
    assert!(
        marker.contains("- title: The Log is the Agent\n"),
        "{marker}"
    );
    assert!(!marker.contains("- type:"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should change the source PDF hash"
    );

    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains(&format!(
            "source_pdf_sha256: {pdf_hash_after_write}\n"
        )),
        "reference note should record the post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write.contains(&format!(
            "source_pdf_sha256: {pdf_hash_before_write}\n"
        )),
        "reference note should not keep the pre-write PDF hash:\n{note_after_write}"
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync after PDF write-back");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "frontmatter PDF write-back should settle in one run:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read settled note"),
        note_after_write
    );
}

#[test]
fn highlights_ref_deprecated_done_status_migrates_to_read_with_pdf_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-done-migration");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    set_pdf_marker_contents(&pdf, "- status: done\n- parent: obsidian\n");
    let old_done_note = fs::read_to_string(&note)
        .expect("read generated ref note")
        .replace("status: wip", "status: done")
        .replace("\\\"status\\\":\\\"wip\\\"", "\\\"status\\\":\\\"done\\\"");
    assert!(
        old_done_note.contains("\\\"status\\\":\\\"done\\\""),
        "test must simulate old stored base status:\n{old_done_note}"
    );
    write_file(&note, &old_done_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run deprecated done migration");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("status_normalization: done->read")
            && dry_run.contains("marker,frontmatter,base")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("note_action: update")
            && dry_run.contains("writes: none"),
        "expected done->read migration dry-run report:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after dry-run"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("deprecated done sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "deprecated marker normalization should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after refusal"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan deprecated done migration");

    assert_success(&output);
    let scan_dry_run = stdout(&output);
    assert!(
        scan_dry_run.contains("status_normalization: done->read")
            && scan_dry_run.contains("pdf_marker_action: would-update")
            && scan_dry_run.contains("writes: none"),
        "expected scan dry-run migration report:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("writing scan deprecated done migration");

    assert_eq!(
        output.status.code(),
        Some(1),
        "writing scan should refuse deprecated marker normalization:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("plan_error:")
            && stdout(&output).contains("--write-pdf")
            && stdout(&output).contains("writes: none"),
        "expected scan planning refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note after scan refusal"),
        old_done_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write deprecated done migration");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(!marker.contains("- status: done\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(pdf_hash_before_write, pdf_hash_after_write);
    let migrated_note = fs::read_to_string(&note).expect("read migrated note");
    assert!(migrated_note.contains("status: read\n"), "{migrated_note}");
    assert!(
        migrated_note
            .contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{migrated_note}"
    );
    assert!(
        !migrated_note.contains("status: done")
            && !migrated_note.contains("\\\"status\\\":\\\"done\\\""),
        "{migrated_note}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat deprecated done migration sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "done migration should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_cancelled_dry_run_requires_and_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-cancelled");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    let generated_note = fs::read_to_string(&note).expect("read ref note");
    let cancelled_note = generated_note.replace(
        "- [ ] #task #ref [[lib/example.pdf]] #hide ^ref",
        "- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] ^ref",
    );
    write_file(&note, &cancelled_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run sync cancelled task");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("pdf_task: cancelled")
            && dry_run.contains("pdf_task_contribution: status=abandoned")
            && dry_run.contains("note_action: update")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("writes: none"),
        "expected cancelled-task dry-run update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan cancelled task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("notes_update: 1")
            && report.contains("pdf_marker_action: would-update")
            && report.contains("writes: none"),
        "expected cancelled task scan dry-run to preview marker work:\n{}",
        format_output(&output)
    );
    assert!(
        !report.contains("malformed") && !stderr(&output).contains("malformed"),
        "cancelled generated task should not be reported malformed:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after dry run"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "cancelled task should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        cancelled_note
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task write-pdf sync");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("pdf_marker_action: update")
            && report.contains("writes: note,pdf"),
        "expected cancelled-task write-pdf sync to write note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should refresh the source PDF hash"
    );
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: abandoned\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write.contains(
            "- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] ^ref\n"
        ),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_after_write}\n")),
        "note should record post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_before_write}\n")),
        "note should not retain pre-write PDF hash:\n{note_after_write}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat cancelled task sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "cancelled-task write-back should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_cancelled_scan_write_pdfs_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-cancelled-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    let cancelled_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [ ] #task", "- [-] #task");
    write_file(&note, &cancelled_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("cancelled task write-pdfs scan");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: true")
            && report.contains("pdf_task: cancelled")
            && report.contains("pdf_task_contribution: status=abandoned")
            && report.contains("pdf_markers_updated: 1")
            && report.contains("writes: note,pdf"),
        "expected scan --write-pdfs to write abandoned note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: abandoned\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains("- [-] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{note_after_write}"
    );
}

#[test]
fn highlights_ref_task_checked_dry_run_requires_and_writes_pdf_marker() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-read");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let checked_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [ ] #task", "- [x] #task");
    write_file(&note, &checked_note);
    let marker_before = pdf_marker_contents(&pdf);
    let pdf_hash_before_write = sha256_file(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task sync");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("pdf_task: checked")
            && dry_run.contains("pdf_task_contribution: status=read")
            && dry_run.contains("note_action: update")
            && dry_run.contains("pdf_marker_action: would-update")
            && dry_run.contains("writes: none"),
        "expected checked-task dry-run update preview:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task scan");

    assert_success(&output);
    let scan = stdout(&output);
    assert!(
        scan.contains("pdf_task_contribution: status=read")
            && scan.contains("write_pdfs: false")
            && scan.contains("pdf_marker_action: would-update")
            && scan.contains("notes_update: 1")
            && scan.contains("writes: none"),
        "expected scan dry-run to preview marker work:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run checked task scan with PDF writes enabled");

    assert_success(&output);
    let scan_write_dry_run = stdout(&output);
    assert!(
        scan_write_dry_run.contains("write_pdfs: true")
            && scan_write_dry_run
                .contains("pdf_task_contribution: status=read")
            && scan_write_dry_run.contains("pdf_marker_action: would-update")
            && scan_write_dry_run.contains("notes_update: 1")
            && scan_write_dry_run.contains("writes: none"),
        "expected scan --dry-run --write-pdfs to stay read-only:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task sync without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "checked task should require --write-pdf:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("--write-pdf"),
        "expected --write-pdf refusal:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task scan without PDF writes");

    assert_eq!(
        output.status.code(),
        Some(1),
        "scan should refuse checked-task PDF marker writes:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: false")
            && report.contains("plan_error:")
            && report.contains("--write-pdf")
            && report.contains("plan_failures: 1")
            && report.contains("writes: none"),
        "expected scan planning refusal:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("scan completed with 1 per-PDF failure(s)"),
        "expected partial failure stderr:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("checked task write-pdfs scan");

    assert_success(&output);
    let scan_write = stdout(&output);
    assert!(
        scan_write.contains("write_pdfs: true")
            && scan_write.contains("pdf_markers_updated: 1")
            && scan_write.contains("writes: note,pdf"),
        "expected scan --write-pdfs to write note and PDF marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let pdf_hash_after_write = sha256_file(&pdf);
    assert_ne!(
        pdf_hash_before_write, pdf_hash_after_write,
        "PDF marker write should refresh the source PDF hash"
    );
    let note_after_write = fs::read_to_string(&note).expect("read note");
    assert!(
        note_after_write.contains("status: read\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{note_after_write}"
    );
    assert!(
        note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_after_write}\n")),
        "note should record post-write PDF hash:\n{note_after_write}"
    );
    assert!(
        !note_after_write
            .contains(&format!("source_pdf_sha256: {pdf_hash_before_write}\n")),
        "note should not retain pre-write PDF hash:\n{note_after_write}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat checked task sync");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "checked-task write-back should settle:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_task_checked_sync_creates_annotation_tasks_before_closing() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-closing-sync");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/closing-order.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/closing-order.md");
    let route_note = vault.join("alice.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Closing Order\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sync creates reference note"),
    );
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    write_file(
        &sidecar,
        "\
## Page 7

Note: marker note mirrored from the PDF

---

> Closing highlight.

- #task Final same-note intake.
- #task Ask Alice about closing order @alice
",
    );
    let checked_note =
        fs::read_to_string(&note).expect("read ref note").replace(
            "- [ ] #task #ref [[lib/books/closing-order.pdf]]",
            "- [x] #task #ref [[lib/books/closing-order.pdf]]",
        );
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync checked task with new annotation tasks");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pdf_task_contribution: status=read")
            && report.contains("annotation_tasks_create: 2")
            && report.contains("annotation_tasks_created: 2")
            && report.contains("routed_task_note_writes: 1"),
        "expected checked-task close to import pending annotation tasks:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read closed ref note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains(
            "- [x] #task #ref [[lib/books/closing-order.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    let same_note_task = find_created_annotation_task(
        &contents,
        "#task Final same-note intake.",
    );
    assert!(
        same_note_task.contains("[[#^h-")
            && same_note_task.contains("[h:: ")
            && same_note_task.contains("[created::"),
        "same-note task should link to its annotation block and carry properties: {same_note_task}"
    );
    let same_note_source_id = annotation_task_source_link_id(&same_note_task);
    assert!(
        highlight_block_ids(&contents).contains(&same_note_source_id),
        "same-note annotation block should exist:\n{contents}"
    );
    let route_contents =
        fs::read_to_string(&route_note).expect("read routed task note");
    let routed_task = find_created_annotation_task(
        &route_contents,
        "#task Ask Alice about closing order",
    );
    assert!(!routed_task.contains("@alice"), "{routed_task}");
    assert!(
        routed_task.contains("[[ref/books/closing-order#^h-")
            && routed_task.contains("[h:: ")
            && routed_task.contains("[created::"),
        "routed task should link to its annotation block and carry properties: {routed_task}"
    );
    let routed_source_id = annotation_task_source_link_id(&routed_task);
    assert!(
        highlight_block_ids(&contents).contains(&routed_source_id),
        "routed annotation block should exist in the ref note:\n{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat sync after closing status");

    assert_success(&output);
    assert!(
        stdout(&output).contains("annotation_tasks_created: 0")
            && stdout(&output).contains("writes: none"),
        "repeat read sync should not create additional annotation tasks:\n{}",
        format_output(&output)
    );
    let repeat_contents =
        fs::read_to_string(&note).expect("read repeat-synced ref note");
    let repeat_route_contents =
        fs::read_to_string(&route_note).expect("read repeat-routed note");
    assert_eq!(
        created_annotation_task_count(
            &repeat_contents,
            "#task Final same-note intake."
        ),
        1,
        "{repeat_contents}"
    );
    assert_eq!(
        created_annotation_task_count(
            &repeat_route_contents,
            "#task Ask Alice about closing order"
        ),
        1,
        "{repeat_route_contents}"
    );
}

#[test]
fn highlights_ref_task_checked_scan_creates_annotation_tasks_before_closing() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-closing-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/scan-closing.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/scan-closing.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Scan Closing\n",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sync creates scan reference note"),
    );
    write_file(
        &sidecar,
        "\
## Page 3

Note: marker note mirrored from the PDF

---

> Scan closing highlight.

- #task Import during scan close.
",
    );
    let checked_note = fs::read_to_string(&note)
        .expect("read scan ref note")
        .replace(
            "- [ ] #task #ref [[lib/books/scan-closing.pdf]]",
            "- [x] #task #ref [[lib/books/scan-closing.pdf]]",
        );
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--dry-run")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run scan checked task with pending annotation task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("write_pdfs: true")
            && report.contains("pdf_task_contribution: status=read")
            && report.contains("annotation_tasks_create: 1")
            && report.contains("pdf_markers_would_update: 1")
            && report.contains("writes: none"),
        "scan dry-run should report the final intake pass:\n{}",
        format_output(&output)
    );
    assert!(
        pdf_marker_contents(&pdf).contains("- status: wip\n"),
        "dry-run should leave the PDF marker wip"
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--verbose")
        .arg("--write-pdfs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write scan checked task with pending annotation task");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("annotation_tasks_created: 1")
            && report.contains("pdf_markers_updated: 1")
            && report.contains("writes: note,pdf"),
        "scan --write-pdfs should write annotation tasks and close marker:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read scan-closed note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains(
            "- [x] #task #ref [[lib/books/scan-closing.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    let created_task = find_created_annotation_task(
        &contents,
        "#task Import during scan close.",
    );
    let source_id = annotation_task_source_link_id(&created_task);
    assert!(
        created_task.contains("[[#^h-")
            && created_task.contains("[h:: ")
            && highlight_block_ids(&contents).contains(&source_id),
        "scan-created task should link to an annotation block and carry h property:\n{contents}"
    );
}

#[test]
fn highlights_ref_task_checked_dirty_tracked_note_is_allowed() {
    let temp = TempDir::new("bob-cli-highlights-ref-task-dirty");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial sync"]);
    let checked_note = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("- [ ] #task", "- [x] #task");
    write_file(&note, &checked_note);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync dirty checked task");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    let contents = fs::read_to_string(&note).expect("read synced note");
    assert!(contents.contains("status: read\n"), "{contents}");
    assert!(
        contents.contains("- [x] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
        "{contents}"
    );
}

#[test]
fn highlights_ref_task_checked_competing_status_edits_fail() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-task-conflict-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        let mut edited = fs::read_to_string(&note)
            .expect("read ref note")
            .replace("- [ ] #task", "- [x] #task");
        if source == "frontmatter" {
            edited = edited.replace("status: wip", "status: abandoned");
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: abandoned\n- parent: obsidian\n",
            );
        }
        write_file(&note, &edited);
        let note_before = fs::read_to_string(&note).expect("read note before");
        let marker_before = pdf_marker_contents(&pdf);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync checked task conflict for {source}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "checked task conflict should fail for {source}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("checked PDF task conflicts")
                && stderr(&output)
                    .contains(&format!("{source} status=\"abandoned\"")),
            "expected checked-task conflict report for {source}:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&note).expect("read note after conflict"),
            note_before
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
    }
}

#[test]
fn highlights_ref_task_cancelled_competing_status_edits_fail() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-task-cancelled-conflict-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        let mut edited = fs::read_to_string(&note)
            .expect("read ref note")
            .replace("- [ ] #task", "- [-] #task");
        if source == "frontmatter" {
            edited = edited.replace("status: wip", "status: read");
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: read\n- parent: obsidian\n",
            );
        }
        write_file(&note, &edited);
        let note_before = fs::read_to_string(&note).expect("read note before");
        let marker_before = pdf_marker_contents(&pdf);

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync cancelled task conflict for {source}: {error}")
            });

        assert_eq!(
            output.status.code(),
            Some(1),
            "cancelled task conflict should fail for {source}:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("cancelled PDF task conflicts")
                && stderr(&output)
                    .contains(&format!("{source} status=\"read\"")),
            "expected cancelled-task conflict report for {source}:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&note).expect("read note after conflict"),
            note_before
        );
        assert_eq!(pdf_marker_contents(&pdf), marker_before);
    }
}

#[test]
fn highlights_ref_status_abandoned_rewrites_generated_task_to_cancelled() {
    for source in ["marker", "frontmatter"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-abandoned-task-render-{source}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/example.pdf");
        let note = vault.join("ref/example.md");
        write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
        assert_success(
            &bob_command()
                .arg("highlights")
                .arg("sync")
                .arg(&pdf)
                .env("BOB_DIR", &vault)
                .output()
                .expect("initial highlights sync"),
        );

        if source == "frontmatter" {
            let edited = fs::read_to_string(&note)
                .expect("read ref note")
                .replace("status: wip", "status: abandoned");
            write_file(&note, &edited);
        } else {
            set_pdf_marker_contents(
                &pdf,
                "- status: abandoned\n- parent: obsidian\n",
            );
        }

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .arg("--write-pdf")
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("sync abandoned status render for {source}: {error}")
            });

        assert_success(&output);
        let marker = pdf_marker_contents(&pdf);
        assert!(marker.contains("- status: abandoned\n"), "{marker}");
        let contents = fs::read_to_string(&note).expect("read synced note");
        assert!(contents.contains("status: abandoned\n"), "{contents}");
        assert!(
            contents
                .contains("- [-] #task #ref [[lib/example.pdf]] #hide ^ref\n"),
            "{contents}"
        );
    }
}

#[test]
fn highlights_ref_non_overlapping_edits_auto_merge_and_settle() {
    let temp = TempDir::new("bob-cli-highlights-ref-auto-merge");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let edited = fs::read_to_string(&note).expect("read ref note").replace(
        "parent: \"[[obsidian]]\"\n",
        "parent: \"[[obsidian]]\"\ntitle: \"Frontmatter Title\"\n",
    );
    write_file(&note, &edited);
    let note_before = fs::read_to_string(&note).expect("read note before");
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("sync_source: auto-merge")
            && stdout(&output).contains("pdf_marker_action: would-update")
            && stdout(&output).contains("writes: none"),
        "expected dry-run auto-merge report:\n{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), note_before);
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("sync_source: auto-merge"),
        "expected auto-merge report:\n{}",
        format_output(&output)
    );
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: read\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian\n"), "{marker}");
    assert!(marker.contains("- title: Frontmatter Title\n"), "{marker}");
    let note_after = fs::read_to_string(&note).expect("read merged note");
    assert!(note_after.contains("status: read\n"), "{note_after}");
    assert!(
        note_after.contains("title: \"Frontmatter Title\"\n"),
        "{note_after}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync after auto-merge");

    assert_success(&output);
    assert!(
        stdout(&output).contains("note_action: none")
            && stdout(&output).contains("writes: none"),
        "auto-merge should settle in one write:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_frontmatter_missing_parent_fails_before_pdf_writeback() {
    let temp =
        TempDir::new("bob-cli-highlights-ref-frontmatter-missing-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("parent: \"[[obsidian]]\"\n", "");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter missing parent");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing frontmatter parent should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing required marker key: parent"),
        "expected missing parent error:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_frontmatter_unsupported_status_fails_before_pdf_writeback() {
    let temp = TempDir::new("bob-cli-highlights-ref-frontmatter-bad-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: complete");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter unsupported status");

    assert_eq!(
        output.status.code(),
        Some(1),
        "unsupported frontmatter status should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output)
            .contains("frontmatter has unsupported status \"complete\""),
        "expected unsupported frontmatter status error:\n{}",
        format_output(&output)
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);
}

#[test]
fn highlights_ref_conflicting_edits_fail_and_prefer_frontmatter_resolves() {
    let temp = TempDir::new("bob-cli-highlights-ref-conflict");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights sync"),
    );

    set_pdf_marker_contents(&pdf, "- status: read\n- parent: obsidian\n");
    let frontmatter_side = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: abandoned");
    write_file(&note, &frontmatter_side);
    let note_before = fs::read_to_string(&note).expect("read note before");
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync conflicting edits");

    assert_eq!(
        output.status.code(),
        Some(1),
        "conflicting edits should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("marker/frontmatter conflict"),
        "expected conflict report:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("status: marker=\"read\"")
            && stderr(&output).contains("frontmatter=\"abandoned\"")
            && stderr(&output).contains("base=\"wip\""),
        "expected field-level conflict report:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after conflict"),
        note_before
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .arg("--prefer")
        .arg("frontmatter")
        .env("BOB_DIR", &vault)
        .output()
        .expect("resolve conflict with frontmatter");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: abandoned\n"), "{marker}");
}

#[test]
fn highlights_ref_sync_renders_sidecar_highlights_and_notes() {
    let temp = TempDir::new("bob-cli-highlights-ref-sidecar-create");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/systems-performance.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/systems-performance.md");
    let old_flat_note = vault.join("ref/systems-performance.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Systems Performance\n",
    );
    write_file(
        &sidecar,
        "\
# Systems Performance

## Page 12

Note: marker note mirrored from the PDF

---

> Latency is not throughput.

Comment: Compare this with SLO notes.

---

Note: Keep a standalone observation after the marker.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync sidecar highlights");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(contents.contains("ref_type: books\n"), "{contents}");
    assert!(
        contents.contains("source_pdf: lib/books/systems-performance.pdf\n"),
        "{contents}"
    );
    assert!(
        contents
            .contains("highlights_sidecar: lib/books/systems-performance.md\n"),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 2\n"), "{contents}");
    assert!(contents.contains("highlights_synced_at: "), "{contents}");
    assert!(contents.contains("# Systems Performance\n"), "{contents}");
    assert!(
        contents.contains(
            "- [ ] #task #ref [[lib/books/systems-performance.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );
    assert!(
        contents.contains("## Highlights\n\n<!-- highlights:begin -->\n"),
        "{contents}"
    );
    assert!(!contents.contains("## Summary\n"), "{contents}");
    assert!(!contents.contains("## My Notes\n"), "{contents}");
    assert!(contents.contains("### Page 12\n"), "{contents}");
    assert!(
        contents.contains("> [!quote] Latency is not throughput.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > [!note] Comment Compare this with SLO notes.\n"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "> [!note] Keep a standalone observation after the marker.\n"
        ),
        "{contents}"
    );
    assert!(
        !contents.contains("marker note mirrored"),
        "first standalone sidecar note should be excluded:\n{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 2, "{contents}");
    assert!(
        !old_flat_note.exists(),
        "nested sync must not create the old flat reference note"
    );

    let stale_ref_type =
        contents.replace("ref_type: books\n", "ref_type: stale\n");
    write_file(&note, &stale_ref_type);
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync stale ref_type");

    assert_success(&output);
    let refreshed = fs::read_to_string(&note).expect("read refreshed note");
    assert!(refreshed.contains("ref_type: books\n"), "{refreshed}");
    assert!(!refreshed.contains("ref_type: stale\n"), "{refreshed}");
}

#[test]
fn highlights_ref_sync_renders_textbundle_image_selections() {
    let temp = TempDir::new("bob-cli-highlights-ref-image-textbundle");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/figures.pdf");
    let textbundle = pdf.with_extension("textbundle");
    let sidecar = textbundle.join("text.md");
    let source_asset = textbundle.join("assets/figure.png");
    let note = vault.join("ref/books/figures.md");
    let note_assets_dir = vault.join("ref/books/figures.assets");
    let image_bytes = b"synthetic png bytes";

    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Figures\n",
    );
    write_file(
        &sidecar,
        "\
# Figures

## Page 12

Note: marker note mirrored from the PDF

---

![Latency figure](assets/figure.png)

Comment: Compare this figure with p.14.
",
    );
    fs::create_dir_all(source_asset.parent().expect("asset parent"))
        .expect("create source asset parent");
    fs::write(&source_asset, image_bytes).expect("write source image asset");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg("--dry-run")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run image textbundle sync");

    assert_success(&output);
    let dry_run = stdout(&output);
    assert!(
        dry_run.contains("images: 1")
            && dry_run.contains("image_assets: 1")
            && dry_run.contains("writes: none"),
        "expected dry-run image report:\n{}",
        format_output(&output)
    );
    assert!(!note.exists(), "dry-run must not create note");
    assert!(
        !note_assets_dir.exists(),
        "dry-run must not create note assets dir"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync image textbundle");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("images: 1")
            && report.contains("image_assets_written: 1")
            && report.contains("writes: note"),
        "expected write image report:\n{}",
        format_output(&output)
    );
    let contents = fs::read_to_string(&note).expect("read image note");
    assert!(
        contents.contains(
            "highlights_sidecar: lib/books/figures.textbundle/text.md\n"
        ),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 1\n"), "{contents}");
    assert!(
        contents.contains("> [!quote] Image ![[ref/books/figures.assets/h-"),
        "{contents}"
    );
    assert!(
        contents
            .contains("> > [!note] Comment Compare this figure with p.14.\n"),
        "{contents}"
    );

    let assets = fs::read_dir(&note_assets_dir)
        .expect("read note assets dir")
        .map(|entry| entry.expect("asset entry").path())
        .collect::<Vec<_>>();
    assert_eq!(assets.len(), 1, "expected one copied image asset");
    let copied_asset = &assets[0];
    let file_name = copied_asset
        .file_name()
        .and_then(OsStr::to_str)
        .expect("asset file name");
    assert!(
        file_name.starts_with("h-") && file_name.ends_with(".png"),
        "unexpected asset filename: {file_name}"
    );
    assert_eq!(
        fs::read(copied_asset).expect("read copied image asset"),
        image_bytes
    );
    assert!(
        contents
            .contains(&format!("![[ref/books/figures.assets/{file_name}]]")),
        "{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat image textbundle sync");

    assert_success(&output);
    let repeated = stdout(&output);
    assert!(
        repeated.contains("image_assets_written: 0")
            && repeated.contains("image_assets_skipped: 1")
            && repeated.contains("writes: none"),
        "expected idempotent image report:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_ref_sync_supports_linked_sidecar_style() {
    let temp = TempDir::new("bob-cli-highlights-ref-linked-sidecar");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/highlights-ref-sync.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/highlights-ref-sync.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Highlights Reference Note Sync\n",
    );
    write_file(
        &sidecar,
        "\
# Highlights Reference Note Sync

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

***

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> It only writes the PDF marker when frontmatter is the selected
source and --write-pdf is supplied.

- Support sase tool call replay?

***

#### [Page 6](highlights://highlights-ref-sync#page=6)

##### 2026-06-03:

> Comment: Compare this with SLO notes.

Some note...

***
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync linked sidecar highlights");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(
        contents
            .contains("highlights_sidecar: lib/books/highlights-ref-sync.md\n"),
        "{contents}"
    );
    assert!(contents.contains("highlights_count: 2\n"), "{contents}");
    assert!(contents.contains("### Page 2\n"), "{contents}");
    assert!(contents.contains("### Page 6\n"), "{contents}");
    assert!(
        contents.contains(
            "> [!quote] It only writes the PDF marker when frontmatter is the selected source and --write-pdf is supplied.\n"
        ),
        "{contents}"
    );
    assert!(
        contents
            .contains("> > [!note] Comment Support sase tool call replay?\n"),
        "{contents}"
    );
    assert!(
        !contents.contains("[!note] Comment - Support sase tool call replay?"),
        "linked bullet comment marker should be stripped:\n{contents}"
    );
    assert!(
        contents.contains("> [!quote] Comment: Compare this with SLO notes.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > [!note] Comment Some note...\n"),
        "{contents}"
    );
    assert!(
        !contents.contains("> [!quote] Highlights Reference Note Sync\n"),
        "linked marker mirror title should not render as a highlight:\n{contents}"
    );
    assert!(
        !contents.contains("[!note] Comment - status: wip"),
        "linked marker mirror fields should not render as a comment:\n{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 2, "{contents}");
}

#[test]
fn highlights_ref_sync_beautifies_linked_sidecar_rendering() {
    let temp = TempDir::new("bob-cli-highlights-ref-beautify");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/beautify.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/beautify.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Beautify\n",
    );
    write_file(
        &sidecar,
        "\
# Beautify

#### [Page 2](highlights://beautify#page=2)

##### 2026-06-10:

> Confusing latency and through-
put leads to mis-sized capa-
city plans with \u{fb01}les.

- Compare this with SLO notes.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync beautified linked sidecar");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    assert!(
        contents.contains(
            "> [!quote] Confusing latency and throughput leads to mis-sized capacity plans with files.\n>\n> > [!note] Comment Compare this with SLO notes.\n"
        ),
        "{contents}"
    );
    assert_eq!(highlight_block_ids(&contents).len(), 1, "{contents}");
}

#[test]
fn highlights_ref_sync_creates_tasks_from_pdf_note_task_bullets() {
    let temp = TempDir::new("bob-cli-highlights-ref-note-tasks");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let created = Local::now().format("%Y-%m-%d").to_string();
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> Highlighted claim.

- #task Reconcile with chapter 3.
- Keep this bullet as a comment.

---

Note:
- #task Ask about the standalone note.
* #task Capture the second standalone task.
- Untagged standalone bullet.
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync sidecar task bullets");

    assert_success(&output);
    let mut contents = fs::read_to_string(&note).expect("read generated note");

    // The generated PDF reading-status task line is unchanged.
    assert!(
        contents.contains(
            "- [ ] #task #ref [[lib/books/task-notes.pdf]] #hide ^ref\n"
        ),
        "{contents}"
    );

    let find_created_task = |contents: &str, prose: &str| -> String {
        contents
            .lines()
            .find(|line| line.starts_with("- [ ]") && line.contains(prose))
            .unwrap_or_else(|| {
                panic!("missing created task for {prose}:\n{contents}")
            })
            .to_string()
    };
    let source_link_id = |line: &str| -> String {
        let start = line.find("[[").expect("source link present") + 2;
        let rest = &line[start..];
        let end = rest.find("]]").expect("source link terminator");
        let inside = &rest[..end];
        let target =
            inside.split_once('|').map_or(inside, |(target, _)| target);
        target
            .rsplit_once("#^")
            .unwrap_or_else(|| panic!("source link has no block id: {line}"))
            .1
            .to_string()
    };

    let reconcile_line =
        find_created_task(&contents, "#task Reconcile with chapter 3.");
    let ask_line =
        find_created_task(&contents, "#task Ask about the standalone note.");
    let capture_line = find_created_task(
        &contents,
        "#task Capture the second standalone task.",
    );

    // Each created task carries a same-note annotation backlink, the short
    // durable processed marker, and a created date.
    for line in [&reconcile_line, &ask_line, &capture_line] {
        assert!(
            line.contains("[[#^h-"),
            "missing annotation source link: {line}"
        );
        assert!(
            !line.contains("[highlight_task:: "),
            "legacy processed marker should not be rendered: {line}"
        );
        assert!(
            line.contains("[h:: "),
            "short processed marker should be rendered: {line}"
        );
        assert!(
            line.contains(&format!("[created::{created}]")),
            "missing created date: {line}"
        );
    }

    // The link resolves to annotation-level h-... blocks. The two standalone
    // note tasks share the note block; the comment task points at the highlight
    // block.
    let block_ids = highlight_block_ids(&contents);
    let reconcile_id = source_link_id(&reconcile_line);
    let ask_id = source_link_id(&ask_line);
    let capture_id = source_link_id(&capture_line);
    assert_eq!(block_ids.len(), 2, "{contents}");
    assert!(block_ids.contains(&reconcile_id), "{contents}");
    assert!(block_ids.contains(&ask_id), "{contents}");
    assert!(block_ids.contains(&capture_id), "{contents}");
    assert_eq!(
        ask_id, capture_id,
        "standalone tasks share annotation block"
    );
    assert_ne!(reconcile_id, ask_id, "comment and note tasks differ");

    assert!(
        contents
            .contains("> > [!note] Comment #task Reconcile with chapter 3.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> > Keep this bullet as a comment.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> [!note] - #task Ask about the standalone note.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> * #task Capture the second standalone task.\n"),
        "{contents}"
    );
    assert!(
        !contents.contains(" ^ht-"),
        "managed highlight blocks should not render task-specific anchors:\n{contents}"
    );
    assert!(
        contents.contains("> - Untagged standalone bullet.\n"),
        "{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync sidecar task bullets");
    assert_success(&output);
    contents = fs::read_to_string(&note).expect("read resynced note");
    assert_eq!(
        contents
            .lines()
            .filter(|line| line.starts_with("- [ ] #task Reconcile"))
            .count(),
        1,
        "{contents}"
    );

    // Complete the comment task and cancel a standalone task, keeping their
    // links; a later sync preserves them verbatim and never duplicates them.
    let reconcile_line =
        find_created_task(&contents, "#task Reconcile with chapter 3.");
    let ask_line =
        find_created_task(&contents, "#task Ask about the standalone note.");
    let reconcile_completed = format!(
        "{} [completion::2026-06-08]",
        reconcile_line.replacen("- [ ]", "- [x]", 1)
    );
    let ask_cancelled = format!(
        "{} [cancelled::2026-06-08]",
        ask_line.replacen("- [ ]", "- [-]", 1)
    );
    let edited = contents
        .replace(&reconcile_line, &reconcile_completed)
        .replace(&ask_line, &ask_cancelled);
    write_file(&note, &edited);

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync completed sidecar task bullets");
    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains(&reconcile_completed), "{updated}");
    assert!(updated.contains(&ask_cancelled), "{updated}");
    assert_eq!(
        updated
            .lines()
            .filter(|line| line.starts_with("- [")
                && line.contains("#task Reconcile with chapter 3."))
            .count(),
        1,
        "{updated}"
    );
    assert_eq!(
        updated
            .lines()
            .filter(|line| line.starts_with("- [")
                && line.contains("#task Ask about the standalone note."))
            .count(),
        1,
        "{updated}"
    );
}

#[test]
fn highlights_ref_sync_skips_legacy_highlight_task_property() {
    let temp = TempDir::new("bob-cli-highlights-ref-legacy-task-property");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/legacy-task.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/legacy-task.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Legacy Task\n",
    );
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note mirrored from the PDF

---

> Highlighted claim.

- #task Legacy follow-up
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("initial sync sidecar legacy task");
    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read generated note");
    let source_block_id = highlight_block_ids(&contents)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing source block id:\n{contents}"));
    let legacy_id = legacy_highlight_task_id(
        "ref/books/legacy-task.md",
        &source_block_id,
        "#task Legacy follow-up",
    );
    let created_line = contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Legacy follow-up"))
        .unwrap_or_else(|| panic!("missing created task:\n{contents}"));
    let legacy_line = format!(
        "- [x] #task Edited legacy follow-up [[ref/books/legacy-task#^{source_block_id}|🔖]] [highlight_task:: {legacy_id}] [created::2026-06-01] [completion::2026-06-02]"
    );
    write_file(&note, &contents.replace(created_line, &legacy_line));

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("resync legacy task property");
    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains(&legacy_line), "{updated}");
    assert_eq!(
        updated
            .lines()
            .filter(|line| {
                line.starts_with("- [")
                    && line.contains("#task Legacy follow-up")
            })
            .count(),
        0,
        "{updated}"
    );
    assert_eq!(
        updated
            .lines()
            .filter(|line| {
                line.starts_with("- [")
                    && line.contains("#task Edited legacy follow-up")
            })
            .count(),
        1,
        "{updated}"
    );
}

#[test]
fn highlights_ref_sync_routes_annotation_tasks_to_existing_root_note() {
    let temp = TempDir::new("bob-cli-highlights-ref-routed-task");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let route_note = vault.join("alice.md");
    let done_note = vault.join("done/alice_done.md");
    let created = Local::now().format("%Y-%m-%d").to_string();
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> Routed claim.

- #task Follow up with Alice @alice
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync routed sidecar task");

    assert_success(&output);
    let ref_contents = fs::read_to_string(&note).expect("read ref note");
    assert!(
        !ref_contents
            .lines()
            .any(|line| line.starts_with("- [ ] #task Follow up with Alice")),
        "routed task should not be inserted into the reference note:\n{ref_contents}"
    );
    assert!(
        ref_contents
            .contains("> > [!note] Comment #task Follow up with Alice @alice\n")
            && !ref_contents.contains(" ^ht-"),
        "managed source should render without task-specific anchors:\n{ref_contents}"
    );
    let mut route_contents =
        fs::read_to_string(&route_note).expect("read routed note");
    let routed_line = route_contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Follow up with Alice"))
        .unwrap_or_else(|| panic!("missing routed task:\n{route_contents}"))
        .to_string();
    assert!(!routed_line.contains("@alice"), "{routed_line}");
    assert!(
        routed_line.contains("[[ref/books/task-notes#^h-")
            && routed_line.contains("|🔖]]"),
        "routed task should link back to the annotation ref note block:\n{routed_line}"
    );
    assert!(
        !routed_line.contains("[highlight_task:: ")
            && routed_line.contains("[h:: ")
            && routed_line.contains(&format!("[created::{created}]")),
        "routed task should carry h marker and created date:\n{routed_line}"
    );

    let completed_line = format!(
        "{} [completion::2026-06-08]",
        routed_line.replacen("- [ ]", "- [x]", 1)
    );
    write_file(
        &route_note,
        &route_contents.replace(&routed_line, &completed_line),
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("resync completed routed task"),
    );
    route_contents =
        fs::read_to_string(&route_note).expect("read rerouted note");
    assert_eq!(
        route_contents
            .lines()
            .filter(|line| line.contains("#task Follow up with Alice"))
            .count(),
        1,
        "{route_contents}"
    );

    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    let edited_archived_line = completed_line
        .replace("#task Follow up with Alice", "#task Followed up with Alice");
    write_file(&done_note, &format!("{edited_archived_line}\n"));
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("resync archived routed task"),
    );
    route_contents = fs::read_to_string(&route_note).expect("read route note");
    assert!(
        !route_contents.contains("#task Follow up with Alice"),
        "archived routed task with edited prose should not be recreated:\n{route_contents}"
    );
}

#[test]
fn highlights_ref_sync_skips_annotation_tasks_for_non_wip_statuses() {
    for status in ["unread", "read", "abandoned"] {
        let temp = TempDir::new(&format!(
            "bob-cli-highlights-ref-non-wip-task-{status}"
        ));
        let vault = temp.path().join("vault");
        let pdf = vault.join("lib/books/task-notes.pdf");
        let sidecar = pdf.with_extension("md");
        let note = vault.join("ref/books/task-notes.md");
        write_highlights_pdf(
            &pdf,
            &format!(
                "- status: {status}\n- parent: obsidian\n- title: Task Notes\n"
            ),
        );
        write_file(
            &sidecar,
            "\
## Page 4

Note: marker note mirrored from the PDF

---

> Claim.

- #task Should not be created.
",
        );

        let output = bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| panic!("sync {status}: {error}"));

        assert_success(&output);
        let contents = fs::read_to_string(&note)
            .unwrap_or_else(|error| panic!("read note for {status}: {error}"));
        assert!(
            !contents
                .lines()
                .any(|line| line
                    .starts_with("- [ ] #task Should not be created.")),
            "{status} PDFs should not create annotation tasks:\n{contents}"
        );
    }
}

#[test]
fn highlights_ref_sync_skips_vault_scan_when_no_annotation_candidates() {
    // Pins the "skip the vault-wide processed-task scan when no plan carries
    // annotation-task candidates" optimization. The sidecar has no `#task`
    // bullets, so there are zero candidates and the processed-task index is
    // never needed. We drop an unreadable (invalid UTF-8) `.md` file into the
    // vault: if a future refactor reintroduced the unconditional scan, the walk
    // would read this file and abort the command, failing this test.
    let temp = TempDir::new("bob-cli-highlights-ref-no-candidate-scan");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    let unreadable = vault.join("unreadable.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
# Task Notes

## Page 4

Note: marker note mirrored from the PDF

---

> A claim with no task bullet.
",
    );
    // Invalid UTF-8 bytes make `fs::read_to_string` fail if this file is ever
    // walked by the processed-task index builder.
    fs::write(&unreadable, [0xff, 0xfe, 0x00, 0x9f])
        .expect("write invalid utf-8 sibling note");

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync wip pdf without annotation tasks");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read ref note");
    assert!(
        !contents.lines().any(|line| line.contains("#task A claim")),
        "no annotation task should be created:\n{contents}"
    );
}

#[test]
fn highlights_ref_scan_groups_routed_tasks_with_parallel_jobs() {
    let temp = TempDir::new("bob-cli-highlights-ref-routed-scan");
    let vault = temp.path().join("vault");
    let route_note = vault.join("alice.md");
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    for (name, title, task) in [
        ("alpha", "Alpha", "Ask Alice about alpha."),
        ("beta", "Beta", "Ask Alice about beta."),
    ] {
        let pdf = vault.join(format!("lib/books/{name}.pdf"));
        write_highlights_pdf(
            &pdf,
            &format!("- status: wip\n- parent: obsidian\n- title: {title}\n"),
        );
        write_file(
            &pdf.with_extension("md"),
            &format!(
                "\
## Page 1

Note: marker note mirrored from the PDF

---

> {title} claim.

- #task {task} @alice
"
            ),
        );
    }

    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--jobs")
        .arg("2")
        .env("BOB_DIR", &vault)
        .output()
        .expect("parallel routed scan");

    assert_success(&output);
    let contents = fs::read_to_string(&route_note).expect("read route note");
    assert_eq!(
        contents.matches("#task Ask Alice about alpha.").count(),
        1,
        "{contents}"
    );
    assert_eq!(
        contents.matches("#task Ask Alice about beta.").count(),
        1,
        "{contents}"
    );
    assert_text_order(
        &contents,
        &[
            "#task Ask Alice about alpha.",
            "#task Ask Alice about beta.",
        ],
    );
}

#[test]
fn highlights_ref_sync_missing_routed_target_fails_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-route");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/books/task-notes.md");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note mirrored from the PDF

---

> Routed claim.

- #task Follow up with Alice @alice
",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync missing routed task target");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing routed target should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output)
            .contains("routed annotation task target does not exist")
            && stderr(&output).contains("alice.md")
            && stderr(&output).contains("create a root-level note"),
        "expected missing route target error:\n{}",
        format_output(&output)
    );
    assert!(
        !note.exists(),
        "failed routed planning should not write the reference note"
    );
}

#[test]
fn highlights_ref_comment_edit_keeps_stable_block_id() {
    let temp = TempDir::new("bob-cli-highlights-ref-comment-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note

---

> Stable quoted text.

Comment: first comment
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial sidecar sync"),
    );
    let initial = fs::read_to_string(&note).expect("read initial note");
    let initial_ids = highlight_block_ids(&initial);
    assert_eq!(initial_ids.len(), 1, "{initial}");

    write_file(
        &sidecar,
        "\
## Page 4

Note: marker note

---

> Stable quoted text.

Comment: revised comment
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync comment edit");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert_eq!(highlight_block_ids(&updated), initial_ids, "{updated}");
    assert!(
        updated.contains("[!note] Comment revised comment"),
        "{updated}"
    );
    assert!(
        !updated.contains("[!note] Comment first comment"),
        "{updated}"
    );
}

#[test]
fn highlights_ref_deleted_highlight_is_tombstoned() {
    let temp = TempDir::new("bob-cli-highlights-ref-tombstone");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 9

Note: marker note

---

> First quote.

---

> Deleted quote.
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial tombstone sync"),
    );
    let initial = fs::read_to_string(&note).expect("read initial note");
    let initial_ids = highlight_block_ids(&initial);
    assert_eq!(initial_ids.len(), 2, "{initial}");
    let deleted_id = initial_ids[1].clone();

    write_file(
        &sidecar,
        "\
## Page 9

Note: marker note

---

> First quote.
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync deleted highlight");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains("highlights_count: 1\n"), "{updated}");
    assert!(updated.contains("### Removed highlights\n"), "{updated}");
    assert!(
        updated.contains(&format!("^{deleted_id}\n")),
        "deleted block id should remain as a tombstone:\n{updated}"
    );
    assert!(
        updated.contains("> [!warning] Removed highlight This annotation is no longer present in the Highlights sidecar.\n"),
        "{updated}"
    );
    assert!(
        !updated.contains("> [!quote] Deleted quote.\n"),
        "{updated}"
    );
}

#[test]
fn highlights_ref_sync_preserves_manual_sections_and_rejects_missing_markers() {
    let temp = TempDir::new("bob-cli-highlights-ref-manual-body");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &sidecar,
        "\
## Page 2

Note: marker note

---

> Initial quote.
",
    );
    assert_success(
        &bob_command()
            .arg("highlights")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial manual-body sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read note")
        .replacen("---\n", "---\nowner: Bryan\n", 1)
        .replace(
            "## Highlights\n\n",
            "## Manual Notes\n\nManual synthesis.\n\n## Highlights\n\n",
        );
    write_file(&note, &edited);
    write_file(
        &sidecar,
        "\
## Page 2

Note: marker note

---

> Initial quote.

Comment: added later
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync with manual content");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains("owner: Bryan\n"), "{updated}");
    assert!(updated.contains("Manual synthesis.\n"), "{updated}");
    assert!(updated.contains("[!note] Comment added later"), "{updated}");

    let unsafe_pdf = vault.join("lib/unsafe.pdf");
    let unsafe_note = vault.join("ref/unsafe.md");
    write_highlights_pdf(&unsafe_pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &unsafe_note,
        "\
---
status: wip
parent: \"[[obsidian]]\"
---

Manual note without generated markers.
",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg(&unsafe_pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync unsafe existing note");

    assert_eq!(
        output.status.code(),
        Some(1),
        "existing note without markers should fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("missing the managed Highlights region"),
        "expected managed-region error:\n{}",
        format_output(&output)
    );
}

#[test]
fn move_done_tasks_commits_and_pushes_collection_changes_only() {
    let temp = TempDir::new("bob-cli-move-done-tasks-git");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let unrelated = vault.join("unrelated.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );
    write_file(&unrelated, "- [ ] unrelated #task\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    write_file(&unrelated, "- [ ] unrelated #task\nlocal edit\n");

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("git:")
            && output_text
                .contains("committed: bob move-done-tasks 2026-06-02")
            && output_text.contains("pushed"),
        "expected git section with commit and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    let archive_contents = fs::read_to_string(&archive).expect("read archive");
    assert!(
        archive_contents.contains("parent: \"[[obsidian]]\"")
            && archive_contents.contains("type: \"[[done]]\"")
            && archive_contents.contains("- [x] done #task"),
        "expected archive metadata and moved task:\n{archive_contents}"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.starts_with("bob move-done-tasks 2026-06-02\n"),
        "expected move-done-tasks commit subject:\n{show}"
    );
    assert!(
        show.contains("\nobsidian.md\n"),
        "expected source in commit:\n{show}"
    );
    assert!(
        show.contains("\ndone/obsidian_done.md\n"),
        "expected archive in commit:\n{show}"
    );
    assert!(
        !show.contains("unrelated.md"),
        "unrelated dirty file must not be committed:\n{show}"
    );

    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");

    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert!(
        status.contains(" M unrelated.md"),
        "unrelated dirty file should remain dirty:\n{status}"
    );
    assert!(
        !status.contains("obsidian.md")
            && !status.contains("done/obsidian_done.md"),
        "collection paths should be clean after commit:\n{status}"
    );
}

#[test]
fn move_done_tasks_commits_link_repairs_with_collection_changes() {
    let temp = TempDir::new("bob-cli-move-done-tasks-link-repair-git");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let daily = vault.join("daily.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
- [x] done #task ^abc123
- [ ] active #task
",
    );
    write_file(
        &daily,
        "Links [[obsidian#^abc123|alias]] and ![[obsidian#^abc123]].\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with link repair");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("Obsidian links repaired: 2")
            && output_text.contains("link-repair files updated: 1")
            && output_text
                .contains("committed: bob move-done-tasks 2026-06-02")
            && output_text.contains("pushed"),
        "expected link repair commit and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&daily).expect("read daily"),
        "Links [[done/obsidian_done#^abc123|alias]] and ![[done/obsidian_done#^abc123]].\n"
    );
    assert!(
        fs::read_to_string(&archive)
            .expect("read archive")
            .contains("- [x] done #task ^abc123"),
        "expected archived task"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.contains("\nobsidian.md\n")
            && show.contains("\ndone/obsidian_done.md\n")
            && show.contains("\ndaily.md\n"),
        "expected source, archive, and link repair note in commit:\n{show}"
    );
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn move_done_tasks_deduplicates_archive_block_ids_and_repairs_links() {
    let temp = TempDir::new("bob-cli-move-done-tasks-block-id-dedup-git");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let daily = vault.join("daily.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
- [x] done #task ^abc123
- [ ] active #task
",
    );
    write_file(&daily, "Links [[obsidian#^abc123]].\n");
    write_file(
        &archive,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task ^abc123
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with block id collision");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("moved block id renames: 1")
            && output_text.contains("Obsidian links repaired: 1")
            && output_text
                .contains("committed: bob move-done-tasks 2026-06-02")
            && output_text.contains("pushed"),
        "expected block id rename, link repair, commit, and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&daily).expect("read daily"),
        "Links [[done/obsidian_done#^abc123-1]].\n"
    );
    let archive_contents = fs::read_to_string(&archive).expect("read archive");
    assert!(
        archive_contents.contains("- [x] archived #task ^abc123\n")
            && archive_contents.contains("- [x] done #task ^abc123-1\n"),
        "expected existing and renamed moved block ids:\n{archive_contents}"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.starts_with("bob move-done-tasks 2026-06-02\n")
            && show.contains("\nobsidian.md\n")
            && show.contains("\ndone/obsidian_done.md\n")
            && show.contains("\ndaily.md\n"),
        "expected source, archive, and repaired link note in commit:\n{show}"
    );
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn move_done_tasks_commits_metadata_only_source_updates() {
    let temp = TempDir::new("bob-cli-move-done-tasks-git-metadata");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(&source, "- [ ] active #task\n");
    write_file(
        &archive,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("source done_tasks updates: 1")
            && output_text
                .contains("committed: bob move-done-tasks 2026-06-02")
            && output_text.contains("pushed"),
        "expected metadata commit and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.starts_with("bob move-done-tasks 2026-06-02\n"),
        "expected move-done-tasks commit subject:\n{show}"
    );
    assert!(
        show.contains("\nobsidian.md\n"),
        "expected source in metadata commit:\n{show}"
    );
    assert!(
        !show.contains("\ndone/obsidian_done.md\n"),
        "metadata-only commit should not stage archive:\n{show}"
    );

    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn move_done_tasks_commits_metadata_only_archive_repairs() {
    let temp = TempDir::new("bob-cli-move-done-tasks-git-archive-metadata");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &archive,
        "\
---
parent: \"[[done]]\"
---

- [x] archived #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks archive metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("archive metadata repairs: 1")
            && output_text
                .contains("committed: bob move-done-tasks 2026-06-02")
            && output_text.contains("pushed"),
        "expected archive metadata commit and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.starts_with("bob move-done-tasks 2026-06-02\n"),
        "expected move-done-tasks commit subject:\n{show}"
    );
    assert!(
        !show.contains("\nobsidian.md\n"),
        "archive-only commit should not stage source:\n{show}"
    );
    assert!(
        show.contains("\ndone/obsidian_done.md\n"),
        "expected archive in metadata repair commit:\n{show}"
    );

    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn move_done_tasks_warns_and_skips_git_for_non_repo_vault() {
    let temp = TempDir::new("bob-cli-move-done-tasks-non-repo");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("-t1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks outside git repo");

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "warning: vault is not a git worktree; skipping commit and push"
        ),
        "expected non-repo warning:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join(".git").exists(),
        "move-done-tasks must not initialize git"
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
}

#[test]
fn move_done_tasks_moves_canceled_tasks_in_non_repo_vault() {
    let temp = TempDir::new("bob-cli-move-done-tasks-canceled-non-repo");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    write_file(
        &source,
        "\
- [-] canceled one #task
- [-] canceled two #task
- [ ] active #task
",
    );

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=2")
        .env("BOB_DIR", &vault)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with canceled tasks");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("files meeting threshold: 1")
            && output_text.contains("task blocks: 2")
            && output_text.contains("moved task blocks: 2")
            && output_text.contains(
                "warning: vault is not a git worktree; skipping commit and push"
            ),
        "expected canceled task movement in non-repo vault:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [-] canceled one #task
- [-] canceled two #task
"
    );
}

#[test]
fn move_done_tasks_rewrites_dirty_link_repair_files() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-link-repair");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let daily = vault.join("daily.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    let source_contents = "\
- [x] done #task ^abc123
- [ ] active #task
";
    write_file(&source, source_contents);
    write_file(&daily, "Reference [[obsidian#^abc123]].\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_daily = "Reference [[obsidian#^abc123]].\nlocal edit\n";
    write_file(&daily, dirty_daily);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty link repair candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("Obsidian links repaired: 1")
            && stdout(&output)
                .contains("committed: bob move-done-tasks 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty link repair candidate success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&daily).expect("read daily"),
        "Reference [[done/obsidian_done#^abc123]].\nlocal edit\n"
    );
    assert!(
        fs::read_to_string(&archive)
            .expect("read archive")
            .contains("- [x] done #task ^abc123"),
        "expected archived task"
    );
    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.contains("\nobsidian.md\n")
            && show.contains("\ndone/obsidian_done.md\n")
            && show.contains("\ndaily.md\n"),
        "expected dirty link repair paths in commit:\n{show}"
    );
    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert!(
        status.trim().is_empty(),
        "dirty link repair paths should be clean after commit:\n{status}"
    );
}

#[test]
fn move_done_tasks_rewrites_dirty_candidate_files() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-candidate");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_source = "\
- [x] done #task
- [ ] active #task
local edit
";
    write_file(&source, dirty_source);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("task blocks: 1")
            && stdout(&output)
                .contains("committed: bob move-done-tasks 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty candidate success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
local edit
"
    );
    assert!(
        fs::read_to_string(&archive)
            .expect("read archive")
            .contains("- [x] done #task"),
        "expected archived task"
    );
    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.contains("\nobsidian.md\n")
            && show.contains("\ndone/obsidian_done.md\n"),
        "expected dirty source and archive in commit:\n{show}"
    );
    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert!(
        status.trim().is_empty(),
        "dirty candidate paths should be clean after commit:\n{status}"
    );
}

#[test]
fn move_done_tasks_rewrites_dirty_metadata_only_source() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-metadata");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(&source, "- [ ] active #task\n");
    write_file(
        &archive,
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_source = "- [ ] active #task\nlocal edit\n";
    write_file(&source, dirty_source);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty metadata candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("source done_tasks updates: 1")
            && stdout(&output)
                .contains("committed: bob move-done-tasks 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty metadata source success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
local edit
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
"
    );
    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.contains("\nobsidian.md\n")
            && !show.contains("\ndone/obsidian_done.md\n"),
        "expected only dirty metadata source in commit:\n{show}"
    );
    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert!(
        status.trim().is_empty(),
        "dirty metadata source should be clean after commit:\n{status}"
    );
}

#[test]
fn move_done_tasks_rewrites_dirty_metadata_only_archive() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-archive");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
    );
    write_file(
        &archive,
        "\
---
parent: \"[[done]]\"
---

- [x] archived #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_archive = "\
---
parent: \"[[done]]\"
---

- [x] archived #task
local edit
";
    write_file(&archive, dirty_archive);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect(
            "run bob move-done-tasks with dirty archive metadata candidate",
        );

    assert_success(&output);
    assert!(
        stdout(&output).contains("archive metadata repairs: 1")
            && stdout(&output)
                .contains("committed: bob move-done-tasks 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty archive metadata success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] archived #task
local edit
"
    );
    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        !show.contains("\nobsidian.md\n")
            && show.contains("\ndone/obsidian_done.md\n"),
        "expected only dirty archive metadata file in commit:\n{show}"
    );
    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert!(
        status.trim().is_empty(),
        "dirty metadata archive should be clean after commit:\n{status}"
    );
}

#[test]
fn pomodoro_formats_native_pomodoro_status() {
    let temp = TempDir::new("bob-cli-pomodoro-path");
    let output = bob_command()
        .arg("pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn tmux_pomodoro_formats_native_pomodoro_status() {
    let temp = TempDir::new("bob-cli-tmux-path");
    let output = bob_command()
        .arg("tmux-pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton | ");
}

#[test]
fn pomodoro_reads_default_bare_daily_file_from_bob_dir() {
    let temp = TempDir::new("bob-cli-pomodoro-default-bare");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        &fs::read_to_string(fixture("pomodoro/day_with_open_pomodoro.md"))
            .expect("read pomodoro fixture"),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with default bare daily path");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_reads_default_bare_daily_file_from_bob_dir() {
    let temp = TempDir::new("bob-cli-script-pomodoro-default-bare");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        &fs::read_to_string(fixture("pomodoro/day_with_open_pomodoro.md"))
            .expect("read pomodoro fixture"),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro with default bare daily path");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_accepts_inline_duration_field_in_time_range() {
    let temp = TempDir::new("bob-cli-script-pomodoro");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn pomodoro_accepts_legacy_unbolded_inline_duration_field_in_time_range() {
    let temp = TempDir::new("bob-cli-pomodoro-legacy");
    let output = bob_command()
        .arg("pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_legacy_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with legacy time range");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_accepts_legacy_unbolded_time_range() {
    let temp = TempDir::new("bob-cli-script-pomodoro-legacy");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_legacy_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro with legacy time range");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn pomodoro_missing_day_file_is_a_successful_noop() {
    let temp = TempDir::new("bob-cli-pomodoro-missing");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with missing day file");

    assert_success(&output);
    assert!(stdout(&output).is_empty(), "expected empty stdout");
    assert!(stderr(&output).is_empty(), "expected empty stderr");
}

#[test]
fn bulk_git_commit_commits_and_pushes_without_running_ob() {
    let temp = TempDir::new("bob-cli-bulk-git-commit");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    fs::create_dir_all(&home).expect("create home");

    // An `ob` stub that fails loudly if invoked: standalone
    // `bob bulk-git-commit` must not touch Obsidian (that moved up to
    // `bob nightly`).
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$STUB_LOG"
exit 99
"#,
    );
    write_executable(
        &stub_bin.join("git"),
        r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "-C" ]; then
  shift 2
fi
case "$1" in
  rev-parse)
    printf 'true\n'
    exit 0
    ;;
  add)
    exit 0
    ;;
  diff)
    exit 0
    ;;
  push)
    exit 0
    ;;
esac
exit 64
"#,
    );

    let output = bob_command()
        .arg("bulk-git-commit")
        .env("BOB_DIR", &vault)
        .env(
            "BOB_BULK_GIT_COMMIT_LOCK_FILE",
            temp.path().join("bob_bulk_git_commit.lock"),
        )
        .env_remove("OB_COMMAND")
        .env("HOME", &home)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob bulk-git-commit");

    assert_success(&output);
    let log_contents = fs::read_to_string(&log).expect("read stub command log");
    assert!(
        !log_contents.contains("ob "),
        "standalone bulk-git-commit must not invoke ob:\n{log_contents}"
    );
    assert!(log_contents.contains(&format!(
        "git -C {} rev-parse --is-inside-work-tree",
        vault.display()
    )));
    assert!(
        log_contents.contains(&format!("git -C {} add -A .", vault.display()))
    );
    assert!(log_contents.contains(&format!(
        "git -C {} diff --cached --quiet --exit-code",
        vault.display()
    )));
    assert!(log_contents.contains(&format!("git -C {} push", vault.display())));
    assert!(
        !log_contents.contains(" commit "),
        "no-change bulk-git-commit should not commit"
    );
}

#[test]
fn legacy_bob_sync_binary_runs_bulk_git_commit_native_path() {
    let temp = TempDir::new("bob-cli-legacy-bob-sync");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create home");
    write_file(&vault.join("initial.md"), "- [ ] initial #task\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    write_file(&vault.join("extra.md"), "- [ ] extra #task\n");

    let output = bob_sync_command()
        .env("BOB_DIR", &vault)
        .env("BOB_BULK_GIT_COMMIT_MESSAGE", "legacy binary commit")
        .env(
            "BOB_BULK_GIT_COMMIT_LOCK_FILE",
            temp.path().join("bob_bulk_git_commit.lock"),
        )
        .env_remove("BOB_CLI_USE_SCRIPT")
        .env_remove("OB_COMMAND")
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run legacy bob_sync binary");

    assert_success(&output);
    assert_eq!(
        stdout(&git_in(&vault, ["log", "-1", "--format=%s"])).trim(),
        "legacy binary commit",
        "legacy bob_sync should use the native bulk-git-commit implementation"
    );
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn renamed_old_top_level_commands_are_unknown() {
    for command in ["move-done-tasks", "bulk-git-commit"] {
        let output = bob_command()
            .arg(command)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob {command} --help: {error}")
            });
        assert_success(&output);
    }

    for command in ["collect-done", "cronjob", "highlights-ref", "sync"] {
        let output = bob_command()
            .arg(command)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob {command} --help: {error}")
            });
        assert_eq!(
            output.status.code(),
            Some(2),
            "old top-level command should be rejected:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("unrecognized subcommand"),
            "expected clap unknown-command error:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn top_level_help_lists_commands_alphabetically_with_examples() {
    let output = bob_command().arg("-h").output().expect("run bob -h");

    assert_success(&output);
    let help = stdout(&output);

    let order = [
        "bulk-git-commit",
        "dataview",
        "highlights",
        "move-done-tasks",
        "nightly",
        "notify",
        "pomodoro",
        "tmux-pomodoro",
    ];
    let mut last = 0;
    for command in order {
        let needle = format!("\n  {command} ");
        let position = help.find(&needle).unwrap_or_else(|| {
            panic!("expected command `{command}` in help:\n{help}")
        });
        assert!(
            position >= last,
            "command `{command}` is out of alphabetical order:\n{help}"
        );
        last = position;
    }

    assert!(
        help.contains("Examples:")
            && help.contains("bob bulk-git-commit")
            && help.contains("bob dataview --source '#project'")
            && help.contains("bob highlights scan --dry-run")
            && help.contains("bob move-done-tasks --threshold 10")
            && help.contains("bob nightly")
            && help.contains("bob pomodoro"),
        "expected an Examples section:\n{help}"
    );
    assert!(
        !help.contains("cronjob"),
        "top-level help should not list the old cronjob spelling:\n{help}"
    );
    assert!(
        !help.contains("highlights-ref"),
        "top-level help should not list the old highlights-ref spelling:\n{help}"
    );
    assert!(
        help.contains("Run 'bob <command> --help' for more information"),
        "expected a per-command help footer:\n{help}"
    );

    assert!(
        !output.stdout.contains(&0x1b),
        "piped help output must not contain ANSI escape codes:\n{help}"
    );
}

#[test]
fn nightly_runs_shared_sync_once_then_wrapped_steps_in_order() {
    let temp = TempDir::new("bob-cli-nightly-happy");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    let extra = vault.join("extra.md");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    // Untracked file the wrapped `bulk-git-commit` step should commit
    // wholesale.
    write_file(&extra, "- [ ] extra #task\n");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "sync" ]; then
  if [ -f "$ARCHIVE_FILE" ]; then
    printf 'archive-exists-before-sync\n' >> "$STUB_LOG"
  else
    printf 'archive-missing-before-sync\n' >> "$STUB_LOG"
  fi
fi
case "$1" in
  sync|sync-status) exit 0 ;;
esac
exit 64
"#,
    );

    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env(
            "BOB_BULK_GIT_COMMIT_MESSAGE",
            "bob bulk-git-commit 2026-06-02",
        )
        .env(
            "BOB_BULK_GIT_COMMIT_LOCK_FILE",
            temp.path().join("bob_bulk_git_commit.lock"),
        )
        .env("HOME", &home)
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("ARCHIVE_FILE", &archive)
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly");

    assert_success(&output);
    let out = stdout(&output);

    // The shared Obsidian sync ran exactly once, before any wrapped step
    // mutated the vault.
    let log_contents = fs::read_to_string(&log).expect("read stub log");
    let sync_calls = log_contents
        .lines()
        .filter(|line| line.starts_with("ob sync --path"))
        .count();
    assert_eq!(
        sync_calls, 1,
        "shared sync should run once:\n{log_contents}"
    );
    assert!(
        log_contents.contains("archive-missing-before-sync"),
        "wrapped steps must run after the shared sync:\n{log_contents}"
    );

    // move-done-tasks committed first, then bulk-git-commit
    // (bulk-git-commit is the newest commit).
    let subjects = stdout(&git_in(&vault, ["log", "--format=%s"]));
    let lines: Vec<&str> = subjects.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some("bob bulk-git-commit 2026-06-02"),
        "bulk-git-commit should be the most recent commit:\n{subjects}"
    );
    assert_eq!(
        lines.get(1).copied(),
        Some("bob move-done-tasks 2026-06-02"),
        "move-done-tasks should be committed before bulk-git-commit:\n{subjects}"
    );

    // The wrapped bulk-git-commit step committed the untracked file wholesale.
    let bulk_show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert!(
        bulk_show.contains("extra.md"),
        "bulk-git-commit step should commit the untracked file:\n{bulk_show}"
    );

    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");

    // move-done-tasks archived the done tasks and linked the source.
    let archive_contents = fs::read_to_string(&archive).expect("read archive");
    assert!(
        archive_contents.contains("parent: \"[[obsidian]]\"")
            && archive_contents.contains("- [x] done 1 #task"),
        "expected archived tasks:\n{archive_contents}"
    );
    assert!(
        fs::read_to_string(&source)
            .expect("read source")
            .contains("done_tasks: \"[[done/obsidian_done]]\""),
        "expected source link in {}",
        source.display()
    );

    // The summary reports every step passing, in plain text.
    assert!(
        out.contains("bob nightly")
            && out.contains("Obsidian sync (shared, runs once)")
            && out.contains("move-done-tasks")
            && out.contains("All steps passed"),
        "expected a structured nightly summary:\n{}",
        format_output(&output)
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "piped nightly output must not contain ANSI escape codes:\n{out}"
    );
}

#[test]
fn nightly_failing_shared_sync_aborts_before_wrapped_steps() {
    let temp = TempDir::new("bob-cli-nightly-sync-fail");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    let head_before = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  printf 'sync failed\n' >&2
  exit 42
fi
exit 64
"#,
    );

    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly with failing sync");

    assert_eq!(
        output.status.code(),
        Some(42),
        "expected gate sync failure exit code:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("Aborted") && out.contains("obsidian-sync"),
        "expected an abort summary:\n{}",
        format_output(&output)
    );
    assert!(
        !out.contains("Move done tasks") && !out.contains("step 1/2"),
        "no wrapped step should run after a failed gate sync:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        head_before,
        "a failed gate sync must not create commits"
    );
    assert!(
        !archive.exists(),
        "archive must not be created after a failed gate sync"
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        done_tasks_source(12),
        "source must be untouched after a failed gate sync"
    );
}

#[test]
fn nightly_failed_step_still_runs_later_steps_and_exits_nonzero() {
    let temp = TempDir::new("bob-cli-nightly-step-fail");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    let blocking_done_path = vault.join("done");
    let extra = vault.join("extra.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    write_file(
        &blocking_done_path,
        "regular file blocking done/ archive directory\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    // The regular file at `done` makes the archive target
    // `done/obsidian_done.md` impossible to read or write, so
    // move-done-tasks fails while the later bulk-git-commit step can still
    // commit unrelated vault changes.
    write_file(&extra, "- [ ] later step still runs #task\n");
    write_successful_ob_stub(&stub_bin);

    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_COMMIT_MESSAGE", "legacy fallback commit")
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly with a failing wrapped step");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected the failing step's exit code:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("\u{2717} move-done-tasks"),
        "expected a failed move-done-tasks marker:\n{}",
        format_output(&output)
    );
    assert!(
        out.contains("1 step failed"),
        "expected a failure count in the summary:\n{}",
        format_output(&output)
    );

    // The bulk-git-commit step still ran: the unrelated vault change was
    // committed and pushed.
    assert_eq!(
        stdout(&git_in(&vault, ["log", "-1", "--format=%s"])).trim(),
        "legacy fallback commit",
        "the later bulk-git-commit step should commit despite the earlier failure"
    );
    let bulk_show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert!(
        bulk_show.contains("extra.md"),
        "bulk-git-commit step should commit the unrelated file:\n{bulk_show}"
    );
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        remote_head, local_head,
        "bulk-git-commit step should push to the remote"
    );
}

fn done_tasks_source(count: usize) -> String {
    let mut text = String::new();
    for index in 1..=count {
        text.push_str(&format!("- [x] done {index} #task\n"));
    }
    text.push_str("- [ ] active #task\n");
    text
}

fn bob_command() -> Command {
    Command::new(BOB_BIN)
}

fn bob_notify_command() -> Command {
    Command::new(BOB_NOTIFY_BIN)
}

fn bob_pomodoro_command() -> Command {
    Command::new(BOB_POMODORO_BIN)
}

fn bob_sync_command() -> Command {
    Command::new(BOB_SYNC_BIN)
}

fn tmux_bob_pomodoro_command() -> Command {
    Command::new(TMUX_BOB_POMODORO_BIN)
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

fn single_script_cache_dir(cache_home: &Path) -> PathBuf {
    let scripts_root = cache_home.join("bob-cli/scripts");
    let mut entries: Vec<_> = fs::read_dir(&scripts_root)
        .unwrap_or_else(|error| {
            panic!("read script cache root {}: {error}", scripts_root.display())
        })
        .map(|entry| entry.expect("read cache entry").path())
        .collect();

    entries.sort();
    assert_eq!(entries.len(), 1, "expected one script cache directory");
    entries.pop().expect("script cache directory")
}

fn init_git_vault_with_remote(temp: &TempDir) -> (PathBuf, PathBuf) {
    let vault = temp.path().join("vault");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&vault).expect("create vault");
    git(["init", "-q", "--bare", path_str(&remote)]);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["remote", "add", "origin", path_str(&remote)]);
    (vault, remote)
}

fn write_successful_ob_stub(stub_bin: &Path) {
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );
}

fn git<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .args(args)
        .output()
        .expect("run git");
    assert_success(&output);
    output
}

fn git_in<I, S>(directory: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("run git");
    assert_success(&output);
    output
}

fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("path is not UTF-8: {}", path.display()))
}

fn path_with_prefix(prefix: &Path) -> String {
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    std::env::join_paths(paths)
        .expect("join PATH")
        .into_string()
        .expect("PATH is UTF-8")
}

fn highlight_block_ids(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('^')
                .filter(|id| id.starts_with("h-"))
                .map(str::to_string)
        })
        .collect()
}

fn find_created_annotation_task(contents: &str, prose: &str) -> String {
    contents
        .lines()
        .find(|line| line.starts_with("- [ ]") && line.contains(prose))
        .unwrap_or_else(|| {
            panic!("missing created annotation task for {prose}:\n{contents}")
        })
        .to_string()
}

fn created_annotation_task_count(contents: &str, prose: &str) -> usize {
    contents
        .lines()
        .filter(|line| line.starts_with("- [") && line.contains(prose))
        .count()
}

fn annotation_task_source_link_id(line: &str) -> String {
    let start = line.find("[[").expect("source link present") + 2;
    let rest = &line[start..];
    let end = rest.find("]]").expect("source link terminator");
    let inside = &rest[..end];
    let target = inside.split_once('|').map_or(inside, |(target, _)| target);
    target
        .rsplit_once("#^")
        .unwrap_or_else(|| panic!("source link has no block id: {line}"))
        .1
        .to_string()
}

fn legacy_highlight_task_id(
    ref_note_path: &str,
    source_block_id: &str,
    identity: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update("v1");
    hasher.update([0]);
    hasher.update(ref_note_path);
    hasher.update([0]);
    hasher.update(source_block_id);
    hasher.update([0]);
    hasher.update(identity);
    hex::encode(hasher.finalize())
}

fn write_highlights_pdf(path: &Path, marker_contents: &str) {
    write_highlights_pdf_pages(path, &[&[marker_contents]]);
}

fn write_highlights_pdf_pages(path: &Path, page_text_annotations: &[&[&str]]) {
    use lopdf::{dictionary, Document, Object, Stream};

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }

    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut page_ids = Vec::new();
    for annotations in page_text_annotations {
        let content_id =
            doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let annot_refs = annotations
            .iter()
            .map(|contents| {
                let annot_id = doc.add_object(dictionary! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => vec![
                        Object::Integer(0),
                        Object::Integer(0),
                        Object::Integer(24),
                        Object::Integer(24),
                    ],
                    "Contents" => pdf_text_string(contents),
                });
                Object::Reference(annot_id)
            })
            .collect::<Vec<_>>();
        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
        };
        if !annot_refs.is_empty() {
            page.set("Annots", Object::Array(annot_refs));
        }
        let page_id = doc.add_object(page);
        page_ids.push(Object::Reference(page_id));
    }
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids,
            "Count" => page_text_annotations.len() as i64,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

fn set_pdf_marker_contents(path: &Path, marker_contents: &str) {
    let mut doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    doc.get_object_mut(marker_id)
        .expect("get marker object")
        .as_dict_mut()
        .expect("marker is dictionary")
        .set("Contents", pdf_text_string(marker_contents));
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

fn set_pdf_marker_literal_contents(path: &Path, marker_contents: &str) {
    let mut doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    doc.get_object_mut(marker_id)
        .expect("get marker object")
        .as_dict_mut()
        .expect("marker is dictionary")
        .set("Contents", pdf_literal_string(marker_contents));
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

fn pdf_marker_contents(path: &Path) -> String {
    let doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    let marker = doc
        .get_dictionary(marker_id)
        .expect("get marker annotation dictionary");
    lopdf::decode_text_string(marker.get(b"Contents").expect("marker contents"))
        .expect("decode marker contents")
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    hex::encode(Sha256::digest(bytes))
}

fn first_text_annotation_id(doc: &lopdf::Document) -> lopdf::ObjectId {
    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).expect("get page dictionary");
        let annots = page
            .get(b"Annots")
            .expect("page annotations")
            .as_array()
            .expect("annotation array");
        for annot in annots {
            let annot_id = annot.as_reference().expect("annotation reference");
            let annot_dict =
                doc.get_dictionary(annot_id).expect("annotation dictionary");
            if annot_dict
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .is_ok_and(|name| name == b"Text")
            {
                return annot_id;
            }
        }
    }
    panic!("missing /Text annotation");
}

fn pdf_text_string(contents: &str) -> lopdf::Object {
    lopdf::Object::String(
        lopdf::encode_utf16_be(contents),
        lopdf::StringFormat::Hexadecimal,
    )
}

fn pdf_literal_string(contents: &str) -> lopdf::Object {
    lopdf::Object::String(
        contents.as_bytes().to_vec(),
        lopdf::StringFormat::Literal,
    )
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| {
        panic!("write executable stub {}: {error}", path.display())
    });
    set_mode(path, 0o755);
}

fn write_obsidian_success_stub(path: &Path, payload: &str) {
    let sentinel_line =
        shell_single_quote(&format!("BOB_DATAVIEW_RESULT\t{payload}"));
    write_executable(
        path,
        &format!(
            "#!/bin/sh\n\
             : > \"$STUB_LOG\"\n\
             for arg in \"$@\"; do printf 'ARG:%s\\n' \"$arg\" >> \"$STUB_LOG\"; done\n\
             printf 'plugin log before\\n'\n\
             printf '%s\\n' {sentinel_line}\n\
             printf 'plugin log after\\n'\n"
        ),
    );
}

fn write_native_parent_chain_fixture(vault: &Path) {
    write_file(&vault.join("ai_ref.md"), "---\n---\n");
    write_file(&vault.join("sase.md"), "---\nparent: \"[[tools]]\"\n---\n");
    write_file(
        &vault.join("agent_ref.md"),
        "---\nparent: \"[[ai_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("memory_ref.md"),
        "---\nparent: \"[[agent_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/direct_ai.md"),
        "---\nsource_pdf: lib/papers/direct-ai.pdf\nsource_path: lib/papers/direct-ai.pdf\nstatus: direct\nparent: \"[[ai_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/memory_os.md"),
        "---\nsource_pdf: lib/papers/memory-os.pdf\nsource_path: lib/papers/memory-os.pdf\nstatus: inherited\nparent: \"[[memory_ref]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/log_is_the_agent.md"),
        "---\nsource_pdf: lib/papers/log-is-the-agent.pdf\nsource_path: lib/papers/log-is-the-agent.pdf\nstatus: other\nparent: \"[[sase]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/chat/obsidian-note-refactor.md"),
        "---\nsource_pdf: lib/chat/obsidian-note-refactor.pdf\nsource_path: lib/chat/obsidian-note-refactor.pdf\nstatus: other\nparent: \"[[obsidian]]\"\n---\n",
    );
    write_file(
        &vault.join("ref/papers/not_a_pdf.md"),
        "---\nstatus: inherited\nparent: \"[[memory_ref]]\"\n---\n",
    );
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn assert_unix_mode(path: &Path, expected: u32) {
    let mode = fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .mode()
        & 0o777;
    assert_eq!(mode, expected, "unexpected mode for {}", path.display());
}

#[cfg(not(unix))]
fn assert_unix_mode(_path: &Path, _expected: u32) {}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command success:\n{}",
        format_output(output)
    );
}

fn assert_stdout_has_no_ansi(output: &Output) {
    assert!(
        !output.stdout.contains(&0x1b),
        "stdout must not contain ANSI escape codes:\n{}",
        stdout(output)
    );
}

fn assert_text_order(text: &str, needles: &[&str]) {
    let mut last = 0;
    for needle in needles {
        let position = text
            .find(needle)
            .unwrap_or_else(|| panic!("expected `{needle}` in text:\n{text}"));
        assert!(position >= last, "`{needle}` is out of order:\n{text}");
        last = position;
    }
}

fn assert_no_long_only_option_lines(label: &str, help: &str) {
    for line in help.lines() {
        let trimmed = line.trim_start();
        let starts_with_long_option = trimmed
            .strip_prefix("--")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|first| first.is_ascii_alphabetic());
        if starts_with_long_option {
            panic!(
                "{label} exposes a long-only option line:\n{line}\n\n{help}"
            );
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn format_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    )
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            current_time_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = remove_dir_all_if_exists(&self.path) {
            eprintln!(
                "failed to remove temp dir {}: {error}",
                self.path.display()
            );
        }
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

fn remove_dir_all_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
