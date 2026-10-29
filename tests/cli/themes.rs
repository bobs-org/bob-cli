//! `bob types` list: catalog, hues, ranking, and cache.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-24 12:00:00";

fn write_theme_vault(root: &Path) {
    write_file(
        &root.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: aqua\n---\n",
    );
    write_file(
        &root.join("dev.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: blue\n---\n",
    );
    write_file(
        &root.join("gtd.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: orange\n---\n",
    );
    write_file(
        &root.join("2026/20261020.md"),
        "## Pomodoros\n\
- [x] (0800-0830 30m) — BOB\n\
- [x] (0900-0930 30m) — JOB\n\
- [x] (1000-1030 30m) — BLOG\n\
- [x] (1100-1130 30m) — PLAN\n\
- [x] (1200-1230 30m) — FIXES\n",
    );
    write_file(
        &root.join("2026/20261024.md"),
        "## Pomodoros\n\
- [ ] () — BOB\n\
- [x] (0800-0830 30m) — PLAN\n\
- [ ] () — FIX\n",
    );
}

fn themes_command(vault: &Path, cache: &Path) -> std::process::Command {
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

fn hold_vault_lock(path: &Path) -> std::fs::File {
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
fn themes_help_is_in_daily_workflow_and_lists_options() {
    let output = bob_command()
        .args(["themes", "--help"])
        .output()
        .expect("bob types --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Manage task types and explain task suggestions"),
        "group about:\n{help}"
    );
    assert!(
        help.contains("Bare `bob types` runs the read-only list"),
        "default list:\n{help}"
    );
    assert!(help.contains("list"), "member list:\n{help}");
    assert!(help.contains("migrate"), "member migrate:\n{help}");
    assert!(help.contains("color"), "member color:\n{help}");
    assert!(help.contains("hide"), "member hide:\n{help}");
    assert!(help.contains("rename"), "member rename:\n{help}");
    assert!(help.contains("split"), "member split:\n{help}");
    assert!(help.contains("unhide"), "member unhide:\n{help}");
    assert!(
        help.contains("docs/task-types.md"),
        "contract link:\n{help}"
    );

    let list = bob_command()
        .args(["themes", "list", "--help"])
        .output()
        .expect("bob types list --help");
    assert_success(&list);
    let list_help = stdout(&list);
    assert_text_order(
        &list_help,
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-f, --format",
            "-F, --fresh",
            "-h, --help",
            "-H, --hidden",
            "-n, --limit",
            "-q, --query",
        ],
    );

    for (verb, flags) in [
        ("hide", &["-b, --bob-dir", "-f, --format", "-h, --help"][..]),
        (
            "unhide",
            &["-b, --bob-dir", "-f, --format", "-h, --help"][..],
        ),
        (
            "color",
            &["-b, --bob-dir", "-f, --format", "-h, --help"][..],
        ),
        (
            "migrate",
            &[
                "-b, --bob-dir",
                "-d, --dry-run",
                "-f, --format",
                "-h, --help",
                "-o, --offline",
            ][..],
        ),
        (
            "rename",
            &[
                "-b, --bob-dir",
                "-d, --dry-run",
                "-f, --format",
                "-h, --help",
                "-m, --merge",
                "-o, --offline",
            ][..],
        ),
        (
            "split",
            &[
                "-b, --bob-dir",
                "-d, --dry-run",
                "-f, --format",
                "-h, --help",
                "-m, --map",
                "-o, --offline",
                "-t, --template",
            ][..],
        ),
    ] {
        let output = bob_command()
            .args(["themes", verb, "--help"])
            .output()
            .unwrap_or_else(|error| panic!("bob types {verb} --help: {error}"));
        assert_success(&output);
        assert_text_order(&stdout(&output), flags);
    }
}

#[test]
fn themes_alias_matches_types_for_list_and_bare_group() {
    let temp = TempDir::new("bob-cli-themes-alias-parity");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    for (alias_args, canonical_args) in [
        (
            &["themes", "list", "-f", "json"][..],
            &["types", "list", "-f", "json"][..],
        ),
        (&["themes", "-f", "json"][..], &["types", "-f", "json"][..]),
    ] {
        let alias = themes_command(&vault, &cache)
            .args(alias_args)
            .output()
            .expect("bob themes list");
        let canonical = themes_command(&vault, &cache)
            .args(canonical_args)
            .output()
            .expect("bob types list");
        assert_eq!(alias.status.code(), canonical.status.code());
        assert_eq!(alias.stdout, canonical.stdout);
        assert_eq!(alias.stderr, canonical.stderr);
    }
}

#[test]
fn themes_lists_expected_hues_on_scratch_vault() {
    let temp = TempDir::new("bob-cli-themes-list");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let output = themes_command(&vault, &cache)
        .args(["themes", "-a"])
        .output()
        .expect("bob types -a");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("TYPES"), "header:\n{text}");
    assert!(text.contains("BOB"), "BOB:\n{text}");
    assert!(
        text.contains("JOB") && text.contains("aqua"),
        "JOB:\n{text}"
    );
    assert!(
        text.contains("PLAN") && text.contains("muted"),
        "PLAN:\n{text}"
    );
    assert!(text.contains("BLOG"), "BLOG:\n{text}");
}

#[test]
fn themes_json_schema_and_query_ranks_fix_before_fixes() {
    let temp = TempDir::new("bob-cli-themes-json");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let output = themes_command(&vault, &cache)
        .args(["themes", "-q", "fix", "-a", "-f", "json"])
        .output()
        .expect("bob types -q fix -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["today"], "2026-10-24");
    assert_eq!(value["hints"], Value::Array(vec![]));
    let names: Vec<&str> = value["themes"]
        .as_array()
        .expect("themes")
        .iter()
        .map(|theme| theme["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names.first().copied(), Some("FIX"));
    assert!(names.contains(&"FIXES"));
    assert!(
        names.iter().position(|name| *name == "FIX")
            < names.iter().position(|name| *name == "FIXES")
    );

    let job = value["themes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|theme| theme["key"] == "job");
    assert!(job.is_none(), "query fix should not list JOB");
}

#[test]
fn themes_fresh_and_cached_json_agree() {
    let temp = TempDir::new("bob-cli-themes-cache");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let fresh = themes_command(&vault, &cache)
        .args(["themes", "-a", "-F", "-f", "json"])
        .output()
        .expect("fresh");
    assert_success(&fresh);
    let cached = themes_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("cached");
    assert_success(&cached);
    let fresh_json: Value =
        serde_json::from_str(stdout(&fresh).trim()).unwrap();
    let cached_json: Value =
        serde_json::from_str(stdout(&cached).trim()).unwrap();
    assert_eq!(fresh_json["themes"], cached_json["themes"]);
    assert_eq!(
        fresh_json["counts"]["known"],
        cached_json["counts"]["known"]
    );
}

#[test]
fn themes_bare_is_list() {
    let temp = TempDir::new("bob-cli-themes-bare");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let bare = themes_command(&vault, &cache)
        .arg("themes")
        .output()
        .expect("bob types");
    let listed = themes_command(&vault, &cache)
        .args(["themes", "list"])
        .output()
        .expect("bob types list");
    assert_success(&bare);
    assert_success(&listed);
    assert_eq!(stdout(&bare), stdout(&listed));
}

#[test]
fn themes_hidden_filter_reads_journal() {
    let temp = TempDir::new("bob-cli-themes-hidden");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);
    write_file(
        &vault.join(".bob/themes/apollo.jsonl"),
        "{\"v\":1,\"ts\":\"2026-10-24T09:00:00-04:00\",\"host\":\"apollo\",\"kind\":\"theme.hide\",\"key\":\"fixes\",\"name\":\"FIXES\",\"via\":\"bob-cli\"}\n",
    );

    let hidden = themes_command(&vault, &cache)
        .args(["themes", "-H", "-f", "json"])
        .output()
        .expect("hidden");
    assert_success(&hidden);
    let value: Value = serde_json::from_str(stdout(&hidden).trim()).unwrap();
    let names: Vec<&str> = value["themes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|theme| theme["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["FIXES"]);
    assert_eq!(value["themes"][0]["hidden"], true);
    assert!(value["counts"]["hidden"].as_u64().unwrap() >= 1);
}

#[test]
fn themes_hide_unhide_color_round_trip_folds_two_hosts() {
    let temp = TempDir::new("bob-cli-themes-write");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let hide = themes_command(&vault, &cache)
        .env("HOSTNAME", "apollo")
        .args(["themes", "hide", "FIXES", "-f", "json"])
        .output()
        .expect("hide");
    assert_success(&hide);
    let hide_json: Value = serde_json::from_str(stdout(&hide).trim()).unwrap();
    assert_eq!(hide_json["ok"], true);
    assert_eq!(hide_json["hidden"][0], "FIXES");
    assert_eq!(hide_json["via"], "bob-cli");

    let color = themes_command(&vault, &cache)
        .env("HOSTNAME", "mac")
        .args([
            "themes",
            "color",
            "BLOG",
            "red",
            "--via",
            "bob-mac-capture",
            "-f",
            "json",
        ])
        .output()
        .expect("color");
    assert_success(&color);
    let color_json: Value =
        serde_json::from_str(stdout(&color).trim()).unwrap();
    assert_eq!(color_json["ok"], true);
    assert_eq!(color_json["name"], "BLOG");
    assert_eq!(color_json["hue"], "red");
    assert_eq!(color_json["pinned"], true);
    assert_eq!(color_json["via"], "bob-mac-capture");

    let listed = themes_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("list");
    assert_success(&listed);
    let value: Value = serde_json::from_str(stdout(&listed).trim()).unwrap();
    let themes = value["themes"].as_array().unwrap();
    let fixes = themes.iter().find(|theme| theme["key"] == "fixes").unwrap();
    let blog = themes.iter().find(|theme| theme["key"] == "blog").unwrap();
    assert_eq!(fixes["hidden"], true);
    assert_eq!(blog["pinned"], true);
    assert_eq!(blog["hue"], "red");
    assert_eq!(blog["hue_source"], "pin");

    let apollo = fs::read_to_string(vault.join(".bob/themes/apollo.jsonl"))
        .expect("apollo journal");
    let mac = fs::read_to_string(vault.join(".bob/themes/mac.jsonl"))
        .expect("mac journal");
    assert!(apollo.contains("theme.hide"));
    assert!(mac.contains("theme.color"));

    let unhide = themes_command(&vault, &cache)
        .env("HOSTNAME", "apollo")
        .args(["themes", "unhide", "FIXES", "-f", "json"])
        .output()
        .expect("unhide");
    assert_success(&unhide);
    let unpin = themes_command(&vault, &cache)
        .env("HOSTNAME", "mac")
        .args(["themes", "color", "BLOG", "auto", "-f", "json"])
        .output()
        .expect("unpin");
    assert_success(&unpin);
    let restored = themes_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("restored");
    let restored_json: Value =
        serde_json::from_str(stdout(&restored).trim()).unwrap();
    let themes = restored_json["themes"].as_array().unwrap();
    let fixes = themes.iter().find(|theme| theme["key"] == "fixes").unwrap();
    let blog = themes.iter().find(|theme| theme["key"] == "blog").unwrap();
    assert_eq!(fixes["hidden"], false);
    assert_eq!(blog["pinned"], false);
}

#[test]
fn themes_write_refusals_are_json_on_nonzero_exit() {
    let temp = TempDir::new("bob-cli-themes-refusals");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);

    let unknown = themes_command(&vault, &cache)
        .args(["themes", "hide", "NOPE", "-f", "json"])
        .output()
        .expect("unknown");
    assert_eq!(unknown.status.code(), Some(1));
    let unknown_json: Value =
        serde_json::from_str(stdout(&unknown).trim()).unwrap();
    assert_eq!(unknown_json["ok"], false);
    assert_eq!(unknown_json["code"], "unknown_theme");

    let invalid = themes_command(&vault, &cache)
        .args(["themes", "color", "BLOG", "purple", "-f", "json"])
        .output()
        .expect("invalid hue");
    assert_eq!(invalid.status.code(), Some(1));
    let invalid_json: Value =
        serde_json::from_str(stdout(&invalid).trim()).unwrap();
    assert_eq!(invalid_json["ok"], false);
    assert_eq!(invalid_json["code"], "invalid_hue");

    let first = themes_command(&vault, &cache)
        .args(["themes", "hide", "FIXES", "-f", "json"])
        .output()
        .expect("first hide");
    assert_success(&first);
    let again = themes_command(&vault, &cache)
        .args(["themes", "hide", "FIXES", "-f", "json"])
        .output()
        .expect("unchanged hide");
    assert_eq!(again.status.code(), Some(1));
    let again_json: Value =
        serde_json::from_str(stdout(&again).trim()).unwrap();
    assert_eq!(again_json["ok"], false);
    assert_eq!(again_json["code"], "unchanged");
    let journal = fs::read_to_string(vault.join(".bob/themes/apollo.jsonl"))
        .expect("journal");
    assert_eq!(journal.lines().count(), 1);
}

#[test]
fn themes_concurrent_hides_both_survive() {
    let temp = TempDir::new("bob-cli-themes-concurrent");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);
    let lock = temp.path().join("bob_sync.lock");

    let mut first = themes_command(&vault, &cache);
    first
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["themes", "hide", "FIXES", "-f", "json"]);
    let mut second = themes_command(&vault, &cache);
    second
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["themes", "hide", "BLOG", "-f", "json"]);

    let handle_a =
        std::thread::spawn(move || first.output().expect("hide FIXES"));
    let handle_b =
        std::thread::spawn(move || second.output().expect("hide BLOG"));
    let out_a = handle_a.join().expect("join FIXES");
    let out_b = handle_b.join().expect("join BLOG");
    assert_success(&out_a);
    assert_success(&out_b);

    let journal = fs::read_to_string(vault.join(".bob/themes/apollo.jsonl"))
        .expect("journal");
    assert!(journal.contains("\"key\":\"fixes\""), "{journal}");
    assert!(journal.contains("\"key\":\"blog\""), "{journal}");
}

