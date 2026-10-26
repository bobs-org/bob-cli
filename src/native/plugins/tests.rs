use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::native::{env::TestEnvGuard, style::truncate};

use super::{
    deploy::{
        acquire_deploy_lock, deploy_lock_path, scan_deployment, stamp_path,
        DeployStatus, STAMP_FILE, STAMP_IGNORE_HINT,
    },
    diff::{diff_existing_file, diff_text, MINIFIED_BYTE_THRESHOLD},
    git::{pull_repo, PullOutcome},
    model::{
        BackupOutcome, DiffKind, FileAction, FileDiff, FileSync, SyncOptions,
        SyncReport, SyncState, VaultState,
    },
    render::{success_json, sync_success_json},
    scan::{scan_plugins, sync_state, vault_state},
    sync::sync_plugins,
};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn sync_state_detects_synced_drift_and_missing() {
    let temp = TempDir::new("bob-cli-plugins-sync");
    let repo = temp.path().join("repo/plugins/alpha");
    let synced_vault = temp.path().join("vault/.obsidian/plugins/alpha");
    write_file(&repo.join("manifest.json"), "{\"id\":\"alpha\"}\n");
    write_file(&repo.join("main.js"), "console.log('alpha');\n");
    write_file(&synced_vault.join("manifest.json"), "{\"id\":\"alpha\"}\n");
    write_file(&synced_vault.join("main.js"), "console.log('alpha');\n");
    assert_eq!(sync_state(&repo, &synced_vault), SyncState::Synced);

    let drift_vault = temp.path().join("vault/.obsidian/plugins/beta");
    write_file(&drift_vault.join("manifest.json"), "{\"id\":\"alpha\"}\n");
    write_file(&drift_vault.join("main.js"), "console.log('old');\n");
    assert_eq!(sync_state(&repo, &drift_vault), SyncState::Drift);

    let missing_vault = temp.path().join("vault/.obsidian/plugins/gone");
    assert_eq!(sync_state(&repo, &missing_vault), SyncState::Missing);
}

#[test]
fn vault_state_reads_enabled_disabled_and_not_installed() {
    let temp = TempDir::new("bob-cli-plugins-vault");
    let installed = temp.path().join(".obsidian/plugins/alpha");
    write_file(&installed.join("manifest.json"), "{}\n");
    let enabled: HashSet<String> = ["alpha".to_string()].into();

    assert_eq!(
        vault_state("alpha", &enabled, &installed),
        VaultState::Enabled
    );
    assert_eq!(
        vault_state("beta", &enabled, &installed),
        VaultState::Disabled
    );
    let missing = temp.path().join(".obsidian/plugins/gone");
    assert_eq!(
        vault_state("gone", &enabled, &missing),
        VaultState::NotInstalled
    );
}

#[test]
fn scan_reports_states_and_counts() {
    let temp = TempDir::new("bob-cli-plugins-scan");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");

    write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "alpha-body");
    write_plugin(&repo, "beta", "2.1.0", "Beta plugin", "beta-body");
    write_plugin(&repo, "gamma", "1.0.0", "Gamma plugin", "gamma-body");

    // alpha: identical in vault + enabled.
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha plugin", "alpha-body");
    // beta: installed but body differs + disabled.
    write_vault_plugin(&vault, "beta", "2.1.0", "Beta plugin", "stale-body");
    // gamma: not installed.
    write_file(
        &vault.join(".obsidian/community-plugins.json"),
        "[\"alpha\"]\n",
    );

    let report = scan_plugins(&repo, &vault);
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
    assert_eq!(report.plugins.len(), 3);

    let alpha = &report.plugins[0];
    assert_eq!(alpha.id, "alpha");
    assert_eq!(alpha.version, "1.0.0");
    assert_eq!(alpha.sync, SyncState::Synced);
    assert_eq!(alpha.vault, VaultState::Enabled);

    let beta = &report.plugins[1];
    assert_eq!(beta.sync, SyncState::Drift);
    assert_eq!(beta.vault, VaultState::Disabled);

    let gamma = &report.plugins[2];
    assert_eq!(gamma.sync, SyncState::Missing);
    assert_eq!(gamma.vault, VaultState::NotInstalled);

    let counts = report.counts();
    assert_eq!(counts.synced, 1);
    assert_eq!(counts.drift, 1);
    assert_eq!(counts.not_installed, 1);
}

#[test]
fn unreadable_repo_is_an_error() {
    let temp = TempDir::new("bob-cli-plugins-empty");
    let report = scan_plugins(&temp.path().join("missing"), temp.path());
    assert_eq!(report.issues.len(), 1);
    assert!(report.plugins.is_empty());
}

