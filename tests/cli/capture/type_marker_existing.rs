//! `#<type>` retyping of existing tasks with D8 relocation.

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
            "plan:\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n",
        ),
    );
    write_file(&vault.join("sase.md"), task_note);
    write_file(&day_file, day);
    (temp, vault, day_file, config)
}

fn write_ready_settings(vault: &std::path::Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
    );
}

fn project(theme: Option<&str>, tasks: &str) -> String {
    let theme = theme
        .map(|theme| format!("theme: {theme}\n"))
        .unwrap_or_default();
    format!("---\ntype: [[project]]\n{theme}---\n# Sase\n\n## Tasks\n{tasks}")
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

#[test]
fn whole_item_link_retype_sets_the_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-link-set",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-x\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-x", "#fix"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn whole_item_link_retype_reports_kept_without_a_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-link-kept",
        &project(Some("WRITE"), "- [ ] #task Write docs ^write-x\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:write-x", "#write"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "kept", "{json}");
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn whole_item_link_retype_clears_the_authored_exception() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-link-cleared",
        &project(
            Some("WRITE"),
            "- [ ] #task Write docs [type:: WRITE] ^write-y\n",
        ),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:write-y", "#write"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "cleared", "{json}");
    assert!(
        !json["task_line"].as_str().unwrap().contains("[type::"),
        "{json}"
    );
}

#[test]
fn ensure_next_retype_sets_field_and_moves_link() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-ensure",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-e\n"),
        "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-e]]\n- [ ] () — FIX\n",
    );
    let (_, json) =
        capture_json(&vault, &day_file, &config, &["@sase+fix-e #fix"], false);
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn ensure_next_attached_literal_retypes_before_placing_the_task_link() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-ensure-literal",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^parser\n"),
        "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^parser]]\n- [ ] () — DEEP WORK\n",
    );
    write_file(
        &config,
        concat!(
            "plan:\n",
            "types:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n  \"DEEP WORK\": violet\n",
        ),
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["@sase+parser #\"DEEP WORK\""],
        false,
    );
    assert_eq!(json["task_theme"]["theme"]["name"], "DEEP WORK", "{json}");
    assert_eq!(json["pomodoro_link_action"], "moved", "{json}");
    assert_eq!(
        json["pomodoro_link_destination"]["name"], "DEEP WORK",
        "{json}"
    );
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(
        day.contains("- [ ] () — DEEP WORK\n  - [[sase#^parser]]"),
        "{day}"
    );
    assert!(!day.contains("BUILD\n  - [[sase#^parser]]"), "{day}");
}

#[test]
fn toggle_retype_sets_field() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-toggle",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-t\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["@sase+fix-t!", "#fix"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert!(
        json["task_line"].as_str().unwrap().contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn relationship_owner_retype_sets_field() {
    let temp = TempDir::new("bob-type-existing-rel");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_ready_settings(&vault);
    write_file(
        &config,
        concat!(
            "plan:\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n",
        ),
    );
    write_file(&vault.join("foo.md"), "- [ ] #task Buy shoes ^bar\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n");
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(["@body+excercise", "&foo:bar", "#fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run relationship retype");
    assert_success(&output);
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .expect("parse relationship JSON");
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    assert!(
        fs::read_to_string(vault.join("body.md"))
            .unwrap()
            .contains("[type:: FIX]"),
        "{json}"
    );
}

#[test]
fn named_form_conflict_refuses_before_any_write() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-conflict",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-c\n"),
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let before_task = fs::read_to_string(vault.join("sase.md")).unwrap();
    let before_day = fs::read_to_string(&day_file).unwrap();
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-c#BUILD", "#fix"],
        true,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    let merged = &json["pomodoro_link_destination"]["merged"];
    assert_eq!(merged["before"], "BUILD", "{json}");
    assert_eq!(merged["after"], "BUILD + FIX", "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).unwrap(),
        before_task
    );
    assert_eq!(fs::read_to_string(&day_file).unwrap(), before_day);
}

#[test]
fn closed_target_refuses_without_a_write() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-closed",
        &project(Some("WRITE"), "- [x] #task Done ^done-c\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let before_task = fs::read_to_string(vault.join("sase.md")).unwrap();
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(["^sase:done-c", "#fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run closed retype");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).unwrap(),
        before_task
    );
}

#[test]
fn stale_strict_key_retypes_without_refusing() {
    let temp = TempDir::new("bob-type-existing-strict");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_ready_settings(&vault);
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 1\n  strict: true\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n",
        ),
    );
    write_file(&vault.join("foo.md"), "- [ ] #task Buy shoes ^bar\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");
    // Ledger already holds BUILD; retyping to FIX adds a second type.
    // The live link sits in a multi-link running entry so the D8 applier
    // must create a new FIX entry. The retired strict cap never refuses.
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[body#^excercise]]\n",
            "  - [[foo#^bar]]\n",
        ),
    );
    let before_day = fs::read_to_string(&day_file).unwrap();
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(["@body+excercise", "&foo:bar", "#fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-07-10 09:15:00");
    // Use a relationship-only retype so the ledger must relocate (no bare
    // placement move).
    let output = command.output().expect("run stale-strict retype");
    assert_success(&output);
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .expect("parse retype JSON");
    assert!(json["task_theme"].is_object(), "{json}");
    assert!(json.get("code").is_none(), "no refusal code: {json}");
    assert_ne!(
        fs::read_to_string(&day_file).unwrap(),
        before_day,
        "retype writes the new FIX entry"
    );
}

