//! Plugins behavior tests.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[test]
fn plugins_list_renders_table_and_summary() {
    let temp = TempDir::new("bob-cli-plugins-list");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugins_fixture(&repo, &vault);

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins list");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected stderr:\n{}",
        stderr(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Bob Plugins - 3 - "),
        "missing header line:\n{out}"
    );
    assert!(
        out.contains("PLUGIN")
            && out.contains("VERSION")
            && out.contains("SYNC")
            && out.contains("VAULT")
            && out.contains("DESCRIPTION"),
        "missing table header:\n{out}"
    );
    assert!(
        out.contains("alpha")
            && out.contains("synced")
            && out.contains("enabled"),
        "missing synced + enabled alpha row:\n{out}"
    );
    assert!(
        out.contains("beta")
            && out.contains("drift")
            && out.contains("disabled"),
        "missing drift + disabled beta row:\n{out}"
    );
    assert!(
        out.contains("gamma")
            && out.contains("missing")
            && out.contains("not installed"),
        "missing not-installed gamma row:\n{out}"
    );
    assert!(
        out.contains("1 synced - 1 drift - 1 not installed"),
        "unexpected footer summary:\n{out}"
    );
    assert_text_order(&out, &["alpha", "beta", "gamma"]);
}

#[test]
fn plugins_default_subcommand_runs_list() {
    let temp = TempDir::new("bob-cli-plugins-default");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugins_fixture(&repo, &vault);

    let output = bob_command()
        .arg("plugins")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins with no subcommand");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Bob Plugins - 3 - ")
            && out.contains("1 synced - 1 drift - 1 not installed"),
        "bare `bob plugins` should default to list:\n{out}"
    );
}

#[test]
fn plugins_list_json_is_machine_readable() {
    let temp = TempDir::new("bob-cli-plugins-json");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugins_fixture(&repo, &vault);

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plugins list -f json");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse plugins json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["count"], 3);
    assert_eq!(value["synced"], 1);
    assert_eq!(value["drift"], 1);
    assert_eq!(value["not_installed"], 1);
    assert_eq!(value["plugins"][0]["id"], "alpha");
    assert_eq!(value["plugins"][0]["version"], "1.0.0");
    assert_eq!(value["plugins"][0]["sync"], "synced");
    assert_eq!(value["plugins"][0]["vault"], "enabled");
    assert_eq!(value["plugins"][1]["id"], "beta");
    assert_eq!(value["plugins"][1]["sync"], "drift");
    assert_eq!(value["plugins"][1]["vault"], "disabled");
    assert_eq!(value["plugins"][2]["id"], "gamma");
    assert_eq!(value["plugins"][2]["sync"], "missing");
    assert_eq!(value["plugins"][2]["vault"], "not_installed");
}

#[test]
fn plugins_list_pulls_repo_before_analysis() {
    let temp = TempDir::new("bob-cli-plugins-list-pull");
    let (repo, upstream) = init_pullable_plugins_repo(&temp);
    let vault = temp.path().join("vault");
    advance_plugins_remote(&upstream, "new");
    write_plugin(
        &vault.join(".obsidian/plugins/alpha"),
        "alpha",
        "1.0.0",
        "new",
    );

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins list with pull");

    assert_success(&output);
    assert!(
        stdout(&output).contains("alpha") && stdout(&output).contains("synced"),
        "list should analyze the pulled repo checkout:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js"))
            .expect("read pulled plugin"),
        "// new\n"
    );
}

#[test]
fn plugins_list_no_pull_uses_existing_checkout() {
    let temp = TempDir::new("bob-cli-plugins-list-no-pull");
    let (repo, upstream) = init_pullable_plugins_repo(&temp);
    let vault = temp.path().join("vault");
    advance_plugins_remote(&upstream, "new");
    write_plugin(
        &vault.join(".obsidian/plugins/alpha"),
        "alpha",
        "1.0.0",
        "new",
    );

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("--no-pull")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins list --no-pull");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "--no-pull should not run git or warn:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("alpha") && stdout(&output).contains("drift"),
        "list should analyze the existing stale checkout:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js"))
            .expect("read unpulled plugin"),
        "// old\n"
    );
}

