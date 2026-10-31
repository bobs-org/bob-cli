//! Task-theme exceptions written by explicitly named Pomodoro captures.

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
            "themes:\n  BUILD: yellow\n  FIX: blue\n  READ: green\n  WRITE: magenta\n  PLAN: blue\n",
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

fn capture_json_stdin(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    config: &std::path::Path,
    draft: &str,
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
    let output = run_with_stdin(
        command
            .env("BOB_DIR", vault)
            .env("BOB_DAY_FILE", day_file)
            .env("BOB_CONFIG_FILE", config)
            .env("BOB_NOW", NOW),
        draft,
    );
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
fn new_named_task_reports_and_writes_the_resolved_theme_exception() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-new",
        &project(Some("WRITE"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let task_before = fs::read_to_string(vault.join("sase.md")).unwrap();
    let day_before = fs::read_to_string(&day_file).unwrap();
    let (_, dry) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Draft a fix @sase:new-fix#fi"],
        true,
    );
    assert_eq!(dry["task_theme"]["action"], "set", "{dry}");
    assert_eq!(dry["task_theme"]["theme"]["name"], "FIX", "{dry}");
    assert_eq!(dry["task_theme"]["previous"], Value::Null, "{dry}");
    assert_eq!(dry["task_theme"]["inherited"]["source"], "note", "{dry}");
    assert_eq!(dry["task_theme"]["inherited"]["theme"]["name"], "WRITE");
    assert!(
        dry["task_line"]
            .as_str()
            .unwrap()
            .contains("[type:: FIX] [created::2026-10-28] ^new-fix"),
        "{dry}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).unwrap(),
        task_before
    );
    assert_eq!(fs::read_to_string(&day_file).unwrap(), day_before);

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .arg("Draft a fix @sase:new-fix#fi")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("dry-run capture human output");
    assert_success(&human);
    assert!(
        stdout(&human).contains("  type  would set type →"),
        "{}",
        format_output(&human)
    );

    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Draft a fix @sase:new-fix#fi"],
        false,
    );
    assert_eq!(written["task_theme"], dry["task_theme"]);
    assert_eq!(written["task_line"], dry["task_line"]);
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).unwrap(),
        concat!(
            "---\ntype: [[project]]\ntheme: WRITE\n---\n# Sase\n\n## Tasks\n\n",
            "- [*] #task Draft a fix [type:: FIX] [created::2026-10-28] ^new-fix\n",
        )
    );
}

#[test]
fn new_named_capture_uses_the_staged_task_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-type-staged-field",
        &project(None, ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Draft a fix [type:: BUILD] @sase:new-fix#fi"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["previous"]["name"], "BUILD", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn new_named_capture_uses_the_global_default_after_note_defaults() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-type-default",
        &project(None, ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n  default_type: CHORE\n",
            "types:\n  CHORE: magenta\n  FIX: blue\n",
        ),
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Draft a fix @sase:new-fix#fi"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert_eq!(
        json["task_theme"]["inherited"]["source"], "default",
        "{json}"
    );
    assert_eq!(
        json["task_theme"]["inherited"]["theme"]["name"], "CHORE",
        "{json}"
    );
}

#[test]
fn a_two_item_batch_stages_and_reports_both_task_theme_writes() {
    let day_before = "## Pomodoros\n- [ ] () — FIX\n- [ ] () — BUILD\n";
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-batch",
        &project(Some("WRITE"), ""),
        day_before,
    );
    let before = fs::read_to_string(vault.join("sase.md")).unwrap();
    let draft =
        "Draft a fix @sase:first#FIX\n\nBuild a thing @sase:second#BUILD\n";
    let (_, dry) = capture_json_stdin(&vault, &day_file, &config, draft, true);
    let captures = dry["captures"].as_array().expect("bulk captures");
    assert_eq!(captures.len(), 2, "{dry}");
    assert_eq!(captures[0]["task_theme"]["action"], "set");
    assert_eq!(captures[0]["task_theme"]["theme"]["name"], "FIX");
    assert_eq!(captures[1]["task_theme"]["action"], "set");
    assert_eq!(captures[1]["task_theme"]["theme"]["name"], "BUILD");
    assert_eq!(fs::read_to_string(vault.join("sase.md")).unwrap(), before);

    let (_, json) =
        capture_json_stdin(&vault, &day_file, &config, draft, false);
    let captures = json["captures"].as_array().expect("bulk captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(captures[0]["task_theme"]["action"], "set");
    assert_eq!(captures[0]["task_theme"], dry["captures"][0]["task_theme"]);
    assert_eq!(captures[1]["task_theme"], dry["captures"][1]["task_theme"]);
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("Draft a fix [type:: FIX]"),
        "first task theme should be committed"
    );
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("Build a thing [type:: BUILD]"),
        "second task theme should be committed"
    );
}

