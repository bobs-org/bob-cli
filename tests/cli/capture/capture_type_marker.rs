//! `#<type>` marker writes for newly captured tasks.
//!
//! Covers inbox, routed, explicit block IDs, `-R` URL tasks, templates,
//! Pomodoro-linked tasks, inherited suggestions, set/cleared/kept,
//! configured/collision/unknown resolution, named-Pomodoro agreement,
//! refusal families, batch rollback, and dry-run equality.

use crate::support::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

fn capture_error_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run refused capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "parse capture error JSON: {error}\n{}",
                format_output(&output)
            )
        });
    (output, json)
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).expect("read snapshot directory") {
            let path = entry.expect("snapshot entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.clone(),
                    fs::read(path).expect("snapshot file"),
                );
            }
        }
    }
    files
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
        &["Fix", "the", "parser", "#fix"],
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
fn inbox_marker_inherits_from_the_actual_mac_inbox_note() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-inbox-default",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: \"[[area]]\"\ntask_type: WRITE\n---\n\n## Tasks\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["Inbox task #fix"], true);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(
        json["task_theme"]["inherited"]["theme"]["name"], "WRITE",
        "{json}"
    );
    assert_eq!(json["task_theme"]["inherited"]["source"], "note", "{json}");
    let line = json["task_line"].as_str().unwrap();
    assert!(line.contains("[type:: FIX]"), "{json}");
    assert!(
        !line.contains("^"),
        "a plain typed inbox task needs no block ID: {json}"
    );
    assert!(
        !line.contains("[fresh::"),
        "creation must not stamp freshness: {json}"
    );
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
        &["Fix", "the", "parser", "#fix", "@sase"],
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
        &["Write", "docs", "#write", "@sase"],
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
fn explicit_kept_field_uses_this_task_as_its_human_label() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-explicit-kept",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix it [type:: FIX] #fix @sase"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "kept", "{json}");
    assert_eq!(json["task_theme"]["previous"]["name"], "FIX", "{json}");
    assert_eq!(
        json["task_theme"]["inherited"]["theme"]["name"], "WRITE",
        "{json}"
    );
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .args(["Fix", "it", "[type::", "FIX]", "#fix", "@sase"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run explicit kept capture");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("type  ") && text.contains("this task"),
        "{text}"
    );
    assert!(!text.contains("from sase"), "{text}");
}

#[test]
fn authored_exception_matching_inherited_type_is_cleared() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-cleared",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    // Body authors `[type:: WRITE]`, I is WRITE from sase, marker is
    // `#write`: X == I with an authored exception means cleared.
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Write", "docs", "[type::", "WRITE]", "#write", "@sase"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "cleared", "{json}");
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn unknown_marker_refuses_transactionally_even_in_dry_run() {
    let (temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-unknown",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let before = snapshot_files(temp.path());
    for args in [
        vec!["Fix it #snack @sase"],
        vec!["-d", "Fix it #snack @sase"],
    ] {
        let (_, json) = capture_error_json(&vault, &day_file, &config, &args);
        assert_eq!(json["code"], "type_not_configured", "{json}");
        assert!(json["error"].as_str().unwrap().contains("#snack"), "{json}");
        assert_eq!(snapshot_files(temp.path()), before);
    }
}

#[test]
fn unknown_hyphenated_slug_is_refused() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-slug",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    // Prefixes and names outside the configured vocabulary never resolve.
    let (_, json) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["Deep work #deep-work @sase"],
    );
    assert_eq!(json["code"], "type_not_configured", "{json}");
}

