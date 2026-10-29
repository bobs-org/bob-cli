//! `bob types rename`: previewed, verified, single-commit global rename.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-24 12:00:00";

#[test]
fn wrong_vault_refuses_before_syncing_or_renaming() {
    let temp = TempDir::new("bob-cli-themes-rename-wrong-vault");
    let (vault, _) = rename_vault(temp.path());
    assert_vault_override_refused(
        bob_command().args(["themes", "rename", "FIXES", "REPAIRS"]),
        &vault,
    );
}

fn rename_vault(
    root: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(cache.clone()).unwrap();
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: aqua\n---\n",
    );
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n\
- [x] (0800-0830 30m) — BOB\n\
- [x] (0900-0930 30m) — FIXES\n\
- [x] (1000-1030 30m) — FIX + FIXES\n\
- [ ] () — GTD\n",
    );
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — FIXES\n- [x] (0800-0830 30m) — GTD\n",
    );
    write_file(
        &vault.join("2026/20261101.md"),
        "## Pomodoros\n- [ ] () — FIXES\n",
    );
    write_file(
        &vault.join("_templates/daily.md"),
        "## Pomodoros\n- [ ] () — FIXES\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

fn rename_command(
    vault: &std::path::Path,
    cache: &std::path::Path,
) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
}

fn rename_json(
    vault: &std::path::Path,
    cache: &std::path::Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let mut command = rename_command(vault, cache);
    command
        .args(["themes", "rename"])
        .args(args)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("bob types rename");
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("rename JSON");
    (output, value)
}

