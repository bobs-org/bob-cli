//! `bob records` collection discovery, validation, schema display, and creation.

use crate::support::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "summary: Places to eat.\n",
            "emoji: \"🍽️\"\n",
            "record:\n",
            "  noun: restaurant\n",
            "  label: Restaurant\n",
            "  fields:\n",
            "    status:\n",
            "      required: true\n",
            "      values: {to_try: \"To try\", liked: \"Liked\"}\n",
            "    category: {required: true, suggest: true}\n",
            "    location: {required: true}\n",
            "---\n\n# Restaurants\n"
        ),
    );
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
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\nparent: \"[[body]]\"\nsummary: Places.\n",
            "record:\n  noun: restaurant\n  label: Restaurant\n  dated_notes: true\n",
            "  fields:\n",
            "    status: {required: true, values: {to_try: \"To try\", liked: \"Liked\"}}\n",
            "    category: {required: true}\n    location: {}\n",
            "    visited: {values: date}\n    tags: {list: true}\n",
            "    rating: {values: number}\n",
            "---\n\n# Restaurants\n"
        ),
    );
    write_file(
        &vault.join("podcasts.md"),
        concat!(
            "---\n",
            "type: [[album]]\nparent: \"[[body]]\"\nsummary: Podcasts.\n",
            "record:\n  noun: podcast\n  label: Podcast\n",
            "  fields:\n",
            "    status: {required: true, values: {to_try: \"To try\", subscribed: \"Subscribed\"}}\n",
            "    category: {required: true}\n",
            "---\n\n# Podcasts\n"
        ),
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
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
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
        &fixture.vault.join("eat/ninja_sushi.md"),
        concat!(
            "---\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
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
    assert_eq!(show_json["fields"][0]["choices"][0]["count"], 0);
    assert_eq!(show_json["fields"][0]["choices"][1]["count"], 1);
    assert_eq!(show_json["fields"][0]["filled"], 1);
    assert_eq!(show_json["fields"][0]["suggest"], false);
    assert!(show_json["fields"][0]["line"].is_u64());
    assert!(show_json["fields"][0].get("default").is_none());
    assert!(show_json["fields"][0].get("min").is_none());
    assert!(show_json["fields"][0].get("max").is_none());
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
fn new_writes_no_h1_and_update_inserts_notes_after_walk_line() {
    let fixture = fixture();
    let created = records_command(&fixture)
        .args([
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
            "-f",
            "json",
        ])
        .output()
        .expect("create new record");
    assert_success(&created);
    let markdown = fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md"))
        .expect("read created record");
    assert!(!markdown.contains("# Ramen Nagi\n"), "{markdown}");
    assert!(markdown.contains("## Notes"), "{markdown}");
    assert!(markdown.contains("Spicy broth"), "{markdown}");

    // A note with no ordinary bullets has only its managed walk line.
    let bare = records_command(&fixture)
        .args([
            "new",
            "restaurant",
            "Udon House",
            "-s",
            "status=to_try",
            "-s",
            "category=Udon",
            "-s",
            "location=Main",
            "-f",
            "json",
        ])
        .output()
        .expect("create bare record");
    assert_success(&bare);
    let bare_markdown =
        fs::read_to_string(fixture.vault.join("eat/udon_house.md"))
            .expect("read bare record");
    let walk_line = "[[eat/ramen_nagi|‹ Ramen Nagi]] · [[eat|↑ Restaurants]] · [[eat/ramen_nagi|Ramen Nagi ›]]";
    assert!(
        bare_markdown.ends_with(&format!("---\n\n{walk_line}\n")),
        "{bare_markdown}"
    );
    assert!(!bare_markdown.contains('#'), "{bare_markdown}");

    // Updating the bare note inserts a canonical Notes section.
    let updated = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "udon_house",
            "-n",
            "First visit",
            "-f",
            "json",
        ])
        .output()
        .expect("update bare record");
    assert_success(&updated);
    let updated_markdown =
        fs::read_to_string(fixture.vault.join("eat/udon_house.md"))
            .expect("read updated record");
    assert!(
        updated_markdown.contains(&format!(
            "---\n\n{walk_line}\n\n## Notes\n\n- First visit\n"
        )),
        "{updated_markdown}"
    );
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
        "---\nparent: \"[[eat]]\"\ntitle: Lists\ntype: \"[[record]]\"\nstatus: liked\ncategory: Ramen\nlocation: Clark\ntags:\n  - old\n  - stale\n---\n## Notes\n",
    );
    write_file(
        &fixture.vault.join("podcasts/talk_python.md"),
        "---\nparent: \"[[podcasts]]\"\ntitle: Talk Python\ntype: \"[[record]]\"\nstatus: subscribed\ncategory: python\n---\n# Talk Python\n",
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
        "---\nparent: \"[[eat]]\"\ntitle: Legacy\ntype: \"[[restaurant]]\"\nstatus: liked\n---\n## Notes\n",
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

#[test]
fn records_init_always_refuses_and_teaches_album_create() {
    let fixture = fixture();
    let output = records_command(&fixture)
        .args(["init", "restaurant", "-p", "body"])
        .output()
        .expect("run retired records init");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let text = format_output(&output);
    assert!(text.contains("bob albums create"), "{text}");
    assert!(text.contains("album notes"), "{text}");
    assert!(!fixture.vault.join("restaurant.md").exists());
    assert!(!fixture.vault.join("restaurants.base").exists());
}

fn album_fixture() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records-album");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(&config, "other: true\n");
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "summary: Places to eat.\n",
            "emoji: \"🍽️\"\n",
            "record:\n",
            "  noun: restaurant\n",
            "  label: Restaurant\n",
            "  dated_notes: true\n",
            "  fields:\n",
            "    status:\n",
            "      required: true\n",
            "      values:\n",
            "        to_try: \"To try\"\n",
            "        liked: \"Liked\"\n",
            "    category:\n",
            "      required: true\n",
            "      suggest: true\n",
            "    location:\n",
            "      required: true\n",
            "---\n\n",
            "# Restaurants\n",
        ),
    );
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

fn album_record(
    parent: &str,
    type_link: &str,
    title: &str,
    extra_body: &str,
) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntype: \"[[{type_link}]]\"\ntitle: {title:?}\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n\n# {title}\n{extra_body}"
    )
}

fn findings(payload: &Value) -> Vec<&Value> {
    payload["types"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|report| report["findings"].as_array())
        .flatten()
        .collect()
}

#[test]
fn album_noun_and_stem_aliases_share_source_metadata() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );

    let by_noun = records_command(&fixture)
        .args(["show", "restaurant", "-f", "json"])
        .output()
        .expect("show by noun");
    assert_success(&by_noun);
    let noun_json = json(&by_noun);
    assert_eq!(noun_json["type"], "restaurant");
    assert_eq!(noun_json["source"], "album");
    assert_eq!(noun_json["album"]["stem"], "eat");
    assert_eq!(noun_json["album"]["title"], "Restaurants");
    assert_eq!(noun_json["album"]["owner"], "body");
    assert_eq!(noun_json["count"], 1);

    let by_stem = records_command(&fixture)
        .args(["show", "eat", "-f", "json"])
        .output()
        .expect("show by stem");
    assert_success(&by_stem);
    assert_eq!(json(&by_stem)["album"]["stem"], "eat");

    let list = records_command(&fixture)
        .args(["-f", "json"])
        .output()
        .expect("list albums");
    assert_success(&list);
    let list_json = json(&list);
    assert_eq!(list_json["types"][0]["source"], "album");
    assert_eq!(list_json["types"][0]["count"], 1);
    assert_eq!(
        list_json["types"][0]["required_fields"],
        serde_json::json!(["status", "category", "location"])
    );
}

#[test]
fn bob_dir_flag_beats_env_for_album_schemas() {
    let first = album_fixture();
    let second = album_fixture();
    write_file(
        &second.vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "summary: Other vault.\n",
            "record:\n",
            "  noun: restaurant\n",
            "  label: Other\n",
            "  fields:\n",
            "    cuisine: {}\n",
            "---\n\n",
            "# Other Restaurants\n",
        ),
    );

    let output = bob_command()
        .args(["records", "show", "eat", "-b"])
        .arg(&second.vault)
        .args(["-f", "json"])
        .env("BOB_DIR", &first.vault)
        .env("BOB_CONFIG_FILE", &first.config)
        .env("NO_COLOR", "1")
        .output()
        .expect("show with -b");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["label"], "Other");
    assert_eq!(payload["album"]["title"], "Other Restaurants");
    assert_eq!(payload["fields"][0]["key"], "cuisine");
}

#[test]
fn album_claim_suppresses_config_even_when_the_album_is_broken() {
    let fixture = album_fixture();
    write_file(
        &fixture.config,
        "records:\n  restaurant:\n    dir: eat\n    label: Config Restaurant\n    fields: {}\n",
    );
    write_file(
        &fixture.vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "record: {noun: restaurant, fields: []}\n",
            "---\n\n",
            "# Broken Eat\n",
        ),
    );
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );

    let list = records_command(&fixture)
        .args(["-f", "json"])
        .output()
        .expect("list mixed");
    assert_success(&list);
    let list_json = json(&list);
    assert_eq!(list_json["types"].as_array().unwrap().len(), 1);
    assert_eq!(list_json["types"][0]["source"], "album");
    assert_eq!(list_json["types"][0]["count"], 1);
    assert_eq!(list_json["types"][0]["album"]["title"], "Broken Eat");

    let show = records_command(&fixture)
        .args(["show", "restaurant", "-f", "json"])
        .output()
        .expect("show broken album");
    assert_eq!(show.status.code(), Some(2));
    assert!(
        json(&show)["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not usable"),
        "{}",
        format_output(&show)
    );
}

#[test]
fn healthy_album_survives_malformed_config_and_a_broken_neighbor() {
    let fixture = album_fixture();
    write_file(&fixture.config, "records: [\n");
    write_file(
        &fixture.vault.join("tv.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "record: {noun: show, fields: []}\n",
            "---\n\n",
            "# Broken Shows\n",
        ),
    );
    fs::create_dir_all(fixture.vault.join("tv")).expect("tv folder");
    write_file(
        &fixture.vault.join("tv/lost.md"),
        "---\nparent: \"[[tv]]\"\ntype: \"[[record]]\"\ntitle: Lost\n---\n# Lost\n",
    );
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );

    let list = records_command(&fixture)
        .args(["-f", "json"])
        .output()
        .expect("list with neighbor");
    assert_success(&list);
    let types = json(&list)["types"].as_array().cloned().unwrap();
    let eat = types.iter().find(|row| row["dir"] == "eat").expect("eat");
    assert_eq!(eat["source"], "album");
    assert_eq!(eat["count"], 1);
    let tv = types.iter().find(|row| row["dir"] == "tv").expect("tv");
    assert_eq!(tv["count"], 1);

    let created = records_command(&fixture)
        .args([
            "new",
            "eat",
            "Ramen Nagi",
            "-s",
            "status=to_try",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ])
        .output()
        .expect("create in healthy album");
    assert_success(&created);
    let markdown = fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md"))
        .expect("created album record");
    assert!(markdown.contains("type: \"[[record]]\""), "{markdown}");
    assert!(markdown.contains("parent: \"[[eat]]\""), "{markdown}");
}