#[test]
fn plugins_list_json_stdout_stays_machine_readable_after_pull() {
    let temp = TempDir::new("bob-cli-plugins-json-pull");
    let (repo, upstream) = init_pullable_plugins_repo(&temp);
    let vault = temp.path().join("vault");
    advance_plugins_remote(&upstream, "new");
    write_plugin(
        &vault.join(".obsidian/plugins/alpha"),
        "alpha",
        "1.0.0",
        "new",
    );

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob plugins list -f json with pull");

    assert_success(&output);
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse plugins json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["plugins"][0]["sync"], "synced");
}

#[test]
fn plugins_list_unreadable_repo_reports_error() {
    let temp = TempDir::new("bob-cli-plugins-missing");
    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-r")
        .arg(temp.path().join("does-not-exist"))
        .arg("-b")
        .arg(temp.path())
        .output()
        .expect("run bob plugins list with missing repo");

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unreadable repo should exit 1:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("Bob Plugins - 0 - "),
        "expected an empty table header:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("failed to read plugins directory"),
        "expected a repo read error on stderr:\n{}",
        stderr(&output)
    );
}

#[test]
fn plugins_sync_pulls_repo_before_copying() {
    let temp = TempDir::new("bob-cli-plugins-sync-pull");
    let (repo, upstream) = init_pullable_plugins_repo(&temp);
    let vault = temp.path().join("vault");
    advance_plugins_remote(&upstream, "new");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(temp.path().join("backups"))
        .output()
        .expect("run bob plugins sync with pull");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read synced plugin"),
        "// new\n",
        "sync should copy the pulled repo bytes into the vault"
    );
}

#[test]
fn plugins_sync_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-plugins-sync-dry");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    let beta_main = vault.join(".obsidian/plugins/beta/main.js");
    let beta_backup = backups.join("20260626-143000/beta/main.js");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--dry-run")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-26 14:30:00")
        .output()
        .expect("run bob plugins sync --dry-run");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Bob Plugins - sync - "),
        "missing sync header:\n{out}"
    );
    assert!(
        out.contains("would copy main.js"),
        "expected a dry-run copy preview:\n{out}"
    );
    assert!(out.contains("@@"), "expected a unified diff hunk:\n{out}");
    assert!(
        out.contains("-// beta-stale") && out.contains("+// beta"),
        "expected old and new diff lines:\n{out}"
    );
    assert!(
        out.contains(&format!("would back up to {}", beta_backup.display())),
        "expected dry-run backup path:\n{out}"
    );
    assert!(
        out.contains(&format!(
            "backups would go in {}",
            backups.join("20260626-143000").display()
        )),
        "expected dry-run backup footer:\n{out}"
    );
    assert!(out.contains("to copy"), "expected a dry-run footer:\n{out}");
    assert_eq!(
        fs::read_to_string(&beta_main).expect("read beta main.js"),
        "// beta-stale\n",
        "dry-run must not modify the vault"
    );
    assert!(
        !vault.join(".obsidian/plugins/gamma").exists(),
        "dry-run must not create the missing gamma plugin"
    );
    assert!(!backups.exists(), "dry-run must not create backup files");
}