#[test]
fn dry_run_and_real_run_agree_on_existing_retype() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-dryrun",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-d\n"),
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, dry) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-d", "#fix"],
        true,
    );
    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-d", "#fix"],
        false,
    );
    assert_eq!(
        written["task_theme"], dry["task_theme"],
        "{dry} vs {written}"
    );
    assert_eq!(written["task_line"], dry["task_line"], "{dry} vs {written}");
    assert_eq!(
        written.get("type_relocation"),
        dry.get("type_relocation"),
        "{dry} vs {written}"
    );
}

#[test]
fn already_queued_link_retype_relocates_the_ledger() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-relocate",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-q\n"),
        "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-q]]\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-q", "#fix"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    // The live link moves from BUILD toward FIX, so a relocation reports.
    assert!(json.get("type_relocation").is_some(), "{json}");
    let relocation = &json["type_relocation"];
    assert!(
        [
            "move_to",
            "rename_entry",
            "rename_placeholder",
            "create_below",
            "append_component"
        ]
        .contains(&relocation["outcome"].as_str().unwrap_or("")),
        "{json}"
    );
    // Human output names the ledger move.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .args(["^sase:fix-q", "#fix"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("dry-run human");
    assert_success(&human);
    assert!(
        stdout(&human).contains("ledger"),
        "{}",
        format_output(&human)
    );
    assert_eq!(relocation["outcome"], "move_to", "{json}");
    assert_eq!(relocation["to"], "FIX", "{json}");
    assert_eq!(json["pomodoro_name"], "FIX", "{json}");
    let blocks = json["pomodoro_blocks"].as_array().expect("blocks");
    assert!(
        blocks.iter().any(|block| {
            block["name"] == "FIX"
                && block["lines"].as_array().is_some_and(|lines| {
                    lines.iter().any(|line| line["change"] != "unchanged")
                })
        }),
        "move_to preview must show a real dest diff: {json}"
    );
}

#[test]
fn named_destination_prefixes_are_checked_after_resolution() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-prefix-conflict",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-p\n"),
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-p#bui #fix"],
        false,
    );
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    let merged = &json["pomodoro_link_destination"]["merged"];
    assert_eq!(merged["before"], "BUILD", "{json}");
    assert_eq!(merged["after"], "BUILD + FIX", "{json}");
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(day.contains("— BUILD + FIX"), "{day}");
}

#[test]
fn rename_placeholder_preview_rewrites_the_headline() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-rename-preview",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-r\n"),
        "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-r]]\n",
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-r", "#fix"],
        true,
    );
    assert_eq!(
        json["type_relocation"]["outcome"], "rename_placeholder",
        "{json}"
    );
    assert_eq!(json["type_relocation"]["to"], "FIX", "{json}");
    assert_eq!(json["pomodoro_name"], "FIX", "{json}");
    let block = json["pomodoro_blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .find(|block| block["name"] == "FIX")
        .expect("FIX block");
    assert_eq!(block["created"], false, "{json}");
    let headline = &block["lines"][0];
    assert_eq!(headline["change"], "changed", "{json}");
    assert!(
        headline["text"].as_str().unwrap().contains("— FIX"),
        "{json}"
    );
    assert!(
        headline["before"]
            .as_str()
            .unwrap_or("")
            .contains("— BUILD"),
        "{json}"
    );
}

#[test]
fn type_relocation_restamps_a_running_entry_when_only_its_name_changes() {
    let task_note = concat!(
        "---\n",
        "type: [[area]]\n",
        "task_type: CHORE\n",
        "offclock_types:\n",
        "  - MEET\n",
        "---\n# Love\n## Tasks\n",
        "- [ ] #task Lunch with Kelly ^lunch\n",
    );
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930** [t:: 30m]) — CHORE\n",
        "  - [[sase#^lunch]]\n",
    );
    let (_temp, vault, day_file, config) =
        fixture("bob-type-existing-clock-name", task_note, day);
    write_file(
        &config,
        concat!("plan:\n", "themes:\n  CHORE: yellow\n  MEET: blue\n",),
    );

    let (_, dry) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:lunch", "#meet"],
        true,
    );
    assert!(dry.get("type_relocation").is_some(), "{dry}");
    assert_eq!(
        fs::read_to_string(&day_file).unwrap(),
        day,
        "dry-run leaves the source note unchanged"
    );

    let (_, written) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:lunch", "#meet"],
        false,
    );
    assert_eq!(
        written["pomodoro_blocks"], dry["pomodoro_blocks"],
        "{dry}\n{written}"
    );
    let after = fs::read_to_string(&day_file).unwrap();
    assert!(
        after.contains("(**0900-0930** [off:: 30m]) — MEET"),
        "{after}"
    );
}

#[test]
fn create_below_preview_reports_a_created_destination() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-type-existing-create-preview",
        &project(Some("WRITE"), "- [ ] #task Fix parser ^fix-c\n"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — BUILD\n",
            "  - [[sase#^fix-c]]\n",
            "  - [[sase#^other-x]]\n",
        ),
    );
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["^sase:fix-c", "#fix"],
        true,
    );
    assert_eq!(json["type_relocation"]["outcome"], "create_below", "{json}");
    assert_eq!(json["type_relocation"]["to"], "FIX", "{json}");
    assert_eq!(json["creates_pomodoro"], true, "{json}");
    assert_eq!(json["pomodoro_name"], "FIX", "{json}");
    let dest = json["pomodoro_blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .find(|block| block["name"] == "FIX")
        .expect("FIX block");
    assert_eq!(dest["created"], true, "{json}");
    assert_eq!(json["pomodoro_link_destination"]["name"], "FIX", "{json}");
    assert_eq!(
        json["pomodoro_link_destination"]["role"], "created",
        "{json}"
    );
}