#[test]
fn refusal_copy_uses_marker_spelling_suggestions_and_attachment_hints() {
    let (temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-refusal-copy",
        &project(Some("WRITE"), "- [ ] #task Existing task ^parser\n"),
        "## Pomodoros\n- [x] (0800-0830) — CHORUS\n- [x] (0830-0900) — CHORE\n- [ ] () — FIX\n",
    );
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "types:\n",
            "  BUILD: yellow\n",
            "  FIX: blue\n",
            "  CHORE: magenta\n",
            "  CHORUS: red\n",
            "  \"DEEP WORK\": violet\n",
            "  WRITE: green\n",
        ),
    );

    let configured = format!(
        "`#builds` is not one of your task types; did you mean `#build`? Add BUILDS under `types:` in {} to use it, or write `\\#builds` to keep it as a tag",
        config.display()
    );
    let two_suggestions = format!(
        "`#chores` is not one of your task types; did you mean `#chore` or `#chorus`? Add CHORES under `types:` in {} to use it, or write `\\#chores` to keep it as a tag",
        config.display()
    );
    let no_suggestion = format!(
        "`#fxi` is not one of your task types; add FXI under `types:` in {} to use it, or write `\\#fxi` to keep it as a tag",
        config.display()
    );
    let literal = format!(
        "`#\"DEEP WURK\"` is not one of your task types; did you mean `#\"DEEP WORK\"`? Add DEEP WURK under `types:` in {} to use it, or write `\\#\"DEEP WURK\"` to keep it as a tag",
        config.display()
    );
    let project_hint = format!(
        "`#fxi` is not one of your task types; add FXI under `types:` in {} to use it (for a project note whose ` :new-id` tasks go under the FXI Pomodoro, write `@sase^new-id@#fxi`)",
        config.display()
    );
    let ensure_hint = format!(
        "`#fxi` is not one of your task types; add FXI under `types:` in {} to use it (to link the task under the FXI Pomodoro, write `^sase:parser#fxi`)",
        config.display()
    );
    let heading_hint = format!(
        "`#fxi` is not one of your task types; add FXI under `types:` in {} to use it, or write `\\#fxi` to keep it as a tag (for a bullet under a heading, write `@sase#fxi`)",
        config.display()
    );
    let before = snapshot_files(temp.path());
    for (draft, expected) in [
        ("Fix it #builds", configured.as_str()),
        ("Fix it #chores", two_suggestions.as_str()),
        ("Fix it #fxi", no_suggestion.as_str()),
        ("Fix it #\"DEEP WURK\"", literal.as_str()),
        ("@sase^new-id#fxi Fix it", project_hint.as_str()),
        ("@sase+parser#fxi", ensure_hint.as_str()),
        ("#fxi Fix it @sase", heading_hint.as_str()),
    ] {
        for dry_run in [false, true] {
            let args = if dry_run {
                vec!["-d", draft]
            } else {
                vec![draft]
            };
            let (_, json) =
                capture_error_json(&vault, &day_file, &config, &args);
            assert_eq!(json["code"], "type_not_configured", "{json}");
            let error = json["error"].as_str().unwrap();
            let copy = error
                .strip_prefix("capture item 1 starting on line 1: ")
                .unwrap_or(error);
            assert_eq!(copy, expected, "{draft:?}: {json}");
            assert_eq!(snapshot_files(temp.path()), before, "{draft:?}");
        }
    }

    write_file(&config, "plan:\n  max_themes: 30\n");
    let no_vocabulary_hint = format!(
        "`#fxi` is not a task type yet: no Pomodoro has used FXI and no `types:` vocabulary is configured; add `types:` to {} (for a project note whose ` :new-id` tasks go under the FXI Pomodoro, write `@sase^new-id@#fxi`)",
        config.display()
    );
    let before_no_vocabulary = snapshot_files(temp.path());
    for dry_run in [false, true] {
        let args = if dry_run {
            vec!["-d", "@sase^new-id#fxi Fix it"]
        } else {
            vec!["@sase^new-id#fxi Fix it"]
        };
        let (_, json) = capture_error_json(&vault, &day_file, &config, &args);
        assert_eq!(json["code"], "type_not_configured", "{json}");
        let error = json["error"].as_str().unwrap();
        let copy = error
            .strip_prefix("capture item 1 starting on line 1: ")
            .unwrap_or(error);
        assert_eq!(copy, no_vocabulary_hint, "{json}");
        assert_eq!(snapshot_files(temp.path()), before_no_vocabulary);
    }
}

#[test]
fn vocabulary_is_authoritative_and_history_is_the_fallback() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-acceptance",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n- [ ] () — SNACK\n",
    );
    let (_, configured_rejection) =
        capture_error_json(&vault, &day_file, &config, &["Do work #snack"]);
    assert_eq!(configured_rejection["code"], "type_not_configured");

    write_file(&config, "plan:\n  max_themes: 30\n");
    let (_, historical) =
        capture_json(&vault, &day_file, &config, &["Do work #snack"], true);
    assert_eq!(historical["task_theme"]["theme"]["name"], "SNACK");

    let (_, inventory) =
        capture_error_json(&vault, &day_file, &config, &["Do work #later"]);
    assert_eq!(inventory["code"], "type_not_configured");
}