#[test]
fn plugins_sync_json_reports_file_actions() {
    let temp = TempDir::new("bob-cli-plugins-sync-json");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    let beta_backup = backups.join("20260626-143000/beta/main.js");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-f")
        .arg("json")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-26 14:30:00")
        .output()
        .expect("run bob plugins sync -f json");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        !out.contains("Bob Plugins"),
        "json stdout must not include the table header:\n{out}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&out).expect("parse plugins sync json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["dry_run"], false);
    assert_eq!(value["copied"], 3);
    assert_eq!(value["skipped"], 0);
    assert_eq!(value["unchanged"], 3);

    let beta_main = plugin_file(&value, "beta", "main.js");
    assert_eq!(beta_main["action"], "updated");
    assert_eq!(beta_main["backup"], beta_backup.to_string_lossy().as_ref());

    let gamma_manifest = plugin_file(&value, "gamma", "manifest.json");
    assert_eq!(gamma_manifest["action"], "created");
    assert_eq!(gamma_manifest["backup"], serde_json::Value::Null);
    let gamma_main = plugin_file(&value, "gamma", "main.js");
    assert_eq!(gamma_main["action"], "created");
    assert_eq!(gamma_main["backup"], serde_json::Value::Null);

    assert_eq!(
        plugin_file(&value, "alpha", "manifest.json")["action"],
        "unchanged"
    );
    assert_eq!(
        plugin_file(&value, "alpha", "main.js")["action"],
        "unchanged"
    );
    assert_eq!(
        plugin_file(&value, "beta", "manifest.json")["action"],
        "unchanged"
    );

    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read beta main.js"),
        "// beta\n",
        "beta should be synced from the repo"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/gamma/main.js"))
            .expect("read gamma main.js"),
        "// gamma\n",
        "gamma should be created from the repo"
    );
    assert_eq!(
        fs::read_to_string(&beta_backup).expect("read beta backup"),
        "// beta-stale\n",
        "backup should contain the overwritten vault contents"
    );
}

#[test]
fn plugins_sync_dry_run_json_writes_nothing() {
    let temp = TempDir::new("bob-cli-plugins-sync-dry-json");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    let beta_main = vault.join(".obsidian/plugins/beta/main.js");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-26 14:30:00")
        .output()
        .expect("run bob plugins sync -d -f json");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let value: serde_json::Value = serde_json::from_str(&stdout(&output))
        .expect("parse plugins sync json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["copied"], 3);
    assert_eq!(
        fs::read_to_string(&beta_main).expect("read beta main.js"),
        "// beta-stale\n",
        "dry-run must not modify the vault"
    );
    assert!(
        !vault.join(".obsidian/plugins/gamma").exists(),
        "dry-run must not create the missing gamma plugin"
    );
    assert!(!backups.exists(), "dry-run must not create backup files");
}

#[test]
fn plugins_sync_json_reports_errors_as_object() {
    let temp = TempDir::new("bob-cli-plugins-sync-json-error");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugins_fixture(&repo, &vault);

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("nope")
        .arg("-f")
        .arg("json")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins sync -p nope -f json");

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unknown plugin should exit 1:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "json errors must not also echo to stderr:\n{}",
        stderr(&output)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&output))
        .expect("parse plugins sync json");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "plugin not found in repo: nope");
}

#[test]
fn plugins_sync_backs_up_overwritten_file() {
    let temp = TempDir::new("bob-cli-plugins-sync-backup");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    let beta_backup = backups.join("20260626-143000/beta/main.js");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("beta")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-26 14:30:00")
        .output()
        .expect("run bob plugins sync -p beta");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("@@")
            && out.contains("-// beta-stale")
            && out.contains("+// beta"),
        "expected real sync diff:\n{out}"
    );
    assert!(
        out.contains(&format!("backed up to {}", beta_backup.display())),
        "expected backup path:\n{out}"
    );
    assert!(
        out.contains(&format!(
            "backups in {}",
            backups.join("20260626-143000").display()
        )),
        "expected backup footer:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(&beta_backup).expect("read beta backup"),
        "// beta-stale\n",
        "backup should contain the overwritten vault contents"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read beta main.js"),
        "// beta\n",
        "beta should be synced from the repo"
    );
}

#[test]
fn plugins_sync_single_plugin_copies_only_that_plugin() {
    let temp = TempDir::new("bob-cli-plugins-sync-one");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("beta")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins sync -p beta");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read beta main.js"),
        "// beta\n",
        "beta should be synced from the repo"
    );
    assert!(
        !vault.join(".obsidian/plugins/gamma").exists(),
        "a single-plugin sync must not touch other plugins"
    );
}

