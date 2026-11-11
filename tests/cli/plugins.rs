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

    write_stamp_ignore(&vault);
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
fn plugins_sync_bare_from_enclosing_checkout_deploys_with_notice() {
    let temp = TempDir::new("bob-cli-plugins-sync-enclosing");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_plugins_fixture(&repo, &vault);

    // An enclosing bob-plugins checkout identified by its origin remote.
    // The origin is a local bare repo whose path still contains the
    // `bobs-org/bob-plugins` marker, so no network is needed.
    let origin = temp.path().join("bobs-org/bob-plugins.git");
    fs::create_dir_all(origin.parent().expect("origin parent"))
        .expect("create origin parent");
    git(["init", "-q", "--bare", path_str(&origin)]);
    let foreign = temp.path().join("foreign");
    git(["init", "-q", path_str(&foreign)]);
    configure_test_git_identity(&foreign);
    write_plugin(&foreign.join("plugins/alpha"), "alpha", "9.9.9", "foreign");
    git_in(&foreign, ["add", "."]);
    git_in(&foreign, ["commit", "-q", "-m", "foreign plugins"]);
    git_in(&foreign, ["remote", "add", "origin", path_str(&origin)]);
    let foreign_cwd = foreign.join("plugins/alpha");
    let foreign_canonical = foreign
        .canonicalize()
        .expect("canonicalize foreign checkout");

    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--no-pull")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bare bob plugins sync");

    assert_success(&output);
    let err = stderr(&output);
    assert!(
        err.contains("using enclosing checkout")
            && err.contains(&foreign_canonical.display().to_string())
            && err.contains(&repo.display().to_string())
            && err.contains("--repo"),
        "bare sync must print the enclosing-checkout notice naming both repos:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains(&foreign_canonical.display().to_string()),
        "table header must name the enclosing checkout:\n{out}"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault plugin"),
        "// foreign\n",
        "a bare sync must deploy the enclosing checkout"
    );
    assert_eq!(
        fs::read_to_string(repo.join("plugins/alpha/main.js"))
            .expect("read resolved repo plugin"),
        "// alpha\n",
        "the enclosing sync must not touch the resolved repo"
    );

    // JSON mode reports the same repo with the additive source field.
    // Stdout stays parseable; the notice rides on stderr.
    let json_out = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--no-pull")
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

    assert_success(&json_out);
    assert!(
        stderr(&json_out).contains("using enclosing checkout"),
        "json mode must still print the notice on stderr:\n{}",
        format_output(&json_out)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&json_out))
        .expect("parse enclosing sync json");
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["repo"].as_str().unwrap_or_default(),
        foreign_canonical.to_string_lossy().as_ref()
    );
    assert_eq!(value["repo_source"], "enclosing_checkout");

    // A dry-run preview from the same cwd prints the notice and writes
    // nothing.
    let dry_vault = temp.path().join("dry-vault");
    let dry_backups = temp.path().join("dry-backups");
    write_plugins_fixture(&temp.path().join("dry-fixture"), &dry_vault);
    let dry = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--dry-run")
        .arg("--no-pull")
        .arg("-b")
        .arg(&dry_vault)
        .arg("-B")
        .arg(&dry_backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bare bob plugins sync --dry-run");

    assert_success(&dry);
    assert!(
        stderr(&dry).contains("using enclosing checkout"),
        "dry-run must print the notice:\n{}",
        format_output(&dry)
    );
    assert_eq!(
        fs::read_to_string(dry_vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read dry vault plugin"),
        "// alpha\n",
        "dry-run must not modify the vault"
    );
    assert!(!dry_backups.exists(), "dry-run must not write backups");
}

#[test]
fn plugins_list_bare_from_enclosing_checkout_names_same_repo() {
    let temp = TempDir::new("bob-cli-plugins-list-enclosing");
    let repo = temp.path().join("repo");
    let vault = temp.path().join("vault");
    write_plugins_fixture(&repo, &vault);

    // Marker checkout (no origin): plugins/ plus package.json.
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
    let foreign_cwd = foreign.join("plugins");
    let foreign_canonical = foreign
        .canonicalize()
        .expect("canonicalize foreign checkout");

    // `bob plugins list` from inside the worktree names the same repo.
    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("--no-pull")
        .arg("-b")
        .arg(&vault)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bob plugins list from enclosing checkout");

    assert_success(&output);
    assert!(
        stderr(&output).contains("using enclosing checkout"),
        "list must print the notice:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains(&foreign_canonical.display().to_string()),
        "list header must name the enclosing checkout:\n{}",
        stdout(&output)
    );

    // The bare default (`bob plugins`) agrees, and JSON carries the source.
    let bare = bob_command()
        .arg("plugins")
        .arg("--no-pull")
        .arg("-b")
        .arg(&vault)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bare bob plugins from enclosing checkout");

    assert_success(&bare);
    assert!(
        stdout(&bare).contains(&foreign_canonical.display().to_string()),
        "bare list must name the enclosing checkout:\n{}",
        stdout(&bare)
    );

    let json_out = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("--no-pull")
        .arg("-f")
        .arg("json")
        .arg("-b")
        .arg(&vault)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(&foreign_cwd)
        .output()
        .expect("run bob plugins list -f json from enclosing checkout");

    assert_success(&json_out);
    let value: serde_json::Value = serde_json::from_str(&stdout(&json_out))
        .expect("parse enclosing list json");
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["repo"].as_str().unwrap_or_default(),
        foreign_canonical.to_string_lossy().as_ref()
    );
    assert_eq!(value["repo_source"], "enclosing_checkout");
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
        .arg("--no-pull")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(repo.join("plugins"))
        .output()
        .expect("run bare bob plugins sync from the resolved checkout");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("using enclosing checkout"),
        "no notice when the cwd sits inside the resolved checkout:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/beta/main.js"))
            .expect("read vault plugin"),
        "// beta\n",
        "a bare sync from the resolved checkout must still deploy"
    );

    let json_out = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--no-pull")
        .arg("-f")
        .arg("json")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &repo)
        .current_dir(repo.join("plugins"))
        .output()
        .expect("run bare sync json from the resolved checkout");

    assert_success(&json_out);
    let value: serde_json::Value = serde_json::from_str(&stdout(&json_out))
        .expect("parse resolved sync json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["repo_source"], "env");
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
            .arg("--no-pull")
            .arg("-b")
            .arg(&vault)
            .arg("-B")
            .arg(&backups)
            .env("BOB_PLUGINS_DIR", &repo)
            .current_dir(cwd)
            .output()
            .expect("run bare bob plugins sync from an unrelated cwd");

        assert_success(&output);
        assert!(
            !stderr(&output).contains("using enclosing checkout"),
            "no notice from outside any bob-plugins checkout:\n{}",
            format_output(&output)
        );
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

    // A bare sync from the marker checkout deploys it with a notice.
    let bare = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--no-pull")
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("BOB_PLUGINS_DIR", &canonical)
        .current_dir(&foreign)
        .output()
        .expect("run bare bob plugins sync from a marker checkout");

    assert_success(&bare);
    assert!(
        stderr(&bare).contains("using enclosing checkout"),
        "bare sync must print the notice:\n{}",
        format_output(&bare)
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault plugin"),
        "// foreign\n",
        "a bare sync must deploy the enclosing checkout"
    );

    // An explicit --repo always wins, with no notice and source flag.
    // Use a fresh vault so the deploy-protection guard (which would refuse
    // to overwrite the foreign clean stamp with unrelated canonical history)
    // does not stand in for resolution.
    let flag_vault = temp.path().join("flag-vault");
    let flag_backups = temp.path().join("flag-backups");
    write_plugins_fixture(&temp.path().join("flag-fixture"), &flag_vault);
    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--no-pull")
        .arg("-r")
        .arg(&canonical)
        .arg("-b")
        .arg(&flag_vault)
        .arg("-B")
        .arg(&flag_backups)
        .arg("-f")
        .arg("json")
        .env("BOB_PLUGINS_DIR", &canonical)
        .current_dir(&foreign)
        .output()
        .expect("run bob plugins sync --repo from a foreign checkout");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("using enclosing checkout"),
        "explicit --repo must not print the notice:\n{}",
        format_output(&output)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&output))
        .expect("parse explicit sync json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["repo_source"], "flag");
    assert_eq!(
        fs::read_to_string(flag_vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault plugin"),
        "// alpha\n",
        "an explicit --repo must deploy that checkout even from a foreign cwd"
    );
}

#[test]
fn plugins_sync_refuses_dirty_deploy_from_another_checkout() {
    let temp = TempDir::new("bob-cli-plugins-guard-ab");
    let (checkout_a, checkout_b, vault, backups) =
        init_two_checkouts_and_vault(&temp);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");

    let deployed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("deploy from checkout A");
    assert_success(&deployed);
    let stamp = vault.join(".obsidian/plugins/alpha/.bob-deploy.json");
    let stamp_bytes = fs::read(&stamp).expect("read stamp");
    assert!(
        String::from_utf8_lossy(&stamp_bytes).contains("\"dirty\": \"dirty\"")
    );

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("overwrite from checkout B");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "other-checkout dirty deploy must refuse:\n{}",
        format_output(&refused)
    );
    let err = stderr(&refused);
    assert!(
        err.contains("refusing to overwrite plugin alpha")
            && err.contains("deployed from uncommitted build")
            && err.contains("--dry-run")
            && err.contains("-F"),
        "refusal must name the plugin, uncommitted deploy, and recovery:\n{}",
        format_output(&refused)
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault main"),
        "// dirty-a\n"
    );
    assert_eq!(fs::read(&stamp).expect("reread stamp"), stamp_bytes);

    configure_test_git_identity(&checkout_b);
    write_file(&checkout_b.join("README.md"), "later\n");
    git_in(&checkout_b, ["add", "README.md"]);
    git_in(
        &checkout_b,
        ["commit", "-q", "-m", "descendant without A's dirty"],
    );
    let still = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("descendant without dirty bytes");
    assert_eq!(still.status.code(), Some(1), "{}", format_output(&still));
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("read vault main"),
        "// dirty-a\n"
    );

    let dry = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-d")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("dry-run refusal");
    assert_eq!(dry.status.code(), Some(1), "{}", format_output(&dry));
    assert_eq!(fs::read(&stamp).expect("stamp after dry-run"), stamp_bytes);

    let forced = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-F")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("forced overwrite");
    assert_success(&forced);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("forced vault main"),
        "// old\n"
    );
    let stamp_backup = find_stamp_backup(&backups);
    assert!(
        stamp_backup.is_some(),
        "forced replacement must back up the prior stamp under {}",
        backups.display()
    );
}

