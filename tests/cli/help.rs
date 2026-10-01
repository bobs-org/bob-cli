//! Cache extraction, native-only help, legacy and script help.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const BOB_NOTIFY_BIN: &str = env!("CARGO_BIN_EXE_bob_notify");
const BOB_POMODORO_BIN: &str = env!("CARGO_BIN_EXE_bob_pomodoro");
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
    let executable_assets = ["bob_pomodoro", "bob_notify", "tmux_bob_pomodoro"];

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
fn task_status_hooks_help_is_native_only() {
    let temp = TempDir::new("bob-cli-task-status-hooks-native-help");
    for spelling in
        ["task-status-hooks", "task-status-setter", "mark-next-tasks"]
    {
        let output = bob_command()
            .arg(spelling)
            .arg("--help")
            .env("BOB_CLI_USE_SCRIPT", "1")
            .env("XDG_CACHE_HOME", temp.path())
            .output()
            .unwrap_or_else(|error| {
                panic!("run native-only bob {spelling} --help: {error}")
            });

        assert_success(&output);
        assert!(
            stdout(&output).contains("Usage: bob task-status-hooks"),
            "expected canonical task-status-hooks help for {spelling}:\n{}",
            format_output(&output)
        );
        assert_stdout_has_no_ansi(&output);

        let diagnostic = bob_command()
            .arg(spelling)
            .arg("--unknown-option")
            .env("BOB_CLI_USE_SCRIPT", "1")
            .env("XDG_CACHE_HOME", temp.path())
            .output()
            .unwrap_or_else(|error| {
                panic!("run native-only bob {spelling} diagnostic: {error}")
            });
        assert_eq!(diagnostic.status.code(), Some(2));
        assert!(
            stderr(&diagnostic).contains("Usage: bob task-status-hooks"),
            "expected canonical task-status-hooks diagnostic for {spelling}:\n{}",
            format_output(&diagnostic)
        );
    }
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "task-status-hooks and its compatibility aliases must stay native-only"
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
fn capture_complete_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-complete-native-help");
    let output = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-complete --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-complete"),
        "expected capture-complete help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-complete should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-parse-native-help");
    let output = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-parse --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-parse"),
        "expected capture-parse help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-parse should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoro_name_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-name-native-help");
    let output = bob_command()
        .arg("capture-pomodoro-name")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-pomodoro-name --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-pomodoro-name"),
        "expected capture-pomodoro-name help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-pomodoro-name should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoros_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-pomodoros-native-help");
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-pomodoros --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-pomodoros"),
        "expected capture-pomodoros help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-pomodoros should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_sections_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-sections-native-help");
    let output = bob_command()
        .arg("capture-sections")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-sections --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-sections"),
        "expected capture-sections help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-sections should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_task_id_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-task-id-native-help");
    let output = bob_command()
        .arg("capture-task-id")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-task-id --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-task-id"),
        "expected capture-task-id help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-task-id should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_task_sections_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-task-sections-native-help");
    let output = bob_command()
        .arg("capture-task-sections")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-task-sections --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-task-sections"),
        "expected capture-task-sections help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-task-sections should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_tasks_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-tasks-native-help");
    let output = bob_command()
        .arg("capture-tasks")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-tasks --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-tasks"),
        "expected capture-tasks help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-tasks should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_targets_help_is_native_only() {
    let temp = TempDir::new("bob-cli-capture-targets-native-help");
    let output = bob_command()
        .arg("capture-targets")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob capture-targets --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob capture-targets"),
        "expected capture-targets help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only capture-targets should not extract script assets"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn dataview_help_is_native_only() {
    let temp = TempDir::new("bob-cli-dataview-native-help");
    let output = bob_command()
        .arg("query")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob query --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob query"),
        "expected query help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only query should not extract script assets"
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
        &["highlights", "create", "--help"],
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
        (&["capture", "--help"], "bob capture"),
        (&["capture-complete", "--help"], "bob capture-complete"),
        (&["capture-parse", "--help"], "bob capture-parse"),
        (
            &["capture-pomodoro-name", "--help"],
            "bob capture-pomodoro-name",
        ),
        (&["capture-pomodoros", "--help"], "bob capture-pomodoros"),
        (&["capture-rewrite", "--help"], "bob capture-rewrite"),
        (&["capture-sections", "--help"], "bob capture-sections"),
        (&["capture-targets", "--help"], "bob capture-targets"),
        (&["capture-task-id", "--help"], "bob capture-task-id"),
        (
            &["capture-task-sections", "--help"],
            "bob capture-task-sections",
        ),
        (&["capture-tasks", "--help"], "bob capture-tasks"),
        (&["gkeep", "--help"], "bob gkeep"),
        (&["query", "--help"], "bob query"),
        (&["highlights", "--help"], "Usage: bob highlights"),
        (
            &["task-status-hooks", "--help"],
            "Usage: bob task-status-hooks",
        ),
        (&["move-done-tasks", "--help"], "usage: bob move-done-tasks"),
        (&["nightly", "--help"], "usage: bob nightly"),
        (&["notify", "--help"], "Notify me when"),
        (&["plugins", "--help"], "bob plugins"),
        (&["pomodoro", "--help"], "usage: bob pomodoro"),
        (&["projects", "--help"], "bob projects"),
        (&["ready", "--help"], "Usage: bob ready"),
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
fn capture_named_start_help_mentions_start_forms() {
    let capture = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run bob capture --help");
    assert_success(&capture);
    let help = stdout(&capture);
    for needle in [
        "=<X>#<pomodoro>",
        "bob capture '=#deep-work'",
        "bob capture '=3#bugs'",
        "bob capture '=x =#bugs'",
    ] {
        assert!(
            help.contains(needle),
            "expected `{needle}` in capture help:\n{}",
            format_output(&capture)
        );
    }
    assert!(
        !help.contains("=`<X>`#"),
        "fixed typo must stay gone:\n{}",
        format_output(&capture)
    );

    let parse = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .output()
        .expect("run bob capture-parse --help");
    assert_success(&parse);
    assert!(
        stdout(&parse).contains("=<X>#"),
        "expected named-start mention:\n{}",
        format_output(&parse)
    );

    let complete = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .output()
        .expect("run bob capture-complete --help");
    assert_success(&complete);
    assert!(
        stdout(&complete).contains("pomodoro_start_name"),
        "expected start-name context:\n{}",
        format_output(&complete)
    );
}

#[test]
fn public_help_surfaces_do_not_list_long_only_options() {
    let bob_cases: &[(&[&str], &str)] = &[
        (&["--help"], "bob --help"),
        (&["capture", "--help"], "bob capture --help"),
        (
            &["capture-complete", "--help"],
            "bob capture-complete --help",
        ),
        (&["capture-parse", "--help"], "bob capture-parse --help"),
        (
            &["capture-pomodoro-name", "--help"],
            "bob capture-pomodoro-name --help",
        ),
        (
            &["capture-pomodoros", "--help"],
            "bob capture-pomodoros --help",
        ),
        (&["capture-rewrite", "--help"], "bob capture-rewrite --help"),
        (
            &["capture-sections", "--help"],
            "bob capture-sections --help",
        ),
        (&["capture-targets", "--help"], "bob capture-targets --help"),
        (&["capture-task-id", "--help"], "bob capture-task-id --help"),
        (
            &["capture-task-sections", "--help"],
            "bob capture-task-sections --help",
        ),
        (&["capture-tasks", "--help"], "bob capture-tasks --help"),
        (&["freshness", "--help"], "bob freshness --help"),
        (
            &["freshness", "list", "--help"],
            "bob freshness list --help",
        ),
        (
            &["freshness", "seed", "--help"],
            "bob freshness seed --help",
        ),
        (&["gkeep", "--help"], "bob gkeep --help"),
        (&["gkeep", "doctor", "--help"], "bob gkeep doctor --help"),
        (&["gkeep", "list", "--help"], "bob gkeep list --help"),
        (&["gkeep", "login", "--help"], "bob gkeep login --help"),
        (&["gkeep", "pull", "--help"], "bob gkeep pull --help"),
        (&["query", "--help"], "bob query --help"),
        (&["highlights", "--help"], "bob highlights --help"),
        (
            &["task-status-hooks", "--help"],
            "bob task-status-hooks --help",
        ),
        (
            &["highlights", "create", "--help"],
            "bob highlights create --help",
        ),
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
        (&["plugins", "--help"], "bob plugins --help"),
        (&["plugins", "list", "--help"], "bob plugins list --help"),
        (&["plugins", "sync", "--help"], "bob plugins sync --help"),
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
fn pomodoro_help_documents_show_stale_option() {
    let temp = TempDir::new("bob-cli-pomodoro-show-stale-help");
    let mut cases = vec![
        (
            bob_command()
                .arg("pomodoro")
                .arg("--help")
                .output()
                .expect("run bob pomodoro --help"),
            "bob pomodoro --help",
        ),
        (
            bob_pomodoro_command()
                .arg("--help")
                .output()
                .expect("run bob_pomodoro --help"),
            "bob_pomodoro --help",
        ),
    ];

    cases.push((
        bob_command()
            .arg("pomodoro")
            .arg("--help")
            .env("BOB_CLI_USE_SCRIPT", "1")
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .output()
            .expect("run script fallback bob pomodoro --help"),
        "script fallback bob pomodoro --help",
    ));

    for (output, label) in cases {
        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains("[-s|--show-stale]")
                && help.contains("-s, --show-stale")
                && help.contains("distinguish an old")
                && help.contains("open Pomodoro from no open Pomodoro"),
            "expected show-stale help in {label}:\n{help}"
        );
        assert_text_order(
            &help,
            &[
                "-d, --debug",
                "-h, --help",
                "-s, --show-stale",
                "-v, --verbose",
            ],
        );
        assert_no_long_only_option_lines(label, &help);
        assert_stdout_has_no_ansi(&output);
    }
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
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
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
fn vault_sync_help_is_native_only_and_defaults_to_run() {
    let temp = TempDir::new("bob-cli-vault-sync-native-help");
    let help = bob_command()
        .arg("vault-sync")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run native-only bob vault-sync --help");

    assert_success(&help);
    assert!(
        stdout(&help).contains("bob vault-sync")
            && stdout(&help).contains("run")
            && stdout(&help).contains("status"),
        "expected vault-sync help text:\n{}",
        format_output(&help)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only vault-sync should not extract script assets"
    );
    assert_stdout_has_no_ansi(&help);

    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let implicit = vault_sync_command(&vault, &temp)
        .arg("--dry-run")
        .output()
        .expect("run implicit vault-sync dry-run");
    let explicit = vault_sync_command(&vault, &temp)
        .arg("run")
        .arg("--dry-run")
        .output()
        .expect("run explicit vault-sync dry-run");

    assert_success(&implicit);
    assert_success(&explicit);
    assert!(
        stdout(&implicit).contains("would push after reconcile"),
        "implicit run should parse run options:\n{}",
        format_output(&implicit)
    );
}

#[test]
fn top_level_help_lists_commands_alphabetically_with_examples() {
    let output = bob_command().arg("-h").output().expect("run bob -h");

    assert_success(&output);
    let help = stdout(&output);

    let order = [
        "capture",
        "capture-pomodoro-name",
        "capture-pomodoros",
        "capture-sections",
        "capture-targets",
        "capture-task-id",
        "capture-task-sections",
        "capture-tasks",
        "freshness",
        "highlights",
        "move-done-tasks",
        "nightly",
        "notify",
        "plan",
        "plugins",
        "pomodoro",
        "projects",
        "query",
        "randomize",
        "ready",
        "task-status-hooks",
        "tmux-pomodoro",
        "vault-sync",
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
            && help.contains(
                "bob capture-pomodoro-name -p 38:0b1c2d3e -n 'deep work'"
            )
            && help.contains("bob capture-pomodoros --format json")
            && help.contains("bob capture-sections --route cash --format json")
            && help.contains("bob capture-targets --format json")
            && help.contains(
                "bob capture-task-id -r file -t 3:1f3a9c2b -i report-id"
            )
            && help.contains("bob capture-task-sections -r foo -i bar")
            && help.contains("bob capture-tasks --route cash --format json")
            && help.contains("bob query --source '#project'")
            && help.contains("bob highlights create report.md")
            && help.contains("bob highlights scan --dry-run")
            && help.contains("bob task-status-hooks --dry-run")
            && help.contains("bob move-done-tasks --threshold 10")
            && help.contains("bob nightly")
            && help.contains("bob pomodoro")
            && help.contains("bob vault-sync status --json"),
        "expected an Examples section:\n{help}"
    );
    assert!(
        !help.contains("bulk-git-commit"),
        "top-level help should not advertise retired bulk-git-commit:\n{help}"
    );
    assert!(
        !help.contains("bob dataview"),
        "top-level help should not advertise the old dataview spelling:\n{help}"
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
        !help.contains("mark-next-tasks"),
        "top-level help should hide the compatibility alias:\n{help}"
    );
    assert!(
        !help.contains("task-status-setter"),
        "top-level help should hide the former canonical spelling:\n{help}"
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

fn bob_notify_command() -> Command {
    Command::new(BOB_NOTIFY_BIN)
}

fn bob_pomodoro_command() -> Command {
    Command::new(BOB_POMODORO_BIN)
}

fn tmux_bob_pomodoro_command() -> Command {
    Command::new(TMUX_BOB_POMODORO_BIN)
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