#[test]
fn themes_hide_lock_busy_when_maintenance_lock_held() {
    let temp = TempDir::new("bob-cli-themes-lock-busy");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_theme_vault(&vault);
    let lock = temp.path().join("bob_sync.lock");
    let _held = hold_vault_lock(&lock);

    let output = themes_command(&vault, &cache)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["themes", "hide", "FIXES", "-f", "json"])
        .output()
        .expect("lock busy");
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "lock_busy");
    assert!(!vault.join(".bob/themes/apollo.jsonl").exists());
}

#[test]
fn themes_list_emits_collision_hints() {
    let temp = TempDir::new("bob-cli-themes-hints");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("2026/20261001.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — SASE\n- [x] (0900-0930 30m) — BLOG\n",
    );
    write_file(
        &vault.join("2026/20261002.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — SASE\n- [x] (0900-0930 30m) — BLOG\n",
    );
    write_file(
        &vault.join("2026/20261003.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — SASE\n",
    );
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — SASE\n",
    );

    let output = themes_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("hints json");
    assert_success(&output);
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    let hints = value["hints"].as_array().expect("hints");
    assert_eq!(hints.len(), 1, "{hints:?}");
    assert_eq!(hints[0]["code"], "theme_hue_collision");
    assert_eq!(hints[0]["themes"][0], "SASE");
    assert_eq!(hints[0]["themes"][1], "BLOG");
    assert_eq!(hints[0]["hue"], "orange");
    assert_eq!(hints[0]["days_together"], 2);
    assert_eq!(hints[0]["suggest"]["name"], "BLOG");
    assert_ne!(hints[0]["suggest"]["hue"], "orange");

    let human = themes_command(&vault, &cache)
        .args(["themes", "-a"])
        .output()
        .expect("hints human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("hint")
            && text.contains("BLOG")
            && text.contains("orange"),
        "{text}"
    );
}

#[test]
fn themes_list_emits_not_configured_hints() {
    let temp = TempDir::new("bob-cli-themes-not-configured-hints");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — SNACK\n- [ ] () — BUILD\n",
    );
    let config = cache.join("config.yml");
    write_file(
        &config,
        "plan:\n  max_themes: 30\nthemes:\n  BUILD: yellow\n  CHORE: magenta\n",
    );

    let output = themes_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["themes", "-f", "json"])
        .output()
        .expect("not-configured hints json");
    assert_success(&output);
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    let hints = value["hints"].as_array().expect("hints");
    let hint = hints
        .iter()
        .find(|hint| hint["code"] == "theme_not_configured")
        .expect("theme_not_configured hint");
    assert_eq!(hint["theme"]["name"], "SNACK");
    assert_eq!(hint["theme"]["key"], "snack");
    assert_eq!(hint["last_used"], "2026-10-24");

    let human = themes_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["themes"])
        .output()
        .expect("not-configured hints human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("SNACK isn't one of your types")
            && text.contains("themes:"),
        "{text}"
    );
}