#[test]
fn ensure_next_named_relocation_sets_a_theme_after_freshness_edits() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-ensure-next",
        &project(Some("BUILD"), "- [ ] #task Work ^work\n"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIX\n",
            "- [ ] () — LATER\n",
            "  - [[sase#^work]]\n",
        ),
    );
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["@sase+work#FIX"], false);
    assert_success(&output);
    assert_eq!(json["toggle_behavior"], "ensure_next", "{json}");
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    let line = json["task_line"].as_str().unwrap();
    assert!(
        line.starts_with("- [*] #task Work [type:: FIX] [fresh:: 2026-10-28]"),
        "{line}"
    );
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains(line),
        "the task post-image should be present in the note"
    );
}

#[test]
fn named_capture_writes_a_type_without_a_shown_suggestion() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-no-suggestion",
        &project(None, "- [ ] #task Work ^work\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work#FIX"], false);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "an explicit #FIX still writes when the cascade has no suggestion"
    );
}

#[test]
fn matching_type_and_guarded_links_do_not_write_a_task_theme() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-same-or-guarded",
        &project(Some("BUILD"), "- [ ] #task Work [theme:: FIX] ^work\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, same_theme) =
        capture_json(&vault, &day_file, &config, &["^sase:work#FIX"], false);
    assert!(same_theme.get("task_theme").is_none(), "{same_theme}");
    assert_eq!(
        same_theme["task_line"].as_str().unwrap(),
        "- [*] #task Work [theme:: FIX] [fresh:: 2026-10-28] ^work"
    );

    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("job.md"),
        "- [ ] #task Prep [id:: job__prep] ^prep\n",
    );
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n\n",
            "- [ ] #task Parent #prj #hide [dependsOn:: job__prep] ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\ntheme: BUILD\n---\n\n",
            "- [?] #task Child #prj #hide #prj_is_blocked ^prj\n",
            "- [?] #task Work #prj_is_blocked [theme:: READ] ^work\n",
        ),
    );
    let (_, guarded) =
        capture_json(&vault, &day_file, &config, &["@child:work#FIX"], false);
    assert_eq!(guarded["kind"], "link_guard", "{guarded}");
    assert!(guarded.get("task_theme").is_none(), "{guarded}");
    assert!(
        fs::read_to_string(vault.join("child.md"))
            .unwrap()
            .contains("[theme:: READ]"),
        "guarded task keeps its existing theme bytes"
    );
}

#[test]
fn ref_today_named_selection_retypes_to_the_explicit_destination() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-ref-today",
        &project(
            Some("BUILD"),
            "- [ ] #task #ref [[ref/papers/attention]] [theme:: FIX] ^ref-attention\n",
        ),
        "## Pomodoros\n- [ ] () — READ\n",
    );
    write_file(
        &vault.join("ref/papers/attention.md"),
        "---\nstatus: ready\ntitle: Attention\nid: attention\n---\n# Attention\n",
    );
    write_toggle_task_settings(&vault);
    let output = bob_command()
        .arg("ref")
        .arg("today")
        .arg("attention")
        .arg("-p")
        .arg("WRITE")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run ref today");
    assert_success(&output);
    let json: Value =
        serde_json::from_slice(&output.stdout).expect("ref today JSON");
    assert_eq!(json["capture"]["task_theme"]["action"], "set", "{json}");
    assert_eq!(
        json["capture"]["task_theme"]["theme"]["name"], "WRITE",
        "{json}"
    );
    assert_eq!(
        json["capture"]["task_theme"]["previous"]["name"], "FIX",
        "{json}"
    );
    assert_eq!(
        json["capture"]["task_theme"]["inherited"]["theme"]["name"], "READ",
        "{json}"
    );
    let task_after = fs::read_to_string(vault.join("sase.md")).unwrap();
    assert!(
        task_after.contains("[type:: WRITE]")
            && !task_after.contains("[theme::"),
        "ref-today retypes the task to its explicit destination: {task_after}"
    );
}

