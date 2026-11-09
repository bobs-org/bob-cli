//! `bob records` collection discovery, validation, schema display, and creation.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const RECORDS_CONFIG: &str = r##"
records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: "🍽️"
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          liked: "Liked"
      category:
        required: true
        suggest: true
      location:
        required: true
"##;

struct RecordsFixture {
    _temp: TempDir,
    vault: PathBuf,
    config: PathBuf,
}

fn fixture() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(vault.join("eat")).expect("create record folder");
    write_file(&config, RECORDS_CONFIG);
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

fn records_command(fixture: &RecordsFixture) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("records")
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_CONFIG_FILE", &fixture.config)
        .env("BOB_NOW", "2026-10-07 13:40:00")
        .env("NO_COLOR", "1");
    command
}

fn capture_command(fixture: &RecordsFixture) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_CONFIG_FILE", &fixture.config)
        .env("BOB_NOW", "2026-10-07 13:40:00")
        .env("NO_COLOR", "1");
    command
}

fn update_fixture() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records-update");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(vault.join("eat")).expect("create eat folder");
    fs::create_dir_all(vault.join("podcasts")).expect("create podcasts folder");
    write_file(
        &config,
        r#"records:
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
"#,
    );
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

fn write_ninja_update(vault: &std::path::Path) {
    write_file(
        &vault.join("eat/ninja_sushi.md"),
        concat!(
            "---\n",
            "title: Ninja Sushi\n",
            "type: \"[[restaurant]]\"\n",
            "status: to_try\n",
            "category: Sushi\n",
            "location: Clark\n",
            "---\n",
            "## Notes\n",
            "- 2025-01-02: First visit.\n",
            "\n",
        ),
    );
}