fn suggest_json(vault: &Path, cache: &Path, target: &str) -> Value {
    let output = themes_command(vault, cache)
        .args(["themes", "suggest", target, "-f", "json"])
        .output()
        .expect("theme explanation");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_slice(&output.stdout).expect("explanation JSON")
}

fn vault_bytes(
    root: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(
        root: &Path,
        dir: &Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(dir).expect("list vault") {
            let entry = entry.expect("entry");
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), files);
            } else {
                files.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(root, root, &mut files);
    files
}

#[test]
fn themes_suggest_explains_overrides_inheritance_and_new_tasks_without_writes()
{
    let temp = TempDir::new("bob-cli-theme-suggest");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: blue\ntheme: FIX\n---\n",
    );
    write_file(&vault.join("work.md"), "---\ntype: [[project]]\nparent: [[dev]]\n---\n- [ ] #task Build [theme:: BOB] ^build\n- [ ] #task Read #ref ^reading\n- [x] #task Done [theme:: BOB] ^done\n- [ ] #task Plain ^plain\n");
    let before = vault_bytes(&vault);
    let task = suggest_json(&vault, &cache, "WORK:build");
    assert_eq!(task["schema_version"], 1);
    assert_eq!(task["ok"], true);
    assert_eq!(
        task["target"],
        serde_json::json!({"route":"work", "block_id":"build", "path":"work.md", "line":5, "new_task":false})
    );
    let suggestion = &task["theme_suggestion"];
    assert_eq!(suggestion["source"], "task");
    assert!(suggestion.get("from").is_none());
    assert_eq!(suggestion["inherited"]["theme"]["name"], "FIX");
    assert_eq!(
        suggestion["inherited"]["from"]["chain"],
        serde_json::json!(["work.md", "dev.md"])
    );
    assert!(suggestion["inherited"].get("inherited").is_none());
    let new_task = suggest_json(&vault, &cache, "work");
    assert_eq!(new_task["target"]["new_task"], true);
    assert!(new_task["target"]["line"].is_null());
    assert!(new_task["target"]["block_id"].is_null());
    assert!(new_task["theme_suggestion"].get("inherited").is_none());
    assert_eq!(new_task["theme_suggestion"]["from"]["hue"], "blue");
    let reading = suggest_json(&vault, &cache, "work:reading");
    assert_eq!(reading["theme_suggestion"]["source"], "reading");
    assert_eq!(reading["theme_suggestion"]["theme"]["known"], false);
    assert!(reading["theme_suggestion"].get("from").is_none());
    let closed = suggest_json(&vault, &cache, "work:done");
    assert!(closed["theme_suggestion"].is_null());
    assert_eq!(
        closed["warnings"],
        serde_json::json!([{"code":"closed_task"}])
    );
    let human = themes_command(&vault, &cache)
        .args(["themes", "suggest", "work:build"])
        .output()
        .unwrap();
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    assert!(stdout(&human).contains("without it: ■ FIX from dev"));
    let human = themes_command(&vault, &cache)
        .args(["themes", "suggest", "work"])
        .output()
        .unwrap();
    assert!(stdout(&human).contains("chain: work.md → dev.md"));
    assert_eq!(before, vault_bytes(&vault));
}

