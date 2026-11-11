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
      rating:
        values: number
  podcast:
    dir: podcasts
    label: Podcast
    emoji: "🎙️"
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          subscribed: "Subscribed"
      category:
        required: true
      url:
        values: url
"#;

const COMPLETE_RAMEN: &str = "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark\n- visited:: today\n- Spicy tuna was great.";

const NINJA: &str = "---\ntitle: Ninja Sushi\ntype: \"[[restaurant]]\"\ncreated: 2025-01-01 12:00\nstatus: to_try\ncategory: Sushi\nlocation: Clark\n---\n## Notes\n- 2025-01-02: First visit.\n";

fn write_ninja(vault: &Path) -> std::path::PathBuf {
    fs::create_dir_all(vault.join("eat")).expect("create eat folder");
    let path = vault.join("eat/ninja_sushi.md");
    write_file(&path, NINJA);
    path
}

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

fn capture_parse(vault: &Path, config: &Path, draft: &str) -> Output {
    bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_DIR", vault)
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-parse")
}

fn apply_rewrite(input: &str, diagnostic: &Value) -> String {
    let rewrite = &diagnostic["rewrite"];
    let start = rewrite["start"].as_u64().expect("rewrite start") as usize;
    let end = rewrite["end"].as_u64().expect("rewrite end") as usize;
    let replacement = rewrite["replacement"]
        .as_str()
        .expect("rewrite replacement");
    format!("{}{}{}", &input[..start], replacement, &input[end..])
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
fn note_files_preserve_typed_values_and_match_parse_and_commit() {
    let (_temp, vault, config) = complete_fixture();
    let draft = "Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location::\n- visited:: today\n- tags:: spicy\n- tags:: lunch\n- rating:: 4.5";
    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", draft])
        .output()
        .expect("typed dry-run");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    let properties = dry_json["note_files"][0]["properties"]
        .as_array()
        .expect("properties");
    let property = |key: &str| {
        properties
            .iter()
            .find(|property| property["key"] == key)
            .unwrap_or_else(|| panic!("missing property {key}: {properties:?}"))
    };
    assert_eq!(property("status")["value"], "liked");
    assert_eq!(property("status")["display"], "Liked");
    assert_eq!(
        property("tags")["value"],
        serde_json::json!(["spicy", "lunch"])
    );
    assert_eq!(property("tags")["display"], "spicy, lunch");
    assert_eq!(property("rating")["value"], 4.5);
    assert_eq!(property("rating")["display"], "4.5");
    assert_eq!(property("visited")["value"], "2026-10-07");
    assert!(property("location").get("value").is_none());
    assert!(property("location").get("display").is_none());

    let parsed = capture_parse(&vault, &config, draft);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    let parsed_fields = parsed_json["record"]["fields"].as_array().unwrap();
    let parsed_status = parsed_fields
        .iter()
        .find(|field| field["key"] == "status")
        .unwrap();
    let parsed_tags = parsed_fields
        .iter()
        .find(|field| field["key"] == "tags")
        .unwrap();
    let parsed_rating = parsed_fields
        .iter()
        .find(|field| field["key"] == "rating")
        .unwrap();
    assert_eq!(parsed_status["value"], "liked");
    assert_eq!(parsed_tags["value"], serde_json::json!(["spicy", "lunch"]));
    assert_eq!(parsed_rating["value"], 4.5);

    let committed = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("typed commit");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(
        committed_json["note_files"][0]["properties"],
        dry_json["note_files"][0]["properties"]
    );
    assert_eq!(
        committed_json["note_files"][0]["markdown"],
        dry_json["note_files"][0]["markdown"]
    );
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
fn capture_record_note_summary_shape_matches_dry_run_and_commit() {
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
    let summary = &dry_json["record_note"];
    assert_eq!(summary["action"], "create");
    assert_eq!(summary["label"], "Restaurant");
    assert_eq!(summary["emoji"], "🍽️");
    assert_eq!(summary["path"], "eat/ramen_nagi.md");
    assert_eq!(summary["notes"], 1);
    assert!(
        summary["properties"].as_array().is_some(),
        "summary properties: {summary}"
    );
    assert!(
        summary.get("markdown").is_none(),
        "summary must not duplicate markdown: {summary}"
    );
    assert!(
        summary.get("note_file").is_none(),
        "summary must not duplicate note_file: {summary}"
    );
    assert_eq!(
        summary["properties"], dry_json["note_files"][0]["properties"],
        "summary properties match note_files"
    );

    let committed = capture(&vault, &config, &["-f", "json", COMPLETE_RAMEN])
        .output()
        .expect("commit record capture");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(
        committed_json["record_note"], dry_json["record_note"],
        "dry-run and commit summaries agree"
    );

    let task = capture(&vault, &config, &["-f", "json", "Buy milk"])
        .output()
        .expect("ordinary task capture");
    assert_success(&task);
    assert!(
        json_output(&task).get("record_note").is_none(),
        "ordinary items omit record_note"
    );

    let (_temp, mixed_vault, mixed_config) = complete_fixture();
    let mixed = format!("Buy milk\n\n{COMPLETE_RAMEN}");
    let mixed_output =
        capture(&mixed_vault, &mixed_config, &["-f", "json", &mixed])
            .output()
            .expect("mixed capture");
    assert_success(&mixed_output);
    let mixed_json = json_output(&mixed_output);
    let captures = mixed_json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert!(
        captures[0].get("record_note").is_none(),
        "mixed task item omits record_note"
    );
    assert_eq!(captures[1]["record_note"]["path"], "eat/ramen_nagi.md");
    assert!(
        mixed_json.get("record_note").is_none(),
        "top-level mirror follows the first (task) item: {mixed_json}"
    );
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
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("add to it with"),
        "offers working update spelling: {json}"
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
fn record_exists_fix_updates_the_existing_record_without_appending_title() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "Ninja Sushi @eat/\n- Visited again with Kelly.";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    let diagnostic = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_exists")
        .expect("duplicate diagnostic");
    assert_eq!(diagnostic["rewrite"]["label"], "Add to it");
    let rewritten = apply_rewrite(draft, diagnostic);
    assert_eq!(rewritten, "@eat/ninja_sushi\n- Visited again with Kelly.");
    let update = json_output(&capture_parse(&vault, &config, &rewritten));
    assert_eq!(update["record"]["action"], "update");
    assert_eq!(update["record"]["stem"], "ninja_sushi");

    let preview =
        capture(&vault, &config, &["--dry-run", "-f", "json", &rewritten])
            .output()
            .expect("dry-run Add to it rewrite");
    assert_success(&preview);
    let preview = json_output(&preview);
    let file = preview["note_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "eat/ninja_sushi.md")
        .expect("updated note file");
    assert_eq!(file["action"], "update");
    assert!(file["body"].as_str().unwrap().contains("Visited again"));
    assert!(
        !file["body"]
            .as_str()
            .unwrap()
            .contains("2026-10-07: Ninja Sushi"),
        "the removed duplicate title is not appended as a note"
    );
}

#[test]
fn record_exists_fix_uses_a_staged_target_but_not_an_incomplete_or_unsafe_one()
{
    let (_temp, vault, config) = complete_fixture();
    let first = "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark";
    let second = "Ramen Nagi @eat/\n- Visited again.";
    let draft = format!("{first}\n\n{second}");
    let parsed = json_output(&capture_parse(&vault, &config, &draft));
    let diagnostic = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_exists")
        .expect("same-draft duplicate");
    let rewritten = apply_rewrite(&draft, diagnostic);
    assert!(rewritten.contains("@eat/ramen_nagi\n- Visited again."));
    let parsed_update =
        json_output(&capture_parse(&vault, &config, &rewritten));
    assert_eq!(parsed_update["items"][1]["record"]["action"], "update");
    let dry =
        capture(&vault, &config, &["--dry-run", "-f", "json", &rewritten])
            .output()
            .expect("dry-run staged update");
    assert_success(&dry);

    let incomplete = concat!(
        "Ramen Nagi @eat/\n- status:: liked\n\n",
        "Ramen Nagi @eat/\n- category:: Ramen\n- location:: Clark"
    );
    let parsed = json_output(&capture_parse(&vault, &config, incomplete));
    let duplicate = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_exists")
        .expect("incomplete staged duplicate");
    assert!(duplicate.get("rewrite").is_none());

    fs::create_dir_all(vault.join("eat/ghost.md"))
        .expect("non-record path collision");
    let non_record =
        "Ghost @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark";
    let parsed = json_output(&capture_parse(&vault, &config, non_record));
    let collision = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_exists")
        .expect("non-record path collision");
    assert!(collision.get("rewrite").is_none());

    write_file(&vault.join("eat/ninja_sushi.md"), NINJA);
    write_file(&vault.join("eat/NINJA_SUSHI.md"), NINJA);
    let ambiguous = "Ninja Sushi @eat/\n- Another visit.";
    let parsed = json_output(&capture_parse(&vault, &config, ambiguous));
    let collision = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_exists")
        .expect("ambiguous record collision");
    assert!(collision.get("rewrite").is_none());
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
fn record_field_template_text_stays_opaque_and_template_expands_sigils() {
    let (_temp, vault, config) = complete_fixture();
    let ordinary =
        "Ramen @eat/\n- status:: liked\n- category:: Ramen\n- location:: %t";
    let output =
        capture(&vault, &config, &["--dry-run", "-f", "json", ordinary])
            .output()
            .expect("opaque field value");
    assert_success(&output);
    assert_eq!(json_output(&output)["kind"], "record_note");
    assert_eq!(
        json_output(&capture_parse(&vault, &config, ordinary))["mode"],
        "record_note"
    );
    let expanded_ordinary =
        capture(&vault, &config, &["-E", "-f", "json", ordinary])
            .output()
            .expect("expand ordinary record with literal %t field");
    assert_success(&expanded_ordinary);
    assert_eq!(json_output(&expanded_ordinary)["expanded"], ordinary);

    let template = "{0} @eat/ %t\n- status:: {1}\n- category:: Ramen\n- location:: {2}\n\nRamen liked %c\n";
    let parsed = capture_parse(&vault, &config, template);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    assert_eq!(
        parsed_json["template"]["rows"][0]["record"]["title"],
        "Ramen"
    );
    assert_eq!(
        parsed_json["template"]["rows"][0]["record"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "location")
            .unwrap()["value"],
        "%c"
    );
    let executed =
        capture(&vault, &config, &["--dry-run", "-f", "json", template])
            .output()
            .expect("template-injected task sigil stays a field value");
    assert_success(&executed);
    assert_eq!(json_output(&executed)["kind"], "record_note");
    assert_eq!(
        json_output(&executed)["note_files"][0]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "location")
            .unwrap()["value"],
        "%c"
    );

    let template_t = template.replace("Ramen liked %c", "Ramen liked %t");
    let parsed_t = capture_parse(&vault, &config, &template_t);
    assert_success(&parsed_t);
    assert_eq!(
        json_output(&parsed_t)["template"]["rows"][0]["record"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "location")
            .unwrap()["value"],
        "%t"
    );
    let executed_t =
        capture(&vault, &config, &["--dry-run", "-f", "json", &template_t])
            .output()
            .expect("template-injected record field %t stays literal");
    assert_success(&executed_t);
    assert_eq!(json_output(&executed_t)["kind"], "record_note");
    assert_eq!(
        json_output(&executed_t)["note_files"][0]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "location")
            .unwrap()["value"],
        "%t"
    );
    let expanded_t =
        capture(&vault, &config, &["-E", "-f", "json", &template_t])
            .output()
            .expect("expand record template with literal %t field");
    assert_success(&expanded_t);
    assert!(json_output(&expanded_t)["expanded"]
        .as_str()
        .unwrap()
        .contains("- location:: %t"));
}

#[cfg(unix)]
#[test]
fn forced_clip_refuses_record_before_running_clipboard_command() {
    use std::os::unix::fs::PermissionsExt;

    let (temp, vault, config) = complete_fixture();
    let invoked = temp.path().join("clipboard-was-run");
    let command = temp.path().join("fake-clipboard");
    write_file(
        &command,
        &format!(
            "#!/bin/sh\nprintf called > '{}'\nprintf clipboard\n",
            invoked.display()
        ),
    );
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755))
        .expect("make fake clipboard executable");
    let output =
        capture(&vault, &config, &["--clip", "-f", "json", COMPLETE_RAMEN])
            .env("BOB_CLIPBOARD_CMD", &command)
            .output()
            .expect("record with forced clip");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["code"], "record_marker_unsupported");
    assert!(!invoked.exists(), "refusal must precede clipboard access");
    assert!(!vault.join("mac_inbox.md").exists());
    assert!(!vault.join("eat").exists());
}