#[test]
fn themes_rename_dry_run_previews_collapse_and_leaves_vault_untouched() {
    let temp = TempDir::new("bob-cli-themes-rename-dry");
    let (vault, cache) = rename_vault(temp.path());

    let output = rename_command(&vault, &cache)
        .args(["themes", "rename", "FIXES", "FIX", "-d", "--merge"])
        .output()
        .expect("dry run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("FIXES → FIX"), "preview:\n{text}");
    assert!(text.contains("2026/20261020.md"), "file:\n{text}");
    assert!(text.contains("_templates/daily.md"), "template:\n{text}");

    // Dry run writes nothing: no journal, no git commit.
    assert!(!vault.join(".bob/themes/apollo.jsonl").exists());
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 1);

    // JSON preview carries files, tails, state, and color.
    let (_, value) =
        rename_json(&vault, &cache, &["FIXES", "FIX", "-d", "--merge"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["mode"], "dry_run");
    assert_eq!(value["from"]["key"], "fixes");
    assert_eq!(value["to"]["key"], "fix");
    let files = value["files"].as_array().expect("files");
    assert_eq!(files.len(), 4);
    let collapsed = files
        .iter()
        .find(|file| file["path"] == "2026/20261020.md")
        .expect("20261020");
    let tails = collapsed["tails"].as_array().expect("tails");
    assert!(tails.iter().any(|tail| {
        tail["before"] == "FIX + FIXES" && tail["after"] == "FIX"
    }));
}

#[test]
fn themes_alias_matches_types_for_rename_dry_run() {
    let temp = TempDir::new("bob-cli-themes-rename-alias-parity");
    let (vault, cache) = rename_vault(temp.path());

    let alias = rename_command(&vault, &cache)
        .args(["themes", "rename", "FIXES", "FIX", "-d"])
        .output()
        .expect("bob themes rename dry-run");
    let canonical = rename_command(&vault, &cache)
        .args(["types", "rename", "FIXES", "FIX", "-d"])
        .output()
        .expect("bob types rename dry-run");

    assert_eq!(alias.status.code(), canonical.status.code());
    assert_eq!(alias.stdout, canonical.stdout);
    assert_eq!(alias.stderr, canonical.stderr);
}

#[test]
fn themes_rename_apply_verifies_commits_and_reappears() {
    let temp = TempDir::new("bob-cli-themes-rename-apply");
    let (vault, cache) = rename_vault(temp.path());

    let output = rename_command(&vault, &cache)
        .args(["themes", "rename", "FIXES", "FIX", "--merge", "-o"])
        .output()
        .expect("apply");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("committed"), "commit:\n{text}");
    assert!(text.contains("sync: pre skipped, post skipped"), "{text}");

    // Files rewritten, template included.
    let day20 = fs::read_to_string(vault.join("2026/20261020.md")).unwrap();
    assert!(day20.contains("— FIX\n"), "{day20}");
    assert!(!day20.contains("FIXES"), "{day20}");
    let template =
        fs::read_to_string(vault.join("_templates/daily.md")).unwrap();
    assert!(template.contains("— FIX"), "{template}");

    // Future note rewritten.
    let future = fs::read_to_string(vault.join("2026/20261101.md")).unwrap();
    assert!(future.contains("— FIX"), "{future}");

    // Journal appended.
    let journal =
        fs::read_to_string(vault.join(".bob/themes/apollo.jsonl")).unwrap();
    assert!(journal.contains("\"kind\":\"theme.rename\""), "{journal}");
    assert!(journal.contains("\"from\":\"fixes\""), "{journal}");
    assert!(journal.contains("\"to\":\"fix\""), "{journal}");

    // One commit for files plus journal.
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 2);
    let show = git_in(&vault, ["show", "--stat", "--oneline", "HEAD"]);
    let stat = stdout(&show);
    assert!(stat.contains("2026/20261020.md"), "{stat}");
    assert!(stat.contains("_templates/daily.md"), "{stat}");
    assert!(stat.contains(".bob/themes/apollo.jsonl"), "{stat}");

    // Verify: OLD absent from a rescan, entry counts unchanged.
    let listed = rename_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("list");
    assert_success(&listed);
    let value: Value = serde_json::from_str(stdout(&listed).trim()).unwrap();
    let keys: Vec<&str> = value["themes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|theme| theme["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"fix"), "{keys:?}");
    assert!(!keys.contains(&"fixes"), "{keys:?}");

    // Reappearance: FIXES returns, list flags it without rewriting.
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — FIX\n- [x] (0800-0830 30m) — GTD\n- [ ] () — FIXES\n",
    );
    let reappeared = rename_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("reappeared");
    assert_success(&reappeared);
    let value: Value =
        serde_json::from_str(stdout(&reappeared).trim()).unwrap();
    let flagged = value["renamed_reappeared"].as_array().unwrap();
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert_eq!(flagged[0]["key"], "fixes");
    assert_eq!(flagged[0]["renamed_to"], "fix");

    // git revert restores every byte (discard the reappearance probe first).
    git_in(&vault, ["checkout", "--", "2026/20261024.md"]);
    let before = git_in(&vault, ["rev-parse", "HEAD"]);
    let sha = stdout(&before).trim().to_string();
    git_in(&vault, ["revert", "--no-edit", &sha]);
    let restored = fs::read_to_string(vault.join("2026/20261020.md")).unwrap();
    assert!(restored.contains("FIXES"), "{restored}");
    assert!(restored.contains("FIX + FIXES"), "{restored}");
}

#[test]
fn themes_rename_leaves_fenced_prose_and_tasks_untouched() {
    let temp = TempDir::new("bob-cli-themes-rename-fenced");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(cache.clone()).unwrap();
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n\
- [x] (0800-0830 30m) — FOO\n\
```md\n\
- [ ] () — FOO\n\
```\n\
Prose FOO should stay.\n\
- [ ] plain checklist — FOO stays\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let output = rename_command(&vault, &cache)
        .args(["themes", "rename", "FOO", "BAR", "-o"])
        .output()
        .expect("rename");
    assert_success(&output);
    let contents = fs::read_to_string(vault.join("2026/20261020.md")).unwrap();
    assert!(
        contents.contains("- [x] (0800-0830 30m) — BAR"),
        "{contents}"
    );
    assert!(contents.contains("- [ ] () — FOO\n```"), "{contents}");
    assert!(contents.contains("Prose FOO should stay."), "{contents}");
    assert!(
        contents.contains("- [ ] plain checklist — FOO stays"),
        "{contents}"
    );
}