#[test]
fn themes_suggest_null_inheritance_repair_hidden_and_invalid_default_warnings()
{
    let temp = TempDir::new("bob-cli-theme-suggest-null");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    write_file(&vault.join("work.md"), "---\ntype: [[project]]\ntheme: [BAD]\nparent: [[dev]]\n---\n- [ ] #task Own (theme:: FIX) ^own\n- [ ] #task Repair [theme:: A] (theme:: B) ^repair\n");
    let own = suggest_json(&vault, &cache, "work:own");
    assert!(own["theme_suggestion"].get("inherited").unwrap().is_null());
    assert_eq!(
        own["warnings"],
        serde_json::json!([{"code":"theme_default_invalid", "path":"work.md"}])
    );
    let none = suggest_json(&vault, &cache, "work:repair");
    assert!(none["theme_suggestion"].is_null());
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\ntheme: FIX\n---\n",
    );
    let hidden = themes_command(&vault, &cache)
        .args(["themes", "hide", "FIX", "-f", "json"])
        .output()
        .unwrap();
    assert_success(&hidden);
    let repaired = suggest_json(&vault, &cache, "work:repair");
    assert_eq!(repaired["theme_suggestion"]["repair"], "duplicate");
    assert_eq!(repaired["theme_suggestion"]["theme"]["name"], "FIX");
    assert!(repaired["theme_suggestion"]["theme"].get("known").is_none());
}