#[test]
fn capture_decodes_escaped_hash_in_record_title() {
    let (_temp, vault, config) = complete_fixture();
    let draft = "Ramen \\#READ @eat/\n- status:: liked\n- category:: Ramen";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("escaped title capture");
    assert_success(&output);
    let written = fs::read_to_string(vault.join("eat/ramen_read.md"))
        .expect("read record");
    assert!(written.contains("title: \"Ramen #READ\""), "{written}");
    assert!(!written.contains("Ramen \\#READ"), "{written}");
}

#[test]
fn capture_parse_resolves_record_templates_with_mapped_values() {
    let (_temp, vault, config) = complete_fixture();
    let draft = "{0} @eat/ %t\n- status:: {1}\n- category:: Ramen\n\n\"Ramen Nagi\" liked\n\"Udon\" WRONG\n\"Café 🍜\" \"\"\n";
    let output = capture_parse(&vault, &config, draft);
    assert_success(&output);
    let json = json_output(&output);
    let rows = json["template"]["rows"].as_array().expect("template rows");
    assert_eq!(rows[0]["record"]["title"], "Ramen Nagi");
    assert_eq!(rows[0]["record"]["missing"], serde_json::json!([]));
    assert_eq!(rows[1]["record"]["title"], "Udon");
    let status = rows[1]["record"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "status")
        .unwrap();
    assert_eq!(status["state"], "invalid");
    let wrong = draft.find("WRONG").unwrap();
    assert_eq!(
        status["range"],
        serde_json::json!({"start": wrong, "end": wrong + 5})
    );
    assert_eq!(rows[2]["record"]["missing"], serde_json::json!(["status"]));
    assert!(rows[2]["needs"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("record_value")));
    assert!(json["diagnostics"].as_array().unwrap().iter().any(|entry| {
        entry["code"] == "record_invalid_value"
            && entry["range"] == serde_json::json!([wrong, wrong + 5])
    }));
    assert_eq!(json["items"][2]["record"], rows[1]["record"]);

    let execution =
        capture(&vault, &config, &["--dry-run", "-f", "json", draft])
            .output()
            .expect("execute invalid template record");
    assert_eq!(
        execution.status.code(),
        Some(2),
        "{}",
        format_output(&execution)
    );
    assert_eq!(json_output(&execution)["code"], "record_invalid_value");

    let collision =
        "{0} @eat/ %t\n- status:: liked\n- category:: Ramen\n\nRamen\nRamen\n";
    let parsed_collision =
        json_output(&capture_parse(&vault, &config, collision));
    assert!(parsed_collision["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["code"] == "record_exists"));
    let collision_run =
        capture(&vault, &config, &["--dry-run", "-f", "json", collision])
            .output()
            .expect("execute colliding template records");
    assert_eq!(collision_run.status.code(), Some(2));
    assert_eq!(json_output(&collision_run)["code"], "record_exists");

    let repeated = "{0} @eat/ %t\n- status:: liked\n- category:: Ramen\n- tags:: {1}\n- tasty broth\n- tags:: soup\n\nRamen spicy\n";
    let repeated_json = json_output(&capture_parse(&vault, &config, repeated));
    let tags = repeated_json["template"]["rows"][0]["record"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "tags")
        .unwrap();
    assert_eq!(tags["value"], serde_json::json!(["spicy", "soup"]));
    assert_eq!(tags["value_ranges"].as_array().unwrap().len(), 2);
    for (needle, range) in [
        ("spicy", tags["value_ranges"][0].clone()),
        ("soup", tags["value_ranges"][1].clone()),
    ] {
        let start = range["start"].as_u64().unwrap() as usize;
        let end = range["end"].as_u64().unwrap() as usize;
        assert_eq!(&repeated[start..end], needle);
    }

    let crlf = "{0} @eat/ %t\r\n- status:: {1}\r\n- category:: Ramen\r\n\r\n\"Café 🍜\" liked\r\n";
    let crlf_json = json_output(&capture_parse(&vault, &config, crlf));
    assert_eq!(
        crlf_json["template"]["rows"][0]["record"]["title"],
        "Café 🍜"
    );
    for span in crlf_json["spans"].as_array().unwrap() {
        let start = span["start"].as_u64().unwrap() as usize;
        let end = span["end"].as_u64().unwrap() as usize;
        assert!(crlf.is_char_boundary(start));
        assert!(crlf.is_char_boundary(end));
    }
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
    assert_eq!(json["grammar"], 26);
    assert_eq!(json["mode"], "record_note");
    assert_eq!(json["record"]["assist_version"], 1);
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

#[test]
fn capture_parse_reports_existing_record_values_and_update_ranges() {
    let temp = TempDir::new("bob-cli-capture-record-update-parse");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("create eat folder");
    let config = write_records_config(&temp);
    write_file(
        &vault.join("eat/ninja_sushi.md"),
        "---\ntitle: Ninja Sushi\ntype: \"[[restaurant]]\"\ncreated: 2025-01-01 12:00\nstatus: to_try\ncategory: Sushi\nlocation: Clark\n---\n## Notes\n- 2025-01-02: First visit.\n",
    );

    let draft = "Went back, still great @eat/ninja_sushi\n- status:: liked";
    let output = capture_parse(&vault, &config, draft);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["grammar"], 26);
    assert_eq!(json["mode"], "record_update");
    assert_eq!(json["body"], "Went back, still great");
    assert_eq!(json["needs"], serde_json::json!([]));
    let record = &json["record"];
    assert_eq!(record["action"], "update");
    assert_eq!(record["stem"], "ninja_sushi");
    assert_eq!(record["stem_source"], "existing");
    assert_eq!(record["title"], "Ninja Sushi");
    assert_eq!(record["notes"], 1);
    let status = record["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "status")
        .unwrap();
    assert_eq!(status["value"], "liked");
    assert_eq!(status["display"], "Liked");
    assert_eq!(status["before"], "to_try");
    assert_eq!(status["before_display"], "To try");
    let (route_start, route_end) = json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|span| span["kind"] == "record_route")
        .map(|span| {
            (
                span["start"].as_u64().unwrap() as usize,
                span["end"].as_u64().unwrap() as usize,
            )
        })
        .unwrap_or_else(|| {
            panic!("record route span missing: {}", json["spans"])
        });
    assert_eq!(&draft[route_start..route_end], "@eat/ninja_sushi");
    assert_spans_sorted_and_disjoint(&json);
}

/// The Mac's `validatedSpanRanges` rule: spans are already ordered, and
/// each `start` is at or after the previous `end`.
fn assert_spans_sorted_and_disjoint(json: &Value) {
    let spans = json["spans"].as_array().expect("spans");
    let mut previous_end = 0u64;
    for span in spans {
        let start = span["start"].as_u64().expect("span start");
        let end = span["end"].as_u64().expect("span end");
        assert!(
            start >= previous_end,
            "overlapping or unsorted spans: {}",
            json["spans"]
        );
        previous_end = end;
    }
}

#[test]
fn capture_parse_record_spans_are_disjoint() {
    let temp = TempDir::new("bob-cli-capture-record-disjoint-spans");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("create eat folder");
    let config = write_records_config(&temp);
    write_ninja(&vault);

    for draft in [
        "@eat/",
        "Ramen Nagi @eat/^custom-stem",
        "@eat/ninja_sushi",
        "Went back @eat/ninja_sushi\n- status:: liked\n- still great",
    ] {
        let output = capture_parse(&vault, &config, draft);
        assert_success(&output);
        let json = json_output(&output);
        let route = json["spans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|span| span["kind"] == "record_route")
            .unwrap_or_else(|| panic!("record_route missing for {draft:?}"));
        let start = route["start"].as_u64().unwrap() as usize;
        let end = route["end"].as_u64().unwrap() as usize;
        let token = &draft[start..end];
        assert!(
            token.starts_with("@eat/"),
            "route token for {draft:?}: {token:?}"
        );
        assert_eq!(
            json["spans"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|span| span["kind"] == "record_stem")
                .count(),
            0,
            "no nested record_stem for {draft:?}: {}",
            json["spans"]
        );
        assert_spans_sorted_and_disjoint(&json);
    }
}

#[test]
fn capture_update_one_line_append_shares_dry_run_and_commit_bytes() {
    let (_temp, vault, config) = complete_fixture();
    let path = write_ninja(&vault);
    let draft = "Went back, still great @eat/ninja_sushi";
    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", draft])
        .output()
        .expect("dry-run one-line update");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    assert_eq!(dry_json["ok"], true);
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["kind"], "record_update");
    assert_eq!(dry_json["placement"], "updated");
    assert_eq!(dry_json["relative_target"], "eat/ninja_sushi.md");
    let files = dry_json["note_files"].as_array().expect("note_files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["action"], "update");
    assert_eq!(files[0]["path"], "eat/ninja_sushi.md");
    let markdown = files[0]["markdown"].as_str().expect("markdown");
    assert!(
        markdown.contains("- 2026-10-07: Went back, still great"),
        "dated parent note: {markdown}"
    );
    assert!(
        markdown.contains("- 2025-01-02: First visit."),
        "keeps existing note: {markdown}"
    );
    assert!(
        markdown.contains("status: to_try"),
        "untouched field stays: {markdown}"
    );
    let status = files[0]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "status")
        .unwrap();
    assert_eq!(status["value"], "to_try");
    assert_eq!(status["before"], "to_try");
    assert_eq!(status["before_display"], "To try");
    assert!(
        files[0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["change"] == "added"
                && line["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Went back, still great")),
        "block-diff added the note: {}",
        files[0]["lines"]
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        NINJA,
        "dry-run does not write"
    );

    let committed = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("commit one-line update");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(committed_json["dry_run"], false);
    assert_eq!(
        committed_json["note_files"][0]["markdown"],
        dry_json["note_files"][0]["markdown"]
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        dry_json["note_files"][0]["markdown"].as_str().unwrap()
    );
}

#[test]
fn capture_update_field_only_and_human_preview() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "@eat/ninja_sushi\n- status:: liked";
    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", draft])
        .output()
        .expect("field-only dry-run");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    let markdown = dry_json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    assert!(markdown.contains("status: liked"), "{markdown}");
    assert!(
        !markdown.contains("2026-10-07"),
        "field-only adds no note: {markdown}"
    );
    let status = dry_json["note_files"][0]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "status")
        .unwrap();
    assert_eq!(status["value"], "liked");
    assert_eq!(status["before"], "to_try");
    assert_eq!(status["before_display"], "To try");
    assert_eq!(status["display"], "Liked");

    let human = capture(&vault, &config, &["--dry-run", draft])
        .output()
        .expect("human dry-run update");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("would update") && text.contains("restaurant"),
        "human dry-run names update: {text}"
    );
    assert!(text.contains("status: liked"), "prints markdown: {text}");

    let committed = capture(&vault, &config, &[draft])
        .output()
        .expect("human commit update");
    assert_success(&committed);
    let text = stdout(&committed);
    assert!(
        text.contains("updated restaurant")
            && text.contains("eat/ninja_sushi.md"),
        "human success names update: {text}"
    );
}