#[test]
fn plugins_sync_clean_landing_reconciles_then_updates() {
    let temp = TempDir::new("bob-cli-plugins-guard-land");
    let seed = temp.path().join("seed");
    write_plugin(&seed.join("plugins/alpha"), "alpha", "1.0.0", "base");
    git(["init", "-q", path_str(&seed)]);
    configure_test_git_identity(&seed);
    git_in(&seed, ["add", "."]);
    git_in(&seed, ["commit", "-q", "-m", "seed"]);

    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    git(["clone", "-q", path_str(&seed), path_str(&checkout_a)]);
    configure_test_git_identity(&checkout_a);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// fix\n");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_stamp_ignore(&vault);
    let first = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("deploy A's dirty fix");
    assert_success(&first);

    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "land the fix"]);
    git(["clone", "-q", path_str(&checkout_a), path_str(&checkout_b)]);
    let landed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("clean landing");
    assert_success(&landed);
    let stamp = fs::read_to_string(
        vault.join(".obsidian/plugins/alpha/.bob-deploy.json"),
    )
    .expect("read stamp");
    assert!(
        stamp.contains("\"dirty\": \"clean\""),
        "exact-match landing must reconcile to a clean stamp: {stamp}"
    );
    assert_eq!(landed.status.code(), Some(0));
    let copied_unchanged = stdout(&landed).contains("up to date")
        || stdout(&landed).contains("0 copied")
        || stdout(&landed).contains("0 to copy");
    assert!(
        copied_unchanged || stdout(&landed).contains("unchanged"),
        "landing identical bytes must not count as a content copy:\n{}",
        stdout(&landed)
    );

    configure_test_git_identity(&checkout_b);
    write_file(&checkout_b.join("plugins/alpha/main.js"), "// later\n");
    git_in(&checkout_b, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_b, ["commit", "-q", "-m", "later update"]);
    let updated = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("descendant update");
    assert_success(&updated);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("later vault"),
        "// later\n"
    );
}