#[test]
fn themes_suggest_reuses_capture_lookup_errors_and_rejects_unsafe_targets() {
    let temp = TempDir::new("bob-cli-theme-suggest-errors");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_file(&vault.join("work.md"), "- [ ] #task Build ^build\nProse ^prose\n- [ ] #task A ^dup\n- [ ] #task B ^dup\n");
    for (target, message, status) in [
        ("work:biuld", "did you mean ^build?", 1),
        ("work:prose", "is not a task (line 2:", 1),
        ("work:dup", "appears 2 times", 1),
        ("work:missing", "run 'bob capture-tasks -r work'", 1),
        ("missing", "read target missing.md", 1),
        ("../work", "route must contain only", 2),
        ("/work", "route must contain only", 2),
        ("work:", "valid block ID", 2),
        ("work:a:b", "valid block ID", 2),
    ] {
        let output = themes_command(&vault, &cache)
            .args(["themes", "suggest", target, "-f", "json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{target}: {}",
            stderr(&output)
        );
        let value: Value =
            serde_json::from_slice(&output.stdout).expect("error JSON");
        assert_eq!(value["ok"], false);
        assert!(
            value["message"].as_str().unwrap().contains(message),
            "{target}: {value}"
        );
    }
    write_file(
        &cache.join("missing-bob-config.yml"),
        "plan:\n  reading_theme: [READ]\n",
    );
    let output = themes_command(&vault, &cache)
        .args(["themes", "suggest", "work", "-f", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["code"], "config");
}

#[cfg(unix)]
#[test]
fn themes_suggest_refuses_symlink_escape_and_tolerates_closed_stdout() {
    use std::os::unix::fs::symlink;
    use std::process::Stdio;
    let temp = TempDir::new("bob-cli-theme-suggest-pipe");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[area]]\ntheme: NEW\n---\n",
    );
    write_file(&temp.path().join("outside.md"), "secret");
    symlink(temp.path().join("outside.md"), vault.join("escape.md")).unwrap();
    let output = themes_command(&vault, &cache)
        .args(["themes", "suggest", "escape", "-f", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("outside the Bob vault"));
    let mut child = themes_command(&vault, &cache)
        .args(["themes", "suggest", "work"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_success(&output);
    assert!(stderr(&output).is_empty());
}

fn unknown_default_hints(
    vault: &Path,
    cache: &Path,
    args: &[&str],
) -> Vec<Value> {
    let output = themes_command(vault, cache)
        .args(["themes", "list", "-f", "json"])
        .args(args)
        .output()
        .unwrap();
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value["hints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|hint| hint["code"] == "theme_default_unknown")
        .cloned()
        .collect()
}

#[test]
fn themes_unknown_defaults_are_vault_wide_sorted_and_do_not_write() {
    let temp = TempDir::new("bob-cli-theme-default-hints");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    for (path, kind, value) in [
        ("z.md", "area", "BOBS"),
        ("sub/a.md", "project", "TYPO"),
        ("known.md", "project", "FIX"),
        ("off.md", "area", "OFF"),
        ("blank.md", "area", "''"),
        ("invalid.md", "area", "[BAD]"),
        ("ignored.md", "reference", "UNUSED"),
        ("done/archive.md", "project", "SKIP"),
        (".bob/internal.md", "area", "SKIP"),
        ("_templates/t.md", "project", "SKIP"),
    ] {
        write_file(
            &vault.join(path),
            &format!("---\ntype: [[{kind}]]\ntheme: {value}\n---\n"),
        );
    }
    let before = vault_bytes(&vault);
    let expected = unknown_default_hints(&vault, &cache, &[]);
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[0]["path"], "sub/a.md");
    assert_eq!(expected[1]["path"], "z.md");
    assert_eq!(expected[1]["name"], "BOBS");
    assert_eq!(expected[1]["similar_to"], serde_json::json!(["BOB"]));
    for flags in [&["-q", "nomatch"][..], &["-n", "0"], &["-H"], &["-F"]] {
        assert_eq!(unknown_default_hints(&vault, &cache, flags), expected);
    }
    let human = themes_command(&vault, &cache)
        .args(["themes", "-q", "nomatch"])
        .output()
        .unwrap();
    assert!(stdout(&human)
        .contains("z.md suggests unknown type BOBS; did you mean BOB?"));
    assert_eq!(before, vault_bytes(&vault));
}

#[test]
fn themes_default_hints_track_edits_additions_deletions_types_and_knownness() {
    let temp = TempDir::new("bob-cli-theme-default-refresh");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    let path = vault.join("work.md");
    write_file(&path, "---\ntype: [[project]]\ntheme: NEW\n---\n");
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 1);
    write_file(&path, "---\ntype: [[project]]\ntheme: NEXT\n---\n");
    assert_eq!(
        unknown_default_hints(&vault, &cache, &[])[0]["name"],
        "NEXT"
    );
    write_file(&path, "---\ntype: [[reference]]\ntheme: NEXT\n---\n");
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(&path, "---\ntype: [[area]]\ntheme: NEXT\n---\n");
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 1);
    write_file(&path, "---\ntype: [[area]]\n---\n");
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(
        &vault.join("new/nested.md"),
        "---\ntype: [[project]]\ntheme: NEXT\n---\n",
    );
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 1);
    fs::remove_file(vault.join("new/nested.md")).unwrap();
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(&path, "---\ntype: [[project]]\ntheme: NEXT\n---\n");
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — NEXT\n",
    );
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(&path, "---\ntype: [[project]]\ntheme: VOCAB\n---\n");
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 1);
    write_file(
        &cache.join("missing-bob-config.yml"),
        "themes:\n  VOCAB: blue\n",
    );
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(&cache.join("missing-bob-config.yml"), "themes: {}\n");
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 1);
}

