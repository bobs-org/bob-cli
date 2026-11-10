//! Move-done tests.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

#[cfg(unix)]
#[test]
fn task_archive_accepts_a_symlinked_vault_root() {
    let temp = TempDir::new("bob-cli-move-done-vault-alias");
    let stub_bin = temp.path().join("bin");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    let alias = temp.path().join("vault-alias");
    std::os::unix::fs::symlink(&vault, &alias).expect("create vault alias");
    write_file(
        &vault.join("work.md"),
        &area_note("- [x] finished #task ^gone\n- [ ] active #task\n"),
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .args(["task", "archive", "-t", "1", "-f", "json"])
        .env("BOB_DIR", &alias)
        .env("BOB_NOW", "2026-06-02")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .output()
        .expect("run archive through a vault alias");

    assert_success(&output);
    let json = stdout(&output);
    assert!(
        json.contains("\"outcome\":\"applied\"")
            || json.contains("\"outcome\": \"applied\""),
        "archive JSON reports an applied write:\n{}",
        format_output(&output)
    );
    let archive = fs::read_to_string(vault.join("done/work_done.md"))
        .expect("archive created under real vault");
    assert!(archive
        .contains("- [x] finished #task [completion:: 2000-01-01] ^gone"));
    assert!(fs::read_to_string(vault.join("work.md"))
        .unwrap()
        .contains("- [ ] active #task"));
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
        &area_note(
            "\
- [x] done #task
- [ ] active #task
",
        ),
    );
    write_file(&unrelated, "- [ ] unrelated #task\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    write_file(&unrelated, "- [ ] unrelated #task\nlocal edit\n");

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("git:")
            && output_text.contains("committed: bob task archive 2026-06-02")
            && output_text.contains("pushed"),
        "expected git section with commit and push:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
type: [[area]]
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
        show.starts_with("bob task archive 2026-06-02\n"),
        "expected bob task archive commit subject:\n{show}"
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
        &area_note(
            "\
- [x] done #task ^abc123
- [ ] active #task
",
        ),
    );
    write_file(
        &daily,
        "Links [[obsidian#^abc123|alias]] and ![[obsidian#^abc123]].\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with link repair");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("Obsidian links repaired: 2")
            && output_text.contains("link-repair files updated: 1")
            && output_text.contains("committed: bob task archive 2026-06-02")
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
            .contains("- [x] done #task [completion:: 2000-01-01] ^abc123"),
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
- [x] done #task [completion:: 2000-01-01] ^abc123
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
        .args(["task", "archive"])
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with block id collision");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("moved block id renames: 1")
            && output_text.contains("Obsidian links repaired: 1")
            && output_text.contains("committed: bob task archive 2026-06-02")
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
            && archive_contents.contains(
                "- [x] done #task [completion:: 2000-01-01] ^abc123-1\n"
            ),
        "expected existing and renamed moved block ids:\n{archive_contents}"
    );

    let show = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=%s", "HEAD"],
    ));
    assert!(
        show.starts_with("bob task archive 2026-06-02\n")
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
        .args(["task", "archive"])
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("source done_tasks updates: 1")
            && output_text.contains("committed: bob task archive 2026-06-02")
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
        show.starts_with("bob task archive 2026-06-02\n"),
        "expected bob task archive commit subject:\n{show}"
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
        .args(["task", "archive"])
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive archive metadata-only in git repo");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("archive metadata repairs: 1")
            && output_text.contains("committed: bob task archive 2026-06-02")
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
        show.starts_with("bob task archive 2026-06-02\n"),
        "expected bob task archive commit subject:\n{show}"
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
        &area_note(
            "\
- [x] done #task
- [ ] active #task
",
        ),
    );

    let output = bob_command()
        .args(["task", "archive"])
        .arg("-t1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive outside git repo");

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
        "bob task archive must not initialize git"
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
type: [[area]]
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
        &area_note(
            "\
- [-] canceled one #task [cancelled:: 2000-01-01]
- [-] canceled two #task [cancelled:: 2000-01-01]
- [ ] active #task
",
        ),
    );

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold=2")
        .env("BOB_DIR", &vault)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with canceled tasks");

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(
        output_text.contains("files selected for archive: 1")
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
type: [[area]]
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

## 2000-01

- [-] canceled one #task [cancelled:: 2000-01-01]
- [-] canceled two #task [cancelled:: 2000-01-01]
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
    let source_contents = area_note(
        "\
- [x] done #task ^abc123
- [ ] active #task
",
    );
    write_file(&source, &source_contents);
    write_file(&daily, "Reference [[obsidian#^abc123]].\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_daily = "Reference [[obsidian#^abc123]].\nlocal edit\n";
    write_file(&daily, dirty_daily);

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with dirty link repair candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("Obsidian links repaired: 1")
            && stdout(&output)
                .contains("committed: bob task archive 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty link repair candidate success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
type: [[area]]
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
            .contains("- [x] done #task [completion:: 2000-01-01] ^abc123"),
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
        &area_note(
            "\
- [x] done #task
- [ ] active #task
",
        ),
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    let dirty_source = area_note(
        "\
- [x] done #task
- [ ] active #task
local edit
",
    );
    write_file(&source, &dirty_source);

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold=1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with dirty candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("task blocks: 1")
            && stdout(&output)
                .contains("committed: bob task archive 2026-06-02")
            && stdout(&output).contains("pushed"),
        "expected dirty candidate success:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&source).expect("read source"),
        "\
---
type: [[area]]
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
        .args(["task", "archive"])
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with dirty metadata candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("source done_tasks updates: 1")
            && stdout(&output)
                .contains("committed: bob task archive 2026-06-02")
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
        .args(["task", "archive"])
        .arg("--threshold=10")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with dirty archive metadata candidate");

    assert_success(&output);
    assert!(
        stdout(&output).contains("archive metadata repairs: 1")
            && stdout(&output)
                .contains("committed: bob task archive 2026-06-02")
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
fn move_done_tasks_rewrites_stayed_pathless_links_in_archive() {
    // End-to-end for `archive_stayed_block_ids`: a pathless `[[#^stayed]]`
    // inside an archived block whose target stays behind gains the source
    // note path, so the archived Depends-On line keeps pointing at the
    // target that did not move.
    let temp = TempDir::new("bob-cli-move-done-tasks-stayed-links");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let source = vault.join("work.md");
    let archive = vault.join("done/work_done.md");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &source,
        &area_note(
            "- [x] #task Archived ^moved\n  - ⛓️ **DEPENDS ON:** [[#^stayed]]\n- [ ] #task Stayed ^stayed\n",
        ),
    );

    let output = bob_command()
        .args(["task", "archive"])
        .arg("-t1")
        .env("BOB_DIR", &vault)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with stayed pathless link");

    assert_success(&output);
    let archive_contents = fs::read_to_string(&archive).expect("read archive");
    assert!(
        archive_contents.contains("[[work#^stayed]]"),
        "expected stayed pathless link to gain the source path:\n{archive_contents}"
    );
    assert!(
        !archive_contents.contains("[[#^stayed]]"),
        "expected no bare stayed link to remain:\n{archive_contents}"
    );
    let source_contents = fs::read_to_string(&source).expect("read source");
    assert!(
        source_contents.contains("- [ ] #task Stayed ^stayed"),
        "expected stayed target to remain in the source:\n{source_contents}"
    );
}

#[test]
fn move_done_tasks_keeps_a_drafted_period_review_drafted() {
    // Archiving a task rewrites a period Review's link to `done/`. The
    // rewrite is bob's, not Bryan's, so the Review must stay `drafted`
    // (and redraftable without -F) instead of turning `edited`.
    let temp = TempDir::new("bob-cli-move-done-tasks-period-review");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_successful_ob_stub(&stub_bin);
    write_file(
        &vault.join("work.md"),
        &area_note(
            "- [x] #task Ship decay ✅ 2026-11-04 ^decay\n- [ ] #task Active\n",
        ),
    );
    let period = |args: &[&str]| {
        bob_command()
            .arg("periods")
            .args(args)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", "2026-11-10")
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .output()
            .expect("run bob periods")
    };
    assert_success(&period(&["ensure", "-d", "2026-11-04", "-q"]));
    let draft = temp.path().join("draft.md");
    write_file(
        &draft,
        "**Headline:** Shipped decay.\n\n**Shipped:**\n\n- [[work#^decay|Decay]]\n",
    );
    assert_success(&period(&[
        "draft",
        "2026-W45",
        "-i",
        draft.to_str().expect("utf-8 path"),
    ]));

    let output = bob_command()
        .args(["task", "archive"])
        .arg("-t1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-11-20")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive with a drafted period review");
    assert_success(&output);

    let note = fs::read_to_string(vault.join("2026/2026-W45.md"))
        .expect("read period note");
    assert!(
        note.contains("[[done/work_done#^decay|Decay]]"),
        "expected the Review link to move to done/:\n{note}"
    );
    let list = stdout(&period(&["list", "-f", "json", "-k", "week"]));
    let rows: serde_json::Value =
        serde_json::from_str(&list).expect("list json");
    let w45 = rows["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "2026-W45")
        .expect("W45 row");
    assert_eq!(w45["review_state"], "drafted", "{note}");
}

fn area_note(body: &str) -> String {
    format!(
        "---\ntype: [[area]]\n---\n\n{}",
        with_archive_test_close_dates(body)
    )
}

fn with_archive_test_close_dates(contents: &str) -> String {
    let mut dated = String::with_capacity(contents.len() + 32);
    for segment in contents.split_inclusive('\n') {
        let (line, ending) = if let Some(line) = segment.strip_suffix("\r\n") {
            (line, "\r\n")
        } else if let Some(line) = segment.strip_suffix('\n') {
            (line, "\n")
        } else {
            (segment, "")
        };
        let task_line = line.trim_start();
        let status = if task_line.starts_with("- [-]") {
            Some("cancelled")
        } else if task_line.starts_with("- [x]")
            || task_line.starts_with("- [X]")
        {
            Some("completion")
        } else {
            None
        };
        let has_date =
            ["[completion::", "[cancelled::", "✅ ", "✔ ", "❌ ", "✖ "]
                .iter()
                .any(|field| line.contains(field));
        if let Some(status) =
            status.filter(|_| task_line.contains("#task") && !has_date)
        {
            let trimmed = line.trim_end();
            let trailing = &line[trimmed.len()..];
            let mut updated = trimmed.to_string();
            let field = format!(" [{status}:: 2000-01-01]");
            if let Some(anchor) = updated.rfind(" ^") {
                updated.insert_str(anchor, &field);
            } else {
                updated.push_str(&field);
            }
            dated.push_str(&updated);
            dated.push_str(trailing);
        } else {
            dated.push_str(line);
        }
        dated.push_str(ending);
    }
    dated
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

fn archive_command(vault: &Path, temp: &TempDir) -> Command {
    let mut command = bob_command();
    command
        .args(["task", "archive"])
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-06-02")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn archive_json(output: &Output) -> serde_json::Value {
    let text = stdout(output);
    assert_eq!(
        text.trim_end().lines().count(),
        1,
        "JSON mode prints exactly one document on stdout:\n{}",
        format_output(output)
    );
    serde_json::from_str(text.trim()).unwrap_or_else(|error| {
        panic!("stdout is JSON ({error}):\n{}", format_output(output))
    })
}

fn init_committed_git_vault(temp: &TempDir) -> (PathBuf, PathBuf) {
    let (vault, remote) = init_git_vault_with_remote(temp);
    write_file(
        &vault.join("obsidian.md"),
        &area_note(
            "\
- [x] done #task ^d1
- [ ] active #task
",
        ),
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);
    (vault, remote)
}

#[test]
fn archive_dry_run_has_no_side_effects_with_a_held_lock_and_sentinel_git() {
    let temp = TempDir::new("bob-cli-archive-dry-run-held");
    let (vault, _remote) = init_committed_git_vault(&temp);
    let stub_bin = temp.path().join("bin");
    let marker = temp.path().join("git-was-invoked");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("git"),
        &format!(
            "#!/bin/sh\necho invoked >> {}\nexit 99\n",
            shell_single_quote(path_str(&marker))
        ),
    );
    // A competing process holds the maintenance lock for the whole run.
    let lock_path = temp.path().join("bob_sync.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    fs2::FileExt::try_lock_exclusive(&lock).expect("hold lock");
    let before_source =
        fs::read(vault.join("obsidian.md")).expect("read source");

    let output = archive_command(&vault, &temp)
        .args(["--dry-run", "--format", "json", "-t", "1"])
        .env("PATH", path_with_prefix(&stub_bin))
        .output()
        .expect("run bob task archive --dry-run");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["exit"]["outcome"], "preview");
    assert_eq!(report["exit"]["code"], 0);
    assert_eq!(report["git"]["outcome"], "not_run");
    assert_eq!(report["moves"][0]["source"], "obsidian.md");
    assert_eq!(report["moves"][0]["archive"], "done/obsidian_done.md");
    assert_eq!(report["moves"][0]["block_ids"], serde_json::json!(["d1"]));
    assert!(!marker.exists(), "dry run must never invoke git");
    assert!(!vault.join("done").exists(), "no archive directory created");
    assert_eq!(fs::read(vault.join("obsidian.md")).unwrap(), before_source);
    assert!(
        !temp.path().join("state").exists(),
        "no recovery state created"
    );
    let status = stdout(&git_in(&vault, ["status", "--short"]));
    assert_eq!(status, "", "vault stays clean:\n{status}");
}

#[test]
fn archive_dry_run_never_creates_the_lock_file() {
    let temp = TempDir::new("bob-cli-archive-dry-run-no-lock");
    let (vault, _remote) = init_committed_git_vault(&temp);
    let missing_lock = temp.path().join("no-such-dir/bob_sync.lock");

    let output = archive_command(&vault, &temp)
        .arg("--dry-run")
        .arg("-t1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", &missing_lock)
        .output()
        .expect("run bob task archive --dry-run");

    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("dry run")
            && text.contains("obsidian.md -> done/obsidian_done.md")
            && text.contains("task blocks to move: 1")
            && text.contains("git:\n  not run: dry run"),
        "expected a human preview:\n{}",
        format_output(&output)
    );
    assert!(!temp.path().join("no-such-dir").exists());
    assert!(!vault.join("done").exists());
}

#[test]
fn archive_json_reports_a_noop_run() {
    let temp = TempDir::new("bob-cli-archive-json-noop");
    let (vault, _remote) = init_git_vault_with_remote(&temp);
    write_file(
        &vault.join("obsidian.md"),
        &area_note("- [ ] active #task\n"),
    );

    let output = archive_command(&vault, &temp)
        .args(["-f", "json"])
        .output()
        .expect("run bob task archive -f json");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["moves"], serde_json::json!([]));
    assert_eq!(report["holds"], serde_json::json!([]));
    assert_eq!(report["grandfathered"], serde_json::json!([]));
    assert_eq!(report["link_repairs"]["files"], serde_json::json!([]));
    assert_eq!(report["link_repairs"]["link_count"], 0);
    assert_eq!(report["exit"]["outcome"], "noop");
    assert_eq!(report["exit"]["code"], 0);
    assert_eq!(
        report["exit"]["recovery_directory"],
        serde_json::Value::Null
    );
    assert_eq!(report["git"]["outcome"], "skipped");
    assert_eq!(report["git"]["skip_reason"], "no_changes");
    assert!(!vault.join("done").exists());
}

#[test]
fn archive_json_reports_the_applied_commit_and_push() {
    let temp = TempDir::new("bob-cli-archive-json-applied");
    let (vault, remote) = init_committed_git_vault(&temp);
    write_file(
        &vault.join("daily.md"),
        "Link [[obsidian#^d1]] stays valid.\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add daily"]);
    git_in(&vault, ["push", "-q"]);

    let output = archive_command(&vault, &temp)
        .args(["--format=json", "--threshold=1"])
        .output()
        .expect("run bob task archive");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["exit"]["outcome"], "applied");
    assert_eq!(report["exit"]["code"], 0);
    assert_eq!(
        report["exit"]["applied_paths"],
        serde_json::json!(["done/obsidian_done.md", "obsidian.md", "daily.md"])
    );
    assert_eq!(report["exit"]["deferred_paths"], serde_json::json!([]));
    assert!(report["exit"]["recovery_directory"].is_string());
    assert_eq!(report["git"]["outcome"], "pushed");
    assert_eq!(report["git"]["committed"], true);
    assert_eq!(report["git"]["pushed"], true);
    assert_eq!(
        report["git"]["commit_message"],
        "bob task archive 2026-06-02"
    );
    assert_eq!(
        report["git"]["paths"],
        serde_json::json!(["daily.md", "done/obsidian_done.md", "obsidian.md"])
    );
    assert_eq!(
        report["link_repairs"],
        serde_json::json!({
            "files": ["daily.md"],
            "link_count": 1,
            "dependency_metadata_count": 0
        })
    );
    assert_eq!(report["moves"][0]["task_count"], 1);
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");
}

#[test]
fn archive_rename_commits_the_removed_old_path_and_second_run_is_noop() {
    let temp = TempDir::new("bob-cli-archive-json-rename");
    let (vault, remote) = init_git_vault_with_remote(&temp);
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[project]]\ndone_tasks: \"[[done/work_old_done]]\"\n---\n\n- [x] shipped #task [completion:: 2000-01-01] ^ship\n- [ ] active #task\n",
    );
    write_file(
        &vault.join("done/work_old_done.md"),
        "---\nparent: \"[[work]]\"\ntype: \"[[done]]\"\n---\n\n- [x] older #task ^old\n",
    );
    write_file(
        &vault.join("notes.md"),
        "See [[done/work_old_done#^old]].\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "HEAD"]);

    let output = archive_command(&vault, &temp)
        .args(["-f", "json", "-t", "1"])
        .output()
        .expect("run bob task archive with a rename");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["exit"]["outcome"], "applied");
    assert_eq!(
        report["renames"],
        serde_json::json!([{
            "source": "work.md",
            "from": "done/work_old_done.md",
            "to": "done/work_done.md",
        }])
    );
    assert_eq!(report["git"]["outcome"], "pushed");
    assert_eq!(
        report["git"]["paths"],
        serde_json::json!([
            "done/work_done.md",
            "done/work_old_done.md",
            "notes.md",
            "work.md"
        ])
    );
    assert!(!vault.join("done/work_old_done.md").exists());
    let archive = fs::read_to_string(vault.join("done/work_done.md")).unwrap();
    assert!(archive.contains("older #task ^old"), "{archive}");
    assert!(
        archive.contains("shipped #task [completion:: 2000-01-01] ^ship"),
        "{archive}"
    );
    let notes = fs::read_to_string(vault.join("notes.md")).unwrap();
    assert_eq!(notes, "See [[done/work_done#^old]].\n");
    let status = stdout(&git_in(&vault, ["status", "--porcelain"]));
    assert_eq!(status, "", "the removal is committed, not left unstaged");
    let removed = stdout(&git_in(
        &vault,
        ["ls-tree", "-r", "--name-only", "HEAD", "--", "done"],
    ));
    assert_eq!(removed.trim(), "done/work_done.md");
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(remote_head, local_head, "push should update bare remote");

    let rerun = archive_command(&vault, &temp)
        .args(["-f", "json", "-t", "1"])
        .output()
        .expect("rerun bob task archive");
    assert_success(&rerun);
    let rerun = archive_json(&rerun);
    assert_eq!(rerun["exit"]["outcome"], "noop");
    assert_eq!(rerun["renames"], serde_json::json!([]));
}