#[test]
fn album_check_accepts_canonical_type_and_warns_on_legacy() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );
    write_file(
        &fixture.vault.join("eat/legacy.md"),
        &album_record("eat", "restaurant", "Legacy", ""),
    );

    let check = records_command(&fixture)
        .args(["check", "-f", "json"])
        .output()
        .expect("check types");
    assert_success(&check);
    let payload = json(&check);
    assert_eq!(payload["summary"]["errors"], 0);
    let codes: Vec<_> = findings(&payload)
        .iter()
        .map(|finding| finding["code"].as_str().unwrap_or_default())
        .collect();
    assert!(codes.contains(&"record_type_legacy"), "{codes:?}");
}

#[test]
fn album_check_reports_parent_checkbox_orphan_and_missing_stub_is_ok() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat/wrong.md"),
        &album_record(
            "body",
            "record",
            "Wrong Parent",
            "\n- [ ] book a table\n\n```\n- [ ] fenced example\n```\n",
        ),
    );
    write_file(
        &fixture.vault.join("stray.md"),
        "---\ntype: \"[[record]]\"\ntitle: Stray\n---\n# Stray\n",
    );
    assert!(!fixture.vault.join("restaurant.md").exists());

    let check = records_command(&fixture)
        .args(["check", "-f", "json"])
        .output()
        .expect("check membership");
    assert_eq!(check.status.code(), Some(1));
    let payload = json(&check);
    let codes: Vec<_> = findings(&payload)
        .iter()
        .map(|finding| finding["code"].as_str().unwrap_or_default())
        .collect();
    assert!(codes.contains(&"record_parent_mismatch"), "{codes:?}");
    assert!(codes.contains(&"record_checkbox"), "{codes:?}");
    assert!(codes.contains(&"record_outside_album"), "{codes:?}");
    assert!(
        !codes.contains(&"record_missing_collection_note"),
        "album kinds do not require noun stubs: {codes:?}"
    );
    assert_eq!(
        findings(&payload)
            .iter()
            .filter(|finding| finding["code"] == "record_checkbox")
            .count(),
        1,
        "fenced checkbox is ignored: {payload}"
    );
}

#[test]
fn malformed_album_schema_still_counts_folder_records() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "record: {noun: restaurant, fields: []}\n",
            "---\n\n",
            "# Broken Eat\n",
        ),
    );
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );

    let list = records_command(&fixture)
        .args(["-f", "json"])
        .output()
        .expect("list broken counts");
    assert_success(&list);
    assert_eq!(json(&list)["types"][0]["count"], 1);

    let check = records_command(&fixture)
        .args(["check", "eat", "-f", "json"])
        .output()
        .expect("check broken counts");
    let payload = json(&check);
    assert_eq!(payload["types"][0]["count"], 1);
    assert!(
        payload["types"][0]["errors"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
}

#[test]
fn album_create_is_byte_exact_and_dry_run_matches_write() {
    let fixture = album_fixture();
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
    ];

    let dry = records_command(&fixture)
        .args(args)
        .args(["-d", "-f", "json"])
        .output()
        .expect("album dry-run");
    assert_success(&dry);
    let dry_json = json(&dry);
    assert_eq!(dry_json["status"], "planned");
    assert_eq!(dry_json["source"], "album");
    let markdown = dry_json["markdown"].as_str().unwrap();
    assert!(markdown.contains("type: \"[[record]]\""), "{markdown}");
    assert!(markdown.contains("parent: \"[[eat]]\""), "{markdown}");
    assert!(!fixture.vault.join("eat/ramen_nagi.md").exists());

    let written = records_command(&fixture)
        .args(args)
        .args(["-f", "json"])
        .output()
        .expect("album write");
    assert_success(&written);
    let written_json = json(&written);
    assert_eq!(written_json["status"], "written");
    assert_eq!(written_json["source"], "album");
    assert_eq!(written_json["album"]["stem"], "eat");
    let on_disk = fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md"))
        .expect("read album record");
    assert_eq!(on_disk, markdown);
}

#[test]
fn album_update_preserves_unrelated_bytes_and_matches_dry_run() {
    let fixture = album_fixture();
    let original = concat!(
        "---\n",
        "parent: \"[[eat]]\"\n",
        "type: \"[[record]]\"\n",
        "title: Ninja Sushi\n",
        "status: to_try\n",
        "category: Sushi\n",
        "location: Clark\n",
        "---\n",
        "## Notes\n",
        "- 2025-01-02: First visit.\n",
        "\n",
        "Keep this trailer.\n",
    );
    write_file(&fixture.vault.join("eat/ninja_sushi.md"), original);

    let dry = records_command(&fixture)
        .args([
            "update",
            "eat",
            "ninja_sushi",
            "-s",
            "status=liked",
            "-d",
            "-f",
            "json",
        ])
        .output()
        .expect("album update dry-run");
    assert_success(&dry);
    let dry_json = json(&dry);
    assert_eq!(dry_json["status"], "planned");
    assert_eq!(dry_json["source"], "album");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        original,
        "dry-run writes nothing"
    );

    let committed = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=liked",
            "-f",
            "json",
        ])
        .output()
        .expect("album update write");
    assert_success(&committed);
    let written =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert_eq!(written, dry_json["markdown"].as_str().unwrap());
    assert!(written.contains("Keep this trailer."), "{written}");
    assert!(written.contains("status: liked"), "{written}");
}

#[test]
fn records_outside_a_broken_album_are_members_not_orphans() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "record: {noun: restaurant, fields: []}\n",
            "---\n\n",
            "# Broken Eat\n",
        ),
    );
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );
    write_file(
        &fixture.vault.join("orphan.md"),
        "---\ntype: \"[[record]]\"\ntitle: Orphan\n---\n# Orphan\n",
    );

    let check = records_command(&fixture)
        .args(["check", "-f", "json"])
        .output()
        .expect("check broken members");
    let payload = json(&check);
    let orphan_files: Vec<_> = findings(&payload)
        .iter()
        .filter(|finding| finding["code"] == "record_outside_album")
        .map(|finding| finding["file"].as_str().unwrap_or_default())
        .collect();
    assert!(orphan_files.contains(&"orphan.md"), "{payload}");
    assert!(
        !orphan_files.contains(&"eat/nagi.md"),
        "broken album members are not orphans: {payload}"
    );
}

// record-writes: every album-backed record write keeps the index current.

fn record_writes_new(
    fixture: &RecordsFixture,
    args: &[&str],
) -> std::process::Output {
    records_command(fixture)
        .args(args)
        .output()
        .expect("records new")
}

#[test]
fn record_writes_new_stages_record_and_index_together() {
    let fixture = album_fixture();
    let body_before =
        fs::read_to_string(fixture.vault.join("body.md")).unwrap();

    let created = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=to_try",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&created);
    let payload = json(&created);
    assert_eq!(payload["status"], "written");
    assert_eq!(payload["source"], "album");
    let album_index = &payload["album_index"];
    assert_eq!(album_index["path"], "eat.md");
    assert_eq!(album_index["state"], "missing");
    assert!(
        album_index["after"]
            .as_str()
            .unwrap()
            .contains("Ramen Nagi"),
        "{payload}"
    );

    let record =
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap();
    assert!(record.contains("title: \"Ramen Nagi\""), "{record}");
    let album = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    assert!(album.contains("Ramen Nagi"), "{album}");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("body.md")).unwrap(),
        body_before,
        "owner note is byte-identical"
    );

    // A stem selector with an explicit `-b` vault routes to the same
    // album and index.
    let vault = fixture.vault.to_str().expect("vault path").to_owned();
    let second = record_writes_new(
        &fixture,
        &[
            "new",
            "eat",
            "Udon",
            "-s",
            "status=liked",
            "-s",
            "category=Udon",
            "-s",
            "location=Main",
            "-b",
            &vault,
            "-f",
            "json",
        ],
    );
    assert_success(&second);
    let second_json = json(&second);
    assert_eq!(second_json["album_index"]["path"], "eat.md");
    let album = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    assert!(album.contains("Ramen Nagi"), "{album}");
    assert!(album.contains("Udon"), "{album}");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("body.md")).unwrap(),
        body_before,
        "owner note stays byte-identical"
    );
}

#[test]
fn record_writes_create_the_ring_and_report_the_existing_neighbour() {
    let fixture = album_fixture();
    let first = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&first);
    let first_json = json(&first);
    let first_markdown = first_json["markdown"].as_str().unwrap();
    assert!(
        first_markdown.contains("[[eat|↑ Restaurants]]"),
        "{first_json}"
    );
    assert!(first_json["walk_files"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap(),
        first_markdown
    );

    let before = first_markdown.to_owned();
    let second = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Udon",
            "-s",
            "status=liked",
            "-s",
            "category=Udon",
            "-s",
            "location=Main",
            "-f",
            "json",
        ],
    );
    assert_success(&second);
    let second_json = json(&second);
    let second_markdown = second_json["markdown"].as_str().unwrap();
    assert!(
        second_markdown.contains(
            "[[eat/ramen_nagi|‹ Ramen Nagi]] · [[eat|↑ Restaurants]] · [[eat/ramen_nagi|Ramen Nagi ›]]"
        ),
        "{second_json}"
    );
    let walk_files = second_json["walk_files"].as_array().unwrap();
    assert_eq!(walk_files.len(), 1, "{second_json}");
    assert_eq!(walk_files[0]["path"], "eat/ramen_nagi.md");
    assert_eq!(walk_files[0]["before"], before);
    let neighbor_after = walk_files[0]["after"].as_str().unwrap();
    assert!(neighbor_after.contains(
        "[[eat/udon|‹ Udon]] · [[eat|↑ Restaurants]] · [[eat/udon|Udon ›]]"
    ));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap(),
        neighbor_after
    );
}