#[test]
fn plugins_sync_committed_dirty_deploy_overwrites_without_force() {
    let temp = TempDir::new("bob-cli-plugins-guard-proof-cli");
    let (checkout_a, _checkout_b, vault, backups) =
        init_two_checkouts_and_vault(&temp);
    configure_test_git_identity(&checkout_a);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");
    let deployed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("dirty deploy from checkout A");
    assert_success(&deployed);

    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "land dirty-a"]);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// later\n");
    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "later update"]);
    let proving_output = git_in(&checkout_a, ["rev-parse", "HEAD~1"]);
    let proving = String::from_utf8_lossy(&proving_output.stdout)
        .trim()
        .to_string();
    let checkout_c = temp.path().join("c");
    git(["clone", "-q", path_str(&checkout_a), path_str(&checkout_c)]);

    let json_out = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("-r")
        .arg(&checkout_c)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("proof dry-run json");
    assert_success(&json_out);
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&json_out)).expect("parse json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["plugins"][0]["proving_commit"], proving.as_str());

    let landed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_c)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("proof sync from clean checkout");
    assert_success(&landed);
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("vault after proof"),
        "// later\n"
    );
    let out = stdout(&landed);
    assert!(
        out.contains("inclusion proof") && out.contains(&proving[..7]),
        "sync output must name the proving commit:\n{}",
        format_output(&landed)
    );
}