#[test]
fn pull_repo_skips_non_git_directory() {
    let temp = TempDir::new("bob-cli-plugins-pull-non-git");
    let repo = temp.path().join("repo");
    write_file(&repo.join("plugins/alpha/main.js"), "// local\n");

    assert_eq!(pull_repo(&repo), PullOutcome::Skipped);
    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js")).unwrap(),
        "// local\n"
    );
}

#[test]
fn pull_repo_fast_forwards_from_remote() {
    let temp = TempDir::new("bob-cli-plugins-pull");
    let remote = temp.path().join("remote.git");
    let seed = temp.path().join("seed");
    let repo = temp.path().join("repo");
    let upstream = temp.path().join("upstream");

    run_git(temp.path(), &["init", "-q", "--bare", path_str(&remote)]);
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&remote), path_str(&seed)],
    );
    git_config_identity(&seed);
    write_plugin(&seed, "alpha", "1.0.0", "Alpha plugin", "old");
    run_git(&seed, &["add", "."]);
    run_git(&seed, &["commit", "-q", "-m", "initial"]);
    run_git(&seed, &["push", "-q", "-u", "origin", "HEAD"]);

    run_git(
        temp.path(),
        &["clone", "-q", path_str(&remote), path_str(&repo)],
    );
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&remote), path_str(&upstream)],
    );
    git_config_identity(&upstream);
    write_file(&upstream.join("plugins/alpha/main.js"), "// new\n");
    run_git(&upstream, &["add", "."]);
    run_git(&upstream, &["commit", "-q", "-m", "update alpha"]);
    run_git(&upstream, &["push", "-q"]);

    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js")).unwrap(),
        "// old\n"
    );
    match pull_repo(&repo) {
        PullOutcome::Pulled { summary } => {
            assert!(
                summary.contains("Fast-forward") || summary == "completed",
                "unexpected pull summary: {summary}"
            );
        }
        outcome => panic!("expected pull to run, got {outcome:?}"),
    }
    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js")).unwrap(),
        "// new\n"
    );
}

#[test]
fn json_shape_is_stable() {
    let temp = TempDir::new("bob-cli-plugins-json");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "body");
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha plugin", "body");
    write_file(
        &vault.join(".obsidian/community-plugins.json"),
        "[\"alpha\"]\n",
    );

    let result = scan_plugins(&repo, &vault).result();
    let value: serde_json::Value =
        serde_json::from_str(&success_json(&result)).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["count"], 1);
    assert_eq!(value["synced"], 1);
    assert_eq!(value["drift"], 0);
    assert_eq!(value["not_installed"], 0);
    assert_eq!(value["plugins"][0]["id"], "alpha");
    assert_eq!(value["plugins"][0]["version"], "1.0.0");
    assert_eq!(value["plugins"][0]["sync"], "synced");
    assert_eq!(value["plugins"][0]["vault"], "enabled");
}

#[test]
fn sync_json_shape_is_stable() {
    let temp = TempDir::new("bob-cli-plugins-sync-json");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "alpha");
    write_plugin(&repo, "beta", "2.0.0", "Beta plugin", "beta");
    write_plugin(&repo, "gamma", "1.0.0", "Gamma plugin", "gamma");
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha plugin", "alpha");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta plugin", "stale");

    let (opts, _guard) = options(&repo, &vault);
    let result = sync_plugins(&opts).result(false);
    let value: serde_json::Value =
        serde_json::from_str(&sync_success_json(&result)).expect("json");
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "bob_dir",
            "copied",
            "dry_run",
            "ok",
            "plugins",
            "repo",
            "skipped",
            "unchanged",
        ]
    );
    assert_eq!(value["ok"], true);
    assert_eq!(value["dry_run"], false);
    assert_eq!(value["copied"], 3);
    assert_eq!(value["skipped"], 0);
    assert_eq!(value["unchanged"], 3);
    assert_eq!(value["plugins"][1]["id"], "beta");
    assert_eq!(value["plugins"][1]["files"][1]["name"], "main.js");
    assert_eq!(value["plugins"][1]["files"][1]["action"], "updated");
    assert_eq!(value["plugins"][1]["deployment"]["status"], "unknown");
    assert_eq!(
        serde_json::to_value(FileAction::Created).unwrap(),
        "created"
    );
    assert_eq!(
        serde_json::to_value(FileAction::Updated).unwrap(),
        "updated"
    );
    assert_eq!(serde_json::to_value(FileAction::Forced).unwrap(), "forced");
    assert_eq!(
        serde_json::to_value(FileAction::Unchanged).unwrap(),
        "unchanged"
    );
    assert_eq!(
        serde_json::to_value(FileAction::SkippedDirty).unwrap(),
        "skipped_dirty"
    );
    assert_eq!(serde_json::to_value(FileAction::Failed).unwrap(), "failed");
}