#[test]
fn records_new_and_capture_create_identical_record_bytes() {
    let records = album_fixture();
    let records_output = record_writes_new(
        &records,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-d",
            "-f",
            "json",
        ],
    );
    assert_success(&records_output);
    let records_markdown = json(&records_output)["markdown"]
        .as_str()
        .unwrap()
        .to_owned();

    let capture = album_fixture();
    let capture_output = capture_command(&capture)
        .args([
            "capture",
            "--dry-run",
            "-f",
            "json",
            "--",
            "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
        ])
        .output()
        .expect("capture dry-run");
    assert_success(&capture_output);
    let capture_markdown = json(&capture_output)["note_files"][0]["markdown"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(records_markdown, capture_markdown);
}

#[test]
fn record_writes_touch_only_two_neighbours_in_a_large_unbackfilled_album() {
    let fixture = album_fixture();
    let mut before = BTreeMap::new();
    for index in 0..104 {
        let title = format!("Record {index:03}");
        let path = fixture.vault.join(format!("eat/record_{index:03}.md"));
        let markdown = album_record("eat", "record", &title, "");
        write_file(&path, &markdown);
        before.insert(path, markdown);
    }

    let output = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Record 050.5",
            "-s",
            "status=liked",
            "-s",
            "category=New",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    let walk_files = payload["walk_files"].as_array().unwrap();
    assert_eq!(walk_files.len(), 2, "{payload}");
    assert_eq!(walk_files[0]["path"], "eat/record_050.md");
    assert_eq!(walk_files[1]["path"], "eat/record_051.md");

    let mut changed = Vec::new();
    for (path, original) in before {
        let current = fs::read_to_string(&path).unwrap();
        if current != original {
            changed
                .push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    changed.sort();
    assert_eq!(changed, ["record_050.md", "record_051.md"]);
    let created = fixture.vault.join(payload["path"].as_str().unwrap());
    assert!(created.exists());
}

#[test]
fn record_writes_update_regroups_the_index() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat/ninja_sushi.md"),
        &album_record("eat", "record", "Ninja Sushi", ""),
    );

    let updated = record_writes_new(
        &fixture,
        &[
            "update",
            "restaurant",
            "ninja_sushi",
            "-s",
            "status=to_try",
            "-f",
            "json",
        ],
    );
    assert_success(&updated);
    let payload = json(&updated);
    assert!(
        payload["album_index"]["after"]
            .as_str()
            .unwrap()
            .contains("To try"),
        "{payload}"
    );
    let album = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    let liked = album.find("Liked").expect("liked group");
    let row = album.find("Ninja Sushi").expect("row");
    assert!(liked < row, "row regrouped under Liked: {album}");
}

#[test]
fn record_writes_notes_only_update_keeps_current_album_identical() {
    let fixture = album_fixture();
    assert_success(&record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
        ],
    ));
    let record_before =
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap();
    assert!(record_before.contains("[[eat|↑ Restaurants]]"));
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();

    let updated = record_writes_new(
        &fixture,
        &[
            "update",
            "eat",
            "ramen_nagi",
            "-n",
            "Went back, still great",
            "-f",
            "json",
        ],
    );
    assert_success(&updated);
    let payload = json(&updated);
    assert!(
        payload.get("album_index").is_none(),
        "current index contributes no write: {payload}"
    );
    assert!(payload.get("walk_files").is_none());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "notes-only update leaves a current album unchanged"
    );
    let record =
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap();
    assert!(record.contains("Went back, still great"), "{record}");
    assert!(record.contains("[[eat|↑ Restaurants]]"), "{record}");
}

#[test]
fn record_writes_noop_update_repairs_a_stale_index() {
    let fixture = album_fixture();
    assert_success(&record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
        ],
    ));
    let record_before =
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap();
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    let stale = album_before.replace("Ramen Nagi", "WRONG");
    assert_ne!(stale, album_before, "test edits the index");
    write_file(&fixture.vault.join("eat.md"), &stale);

    // Repeating the stored values is a record no-op that still repairs.
    let repaired = record_writes_new(
        &fixture,
        &[
            "update",
            "restaurant",
            "ramen_nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&repaired);
    let payload = json(&repaired);
    assert_eq!(payload["album_index"]["state"], "stale");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap(),
        record_before,
        "no-op update rewrites no record bytes"
    );
    let album = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    assert!(album.contains("Ramen Nagi"), "{album}");
    assert!(!album.contains("WRONG"), "{album}");
}

#[test]
fn record_writes_dry_run_matches_commit_without_touching_disk() {
    let fixture = album_fixture();
    let dry = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-d",
            "-f",
            "json",
        ],
    );
    assert_success(&dry);
    let dry_json = json(&dry);
    assert_eq!(dry_json["status"], "planned");
    let planned_album = dry_json["album_index"]["after"]
        .as_str()
        .expect("planned album bytes")
        .to_owned();
    assert!(
        !fixture.vault.join("eat/ramen_nagi.md").exists(),
        "dry-run writes no record"
    );

    let committed = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&committed);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap(),
        dry_json["markdown"].as_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        planned_album,
        "dry-run and commit share planned album bytes"
    );

    let human = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Second",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-d",
        ],
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("eat.md"), "{text}");
    assert!(text.contains("Second"), "{text}");
    assert!(
        !fixture.vault.join("eat/second.md").exists(),
        "human dry-run writes nothing"
    );
}

#[test]
fn record_writes_crlf_album_stays_crlf_and_matches_dry_run() {
    let fixture = album_fixture();
    let album = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    write_file(&fixture.vault.join("eat.md"), &album.replace('\n', "\r\n"));

    let dry = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-d",
            "-f",
            "json",
        ],
    );
    assert_success(&dry);
    let planned = json(&dry)["album_index"]["after"]
        .as_str()
        .expect("planned album bytes")
        .to_owned();
    assert!(planned.contains("\r\n"), "CRLF preserved");

    let committed = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&committed);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        planned
    );
}

#[test]
fn record_writes_incomplete_new_writes_neither_file() {
    let fixture = album_fixture();
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();

    let incomplete = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
        ],
    );
    assert!(
        !incomplete.status.success(),
        "missing required field refuses"
    );
    assert!(
        !fixture.vault.join("eat/ramen_nagi.md").exists(),
        "no record file"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "incomplete input stages no album edit"
    );
}

#[test]
fn record_writes_new_ignores_legacy_config_and_rebuilds_album_index() {
    let fixture = fixture();
    write_valid_collection(&fixture);

    let created = records_command(&fixture)
        .args([
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ])
        .output()
        .expect("album new with legacy config");
    assert_success(&created);
    let payload = json(&created);
    assert_eq!(payload["source"], "album");
    assert_eq!(payload["album_index"]["state"], "missing", "{payload}");
    let album = fs::read_to_string(fixture.vault.join("eat.md"))
        .expect("album index is current");
    assert!(album.contains("[[eat/ramen_nagi|Ramen Nagi]]"), "{album}");
}

#[test]
fn record_writes_damaged_folder_refuses_new_and_update_including_dry_run() {
    let fixture = album_fixture();
    write_file(
        &fixture.vault.join("eat/nagi.md"),
        &album_record("eat", "record", "Nagi", ""),
    );
    fs::create_dir_all(fixture.vault.join("eat/blocked.md"))
        .expect("directory occupant");
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();

    for extra in [&["-f", "json"][..], &["-d", "-f", "json"][..]] {
        let mut args = vec![
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
        ];
        args.extend(extra.iter().copied());
        let refused = record_writes_new(&fixture, &args);
        assert!(
            !refused.status.success(),
            "damaged folder refuses new {extra:?}: {}",
            format_output(&refused)
        );
        assert!(
            format_output(&refused).contains("eat"),
            "reports the affected path: {}",
            format_output(&refused)
        );
    }
    let update = record_writes_new(
        &fixture,
        &[
            "update",
            "restaurant",
            "nagi",
            "-s",
            "status=to_try",
            "-f",
            "json",
        ],
    );
    assert!(!update.status.success(), "damaged folder refuses update");
    assert!(
        !fixture.vault.join("eat/ramen_nagi.md").exists(),
        "writes neither record nor index"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "album unchanged"
    );
}

#[test]
fn record_writes_unreadable_member_refuses_but_malformed_allows_write() {
    let fixture = album_fixture();
    fs::write(
        fixture.vault.join("eat/unreadable.md"),
        b"---\ntitle: \xff\n---\n".as_slice(),
    )
    .expect("unreadable member");
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    let refused = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert!(!refused.status.success(), "unreadable member refuses");
    assert!(
        !fixture.vault.join("eat/ramen_nagi.md").exists(),
        "no record file"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "no index published from incomplete inputs"
    );
    fs::remove_file(fixture.vault.join("eat/unreadable.md")).expect("cleanup");
    write_file(
        &fixture.vault.join("eat/malformed.md"),
        "---\n: bad: [unclosed\n---\n\n# Malformed\n",
    );
    let allowed = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-f",
            "json",
        ],
    );
    assert_success(&allowed);
}

#[test]
fn record_writes_human_dry_run_shows_unified_album_diff() {
    let fixture = album_fixture();
    let dry = record_writes_new(
        &fixture,
        &[
            "new",
            "restaurant",
            "Ramen Nagi",
            "-s",
            "status=liked",
            "-s",
            "category=Ramen",
            "-s",
            "location=Clark",
            "-d",
        ],
    );
    assert_success(&dry);
    let out = format_output(&dry);
    assert!(out.contains("eat.md"), "file header: {out}");
    assert!(
        out.contains("--- a/eat.md") || out.contains("a/eat.md"),
        "unified diff header: {out}"
    );
    assert!(
        out.contains('+') && out.contains("Ramen Nagi"),
        "changed lines: {out}"
    );
}

// record-read: bob records show TYPE STEM

fn show_fixture() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records-show");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(&config, "other: true\n");
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(&vault.join("eat.md"), SHOW_ALBUM);
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

const SHOW_ALBUM: &str = concat!(
    "---\n",
    "type: [[album]]\n",
    "parent: \"[[body]]\"\n",
    "summary: Places to eat.\n",
    "emoji: \"🍽️\"\n",
    "record:\n",
    "  noun: restaurant\n",
    "  label: Restaurant\n",
    "  dated_notes: true\n",
    "  legacy: [lid, source_*]\n",
    "  fields:\n",
    "    status:\n",
    "      required: true\n",
    "      values:\n",
    "        to_try: \"To try\"\n",
    "        liked: \"Liked\"\n",
    "    category:\n",
    "      required: true\n",
    "      suggest: true\n",
    "    location: {}\n",
    "    tags: {list: true}\n",
    "    rating: {values: number}\n",
    "---\n\n",
    "# Restaurants\n",
);