#[test]
fn plugins_sync_preserves_runtime_data_json() {
    let temp = TempDir::new("bob-cli-plugins-sync-data");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    let data_json = vault.join(".obsidian/plugins/beta/data.json");
    write_file(&data_json, "{\"setting\":true}\n");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("beta")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins sync -p beta");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&data_json).expect("read data.json"),
        "{\"setting\":true}\n",
        "data.json is a runtime file and must never be synced"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read beta main.js"),
        "// beta\n",
        "managed files should still be synced"
    );
}

#[test]
fn plugins_sync_refuses_dirty_vault_file_then_forces() {
    let temp = TempDir::new("bob-cli-plugins-sync-dirty");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");

    let manifest =
        "{\n  \"id\": \"delta\",\n  \"version\": \"1.0.0\",\n  \"description\": \"delta\"\n}\n";
    write_file(&repo.join("plugins/delta/manifest.json"), manifest);
    write_file(&repo.join("plugins/delta/main.js"), "// repo\n");
    write_file(
        &vault.join(".obsidian/plugins/delta/manifest.json"),
        manifest,
    );
    let vault_main = vault.join(".obsidian/plugins/delta/main.js");
    write_file(&vault_main, "// committed\n");

    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.name", "Test User"]);
    git_in(&vault, ["config", "user.email", "test@example.com"]);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);

    // Locally edit the vault copy so it is dirty in Git.
    write_file(&vault_main, "// dirty\n");

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("delta")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins sync -p delta");

    assert_success(&refused);
    let out = stdout(&refused);
    assert!(
        out.contains("skipped main.js") && out.contains("dirty"),
        "expected a dirty-file refusal warning:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(&vault_main).expect("read delta main.js"),
        "// dirty\n",
        "a dirty vault file must not be overwritten without --force"
    );

    let forced = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("delta")
        .arg("-F")
        .arg("-B")
        .arg(&backups)
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob plugins sync -p delta -F");

    assert_success(&forced);
    assert_eq!(
        fs::read_to_string(&vault_main).expect("read delta main.js"),
        "// repo\n",
        "--force must overwrite the dirty vault file"
    );
}

#[test]
fn plugins_sync_bare_from_foreign_checkout_refuses_before_pull_or_copy() {
    let temp = TempDir::new("bob-cli-plugins-sync-guard-foreign");
    let (repo, upstream) = init_pullable_plugins_repo(&temp);
    advance_plugins_remote(&upstream, "new");
    let fixture = temp.path().join("fixture");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&fixture, &vault);

    // A second bob-plugins checkout, identified by its origin remote, that
    // the bare sync must not deploy over.
    let foreign = temp.path().join("foreign");
    git(["init", "-q", path_str(&foreign)]);
    configure_test_git_identity(&foreign);
    write_plugin(&foreign.join("plugins/alpha"), "alpha", "9.9.9", "foreign");
    git_in(&foreign, ["add", "."]);
    git_in(&foreign, ["commit", "-q", "-m", "foreign plugins"]);
    git_in(
        &foreign,
        [
            "remote",
            "add",
            "origin",
            "https://github.com/bobs-org/bob-plugins.git",
        ],
    );
    let foreign_cwd = foreign.join("plugins/alpha");
    let foreign_canonical = foreign
        .canonicalize()
        .expect("canonicalize foreign checkout");

    // Both the real sync and its preview refuse.
    for extra in [vec![], vec!["--dry-run"]] {
        let mut command = bob_command();
        command
            .arg("plugins")
            .arg("sync")
            .arg("-b")
            .arg(&vault)
            .arg("-B")
            .arg(&backups)
            .env("BOB_PLUGINS_DIR", &repo)
            .current_dir(&foreign_cwd);
        for arg in &extra {
            command.arg(arg);
        }
        let output = command.output().expect("run bare bob plugins sync");

        assert_eq!(
            output.status.code(),
            Some(2),
            "a bare sync from a foreign checkout must refuse:\n{}",
            format_output(&output)
        );
        let err = stderr(&output);
        assert!(
            err.contains("refusing bare sync"),
            "expected a refusal on stderr:\n{}",
            format_output(&output)
        );
        assert!(
            err.contains(&repo.display().to_string())
                && err.contains(&foreign_canonical.display().to_string()),
            "the refusal must name both checkouts:\n{}",
            format_output(&output)
        );
        assert!(
            err.contains("--repo"),
            "the refusal must name the flag to run instead:\n{}",
            format_output(&output)
        );
    }

    let refused_json = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-f")
        .arg("json")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bare bob plugins sync -f json");

    assert_eq!(
        refused_json.status.code(),
        Some(2),
        "a refused bare sync must stay nonzero in json mode:\n{}",
        format_output(&refused_json)
    );
    assert!(
        stderr(&refused_json).is_empty(),
        "json errors must not also echo to stderr:\n{}",
        format_output(&refused_json)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&refused_json))
        .expect("parse refused sync json");
    assert_eq!(value["ok"], false);
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("refusing bare sync"),
        "json error must carry the refusal:\n{value}"
    );

    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js"))
            .expect("read resolved repo plugin"),
        "// old\n",
        "the refused sync must not pull the resolved repo"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read vault plugin"),
        "// beta-stale\n",
        "the refused sync must not touch the vault"
    );
    assert!(
        !vault.join(".obsidian/plugins/gamma").exists(),
        "the refused sync must not create the missing gamma plugin"
    );
    assert!(!backups.exists(), "the refused sync must not write backups");
}