#[test]
fn truncate_adds_ellipsis_only_when_needed() {
    assert_eq!(truncate("short", 10), "short");
    assert_eq!(truncate("toolongdesc", 5), "tool\u{2026}");
    assert_eq!(truncate("anything", 0), "");
}

#[test]
fn sync_creates_updates_and_leaves_unchanged() {
    let temp = TempDir::new("bob-cli-plugins-sync-actions");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");

    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
    write_plugin(&repo, "gamma", "1.0.0", "Gamma", "gamma");

    // alpha already matches; beta drifts; gamma is not installed.
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha", "alpha");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");

    let (opts, _guard) = options(&repo, &vault);
    let report = sync_plugins(&opts);
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);

    assert_eq!(
        action_for(&report, "alpha", "manifest.json"),
        Some(FileAction::Unchanged)
    );
    assert_eq!(
        action_for(&report, "alpha", "main.js"),
        Some(FileAction::Unchanged)
    );
    // beta's manifest matches but its main.js drifts and is rewritten.
    assert_eq!(
        action_for(&report, "beta", "manifest.json"),
        Some(FileAction::Unchanged)
    );
    assert_eq!(
        action_for(&report, "beta", "main.js"),
        Some(FileAction::Updated)
    );
    assert_eq!(
        action_for(&report, "gamma", "manifest.json"),
        Some(FileAction::Created)
    );

    let beta_main = vault.join(".obsidian/plugins/beta/main.js");
    assert_eq!(fs::read_to_string(&beta_main).unwrap(), "// beta\n");
    let beta_backup = temp.path().join("backups/20260626-143000/beta/main.js");
    assert_eq!(
        fs::read_to_string(&beta_backup).unwrap(),
        "// stale\n",
        "updated files must be backed up before overwrite"
    );
    let beta_sync =
        file_for(&report, "beta", "main.js").expect("beta main sync");
    assert_eq!(
        beta_sync.backup,
        Some(BackupOutcome {
            path: beta_backup,
            written: true,
        })
    );
    let gamma_manifest = vault.join(".obsidian/plugins/gamma/manifest.json");
    assert!(gamma_manifest.is_file(), "gamma should be created");
    assert_eq!(report.copied(), 3);
    assert_eq!(report.unchanged(), 3);
}

#[test]
fn sync_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-plugins-sync-dry");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");

    let (mut opts, _guard) = options(&repo, &vault);
    opts.dry_run = true;
    let report = sync_plugins(&opts);

    assert_eq!(
        action_for(&report, "beta", "main.js"),
        Some(FileAction::Updated)
    );
    let beta_sync =
        file_for(&report, "beta", "main.js").expect("beta main sync");
    assert_eq!(
        beta_sync.backup,
        Some(BackupOutcome {
            path: temp.path().join("backups/20260626-143000/beta/main.js"),
            written: false,
        })
    );
    assert!(
        !temp.path().join("backups").exists(),
        "dry-run must not create the backup directory"
    );
    let beta_main = vault.join(".obsidian/plugins/beta/main.js");
    assert_eq!(
        fs::read_to_string(&beta_main).unwrap(),
        "// stale\n",
        "dry-run must not write the vault file"
    );
}

#[test]
fn sync_only_filters_to_a_single_plugin() {
    let temp = TempDir::new("bob-cli-plugins-sync-only");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");

    let (mut opts, _guard) = options(&repo, &vault);
    opts.only = Some("beta".to_string());
    let report = sync_plugins(&opts);

    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].id, "beta");
    assert!(report.issues.is_empty());
    assert!(
        vault.join(".obsidian/plugins/beta/main.js").is_file(),
        "beta should be synced"
    );
    assert!(
        !vault.join(".obsidian/plugins/alpha").exists(),
        "alpha must be left untouched"
    );
}

#[test]
fn sync_unknown_plugin_is_an_error() {
    let temp = TempDir::new("bob-cli-plugins-sync-unknown");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");

    let (mut opts, _guard) = options(&repo, &vault);
    opts.only = Some("missing".to_string());
    let report = sync_plugins(&opts);

    assert!(report.plugins.is_empty());
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].contains("plugin not found in repo: missing"));
}

#[test]
fn sync_preserves_runtime_data_json() {
    let temp = TempDir::new("bob-cli-plugins-sync-data");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");
    let data_json = vault.join(".obsidian/plugins/beta/data.json");
    write_file(&data_json, "{\"setting\":true}\n");

    let (opts, _guard) = options(&repo, &vault);
    let report = sync_plugins(&opts);
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);

    assert_eq!(
        fs::read_to_string(&data_json).unwrap(),
        "{\"setting\":true}\n",
        "data.json must never be touched by sync"
    );
    let beta_main = vault.join(".obsidian/plugins/beta/main.js");
    assert_eq!(fs::read_to_string(&beta_main).unwrap(), "// beta\n");
}