fn show_record(
    fixture: &RecordsFixture,
    args: &[&str],
) -> std::process::Output {
    records_command(fixture)
        .args(args)
        .output()
        .expect("bob records show")
}

fn show_json(output: &std::process::Output) -> Value {
    json(output)
}

fn property<'a>(payload: &'a Value, key: &str) -> &'a Value {
    payload["record"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == key)
        .unwrap_or_else(|| panic!("missing property {key}: {payload}"))
}

fn identity<'a>(payload: &'a Value, key: &str) -> &'a Value {
    payload["record"]["identity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == key)
        .unwrap_or_else(|| panic!("missing identity {key}: {payload}"))
}

fn other<'a>(payload: &'a Value, key: &str) -> &'a Value {
    payload["record"]["other"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == key)
        .unwrap_or_else(|| panic!("missing other {key}: {payload}"))
}

fn field_schema<'a>(payload: &'a Value, key: &str) -> &'a Value {
    payload["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == key)
        .unwrap_or_else(|| panic!("missing field {key}: {payload}"))
}

fn write_show_record(vault: &Path, stem: &str, front: &str, body: &str) {
    write_file(
        &vault.join(format!("eat/{stem}.md")),
        &format!("---\n{front}---\n{body}"),
    );
}

fn healthy_front() -> &'static str {
    concat!(
        "title: Ninja Sushi\n",
        "parent: \"[[eat]]\"\n",
        "type: \"[[record]]\"\n",
        "created: \"2026-10-07T13:40:00-0400\"\n",
        "status: liked\n",
        "category: Sushi\n",
        "location: Clark\n",
        "tags:\n",
        "  - sushi\n",
        "  - lunch\n",
        "rating: 4\n",
    )
}

fn init_git_vault(vault: &Path) {
    git_in(vault, ["init", "-q"]);
    git_in(vault, ["config", "user.email", "t@t"]);
    git_in(vault, ["config", "user.name", "t"]);
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-qm", "init"]);
}

fn read_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, fs::read(&path).unwrap());
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn fingerprint_ok(value: &Value) {
    let text = value.as_str().unwrap_or_default();
    assert!(text.starts_with("sha256:"), "{value}");
    assert_eq!(text.len(), "sha256:".len() + 64, "{value}");
    let hex = &text["sha256:".len()..];
    assert!(
        hex.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')),
        "{text}"
    );
}

#[test]
fn show_type_only_json_stays_schema_shaped() {
    let fixture = show_fixture();
    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        healthy_front(),
        "# Ninja Sushi\n",
    );
    let output = show_record(&fixture, &["show", "restaurant", "-f", "json"]);
    assert_success(&output);
    let payload = show_json(&output);
    assert!(payload.get("record").is_none(), "{payload}");
    for field in payload["fields"].as_array().unwrap() {
        assert!(
            field.get("values").is_none(),
            "TYPE-only schema must omit values: {field}"
        );
        assert!(field.get("top_values").is_some(), "{field}");
    }
    assert_eq!(payload["type"], "restaurant");
}

#[test]
fn show_one_record_reports_typed_properties_identity_and_fingerprint() {
    let fixture = show_fixture();
    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        healthy_front(),
        "# Ninja Sushi\n\n## Notes\n- First visit.\n",
    );
    for (index, category) in ["Ramen", "Pizza", "Tacos", "Burgers", "Thai"]
        .iter()
        .enumerate()
    {
        write_show_record(
            &fixture.vault,
            &format!("extra_{index}"),
            &format!(
                "parent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Extra {index}\nstatus: to_try\ncategory: {category}\n"
            ),
            &format!("# Extra {index}\n"),
        );
    }

    let output = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&output);
    let payload = show_json(&output);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["type"], "restaurant");
    assert_eq!(payload["record"]["stem"], "ninja_sushi");
    assert_eq!(payload["record"]["path"], "eat/ninja_sushi.md");
    assert_eq!(payload["record"]["title"], "Ninja Sushi");
    assert_eq!(payload["record"]["heading"], "Ninja Sushi");
    assert_eq!(payload["record"]["created"], "2026-10-07T13:40:00-0400");
    fingerprint_ok(&payload["record"]["fingerprint"]);
    fingerprint_ok(&payload["record"]["schema_fingerprint"]);
    assert_eq!(payload["record"]["frontmatter_end_line"], 13);
    assert_eq!(payload["record"]["group"]["field"], "status");
    assert_eq!(payload["record"]["group"]["value"], "liked");
    assert_eq!(payload["record"]["group"]["label"], "Liked");

    let status = property(&payload, "status");
    assert_eq!(status["state"], "set");
    assert_eq!(status["value"], "liked");
    assert_eq!(status["display"], "Liked");
    assert_eq!(status["raw"], "liked");
    assert_eq!(status["line"], 6);
    assert_eq!(status["required"], true);
    assert_eq!(status["kind"], "choices");

    let tags = property(&payload, "tags");
    assert_eq!(tags["state"], "set");
    assert_eq!(tags["list"], true);
    assert_eq!(tags["value"], serde_json::json!(["sushi", "lunch"]));
    assert_eq!(tags["display"], "sushi, lunch");

    assert_eq!(identity(&payload, "parent")["value"], "[[eat]]");
    assert_eq!(identity(&payload, "type")["value"], "[[record]]");
    assert_eq!(identity(&payload, "title")["value"], "Ninja Sushi");
    assert_eq!(
        identity(&payload, "created")["value"],
        "2026-10-07T13:40:00-0400"
    );

    let category = field_schema(&payload, "category");
    let top_values = category["top_values"].as_array().unwrap();
    let values = category["values"].as_array().unwrap();
    assert_eq!(top_values.len(), 5, "{category}");
    assert!(values.len() >= 6, "{category}");
    assert!(
        values.iter().any(|row| row["value"] == "Sushi"),
        "{category}"
    );
    assert!(field_schema(&payload, "status")["values"]
        .as_array()
        .unwrap()
        .is_empty());

    let catalog = bob_command()
        .arg("__catalog")
        .arg("--protocol")
        .arg("1")
        .output()
        .expect("catalog");
    assert_success(&catalog);
    let catalog_json: Value =
        serde_json::from_str(&stdout(&catalog)).expect("parse catalog");
    let show = catalog_json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "records-show")
        .expect("records-show");
    let slots: std::collections::BTreeSet<String> = show["args"]
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
    assert!(slots.contains("TYPE:record-type"), "{slots:?}");
    assert!(slots.contains("STEM:record-stem"), "{slots:?}");

    let human = show_record(&fixture, &["show", "restaurant", "ninja_sushi"]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("Ninja Sushi"), "{text}");
    assert!(text.contains("Restaurant"), "{text}");
    assert!(text.contains("Liked"), "{text}");
    assert!(text.contains("0 other properties"), "{text}");
}

#[test]
fn show_one_record_marks_none_missing_invalid_legacy_and_heading() {
    let fixture = show_fixture();
    write_show_record(
        &fixture.vault,
        "sparse",
        concat!(
            "title: Equilibrium\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
            "status: liked\n",
            "category: Sushi\n",
        ),
        "# Different Heading\n",
    );
    write_show_record(
        &fixture.vault,
        "missing_status",
        concat!(
            "title: Missing\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
            "category: Sushi\n",
        ),
        "# Missing\n",
    );
    write_show_record(
        &fixture.vault,
        "invalid_choice",
        concat!(
            "title: Invalid\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
            "status: nope\n",
            "category: Sushi\n",
        ),
        "# Invalid\n",
    );
    write_show_record(
        &fixture.vault,
        "legacy_lid",
        concat!(
            "title: Legacy\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
            "status: liked\n",
            "category: Sushi\n",
            "lid: abc\n",
            "source_url: https://example.com\n",
            "extra: leftover\n",
        ),
        "# Legacy\n",
    );

    let sparse = show_json(&show_record(
        &fixture,
        &["show", "eat", "sparse", "-f", "json"],
    ));
    assert_eq!(sparse["record"]["title"], "Equilibrium");
    assert_eq!(sparse["record"]["heading"], "Different Heading");
    assert_eq!(property(&sparse, "location")["state"], "none");
    assert!(property(&sparse, "location")["line"].is_null());
    assert_eq!(property(&sparse, "tags")["state"], "none");
    assert_eq!(property(&sparse, "rating")["state"], "none");
    assert_eq!(property(&sparse, "status")["state"], "set");
    assert!(sparse["record"]["group"].is_object());

    let missing = show_record(
        &fixture,
        &["show", "restaurant", "missing_status", "-f", "json"],
    );
    assert_success(&missing);
    let missing_json = show_json(&missing);
    assert_eq!(property(&missing_json, "status")["state"], "missing");
    assert!(property(&missing_json, "status")["line"].is_null());
    let codes: Vec<_> = missing_json["record"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap_or_default())
        .collect();
    assert!(codes.contains(&"record_missing_field"), "{codes:?}");
    assert!(missing_json["record"]["group"].is_null());

    let invalid = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "invalid_choice", "-f", "json"],
    ));
    let status = property(&invalid, "status");
    assert_eq!(status["state"], "invalid");
    assert_eq!(status["raw"], "nope");
    assert!(status["value"].is_null());
    assert!(status["display"].is_null());
    assert_eq!(status["line"], 5);

    let legacy = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "legacy_lid", "-f", "json"],
    ));
    assert_eq!(other(&legacy, "lid")["legacy"], true);
    assert_eq!(other(&legacy, "source_url")["legacy"], true);
    assert_eq!(other(&legacy, "extra")["legacy"], false);
    assert_eq!(other(&legacy, "lid")["value"], "abc");

    let human =
        show_record(&fixture, &["show", "restaurant", "missing_status"]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("missing"), "{text}");
    assert!(text.contains("other properties"), "{text}");
}

#[test]
fn show_one_record_accepts_crlf_and_bom_and_stays_read_only() {
    let fixture = show_fixture();
    let unix = format!("---\n{}---\n# Ninja Sushi\n", healthy_front());
    let crlf = unix.replace('\n', "\r\n");
    write_file(&fixture.vault.join("eat/ninja_sushi.md"), &crlf);
    init_git_vault(&fixture.vault);
    let before = read_tree(&fixture.vault);
    let head = stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]));

    let crlf_json = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&crlf_json);
    let payload = show_json(&crlf_json);
    assert_eq!(property(&payload, "status")["line"], 6);
    assert_eq!(payload["record"]["heading"], "Ninja Sushi");
    assert_eq!(read_tree(&fixture.vault), before, "CRLF show wrote bytes");
    let fingerprint = payload["record"]["fingerprint"].clone();

    let bom = format!("\u{feff}{unix}");
    write_file(&fixture.vault.join("eat/ninja_sushi.md"), &bom);
    let bom_output = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&bom_output);
    let bom_json = show_json(&bom_output);
    assert_eq!(bom_json["record"]["fingerprint"], fingerprint);
    assert_eq!(property(&bom_json, "status")["line"], 6);

    write_file(&fixture.vault.join("eat/ninja_sushi.md"), &crlf);
    git_in(&fixture.vault, ["checkout", "--", "."]);
    let shown = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&shown);
    assert_eq!(read_tree(&fixture.vault), before, "vault bytes changed");
    assert_eq!(stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])), head);
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["status", "--porcelain"])),
        ""
    );
}