#[test]
fn capture_update_undated_podcast_and_missing_notes() {
    let (_temp, vault, config) = complete_fixture();
    fs::create_dir_all(vault.join("podcasts")).expect("podcasts folder");
    write_file(
        &vault.join("podcasts/talk_python.md"),
        "---\ntitle: Talk Python\ntype: \"[[podcast]]\"\nstatus: subscribed\ncategory: python\n---\n# Talk Python\n",
    );
    let draft = "Listened to the async episode @podcasts/talk_python";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("undated podcast update");
    assert_success(&output);
    let json = json_output(&output);
    let markdown = json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    assert!(
        markdown.contains("- Listened to the async episode"),
        "undated note: {markdown}"
    );
    assert!(
        !markdown.contains("2026-10-07"),
        "podcast notes are undated: {markdown}"
    );
    assert!(
        markdown.contains("## Notes"),
        "creates missing Notes: {markdown}"
    );
}

#[test]
fn capture_update_preserves_messy_note_bytes() {
    let (_temp, vault, config) = complete_fixture();
    let original = include_str!("../../fixtures/records/source/eat/ahrres.md");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(&vault.join("eat/ahrres.md"), original);
    let draft = "Revisit was great @eat/ahrres\n- location:: Clark";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("messy note update");
    assert_success(&output);
    let json = json_output(&output);
    let markdown = json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    let expected = original
        .replacen("location: \"Westfield\"\n", "location: \"Clark\"\n", 1)
        .replacen(
            "## Notes\n\n## Source",
            "## Notes\n\n- 2026-10-07: Revisit was great\n\n## Source",
            1,
        );
    assert_eq!(markdown, expected);
    assert_eq!(
        fs::read_to_string(vault.join("eat/ahrres.md")).unwrap(),
        expected
    );
}