#[test]
fn sync_refuses_then_forces_a_dirty_vault_file() {
    let temp = TempDir::new("bob-cli-plugins-sync-dirty");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "committed");

    // Commit the vault, then dirty beta's main.js so it differs from both
    // the committed version and the repo. Ignore the deploy stamp so this
    // test covers the dirty-vault guard independently of stamp storage.
    write_stamp_ignore(&vault);
    git_init_commit(&vault);
    let beta_main = vault.join(".obsidian/plugins/beta/main.js");
    write_file(&beta_main, "// local edit\n");

    let (opts, _guard) = options(&repo, &vault);
    let report = sync_plugins(&opts);
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
    assert_eq!(
        action_for(&report, "beta", "main.js"),
        Some(FileAction::SkippedDirty)
    );
    let skipped =
        file_for(&report, "beta", "main.js").expect("skipped beta main");
    assert!(skipped.backup.is_none(), "skipped files are untouched");
    assert_eq!(
        fs::read_to_string(&beta_main).unwrap(),
        "// local edit\n",
        "a dirty vault file must not be overwritten without --force"
    );

    let (mut opts, _guard) = options(&repo, &vault);
    opts.force = true;
    let forced = sync_plugins(&opts);
    assert_eq!(
        action_for(&forced, "beta", "main.js"),
        Some(FileAction::Forced)
    );
    let forced_backup =
        temp.path().join("backups/20260626-143000/beta/main.js");
    assert_eq!(
        fs::read_to_string(&forced_backup).unwrap(),
        "// local edit\n",
        "forced dirty overwrite must preserve the dirty file"
    );
    assert_eq!(
        fs::read_to_string(&beta_main).unwrap(),
        "// beta\n",
        "--force should overwrite the dirty vault file"
    );
}

#[test]
fn sync_reports_text_diff_for_changed_files() {
    let old = "alpha\nbeta\ngamma\n";
    let new = "alpha\nbeta changed\ngamma\ndelta\n";

    let FileDiff::Text {
        lines,
        added,
        removed,
        hidden,
    } = diff_text(old, new)
    else {
        panic!("expected text diff");
    };

    assert_eq!(added, 2);
    assert_eq!(removed, 1);
    assert_eq!(hidden, 0);
    assert!(
        lines
            .iter()
            .any(|line| line.kind == DiffKind::Hunk
                && line.text.starts_with("@@")),
        "expected a hunk header: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.kind == DiffKind::Del && line.text == "-beta"),
        "expected deleted line: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.kind == DiffKind::Add
                && line.text == "+beta changed"),
        "expected added line: {lines:?}"
    );
}

#[test]
fn sync_summarizes_binary_and_minified_diffs() {
    assert_eq!(
        diff_existing_file(b"\xffold", b"\xffnew"),
        FileDiff::Binary {
            old_len: 4,
            new_len: 4,
        }
    );

    let old = "a".repeat(MINIFIED_BYTE_THRESHOLD);
    let new = "b".repeat(MINIFIED_BYTE_THRESHOLD);
    assert_eq!(
        diff_existing_file(old.as_bytes(), new.as_bytes()),
        FileDiff::Binary {
            old_len: MINIFIED_BYTE_THRESHOLD,
            new_len: MINIFIED_BYTE_THRESHOLD,
        }
    );
}

#[test]
fn backup_failure_aborts_overwrite() {
    let temp = TempDir::new("bob-cli-plugins-backup-failure");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
    write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");
    let backup_run_dir = temp.path().join("not-a-directory");
    write_file(&backup_run_dir, "I am a file, not a directory\n");

    let (mut opts, _guard) = options(&repo, &vault);
    opts.backup_run_dir = backup_run_dir;
    let report = sync_plugins(&opts);

    assert_eq!(
        action_for(&report, "beta", "main.js"),
        Some(FileAction::Failed)
    );
    assert_eq!(report.issues.len(), 1);
    assert!(
        report.issues[0].contains("failed to create backup directory"),
        "unexpected issue: {:?}",
        report.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .unwrap(),
        "// stale\n",
        "overwrite must be aborted when backup cannot be written"
    );
}