#[test]
fn themes_rename_refusals_are_json_on_nonzero_exit() {
    let temp = TempDir::new("bob-cli-themes-rename-refusals");
    let (vault, cache) = rename_vault(temp.path());

    // unknown_theme
    let (output, value) = rename_json(&vault, &cache, &["NOPE", "NEW", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unknown_theme");

    // invalid_name: NEW contains +
    let (output, value) = rename_json(&vault, &cache, &["FIXES", "A+B", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "invalid_name");

    // unchanged
    let (output, value) =
        rename_json(&vault, &cache, &["FIXES", "fixes", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unchanged");

    // theme_exists without --merge (FIX already used in FIX + FIXES)
    let (output, value) = rename_json(&vault, &cache, &["FIXES", "FIX", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "theme_exists");

    // config_reference: GTD is explicitly exempt in this config.
    let config = cache.join("gtd-exempt.yml");
    write_file(&config, "plan:\n  exempt: [GTD]\n");
    let mut command = rename_command(&vault, &cache);
    command
        .env("BOB_CONFIG_FILE", &config)
        .args(["themes", "rename", "GTD", "NEWGTD", "-d", "-f", "json"]);
    let output = command.output().expect("GTD config-reference JSON");
    let value: Value = serde_json::from_str(stdout(&output).trim())
        .expect("GTD config-reference JSON parses");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "config_reference");

    // dirty_paths offline
    let dirty_path = vault.join("2026/20261020.md");
    let dirty = fs::read_to_string(&dirty_path).unwrap() + "\ndirty\n";
    write_file(&dirty_path, &dirty);
    let (output, value) =
        rename_json(&vault, &cache, &["FIXES", "FIX", "--merge", "-o"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "dirty_paths");
    git_in(&vault, ["checkout", "--", "2026/20261020.md"]);

    // not_a_worktree
    let plain = temp.path().join("plain");
    let plain_vault = plain.join("vault");
    let plain_cache = plain.join("cache");
    fs::create_dir_all(plain_vault.join("2026")).unwrap();
    fs::create_dir_all(&plain_cache).unwrap();
    write_file(
        &plain_vault.join("2026/20261020.md"),
        "## Pomodoros\n- [ ] () — FOO\n",
    );
    let (output, value) =
        rename_json(&plain_vault, &plain_cache, &["FOO", "BAR", "-o"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "not_a_worktree");
}

#[test]
fn themes_rename_lock_busy_when_maintenance_lock_held() {
    let temp = TempDir::new("bob-cli-themes-rename-lock");
    let (vault, cache) = rename_vault(temp.path());
    let lock = temp.path().join("bob_sync.lock");
    let held = hold_vault_lock(&lock);

    let output = rename_command(&vault, &cache)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args([
            "themes", "rename", "FIXES", "FIX", "--merge", "-o", "-f", "json",
        ])
        .output()
        .expect("lock busy");
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "lock_busy");
    assert!(!vault.join(".bob/themes/apollo.jsonl").exists());
    drop(held);
}

fn listed_reappeared(
    vault: &std::path::Path,
    cache: &std::path::Path,
) -> Vec<Value> {
    let output = rename_command(vault, cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("list reappeared");
    assert_success(&output);
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    value["renamed_reappeared"].as_array().unwrap().clone()
}

fn add_old_theme_back(vault: &std::path::Path) {
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — FIX\n- [x] (0800-0830 30m) — GTD\n- [ ] () — FIXES\n",
    );
}

#[test]
fn themes_rename_cycle_does_not_report_reappearance() {
    let temp = TempDir::new("bob-cli-themes-rename-cycle");
    let (vault, cache) = rename_vault(temp.path());
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "rename", "FIXES", "FIX", "--merge", "-o"])
            .output()
            .expect("rename out"),
    );
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "rename", "FIX", "FIXES", "--merge", "-o"])
            .output()
            .expect("rename back"),
    );
    let flagged = listed_reappeared(&vault, &cache);
    assert!(flagged.is_empty(), "{flagged:?}");
}