#[test]
fn named_pomodoro_conflict_refuses_before_any_write() {
    let (temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-conflict",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let before = snapshot_files(temp.path());
    let (_, json) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["First @sase:first-1#BUILD #build\n\nSecond @sase:second-1#BUILD #fix"],
    );
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["code"], "type_marker_conflict", "{json}");
    assert!(
        json["error"].as_str().unwrap().contains("Pomodoro BUILD"),
        "{json}"
    );
    // The first item's successful plan cannot write when the second
    // item's resolved destination conflicts.
    assert_eq!(snapshot_files(temp.path()), before);
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
        &["Fix", "it", "@sase:agree-1#FIX", "#fix"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
}

#[test]
fn named_destination_prefixes_are_checked_after_resolution() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-prefix-conflict",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let (_, json) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["Fix it @sase:prefix-1#bui #fix"],
    );
    assert_eq!(json["code"], "type_marker_conflict", "{json}");
    assert!(json["error"].as_str().unwrap().contains("Pomodoro BUILD"));
    assert!(json["error"].as_str().unwrap().contains("task BUILD"));
}

#[test]
fn named_containers_and_composites_accept_a_matching_type() {
    for (label, pomodoros, args) in [
        (
            "container",
            "## Pomodoros\n- [ ] () — FOCUS\n",
            "Fix it @sase:container-1#FOCUS #fix",
        ),
        (
            "composite",
            "## Pomodoros\n- [ ] () — BUILD + FIX\n",
            "Fix it @sase:composite-1#BUILD+FIX #fix",
        ),
    ] {
        let (_temp, vault, day_file, config) = fixture(
            &format!("bob-capture-type-marker-{label}"),
            &project(Some("WRITE"), ""),
            pomodoros,
        );
        let (_, json) = capture_json(&vault, &day_file, &config, &[args], true);
        assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
        assert_eq!(
            json["pomodoro_link_destination"]["name"],
            if label == "container" {
                "FOCUS"
            } else {
                "BUILD+FIX"
            },
            "{json}"
        );
    }
}

#[test]
fn exact_block_ids_and_template_rows_keep_the_marker() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-id-template",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, explicit_id) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Task with id @sase^typed-id#fix"],
        true,
    );
    assert_eq!(explicit_id["block_id"], "typed-id", "{explicit_id}");
    assert!(
        explicit_id["task_line"]
            .as_str()
            .unwrap()
            .contains("[type:: FIX]"),
        "{explicit_id}"
    );

    let (_, template) = capture_json(
        &vault,
        &day_file,
        &config,
        &["@@template:\nTask {0} #fix\n\nparser\n"],
        true,
    );
    assert_eq!(template["template"]["row_count"], 1, "{template}");
    assert_eq!(template["text"], "Task parser", "{template}");
    assert_eq!(template["task_theme"]["theme"]["name"], "FIX", "{template}");
}

#[test]
fn leading_attached_project_marker_accepts_a_spaced_literal() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-leading-attached",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "types:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n  \"DEEP WORK\": violet\n",
        ),
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["@sase^parser#\"DEEP WORK\" Fix the parser"],
        false,
    );
    assert_eq!(json["block_id"], "parser", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "DEEP WORK", "{json}");
    assert!(
        json["task_line"]
            .as_str()
            .unwrap()
            .contains("[type:: DEEP WORK]"),
        "{json}"
    );
}

#[test]
fn root_and_reading_defaults_precede_the_note_default() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-roots-reading",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    write_file(
        &vault.join("area.md"),
        "---\ntype: \"[[area]]\"\ntask_type: FIX\n---\n",
    );
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n  reading_type: READ\n",
            "types:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n  READ: green\n",
        ),
    );

    let (_, rooted) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Follow root [root:: area] #fix @sase"],
        true,
    );
    assert_eq!(rooted["task_theme"]["action"], "kept", "{rooted}");
    assert_eq!(
        rooted["task_theme"]["inherited"]["theme"]["name"], "FIX",
        "{rooted}"
    );
    assert_eq!(
        rooted["task_theme"]["inherited"]["source"], "note",
        "{rooted}"
    );

    let (_, reading) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Read article \\#ref @sase:reading-1"],
        true,
    );
    assert!(reading.get("task_theme").is_none(), "{reading}");
    assert_eq!(
        reading["pomodoro_link_destination"]["type"]["theme"]["name"], "READ",
        "{reading}"
    );
    assert_eq!(
        reading["pomodoro_link_destination"]["type"]["source"], "reading",
        "{reading}"
    );
}

