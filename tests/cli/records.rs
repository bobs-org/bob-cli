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
        .env("NO_COLOR", "1");
    command
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
