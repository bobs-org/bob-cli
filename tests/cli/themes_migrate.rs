//! `bob types migrate`: previewed, verified, single-commit spelling rewrite.

use crate::support::*;
use serde_json::Value;
use std::fs;

fn migrate_vault(
    root: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\ntheme: BUILD\n---\n\
- [ ] #task Open [theme:: WRITE] ^open\n\
- [ ] #task Paren (theme::  FIX) ^paren\n\
- [x] #task Closed [theme:: BUILD] ^closed\n\
- [ ] #task Canonical [type:: READ] ^canonical\n\
- [ ] #task Mixed [type:: BUILD] (theme:: BUILD) ^mixed\n\
- [ ] #task Invalid [theme:: BUILD + FIX] ^invalid\n\
Prose [theme:: BUILD] stays.\n\
```md\n\
- [ ] #task Fenced [theme:: BUILD]\n\
```\n",
    );
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[project]]\ntask_type: BUILD\ntheme: BUILD\n---\n",
    );
    write_file(
        &vault.join("conflict.md"),
        "---\ntype: [[area]]\ntask_type: BUILD\ntheme: FIX\n---\n",
    );
    write_file(
        &vault.join("quoted.md"),
        "---\ntype: [[project]]\ntheme: \"WRITE\"\n---\n",
    );
    write_file(
        &vault.join("done/archive.md"),
        "---\ntype: [[area]]\ntheme: BUILD\n---\n\
- [ ] #task Archived [theme:: WRITE] ^arch\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

fn migrate_command(
    vault: &std::path::Path,
    cache: &std::path::Path,
) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
}

fn migrate_json(
    vault: &std::path::Path,
    cache: &std::path::Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let mut command = migrate_command(vault, cache);
    command
        .args(["types", "migrate"])
        .args(args)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("bob types migrate");
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("migrate JSON");
    (output, value)
}

fn hold_vault_lock(path: &std::path::Path) -> fs::File {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("lock dir");
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("open lock");
    fs2::FileExt::try_lock_exclusive(&file).expect("take lock");
    file
}

#[test]
fn wrong_vault_refuses_before_syncing_or_migrating() {
    let temp = TempDir::new("bob-cli-themes-migrate-wrong-vault");
    let (vault, _) = migrate_vault(temp.path());
    assert_vault_override_refused(
        bob_command().args(["types", "migrate"]),
        &vault,
    );
}

#[test]
fn themes_migrate_dry_run_previews_and_leaves_vault_untouched() {
    let temp = TempDir::new("bob-cli-themes-migrate-dry");
    let (vault, cache) = migrate_vault(temp.path());
    let before_job = fs::read_to_string(vault.join("job.md")).unwrap();
    let before_dev = fs::read_to_string(vault.join("dev.md")).unwrap();

    let output = migrate_command(&vault, &cache)
        .args(["types", "migrate", "-d"])
        .output()
        .expect("dry run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("job.md"), "preview:\n{text}");
    assert!(text.contains("dev.md"), "preview:\n{text}");
    assert!(text.contains("quoted.md"), "preview:\n{text}");
    assert!(text.contains("needs repair"), "repair:\n{text}");
    assert!(text.contains("conflicts"), "conflict:\n{text}");

    assert_eq!(
        fs::read_to_string(vault.join("job.md")).unwrap(),
        before_job
    );
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).unwrap(),
        before_dev
    );
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 1);

    let (_, value) = migrate_json(&vault, &cache, &["-d"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["mode"], "dry_run");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["fields"], 2);
    assert_eq!(value["defaults"], 3);
    let files = value["files"].as_array().expect("files");
    assert!(files.iter().any(|file| file["path"] == "job.md"
        && file["fields"] == 2
        && file["defaults"] == 1));
    assert!(files
        .iter()
        .any(|file| file["path"] == "dev.md" && file["defaults"] == 1));
    let repairs = value["needs_repair"].as_array().expect("needs_repair");
    assert_eq!(repairs.len(), 2);
    assert!(repairs.iter().any(|row| row["kind"] == "duplicate"));
    assert!(repairs.iter().any(|row| row["kind"] == "invalid"));
    let conflicts = value["conflicts"].as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "conflict.md");
}

#[test]
fn themes_alias_matches_types_for_migrate_dry_run() {
    let temp = TempDir::new("bob-cli-themes-migrate-alias-parity");
    let (vault, cache) = migrate_vault(temp.path());

    let alias = migrate_command(&vault, &cache)
        .args(["themes", "migrate", "-d"])
        .output()
        .expect("bob themes migrate dry-run");
    let canonical = migrate_command(&vault, &cache)
        .args(["types", "migrate", "-d"])
        .output()
        .expect("bob types migrate dry-run");

    assert_eq!(alias.status.code(), canonical.status.code());
    assert_eq!(alias.stdout, canonical.stdout);
    assert_eq!(alias.stderr, canonical.stderr);
}