#[test]
fn archive_json_reports_skipped_git_for_a_non_worktree_vault() {
    let temp = TempDir::new("bob-cli-archive-json-non-worktree");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("obsidian.md"),
        &area_note("- [x] done #task\n- [ ] active #task\n"),
    );

    let output = archive_command(&vault, &temp)
        .args(["-f", "json", "-t", "1"])
        .output()
        .expect("run bob task archive in a non-worktree");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["exit"]["outcome"], "applied");
    assert_eq!(report["git"]["outcome"], "skipped");
    assert_eq!(report["git"]["skip_reason"], "not_worktree");
    assert!(report["git"]["warning"]
        .as_str()
        .unwrap()
        .contains("not a git worktree"));
    assert!(vault.join("done/obsidian_done.md").exists());
}

#[test]
fn archive_git_failure_is_nonzero_and_reports_the_applied_changes() {
    let temp = TempDir::new("bob-cli-archive-json-git-failure");
    let (vault, remote) = init_committed_git_vault(&temp);
    // The initial push registered the remote; removing it makes `git push`
    // fail after the vault write and the commit already succeeded.
    git_in(&vault, ["remote", "remove", "origin"]);
    drop(remote);

    let output = archive_command(&vault, &temp)
        .args(["-f", "json", "-t", "1"])
        .output()
        .expect("run bob task archive with a failing push");

    assert!(
        !output.status.success(),
        "a Git failure is nonzero:\n{}",
        format_output(&output)
    );
    let report = archive_json(&output);
    assert_eq!(report["exit"]["outcome"], "failed");
    assert_eq!(report["exit"]["reason"], "git_failed");
    assert_eq!(
        report["exit"]["code"].as_i64(),
        output.status.code().map(i64::from),
        "JSON exit code matches the process exit code"
    );
    assert!(report["exit"]["message"]
        .as_str()
        .unwrap()
        .contains("vault changes were applied"));
    assert_eq!(
        report["exit"]["applied_paths"],
        serde_json::json!(["done/obsidian_done.md", "obsidian.md"])
    );
    assert_eq!(report["git"]["outcome"], "failed");
    assert_eq!(report["git"]["committed"], true);
    assert_eq!(report["git"]["pushed"], false);
    assert_eq!(report["git"]["failure"]["step"], "git push");
    assert!(
        stderr(&output).contains("git push failed"),
        "subprocess diagnostics go to stderr:\n{}",
        format_output(&output)
    );
    let archive =
        fs::read_to_string(vault.join("done/obsidian_done.md")).unwrap();
    assert!(
        archive.contains("done #task [completion:: 2000-01-01] ^d1"),
        "{archive}"
    );
    let commits = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(
        commits.lines().next(),
        Some("bob task archive 2026-06-02"),
        "the commit already happened:\n{commits}"
    );
}