fn capture_json(output: &std::process::Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn json(output: &std::process::Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn write_valid_collection(fixture: &RecordsFixture) {
    write_file(
        &fixture.vault.join("restaurant.md"),
        "---\ntype: \"[[type]]\"\n---\n\n# Restaurant\n",
    );
    write_file(&fixture.vault.join("eat.md"), "# Restaurants\n");
    write_file(
        &fixture.vault.join("eat/ninja_sushi.md"),
        concat!(
            "---\n",
            "type: \"[[restaurant]]\"\n",
            "title: Ninja Sushi\n",
            "status: liked\n",
            "category: Sushi\n",
            "location: Clark\n",
            "---\n\n",
            "# Ninja Sushi\n",
        ),
    );
}

#[test]
fn list_show_and_check_report_a_valid_collection_as_json() {
    let fixture = fixture();
    write_valid_collection(&fixture);

    let list = records_command(&fixture)
        .args(["-f", "json"])
        .output()
        .expect("list records");
    assert_success(&list);
    let list_json = json(&list);
    assert_eq!(list_json["types"][0]["id"], "restaurant");
    assert_eq!(list_json["types"][0]["dir"], "eat");
    assert_eq!(list_json["types"][0]["count"], 1);
    assert_eq!(
        list_json["types"][0]["required_fields"],
        serde_json::json!(["status", "category", "location"])
    );

    let show = records_command(&fixture)
        .args(["show", "restaurant", "-f", "json"])
        .output()
        .expect("show record type");
    assert_success(&show);
    let show_json = json(&show);
    assert_eq!(show_json["count"], 1);
    assert_eq!(show_json["fields"][0]["choices"][0]["value"], "to_try");
    assert_eq!(show_json["fields"][1]["top_values"][0]["value"], "Sushi");

    let check = records_command(&fixture)
        .args(["check", "-f", "json"])
        .output()
        .expect("check records");
    assert_success(&check);
    let check_json = json(&check);
    assert_eq!(check_json["summary"]["errors"], 0);
    assert_eq!(check_json["types"][0]["count"], 1);
}

#[test]
fn new_dry_run_then_write_and_duplicate_refusal() {
    let fixture = fixture();
    let args = [
        "new",
        "restaurant",
        "Ramen Nagi",
        "-s",
        "status=to_try",
        "-s",
        "category=Ramen",
        "-s",
        "location=Clark",
        "-n",
        "Spicy broth",
    ];

    let dry_run = records_command(&fixture)
        .args(args)
        .args(["-d", "-f", "json"])
        .output()
        .expect("dry-run new record");
    assert_success(&dry_run);
    let dry_run_json = json(&dry_run);
    assert_eq!(dry_run_json["status"], "planned");
    assert_eq!(dry_run_json["path"], "eat/ramen_nagi.md");
    assert!(dry_run_json["markdown"]
        .as_str()
        .unwrap()
        .contains("Spicy broth"));
    assert!(!fixture.vault.join("eat/ramen_nagi.md").exists());

    let created = records_command(&fixture)
        .args(args)
        .args(["-f", "json"])
        .output()
        .expect("create new record");
    assert_success(&created);
    assert_eq!(json(&created)["status"], "written");
    let markdown = fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md"))
        .expect("read created record");
    assert!(markdown.contains("status: to_try"), "{markdown}");
    assert!(markdown.contains("category: \"Ramen\""), "{markdown}");
    assert!(markdown.contains("Spicy broth"), "{markdown}");

    let duplicate = records_command(&fixture)
        .args(args)
        .args(["-f", "json"])
        .output()
        .expect("refuse duplicate record");
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(json(&duplicate)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
}

#[test]
fn update_matches_capture_for_fields_notes_dates_and_lists() {
    let fixture = update_fixture();
    write_ninja_update(&fixture.vault);
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();

    let capture_dry = capture_command(&fixture)
        .args(["capture", "--dry-run", "-f", "json"])
        .arg("@eat/ninja_sushi\n- status:: liked\n- visited:: today")
        .output()
        .expect("capture dry-run update");
    assert_success(&capture_dry);
    let capture_json_value = capture_json(&capture_dry);
    let capture_markdown = capture_json_value["note_files"][0]["markdown"]
        .as_str()
        .expect("capture markdown")
        .to_owned();

    let update_dry = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=liked",
            "-s",
            "visited=today",
            "-d",
            "-f",
            "json",
        ])
        .output()
        .expect("records dry-run update");
    assert_success(&update_dry);
    let update_json_value = json(&update_dry);
    assert_eq!(update_json_value["status"], "planned");
    assert_eq!(
        update_json_value["markdown"].as_str().unwrap(),
        capture_markdown,
        "records dry-run matches capture"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "dry-run writes nothing"
    );

    let committed = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=liked",
            "-s",
            "visited=today",
            "-f",
            "json",
        ])
        .output()
        .expect("commit records update");
    assert_success(&committed);
    let committed_json = json(&committed);
    assert_eq!(committed_json["status"], "written");
    assert_eq!(committed_json["path"], "eat/ninja_sushi.md");
    assert_eq!(
        committed_json["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["status".to_string(), "visited".to_string()]
    );
    let status = committed_json["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "status")
        .expect("status property");
    assert_eq!(status["value"], "liked");
    assert_eq!(status["before"], "to_try");
    assert_eq!(status["before_display"], "To try");
    assert_eq!(status["display"], "Liked");
    let written =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert_eq!(
        written,
        update_json_value["markdown"].as_str().unwrap(),
        "commit matches dry-run"
    );
    assert_eq!(written, capture_markdown, "commit matches capture");
    assert!(written.contains("status: liked"), "{written}");
    assert!(written.contains("visited: 2026-10-07"), "{written}");

    let human = records_command(&fixture)
        .args(["update", "restaurant", "ninja_sushi", "-s", "status=liked"])
        .output()
        .expect("human no-op update");
    assert_success(&human);
    assert!(
        stdout(&human).contains("updated restaurant"),
        "{}",
        format_output(&human)
    );
}

#[test]
fn update_note_only_list_replace_empty_optional_and_warnings() {
    let fixture = update_fixture();
    write_ninja_update(&fixture.vault);
    write_file(
        &fixture.vault.join("eat/lists.md"),
        "---\ntitle: Lists\ntype: \"[[restaurant]]\"\nstatus: liked\ncategory: Ramen\nlocation: Clark\ntags:\n  - old\n  - stale\n---\n## Notes\n",
    );
    write_file(
        &fixture.vault.join("podcasts/talk_python.md"),
        "---\ntitle: Talk Python\ntype: \"[[podcast]]\"\nstatus: subscribed\ncategory: python\n---\n# Talk Python\n",
    );

    let note_only = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-n",
            "Went back, still great",
            "-f",
            "json",
        ])
        .output()
        .expect("note-only update");
    assert_success(&note_only);
    let note_json = json(&note_only);
    assert_eq!(note_json["notes"], 1);
    let markdown =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert!(
        markdown.contains("- 2026-10-07: Went back, still great"),
        "dated note: {markdown}"
    );

    let capture_lists = capture_command(&fixture)
        .args(["capture", "--dry-run", "-f", "json"])
        .arg("@eat/lists\n- tags:: spicy\n- tags:: lunch")
        .output()
        .expect("capture list replace");
    assert_success(&capture_lists);
    let capture_markdown = capture_json(&capture_lists)["note_files"][0]
        ["markdown"]
        .as_str()
        .expect("capture list markdown")
        .to_owned();

    let lists = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "lists",
            "-s",
            "tags=spicy",
            "-s",
            "tags=lunch",
            "-f",
            "json",
        ])
        .output()
        .expect("list replace update");
    assert_success(&lists);
    let lists_json = json(&lists);
    let tags = lists_json["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "tags")
        .expect("tags property");
    assert_eq!(tags["value"], serde_json::json!(["spicy", "lunch"]));
    assert_eq!(tags["before"], serde_json::json!(["old", "stale"]));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/lists.md")).unwrap(),
        capture_markdown,
        "list replace matches capture"
    );

    let cleared = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "lists",
            "-s",
            "location=",
            "-f",
            "json",
        ])
        .output()
        .expect("clear optional field");
    assert_success(&cleared);
    let cleared_markdown =
        fs::read_to_string(fixture.vault.join("eat/lists.md")).unwrap();
    assert!(
        !cleared_markdown.contains("location:"),
        "optional cleared: {cleared_markdown}"
    );

    let undated = records_command(&fixture)
        .args([
            "update",
            "podcast",
            "talk_python",
            "-n",
            "Listened to the async episode",
            "-f",
            "json",
        ])
        .output()
        .expect("undated podcast note");
    assert_success(&undated);
    let podcast_markdown =
        fs::read_to_string(fixture.vault.join("podcasts/talk_python.md"))
            .unwrap();
    assert!(
        podcast_markdown.contains("- Listened to the async episode"),
        "{podcast_markdown}"
    );
    assert!(
        !podcast_markdown.contains("2026-10-07: Listened"),
        "podcast notes stay undated: {podcast_markdown}"
    );
    assert!(
        podcast_markdown.contains("## Notes"),
        "missing Notes created: {podcast_markdown}"
    );

    write_file(
        &fixture.vault.join("eat/legacy.md"),
        "---\ntitle: Legacy\ntype: \"[[restaurant]]\"\nstatus: liked\n---\n## Notes\n",
    );
    let warned = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "legacy",
            "-n",
            "Still good",
            "-f",
            "json",
        ])
        .output()
        .expect("historical warning update");
    assert_success(&warned);
    let warned_json = json(&warned);
    assert!(
        warned_json["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["field"] == "category"),
        "historical defect surfaces: {warned_json}"
    );
    assert!(
        stderr(&warned).contains("bob records check"),
        "warning forwarded: {}",
        format_output(&warned)
    );
    assert!(
        fs::read_to_string(fixture.vault.join("eat/legacy.md"))
            .unwrap()
            .contains("- 2026-10-07: Still good"),
        "warning does not block the update"
    );
}