#[test]
fn capture_update_replaces_lists_in_request_order() {
    let (_temp, vault, config) = complete_fixture();
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(
        &vault.join("eat/lists.md"),
        "---\ntitle: Lists\ntype: \"[[restaurant]]\"\nstatus: liked\ncategory: Ramen\nlocation: Clark\ntags:\n  - old\n  - stale\n---\n## Notes\n",
    );
    let draft = "@eat/lists\n- tags:: spicy\n- tags:: lunch";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("list replace");
    assert_success(&output);
    let json = json_output(&output);
    let markdown = json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    assert!(
        markdown.contains("spicy") && markdown.contains("lunch"),
        "{markdown}"
    );
    assert!(!markdown.contains("old"), "{markdown}");
    let tags = json["note_files"][0]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "tags")
        .unwrap();
    assert_eq!(tags["value"], serde_json::json!(["spicy", "lunch"]));
    assert_eq!(tags["before"], serde_json::json!(["old", "stale"]));
}

#[test]
fn capture_ordered_same_target_updates_accumulate() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "First revisit @eat/ninja_sushi\n- status:: liked\n\nSecond revisit @eat/ninja_sushi\n- location:: Westfield";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("ordered same-target updates");
    assert_success(&output);
    let json = json_output(&output);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "record_update");
    assert_eq!(captures[1]["kind"], "record_update");
    let files = json["note_files"].as_array().expect("note_files");
    assert_eq!(files.len(), 2);
    let first = files[0]["markdown"].as_str().unwrap();
    let second = files[1]["markdown"].as_str().unwrap();
    assert!(first.contains("status: liked"), "{first}");
    assert!(first.contains("- 2026-10-07: First revisit"), "{first}");
    assert!(
        !first.contains("Westfield"),
        "first item does not see later fields: {first}"
    );
    assert!(second.contains("status: liked"), "{second}");
    assert!(second.contains("Westfield"), "{second}");
    assert!(
        second.contains("- 2026-10-07: First revisit")
            && second.contains("- 2026-10-07: Second revisit"),
        "second item keeps the first note: {second}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat/ninja_sushi.md")).unwrap(),
        second
    );
}