#[test]
fn unnamed_destinations_do_not_write_task_themes() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-unnamed",
        &project(Some("BUILD"), "- [ ] #task Work [theme:: READ] ^work\n"),
        "## Pomodoros\n- [ ] ()\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work"], false);
    assert!(json.get("task_theme").is_none(), "{json}");
    assert!(
        json["task_line"]
            .as_str()
            .unwrap()
            .contains("[theme:: READ]"),
        "{json}"
    );
}

#[test]
fn inherited_build_plus_hash_fix_writes_type_fix() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-inherited-build",
        &project(Some("BUILD"), "- [ ] #task Work ^work\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work#fix"], false);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert_eq!(json["task_theme"]["inherited"]["theme"]["name"], "BUILD");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "inherited BUILD plus #fix writes [type:: FIX]"
    );
}

#[test]
fn hash_build_clears_a_fix_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-clear",
        &project(Some("BUILD"), "- [ ] #task Work [type:: FIX] ^work\n"),
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work#build"], false);
    assert_eq!(json["task_theme"]["action"], "cleared", "{json}");
    assert_eq!(json["task_theme"]["theme"], Value::Null, "{json}");
    let line = json["task_line"].as_str().unwrap();
    assert!(
        !line.contains("[type::") && !line.contains("[theme::"),
        "{line}"
    );
}

#[test]
fn hash_plan_writes_an_exempt_type() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-plan",
        &project(Some("BUILD"), "- [ ] #task Work ^work\n"),
        "## Pomodoros\n- [ ] () — PLAN\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work#plan"], false);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "PLAN", "{json}");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: PLAN]"),
        "PLAN is a legal retype result"
    );
}

#[test]
fn hash_later_keeps_the_task_type() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-later",
        &project(Some("BUILD"), "- [ ] #task Work [type:: FIX] ^work\n"),
        "## Pomodoros\n- [ ] () — LATER\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work#later"], false);
    assert!(json.get("task_theme").is_none(), "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
    let warnings = json
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|warning| warning.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|message| message
            == "LATER is a label · tasks keep their own types"),
        "{json}"
    );
}

#[test]
fn hash_fix_into_build_plus_fix_keeps_fix_and_retypes_write() {
    let day = "## Pomodoros\n- [ ] () — BUILD + FIX\n";
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-composite-keep",
        &project(Some("BUILD"), "- [ ] #task Keep [type:: FIX] ^keep\n"),
        day,
    );
    let (_, keep) =
        capture_json(&vault, &day_file, &config, &["^sase:keep#fix"], false);
    assert!(keep.get("task_theme").is_none(), "{keep}");
    assert!(
        keep["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{keep}"
    );

    write_file(
        &vault.join("sase.md"),
        &project(Some("BUILD"), "- [ ] #task Other [type:: WRITE] ^other\n"),
    );
    let (_, retyped) =
        capture_json(&vault, &day_file, &config, &["^sase:other#fix"], false);
    assert_eq!(retyped["task_theme"]["action"], "set", "{retyped}");
    assert_eq!(retyped["task_theme"]["theme"]["name"], "FIX", "{retyped}");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "WRITE under BUILD + FIX plus #fix becomes FIX"
    );
}

#[test]
fn named_project_note_bullets_retype_linked_tasks() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-project-note",
        &project(Some("BUILD"), ""),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json_stdin(
        &vault,
        &day_file,
        &config,
        "Ship it @sase^exit@#fix\n- Draft the memo :draft-memo\n",
        false,
    );
    assert_eq!(json["kind"], "project_note", "{json}");
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    let line = json["project_note"]["task_links"][0]["task_line"]
        .as_str()
        .unwrap();
    assert!(line.contains("[type:: FIX]"), "{json}");
    assert!(
        fs::read_to_string(vault.join("sase_exit.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "named project-note bullets retype each linked task"
    );
}

#[test]
fn active_existing_task_link_start_shorthand_uses_the_resolved_name() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-capture-task-theme-start-shorthand",
        &project(Some("BUILD"), "- [ ] #task Work [theme:: BUILD] ^work\n"),
        "## Pomodoros\n- [ ] ()\n",
    );
    write_toggle_task_settings(&vault);
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["^sase:work=fix"], false);
    assert_success(&output);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(json.get("pomodoro_start").is_some(), "{json}");
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "the task line should carry the selected theme"
    );
}