#[test]
fn plugins_list_and_json_report_uncommitted_deployment() {
    let temp = TempDir::new("bob-cli-plugins-list-uncommitted");
    let repo = temp.path().join("repo");
    write_plugin(&repo.join("plugins/alpha"), "alpha", "1.0.0", "alpha");
    git(["init", "-q", path_str(&repo)]);
    configure_test_git_identity(&repo);
    git_in(&repo, ["add", "."]);
    git_in(&repo, ["commit", "-q", "-m", "clean"]);
    write_file(&repo.join("plugins/alpha/main.js"), "// dirty\n");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_stamp_ignore(&vault);
    let synced = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .arg("-f")
        .arg("json")
        .output()
        .expect("dirty deploy");
    assert_success(&synced);
    let sync_json: serde_json::Value =
        serde_json::from_str(&stdout(&synced)).expect("sync json");
    assert_eq!(sync_json["ok"], true);
    assert_eq!(
        sync_json["plugins"][0]["deployment"]["status"],
        "uncommitted"
    );
    assert_eq!(sync_json["copied"], 2);

    let listed = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-n")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("list after dirty deploy");
    assert_success(&listed);
    let out = stdout(&listed);
    assert!(
        out.contains("deployed from uncommitted build"),
        "list must show uncommitted provenance:\n{out}"
    );

    let listed_json = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("-n")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("list json after dirty deploy");
    assert_success(&listed_json);
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&listed_json)).expect("list json");
    assert_eq!(value["plugins"][0]["deployment"]["status"], "uncommitted");
    assert_eq!(value["plugins"][0]["sync"], "synced");
}

#[test]
fn plugins_sync_git_vault_requires_ignored_stamp() {
    let temp = TempDir::new("bob-cli-plugins-stamp-ignore-cli");
    let repo = temp.path().join("repo");
    write_plugin(&repo.join("plugins/alpha"), "alpha", "1.0.0", "alpha");
    let vault = temp.path().join("vault");
    let backups = temp.path().join("backups");
    write_file(&vault.join("README.md"), "vault\n");
    git(["init", "-q", path_str(&vault)]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "vault"]);

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("sync without stamp ignore");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "{}",
        format_output(&refused)
    );
    assert!(
        stderr(&refused).contains("not ignored")
            || stderr(&refused).contains("tracked"),
        "{}",
        format_output(&refused)
    );
    assert!(
        !vault.join(".obsidian/plugins/alpha/main.js").exists(),
        "unignored stamp storage must block copies"
    );

    write_stamp_ignore(&vault);
    git_in(&vault, ["add", ".gitignore"]);
    git_in(&vault, ["commit", "-q", "-m", "ignore stamps"]);
    let allowed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&repo)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .output()
        .expect("sync with stamp ignore");
    assert_success(&allowed);
    assert!(vault.join(".obsidian/plugins/alpha/main.js").is_file());
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

fn write_stamp_ignore(vault: &Path) {
    write_file(
        &vault.join(".gitignore"),
        ".obsidian/plugins/*/.bob-deploy.json\n",
    );
}

