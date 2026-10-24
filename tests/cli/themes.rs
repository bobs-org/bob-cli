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
- [x] (1100-1130 30m) — GTD\n\
- [x] (1200-1230 30m) — FIXES\n",
    );
    write_file(
        &root.join("2026/20261024.md"),
        "## Pomodoros\n\
- [ ] () — BOB\n\
- [x] (0800-0830 30m) — GTD\n\
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
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
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
        text.contains("GTD") && text.contains("muted"),
        "GTD:\n{text}"
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