#[test]
fn archive_reports_holds_for_custom_open_statuses() {
    let temp = TempDir::new("bob-cli-archive-json-custom-hold");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("obsidian.md"),
        &area_note(
            "\
- [x] parent #task ^p
  - [?] blocked child #task ^c
- [x] other #task
",
        ),
    );
    write_blocked_tasks_settings(&vault);

    let output = archive_command(&vault, &temp)
        .args(["-n", "-f", "json", "-t", "1"])
        .output()
        .expect("run bob task archive with custom statuses");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(
        report["holds"],
        serde_json::json!([{
            "path": "obsidian.md",
            "line": 5,
            "block_id": "p",
            "reason": "open_subtask"
        }])
    );
    assert_eq!(report["moves"][0]["task_count"], 1);
    assert_eq!(
        report["moves"][0]["roots"],
        serde_json::json!([{"line": 7, "block_id": null}])
    );
}

#[test]
fn archive_option_spellings_are_equivalent() {
    let temp = TempDir::new("bob-cli-archive-spellings");
    let (vault, _remote) = init_committed_git_vault(&temp);
    let spellings: [&[&str]; 4] = [
        &["-n", "-f", "json", "-t", "1"],
        &["--dry-run", "--format", "json", "--threshold", "1"],
        &["--dry-run", "--format=json", "--threshold=1"],
        &["-n", "-fjson", "-t1"],
    ];
    let mut outputs = Vec::new();
    for args in spellings {
        let output = archive_command(&vault, &temp)
            .args(args)
            .output()
            .expect("run bob task archive spelling");
        assert_success(&output);
        outputs.push(stdout(&output));
    }
    for output in &outputs[1..] {
        assert_eq!(output, &outputs[0], "spellings print the same report");
    }
    assert_eq!(
        stdout(&git_in(&vault, ["status", "--short"])),
        "",
        "preview left the vault clean"
    );
}