fn init_two_checkouts_and_vault(
    temp: &TempDir,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let seed = temp.path().join("seed");
    write_plugin(&seed.join("plugins/alpha"), "alpha", "1.0.0", "old");
    git(["init", "-q", path_str(&seed)]);
    configure_test_git_identity(&seed);
    git_in(&seed, ["add", "."]);
    git_in(&seed, ["commit", "-q", "-m", "seed"]);
    let checkout_a = temp.path().join("a");
    let checkout_b = temp.path().join("b");
    git(["clone", "-q", path_str(&seed), path_str(&checkout_a)]);
    git(["clone", "-q", path_str(&seed), path_str(&checkout_b)]);
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    let backups = temp.path().join("backups");
    (checkout_a, checkout_b, vault, backups)
}

fn find_stamp_backup(backups: &Path) -> Option<PathBuf> {
    let Ok(runs) = fs::read_dir(backups) else {
        return None;
    };
    for run in runs.flatten() {
        let stamp = run.path().join("alpha/.bob-deploy.json");
        if stamp.is_file() {
            return Some(stamp);
        }
    }
    None
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

fn init_bare_guard_fixture(
    temp: &TempDir,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let remote = temp.path().join("guard-remote.git");
    git(["init", "-q", "--bare", path_str(&remote)]);
    let seed = temp.path().join("guard-seed");
    git(["clone", "-q", path_str(&remote), path_str(&seed)]);
    configure_test_git_identity(&seed);
    write_plugin(&seed.join("plugins/alpha"), "alpha", "1.0.0", "old");
    git_in(&seed, ["add", "."]);
    git_in(&seed, ["commit", "-q", "-m", "seed"]);
    git_in(&seed, ["push", "-q", "-u", "origin", "HEAD"]);
    let checkout_a = temp.path().join("guard-a");
    let checkout_b = temp.path().join("guard-b");
    git(["clone", "-q", path_str(&remote), path_str(&checkout_a)]);
    git(["clone", "-q", path_str(&remote), path_str(&checkout_b)]);
    configure_test_git_identity(&checkout_a);
    configure_test_git_identity(&checkout_b);
    let vault = temp.path().join("vault");
    write_stamp_ignore(&vault);
    let backups = temp.path().join("backups");
    (checkout_a, checkout_b, vault, backups)
}

fn rev_parse_head(repo: &Path) -> String {
    String::from_utf8_lossy(&git_in(repo, ["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string()
}

fn remote_refs_snapshot(repo: &Path) -> String {
    String::from_utf8_lossy(
        &git_in(
            repo,
            ["for-each-ref", "--format=%(objectname)", "refs/remotes"],
        )
        .stdout,
    )
    .into_owned()
}

#[test]
fn plugins_sync_dirty_refusal_names_landed_build() {
    let temp = TempDir::new("bob-cli-plugins-guard-landed");
    let (checkout_a, checkout_b, vault, backups) =
        init_bare_guard_fixture(&temp);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");

    let deployed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env("SASE_AGENT_ID", "agent-a")
        .output()
        .expect("dirty deploy from checkout A");
    assert_success(&deployed);
    let stamp = vault.join(".obsidian/plugins/alpha/.bob-deploy.json");
    let stamp_bytes = fs::read(&stamp).expect("read stamp");

    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "land dirty-a"]);
    git_in(&checkout_a, ["push", "-q", "origin", "HEAD"]);
    let landed_sha = rev_parse_head(&checkout_a);
    let landed_short: String = landed_sha.chars().take(7).collect();
    git_in(&checkout_b, ["fetch", "-q", "origin"]);

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("refuse from stale checkout B");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "landed dirty build must still refuse:\n{}",
        format_output(&refused)
    );
    let err = stderr(&refused);
    assert!(
        err.contains("refusing to overwrite plugin alpha")
            && err.contains("deployed from uncommitted build")
            && err.contains(&landed_short)
            && err.contains("origin/")
            && err.contains("agent-a")
            && err.contains("no -F"),
        "refusal must name the landed commit, its origin ref, and the agent:\n{}",
        format_output(&refused)
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("vault after refusal"),
        "// dirty-a\n"
    );
    assert_eq!(fs::read(&stamp).expect("stamp after refusal"), stamp_bytes);
}

#[test]
fn plugins_sync_landed_recovery_without_force() {
    let temp = TempDir::new("bob-cli-plugins-guard-recover");
    let (checkout_a, checkout_b, vault, backups) =
        init_bare_guard_fixture(&temp);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");
    let deployed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("dirty deploy from checkout A");
    assert_success(&deployed);

    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "land dirty-a"]);
    git_in(&checkout_a, ["push", "-q", "origin", "HEAD"]);
    let landed_sha = rev_parse_head(&checkout_a);
    git_in(&checkout_b, ["fetch", "-q", "origin"]);

    write_file(
        &checkout_b.join("plugins/alpha/manifest.json"),
        "{\n  \"id\": \"alpha\",\n  \"version\": \"9.9.9\",\n  \"description\": \"alpha keeps things tidy\"\n}\n",
    );
    git_in(&checkout_b, ["stash", "push", "-q", "-u", "-m", "b work"]);
    git_in(&checkout_b, ["merge", "-q", "--ff-only", "@{upstream}"]);
    let clean = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("clean reconcile after update");
    assert_success(&clean);
    let stamp_body = fs::read_to_string(
        vault.join(".obsidian/plugins/alpha/.bob-deploy.json"),
    )
    .expect("read reconciled stamp");
    assert!(
        stamp_body.contains("\"dirty\": \"clean\"")
            && stamp_body.contains(&landed_sha),
        "clean sync must reconcile to the landed commit: {stamp_body}"
    );

    git_in(&checkout_b, ["stash", "pop", "-q"]);
    let redeploy = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("redeploy B change without force");
    assert_success(&redeploy);
    assert!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/manifest.json"))
            .expect("vault manifest after redeploy")
            .contains("\"version\": \"9.9.9\""),
        "vault must keep B's manifest change after redeploy"
    );
    assert_eq!(
        fs::read_to_string(vault.join(".obsidian/plugins/alpha/main.js"))
            .expect("vault after redeploy"),
        "// dirty-a\n"
    );
}