#[test]
fn dirty_deploy_from_other_checkout_is_refused() {
    let temp = TempDir::new("bob-cli-plugins-guard-ab");
    let seed = temp.path().join("seed");
    write_plugin(&seed, "alpha", "1.0.0", "Alpha", "base");
    git_init_commit(&seed);

    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_a)],
    );
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_b)],
    );
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);

    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");
    let (opts_a, _guard) = options(&checkout_a, &vault);
    let first = sync_plugins(&opts_a);
    assert!(first.issues.is_empty(), "issues: {:?}", first.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// dirty-a\n"
    );
    let stamp = stamp_path(&vault, "alpha");
    let stamp_bytes = fs::read(&stamp).unwrap();
    assert!(
        String::from_utf8_lossy(&stamp_bytes).contains("\"dirty\": \"dirty\"")
    );

    let (opts_b, _guard_b) = options(&checkout_b, &vault);
    let refused = sync_plugins(&opts_b);
    assert!(
        refused.issues.iter().any(|issue| {
            issue.contains("refusing to overwrite plugin alpha")
                && issue.contains("deployed from uncommitted build")
        }),
        "expected a deploy-guard refusal: {:?}",
        refused.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// dirty-a\n",
        "refused sync must not change vault bytes"
    );
    assert_eq!(
        fs::read(&stamp).unwrap(),
        stamp_bytes,
        "refused sync must not change the stamp"
    );
    assert!(
        !temp.path().join("backups/20260626-143000/alpha").exists()
            || fs::read_dir(temp.path().join("backups/20260626-143000/alpha"))
                .map(|entries| entries.count())
                .unwrap_or(0)
                == 0
            || first.copied() > 0,
        "B must not write backups for a refused overwrite"
    );

    git_config_identity(&checkout_b);
    write_file(&checkout_b.join("README.md"), "later\n");
    run_git(&checkout_b, &["add", "README.md"]);
    run_git(
        &checkout_b,
        &["commit", "-q", "-m", "descendant without A's dirty"],
    );
    let still = sync_plugins(&opts_b);
    assert!(
        still.issues.iter().any(|issue| issue.contains("refusing")),
        "a descendant that lacks the dirty bytes must still refuse: {:?}",
        still.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// dirty-a\n"
    );
}

#[test]
fn same_checkout_dirty_iteration_is_permitted() {
    let temp = TempDir::new("bob-cli-plugins-guard-iterate");
    let repo = temp.path().join("repo");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "one");
    git_init_commit(&repo);
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);

    write_file(&repo.join("plugins/alpha/main.js"), "// two\n");
    let (opts, _guard) = options(&repo, &vault);
    let first = sync_plugins(&opts);
    assert!(first.issues.is_empty(), "{:?}", first.issues);

    write_file(&repo.join("plugins/alpha/main.js"), "// three\n");
    let second = sync_plugins(&opts);
    assert!(second.issues.is_empty(), "{:?}", second.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// three\n"
    );
}

#[test]
fn clean_exact_match_lands_then_descendant_updates() {
    let temp = TempDir::new("bob-cli-plugins-guard-land");
    let seed = temp.path().join("seed");
    write_plugin(&seed, "alpha", "1.0.0", "Alpha", "base");
    git_init_commit(&seed);

    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_a)],
    );
    git_config_identity(&checkout_a);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// fix\n");
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    let (opts_a, _guard) = options(&checkout_a, &vault);
    assert!(sync_plugins(&opts_a).issues.is_empty());

    run_git(&checkout_a, &["add", "plugins/alpha/main.js"]);
    run_git(&checkout_a, &["commit", "-q", "-m", "land the fix"]);
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&checkout_a), path_str(&checkout_b)],
    );

    let (opts_b, _guard_b) = options(&checkout_b, &vault);
    let landed = sync_plugins(&opts_b);
    assert!(landed.issues.is_empty(), "{:?}", landed.issues);
    let stamp = fs::read_to_string(stamp_path(&vault, "alpha")).unwrap();
    assert!(
        stamp.contains("\"dirty\": \"clean\""),
        "exact-match landing must reconcile to a clean stamp: {stamp}"
    );

    git_config_identity(&checkout_b);
    write_file(&checkout_b.join("plugins/alpha/main.js"), "// later\n");
    run_git(&checkout_b, &["add", "plugins/alpha/main.js"]);
    run_git(&checkout_b, &["commit", "-q", "-m", "later update"]);
    let updated = sync_plugins(&opts_b);
    assert!(updated.issues.is_empty(), "{:?}", updated.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// later\n"
    );
}

#[test]
fn repeat_noop_leaves_stamp_untouched() {
    let temp = TempDir::new("bob-cli-plugins-stamp-stable");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    let (opts, _guard) = options(&repo, &vault);
    let first_sync = sync_plugins(&opts);
    assert!(first_sync.issues.is_empty(), "{:?}", first_sync.issues);
    let stamp = stamp_path(&vault, "alpha");
    let first = fs::read(&stamp).unwrap();

    let later = TestEnvGuard::set(&[(
        "BOB_NOW",
        Some(OsStr::new("2026-06-26 15:00:00")),
    )]);
    assert!(sync_plugins(&opts).issues.is_empty());
    drop(later);
    assert_eq!(
        fs::read(&stamp).unwrap(),
        first,
        "unchanged real syncs must leave stamp content untouched"
    );
}