#[test]
fn themes_rename_later_color_or_unhide_clears_reappearance() {
    let temp = TempDir::new("bob-cli-themes-rename-readopt-color");
    let (vault, cache) = rename_vault(temp.path());
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "rename", "FIXES", "FIX", "--merge", "-o"])
            .output()
            .expect("rename"),
    );
    add_old_theme_back(&vault);
    let flagged = listed_reappeared(&vault, &cache);
    assert_eq!(flagged.len(), 1, "{flagged:?}");

    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "color", "FIXES", "red"])
            .output()
            .expect("color"),
    );
    let flagged = listed_reappeared(&vault, &cache);
    assert!(flagged.is_empty(), "color should re-adopt: {flagged:?}");
}

#[test]
fn themes_rename_later_unhide_clears_reappearance() {
    let temp = TempDir::new("bob-cli-themes-rename-readopt-unhide");
    let (vault, cache) = rename_vault(temp.path());
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "rename", "FIXES", "FIX", "--merge", "-o"])
            .output()
            .expect("rename"),
    );
    add_old_theme_back(&vault);
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "hide", "FIXES"])
            .output()
            .expect("hide"),
    );
    let flagged = listed_reappeared(&vault, &cache);
    assert_eq!(flagged.len(), 1, "hide must not re-adopt: {flagged:?}");
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "unhide", "FIXES"])
            .output()
            .expect("unhide"),
    );
    let flagged = listed_reappeared(&vault, &cache);
    assert!(flagged.is_empty(), "unhide should re-adopt: {flagged:?}");
}

#[test]
fn themes_rename_later_hide_does_not_clear_reappearance() {
    let temp = TempDir::new("bob-cli-themes-rename-hide");
    let (vault, cache) = rename_vault(temp.path());
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "rename", "FIXES", "FIX", "--merge", "-o"])
            .output()
            .expect("rename"),
    );
    add_old_theme_back(&vault);
    assert_success(
        &rename_command(&vault, &cache)
            .args(["themes", "hide", "FIXES"])
            .output()
            .expect("hide"),
    );
    let flagged = listed_reappeared(&vault, &cache);
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert_eq!(flagged[0]["key"], "fixes");
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
fn themes_rename_preserves_trailing_whitespace_and_mixed_endings_offline() {
    let temp = TempDir::new("bob-cli-themes-rename-trailing-whitespace");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: aqua\n---\n",
    );
    // Open and completed entries carry trailing spaces/tabs, with mixed
    // line endings and a changed final row without a newline. autocrlf
    // off keeps the fixture and committed bytes exact.
    let before: &[u8] = b"## Pomodoros\r\n- [ ] () \xe2\x80\x94 JOB \t\r\n- [x] (0800-0830 30m) \xe2\x80\x94 JOB  \n- [ ] () \xe2\x80\x94 JOB\t";
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::write(vault.join("2026/20261020.md"), before).unwrap();
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["config", "core.autocrlf", "false"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (output, value) = rename_json(&vault, &cache, &["JOB", "WRITE", "-o"]);
    assert_success(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["mode"], "write");
    assert_eq!(value["files"][0]["entries"], 3);
    assert_eq!(value["files"][0]["tails"].as_array().unwrap().len(), 3);

    // All bytes outside the component spans survive, including trailing
    // whitespace and each line's original terminator.
    let after = fs::read(vault.join("2026/20261020.md")).unwrap();
    let expected: &[u8] = b"## Pomodoros\r\n- [ ] () \xe2\x80\x94 WRITE \t\r\n- [x] (0800-0830 30m) \xe2\x80\x94 WRITE  \n- [ ] () \xe2\x80\x94 WRITE\t";
    assert_eq!(after, expected);

    // Transaction semantics hold: one commit plus the journal event.
    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 2);
    let subject = git_in(&vault, ["log", "-1", "--format=%s"]);
    assert_eq!(stdout(&subject).trim(), "bob types rename: JOB → WRITE");
    let journal =
        fs::read_to_string(vault.join(".bob/themes/apollo.jsonl")).unwrap();
    assert_eq!(journal.lines().count(), 1, "{journal}");
    assert!(journal.contains("\"kind\":\"theme.rename\""), "{journal}");
    assert!(journal.contains("\"from\":\"job\""), "{journal}");
    assert!(journal.contains("\"to\":\"write\""), "{journal}");
}

