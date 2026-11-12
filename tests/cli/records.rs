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
    assert_eq!(show_json["fields"][0]["choices"][0]["count"], 0);
    assert_eq!(show_json["fields"][0]["choices"][1]["count"], 1);
    assert_eq!(show_json["fields"][0]["filled"], 1);
    assert_eq!(show_json["fields"][0]["suggest"], false);
    assert_eq!(show_json["fields"][0]["line"], Value::Null);
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

const INIT_CONFIG: &str = r#"
records:
  widget:
    dir: widgets
    label: Widget
    emoji: "🧩"
    fields:
      status:
        required: true
        values:
          to_try: "🔖 To try"
          done: "✅ Done"
"#;

fn init_fixture() -> RecordsFixture {
    let temp = TempDir::new("bob-cli-records-init");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&config, INIT_CONFIG);
    RecordsFixture {
        _temp: temp,
        vault,
        config,
    }
}

fn piece<'a>(payload: &'a Value, kind: &str) -> &'a Value {
    payload["pieces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|piece| piece["kind"] == kind)
        .unwrap_or_else(|| panic!("missing piece {kind}: {payload}"))
}

#[test]
fn init_scaffolds_on_a_temporary_vault_without_creating_the_folder() {
    let fixture = init_fixture();
    let dry_run = records_command(&fixture)
        .args(["init", "widget", "-p", "stuff", "-d", "-f", "json"])
        .output()
        .expect("dry-run init");
    assert_success(&dry_run);
    let dry_json = json(&dry_run);
    assert_eq!(dry_json["status"], "planned");
    assert_eq!(piece(&dry_json, "type_note")["action"], "create");
    assert_eq!(piece(&dry_json, "collection")["action"], "create");
    assert_eq!(piece(&dry_json, "base")["action"], "create");
    assert!(!fixture.vault.join("widget.md").exists());
    assert!(!fixture.vault.join("widgets.md").exists());
    assert!(!fixture.vault.join("widgets.base").exists());
    assert!(!fixture.vault.join("widgets").exists());

    let written = records_command(&fixture)
        .args(["init", "widget", "-p", "stuff", "-f", "json"])
        .output()
        .expect("init widget");
    assert_success(&written);
    let payload = json(&written);
    assert_eq!(payload["status"], "written");
    assert_eq!(payload["type"], "widget");
    assert_eq!(payload["dir"], "widgets");

    let type_note = fs::read_to_string(fixture.vault.join("widget.md"))
        .expect("read type note");
    assert!(type_note.contains("parent: \"[[type]]\""), "{type_note}");
    assert!(
        type_note.contains("Schema: `bob records show widget`"),
        "{type_note}"
    );

    let collection = fs::read_to_string(fixture.vault.join("widgets.md"))
        .expect("read collection");
    assert!(collection.contains("parent: \"[[stuff]]\""), "{collection}");
    assert!(collection.contains("![[widgets.base]]"), "{collection}");

    let base = fs::read_to_string(fixture.vault.join("widgets.base"))
        .expect("read Base");
    let parsed: Value =
        serde_yaml::from_str(&base).expect("starter Base parses as YAML");
    assert!(
        parsed["filters"]["and"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str() == Some(r#"file.inFolder("widgets")"#)),
        "{base}"
    );
    assert_eq!(
        parsed["views"][0]["groupBy"]["property"].as_str(),
        Some("formula.status_label")
    );
    assert!(
        base.contains(r#"if(status == "to_try", "🔖 To try""#),
        "{base}"
    );
    assert!(
        !fixture.vault.join("widgets").exists(),
        "init must not create an empty record folder"
    );
}

#[test]
fn init_is_idempotent_and_never_overwrites() {
    let fixture = init_fixture();
    let first = records_command(&fixture)
        .args(["init", "widget", "-p", "stuff"])
        .output()
        .expect("first init");
    assert_success(&first);

    write_file(&fixture.vault.join("widget.md"), "KEEP THE TYPE NOTE\n");
    write_file(&fixture.vault.join("widgets.md"), "KEEP THE COLLECTION\n");
    let original_base =
        fs::read_to_string(fixture.vault.join("widgets.base")).unwrap();

    let second = records_command(&fixture)
        .args(["init", "widget", "-p", "other", "-f", "json"])
        .output()
        .expect("second init");
    assert_success(&second);
    let payload = json(&second);
    assert_eq!(piece(&payload, "type_note")["action"], "keep");
    assert_eq!(piece(&payload, "collection")["action"], "keep");
    assert_eq!(piece(&payload, "base")["action"], "keep");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("widget.md")).unwrap(),
        "KEEP THE TYPE NOTE\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("widgets.md")).unwrap(),
        "KEEP THE COLLECTION\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("widgets.base")).unwrap(),
        original_base
    );
}

#[test]
fn init_keeps_an_existing_collection_without_requiring_parent() {
    let fixture = init_fixture();
    write_file(&fixture.vault.join("widgets.md"), "already here\n");
    let output = records_command(&fixture)
        .args(["init", "widget", "-f", "json"])
        .output()
        .expect("init without parent");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(piece(&payload, "collection")["action"], "keep");
    assert_eq!(piece(&payload, "type_note")["action"], "create");
    assert_eq!(piece(&payload, "base")["action"], "create");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("widgets.md")).unwrap(),
        "already here\n"
    );
}

#[test]
fn init_refuses_a_missing_collection_without_parent() {
    let fixture = init_fixture();
    let output = records_command(&fixture)
        .args(["init", "widget", "-f", "json"])
        .output()
        .expect("init missing parent");
    assert_eq!(output.status.code(), Some(2));
    let payload = json(&output);
    let message = payload["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("-p/--parent") && message.contains("widgets.md"),
        "{message}"
    );
    assert!(!fixture.vault.join("widget.md").exists());
    assert!(!fixture.vault.join("widgets.md").exists());
    assert!(!fixture.vault.join("widgets.base").exists());
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
    assert_eq!(
        fs::read_to_string(fixture.vault.join("eat.md")).unwrap(),
        album_before,
        "notes-only update leaves a current album unchanged"
    );
    let record =
        fs::read_to_string(fixture.vault.join("eat/ramen_nagi.md")).unwrap();
    assert!(record.contains("Went back, still great"), "{record}");
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
fn record_writes_config_new_carries_no_album_index() {
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
        .expect("config new");
    assert_success(&created);
    let payload = json(&created);
    assert_eq!(payload["source"], "config");
    assert!(
        payload.get("album_index").is_none(),
        "config kinds plan no album write: {payload}"
    );
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