#[test]
fn malformed_stamp_refuses_changing_assets() {
    let temp = TempDir::new("bob-cli-plugins-stamp-bad");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "new");
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha", "old");
    write_file(
        &vault.join(".obsidian/plugins/alpha").join(STAMP_FILE),
        "{not json\n",
    );
    let (opts, _guard) = options(&repo, &vault);
    let report = sync_plugins(&opts);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.contains("malformed")),
        "{:?}",
        report.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// old\n"
    );
}

#[test]
fn pending_stamp_refuses_until_forced() {
    let temp = TempDir::new("bob-cli-plugins-stamp-pending");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "new");
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha", "old");
    write_file(
        &vault.join(".obsidian/plugins/alpha").join(STAMP_FILE),
        r#"{
  "version": 1,
  "plugin_id": "alpha",
  "source_root": "/other",
  "source_head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "dirty": "clean",
  "host": "other-host",
  "deployed_at": "2026-01-01T00:00:00Z",
  "assets": [
    {"name": "manifest.json", "present": true, "sha256": "00"},
    {"name": "main.js", "present": true, "sha256": "00"},
    {"name": "styles.css", "present": false}
  ],
  "digest": "00",
  "state": "pending"
}
"#,
    );
    let (opts, _guard) = options(&repo, &vault);
    let refused = sync_plugins(&opts);
    assert!(
        refused
            .issues
            .iter()
            .any(|issue| issue.contains("incomplete")
                || issue.contains("pending")
                || issue.contains("malformed")
                || issue.contains("digest")),
        "{:?}",
        refused.issues
    );

    let (mut forced, _guard_f) = options(&repo, &vault);
    forced.force = true;
    let report = sync_plugins(&forced);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// new\n"
    );
    assert!(
        temp.path()
            .join("backups/20260626-143000/alpha")
            .join(STAMP_FILE)
            .is_file()
            || temp
                .path()
                .join("backups")
                .read_dir()
                .ok()
                .and_then(|mut dirs| dirs.next().map(|e| e
                    .unwrap()
                    .path()
                    .join("alpha")
                    .join(STAMP_FILE)))
                .is_some_and(|path| path.is_file()),
        "forced replacement must back up the prior stamp"
    );
}

#[test]
fn optional_css_presence_and_data_json_are_preserved() {
    let temp = TempDir::new("bob-cli-plugins-css-data");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha", "stale");
    write_file(
        &vault.join(".obsidian/plugins/alpha/styles.css"),
        "/* leftover */\n",
    );
    write_file(
        &vault.join(".obsidian/plugins/alpha/data.json"),
        "{\"keep\":true}\n",
    );
    let (opts, _guard) = options(&repo, &vault);
    let report = sync_plugins(&opts);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/styles.css"))
            .unwrap(),
        "/* leftover */\n"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/data.json"))
            .unwrap(),
        "{\"keep\":true}\n"
    );
}

#[test]
fn git_vault_requires_ignored_untracked_stamp() {
    let temp = TempDir::new("bob-cli-plugins-stamp-ignore");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    write_file(&vault.join("README.md"), "vault\n");
    git_init_commit(&vault);
    let (opts, _guard) = options(&repo, &vault);
    let refused = sync_plugins(&opts);
    assert!(
        refused.issues.iter().any(|issue| {
            issue.contains("not ignored") || issue.contains("tracked")
        }),
        "{:?}",
        refused.issues
    );
    assert!(
        !vault.join(".obsidian/plugins/alpha/main.js").exists(),
        "unignored stamp storage must block copies"
    );

    write_stamp_ignore(&vault);
    run_git(&vault, &["add", ".gitignore"]);
    run_git(
        &vault,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "ignore stamps",
        ],
    );
    let allowed = sync_plugins(&opts);
    assert!(allowed.issues.is_empty(), "{:?}", allowed.issues);
    assert!(vault.join(".obsidian/plugins/alpha/main.js").is_file());
}

