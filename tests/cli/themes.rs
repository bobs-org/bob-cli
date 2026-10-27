//! `bob themes` list: catalog, hues, ranking, and cache.

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
        .expect("bob themes --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("List remembered Pomodoro themes"),
        "group about:\n{help}"
    );
    assert!(
        help.contains("Bare `bob themes` runs the read-only list"),
        "default list:\n{help}"
    );
    assert!(help.contains("list"), "member list:\n{help}");
    assert!(help.contains("color"), "member color:\n{help}");
    assert!(help.contains("hide"), "member hide:\n{help}");
    assert!(help.contains("rename"), "member rename:\n{help}");
    assert!(help.contains("unhide"), "member unhide:\n{help}");
    assert!(
        help.contains("docs/pomodoro-themes.md"),
        "contract link:\n{help}"
    );

    let list = bob_command()
        .args(["themes", "list", "--help"])
        .output()
        .expect("bob themes list --help");
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
    ] {
        let output = bob_command()
            .args(["themes", verb, "--help"])
            .output()
            .unwrap_or_else(|error| {
                panic!("bob themes {verb} --help: {error}")
            });
        assert_success(&output);
        assert_text_order(&stdout(&output), flags);
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
        .expect("bob themes -a");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("THEMES"), "header:\n{text}");
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
        .expect("bob themes -q fix -f json");
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
        .expect("bob themes");
    let listed = themes_command(&vault, &cache)
        .args(["themes", "list"])
        .output()
        .expect("bob themes list");
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