#[test]
fn capture_create_then_update_aligns_parse_dry_run_and_commit() {
    let (_temp, vault, config) = complete_fixture();
    let draft = format!(
        "{COMPLETE_RAMEN}\n\nWent back for extra spice @eat/ramen_nagi\n- location:: Westfield"
    );
    let parsed = capture_parse(&vault, &config, &draft);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    let items = parsed_json["items"].as_array().expect("parse items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["mode"], "record_note");
    assert_eq!(items[1]["mode"], "record_update");
    assert_eq!(items[1]["record"]["action"], "update");
    assert_eq!(items[1]["record"]["stem"], "ramen_nagi");
    assert_eq!(items[1]["record"]["stem_source"], "existing");
    let location = items[1]["record"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "location")
        .unwrap();
    assert_eq!(location["value"], "Westfield");
    assert_eq!(location["before"], "Clark");

    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", &draft])
        .output()
        .expect("create-then-update dry-run");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    let files = dry_json["note_files"].as_array().expect("note_files");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["action"], "create");
    assert_eq!(files[1]["action"], "update");
    let created = files[0]["markdown"].as_str().unwrap();
    let updated = files[1]["markdown"].as_str().unwrap();
    assert!(created.contains("location: \"Clark\""), "{created}");
    assert!(updated.contains("location: \"Westfield\""), "{updated}");
    assert!(
        updated.contains("- 2026-10-07: Went back for extra spice"),
        "{updated}"
    );
    assert!(
        !vault.join("eat").exists(),
        "dry-run does not create the folder"
    );

    let committed = capture(&vault, &config, &["-f", "json", &draft])
        .output()
        .expect("create-then-update commit");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(
        committed_json["note_files"][1]["markdown"],
        dry_json["note_files"][1]["markdown"]
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat/ramen_nagi.md")).unwrap(),
        updated
    );
}