#[test]
fn show_one_record_refuses_unknown_parent_mismatch_and_outside_folder() {
    let fixture = show_fixture();
    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        healthy_front(),
        "# Ninja Sushi\n",
    );
    write_show_record(
        &fixture.vault,
        "wrong_parent",
        concat!(
            "title: Wrong\n",
            "parent: \"[[body]]\"\n",
            "type: \"[[record]]\"\n",
            "status: liked\n",
            "category: Sushi\n",
        ),
        "# Wrong\n",
    );
    fs::create_dir_all(fixture.vault.join("eat/nested")).expect("nested");
    write_file(
        &fixture.vault.join("eat/nested/buried.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Buried\nstatus: liked\ncategory: Sushi\n---\n# Buried\n",
    );
    write_file(
        &fixture.vault.join("stray.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Stray\nstatus: liked\ncategory: Sushi\n---\n# Stray\n",
    );

    let unknown = show_record(
        &fixture,
        &["show", "restaurant", "missing_note", "-f", "json"],
    );
    assert_eq!(unknown.status.code(), Some(2));
    let unknown_json = show_json(&unknown);
    assert_eq!(unknown_json["error"]["code"], "record_not_found");
    assert!(unknown_json.get("record").is_none(), "{unknown_json}");

    let mismatch = show_record(
        &fixture,
        &["show", "restaurant", "wrong_parent", "-f", "json"],
    );
    assert_eq!(mismatch.status.code(), Some(2));
    let mismatch_json = show_json(&mismatch);
    assert_eq!(mismatch_json["error"]["code"], "record_parent_mismatch");
    assert!(
        mismatch_json["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("parent"),
        "{mismatch_json}"
    );

    let nested =
        show_record(&fixture, &["show", "restaurant", "buried", "-f", "json"]);
    assert_eq!(nested.status.code(), Some(2));
    assert_eq!(show_json(&nested)["error"]["code"], "record_outside_album");

    let stray =
        show_record(&fixture, &["show", "restaurant", "stray", "-f", "json"]);
    assert_eq!(stray.status.code(), Some(2));
    assert_eq!(show_json(&stray)["error"]["code"], "record_outside_album");
}

#[test]
fn show_one_record_fingerprint_ignores_format_and_tracks_value_and_schema() {
    let fixture = show_fixture();
    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        healthy_front(),
        "# Ninja Sushi\n\n## Notes\n- one\n",
    );
    let first = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    ));
    let fingerprint = first["record"]["fingerprint"].clone();
    let schema = first["record"]["schema_fingerprint"].clone();

    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        concat!(
            "rating: 4\n",
            "tags: [sushi, lunch]\n",
            "location: Clark\n",
            "category: Sushi\n",
            "status: liked\n",
            "created: \"2026-10-07T13:40:00-0400\"\n",
            "type: \"[[record]]\"\n",
            "parent: \"[[eat]]\"\n",
            "title: Ninja Sushi\n",
        ),
        "# Ninja Sushi\n\n## Notes\n- two\n",
    );
    let reordered = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    ));
    assert_eq!(reordered["record"]["fingerprint"], fingerprint);
    assert_eq!(reordered["record"]["schema_fingerprint"], schema);

    write_show_record(
        &fixture.vault,
        "ninja_sushi",
        concat!(
            "title: Ninja Sushi\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[record]]\"\n",
            "created: \"2026-10-07T13:40:00-0400\"\n",
            "status: to_try\n",
            "category: Sushi\n",
            "location: Clark\n",
            "tags:\n",
            "  - sushi\n",
            "  - lunch\n",
            "rating: 4\n",
        ),
        "# Ninja Sushi\n",
    );
    let changed = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    ));
    assert_ne!(changed["record"]["fingerprint"], fingerprint);
    assert_eq!(changed["record"]["schema_fingerprint"], schema);

    let mutated_album = SHOW_ALBUM.replace(
        "    rating: {values: number}\n",
        "    rating: {values: number}\n    url: {}\n",
    );
    write_file(&fixture.vault.join("eat.md"), &mutated_album);
    let schema_changed = show_json(&show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    ));
    assert_ne!(schema_changed["record"]["schema_fingerprint"], schema);
    assert_ne!(
        schema_changed["record"]["fingerprint"],
        changed["record"]["fingerprint"]
    );
}

fn envelope_fixture() -> RecordsFixture {
    let fixture = update_fixture();
    write_ninja_update(&fixture.vault);
    fixture
}

fn update_with_envelope(
    fixture: &RecordsFixture,
    args: &[&str],
) -> std::process::Output {
    records_command(fixture)
        .args(args)
        .output()
        .expect("bob records update")
}

fn envelope_json(output: &std::process::Output) -> Value {
    json(output)
}

fn show_fingerprint(fixture: &RecordsFixture, stem: &str) -> String {
    let output = update_with_envelope(
        fixture,
        &["show", "restaurant", stem, "-f", "json"],
    );
    assert_success(&output);
    json(&output)["record"]["fingerprint"]
        .as_str()
        .expect("record fingerprint")
        .to_owned()
}

fn write_envelope(vault: &Path, name: &str, text: &str) -> PathBuf {
    let path = vault.join(name);
    write_file(&path, text);
    path
}

fn envelope_run(
    fixture: &RecordsFixture,
    envelope: &str,
    extra: &[&str],
) -> std::process::Output {
    let path = write_envelope(&fixture.vault, "envelope.json", envelope);
    let path_text = path.to_string_lossy().into_owned();
    let mut args =
        vec!["update", "restaurant", "ninja_sushi", "-i", &path_text];
    args.extend(extra);
    update_with_envelope(fixture, &args)
}

fn envelope_doc(fingerprint: &str, ops: &str) -> String {
    format!(
        "{{\"schema_version\":1,\"expect\":\"{fingerprint}\",\"ops\":[{ops}]}}"
    )
}

#[test]
fn update_envelope_set_unset_and_list_replace() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"status","value":"liked"},{"op":"field.unset","field":"location"},{"op":"field.set","field":"tags","value":["spicy","lunch"]}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "update");
    assert_eq!(payload["dry_run"], false);
    assert_eq!(payload["type"], "restaurant");
    assert_eq!(payload["stem"], "ninja_sushi");
    assert_eq!(payload["path"], "eat/ninja_sushi.md");
    assert_eq!(payload["error"], Value::Null);
    assert_eq!(
        payload["changed"],
        serde_json::json!(["status", "location", "tags"])
    );
    let changes = payload["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["key"], "status");
    assert_eq!(changes[0]["before_display"], "To try");
    assert_eq!(changes[0]["after_display"], "Liked");
    assert_eq!(changes[1]["key"], "location");
    assert_eq!(changes[1]["before_display"], "Clark");
    assert_eq!(changes[1]["after_display"], Value::Null);
    assert!(
        payload["fingerprint_before"].as_str().unwrap()
            != payload["fingerprint_after"].as_str().unwrap()
    );
    fingerprint_ok(&payload["fingerprint_after"]);
    assert_eq!(payload["group_before"]["value"], "to_try");
    assert_eq!(payload["group_after"]["value"], "liked");
    assert_eq!(payload["group_after"]["label"], "Liked");
    assert_eq!(
        payload["consequence"],
        "3 fields updated (Status, Location, Tags); moves to the Liked group"
    );
    let inverse = payload["inverse_ops"].as_array().expect("inverse");
    assert_eq!(inverse.len(), 3);
    assert_eq!(
        inverse[0],
        serde_json::json!({"op":"field.unset","field":"tags"})
    );
    assert_eq!(
        inverse[1],
        serde_json::json!({"op":"field.set","field":"location","value":"Clark"})
    );
    assert_eq!(
        inverse[2],
        serde_json::json!({"op":"field.set","field":"status","value":"to_try"})
    );
    // The album index rebuilds in the same guarded write.
    assert_eq!(payload["album_index"]["path"], "eat.md");

    let markdown =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert!(markdown.contains("status: liked"), "{markdown}");
    assert!(!markdown.contains("location:"), "{markdown}");
    assert!(markdown.contains("category: Sushi"), "{markdown}");
    let index = fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    assert!(index.contains("[[eat/ninja_sushi|Ninja Sushi]]"), "{index}");
}

#[test]
fn update_envelope_empty_ops_is_noop() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let output = envelope_run(
        &fixture,
        &envelope_doc(&fingerprint, ""),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["changed"], serde_json::json!([]));
    assert_eq!(payload["changes"], serde_json::json!([]));
    assert_eq!(payload["inverse_ops"], serde_json::json!([]));
    assert_eq!(payload["fingerprint_after"], payload["fingerprint_before"]);
    assert_eq!(
        payload["consequence"],
        "no-op: the envelope carries no operations"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "no-op preserves bytes"
    );
}

#[test]
fn update_envelope_stale_repaints_in_one_call() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    // A concurrent edit lands after the envelope was read.
    let classic = update_with_envelope(
        &fixture,
        &["update", "restaurant", "ninja_sushi", "-s", "status=liked"],
    );
    assert_success(&classic);

    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"status","value":"to_try"}"#,
        ),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2));
    let payload = envelope_json(&output);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["code"], "stale_record");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("nothing was written"));
    assert!(
        payload["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("fingerprint"),
        "{payload}"
    );
    let current = show_fingerprint(&fixture, "ninja_sushi");
    assert_ne!(current, fingerprint);
    assert_eq!(payload["record"]["fingerprint"], current);
    assert_eq!(payload["fingerprint_before"], current);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before.replace("status: to_try", "status: liked"),
        "stale writes nothing"
    );

    let human = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"status","value":"to_try"}"#,
        ),
        &[],
    );
    assert_eq!(human.status.code(), Some(2));
    assert!(
        stderr(&human).contains("changed on disk"),
        "{}",
        format_output(&human)
    );
    assert!(
        stderr(&human).contains("hint: re-read"),
        "{}",
        format_output(&human)
    );
}