#[test]
fn themes_rename_refuses_configured_old_and_warns_unconfigured_new() {
    let temp = TempDir::new("bob-cli-themes-rename-configured");
    let (vault, cache) = rename_vault(temp.path());
    let config = cache.join("config.yml");
    write_file(
        &config,
        "plan:\n  max_themes: 30\nthemes:\n  BUILD: yellow\n  FIXES: red\n",
    );

    let (output, value) = {
        let mut command = rename_command(&vault, &cache);
        command
            .env("BOB_CONFIG_FILE", &config)
            .args(["themes", "rename", "FIXES", "SNACK", "-d", "-f", "json"]);
        let output = command.output().expect("rename configured");
        let value: Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        (output, value)
    };
    assert_ne!(output.status.code(), Some(0), "{value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "config_reference");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("configured type"),
        "{value}"
    );

    write_file(
        &config,
        "plan:\n  max_themes: 30\nthemes:\n  BUILD: yellow\n",
    );
    let mut command = rename_command(&vault, &cache);
    command
        .env("BOB_CONFIG_FILE", &config)
        .args(["themes", "rename", "FIXES", "SNACK", "-d", "-f", "json"]);
    let warned = command.output().expect("rename unconfigured");
    assert_success(&warned);
    let warned_value: Value =
        serde_json::from_str(stdout(&warned).trim()).expect("json");
    assert_eq!(warned_value["ok"], true);
    let warnings = warned_value["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap()
            .contains("SNACK isn't one of your types")),
        "{warnings:?}"
    );

    write_file(
        &config,
        "plan:\n  max_themes: 30\nthemes:\n  FIXES: red\n  FIX: yellow\n",
    );
    let mut merge = rename_command(&vault, &cache);
    merge.env("BOB_CONFIG_FILE", &config).args([
        "themes", "rename", "FIXES", "FIX", "-d", "--merge", "-f", "json",
    ]);
    let merged = merge.output().expect("merge configured");
    assert_success(&merged);
    let merged_value: Value =
        serde_json::from_str(stdout(&merged).trim()).expect("json");
    let merge_warnings = merged_value["warnings"].as_array().expect("warnings");
    assert!(
        merge_warnings.iter().any(|warning| warning
            .as_str()
            .unwrap()
            .contains("stays in themes:")),
        "{merge_warnings:?}"
    );
}