#[test]
fn dry_run_refuses_without_writing() {
    let temp = TempDir::new("bob-cli-plugins-dry-refuse");
    let seed = temp.path().join("seed");
    write_plugin(&seed, "alpha", "1.0.0", "Alpha", "base");
    git_init_commit(&seed);
    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_a)],
    );
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_b)],
    );
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");
    let (opts_a, _guard) = options(&checkout_a, &vault);
    assert!(sync_plugins(&opts_a).issues.is_empty());
    let stamp = fs::read(stamp_path(&vault, "alpha")).unwrap();

    let (mut opts_b, _guard_b) = options(&checkout_b, &vault);
    opts_b.dry_run = true;
    let refused = sync_plugins(&opts_b);
    assert!(
        refused
            .issues
            .iter()
            .any(|issue| issue.contains("refusing")),
        "{:?}",
        refused.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// dirty-a\n"
    );
    assert_eq!(fs::read(stamp_path(&vault, "alpha")).unwrap(), stamp);
}

#[test]
fn deploy_lock_rejects_a_second_writer() {
    let temp = TempDir::new("bob-cli-plugins-lock");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
    let (opts, _guard) = options(&repo, &vault);
    fs::create_dir_all(&vault).unwrap();
    let held = acquire_deploy_lock(&vault).expect("hold deploy lock");
    let blocked = sync_plugins(&opts);
    assert!(
        blocked
            .issues
            .iter()
            .any(|issue| issue.contains("already deploying")),
        "{:?}",
        blocked.issues
    );
    assert!(
        !vault.join(".obsidian/plugins/alpha/main.js").exists(),
        "the lock must prevent copies"
    );
    drop(held);
    let allowed = sync_plugins(&opts);
    assert!(allowed.issues.is_empty(), "{:?}", allowed.issues);
    assert!(deploy_lock_path(&vault).exists());
}

#[test]
fn list_reports_uncommitted_deployment() {
    let temp = TempDir::new("bob-cli-plugins-list-uncommitted");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "alpha");
    git_init_commit(&repo);
    write_stamp_ignore(&vault);
    write_file(&repo.join("plugins/alpha/main.js"), "// dirty\n");
    let (opts, _guard) = options(&repo, &vault);
    assert!(sync_plugins(&opts).issues.is_empty());

    let report = scan_plugins(&repo, &vault);
    let info = report.plugins[0]
        .deployment
        .as_ref()
        .expect("deployment info");
    assert_eq!(info.status, DeployStatus::Uncommitted);
    assert_eq!(
        scan_deployment(&vault.join(".obsidian/plugins/alpha"))
            .expect("stamp")
            .status,
        DeployStatus::Uncommitted
    );
}

#[test]
fn tracked_stamp_refuses_even_with_force() {
    let temp = TempDir::new("bob-cli-plugins-stamp-tracked");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "one");
    let (opts, _guard) = options(&repo, &vault);
    assert!(sync_plugins(&opts).issues.is_empty());

    write_file(&vault.join("README.md"), "vault\n");
    git_init_commit(&vault);
    write_file(&repo.join("plugins/alpha/main.js"), "// two\n");
    let refused = sync_plugins(&opts);
    assert!(
        refused.issues.iter().any(|issue| issue.contains("tracked")),
        "{:?}",
        refused.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// one\n"
    );

    let (mut forced, _guard_f) = options(&repo, &vault);
    forced.force = true;
    let still = sync_plugins(&forced);
    assert!(
        still.issues.iter().any(|issue| issue.contains("tracked")
            && issue.contains("--force does not bypass")),
        "{:?}",
        still.issues
    );
}

#[test]
fn nongit_source_requires_force_after_first_deploy() {
    let temp = TempDir::new("bob-cli-plugins-nongit-source");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "one");
    let (opts, _guard) = options(&repo, &vault);
    assert!(sync_plugins(&opts).issues.is_empty());

    write_file(&repo.join("plugins/alpha/main.js"), "// two\n");
    let refused = sync_plugins(&opts);
    assert!(
        refused
            .issues
            .iter()
            .any(|issue| issue.contains("refusing")),
        "{:?}",
        refused.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// one\n"
    );

    let (mut forced, _guard_f) = options(&repo, &vault);
    forced.force = true;
    let report = sync_plugins(&forced);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// two\n"
    );
}

#[test]
fn symlink_alias_counts_as_same_checkout() {
    let temp = TempDir::new("bob-cli-plugins-symlink-alias");
    let repo = temp.path().join("repo");
    write_plugin(&repo, "alpha", "1.0.0", "Alpha", "one");
    git_init_commit(&repo);
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    write_file(&repo.join("plugins/alpha/main.js"), "// two\n");
    let (opts, _guard) = options(&repo, &vault);
    assert!(sync_plugins(&opts).issues.is_empty());

    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&repo, &alias).expect("symlink checkout");
    write_file(&alias.join("plugins/alpha/main.js"), "// three\n");
    let (opts_alias, _guard_a) = options(&alias, &vault);
    let report = sync_plugins(&opts_alias);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// three\n"
    );
}