#[test]
fn update_missing_invalid_unknown_noop_and_dry_run() {
    let fixture = update_fixture();
    write_ninja_update(&fixture.vault);

    let unknown_type = records_command(&fixture)
        .args(["update", "nope", "ninja_sushi", "-f", "json"])
        .output()
        .expect("unknown type");
    assert_eq!(unknown_type.status.code(), Some(2));
    assert!(json(&unknown_type)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown record type"));

    let unknown_stem = records_command(&fixture)
        .args(["update", "restaurant", "missing", "-f", "json"])
        .output()
        .expect("unknown stem");
    assert_eq!(unknown_stem.status.code(), Some(2));
    assert!(json(&unknown_stem)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not exist"));

    let invalid = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=bogus",
            "-f",
            "json",
        ])
        .output()
        .expect("invalid value");
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(json(&invalid)["status"], "incomplete");

    let empty_required = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=",
            "-f",
            "json",
        ])
        .output()
        .expect("empty required");
    assert_eq!(empty_required.status.code(), Some(2));
    assert!(json(&empty_required)["missing"]
        .as_array()
        .unwrap()
        .iter()
        .any(|key| key == "status"));

    let bad_set = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "novalue",
            "-f",
            "json",
        ])
        .output()
        .expect("malformed set");
    assert_eq!(bad_set.status.code(), Some(2));

    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let before_meta = fs::metadata(fixture.vault.join("eat/ninja_sushi.md"))
        .expect("stat before no-op");
    let noop = records_command(&fixture)
        .args(["update", "restaurant", "ninja_sushi", "-f", "json"])
        .output()
        .expect("no-op update");
    assert_success(&noop);
    assert_eq!(json(&noop)["status"], "written");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "no-op preserves bytes"
    );
    assert_eq!(
        fs::metadata(fixture.vault.join("eat/ninja_sushi.md"))
            .expect("stat after no-op")
            .modified()
            .unwrap(),
        before_meta.modified().unwrap(),
        "no-op does not rewrite the note"
    );

    let dry = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=liked",
            "-d",
            "-f",
            "json",
        ])
        .output()
        .expect("dry-run update");
    assert_success(&dry);
    assert_eq!(json(&dry)["status"], "planned");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "dry-run writes nothing"
    );
}

