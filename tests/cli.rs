use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const BOB_POMODORO_RUNTIMES_BIN: &str =
    env!("CARGO_BIN_EXE_bob_pomodoro_runtimes");
const STOPWATCH: &str = "\u{23f1}\u{fe0f}";

#[test]
fn cache_extraction_writes_expected_files_and_modes() {
    let temp = TempDir::new("bob-cli-cache");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run bob pomodoro-runtimes --help");

    assert_success(&output);

    let script_dir = single_script_cache_dir(temp.path());
    let executable_assets = [
        "bob_pomodoro",
        "bob_pomodoro_runtimes",
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
fn pomodoro_runtimes_help_runs_without_script_cache() {
    let temp = TempDir::new("bob-cli-native-help");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--help")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native bob pomodoro-runtimes --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("Annotate completed Bob Pomodoro"),
        "expected native help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native help should not extract script assets"
    );
}

#[test]
fn collect_done_help_is_native_only() {
    let temp = TempDir::new("bob-cli-collect-done-native-help");
    let output = bob_command()
        .arg("collect-done")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native-only bob collect-done --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("usage: bob collect-done"),
        "expected collect-done help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native-only collect-done should not extract script assets"
    );
}

#[test]
fn collect_done_runs_sync_before_writing_archive_and_source_files() {
    let temp = TempDir::new("bob-cli-collect-done-sync");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("foo/bar.md");
    let archive = vault.join("done/foo/bar_done.md");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_file(
        &source,
        "\
# Project

- [x] done one #task
  detail
- [-] canceled two #task
- [ ] active #task
",
    );
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
  if grep -q 'done one' "$SOURCE_FILE"; then
    printf 'source-has-task-before-sync\n' >> "$STUB_LOG"
  else
    printf 'source-mutated-before-sync\n' >> "$STUB_LOG"
  fi
  exit 0
fi
exit 64
"#,
    );

    let output = bob_command()
        .arg("collect-done")
        .arg("--threshold")
        .arg("2")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("SOURCE_FILE", &source)
        .env("ARCHIVE_FILE", &archive)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done");

    assert_success(&output);
    let log_contents = fs::read_to_string(&log).expect("read stub log");
    assert!(
        log_contents.contains(&format!("ob sync --path {}", vault.display())),
        "expected ob sync call:\n{log_contents}"
    );
    assert!(
        log_contents.contains("archive-missing-before-sync"),
        "archive should not be written before sync:\n{log_contents}"
    );
    assert!(
        log_contents.contains("source-has-task-before-sync"),
        "source should not be rewritten before sync:\n{log_contents}"
    );

    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
done_tasks: \"[[done/foo/bar_done]]\"
---

# Project

- [ ] active #task
"
    );
    assert_eq!(
        fs::read_to_string(&archive).expect("read archive"),
        "\
---
parent: \"[[foo/bar]]\"
type: \"[[done]]\"
---

- [x] done one #task
  detail
- [-] canceled two #task
"
    );
    let stdout = stdout(&output);
    assert!(
        stdout.contains("sync:")
            && stdout.contains("scan:")
            && stdout.contains("moves:")
            && stdout.contains("summary:"),
        "expected collect-done output sections:\n{}",
        format_output(&output)
    );
}

#[test]
fn collect_done_runs_sync_before_metadata_only_source_writes() {
    let temp = TempDir::new("bob-cli-collect-done-metadata-sync");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
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
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "sync" ]; then
  if grep -q 'done_tasks' "$SOURCE_FILE"; then
    printf 'source-mutated-before-sync\n' >> "$STUB_LOG"
  else
    printf 'source-needs-metadata-before-sync\n' >> "$STUB_LOG"
  fi
  exit 0
fi
exit 64
"#,
    );

    let output = bob_command()
        .arg("collect-done")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("SOURCE_FILE", &source)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done metadata-only");

    assert_success(&output);
    let log_contents = fs::read_to_string(&log).expect("read stub log");
    assert!(
        log_contents.contains(&format!("ob sync --path {}", vault.display())),
        "expected ob sync call:\n{log_contents}"
    );
    assert!(
        log_contents.contains("source-needs-metadata-before-sync"),
        "source should not be rewritten before sync:\n{log_contents}"
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
    assert!(
        stdout(&output).contains("source done_tasks updates: 1")
            && stdout(&output).contains("moved task blocks: 0"),
        "expected metadata-only summary:\n{}",
        format_output(&output)
    );
}