#[test]
fn update_envelope_typed_refusals() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let cases = [
        (
            r#"{"op":"field.set","field":"status","value":"bogus"}"#,
            "record_invalid_value",
            "status",
        ),
        (
            r#"{"op":"field.unset","field":"status"}"#,
            "record_required_empty",
            "status",
        ),
        (
            r#"{"op":"field.set","field":"statu","value":"liked"}"#,
            "record_unknown_field",
            "statu",
        ),
        (
            r#"{"op":"field.set","field":"location","value":""}"#,
            "envelope_empty_value",
            "location",
        ),
    ];
    for (ops, code, field) in cases {
        let output = envelope_run(
            &fixture,
            &envelope_doc(&fingerprint, ops),
            &["-f", "json"],
        );
        assert_eq!(output.status.code(), Some(2), "{ops}");
        let payload = envelope_json(&output);
        assert_eq!(payload["ok"], false, "{ops}");
        assert_eq!(payload["error"]["code"], code, "{ops}");
        assert_eq!(payload["error"]["field"], field, "{ops}");
        assert_eq!(payload["changed"], serde_json::json!([]), "{ops}");
        assert_eq!(
            fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md"))
                .unwrap(),
            before,
            "refusal writes nothing: {ops}"
        );
    }
    let unknown = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"statu","value":"liked"}"#,
        ),
        &["-f", "json"],
    );
    assert!(envelope_json(&unknown)["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("status"));
    let empty = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"location","value":""}"#,
        ),
        &["-f", "json"],
    );
    assert!(envelope_json(&empty)["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("field.unset"));
}

#[test]
fn update_envelope_protocol_refusals() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let cases = [
        (
            r#"{"op":"field.set","field":"status","value":"liked"},{"op":"field.unset","field":"status"}"#,
            "envelope_duplicate_field",
        ),
        (
            r#"{"op":"field.add","field":"status"}"#,
            "envelope_unknown_op",
        ),
        (
            r#"{"op":"field.set","field":"status","value":["liked"]}"#,
            "envelope_wrong_type",
        ),
        (
            r#"{"op":"field.set","field":"tags","value":"spicy"}"#,
            "envelope_wrong_type",
        ),
    ];
    for (ops, code) in cases {
        let output = envelope_run(
            &fixture,
            &envelope_doc(&fingerprint, ops),
            &["-f", "json"],
        );
        assert_eq!(output.status.code(), Some(2), "{ops}");
        assert_eq!(envelope_json(&output)["error"]["code"], code, "{ops}");
    }
    let duplicate_key = format!(
        "{{\"schema_version\":1,\"expect\":\"{fingerprint}\",\"ops\":[],\"ops\":[]}}"
    );
    let output = envelope_run(&fixture, &duplicate_key, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelope_json(&output)["error"]["code"],
        "envelope_duplicate_key"
    );
    let wrong_version = format!(
        "{{\"schema_version\":2,\"expect\":\"{fingerprint}\",\"ops\":[]}}"
    );
    let output = envelope_run(&fixture, &wrong_version, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        envelope_json(&output)["error"]["code"],
        "envelope_unknown_version"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "protocol refusals write nothing"
    );
}

#[test]
fn update_envelope_typed_numbers_and_comma_lists_round_trip() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"rating","value":4.5}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    assert_eq!(payload["changes"][0]["after_display"], "4.5", "{payload}");
    assert_eq!(payload["consequence"], "Rating: set to 4.5");

    let second = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &second,
            r#"{"op":"field.set","field":"rating","value":"4"}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    assert_eq!(envelope_json(&output)["changes"][0]["after_display"], "4");

    let third = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &third,
            r#"{"op":"field.set","field":"tags","value":["a,b","c"]}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    let markdown =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert!(markdown.contains("  - \"a,b\"\n  - \"c\"\n"), "{markdown}");
    let tags = payload["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["key"] == "tags")
        .expect("tags property");
    assert_eq!(tags["value"], serde_json::json!(["a,b", "c"]));

    let shown = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&shown);
    let shown = show_json(&shown);
    assert_eq!(property(&shown, "tags")["raw"], tags["value"]);
    assert_eq!(property(&shown, "tags")["value"], tags["value"]);

    // Undo replays with expect = fingerprint_after and ops = inverse_ops.
    let after = payload["fingerprint_after"].as_str().unwrap().to_owned();
    let inverse = payload["inverse_ops"].clone();
    let replay = format!(
        "{{\"schema_version\":1,\"expect\":\"{after}\",\"ops\":{inverse}}}"
    );
    let output = envelope_run(&fixture, &replay, &["-f", "json"]);
    assert_success(&output);
    let replayed = envelope_json(&output);
    // Only the tags op replays; the earlier rating edit stays applied.
    assert_eq!(replayed["fingerprint_after"], third);
    let restored =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    assert!(!restored.contains("tags:"), "{restored}");
    assert!(restored.contains("rating: 4"), "{restored}");
}

#[test]
fn show_record_literal_list_choices_keep_commas() {
    let fixture = envelope_fixture();
    let album = fixture.vault.join("eat.md");
    let schema = fs::read_to_string(&album).unwrap().replace(
        "tags: {list: true}",
        "tags: {list: true, values: {'Science, fiction': Fiction, 'c,d': Other}}",
    );
    write_file(&album, &schema);
    let note = fixture.vault.join("eat/ninja_sushi.md");
    let original = fs::read_to_string(&note).unwrap().replacen(
        "---\n",
        "---\ntags: [\"Science, fiction\", \"c,d\"]\n",
        1,
    );
    write_file(&note, &original);
    let shown = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&shown);
    let shown = show_json(&shown);
    let tags = property(&shown, "tags");
    assert_eq!(tags["state"], "set");
    assert_eq!(
        tags["value"],
        serde_json::json!(["Science, fiction", "c,d"])
    );
    assert_eq!(tags["display"], "Fiction, Other");
    assert!(shown["record"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|finding| finding["field"] != "tags"
            && finding["key_path"] != "tags"));
    assert_eq!(fs::read_to_string(&note).unwrap(), original);
}

#[test]
fn update_envelope_literal_list_noop_preserves_bytes() {
    let fixture = envelope_fixture();
    let note = fixture.vault.join("eat/ninja_sushi.md");
    let original = fs::read_to_string(&note).unwrap();
    let original = original.replacen(
        "---\n",
        "---\ntags: [\"Science, fiction\", \"c,d\"]\n",
        1,
    );
    write_file(&note, &original);
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"tags","value":["Science, fiction","c,d"]}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    assert_eq!(payload["changed"], serde_json::json!([]));
    assert_eq!(payload["fingerprint_after"], fingerprint);
    assert_eq!(fs::read_to_string(&note).unwrap(), original);
}

#[test]
fn update_envelope_literal_list_current_and_undo_keep_commas() {
    let fixture = envelope_fixture();
    let note = fixture.vault.join("eat/ninja_sushi.md");
    let original = fs::read_to_string(&note).unwrap().replacen(
        "---\n",
        "---\ntags:\n  - \"Science, fiction\"\n  - \"c,d\"\n",
        1,
    );
    write_file(&note, &original);
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"tags","value":["replacement"]}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    let tags = payload["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "tags")
        .unwrap();
    let expected = serde_json::json!(["Science, fiction", "c,d"]);
    assert_eq!(tags["before"], expected);
    assert_eq!(payload["inverse_ops"][0]["value"], expected);
    let replay = serde_json::json!({
        "schema_version": 1,
        "expect": payload["fingerprint_after"],
        "ops": payload["inverse_ops"],
    });
    let output = envelope_run(&fixture, &replay.to_string(), &["-f", "json"]);
    assert_success(&output);
    assert_eq!(envelope_json(&output)["fingerprint_after"], fingerprint);
    assert_eq!(fs::read_to_string(&note).unwrap(), original);
    let shown = show_record(
        &fixture,
        &["show", "restaurant", "ninja_sushi", "-f", "json"],
    );
    assert_success(&shown);
    assert_eq!(property(&show_json(&shown), "tags")["value"], expected);
}

#[test]
fn update_envelope_dry_run_and_stdin() {
    let fixture = envelope_fixture();
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let before =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let album_before =
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap();
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"location","value":"Queens"}"#,
        ),
        &["-d", "-f", "json"],
    );
    assert_success(&output);
    let payload = envelope_json(&output);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(
        payload["consequence"],
        "would apply: Location: Clark → Queens"
    );
    assert_ne!(payload["fingerprint_after"], payload["fingerprint_before"]);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        before,
        "dry run writes nothing"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "dry run leaves the index alone"
    );

    let path = write_envelope(
        &fixture.vault,
        "stdin-envelope.json",
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"location","value":"Queens"}"#,
        ),
    );
    let stdin_text = fs::read_to_string(&path).unwrap();
    let mut command = records_command(&fixture);
    let output = command
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-i",
            "-",
            "-f",
            "json",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(stdin_text.as_bytes())?;
            child.wait_with_output()
        })
        .expect("bob records update over stdin");
    assert_success(&output);
    assert_eq!(envelope_json(&output)["ok"], true);

    let conflicted = update_with_envelope(
        &fixture,
        &[
            "update",
            "restaurant",
            "ninja_sushi",
            "-i",
            "envelope.json",
            "-s",
            "status=liked",
            "-f",
            "json",
        ],
    );
    assert_eq!(conflicted.status.code(), Some(2));
    assert!(stderr(&conflicted).contains("cannot be used with"));
}

#[test]
fn update_envelope_preserves_crlf_and_refuses_bom() {
    let fixture = envelope_fixture();
    let original =
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    write_file(
        &fixture.vault.join("eat/ninja_sushi.md"),
        &original.replace('\n', "\r\n"),
    );
    let fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &fingerprint,
            r#"{"op":"field.set","field":"status","value":"liked"}"#,
        ),
        &["-f", "json"],
    );
    assert_success(&output);
    let bytes = fs::read(fixture.vault.join("eat/ninja_sushi.md")).unwrap();
    let text = String::from_utf8(bytes).expect("utf8 record");
    assert!(text.contains("\r\n"), "CRLF preserved");
    assert!(!text.contains("status: to_try"), "{text}");
    assert!(text.contains("status: liked"), "{text}");
    assert!(text.contains("category: Sushi\r\n"), "{text}");

    // A BOM note refuses like the classic path and keeps every byte.
    let bom = format!("\u{feff}{original}");
    write_file(&fixture.vault.join("eat/ninja_sushi.md"), &bom);
    let bom_fingerprint = show_fingerprint(&fixture, "ninja_sushi");
    let output = envelope_run(
        &fixture,
        &envelope_doc(
            &bom_fingerprint,
            r#"{"op":"field.set","field":"status","value":"liked"}"#,
        ),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope_json(&output)["error"]["code"], "record_uneditable");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat/ninja_sushi.md")).unwrap(),
        bom,
        "BOM refusal writes nothing"
    );
}

