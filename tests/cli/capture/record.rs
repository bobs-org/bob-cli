//! Record-write CLI tests (phase `record-writes`).
//!
//! Claimed record notes plan and install through the capture
//! transaction: dry-run and commit share Markdown, collisions refuse
//! without suffixes, and later-item failures write nothing.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;

const NOW: &str = "2026-10-07 13:40:00";

const RECORDS_CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: "🍽️"
    dated_notes: true
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          liked: "Liked"
      category:
        required: true
      location: {}
      visited:
        values: date
      tags:
        list: true
      website:
        values: url
"#;

const COMPLETE_RAMEN: &str = "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark\n- visited:: today\n- Spicy tuna was great.";

fn capture(
    vault: &Path,
    config: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.env("BOB_NOW", NOW);
    command.env("BOB_CONFIG_FILE", config);
    command.env("NO_COLOR", "1");
    command.arg("capture").arg("-b").arg(vault);
    for arg in args {
        command.arg(arg);
    }
    command
}

fn write_records_config(temp: &TempDir) -> std::path::PathBuf {
    let config = temp.path().join("config.yml");
    write_file(&config, RECORDS_CONFIG);
    config
}

fn json_output(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn complete_fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-record-write");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = write_records_config(&temp);
    (temp, vault, config)
}

fn daily_note(vault: &Path) -> std::path::PathBuf {
    vault.join("2026-10-07.md")
}

#[test]
fn capture_record_dry_run_and_commit_share_markdown() {
    let (_temp, vault, config) = complete_fixture();
    let dry = capture(
        &vault,
        &config,
        &["--dry-run", "-f", "json", COMPLETE_RAMEN],
    )
    .output()
    .expect("dry-run record capture");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    assert_eq!(dry_json["ok"], true);
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["kind"], "record_note");
    assert_eq!(dry_json["placement"], "created");
    assert_eq!(dry_json["relative_target"], "eat/ramen_nagi.md");
    let files = dry_json["note_files"].as_array().expect("note_files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "eat/ramen_nagi.md");
    assert_eq!(files[0]["action"], "create");
    assert_eq!(files[0]["record_type"], "restaurant");
    let markdown = files[0]["markdown"].as_str().expect("markdown");
    assert!(markdown.contains("title: \"Ramen Nagi\""));
    assert!(markdown.contains("status: liked"));
    assert!(markdown.contains("category: \"Ramen\""));
    assert!(markdown.contains("location: \"Clark\""));
    assert!(markdown.contains("visited: 2026-10-07"));
    assert!(markdown.contains("- 2026-10-07: Spicy tuna was great."));
    assert!(!vault.join("eat").exists(), "dry-run creates no folder");
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox task");
    assert!(!daily_note(&vault).exists(), "no daily note");

    let committed = capture(&vault, &config, &["-f", "json", COMPLETE_RAMEN])
        .output()
        .expect("commit record capture");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(committed_json["ok"], true);
    assert_eq!(committed_json["dry_run"], false);
    assert_eq!(
        committed_json["note_files"][0]["markdown"],
        dry_json["note_files"][0]["markdown"]
    );
    let written = fs::read_to_string(vault.join("eat/ramen_nagi.md"))
        .expect("read committed record");
    assert_eq!(
        written,
        dry_json["note_files"][0]["markdown"].as_str().unwrap()
    );
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox task");
    assert!(!daily_note(&vault).exists(), "no daily note");
}

#[test]
fn capture_record_human_dry_run_prints_markdown() {
    let (_temp, vault, config) = complete_fixture();
    let output = capture(&vault, &config, &["--dry-run", COMPLETE_RAMEN])
        .output()
        .expect("human dry-run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("would create") && text.contains("new restaurant"),
        "human dry-run header: {text}"
    );
    assert!(
        text.contains("title: \"Ramen Nagi\""),
        "prints markdown: {text}"
    );
    assert!(!vault.join("eat").exists(), "dry-run creates no folder");
}

#[test]
fn capture_record_human_success_names_type_and_fields() {
    let (_temp, vault, config) = complete_fixture();
    let output = capture(&vault, &config, &[COMPLETE_RAMEN])
        .output()
        .expect("human success");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("new restaurant") && text.contains("eat/ramen_nagi.md"),
        "human success header: {text}"
    );
    assert!(text.contains("Liked") || text.contains("liked"), "{text}");
    assert!(text.contains("1 note"), "{text}");
}

#[test]
fn capture_mixed_task_and_record_succeeds() {
    let (_temp, vault, config) = complete_fixture();
    let draft = format!("Buy milk\n\n{COMPLETE_RAMEN}");
    let output = capture(&vault, &config, &["-f", "json", &draft])
        .output()
        .expect("mixed capture");
    assert_success(&output);
    let json = json_output(&output);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "task");
    assert_eq!(captures[1]["kind"], "record_note");
    assert_eq!(json["note_files"].as_array().unwrap().len(), 1);
    assert!(vault.join("mac_inbox.md").exists(), "task inbox");
    assert!(vault.join("eat/ramen_nagi.md").exists(), "record note");
}

#[test]
fn capture_later_item_failure_writes_nothing() {
    let (_temp, vault, config) = complete_fixture();
    let draft = "Buy milk\n\nRamen Nagi @eat/\n- status:: liked";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("later-item failure");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("required field"),
        "incomplete message: {json}"
    );
    assert!(
        json["draft"]
            .as_str()
            .unwrap_or_default()
            .contains("category::"),
        "ready-to-copy draft: {json}"
    );
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox write");
    assert!(!vault.join("eat").exists(), "no record folder");
}

