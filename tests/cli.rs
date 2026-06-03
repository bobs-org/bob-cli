use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

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
fn highlights_ref_help_is_native_only() {
    let temp = TempDir::new("bob-cli-highlights-ref-native-help");
    let output = bob_command()
        .arg("highlights-ref")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob highlights-ref --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("bob highlights-ref"),
        "expected highlights-ref help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only highlights-ref should not extract script assets"
    );
}

#[test]
fn highlights_ref_subcommand_help_works() {
    let cases: &[&[&str]] = &[
        &["highlights-ref", "--help"],
        &["highlights-ref", "scan", "--help"],
        &["highlights-ref", "sync", "--help"],
        &["highlights-ref", "doctor", "--help"],
        &["highlights-ref", "marker", "--help"],
    ];

    for args in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert_success(&output);
        let help = stdout(&output);
        assert!(
            help.contains("Usage: bob highlights-ref"),
            "expected highlights-ref usage for {args:?}:\n{}",
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
        (&["cronjob", "--help"], "usage: bob cronjob"),
        (&["highlights-ref", "--help"], "Usage: bob highlights-ref"),
        (&["move-done-tasks", "--help"], "usage: bob move-done-tasks"),
        (&["notify", "--help"], "Notify me when"),
        (&["pomodoro", "--help"], "usage: bob pomodoro"),
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
fn cronjob_help_exits_before_operational_work() {
    let temp = TempDir::new("bob-cli-cronjob-help");
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
        .arg("cronjob")
        .arg("--help")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob cronjob --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob cronjob"),
        "expected cronjob help:\n{}",
        format_output(&output)
    );
    assert!(
        !log.exists(),
        "bob cronjob --help must not run ob or git:\n{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_help_lists_subcommands_alphabetically() {
    let output = bob_command()
        .arg("highlights-ref")
        .arg("--help")
        .output()
        .expect("run bob highlights-ref --help");

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
        .arg("highlights-ref")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob highlights-ref sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Arguments:") && help.contains("<PDF>"),
        "expected PDF positional argument in Arguments section:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "--bob-dir",
            "--dry-run",
            "--lib-dir",
            "--prefer",
            "--ref-dir",
            "--write-pdf",
        ],
    );
    assert_stdout_has_no_ansi(&output);
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
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights-ref sync");

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
        !contents.contains("ref_type:"),
        "top-level library PDFs should not derive ref_type:\n{contents}"
    );
    assert!(contents.contains("highlights_marker_hash: "), "{contents}");

    let output = bob_command()
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat bob highlights-ref sync");

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
        .arg("highlights-ref")
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
fn highlights_ref_sync_rejects_missing_marker_status_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-status");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-status.pdf");
    let note = vault.join("ref/missing-status.md");
    write_highlights_pdf(
        &pdf,
        "- parent: [[obsidian]]\n- title: Missing Status\n",
    );

    let output = bob_command()
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights-ref sync");

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
fn highlights_ref_sync_rejects_missing_marker_parent_without_note_write() {
    let temp = TempDir::new("bob-cli-highlights-ref-missing-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/missing-parent.pdf");
    let note = vault.join("ref/missing-parent.md");
    write_highlights_pdf(&pdf, "- status: wip\n- title: Missing Parent\n");

    let output = bob_command()
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob highlights-ref sync");

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
fn highlights_ref_sync_rejects_malformed_and_duplicate_marker_lists() {
    let cases = [
        (
            "malformed-marker",
            "status: wip\n- parent: [[obsidian]]\n",
            "invalid marker item on line 1",
        ),
        (
            "duplicate-marker-key",
            "- status: wip\n- parent: [[obsidian]]\n- Status: done\n",
            "duplicate marker key on line 3",
        ),
        (
            "managed-type-marker-key",
            "- status: wip\n- parent: [[obsidian]]\n- type: [[book]]\n",
            "'type' is command-managed",
        ),
        (
            "managed-ref-type-marker-key",
            "- status: wip\n- parent: [[obsidian]]\n- ref_type: books\n",
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
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob highlights-ref sync for {name}: {error}")
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
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
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
            OsString::from("highlights-ref"),
            OsString::from("scan"),
            OsString::from("--dry-run"),
        ],
        vec![
            OsString::from("highlights-ref"),
            OsString::from("sync"),
            OsString::from(path_str(&pdf)),
            OsString::from("--dry-run"),
        ],
        vec![OsString::from("highlights-ref"), OsString::from("doctor")],
        vec![
            OsString::from("highlights-ref"),
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
            "highlights-ref inspection command modified the PDF"
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
        "- status: wip\n- parent: [[obsidian]]\n- title: Systems Performance\n",
    );
    write_highlights_pdf(
        &second_pdf,
        "- status: queued\n- parent: [[obsidian]]\n- title: Rust Book\n",
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
        .arg("highlights-ref")
        .arg("scan")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("dry-run highlights-ref scan");

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
        .arg("highlights-ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("write highlights-ref scan");

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
            && first_contents.contains("> First quote.\n"),
        "{first_contents}"
    );
    assert!(
        second_contents.contains("ref_type: papers\n")
            && second_contents.contains("> Second quote.\n"),
        "{second_contents}"
    );

    let output = bob_command()
        .arg("highlights-ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .output()
        .expect("repeat highlights-ref scan");

    assert_success(&output);
    let repeated = stdout(&output);
    assert!(repeated.contains("notes_unchanged: 2"), "{repeated}");
    assert!(repeated.contains("writes: none"), "{repeated}");
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
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: [[obsidian]]\n");
    write_highlights_pdf(
        &second_pdf,
        "- status: queued\n- parent: [[obsidian]]\n",
    );

    let output = bob_command()
        .arg("highlights-ref")
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
    write_highlights_pdf(&first_pdf, "- status: wip\n- parent: [[obsidian]]\n");
    write_highlights_pdf(
        &second_pdf,
        "- status: queued\n- parent: [[obsidian]]\n",
    );

    let output = bob_command()
        .arg("highlights-ref")
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
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
    assert_success(
        &bob_command()
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights-ref sync"),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial sync"]);
    let dirty_note = fs::read_to_string(&note)
        .expect("read note")
        .replace("## My Notes\n\n", "## My Notes\n\nLocal edit.\n\n");
    write_file(&note, &dirty_note);
    set_pdf_marker_contents(&pdf, "- status: done\n- parent: [[obsidian]]\n");

    let output = bob_command()
        .arg("highlights-ref")
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
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
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
        .arg("highlights-ref")
        .arg("doctor")
        .env("BOB_DIR", &vault)
        .env("OB_COMMAND", &ob_stub)
        .output()
        .expect("run highlights-ref doctor");

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
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
    assert_success(
        &bob_command()
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights-ref sync"),
    );

    set_pdf_marker_contents(&pdf, "- status: done\n- parent: [[obsidian]]\n");
    let output = bob_command()
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync marker edit");

    assert_success(&output);
    let contents = fs::read_to_string(&note).expect("read updated ref note");
    assert!(contents.contains("status: done\n"), "{contents}");
}

#[test]
fn highlights_ref_frontmatter_edit_updates_marker_when_pdf_writes_enabled() {
    let temp = TempDir::new("bob-cli-highlights-ref-frontmatter-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
    assert_success(
        &bob_command()
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights-ref sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: complete");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights-ref")
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
        .arg("highlights-ref")
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
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .arg("--write-pdf")
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync frontmatter edit");

    assert_success(&output);
    let marker = pdf_marker_contents(&pdf);
    assert!(marker.contains("- status: complete\n"), "{marker}");
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
        .arg("highlights-ref")
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
fn highlights_ref_frontmatter_missing_parent_fails_before_pdf_writeback() {
    let temp =
        TempDir::new("bob-cli-highlights-ref-frontmatter-missing-parent");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
    assert_success(
        &bob_command()
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights-ref sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("parent: \"[[obsidian]]\"\n", "");
    write_file(&note, &edited);
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights-ref")
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
fn highlights_ref_conflicting_edits_fail_and_prefer_frontmatter_resolves() {
    let temp = TempDir::new("bob-cli-highlights-ref-conflict");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
    assert_success(
        &bob_command()
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial highlights-ref sync"),
    );

    set_pdf_marker_contents(
        &pdf,
        "- status: marker-side\n- parent: [[obsidian]]\n",
    );
    let frontmatter_side = fs::read_to_string(&note)
        .expect("read ref note")
        .replace("status: wip", "status: frontmatter-side");
    write_file(&note, &frontmatter_side);
    let note_before = fs::read_to_string(&note).expect("read note before");
    let marker_before = pdf_marker_contents(&pdf);

    let output = bob_command()
        .arg("highlights-ref")
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
    assert_eq!(
        fs::read_to_string(&note).expect("read note after conflict"),
        note_before
    );
    assert_eq!(pdf_marker_contents(&pdf), marker_before);

    let output = bob_command()
        .arg("highlights-ref")
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
    assert!(marker.contains("- status: frontmatter-side\n"), "{marker}");
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
        "- status: wip\n- parent: [[obsidian]]\n- title: Systems Performance\n",
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
        .arg("highlights-ref")
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
        contents.contains("PDF: [[lib/books/systems-performance.pdf]]\n"),
        "{contents}"
    );
    assert!(
        contents.contains("## Summary\n\n## My Notes\n\n## Highlights\n"),
        "{contents}"
    );
    assert!(contents.contains("### Page 12\n"), "{contents}");
    assert!(
        contents.contains("> Latency is not throughput.\n"),
        "{contents}"
    );
    assert!(
        contents.contains("> [comment] Compare this with SLO notes.\n"),
        "{contents}"
    );
    assert!(
        contents.contains(
            "> [note] Keep a standalone observation after the marker.\n"
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
        .arg("highlights-ref")
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
fn highlights_ref_comment_edit_keeps_stable_block_id() {
    let temp = TempDir::new("bob-cli-highlights-ref-comment-edit");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
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
            .arg("highlights-ref")
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
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync comment edit");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert_eq!(highlight_block_ids(&updated), initial_ids, "{updated}");
    assert!(updated.contains("[comment] revised comment"), "{updated}");
    assert!(!updated.contains("[comment] first comment"), "{updated}");
}

#[test]
fn highlights_ref_deleted_highlight_is_tombstoned() {
    let temp = TempDir::new("bob-cli-highlights-ref-tombstone");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
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
            .arg("highlights-ref")
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
        .arg("highlights-ref")
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
    assert!(!updated.contains("> Deleted quote.\n"), "{updated}");
}

#[test]
fn highlights_ref_sync_preserves_manual_sections_and_rejects_missing_markers() {
    let temp = TempDir::new("bob-cli-highlights-ref-manual-body");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/example.pdf");
    let sidecar = pdf.with_extension("md");
    let note = vault.join("ref/example.md");
    write_highlights_pdf(&pdf, "- status: wip\n- parent: [[obsidian]]\n");
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
            .arg("highlights-ref")
            .arg("sync")
            .arg(&pdf)
            .env("BOB_DIR", &vault)
            .output()
            .expect("initial manual-body sync"),
    );
    let edited = fs::read_to_string(&note)
        .expect("read note")
        .replacen("---\n", "---\nowner: Bryan\n", 1)
        .replace("## My Notes\n\n", "## My Notes\n\nManual synthesis.\n\n");
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
        .arg("highlights-ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .output()
        .expect("sync with manual content");

    assert_success(&output);
    let updated = fs::read_to_string(&note).expect("read updated note");
    assert!(updated.contains("owner: Bryan\n"), "{updated}");
    assert!(updated.contains("Manual synthesis.\n"), "{updated}");
    assert!(updated.contains("[comment] added later"), "{updated}");

    let unsafe_pdf = vault.join("lib/unsafe.pdf");
    let unsafe_note = vault.join("ref/unsafe.md");
    write_highlights_pdf(
        &unsafe_pdf,
        "- status: wip\n- parent: [[obsidian]]\n",
    );
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
        .arg("highlights-ref")
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
        .arg("--threshold=1")
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
fn move_done_tasks_refuses_dirty_link_repair_files_before_mutation() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-link-repair");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let daily = vault.join("daily.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    write_successful_ob_stub(&stub_bin);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    let source_contents = "\
- [x] done #task ^abc123
- [ ] active #task
";
    write_file(&source, source_contents);
    write_file(&daily, "Reference [[obsidian#^abc123]].\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    let dirty_daily = "Reference [[obsidian#^abc123]].\nlocal edit\n";
    write_file(&daily, dirty_daily);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty link repair candidate");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected dirty link repair candidate failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("refusing: pre-existing changes in candidate files"),
        "expected dirty candidate stdout:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("daily.md"),
        "expected dirty link repair path in stderr:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        source_contents
    );
    assert_eq!(fs::read_to_string(&daily).expect("read daily"), dirty_daily);
    assert!(
        !archive.exists(),
        "archive should not be created when link repair candidate is dirty"
    );
}

#[test]
fn move_done_tasks_refuses_dirty_candidate_files_before_mutation() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-candidate");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    write_successful_ob_stub(&stub_bin);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
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
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty candidate");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected dirty candidate failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("refusing: pre-existing changes in candidate files"),
        "expected dirty candidate stdout:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("obsidian.md"),
        "expected dirty candidate path in stderr:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        dirty_source
    );
    assert!(
        !archive.exists(),
        "archive should not be created when candidate is dirty"
    );
}

#[test]
fn move_done_tasks_refuses_dirty_metadata_only_source_before_mutation() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-metadata");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    write_successful_ob_stub(&stub_bin);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
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
    let dirty_source = "- [ ] active #task\nlocal edit\n";
    write_file(&source, dirty_source);

    let output = bob_command()
        .arg("move-done-tasks")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob move-done-tasks with dirty metadata candidate");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected dirty candidate failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("refusing: pre-existing changes in candidate files"),
        "expected dirty candidate stdout:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("obsidian.md"),
        "expected dirty candidate path in stderr:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        dirty_source
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
}

#[test]
fn move_done_tasks_refuses_dirty_metadata_only_archive_before_mutation() {
    let temp = TempDir::new("bob-cli-move-done-tasks-dirty-archive");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    write_successful_ob_stub(&stub_bin);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
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
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect(
            "run bob move-done-tasks with dirty archive metadata candidate",
        );

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected dirty candidate failure:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output)
            .contains("refusing: pre-existing changes in candidate files"),
        "expected dirty candidate stdout:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("done/obsidian_done.md"),
        "expected dirty archive candidate path in stderr:\n{}",
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
        dirty_archive
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
    // `bob cronjob`).
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

    for command in ["collect-done", "sync"] {
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
        "cronjob",
        "highlights-ref",
        "move-done-tasks",
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
            && help.contains("bob move-done-tasks --threshold 10")
            && help.contains("bob pomodoro"),
        "expected an Examples section:\n{help}"
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
fn cronjob_runs_shared_sync_once_then_wrapped_steps_in_order() {
    let temp = TempDir::new("bob-cli-cronjob-happy");
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
        .arg("cronjob")
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
        .expect("run bob cronjob");

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
        out.contains("bob cronjob")
            && out.contains("Obsidian sync (shared, runs once)")
            && out.contains("move-done-tasks")
            && out.contains("All steps passed"),
        "expected a structured cronjob summary:\n{}",
        format_output(&output)
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "piped cronjob output must not contain ANSI escape codes:\n{out}"
    );
}

#[test]
fn cronjob_failing_shared_sync_aborts_before_wrapped_steps() {
    let temp = TempDir::new("bob-cli-cronjob-sync-fail");
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
        .arg("cronjob")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob cronjob with failing sync");

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
fn cronjob_failed_step_still_runs_later_steps_and_exits_nonzero() {
    let temp = TempDir::new("bob-cli-cronjob-step-fail");
    let stub_bin = temp.path().join("bin");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    // A pre-existing edit to a move-done-tasks candidate makes
    // move-done-tasks refuse (exit 1); the later bulk-git-commit step must
    // still run and commit it.
    let dirty_source = format!("{}local edit\n", done_tasks_source(12));
    write_file(&source, &dirty_source);
    write_successful_ob_stub(&stub_bin);

    let output = bob_command()
        .arg("cronjob")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_COMMIT_MESSAGE", "legacy fallback commit")
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("OB_COMMAND", stub_bin.join("ob"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob cronjob with a failing wrapped step");

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

    // The bulk-git-commit step still ran: the dirty edit was committed and
    // pushed.
    assert_eq!(
        stdout(&git_in(&vault, ["log", "-1", "--format=%s"])).trim(),
        "legacy fallback commit",
        "the later bulk-git-commit step should commit despite the earlier failure"
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

fn write_highlights_pdf(path: &Path, marker_contents: &str) {
    use lopdf::{dictionary, Document, Object, Stream};

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }

    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let marker_id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(24),
            Object::Integer(24),
        ],
        "Contents" => pdf_text_string(marker_contents),
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ],
        "Contents" => content_id,
        "Annots" => vec![Object::Reference(marker_id)],
    });
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
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
