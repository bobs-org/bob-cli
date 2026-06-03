use std::{
    ffi::{OsStr, OsString},
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
const BOB_SYNC_BIN: &str = env!("CARGO_BIN_EXE_bob_sync");

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
fn highlights_ref_phase_one_commands_do_not_modify_vault_files() {
    let temp = TempDir::new("bob-cli-highlights-ref-no-write");
    let vault = temp.path().join("vault");
    let lib = vault.join("lib");
    let ref_dir = vault.join("ref");
    let pdf = lib.join("example.pdf");
    let note = ref_dir.join("example.md");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::create_dir_all(&ref_dir).expect("create ref dir");
    write_file(&pdf, "%PDF-1.4\n%%EOF\n");
    write_file(
        &note,
        "---\nparent: \"[[obsidian]]\"\n---\n\nManual note.\n",
    );

    let before = snapshot_files(&vault);
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
            snapshot_files(&vault),
            before,
            "highlights-ref Phase 1 command modified vault files"
        );
    }
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

fn bob_sync_command() -> Command {
    Command::new(BOB_SYNC_BIN)
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

fn snapshot_files(root: &Path) -> Vec<(PathBuf, String)> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Vec<(PathBuf, String)>,
    ) {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .unwrap_or_else(|error| {
                panic!("read directory {}: {error}", directory.display())
            })
            .map(|entry| entry.expect("read directory entry").path())
            .collect();
        entries.sort();

        for path in entries {
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or_else(|error| {
                        panic!(
                            "strip root {} from {}: {error}",
                            root.display(),
                            path.display()
                        )
                    })
                    .to_path_buf();
                let contents =
                    fs::read_to_string(&path).unwrap_or_else(|error| {
                        panic!("read file {}: {error}", path.display())
                    });
                files.push((relative, contents));
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files
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