fn coverage_vault(
    root: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: yellow\ntheme: BUILD\n---\n\
- [ ] #task Open work [theme:: BUILD] ^open-work\n",
    );
    write_file(
        &vault.join("quoted.md"),
        "---\ntype: [[project]]\nparent: [[dev]]\ntheme: \"BUILD\"\n---\n\
- [x] #task Paren field (theme:: BUILD) ^paren-field\n",
    );
    write_file(
        &vault.join("single.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ntheme: 'BUILD'\n---\n",
    );
    write_file(
        &vault.join("done/old.md"),
        "---\ntype: [[project]]\nparent: [[dev]]\n---\n\
- [x] #task Archived [theme:: BUILD] ^archived\n",
    );
    write_file(
        &vault.join("prose.md"),
        "Prose BUILD and [theme:: BUILD] stay.\n\
```md\n\
- [ ] #task Fenced [theme:: BUILD]\n\
```\n",
    );
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — BUILD\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

#[test]
fn themes_rename_rewrites_defaults_and_task_fields() {
    let temp = TempDir::new("bob-cli-themes-rename-coverage");
    let (vault, cache) = coverage_vault(temp.path());

    let (output, value) = rename_json(&vault, &cache, &["BUILD", "FIX", "-d"]);
    assert_success(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["sessions"], 1);
    assert_eq!(value["defaults"], 3);
    assert_eq!(value["task_themes"], 3);
    let notes = value["default_notes"].as_array().expect("default_notes");
    let listed: Vec<&str> = notes.iter().filter_map(|n| n.as_str()).collect();
    assert!(listed.contains(&"dev.md"), "{listed:?}");
    assert!(listed.contains(&"quoted.md"), "{listed:?}");
    assert!(listed.contains(&"single.md"), "{listed:?}");

    let (output, _) = rename_json(&vault, &cache, &["BUILD", "FIX", "-o"]);
    assert_success(&output);

    let dev = fs::read_to_string(vault.join("dev.md")).unwrap();
    assert!(dev.contains("theme: FIX\n"), "{dev}");
    assert!(dev.contains("[theme:: FIX]"), "{dev}");
    assert!(!dev.contains("BUILD"), "{dev}");

    let quoted = fs::read_to_string(vault.join("quoted.md")).unwrap();
    assert!(quoted.contains("theme: \"FIX\"\n"), "{quoted}");
    assert!(quoted.contains("(theme:: FIX)"), "{quoted}");

    let single = fs::read_to_string(vault.join("single.md")).unwrap();
    assert!(single.contains("theme: 'FIX'\n"), "{single}");

    let archived = fs::read_to_string(vault.join("done/old.md")).unwrap();
    assert!(archived.contains("[theme:: FIX]"), "{archived}");

    let prose = fs::read_to_string(vault.join("prose.md")).unwrap();
    assert!(
        prose.contains("Prose BUILD and [theme:: BUILD] stay."),
        "{prose}"
    );
    assert!(
        prose.contains("- [ ] #task Fenced [theme:: BUILD]"),
        "{prose}"
    );

    let day = fs::read_to_string(vault.join("2026/20261020.md")).unwrap();
    assert!(day.contains("— FIX\n"), "{day}");
}

#[test]
fn themes_rename_merge_rewrites_task_fields_to_new() {
    let temp = TempDir::new("bob-cli-themes-rename-merge-fields");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ntheme: FIXES\n---\n\
- [ ] #task Keep [theme:: FIX] ^keep\n\
- [ ] #task Move [theme:: FIXES] ^move\n",
    );
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n- [ ] () — FIXES\n- [x] (0800-0830 30m) — FIX\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (output, value) =
        rename_json(&vault, &cache, &["FIXES", "FIX", "--merge", "-o"]);
    assert_success(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["task_themes"], 1);
    assert_eq!(value["defaults"], 1);

    let dev = fs::read_to_string(vault.join("dev.md")).unwrap();
    assert!(dev.contains("theme: FIX\n"), "{dev}");
    assert!(dev.contains("[theme:: FIX] ^keep"), "{dev}");
    assert!(dev.contains("[theme:: FIX] ^move"), "{dev}");
    assert!(!dev.contains("FIXES"), "{dev}");
}

#[test]
fn themes_rename_refuses_reading_theme() {
    let temp = TempDir::new("bob-cli-themes-rename-reading-theme");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n- [ ] () — READ\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (output, value) = rename_json(&vault, &cache, &["READ", "STUDY", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "config_reference");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("plan.reading_theme"),
        "{value}"
    );

    let config = cache.join("reading-off.yml");
    write_file(&config, "plan:\n  reading_theme: off\n");
    let mut command = rename_command(&vault, &cache);
    command
        .env("BOB_CONFIG_FILE", &config)
        .args(["themes", "rename", "READ", "STUDY", "-d", "-f", "json"]);
    let allowed = command.output().expect("reading_theme off");
    assert_success(&allowed);
    let allowed_value: Value =
        serde_json::from_str(stdout(&allowed).trim()).expect("json");
    assert_eq!(allowed_value["ok"], true);
    assert_eq!(allowed_value["sessions"], 1);
}