#[test]
fn plugins_sync_dirty_refusal_generic_remedy_without_fetch() {
    let temp = TempDir::new("bob-cli-plugins-guard-generic");
    let (checkout_a, checkout_b, vault, backups) =
        init_bare_guard_fixture(&temp);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// dirty-a\n");
    let deployed = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("dirty deploy from checkout A");
    assert_success(&deployed);

    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "land dirty-a"]);
    git_in(&checkout_a, ["push", "-q", "origin", "HEAD"]);
    let landed_sha = rev_parse_head(&checkout_a);
    let landed_short: String = landed_sha.chars().take(7).collect();
    let before = remote_refs_snapshot(&checkout_b);

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("refuse without fetching");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "unfetched landed build must still refuse:\n{}",
        format_output(&refused)
    );
    let err = stderr(&refused);
    assert!(
        err.contains("git fetch") && err.contains("no -F"),
        "generic remedy must point at fetch without force:\n{}",
        format_output(&refused)
    );
    assert!(
        !err.contains(&landed_short) && !err.contains(&landed_sha),
        "generic remedy must not name the unfetched commit:\n{}",
        format_output(&refused)
    );
    assert_eq!(
        remote_refs_snapshot(&checkout_b),
        before,
        "refusal must not fetch or mutate remote-tracking refs"
    );
}

#[test]
fn plugins_sync_clean_refusal_names_landed_revision() {
    let temp = TempDir::new("bob-cli-plugins-guard-clean");
    let (checkout_a, checkout_b, vault, backups) =
        init_bare_guard_fixture(&temp);
    write_file(&checkout_a.join("plugins/alpha/main.js"), "// new\n");
    git_in(&checkout_a, ["add", "plugins/alpha/main.js"]);
    git_in(&checkout_a, ["commit", "-q", "-m", "new revision"]);
    let clean = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_a)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("clean deploy from checkout A");
    assert_success(&clean);
    git_in(&checkout_a, ["push", "-q", "origin", "HEAD"]);
    let landed_sha = rev_parse_head(&checkout_a);
    let landed_short: String = landed_sha.chars().take(7).collect();
    git_in(&checkout_b, ["fetch", "-q", "origin"]);

    let refused = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("-p")
        .arg("alpha")
        .arg("-n")
        .arg("-r")
        .arg(&checkout_b)
        .arg("-b")
        .arg(&vault)
        .arg("-B")
        .arg(&backups)
        .env_remove("SASE_AGENT_ID")
        .output()
        .expect("clean non-ancestor refusal");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "stale clean checkout must refuse:\n{}",
        format_output(&refused)
    );
    let err = stderr(&refused);
    assert!(
        err.contains("refusing to overwrite plugin alpha")
            && err.contains(&landed_short)
            && err.contains("origin/")
            && err.contains("no -F"),
        "clean refusal must name the landed revision on its origin ref:\n{}",
        format_output(&refused)
    );
}