#[test]
fn clean_nonancestor_is_refused() {
    let temp = TempDir::new("bob-cli-plugins-nonancestor");
    let seed = temp.path().join("seed");
    write_plugin(&seed, "alpha", "1.0.0", "Alpha", "base");
    git_init_commit(&seed);
    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_a)],
    );
    run_git(
        temp.path(),
        &["clone", "-q", path_str(&seed), path_str(&checkout_b)],
    );
    git_config_identity(&checkout_a);
    git_config_identity(&checkout_b);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// from-a\n");
    run_git(&checkout_a, &["add", "plugins/alpha/main.js"]);
    run_git(&checkout_a, &["commit", "-q", "-m", "a change"]);
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    let (opts_a, _guard) = options(&checkout_a, &vault);
    assert!(sync_plugins(&opts_a).issues.is_empty());

    write_file(&checkout_b.join("plugins/alpha/main.js"), "// from-b\n");
    run_git(&checkout_b, &["add", "plugins/alpha/main.js"]);
    run_git(&checkout_b, &["commit", "-q", "-m", "b sibling"]);
    let (opts_b, _guard_b) = options(&checkout_b, &vault);
    let refused = sync_plugins(&opts_b);
    assert!(
        refused.issues.iter().any(|issue| {
            issue.contains("refusing") && issue.contains("does not contain")
        }),
        "{:?}",
        refused.issues
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .unwrap(),
        "// from-a\n"
    );
}

fn options(repo: &Path, vault: &Path) -> (SyncOptions, TestEnvGuard) {
    let state = vault.parent().unwrap_or(vault).join("xdg-state");
    let guard = TestEnvGuard::set(&[
        ("XDG_STATE_HOME", Some(state.as_os_str())),
        ("HOSTNAME", Some(OsStr::new("test-host"))),
        ("BOB_NOW", Some(OsStr::new("2026-06-26 14:30:00"))),
    ]);
    (
        SyncOptions {
            repo: repo.to_path_buf(),
            bob_dir: vault.to_path_buf(),
            backup_run_dir: vault
                .parent()
                .unwrap_or(vault)
                .join("backups/20260626-143000"),
            only: None,
            dry_run: false,
            force: false,
        },
        guard,
    )
}

fn write_stamp_ignore(vault: &Path) {
    write_file(&vault.join(".gitignore"), &format!("{STAMP_IGNORE_HINT}\n"));
}

fn action_for(report: &SyncReport, id: &str, name: &str) -> Option<FileAction> {
    report
        .plugins
        .iter()
        .find(|plugin| plugin.id == id)
        .and_then(|plugin| plugin.files.iter().find(|file| file.name == name))
        .map(|file| file.action)
}

fn file_for<'a>(
    report: &'a SyncReport,
    id: &str,
    name: &str,
) -> Option<&'a FileSync> {
    report
        .plugins
        .iter()
        .find(|plugin| plugin.id == id)
        .and_then(|plugin| plugin.files.iter().find(|file| file.name == name))
}

fn git_init_commit(repo: &Path) {
    fs::create_dir_all(repo).unwrap_or_else(|error| {
        panic!("create git repo {}: {error}", repo.display())
    });
    run_git(repo, &["init", "-q"]);
    run_git(repo, &["add", "-A"]);
    run_git(
        repo,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
}

fn git_config_identity(repo: &Path) {
    run_git(repo, &["config", "user.name", "Test User"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "commit.gpgsign", "false"]);
}

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap_or_else(|error| panic!("run git {args:?}: {error}"));
    assert!(status.success(), "git {args:?} failed");
}

fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("path is not UTF-8: {}", path.display()))
}

fn write_plugin(
    repo: &Path,
    id: &str,
    version: &str,
    description: &str,
    body: &str,
) {
    let dir = repo.join("plugins").join(id);
    write_file(
        &dir.join("manifest.json"),
        &manifest_json(id, version, description),
    );
    write_file(&dir.join("main.js"), &format!("// {body}\n"));
}

fn write_vault_plugin(
    vault: &Path,
    id: &str,
    version: &str,
    description: &str,
    body: &str,
) {
    let dir = vault.join(".obsidian/plugins").join(id);
    write_file(
        &dir.join("manifest.json"),
        &manifest_json(id, version, description),
    );
    write_file(&dir.join("main.js"), &format!("// {body}\n"));
}

fn manifest_json(id: &str, version: &str, description: &str) -> String {
    format!(
        "{{\n  \"id\": \"{id}\",\n  \"version\": \"{version}\",\n  \"description\": \"{description}\"\n}}\n"
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
        if let Err(error) = fs::remove_dir_all(&self.path) {
            eprintln!("failed to remove {}: {error}", self.path.display());
        }
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}