#[test]
fn plugins_sync_bare_from_resolved_checkout_is_allowed() {
    let temp = TempDir::new("bob-cli-plugins-sync-guard-resolved");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);
    git(["init", "-q", path_str(&repo)]);
    configure_test_git_identity(&repo);
    git_in(&repo, ["add", "."]);
    git_in(&repo, ["commit", "-q", "-m", "resolved plugins"]);

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(repo.join("plugins"))
        .output()
        .expect("run bare bob plugins sync from the resolved checkout");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read vault plugin"),
        "// beta\n",
        "a bare sync from the resolved checkout must still deploy"
    );
}

#[test]
fn plugins_sync_bare_from_unrelated_cwd_is_allowed() {
    let temp = TempDir::new("bob-cli-plugins-sync-guard-unrelated");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);

    // A plain directory outside any checkout.
    let unrelated = temp.path().join("unrelated");
    fs::create_dir_all(&unrelated).expect("create unrelated cwd");

    // A Git checkout that is not a bob-plugins checkout.
    let other = temp.path().join("other");
    git(["init", "-q", path_str(&other)]);
    configure_test_git_identity(&other);
    write_file(&other.join("README.md"), "# other\n");
    git_in(&other, ["add", "."]);
    git_in(&other, ["commit", "-q", "-m", "other repo"]);
    git_in(
        &other,
        [
            "remote",
            "add",
            "origin",
            "https://github.com/example/other.git",
        ],
    );

    for cwd in [&unrelated, &other] {
        let output = bob_command()
            .arg("plugins")
            .arg("sync")
            .arg("-b")
            .arg(&vault)
            .arg("-B")
            .arg(&backups)
            .env("BOB_PLUGINS_DIR", &repo)
            .current_dir(cwd)
            .output()
            .expect("run bare bob plugins sync from an unrelated cwd");

        assert_success(&output);
    }
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read vault plugin"),
        "// beta\n",
        "a bare sync from an unrelated cwd must still deploy"
    );
}

#[test]
fn plugins_sync_explicit_repo_from_foreign_checkout_is_allowed() {
    let temp = TempDir::new("bob-cli-plugins-sync-guard-explicit");
    let canonical = temp.path().join("canonical");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&canonical, &vault);

    // A foreign checkout identified by the repo-root marker alone: no origin
    // remote, but a plugins/ dir plus the monorepo package.json.
    let foreign = temp.path().join("foreign");
    git(["init", "-q", path_str(&foreign)]);
    configure_test_git_identity(&foreign);
    write_plugin(&foreign.join("plugins/alpha"), "alpha", "9.9.9", "foreign");
    write_file(
        &foreign.join("package.json"),
        "{\"name\":\"bob-plugins\"}\n",
    );
    git_in(&foreign, ["add", "."]);
    git_in(&foreign, ["commit", "-q", "-m", "foreign plugins"]);

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &canonical)
        .current_dir(&foreign)
        .output()
        .expect("run bare bob plugins sync from a marker checkout");

    assert_eq!(
        refused.status.code(),
        Some(2),
        "a bare sync from a marker checkout must refuse:\n{}",
        format_output(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing bare sync"),
        "expected a refusal on stderr:\n{}",
        format_output(&refused)
    );

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-r")
        .arg(&foreign)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &canonical)
        .current_dir(&foreign)
        .output()
        .expect("run bob plugins sync --repo from a foreign checkout");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault plugin"),
        "// foreign\n",
        "an explicit --repo must deploy the foreign checkout"
    );
}