#[test]
fn themes_migrate_apply_verifies_and_commits() {
    let temp = TempDir::new("bob-cli-themes-migrate-apply");
    let (vault, cache) = migrate_vault(temp.path());

    let output = migrate_command(&vault, &cache)
        .args(["types", "migrate", "-o"])
        .output()
        .expect("apply");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("committed"), "commit:\n{text}");
    assert!(
        text.contains("bob types migrate: 2 fields, 3 defaults"),
        "{text}"
    );
    assert!(text.contains("sync: pre skipped, post skipped"), "{text}");

    let job = fs::read_to_string(vault.join("job.md")).unwrap();
    assert!(job.contains("[type:: WRITE]"), "{job}");
    assert!(job.contains("(type::  FIX)"), "{job}");
    assert!(job.contains("task_type: BUILD"), "{job}");
    assert!(!job.contains("[theme:: WRITE]"), "{job}");
    assert!(job.contains("- [x] #task Closed [theme:: BUILD]"), "{job}");
    assert!(job.contains("[type:: READ]"), "{job}");
    assert!(job.contains("[type:: BUILD] (theme:: BUILD)"), "{job}");
    assert!(job.contains("[theme:: BUILD + FIX]"), "{job}");
    assert!(job.contains("Prose [theme:: BUILD] stays."), "{job}");
    assert!(job.contains("- [ ] #task Fenced [theme:: BUILD]"), "{job}");

    let dev = fs::read_to_string(vault.join("dev.md")).unwrap();
    assert_eq!(dev, "---\ntype: [[project]]\ntask_type: BUILD\n---\n");

    let quoted = fs::read_to_string(vault.join("quoted.md")).unwrap();
    assert_eq!(
        quoted,
        "---\ntype: [[project]]\ntask_type: \"WRITE\"\n---\n"
    );

    let conflict = fs::read_to_string(vault.join("conflict.md")).unwrap();
    assert!(conflict.contains("task_type: BUILD"), "{conflict}");
    assert!(conflict.contains("theme: FIX"), "{conflict}");

    let archived = fs::read_to_string(vault.join("done/archive.md")).unwrap();
    assert!(archived.contains("theme: BUILD"), "{archived}");
    assert!(archived.contains("[theme:: WRITE]"), "{archived}");

    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 2);
    let show = git_in(&vault, ["show", "--stat", "--format=%s", "HEAD"]);
    let stat = stdout(&show);
    assert!(
        stat.contains("bob types migrate: 2 fields, 3 defaults"),
        "{stat}"
    );
    assert!(stat.contains("job.md"), "{stat}");
    assert!(stat.contains("dev.md"), "{stat}");
    assert!(stat.contains("quoted.md"), "{stat}");
    assert!(!stat.contains("conflict.md"), "{stat}");
    assert!(!stat.contains("done/archive.md"), "{stat}");

    git_in(&vault, ["revert", "--no-edit", "HEAD"]);
    let restored = fs::read_to_string(vault.join("job.md")).unwrap();
    assert!(restored.contains("[theme:: WRITE]"), "{restored}");
    assert!(restored.contains("theme: BUILD"), "{restored}");
}

#[test]
fn themes_migrate_second_run_is_unchanged() {
    let temp = TempDir::new("bob-cli-themes-migrate-idempotent");
    let (vault, cache) = migrate_vault(temp.path());
    let first = migrate_command(&vault, &cache)
        .args(["types", "migrate", "-o"])
        .output()
        .expect("first migrate");
    assert_success(&first);

    let (output, value) = migrate_json(&vault, &cache, &["-o"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unchanged");
    assert_eq!(value["fields"], 0);
    assert_eq!(value["defaults"], 0);
    assert!(value["needs_repair"].as_array().unwrap().len() >= 2);
    assert_eq!(value["conflicts"].as_array().unwrap().len(), 1);
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 2);
}

#[test]
fn themes_migrate_refusals_are_json_on_nonzero_exit() {
    let temp = TempDir::new("bob-cli-themes-migrate-refusals");
    let (vault, cache) = migrate_vault(temp.path());

    let clean = temp.path().join("clean");
    let (clean_vault, clean_cache) = migrate_vault(&clean);
    write_file(
        &clean_vault.join("job.md"),
        "---\ntype: [[area]]\ntask_type: BUILD\n---\n- [ ] #task Open [type:: WRITE] ^open\n",
    );
    write_file(&clean_vault.join("dev.md"), "---\ntype: [[project]]\n---\n");
    write_file(
        &clean_vault.join("conflict.md"),
        "---\ntype: [[area]]\n---\n",
    );
    write_file(
        &clean_vault.join("quoted.md"),
        "---\ntype: [[project]]\n---\n",
    );
    write_file(&clean_vault.join("done/archive.md"), "archive\n");
    git_in(&clean_vault, ["add", "-A"]);
    git_in(&clean_vault, ["commit", "-qm", "canonical"]);
    let (output, value) = migrate_json(&clean_vault, &clean_cache, &["-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unchanged");

    let dirty_path = vault.join("job.md");
    let dirty = fs::read_to_string(&dirty_path).unwrap() + "\ndirty\n";
    write_file(&dirty_path, &dirty);
    let (output, value) = migrate_json(&vault, &cache, &["-o"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "dirty_paths");
    git_in(&vault, ["checkout", "--", "job.md"]);

    let plain = temp.path().join("plain");
    let plain_vault = plain.join("vault");
    let plain_cache = plain.join("cache");
    fs::create_dir_all(&plain_cache).unwrap();
    write_file(
        &plain_vault.join("job.md"),
        "---\ntype: [[area]]\ntheme: BUILD\n---\n- [ ] #task Open [theme:: WRITE] ^open\n",
    );
    let (output, value) = migrate_json(&plain_vault, &plain_cache, &["-o"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "not_a_worktree");
}

#[test]
fn themes_migrate_lock_busy_when_maintenance_lock_held() {
    let temp = TempDir::new("bob-cli-themes-migrate-lock");
    let (vault, cache) = migrate_vault(temp.path());
    let lock = temp.path().join("bob_sync.lock");
    let held = hold_vault_lock(&lock);

    let output = migrate_command(&vault, &cache)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["types", "migrate", "-o", "-f", "json"])
        .output()
        .expect("lock busy");
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "lock_busy");
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 1);
    drop(held);
}