#[test]
fn update_envelope_capability_in_album_reports() {
    let fixture = envelope_fixture();
    for command in ["list", "check"] {
        let mut albums = bob_command();
        let output = albums
            .arg("albums")
            .arg(command)
            .env("BOB_DIR", &fixture.vault)
            .env("BOB_CONFIG_FILE", &fixture.config)
            .env("BOB_NOW", "2026-10-07 13:40:00")
            .env("NO_COLOR", "1")
            .arg("-f")
            .arg("json")
            .output()
            .expect("bob albums report");
        assert_success(&output);
        assert_eq!(
            json(&output)["capabilities"],
            serde_json::json!(["record_values_v1", "record_walk_v1"]),
            "{command} carries the capability flag"
        );
        assert_eq!(json(&output)["schema_version"], 1);
    }
}
fn write_tidy_records(vault: &std::path::Path) {
    fs::create_dir_all(vault.join("podcasts")).expect("podcasts");
    write_file(
        &vault.join("podcasts.md"),
        concat!(
            "---\n",
            "type: [[album]]\nparent: \"[[body]]\"\nsummary: Podcasts.\n",
            "record:\n  noun: podcast\n  label: Podcast\n",
            "  fields:\n    status: {required: true}\n    category: {required: true}\n",
            "---\n\n# Podcasts\n"
        ),
    );
    write_file(
        &vault.join("eat/empty_eof.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Empty Eof\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Empty Eof\n\n## Notes\n",
    );
    write_file(
        &vault.join("eat/empty_source.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Empty Source\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Empty Source\n\n## Notes\n\n## Source\n\n```text\nrow\n```\n",
    );
    write_file(
        &vault.join("eat/kept.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Kept\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Kept\n\n## Notes\n\n- hello\n",
    );
    write_file(
        &vault.join("eat/differing.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Differing\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Something Else\n\n## Notes\n\n- hello\n",
    );
    write_file(
        &vault.join("eat/buried.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Buried\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\nIntro prose.\n\n# Buried\n",
    );
    write_file(
        &vault.join("eat/fenced.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Fenced\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n```text\n# Fenced\n```\n\n- hello\n",
    );
    write_file(
        &vault.join("eat/quoted.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: \"Ma \\\"Pa\\\" Deli\"\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Ma \"Pa\" Deli\n",
    );
    write_file(
        &vault.join("eat/not_a_record.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[area]]\"\ntitle: Not A Record\n---\n\n# Not A Record\n\n## Notes\n",
    );
    write_file(
        &vault.join("eat.md.bak"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Backup\nstatus: to_try\ncategory: Ramen\nlocation: Clark\n---\n\n# Backup\n\n## Notes\n",
    );
    write_file(
        &vault.join("podcasts/empty_cast.md"),
        "---\nparent: \"[[podcasts]]\"\ntype: \"[[record]]\"\ntitle: Empty Cast\nstatus: to_try\ncategory: python\n---\n\n# Empty Cast\n\n## Notes\n",
    );
}

fn tidy_fixture() -> RecordsFixture {
    let fixture = fixture();
    write_tidy_records(&fixture.vault);
    fixture
}

fn tidy_sync_fixture(temp: &TempDir) -> (PathBuf, PathBuf) {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "summary: Places to eat.\n",
            "emoji: \"🍽️\"\n",
            "record:\n",
            "  noun: restaurant\n",
            "  label: Restaurant\n",
            "  fields:\n",
            "    status:\n",
            "      required: true\n",
            "      values: {to_try: \"To try\", liked: \"Liked\"}\n",
            "    category: {required: true}\n",
            "    location: {required: true}\n",
            "---\n\n# Restaurants\n"
        ),
    );
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_tidy_records(&vault);
    let config = temp.path().join("config.yml");
    write_file(&config, RECORDS_CONFIG);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "tidy fixture"]);
    git_in(&vault, ["push", "-q"]);
    (vault, config)
}

fn tidy_sync_command(
    vault: &PathBuf,
    config: &PathBuf,
    temp: &TempDir,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("records")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", "2026-10-07 13:40:00")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", temp.path().join("home-dir"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command
}

#[test]
fn tidy_dry_run_writes_nothing_and_leaves_head_unchanged() {
    let fixture = tidy_fixture();
    init_git_vault(&fixture.vault);
    let before = read_tree(&fixture.vault);
    let head = stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]));

    let dry = records_command(&fixture)
        .args(["tidy", "-d"])
        .output()
        .expect("tidy dry-run");
    assert_success(&dry);
    assert_eq!(read_tree(&fixture.vault), before, "dry-run writes nothing");
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])),
        head,
        "dry-run commits nothing"
    );
    let text = stdout(&dry);
    assert!(text.contains("empty_eof.md"), "{text}");
    assert!(text.contains("empty_source.md"), "{text}");
    assert!(text.contains("empty_cast.md"), "{text}");
    // kept.md loses only its redundant H1; quoted.md loses its quoted H1.
    assert!(text.contains("kept.md"), "{text}");
    assert!(text.contains("quoted.md"), "{text}");
    assert!(text.contains("3 empty Notes sections"), "{text}");
    assert!(text.contains("5 redundant H1s"), "{text}");
    assert!(!text.contains("not_a_record.md"), "{text}");
    assert!(!text.contains("fenced.md"), "{text}");
    // Differing and buried H1s are kept, with reasons.
    assert!(text.contains("2 notes keep their H1"), "{text}");
    assert!(text.contains("differing.md"), "{text}");
    assert!(text.contains("buried.md"), "{text}");
}

#[test]
fn tidy_real_run_commits_once_and_changes_only_record_bodies() {
    let temp = TempDir::new("bob-cli-records-tidy-real");
    let (vault, config) = tidy_sync_fixture(&temp);

    let real = tidy_sync_command(&vault, &config, &temp, &["tidy"])
        .output()
        .expect("tidy real run");
    assert_success(&real);
    let text = stdout(&real);
    assert!(text.contains("5 notes"), "{text}");
    assert!(text.contains("3 empty Notes sections"), "{text}");
    assert!(text.contains("5 redundant H1s"), "{text}");

    let log = stdout(&git_in(&vault, ["log", "--oneline", "-2"]));
    assert!(log.contains("records: tidy (5 notes)"), "{log}");

    for path in [
        "eat/empty_eof.md",
        "eat/empty_source.md",
        "podcasts/empty_cast.md",
    ] {
        let contents = fs::read_to_string(vault.join(path)).expect("read");
        assert!(!contents.contains("## Notes"), "{path}: {contents}");
        assert!(
            contents
                .lines()
                .all(|line| line.starts_with("## ") || !line.starts_with('#')),
            "{path} lost its H1: {contents}"
        );
    }
    // The H1-only note ends right after the frontmatter.
    let quoted = fs::read_to_string(vault.join("eat/quoted.md")).unwrap();
    assert!(quoted.ends_with("location: Clark\n---\n"), "{quoted}");
    // Non-empty records keep their Notes but lose the redundant H1;
    // differing and buried H1s are kept; non-records stay put.
    let kept = fs::read_to_string(vault.join("eat/kept.md")).unwrap();
    assert!(kept.contains("- hello"), "{kept}");
    assert!(!kept.contains("# Kept"), "{kept}");
    let differing = fs::read_to_string(vault.join("eat/differing.md")).unwrap();
    assert!(differing.contains("# Something Else"), "{differing}");
    let buried = fs::read_to_string(vault.join("eat/buried.md")).unwrap();
    assert!(buried.contains("# Buried"), "{buried}");
    let skipped =
        fs::read_to_string(vault.join("eat/not_a_record.md")).unwrap();
    assert!(skipped.contains("## Notes"), "{skipped}");
    let album = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(album.contains("# Restaurants"), "{album}");

    // A second run is a no-op with no new commit.
    let head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    let again = tidy_sync_command(&vault, &config, &temp, &["tidy"])
        .output()
        .expect("tidy no-op");
    assert_success(&again);
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        head,
        "no-op commits nothing"
    );
    assert!(stdout(&again).contains("0 tidied"), "{}", stdout(&again));
}

#[test]
fn tidy_type_scoping_and_json_shape() {
    let temp = TempDir::new("bob-cli-records-tidy-scope");
    let (vault, config) = tidy_sync_fixture(&temp);

    let scoped = tidy_sync_command(
        &vault,
        &config,
        &temp,
        &["tidy", "eat", "-d", "-f", "json"],
    )
    .output()
    .expect("tidy scoped dry-run");
    assert_success(&scoped);
    let payload = json(&scoped);
    assert_eq!(payload["command"], "tidy");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["counts"]["scanned"], 7);
    assert_eq!(payload["counts"]["changed"], 4);
    assert_eq!(payload["counts"]["h1_removed"], 4);
    assert_eq!(payload["counts"]["h1_kept"], 2);
    let paths = payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "eat/empty_eof.md",
            "eat/empty_source.md",
            "eat/kept.md",
            "eat/quoted.md"
        ]
    );
    assert!(payload["changes"][0].get("diff").is_some());
    assert_eq!(payload["changes"][0]["notes_removed"], true);
    assert_eq!(payload["changes"][0]["h1_removed"], true);
    assert!(payload["changes"][0]["h1_kept_reason"].is_null());
    assert_eq!(payload["changes"][2]["notes_removed"], false);
    assert_eq!(payload["changes"][2]["h1_removed"], true);
    assert_eq!(payload["kept_h1"][0]["path"], "eat/buried.md");
    assert_eq!(
        payload["kept_h1"][1]["reason"],
        "the H1 differs from the frontmatter title"
    );
    assert!(payload["commit"].is_null());

    let unknown = tidy_sync_command(
        &vault,
        &config,
        &temp,
        &["tidy", "nope", "-f", "json"],
    )
    .output()
    .expect("unknown type");
    assert_eq!(unknown.status.code(), Some(2));

    // JSON real run omits diffs but keeps the envelope shape.
    let real = tidy_sync_command(
        &vault,
        &config,
        &temp,
        &["tidy", "podcast", "-f", "json"],
    )
    .output()
    .expect("tidy podcast");
    assert_success(&real);
    let real_json = json(&real);
    assert_eq!(real_json["counts"]["changed"], 1);
    assert_eq!(real_json["counts"]["h1_removed"], 1);
    assert_eq!(real_json["counts"]["h1_kept"], 0);
    assert!(real_json["changes"][0].get("diff").is_none());
    assert_eq!(real_json["changes"][0]["notes_removed"], true);
    assert_eq!(real_json["changes"][0]["h1_removed"], true);
    assert!(real_json["commit"]["subject"]
        .as_str()
        .unwrap()
        .contains("records: tidy (1 notes)"));
}