/// Writes a three-plugin repo and matching vault exercising every state:
/// `alpha` is synced and enabled, `beta` drifts and is disabled, and `gamma`
/// is absent from the vault (not installed).
fn write_plugins_fixture(repo: &Path, vault: &Path) {
    write_plugin(&repo.join("plugins/alpha"), "alpha", "1.0.0", "alpha");
    write_plugin(&repo.join("plugins/beta"), "beta", "2.0.0", "beta");
    write_plugin(&repo.join("plugins/gamma"), "gamma", "1.5.0", "gamma");

    // alpha matches the repo byte-for-byte; beta's main.js diverges.
    write_plugin(
        &vault.join(".obsidian/plugins/alpha"),
        "alpha",
        "1.0.0",
        "alpha",
    );
    write_plugin(
        &vault.join(".obsidian/plugins/beta"),
        "beta",
        "2.0.0",
        "beta-stale",
    );
    write_file(
        &vault.join(".obsidian/community-plugins.json"),
        "[\"alpha\"]\n",
    );
}

fn plugin_file<'a>(
    value: &'a serde_json::Value,
    id: &str,
    name: &str,
) -> &'a serde_json::Value {
    value["plugins"]
        .as_array()
        .expect("plugins array")
        .iter()
        .find(|plugin| plugin["id"] == id)
        .unwrap_or_else(|| panic!("missing plugin {id}"))["files"]
        .as_array()
        .expect("files array")
        .iter()
        .find(|file| file["name"] == name)
        .unwrap_or_else(|| panic!("missing file {id}/{name}"))
}

fn write_plugin(dir: &Path, id: &str, version: &str, body: &str) {
    write_file(
        &dir.join("manifest.json"),
        &format!(
            "{{\n  \"id\": \"{id}\",\n  \"version\": \"{version}\",\n  \"description\": \"{id} keeps things tidy\"\n}}\n"
        ),
    );
    write_file(&dir.join("main.js"), &format!("// {body}\n"));
}

fn init_pullable_plugins_repo(temp: &TempDir) -> (PathBuf, PathBuf) {
    let remote = temp.path().join("plugins-remote.git");
    let seed = temp.path().join("plugins-seed");
    let repo = temp.path().join("repo");
    let upstream = temp.path().join("plugins-upstream");

    git(["init", "-q", "--bare", path_str(&remote)]);
    git(["clone", "-q", path_str(&remote), path_str(&seed)]);
    configure_test_git_identity(&seed);
    write_plugin(&seed.join("plugins/alpha"), "alpha", "1.0.0", "old");
    git_in(&seed, ["add", "."]);
    git_in(&seed, ["commit", "-q", "-m", "initial plugins"]);
    git_in(&seed, ["push", "-q", "-u", "origin", "HEAD"]);

    git(["clone", "-q", path_str(&remote), path_str(&repo)]);
    git(["clone", "-q", path_str(&remote), path_str(&upstream)]);
    configure_test_git_identity(&upstream);
    (repo, upstream)
}

fn advance_plugins_remote(upstream: &Path, body: &str) {
    write_file(
        &upstream.join("plugins/alpha/main.js"),
        &format!("// {body}\n"),
    );
    git_in(upstream, ["add", "."]);
    git_in(upstream, ["commit", "-q", "-m", "update alpha"]);
    git_in(upstream, ["push", "-q"]);
}