#[test]
fn themes_list_does_not_invent_off_or_whitespace_unknown_hints() {
    let temp = TempDir::new("bob-cli-theme-default-off-blank");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    write_file(
        &vault.join("padded-off.md"),
        "---\ntype: [[area]]\ntheme: \" off \"\n---\n",
    );
    write_file(
        &vault.join("quoted-blank.md"),
        "---\ntype: [[project]]\ntheme: \"   \"\n---\n",
    );
    write_file(
        &vault.join("unindented.md"),
        "---\ntype: [[project]]\ntheme:\n- WRITE\n---\n",
    );
    assert!(unknown_default_hints(&vault, &cache, &[]).is_empty());
    write_file(
        &vault.join("padded-off.md"),
        "---\ntype: [[area]]\ntheme: MYSTERY\n---\n",
    );
    let after_edit = unknown_default_hints(&vault, &cache, &[]);
    assert_eq!(after_edit.len(), 1);
    assert_eq!(after_edit[0]["path"], "padded-off.md");
    assert_eq!(after_edit[0]["name"], "MYSTERY");
}

#[test]
fn themes_default_cache_rebuilds_on_corruption_fresh_and_new_vaults() {
    let temp = TempDir::new("bob-cli-theme-default-cache");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    write_file(
        &vault.join("work.md"),
        "---\ntype: [[project]]\ntheme: NEW\n---\n",
    );
    let expected = unknown_default_hints(&vault, &cache, &[]);
    let cache_file = fs::read_dir(cache.join("bob-cli/themes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("note-defaults-v1-")
        })
        .unwrap();
    fs::write(&cache_file, b"{broken").unwrap();
    assert_eq!(unknown_default_hints(&vault, &cache, &[]), expected);
    let mut value: Value =
        serde_json::from_slice(&fs::read(&cache_file).unwrap()).unwrap();
    value["root"]["files"]["work.md"]["default"] = serde_json::json!("STALE");
    fs::write(&cache_file, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(unknown_default_hints(&vault, &cache, &["-F"]), expected);
    let second = temp.path().join("second");
    write_file(
        &second.join("work.md"),
        "---\ntype: [[area]]\ntheme: SECOND\n---\n",
    );
    assert_eq!(
        unknown_default_hints(&second, &cache, &[])[0]["name"],
        "SECOND"
    );
    assert_eq!(unknown_default_hints(&vault, &cache, &[]), expected);
}

#[test]
fn themes_default_cache_rechecks_same_size_edits_and_root_area_colors() {
    let temp = TempDir::new("bob-cli-theme-default-metadata");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    write_theme_vault(&vault);
    let path = vault.join("work.md");
    write_file(&path, "---\ntype: [[project]]\ntheme: NEW\n---\n");
    let old = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(unknown_default_hints(&vault, &cache, &[])[0]["name"], "NEW");
    write_file(&path, "---\ntype: [[project]]\ntheme: OLD\n---\n");
    // Explicitly change mtime so this test does not depend on FS timestamp resolution.
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(old + std::time::Duration::from_secs(1)),
        )
        .unwrap();
    assert_eq!(unknown_default_hints(&vault, &cache, &[])[0]["name"], "OLD");
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: red\n---\n",
    );
    let output = themes_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .unwrap();
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let job = value["themes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|theme| theme["key"] == "job")
        .unwrap();
    assert_eq!(job["hue"], "red");
    assert_eq!(job["hue_source"], "area");
}