#[test]
fn collect_done_skips_missing_ob_command_and_writes_vault_changes() {
    let temp = TempDir::new("bob-cli-collect-done-no-ob");
    let vault = temp.path().join("vault");
    let path_without_ob = temp.path().join("empty-bin");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&path_without_ob).expect("create empty PATH dir");
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );

    let output = bob_command()
        .arg("collect-done")
        .arg("--threshold=1")
        .env_remove("OB_COMMAND")
        .env("BOB_DIR", &vault)
        .env("PATH", &path_without_ob)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done without ob");

    assert_success(&output);
    assert!(
        stdout(&output).contains("skipped: ob command not found"),
        "expected explicit missing-ob output:\n{}",
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
            && archive_contents.contains("type: \"[[done]]\""),
        "expected archive metadata:\n{archive_contents}"
    );
}

#[test]
fn collect_done_failing_sync_stops_before_mutation() {
    let temp = TempDir::new("bob-cli-collect-done-sync-fail");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_file(
        &source,
        "\
- [x] done #task
- [ ] active #task
",
    );
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
        .arg("collect-done")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done with failing sync");

    assert_eq!(
        output.status.code(),
        Some(42),
        "expected sync failure exit code:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("ob sync failed with exit code 42"),
        "expected sync failure message:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
- [x] done #task
- [ ] active #task
"
    );
    assert!(
        !archive.exists(),
        "archive should not be created after failed sync"
    );
}

#[test]
fn collect_done_commits_and_pushes_collection_changes_only() {
    let temp = TempDir::new("bob-cli-collect-done-git");
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
        .arg("collect-done")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("git:")
            && output_text.contains("committed: bob collect-done 2026-06-02")
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
        show.starts_with("bob collect-done 2026-06-02\n"),
        "expected collect-done commit subject:\n{show}"
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
fn collect_done_commits_metadata_only_source_updates() {
    let temp = TempDir::new("bob-cli-collect-done-git-metadata");
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
        .arg("collect-done")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("source done_tasks updates: 1")
            && output_text.contains("committed: bob collect-done 2026-06-02")
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
        show.starts_with("bob collect-done 2026-06-02\n"),
        "expected collect-done commit subject:\n{show}"
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
fn collect_done_commits_metadata_only_archive_repairs() {
    let temp = TempDir::new("bob-cli-collect-done-git-archive-metadata");
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
        .arg("collect-done")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done archive metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("archive metadata repairs: 1")
            && output_text.contains("committed: bob collect-done 2026-06-02")
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
        show.starts_with("bob collect-done 2026-06-02\n"),
        "expected collect-done commit subject:\n{show}"
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
fn collect_done_warns_and_skips_git_for_non_repo_vault() {
    let temp = TempDir::new("bob-cli-collect-done-non-repo");
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
        .arg("collect-done")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done outside git repo");

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
        "collect-done must not initialize git"
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
fn collect_done_refuses_dirty_candidate_files_before_mutation() {
    let temp = TempDir::new("bob-cli-collect-done-dirty-candidate");
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
        .arg("collect-done")
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done with dirty candidate");

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
fn collect_done_refuses_dirty_metadata_only_source_before_mutation() {
    let temp = TempDir::new("bob-cli-collect-done-dirty-metadata");
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
        .arg("collect-done")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done with dirty metadata candidate");

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
fn collect_done_refuses_dirty_metadata_only_archive_before_mutation() {
    let temp = TempDir::new("bob-cli-collect-done-dirty-archive");
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
        .arg("collect-done")
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob collect-done with dirty archive metadata candidate");

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
fn pass_through_arguments_and_exit_statuses_reach_runtimes_command() {
    let temp = TempDir::new("bob-cli-runtimes-check");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("needs_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/needs_runtime_suffixes.md"),
        &note,
    )
    .expect("copy runtime fixture");

    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--check")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes --check");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected child status 1, got:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("would update"),
        "expected runtimes command to receive --check:\n{}",
        format_output(&output)
    );
}

#[test]
fn pomodoro_runtimes_updates_notes_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-runtimes-update");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("legacy_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/legacy_runtime_suffixes.md"),
        &note,
    )
    .expect("copy legacy runtime fixture");

    let first = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes");

    assert_success(&first);
    assert!(
        stdout(&first).contains("updated:"),
        "expected first run to update note:\n{}",
        format_output(&first)
    );

    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains("\n## Pomodoros\n"));
    assert!(contents.contains(&format!(
        "- [x] (09:00-09:25 {STOPWATCH} 25m) Replace legacy runtime suffix"
    )));
    assert!(contents.contains(&format!(
        "- [x] (09:30-09:55 {STOPWATCH} 25m) Recalculate in-parentheses runtime"
    )));
    assert!(contents.contains(&format!(
        "- [x] (10:00-10:10 {STOPWATCH} 10m) Remove trailing stopwatch suffix"
    )));
    assert!(
        contents.contains(&format!("- [x] (10:15-10:40 {STOPWATCH} 25m)\n"))
    );
    assert!(!contents.contains("[runtime::"));
    assert!(!contents.contains(&format!("## Pomodoros {STOPWATCH}")));
    assert!(!contents.contains(&format!("(09:30-09:55 {STOPWATCH} 10m)")));
    assert!(!contents.contains(&format!("(10:15-10:40 {STOPWATCH} 5m)")));
    assert!(!contents
        .contains(&format!("Remove trailing stopwatch suffix {STOPWATCH} 5m")));

    let second = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("rerun bob pomodoro-runtimes");

    assert_success(&second);
    assert!(
        stdout(&second).is_empty(),
        "second run should be idempotent:\n{}",
        format_output(&second)
    );
}

#[test]
fn pomodoro_runtimes_skips_missing_ob_command_and_updates_note() {
    let temp = TempDir::new("bob-cli-runtimes-no-ob");
    let path_without_ob = temp.path().join("empty-bin");
    fs::create_dir_all(&path_without_ob).expect("create empty PATH dir");

    let note = temp.path().join("needs_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/needs_runtime_suffixes.md"),
        &note,
    )
    .expect("copy runtime fixture");

    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env_remove("BOB_CLI_USE_SCRIPT")
        .env_remove("OB_COMMAND")
        .env("PATH", &path_without_ob)
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes without ob");

    assert_success(&output);
    assert!(
        stdout(&output).contains("updated:"),
        "expected missing-ob run to update note:\n{}",
        format_output(&output)
    );
    assert!(
        !stderr(&output).contains("ob command not found"),
        "missing ob should be skipped silently:\n{}",
        format_output(&output)
    );

    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains("\n## Pomodoros\n"));
    assert!(contents.contains(&format!(
        "- [x] (09:00-09:25 {STOPWATCH} 25m) Import Bob scripts"
    )));
}