#[test]
fn capture_mixed_task_create_update_and_later_invalid_writes_nothing() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let mixed = format!(
        "Buy milk\n\n{COMPLETE_RAMEN}\n\nWent back @eat/ninja_sushi\n- status:: liked"
    );
    let output = capture(&vault, &config, &["-f", "json", &mixed])
        .output()
        .expect("mixed task/create/update");
    assert_success(&output);
    let json = json_output(&output);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 3);
    assert_eq!(captures[0]["kind"], "task");
    assert_eq!(captures[1]["kind"], "record_note");
    assert_eq!(captures[2]["kind"], "record_update");
    assert!(vault.join("mac_inbox.md").exists(), "task inbox");
    assert!(vault.join("eat/ramen_nagi.md").exists(), "created record");
    assert!(fs::read_to_string(vault.join("eat/ninja_sushi.md"))
        .unwrap()
        .contains("status: liked"));

    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let invalid = "Went back @eat/ninja_sushi\n- status:: liked\n\nBroken @eat/\n- status:: liked";
    let output = capture(&vault, &config, &["-f", "json", invalid])
        .output()
        .expect("later invalid item");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(vault.join("eat/ninja_sushi.md")).unwrap(),
        NINJA,
        "later invalid item writes nothing"
    );
}

#[test]
fn capture_noop_update_does_not_rewrite() {
    let (_temp, vault, config) = complete_fixture();
    let path = write_ninja(&vault);
    let before = fs::metadata(&path).unwrap();
    let output = capture(&vault, &config, &["-f", "json", "@eat/ninja_sushi"])
        .output()
        .expect("no-op update");
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["kind"], "record_update");
    assert_eq!(json["note_files"][0]["action"], "update");
    assert_eq!(json["note_files"][0]["markdown"].as_str().unwrap(), NINJA);
    assert_eq!(fs::read_to_string(&path).unwrap(), NINJA);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(
        before.modified().unwrap(),
        after.modified().unwrap(),
        "no-op does not rewrite the note"
    );
}

#[test]
fn capture_update_empty_required_and_historical_warning() {
    let (_temp, vault, config) = complete_fixture();
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(
        &vault.join("eat/legacy.md"),
        "---\ntitle: Legacy\ntype: \"[[restaurant]]\"\nstatus: liked\n---\n## Notes\n",
    );
    let empty_required = "@eat/legacy\n- status::";
    let output = capture(&vault, &config, &["-f", "json", empty_required])
        .output()
        .expect("empty required update");
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

    let warning_draft = "Still good @eat/legacy";
    let output = capture(&vault, &config, &["-f", "json", warning_draft])
        .output()
        .expect("historical warning update");
    assert_success(&output);
    let json = json_output(&output);
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("bob records check")),
        "forwards historical warning: {warnings:?}"
    );
    assert!(
        fs::read_to_string(vault.join("eat/legacy.md"))
            .unwrap()
            .contains("- 2026-10-07: Still good"),
        "warning does not block the update"
    );
}