#[test]
fn capture_same_draft_collision_writes_nothing() {
    let (_temp, vault, config) = complete_fixture();
    let draft = format!("{COMPLETE_RAMEN}\n\n{COMPLETE_RAMEN}");
    let output = capture(&vault, &config, &["-f", "json", &draft])
        .output()
        .expect("same-draft collision");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["code"], "record_exists");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("record already exists"),
        "{json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("@eat/ramen_nagi"),
        "names reserved update: {json}"
    );
    assert!(!vault.join("eat").exists(), "no writes");
}

#[test]
fn capture_on_disk_and_case_insensitive_collisions() {
    let (_temp, vault, config) = complete_fixture();
    fs::create_dir_all(vault.join("eat")).expect("create eat");
    write_file(&vault.join("eat/ramen_nagi.md"), "# existing\n");
    let output = capture(&vault, &config, &["-f", "json", COMPLETE_RAMEN])
        .output()
        .expect("on-disk collision");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["code"], "record_exists");
    assert_eq!(
        fs::read_to_string(vault.join("eat/ramen_nagi.md")).unwrap(),
        "# existing\n",
        "existing bytes preserved"
    );

    let temp = TempDir::new("bob-cli-capture-record-case");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("create eat");
    let config = write_records_config(&temp);
    write_file(&vault.join("eat/RAMEN_NAGI.md"), "# Case\n");
    let output = capture(&vault, &config, &["-f", "json", COMPLETE_RAMEN])
        .output()
        .expect("case collision");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["code"], "record_exists");
    assert_eq!(
        fs::read_to_string(vault.join("eat/RAMEN_NAGI.md")).unwrap(),
        "# Case\n"
    );
}

#[test]
fn capture_explicit_stem_and_missing_folder() {
    let (_temp, vault, config) = complete_fixture();
    let draft =
        "Ramen Nagi @eat/^ramen-nagi\n- status:: liked\n- category:: Ramen";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("explicit stem");
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["relative_target"], "eat/ramen-nagi.md");
    assert!(vault.join("eat/ramen-nagi.md").exists());
    assert!(vault.join("eat").is_dir(), "commit creates missing folder");
}

#[test]
fn capture_required_default_none_and_list_semantics() {
    let (_temp, vault, config) = complete_fixture();
    let empty_required = "Ramen Nagi @eat/\n- status::\n- category:: Ramen";
    let output = capture(&vault, &config, &["-f", "json", empty_required])
        .output()
        .expect("empty required");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert!(
        json["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "status"),
        "{json}"
    );
    assert!(
        json["draft"]
            .as_str()
            .unwrap_or_default()
            .contains("status:: …"),
        "empty required stays in draft: {json}"
    );
    assert!(
        json["draft"].as_str().unwrap_or_default().contains("liked"),
        "choice hint: {json}"
    );

    let listed = "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- tags:: spicy\n- tags:: lunch\n- location::";
    let output = capture(&vault, &config, &["-f", "json", listed])
        .output()
        .expect("list and none");
    assert_success(&output);
    let written =
        fs::read_to_string(vault.join("eat/ramen_nagi.md")).expect("read");
    assert!(
        written.contains("tags:\n  - \"spicy\"\n  - \"lunch\"\n"),
        "{written}"
    );
    assert!(
        !written.contains("location:"),
        "empty optional omitted: {written}"
    );
}

#[test]
fn capture_template_later_row_failure_writes_nothing() {
    let (_temp, vault, config) = complete_fixture();
    let draft = "{0} @eat/ %t\n- status:: {1}\n- category:: Ramen\n\n\"Ramen Nagi\" liked\n\"Bad Place\" nope\n";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("template later fail");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        !vault.join("eat").exists(),
        "no writes after later row fail"
    );
}

#[test]
fn capture_global_route_does_not_override_record() {
    let (_temp, vault, config) = complete_fixture();
    let draft = format!("@@inbox\n{COMPLETE_RAMEN}\n\nBuy milk");
    let output = capture(&vault, &config, &["-f", "json", &draft])
        .output()
        .expect("@@ precedence");
    assert_success(&output);
    let json = json_output(&output);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "record_note");
    assert_eq!(captures[0]["relative_target"], "eat/ramen_nagi.md");
    assert_eq!(captures[1]["kind"], "task");
    assert_eq!(captures[1]["relative_target"], "inbox.md");
    assert_eq!(json["global_destination"]["route"], "inbox");
    assert!(vault.join("inbox.md").exists(), "sibling inherits @@inbox");
    assert!(
        vault.join("eat/ramen_nagi.md").exists(),
        "record keeps @eat/"
    );
}

#[test]
fn capture_record_field_urls_stay_opaque() {
    let (_temp, vault, config) = complete_fixture();
    let url = "https://mail.google.com/mail/u/0/#inbox/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx";
    let draft = format!(
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- website:: {url}\n- see {url}"
    );
    let output = capture(&vault, &config, &["-f", "json", &draft])
        .output()
        .expect("url opacity");
    assert_success(&output);
    let written =
        fs::read_to_string(vault.join("eat/ramen_nagi.md")).expect("read");
    assert!(
        written.matches(url).count() >= 2,
        "field and note keep original URL:\n{written}"
    );
    assert!(!written.contains("gmail"), "no gmail rewrite in {written}");
}

#[test]
fn capture_parse_reports_record_note_wire() {
    let temp = TempDir::new("bob-cli-capture-record-parse");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = write_records_config(&temp);

    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@eat/ Ramen Nagi")
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["grammar"], 20);
    assert_eq!(json["mode"], "record_note");
    assert_eq!(json["route"], "eat");
    assert_eq!(json["body"], "Ramen Nagi");
    let kinds: Vec<&str> = json["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .map(|span| span["kind"].as_str().unwrap_or_default())
        .collect();
    assert!(kinds.contains(&"record_route"), "{kinds:?}");
}