#[test]
fn archive_usage_errors_exit_two_without_a_report() {
    let temp = TempDir::new("bob-cli-archive-usage");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    for args in [
        &["--format", "xml"][..],
        &["-f"][..],
        &["--format=yaml"][..],
        &["--dry-run=1"][..],
        &["extra"][..],
    ] {
        let output = archive_command(&vault, &temp)
            .args(args)
            .output()
            .expect("run bob task archive with bad usage");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?} is a usage error:\n{}",
            format_output(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?} prints no report");
        assert!(
            stderr(&output).contains("Try 'bob task archive --help'"),
            "{args:?}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn invalid_archive_config_fails_before_planning_in_both_output_modes() {
    let temp = TempDir::new("bob-cli-archive-invalid-config");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_file(&config, "archive:\n  keep_days: 0\n");

    for args in [&["-n", "-f", "json"][..], &["-n"][..]] {
        let output = archive_command(&vault, &temp)
            .args(args)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("run archive with invalid configuration");
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains("archive.keep_days"),
            "{}",
            format_output(&output)
        );
        if args.contains(&"json") {
            let report = archive_json(&output);
            assert_eq!(report["exit"]["outcome"], "failed");
            assert_eq!(report["exit"]["reason"], "plan_failed");
            assert!(report["exit"]["message"]
                .as_str()
                .unwrap()
                .contains("archive.keep_days"));
        } else {
            assert!(stdout(&output).contains("failed: plan_failed"));
        }
    }
}

#[test]
fn invalid_archive_keep_falls_back_and_warns_once_per_note() {
    let temp = TempDir::new("bob-cli-archive-invalid-note-cap");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = temp.path().join("config.yml");
    write_file(&config, "archive:\n  keep_max: 2\n");
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[area]]\narchive_keep: 0\n---\n\n\
- [x] first #task [completion:: 2026-06-02] ^first\n\
- [x] second #task [completion:: 2026-06-02] ^second\n\
- [x] third #task [completion:: 2026-06-02] ^third\n\
- [x] fourth #task [completion:: 2026-06-02] ^fourth\n",
    );

    let output = archive_command(&vault, &temp)
        .args(["-n", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run archive with invalid note cap");

    assert_success(&output);
    let report = archive_json(&output);
    assert_eq!(report["moves"][0]["task_count"], 2);
    assert!(report.get("archive_keep").is_none());
    assert_eq!(stderr(&output).matches("archive_keep").count(), 1);
    assert!(stderr(&output).contains("work.md"));
}
