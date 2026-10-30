//! `t:<type>` marker writes for newly captured tasks.
//!
//! Covers inbox, routed, explicit block IDs, `-R` URL tasks, templates,
//! Pomodoro-linked tasks, inherited suggestions, set/cleared/kept,
//! configured/collision/unknown resolution, named-Pomodoro agreement,
//! refusal families, batch rollback, and dry-run equality.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-28 09:15:00";

fn fixture(
    prefix: &str,
    task_note: &str,
    day: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261028.md");
    let config = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n",
        ),
    );
    write_file(&vault.join("sase.md"), task_note);
    write_file(&day_file, day);
    (temp, vault, day_file, config)
}

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    config: &std::path::Path,
    args: &[&str],
    dry_run: bool,
) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("-d");
    }
    command
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run capture");
    assert_success(&output);
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!("parse capture JSON: {error}\n{}", format_output(&output))
        });
    (output, json)
}

fn project(theme: Option<&str>, tasks: &str) -> String {
    let theme = theme
        .map(|theme| format!("theme: {theme}\n"))
        .unwrap_or_default();
    format!("---\ntype: [[project]]\n{theme}---\n# Sase\n\n## Tasks\n{tasks}")
}

#[test]
fn inbox_marker_sets_the_explicit_type() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-inbox",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "the", "parser", "t:fix"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(json["task_theme"]["theme"]["key"] == "fix", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
    // Marker text is stripped from the body.
    assert_eq!(json["text"], "Fix the parser", "{json}");
}

#[test]
fn routed_marker_reports_the_inherited_suggestion() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-routed",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "the", "parser", "t:fix", "@sase"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert_eq!(
        json["task_theme"]["inherited"]["theme"]["name"], "WRITE",
        "{json}"
    );
    assert_eq!(json["task_theme"]["inherited"]["source"], "note", "{json}");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn matching_inherited_type_reports_kept_without_a_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-kept",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Write", "docs", "t:write", "@sase"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "kept", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "WRITE", "{json}");
    // Kept with no authored field writes no field.
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn authored_exception_matching_inherited_type_is_cleared() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-cleared",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    // Body authors `[type:: WRITE]`, I is WRITE from sase, marker is
    // `t:write`: X == I with an authored exception means cleared.
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Write", "docs", "[type::", "WRITE]", "t:write", "@sase"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "cleared", "{json}");
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn unknown_marker_writes_canonical_spelling_with_a_warning() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-unknown",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "it", "t:snack", "@sase"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "SNACK", "{json}");
    // Structured warning is present.
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "theme_not_configured"),
        "{json}"
    );
}

#[test]
fn hyphenated_slug_writes_canonical_spelling_when_unknown() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-slug",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    // `deep-work` has no configured or catalog entry, so the slug→space
    // fallback finds nothing and the canonical `DEEP-WORK` spelling writes
    // with an unconfigured warning. Prefixes never resolve.
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Deep", "work", "t:deep-work", "@sase"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "DEEP-WORK", "{json}");
    assert_eq!(json["task_theme"]["theme"]["key"], "deep-work", "{json}");
}

#[test]
fn named_pomodoro_conflict_refuses_before_any_write() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-conflict",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let before_task = fs::read_to_string(vault.join("sase.md")).unwrap();
    let before_day = fs::read_to_string(&day_file).unwrap();
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("-d")
        .args(["Fix", "it", "@sase:conflict-1#BUILD", "t:fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run conflicting capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .expect("parse conflict JSON");
    assert_eq!(json["ok"], false, "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("disagrees with Pomodoro"),
        "{json}"
    );
    // No vault bytes committed.
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).unwrap(),
        before_task
    );
    assert_eq!(fs::read_to_string(&day_file).unwrap(), before_day);
}

#[test]
fn named_pomodoro_component_match_accepts_the_explicit_pick() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-agree",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "it", "@sase:agree-1#FIX", "t:fix"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
}

#[test]
fn existing_target_marker_is_refused_honestly() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-existing",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(["^sase:missing-x", "t:fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run existing-target capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .expect("parse refusal JSON");
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("not supported yet"),
        "{json}"
    );
}

#[test]
fn dry_run_and_real_run_agree_on_marker_writes() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-dryrun",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, dry) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "the", "parser", "t:fix", "@sase:dry-1"],
        true,
    );
    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "the", "parser", "t:fix", "@sase:dry-1"],
        false,
    );
    assert_eq!(
        written["task_theme"], dry["task_theme"],
        "{dry} vs {written}"
    );
    assert_eq!(written["task_line"], dry["task_line"], "{dry} vs {written}");
}

#[test]
fn kept_marker_renders_the_human_type_row() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-human",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .args(["Write", "docs", "t:write", "@sase"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("dry-run capture human output");
    assert_success(&human);
    assert!(
        stdout(&human).contains("  type  ")
            && stdout(&human).contains("from sase"),
        "{}",
        format_output(&human)
    );
}