#[test]
fn themes_default_cache_refreshes_large_vaults_and_propagates_read_errors() {
    let temp = TempDir::new("bob-cli-theme-default-large-vault");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    // Enough notes for multiple refresh workers, with defaults spread across
    // the sorted file list and many initially untyped notes between them.
    for index in 0..1100 {
        let contents = if index % 137 == 0 {
            "---\ntype: [[project]]\ntheme: UNKNOWN\n---\n"
        } else {
            "ordinary note\n"
        };
        write_file(&vault.join(format!("nested/{index:04}.md")), contents);
    }
    let cold = unknown_default_hints(&vault, &cache, &[]);
    assert_eq!(cold.len(), 9);
    assert_eq!(unknown_default_hints(&vault, &cache, &[]), cold);
    write_file(
        &vault.join("nested/0500.md"),
        "---\ntype: [[area]]\ntheme: RETYPED\n---\n",
    );
    fs::remove_file(vault.join("nested/0274.md")).unwrap();
    write_file(&vault.join("nested/0959.md"), "ordinary again\n");
    write_file(
        &vault.join("added/new.md"),
        "---\ntype: [[project]]\ntheme: ADDED\n---\n",
    );
    let before = vault_bytes(&vault);
    let warm = unknown_default_hints(&vault, &cache, &[]);
    assert_eq!(warm.len(), 9);
    assert!(warm.iter().any(|hint| hint["name"] == "RETYPED"));
    assert!(warm.iter().any(|hint| hint["name"] == "ADDED"));
    assert!(!warm.iter().any(|hint| hint["path"] == "nested/0274.md"));
    assert!(!warm.iter().any(|hint| hint["path"] == "nested/0959.md"));
    assert_eq!(unknown_default_hints(&vault, &cache, &["-F"]), warm);
    assert_eq!(vault_bytes(&vault), before);

    fs::write(vault.join("nested/0500.md"), [0xff]).unwrap();
    let output = themes_command(&vault, &cache)
        .args(["themes", "list", "-f", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert!(stdout(&output).contains("read note defaults"));
}

#[cfg(unix)]
#[test]
fn themes_default_cache_discovers_additions_through_a_vault_symlink() {
    let temp = TempDir::new("bob-cli-theme-default-root-symlink");
    let vault = temp.path().join("vault");
    let alias = temp.path().join("alias");
    let cache = temp.path().join("cache");
    write_file(
        &vault.join("one.md"),
        "---\ntype: [[area]]\ntheme: FIRST\n---\n",
    );
    std::os::unix::fs::symlink(&vault, &alias).unwrap();
    assert_eq!(unknown_default_hints(&alias, &cache, &[]).len(), 1);
    write_file(
        &vault.join("two.md"),
        "---\ntype: [[project]]\ntheme: SECOND\n---\n",
    );
    assert_eq!(unknown_default_hints(&alias, &cache, &[]).len(), 2);
    assert_eq!(unknown_default_hints(&vault, &cache, &[]).len(), 2);
}