#[test]
fn update_optional_empty_reports_no_record_value_and_removes_key() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    // `location` is optional; an empty edit is a legal key removal.
    let draft = "@eat/ninja_sushi\n- location::";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    assert_eq!(parsed["record"]["action"], "update");
    assert_eq!(parsed["record"]["missing"], serde_json::json!([]));
    assert!(
        !parsed["needs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|need| need == "record_value"),
        "optional removal needs nothing: {parsed}"
    );
    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", draft])
        .output()
        .expect("dry-run optional removal");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    let markdown = dry_json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    assert!(!markdown.contains("location:"), "key removed: {markdown}");
}

#[test]
fn update_required_empty_still_needs_record_value() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "@eat/ninja_sushi\n- status::";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    assert_eq!(parsed["record"]["action"], "update");
    assert!(
        parsed["needs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|need| need == "record_value"),
        "required empty still needs a value: {parsed}"
    );
    assert_eq!(
        parsed["record"]["missing"],
        serde_json::json!(["status"]),
        "{parsed}"
    );
}

#[test]
fn update_optional_empty_batch_items_report_no_record_value() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    fs::write(
        vault.join("eat/ramen_nagi.md"),
        "---\ntitle: Ramen Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n# Ramen\n",
    )
    .expect("second record");
    // Two-item draft exercises the batch `items[]` merge path.
    let draft =
        "@eat/ninja_sushi\n- location::\n\n@eat/ramen_nagi\n- location::";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    for item in parsed["items"].as_array().expect("items") {
        assert_eq!(item["record"]["missing"], serde_json::json!([]));
        assert!(
            !item["needs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|need| need == "record_value"),
            "batch optional removal needs nothing: {item}"
        );
    }
    assert!(
        !parsed["needs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|need| need == "record_value"),
        "top-level needs nothing: {parsed}"
    );
}

#[test]
fn update_recovery_draft_keeps_parent_note_without_suggestions() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    // Missing required `status` forces the incomplete-draft path.
    let draft = "Went back @eat/ninja_sushi\n- status::";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("incomplete update");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    let draft_text = json["draft"].as_str().unwrap_or_default();
    assert!(
        draft_text.contains("Went back @eat/ninja_sushi"),
        "keeps parent note: {draft_text}"
    );
    assert!(
        draft_text.contains("- status::"),
        "offers the missing key: {draft_text}"
    );
    assert!(
        !draft_text.contains("category::"),
        "no optional suggestion lines on updates: {draft_text}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat/ninja_sushi.md")).unwrap(),
        NINJA,
        "no write on incomplete"
    );
}

#[test]
fn update_record_title_falls_back_to_stem_not_parent_note() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "Went back @eat/ninja_sushy\n- status:: liked";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    assert_eq!(parsed["record"]["action"], "update");
    assert_eq!(parsed["record"]["stem"], "ninja_sushy");
    assert_eq!(
        parsed["record"]["title"], "ninja_sushy",
        "unresolved update title is the stem: {parsed}"
    );
}

#[test]
fn update_unknown_target_reports_not_found_with_suggestion() {
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let draft = "Went back @eat/ninja_sushy\n- status:: liked";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    let codes: Vec<&str> = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["code"].as_str().unwrap_or_default())
        .collect();
    assert!(
        codes.contains(&"record_not_found"),
        "unknown stem is not-found: {parsed}"
    );
    let diagnostic = parsed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == "record_not_found")
        .unwrap();
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ninja_sushi"),
        "did-you-mean names the stem: {diagnostic}"
    );
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("unknown update");
    assert_ne!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json_output(&output)["code"], "record_not_found");
    assert_eq!(
        fs::read_to_string(vault.join("eat/ninja_sushi.md")).unwrap(),
        NINJA,
        "no write on unknown target"
    );
}

#[test]
fn update_ambiguous_target_refuses_without_writing() {
    let temp = TempDir::new("bob-cli-record-ambiguous");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("create eat");
    let config = write_records_config(&temp);
    for name in ["Foo.md", "foo.md"] {
        // Skip on case-insensitive filesystems where the second write would
        // clobber the first.
        let path = vault.join(format!("eat/{name}"));
        if path.exists() {
            return;
        }
        write_file(
            &path,
            &format!("---\ntitle: {name}\nstatus: liked\ncategory: Ramen\n---\n# {name}\n"),
        );
    }
    if !vault.join("eat/Foo.md").exists() || !vault.join("eat/foo.md").exists()
    {
        return;
    }
    let draft = "Note @eat/foo\n- status:: liked";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    assert!(
        parsed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["code"] == "record_not_found"),
        "ambiguous stem is not-found: {parsed}"
    );
    let before_foo = fs::read_to_string(vault.join("eat/Foo.md")).unwrap();
    let before_lower = fs::read_to_string(vault.join("eat/foo.md")).unwrap();
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("ambiguous update");
    assert_ne!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(vault.join("eat/Foo.md")).unwrap(),
        before_foo,
        "no write"
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat/foo.md")).unwrap(),
        before_lower,
        "no write"
    );
}