#[test]
fn update_preserves_foreign_bytes_and_catalog_covers_update() {
    let fixture = update_fixture();
    let original = include_str!("../fixtures/records/source/eat/ahrres.md");
    write_file(&fixture.vault.join("eat/ahrres.md"), original);

    let foreign =
        format!("{original}\n- 2026-10-06: Foreign concurrent note.\n");
    write_file(&fixture.vault.join("eat/ahrres.md"), &foreign);

    let output = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ahrres",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ])
        .output()
        .expect("update preserves foreign bytes");
    assert_success(&output);
    let written =
        fs::read_to_string(fixture.vault.join("eat/ahrres.md")).unwrap();
    assert!(
        written.contains("Foreign concurrent note."),
        "foreign bytes survive: {written}"
    );
    assert!(written.contains("location: \"Clark\""), "{written}");

    let catalog = bob_command()
        .arg("__catalog")
        .arg("--protocol")
        .arg("1")
        .output()
        .expect("run __catalog");
    assert_success(&catalog);
    let catalog_json: Value =
        serde_json::from_str(&stdout(&catalog)).expect("parse catalog");
    let update = catalog_json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "records-update")
        .expect("records-update catalog entry");
    assert_eq!(update["path"], serde_json::json!(["records", "update"]));
    assert_eq!(update["preview"], serde_json::json!(["--dry-run"]));
    let slots: std::collections::BTreeSet<String> = update["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| {
            format!(
                "{}:{}",
                arg["name"].as_str().unwrap_or_default(),
                arg["slot"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert!(slots.contains("TYPE:record-type"), "TYPE slot: {slots:?}");
    assert!(slots.contains("STEM:record-stem"), "STEM slot: {slots:?}");
    assert!(slots.contains("SET:record-set"), "SET slot: {slots:?}");
}
