//! Alias parity, group routing, snapshots, and completion for the
//! command-tree regrouping.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const BOB_NOTIFY_BIN: &str = env!("CARGO_BIN_EXE_bob_notify");
const BOB_POMODORO_BIN: &str = env!("CARGO_BIN_EXE_bob_pomodoro");
const TMUX_BOB_POMODORO_BIN: &str = env!("CARGO_BIN_EXE_tmux_bob_pomodoro");

fn assert_outputs_identical(left: &Output, right: &Output, label: &str) {
    assert_masked_parity(left, right, None, None, label);
}

fn assert_masked_parity(
    left: &Output,
    right: &Output,
    left_root: Option<&Path>,
    right_root: Option<&Path>,
    label: &str,
) {
    let normalize = |output: &Output, root: Option<&Path>| {
        let rewrite = |bytes: &[u8]| {
            let mut text = String::from_utf8_lossy(bytes).into_owned();
            if let Some(root) = root {
                text = text.replace(&root.display().to_string(), "<root>");
            }
            text.lines()
                .map(|line| {
                    if let Some((prefix, _)) =
                        line.split_once("recovery copies:")
                    {
                        format!("{prefix}recovery copies: <path>")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        (
            output.status.code(),
            rewrite(&output.stdout),
            rewrite(&output.stderr),
        )
    };
    let left_norm = normalize(left, left_root);
    let right_norm = normalize(right, right_root);
    assert_eq!(
        left_norm.0,
        right_norm.0,
        "{label}: exit code\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
    assert_eq!(
        left_norm.1,
        right_norm.1,
        "{label}: stdout\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
    assert_eq!(
        left_norm.2,
        right_norm.2,
        "{label}: stderr\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
}

fn run_args(args: &[&str]) -> Output {
    bob_command()
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"))
}

fn run_configured<F>(args: &[&str], configure: F) -> Output
where
    F: FnOnce(&mut Command),
{
    let mut command = bob_command();
    command.args(args);
    configure(&mut command);
    command
        .output()
        .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"))
}

fn complete(words: &[&str]) -> Output {
    let mut command = bob_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--");
    for word in words {
        command.arg(word);
    }
    command.output().expect("run bob __complete")
}

fn complete_values(words: &[&str]) -> Vec<String> {
    let output = complete(words);
    assert_success(&output);
    stdout(&output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn every_alias_matches_canonical_help_and_invalid_option() {
    let pairs: &[(&[&str], &[&str])] = &[
        (&["mark-next-tasks"], &["task", "reconcile"]),
        (&["task-status-setter"], &["task", "reconcile"]),
        (&["task-status-hooks"], &["task", "reconcile"]),
        (&["move-done-tasks"], &["task", "archive"]),
        (&["randomize"], &["task", "reroll"]),
        (&["notify"], &["pomodoro", "notify"]),
        (&["tmux-pomodoro"], &["pomodoro", "tmux"]),
        (&["highlights"], &["ref"]),
        (&["highlights-ref"], &["ref"]),
    ];
    for (old, canonical) in pairs {
        let mut old_help = old.to_vec();
        old_help.push("--help");
        let mut new_help = canonical.to_vec();
        new_help.push("--help");
        assert_outputs_identical(
            &run_args(&old_help),
            &run_args(&new_help),
            &format!("{old:?} --help"),
        );

        let mut old_bad = old.to_vec();
        old_bad.push("--not-a-real-flag");
        let mut new_bad = canonical.to_vec();
        new_bad.push("--not-a-real-flag");
        assert_outputs_identical(
            &run_args(&old_bad),
            &run_args(&new_bad),
            &format!("{old:?} invalid option"),
        );
    }
}

#[test]
fn reconcile_aliases_match_canonical_dry_run_json_and_live_write() {
    for old in ["task-status-hooks", "task-status-setter", "mark-next-tasks"] {
        let temp = TempDir::new(&format!("bob-cli-alias-reconcile-{old}"));
        let old_vault = temp.path().join("old");
        let new_vault = temp.path().join("new");
        write_reconcile_fixture(&old_vault);
        write_reconcile_fixture(&new_vault);

        let dry_old = run_configured(
            &[old, "--dry-run", "-f", "json", "--bob-dir"],
            |command| {
                command
                    .arg(&old_vault)
                    .env("BOB_DAY_FILE", old_vault.join("2026/20260710.md"))
                    .env("XDG_STATE_HOME", old_vault.join("state"));
            },
        );
        let dry_new = run_configured(
            &["task", "reconcile", "--dry-run", "-f", "json", "--bob-dir"],
            |command| {
                command
                    .arg(&new_vault)
                    .env("BOB_DAY_FILE", new_vault.join("2026/20260710.md"))
                    .env("XDG_STATE_HOME", new_vault.join("state"));
            },
        );
        assert_masked_parity(
            &dry_old,
            &dry_new,
            Some(&old_vault),
            Some(&new_vault),
            &format!("{old} dry-run json"),
        );

        let live_old = run_configured(&[old, "--bob-dir"], |command| {
            command
                .arg(&old_vault)
                .env("BOB_DAY_FILE", old_vault.join("2026/20260710.md"))
                .env("XDG_STATE_HOME", old_vault.join("state"));
        });
        let live_new =
            run_configured(&["task", "reconcile", "--bob-dir"], |command| {
                command
                    .arg(&new_vault)
                    .env("BOB_DAY_FILE", new_vault.join("2026/20260710.md"))
                    .env("XDG_STATE_HOME", new_vault.join("state"));
            });
        assert_masked_parity(
            &live_old,
            &live_new,
            Some(&old_vault),
            Some(&new_vault),
            &format!("{old} live write"),
        );
        assert_eq!(
            walk_files(&old_vault),
            walk_files(&new_vault),
            "{old} live write vault files"
        );
    }
}

#[test]
fn move_done_tasks_alias_matches_canonical_on_git_fixture() {
    let temp = TempDir::new("bob-cli-alias-archive");
    let old_root = temp.path().join("old");
    let new_root = temp.path().join("new");
    fs::create_dir_all(&old_root).expect("old root");
    fs::create_dir_all(&new_root).expect("new root");
    let (old_vault, old_bin) = init_archive_git(&old_root);
    let (new_vault, new_bin) = init_archive_git(&new_root);

    let old_out =
        run_configured(&["move-done-tasks", "--threshold=1"], |command| {
            command
                .env("BOB_DIR", &old_vault)
                .env("BOB_NOW", "2026-06-02")
                .env("PATH", path_with_prefix(&old_bin))
                .env("XDG_CACHE_HOME", old_root.join("cache"));
        });
    let new_out =
        run_configured(&["task", "archive", "--threshold=1"], |command| {
            command
                .env("BOB_DIR", &new_vault)
                .env("BOB_NOW", "2026-06-02")
                .env("PATH", path_with_prefix(&new_bin))
                .env("XDG_CACHE_HOME", new_root.join("cache"));
        });
    assert_masked_parity(
        &old_out,
        &new_out,
        Some(&old_root),
        Some(&new_root),
        "move-done-tasks live",
    );
    assert_eq!(walk_files(&old_vault), walk_files(&new_vault));
}

#[test]
fn randomize_alias_matches_canonical_dry_run_seed() {
    let temp = TempDir::new("bob-cli-alias-reroll");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let old_out = run_configured(
        &["randomize", "--dry-run", "--seed", "0x7f3a91c2"],
        |command| {
            command.env("BOB_DIR", &vault).env("BOB_NOW", "2026-09-28");
        },
    );
    let new_out = run_configured(
        &["task", "reroll", "--dry-run", "--seed", "0x7f3a91c2"],
        |command| {
            command.env("BOB_DIR", &vault).env("BOB_NOW", "2026-09-28");
        },
    );
    assert_outputs_identical(&old_out, &new_out, "randomize --dry-run --seed");
}

#[test]
fn tmux_pomodoro_alias_matches_canonical_on_fixture() {
    let temp = TempDir::new("bob-cli-alias-tmux");
    let old_out = run_configured(&["tmux-pomodoro"], |command| {
        command
            .env(
                "BOB_DAY_FILE",
                fixture("pomodoro/day_with_open_pomodoro.md"),
            )
            .env("BOB_NOW", "2026-06-01 09:10:01")
            .env("XDG_CACHE_HOME", temp.path().join("old-cache"));
    });
    let new_out = run_configured(&["pomodoro", "tmux"], |command| {
        command
            .env(
                "BOB_DAY_FILE",
                fixture("pomodoro/day_with_open_pomodoro.md"),
            )
            .env("BOB_NOW", "2026-06-01 09:10:01")
            .env("XDG_CACHE_HOME", temp.path().join("new-cache"));
    });
    assert_outputs_identical(&old_out, &new_out, "tmux-pomodoro fixture");
}

#[test]
fn notify_alias_matches_canonical_help_and_bad_arity() {
    assert_outputs_identical(
        &run_args(&["notify", "--help"]),
        &run_args(&["pomodoro", "notify", "--help"]),
        "notify --help",
    );
    assert_outputs_identical(
        &run_args(&["notify"]),
        &run_args(&["pomodoro", "notify"]),
        "notify bad arity",
    );
}

#[test]
fn pomodoro_status_forms_match_with_and_without_show_stale() {
    let day = fixture("pomodoro/day_with_open_pomodoro.md");
    for extra in [&[][..], &["--show-stale"][..], &["-s"][..]] {
        let mut bob_bare = vec!["pomodoro"];
        bob_bare.extend_from_slice(extra);
        let mut bob_status = vec!["pomodoro", "status"];
        bob_status.extend_from_slice(extra);

        let configure = |command: &mut Command| {
            command
                .env("BOB_DAY_FILE", &day)
                .env("BOB_NOW", "2026-06-01 09:10:01");
        };
        let bare = run_configured(&bob_bare, configure);
        let status = run_configured(&bob_status, configure);
        assert_outputs_identical(
            &bare,
            &status,
            &format!("bob pomodoro vs status {extra:?}"),
        );

        let mut legacy = Command::new(BOB_POMODORO_BIN);
        legacy.args(extra);
        configure(&mut legacy);
        let legacy_out = legacy.output().expect("run bob_pomodoro");
        assert_outputs_identical(
            &bare,
            &legacy_out,
            &format!("bob pomodoro vs bob_pomodoro {extra:?}"),
        );
    }
}

#[test]
fn script_fallback_uses_embedded_assets_for_canonical_and_old_spellings() {
    let temp = TempDir::new("bob-cli-alias-script");
    let pairs: &[(&[&str], &[&str])] = &[
        (&["notify", "--help"], &["pomodoro", "notify", "--help"]),
        (
            &["pomodoro", "status", "--help"],
            &["pomodoro", "status", "--help"],
        ),
        (
            &["tmux-pomodoro", "--help"],
            &["pomodoro", "tmux", "--help"],
        ),
    ];
    for (old, canonical) in pairs {
        let old_out = run_configured(old, |command| {
            command
                .env("BOB_CLI_USE_SCRIPT", "1")
                .env("XDG_CACHE_HOME", temp.path().join("old"));
        });
        let new_out = run_configured(canonical, |command| {
            command
                .env("BOB_CLI_USE_SCRIPT", "1")
                .env("XDG_CACHE_HOME", temp.path().join("new"));
        });
        assert_outputs_identical(
            &old_out,
            &new_out,
            &format!("script fallback {old:?}"),
        );
        assert_success(&old_out);
    }

    for (bin, marker) in [
        (BOB_NOTIFY_BIN, "Notify me when"),
        (BOB_POMODORO_BIN, "Show the current Pomodoro status"),
        (TMUX_BOB_POMODORO_BIN, "Print the current Pomodoro status"),
    ] {
        let output = Command::new(bin)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| panic!("run {bin} --help: {error}"));
        assert_success(&output);
        assert!(
            stdout(&output).contains(marker),
            "expected `{marker}` from {bin}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn group_routing_and_help() {
    let bare_task = run_args(&["task"]);
    assert_eq!(bare_task.status.code(), Some(2));
    assert!(stdout(&bare_task).is_empty());
    assert!(stderr(&bare_task).contains("Usage: bob task"));

    for args in [
        &["task", "-h"][..],
        &["task", "--help"][..],
        &["task", "help", "reroll"][..],
        &["help", "task", "reroll"][..],
    ] {
        let output = run_args(args);
        assert_success(&output);
        assert!(
            stdout(&output).contains("bob task")
                || stdout(&output).contains("Usage: bob task"),
            "expected task help for {args:?}:\n{}",
            format_output(&output)
        );
    }

    let reroll_help = run_args(&["task", "help", "reroll"]);
    let reroll_direct = run_args(&["task", "reroll", "--help"]);
    assert_outputs_identical(&reroll_help, &reroll_direct, "task help reroll");

    let nosuch = run_args(&["task", "nosuch"]);
    assert_eq!(nosuch.status.code(), Some(2));
    assert!(stderr(&nosuch).contains("unrecognized subcommand"));

    let typo = run_args(&["pomodoro", "statsu"]);
    assert_eq!(typo.status.code(), Some(2));
    assert!(
        stderr(&typo).contains("unrecognized subcommand")
            && stderr(&typo).contains("status"),
        "expected a status suggestion:\n{}",
        format_output(&typo)
    );

    let group = run_args(&["pomodoro", "--help"]);
    assert_success(&group);
    assert!(stdout(&group).contains("Usage: bob pomodoro [COMMAND]"));
    assert!(!stdout(&group).contains("[-s|--show-stale]"));
}

#[test]
fn capture_grammar_does_not_swallow_group_words() {
    for text in ["parse", "complete", "tasks", "parse invoices", "api design"] {
        let temp = TempDir::new("bob-cli-capture-guard");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("--dry-run")
            .arg("--format")
            .arg("json")
            .arg("-b")
            .arg(&vault);
        for word in text.split_whitespace() {
            command.arg(word);
        }
        let output = command
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run capture grammar guard");
        assert_eq!(
            output.status.code(),
            Some(0),
            "capture {text:?} should stay a task capture:\n{}",
            format_output(&output)
        );
        let json: serde_json::Value = serde_json::from_str(
            stdout(&output).trim(),
        )
        .unwrap_or_else(|error| {
            panic!("capture {text:?} JSON: {error}\n{}", format_output(&output))
        });
        assert_eq!(json["ok"], true, "capture {text:?}: {json}");
    }
}

#[test]
fn group_help_matches_snapshots_within_80_columns() {
    let task = run_args(&["task", "--help"]);
    let pomodoro = run_args(&["pomodoro", "--help"]);
    assert_success(&task);
    assert_success(&pomodoro);
    assert_eq!(
        stdout(&task),
        include_str!("../fixtures/help/task.txt"),
        "bob task --help snapshot changed"
    );
    assert_eq!(
        stdout(&pomodoro),
        include_str!("../fixtures/help/pomodoro.txt"),
        "bob pomodoro --help snapshot changed"
    );
    for output in [&task, &pomodoro] {
        for line in stdout(output).lines() {
            assert!(
                line.chars().count() <= 80,
                "group help line exceeds 80 columns ({}): {line}",
                line.chars().count()
            );
        }
    }
}

#[test]
fn completion_offers_group_members_status_flags_and_alias_options() {
    let task = complete_values(&["bob", "task", ""]);
    for name in ["archive", "reconcile", "reroll"] {
        assert!(task.contains(&name.to_string()), "missing {name}: {task:?}");
    }

    let pomodoro = complete_values(&["bob", "pomodoro", ""]);
    for name in ["notify", "status", "tmux"] {
        assert!(
            pomodoro.contains(&name.to_string()),
            "missing {name}: {pomodoro:?}"
        );
    }

    let flags = complete_values(&["bob", "pomodoro", "-"]);
    for flag in ["-d", "-s", "-v"] {
        assert!(
            flags.contains(&flag.to_string()),
            "missing {flag}: {flags:?}"
        );
    }

    let level = complete_values(&["bob", "randomize", "--l"]);
    assert!(
        level.iter().any(|value| value.contains("--level")),
        "randomize --l should complete --level: {level:?}"
    );

    let root = complete_values(&["bob", ""]);
    for alias in [
        "mark-next-tasks",
        "task-status-setter",
        "task-status-hooks",
        "move-done-tasks",
        "randomize",
        "notify",
        "tmux-pomodoro",
        "highlights",
        "highlights-ref",
    ] {
        assert!(
            !root.contains(&alias.to_string()),
            "root completion must not offer {alias}"
        );
    }
    assert!(
        root.contains(&"ref".to_string()),
        "root completion must offer ref: {root:?}"
    );
}

fn write_reconcile_fixture(vault: &Path) {
    write_blocked_tasks_settings(vault);
    write_file(
        &vault.join("2026/20260710.md"),
        include_str!("../fixtures/task_status_hooks/2026/20260710.md"),
    );
    write_file(
        &vault.join("dev.md"),
        include_str!("../fixtures/task_status_hooks/dev.md"),
    );
    write_file(
        &vault.join("Projects/Alpha.md"),
        include_str!("../fixtures/task_status_hooks/Projects/Alpha.md"),
    );
}

fn init_archive_git(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let remote = root.join("remote.git");
    let stub_bin = root.join("bin");
    fs::create_dir_all(&vault).expect("create vault");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    git(["init", "-q", "--bare", path_str(&remote)]);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["remote", "add", "origin", path_str(&remote)]);
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );
    write_file(
        &vault.join("obsidian.md"),
        "- [x] done #task\n- [ ] active #task\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    (vault, stub_bin)
}

fn walk_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    walk_files_inner(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_files_inner(
    root: &Path,
    path: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) {
    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child = entry.path();
        if child
            .file_name()
            .is_some_and(|name| name == ".git" || name == "state")
        {
            continue;
        }
        if child.is_dir() {
            walk_files_inner(root, &child, files);
        } else {
            let relative = child
                .strip_prefix(root)
                .expect("child under root")
                .to_string_lossy()
                .into_owned();
            let contents = fs::read(&child).expect("read file");
            files.push((relative, contents));
        }
    }
}