#[test]
fn script_pomodoro_runtimes_skips_missing_ob_command_and_updates_note() {
    let temp = TempDir::new("bob-cli-script-runtimes-no-ob");
    let note = temp.path().join("legacy_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/legacy_runtime_suffixes.md"),
        &note,
    )
    .expect("copy legacy runtime fixture");

    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("OB_COMMAND", temp.path().join("missing-ob"))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro-runtimes without ob");

    assert_success(&output);
    assert!(
        stdout(&output).contains("updated:"),
        "expected script missing-ob run to update note:\n{}",
        format_output(&output)
    );
    assert!(
        !stderr(&output).contains("ob command not found"),
        "missing script ob should be skipped silently:\n{}",
        format_output(&output)
    );

    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains("\n## Pomodoros\n"));
    assert!(
        contents.contains(&format!("- [x] (10:15-10:40 {STOPWATCH} 25m)\n"))
    );
    assert!(!contents.contains("[runtime::"));
    assert!(!contents.contains(&format!("(10:15-10:40 {STOPWATCH} 5m)")));
}

#[test]
fn legacy_pomodoro_runtimes_shim_uses_native_implementation() {
    let temp = TempDir::new("bob-cli-runtimes-shim");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("needs_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/needs_runtime_suffixes.md"),
        &note,
    )
    .expect("copy runtime fixture");

    let output = Command::new(BOB_POMODORO_RUNTIMES_BIN)
        .arg("--check")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run legacy bob_pomodoro_runtimes shim");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected child status 1, got:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("would update"),
        "expected shim to run native check path:\n{}",
        format_output(&output)
    );
}

#[test]
fn pomodoro_runtimes_reports_missing_note_without_touching_real_vault() {
    let temp = TempDir::new("bob-cli-runtimes-missing");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let missing = temp.path().join("missing-day.md");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&missing)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes with missing note");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected missing note status 2:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("note not found"),
        "expected missing note error:\n{}",
        format_output(&output)
    );
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
fn bob_sync_uses_stubbed_ob_and_git_commands() {
    let temp = TempDir::new("bob-cli-sync");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    fs::create_dir_all(&home).expect("create home");

    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$STUB_LOG"
case "$1" in
  sync|sync-status) exit 0 ;;
esac
exit 64
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
        .arg("sync")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob sync");

    assert_success(&output);
    let log_contents = fs::read_to_string(&log).expect("read stub command log");
    assert!(
        log_contents.contains(&format!("ob sync --path {}", vault.display()))
    );
    assert!(log_contents
        .contains(&format!("ob sync-status --path {}", vault.display())));
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
        "no-change sync should not commit"
    );
}

#[test]
fn top_level_help_lists_commands_alphabetically_with_examples() {
    let output = bob_command().arg("-h").output().expect("run bob -h");

    assert_success(&output);
    let help = stdout(&output);

    let order = [
        "collect-done",
        "notify",
        "pomodoro",
        "pomodoro-runtimes",
        "sync",
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
            && help.contains("bob collect-done --threshold 10")
            && help.contains("bob pomodoro-runtimes --check"),
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

fn bob_command() -> Command {
    Command::new(BOB_BIN)
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