#[test]
fn tidy_update_inserts_notes_above_source() {
    let fixture = update_fixture();
    write_file(
        &fixture.vault.join("eat/sourced.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: Sourced\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n\n# Sourced\n\n## Source\n\n```text\nrow\n```\n",
    );
    let updated = records_command(&fixture)
        .args([
            "update",
            "restaurant",
            "sourced",
            "-n",
            "Revisit was great",
            "-f",
            "json",
        ])
        .output()
        .expect("update sourced record");
    assert_success(&updated);
    let markdown =
        fs::read_to_string(fixture.vault.join("eat/sourced.md")).unwrap();
    assert!(
        markdown.contains(
            "## Notes\n\n- 2026-10-07: Revisit was great\n\n## Source"
        ),
        "Notes inserted above Source: {markdown}"
    );
}

fn walk_vault() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records-walk");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write_file(&config, RECORDS_CONFIG);
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places to eat.\nemoji: \"🍽️\"\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields:\n    status:\n      required: true\n      values: {to_try: \"To try\", liked: \"Liked\"}\n    category: {required: true}\n    location: {required: true}\n---\n\n# Eat\n",
    );
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

fn write_walk_record(vault: &Path, stem: &str, title: &str, body: &str) {
    write_file(
        &vault.join(format!("eat/{stem}.md")),
        &format!(
            "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: \"{title}\"\nstatus: liked\ncategory: Sushi\nlocation: Clark\n---\n{body}"
        ),
    );
}

#[test]
fn walk_show_reports_ring_positions() {
    let fixture = walk_vault();
    write_walk_record(
        &fixture.vault,
        "apple",
        "Apple",
        "\n[[eat/zebra|‹ Zebra]] · [[eat|↑ Eat]] · [[eat/mango|Mango ›]]\n",
    );
    write_walk_record(
        &fixture.vault,
        "mango",
        "Mango",
        "\n[[eat/apple|‹ Apple]] · [[eat|↑ Eat]] · [[eat/zebra|Zebra ›]]\n",
    );
    write_walk_record(
        &fixture.vault,
        "zebra",
        "Zebra",
        "\n[[eat/mango|‹ Mango]] · [[eat|↑ Eat]] · [[eat/apple|Apple ›]]\n",
    );
    let output = records_command(&fixture)
        .args(["show", "restaurant", "mango", "-f", "json"])
        .output()
        .expect("show mango");
    assert_success(&output);
    let payload = json(&output);
    let walk = &payload["record"]["walk"];
    assert_eq!(walk["state"], "current");
    assert_eq!(walk["position"], 2);
    assert_eq!(walk["total"], 3);
    assert_eq!(walk["prev"]["stem"], "apple");
    assert_eq!(walk["next"]["stem"], "zebra");
    let human = records_command(&fixture)
        .args(["show", "restaurant", "mango"])
        .output()
        .expect("show mango human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("2 of 3 · ‹ Apple · Zebra ›"), "{text}");
    assert!(!text.contains("walk line"), "{text}");

    // Singleton rings carry only the album link and a bare count.
    let solo = walk_vault();
    write_walk_record(&solo.vault, "only", "Only", "\n[[eat|↑ Eat]]\n");
    let output = records_command(&solo)
        .args(["show", "restaurant", "only", "-f", "json"])
        .output()
        .expect("show only");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["record"]["walk"]["state"], "current");
    assert_eq!(payload["record"]["walk"]["position"], 1);
    assert_eq!(payload["record"]["walk"]["total"], 1);
    assert!(payload["record"]["walk"]["prev"].is_null());
    let human = records_command(&solo)
        .args(["show", "restaurant", "only"])
        .output()
        .expect("show only human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("1 of 1"), "{text}");

    // Missing lines report the ring slot plus a muted repair hint.
    let missing = walk_vault();
    write_walk_record(&missing.vault, "apple", "Apple", "\n");
    write_walk_record(&missing.vault, "mango", "Mango", "\n");
    let output = records_command(&missing)
        .args(["show", "restaurant", "mango", "-f", "json"])
        .output()
        .expect("show missing");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["record"]["walk"]["state"], "missing");
    assert_eq!(payload["record"]["walk"]["position"], 2);
    let human = records_command(&missing)
        .args(["show", "restaurant", "mango"])
        .output()
        .expect("show missing human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("2 of 2"), "{text}");
    assert!(
        text.contains("walk line missing — bob albums sync"),
        "{text}"
    );
}

#[test]
fn walk_show_is_null_when_the_album_is_held() {
    let fixture = walk_vault();
    write_walk_record(&fixture.vault, "apple", "Apple", "\n");
    fs::write(
        fixture.vault.join("eat/unreadable.md"),
        b"---\ntitle: \xff\n---\n".as_slice(),
    )
    .expect("unreadable member");

    let output = records_command(&fixture)
        .args(["show", "restaurant", "apple", "-f", "json"])
        .output()
        .expect("show held album record");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["record"]["walk"], Value::Null);

    let human = records_command(&fixture)
        .args(["show", "restaurant", "apple"])
        .output()
        .expect("show held album record human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(!text.contains("1 of 1"), "{text}");
    assert!(!text.contains("N of M"), "{text}");
}

#[test]
fn walk_check_reports_stale_and_damaged() {
    let fixture = walk_vault();
    write_walk_record(&fixture.vault, "apple", "Apple", "\n");
    write_walk_record(&fixture.vault, "mango", "Mango", "\n");
    let output = records_command(&fixture)
        .args(["check", "restaurant", "-f", "json"])
        .output()
        .expect("check stale");
    assert_success(&output);
    let payload = json(&output);
    let findings = payload["types"][0]["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "record_walk_stale"),
        "{payload}"
    );
    assert!(
        !findings
            .iter()
            .any(|finding| finding["code"] == "record_walk_damaged"),
        "{payload}"
    );

    write_file(
        &fixture.vault.join("eat/apple.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\ntitle: \"Apple\"\nstatus: liked\ncategory: Sushi\nlocation: Clark\n---\n\n[[eat/mango|‹ Mango]] · [[eat|↑ Eat]] · [[eat/mango|Mango ›]]\n\n[[eat|↑ Eat]] extra\n",
    );
    let output = records_command(&fixture)
        .args(["check", "restaurant", "-f", "json"])
        .output()
        .expect("check damaged");
    assert_success(&output);
    let payload = json(&output);
    let findings = payload["types"][0]["findings"].as_array().unwrap();
    let damaged = findings
        .iter()
        .find(|finding| finding["code"] == "record_walk_damaged")
        .expect("damaged finding");
    assert_eq!(damaged["file"], "eat/apple.md");
    assert!(damaged["severity"] == "warning", "{payload}");
}

#[test]
fn walk_tidy_skips_walk_line_and_update_appends_after_it() {
    let fixture = walk_vault();
    let walk = "[[eat/apple|‹ Apple]] · [[eat|↑ Eat]] · [[eat/zebra|Zebra ›]]";
    // H1 directly below the walk line is still redundant.
    write_walk_record(
        &fixture.vault,
        "mango",
        "Mango",
        &format!("\n{walk}\n\n# Mango\n\n## Notes\n\n- hello\n"),
    );
    // Walk-only bodies gain Notes below the walk line.
    write_walk_record(
        &fixture.vault,
        "zebra",
        "Zebra",
        "\n[[eat/mango|‹ Mango]] · [[eat|↑ Eat]] · [[eat/apple|Apple ›]]\n",
    );
    write_walk_record(&fixture.vault, "apple", "Apple", "\n# Apple\n");
    let dry = records_command(&fixture)
        .args(["tidy", "restaurant", "-d"])
        .output()
        .expect("tidy dry-run");
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(text.contains("eat/mango.md"), "{text}");
    assert!(text.contains("redundant H1"), "{text}");

    // CRLF walk lines survive tidy with their endings intact.
    let crlf = walk_vault();
    let body =
        format!("\r\n{walk}\r\n\r\n# Mango\r\n\r\n## Notes\r\n\r\n- hello\r\n");
    write_file(
        &crlf.vault.join("eat/mango.md"),
        &format!(
            "---\r\nparent: \"[[eat]]\"\r\ntype: \"[[record]]\"\r\ntitle: \"Mango\"\r\nstatus: liked\r\ncategory: Sushi\r\nlocation: Clark\r\n---{body}"
        ),
    );
    write_walk_record(&crlf.vault, "apple", "Apple", "\n# Apple\n");
    let dry = records_command(&crlf)
        .args(["tidy", "restaurant", "-d"])
        .output()
        .expect("tidy crlf");
    assert_success(&dry);

    // Updates append notes after the walk line and stage no walk edits.
    let updated = records_command(&fixture)
        .args(["update", "restaurant", "zebra", "-n", "Back again"])
        .output()
        .expect("update zebra");
    assert_success(&updated);
    let markdown =
        fs::read_to_string(fixture.vault.join("eat/zebra.md")).unwrap();
    assert!(
        markdown.contains(
            "[[eat/mango|‹ Mango]] · [[eat|↑ Eat]] · [[eat/apple|Apple ›]]\n\n## Notes\n\n- Back again"
        ),
        "Notes appended below walk line: {markdown}"
    );
    let legacy = walk_vault();
    write_walk_record(
        &legacy.vault,
        "apple",
        "Apple",
        "\n[[eat/zebra|‹ Zebra]] · [[eat|↑ Eat]] · [[eat/mango|Mango ›]]\n\n# Apple\n\n## Source\n\n```text\nrow\n```\n",
    );
    let updated = records_command(&legacy)
        .args(["update", "restaurant", "apple", "-n", "Tasty"])
        .output()
        .expect("update apple");
    assert_success(&updated);
    let markdown =
        fs::read_to_string(legacy.vault.join("eat/apple.md")).unwrap();
    assert!(
        markdown.contains("## Notes\n\n- Tasty\n\n## Source"),
        "Notes inserted before sections: {markdown}"
    );
    assert!(
        markdown.contains(
            "[[eat/zebra|‹ Zebra]] · [[eat|↑ Eat]] · [[eat/mango|Mango ›]]\n\n# Apple"
        ),
        "walk line kept above H1: {markdown}"
    );
}