#[test]
fn global_and_forced_routes_preserve_new_task_applicability() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-global-forced",
        &project(Some("WRITE"), "- [ ] #task Parent ^parent\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );

    let (_, global) = capture_json(
        &vault,
        &day_file,
        &config,
        &["@@sase\n\nGlobal task #fix"],
        true,
    );
    assert_eq!(global["task_theme"]["theme"]["name"], "FIX", "{global}");

    for args in [
        "@@sase+parent\n\nA child #fix",
        "-r sase --task parent A child #fix",
    ] {
        let (_, error) = if args.starts_with("-r ") {
            capture_error_json(
                &vault,
                &day_file,
                &config,
                &["-r", "sase", "--task", "parent", "A child #fix"],
            )
        } else {
            capture_error_json(&vault, &day_file, &config, &[args])
        };
        assert_eq!(error["code"], "type_marker_not_applicable", "{error}");
    }
}

#[test]
fn typed_link_uses_the_written_type_before_bare_pomodoro_placement() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-bare-placement",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let (_, dry) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix it @sase:typed-1 #fix"],
        true,
    );
    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix it @sase:typed-1 #fix"],
        false,
    );
    assert_eq!(dry["pomodoro_link_destination"]["name"], "FIX", "{dry}");
    assert_eq!(
        written["pomodoro_link_destination"]["name"], "FIX",
        "{written}"
    );
    assert_eq!(
        written["task_theme"], dry["task_theme"],
        "{dry} vs {written}"
    );
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(day.contains("[[sase#^typed-1]]"), "{day}");
    assert!(day.contains("— FIX"), "{day}");
}

#[test]
fn blocked_new_tasks_keep_their_explicit_type_without_a_link() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-blocked",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Future task @sase:future-1#FIX s:5 #fix"],
        true,
    );
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert_eq!(
        json["link_guard"]["detail"], "not linked (scheduled 2026-11-02)",
        "{json}"
    );
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn task_modifiers_do_not_receive_the_subject_type() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-modifiers",
        &project(Some("WRITE"), "- [ ] #task Parent ^parent\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    write_blocked_tasks_settings(&vault);
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Child task @sase:typed-child #fix &sase:parent"],
        true,
    );
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
    assert!(
        json["task_line"].as_str().unwrap().contains("typed-child"),
        "{json}"
    );
    let parent = fs::read_to_string(vault.join("sase.md")).unwrap();
    assert!(parent.contains("Parent ^parent"), "{parent}");
    assert!(!parent.contains("[type:: FIX]"), "{parent}");
}

#[test]
fn marker_errors_keep_their_json_codes() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-error-codes",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    for (draft, code) in [
        ("Fix it #\"BUILD + FIX\"", "invalid_type_marker"),
        ("Fix it #fix #write", "duplicate_capture_marker"),
        ("#fix", "type_marker_needs_task"),
        ("https://example.test/a\n#fix", "type_marker_not_applicable"),
        (
            "https://example.test/a\n- #fix",
            "type_marker_not_applicable",
        ),
    ] {
        let (_, json) =
            capture_error_json(&vault, &day_file, &config, &[draft]);
        assert_eq!(json["code"], code, "{draft:?}: {json}");
    }
}

#[test]
fn t_colon_spelling_stays_plain_task_text() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-retired-t",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix the parser t:snack"],
        true,
    );
    assert_eq!(json["text"], "Fix the parser t:snack", "{json}");
    assert!(json.get("task_theme").is_none(), "{json}");
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn url_task_override_keeps_the_marker_as_a_new_task() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-type-marker-url-task",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["-R", "https://example.test/a\n#fix"],
        true,
    );
    assert_eq!(json["kind"], "task", "{json}");
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
        .args(["^sase:missing-x", "#fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run existing-target capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .expect("parse refusal JSON");
    // Existing retypes are supported now: a missing target fails honestly
    // for the missing task, never with the retired "not supported yet".
    let error = json["error"].as_str().unwrap_or_default().to_string();
    assert!(!error.contains("not supported yet"), "{json}");
    assert!(
        error.contains("disappeared") || error.contains("no task"),
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
        &["Fix", "the", "parser", "#fix", "@sase:dry-1"],
        true,
    );
    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Fix", "the", "parser", "#fix", "@sase:dry-1"],
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
        .args(["Write", "docs", "#write", "@sase"])
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