#[cfg(unix)]
#[test]
fn update_symlinked_target_refuses_without_writing() {
    use std::os::unix::fs::symlink;
    let (_temp, vault, config) = complete_fixture();
    write_ninja(&vault);
    let link = vault.join("eat/alias.md");
    if symlink(vault.join("eat/ninja_sushi.md"), &link).is_err() {
        return;
    }
    let draft = "Revisit @eat/alias\n- status:: liked";
    let parsed = json_output(&capture_parse(&vault, &config, draft));
    assert!(
        parsed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["code"] == "record_invalid_value"),
        "symlink is an unsafe target: {parsed}"
    );
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("symlink update");
    assert_ne!(output.status.code(), Some(0), "{}", format_output(&output));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "alias still a symlink"
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat/ninja_sushi.md")).unwrap(),
        NINJA,
        "real note unchanged"
    );
}

#[test]
fn update_noop_prefilled_edit_changes_no_bytes() {
    let (_temp, vault, config) = complete_fixture();
    fs::create_dir_all(vault.join("eat")).expect("create eat");
    // Quoted scalar and flow-style list must round-trip as a no-op.
    let original = "---\ntitle: Ninja Sushi\nstatus: liked\ncategory: Sushi\nlocation: \"Clark\"\ntags: [hot, soup]\n---\n# Ninja\n";
    write_file(&vault.join("eat/ninja_sushi.md"), original);
    let draft = "@eat/ninja_sushi\n- location:: Clark\n- tags:: hot, soup";
    let dry = capture(&vault, &config, &["--dry-run", "-f", "json", draft])
        .output()
        .expect("dry-run no-op update");
    assert_success(&dry);
    let json = json_output(&dry);
    let file = json["note_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "eat/ninja_sushi.md")
        .expect("note file");
    assert_eq!(
        file["markdown"].as_str().unwrap_or_default(),
        original,
        "equal values change no bytes: {file}"
    );
    assert!(
        file["lines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|line| line["change"] == "unchanged"),
        "all lines unchanged: {file}"
    );
}

const ALBUM_BODY: &str = "---\ntype: [[area]]\n---\n# Body\n";

const ALBUM_EAT: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: City restaurants worth revisiting.\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields: {}\n---\n# Restaurants and Cafes\n";

fn album_fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-album");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("body.md"), ALBUM_BODY);
    write_file(&vault.join("eat.md"), ALBUM_EAT);
    let config = write_records_config(&temp);
    (temp, vault, config)
}

#[test]
fn capture_album_create_writes_canonical_storage_with_metadata() {
    let (_temp, vault, config) = album_fixture();
    // No fields typed: the album schema (no required fields) wins over the
    // transitional config, which would demand status/category/location.
    let dry = capture(
        &vault,
        &config,
        &["--dry-run", "-f", "json", "Ramen Nagi @eat/"],
    )
    .output()
    .expect("album dry-run");
    assert_success(&dry);
    let dry_json = json_output(&dry);
    let dry_file = dry_json["note_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "eat/ramen_nagi.md")
        .expect("album note file");
    assert_eq!(dry_file["action"], "create");
    assert_eq!(dry_file["record_type"], "restaurant", "noun identity kept");
    assert_eq!(dry_file["source"], "album");
    assert_eq!(dry_file["album"]["stem"], "eat");
    assert_eq!(dry_file["album"]["title"], "Restaurants and Cafes");
    assert_eq!(dry_file["album"]["owner"], "body");
    let markdown = dry_file["markdown"].as_str().unwrap_or_default();
    assert!(markdown.contains("parent: \"[[eat]]\""), "{markdown}");
    assert!(markdown.contains("type: \"[[record]]\""), "{markdown}");
    assert!(!markdown.contains("[[restaurant]]"), "{markdown}");
    assert!(!vault.join("eat").exists(), "dry-run creates no folder");

    let committed =
        capture(&vault, &config, &["-f", "json", "Ramen Nagi @eat/"])
            .output()
            .expect("album commit");
    assert_success(&committed);
    let committed_json = json_output(&committed);
    assert_eq!(
        committed_json["note_files"][0]["markdown"],
        dry_json["note_files"][0]["markdown"],
        "dry-run bytes equal committed bytes"
    );
    let written = fs::read_to_string(vault.join("eat/ramen_nagi.md"))
        .expect("record note written");
    assert_eq!(
        written,
        dry_json["note_files"][0]["markdown"].as_str().unwrap()
    );
}

#[test]
fn capture_album_human_success_names_album_and_owner() {
    let (_temp, vault, config) = album_fixture();
    let output = capture(&vault, &config, &["Ramen Nagi @eat/"])
        .output()
        .expect("human album capture");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("new restaurant")
            && text.contains("Restaurants and Cafes")
            && text.contains("body"),
        "human success names album and owner: {text}"
    );
}

#[test]
fn capture_bare_album_task_home_fails_and_writes_nothing() {
    let (_temp, vault, config) = album_fixture();
    let before =
        fs::read_to_string(vault.join("eat.md")).expect("read album note");
    let draft = "Ramen Nagi @eat/\n\nBlaze @eat";
    let output = capture(&vault, &config, &["-f", "json", draft])
        .output()
        .expect("mixed album capture");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json = json_output(&output);
    assert_eq!(json["code"], "album_holds_no_tasks");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("holds no tasks"),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("eat.md")).expect("album unchanged"),
        before,
        "album note untouched"
    );
    assert!(!vault.join("eat").exists(), "no record folder written");
}
