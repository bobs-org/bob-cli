//! Move-done tests.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

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
            && output_text
                .contains("warning: vault is not a git worktree; skipping commit and push"),
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
