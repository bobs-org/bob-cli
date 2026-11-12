//! `bob albums list` and `check` contract tests.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn vault_fixture(name: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    (temp, vault)
}

fn albums_command(vault: &Path) -> Command {
    let mut command = bob_command();
    command
        .arg("albums")
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("BOB_NOW", "2026-11-10 12:00:00");
    command
}

fn write_note(vault: &Path, name: &str, contents: &str) {
    write_file(&vault.join(name), contents);
}

fn area_note() -> &'static str {
    "---\ntype: [[area]]\n---\n\n# Area\n"
}

fn project_note(parent: &str, root: bool) -> String {
    format!(
        "---\ntype: [[project]]\nparent: \"[[{parent}]]\"\n{}---\n\n# Project\n",
        if root { "root: true\n" } else { "" }
    )
}

fn album(
    parent: &str,
    title: &str,
    summary: &str,
    noun: &str,
    extra_frontmatter: &str,
    fields: &str,
) -> String {
    let schema = if fields.trim().is_empty() {
        format!("  noun: {noun}\n")
    } else {
        format!("  noun: {noun}\n  fields:\n{fields}")
    };
    format!(
        "---\ntype: [[album]]\nparent: \"[[{parent}]]\"\nsummary: {summary}\n{extra_frontmatter}record:\n{schema}---\n\n# {title}\n"
    )
}

fn restaurant_fields() -> &'static str {
    "    status:\n      required: true\n      values: {to_try: 🔖 To try, liked: 💚 Liked, not_liked: 🚫 Not liked}\n    category: {required: true, suggest: true}\n    location: {required: true, suggest: true}\n"
}

fn record_note(
    album: &str,
    noun: &str,
    title: &str,
    status: &str,
    location: &str,
    created: &str,
) -> String {
    format!(
        "---\nparent: \"[[{album}]]\"\ntype: \"[[{noun}]]\"\ntitle: {title:?}\nstatus: {status}\ncategory: Ramen\nlocation: {location}\ncreated: {created}\n---\n\n# {title}\n"
    )
}

fn four_valid_albums(vault: &Path) {
    write_note(vault, "body.md", area_note());
    write_note(vault, "mind.md", area_note());
    write_note(
        vault,
        "eat.md",
        &album(
            "body",
            "Eat",
            "Places to eat.",
            "restaurant",
            "emoji: 🍽️\nindex_by: location\n",
            restaurant_fields(),
        ),
    );
    write_note(
        vault,
        "movies.md",
        &album(
            "body",
            "Movies",
            "Films to watch.",
            "movie",
            "",
            "    year: {values: number}\n",
        ),
    );
    write_note(
        vault,
        "books.md",
        &album(
            "mind",
            "Books",
            "Reading list.",
            "book",
            "",
            "    author: {}\n",
        ),
    );
    write_note(
        vault,
        "podcasts.md",
        &album(
            "body",
            "Podcasts",
            "Shows to hear.",
            "podcast",
            "",
            "    status: {values: {queued: Queued, done: Done}}\n",
        ),
    );
    write_note(
        vault,
        "eat/ani_ramen_house.md",
        &record_note(
            "eat",
            "restaurant",
            "Ani Ramen House",
            "to_try",
            "Clark",
            "2026-11-01T12:00:00-0500",
        ),
    );
    write_note(
        vault,
        "eat/bubble_tea.md",
        &record_note(
            "eat",
            "restaurant",
            "Bubble Tea",
            "to_try",
            "Cranford",
            "2026-11-08T12:00:00-0500",
        ),
    );
    write_note(
        vault,
        "eat/cafe.md",
        &record_note(
            "eat",
            "restaurant",
            "Cafe",
            "liked",
            "Clark",
            "2026-10-01T12:00:00-0500",
        ),
    );
    write_note(
        vault,
        "eat/unknown.md",
        &record_note(
            "eat",
            "restaurant",
            "Unknown",
            "nope",
            "Clark",
            "2026-09-01T12:00:00-0500",
        ),
    );
}

fn album_by_stem<'a>(payload: &'a Value, stem: &str) -> &'a Value {
    payload["albums"]
        .as_array()
        .expect("albums array")
        .iter()
        .find(|entry| entry["stem"] == stem)
        .unwrap_or_else(|| panic!("missing album {stem}"))
}

fn json(output: &std::process::Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("expected JSON ({error}):\n{}", format_output(output))
    })
}

#[test]
fn albums_list_empty_vault_teaches_create() {
    let (_temp, vault) = vault_fixture("albums-empty");
    let output = albums_command(&vault)
        .arg("list")
        .output()
        .expect("run albums list");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("No albums."), "text:\n{text}");
    assert!(
        text.contains("bob albums create"),
        "empty list should teach create:\n{text}"
    );
}

#[test]
fn albums_list_and_check_json_shape() {
    let (_temp, vault) = vault_fixture("albums-json");
    four_valid_albums(&vault);
    let output = albums_command(&vault)
        .arg("list")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run list json");
    assert!(output.status.success(), "{}", format_output(&output));
    assert_eq!(output.status.code(), Some(0));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse json");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert!(payload.get("index").is_none());
    assert_eq!(
        payload["catalogs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|catalog| catalog["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["body.md", "mind.md"]
    );
    assert_eq!(payload["catalogs"][0]["state"], "missing");
    assert_eq!(payload["catalogs"][0]["albums"], 3);
    assert_eq!(
        payload["catalogs"][0]["issues"][0]["code"],
        "album_catalog_stale"
    );
    let albums = payload["albums"].as_array().unwrap();
    assert_eq!(albums.len(), 4);
    let paths: Vec<&str> = albums
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["books.md", "eat.md", "movies.md", "podcasts.md"]);

    let eat = album_by_stem(&payload, "eat");
    assert_eq!(eat["title"], "Eat");
    assert_eq!(eat["emoji"], "🍽️");
    assert_eq!(eat["noun"], "restaurant");
    assert_eq!(eat["parent"], "body");
    assert_eq!(eat["parent_kind"], "area");
    assert_eq!(eat["owner"], "body");
    assert_eq!(eat["root"], "body");
    assert_eq!(eat["group_field"], "status");
    assert_eq!(eat["index_by"], "location");
    assert_eq!(
        eat["index_facets"],
        serde_json::json!(["category", "location"])
    );
    assert_eq!(eat["records"], 4);
    assert_eq!(eat["needs_status"], 1);
    assert_eq!(eat["last_record"], "2026-11-08");
    assert_eq!(eat["index"]["state"], "missing");
    let groups = eat["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["value"], "to_try");
    assert_eq!(groups[0]["count"], 2);
    assert_eq!(
        groups[0]["peek"],
        serde_json::json!(["Ani Ramen House", "Bubble Tea"])
    );
    assert_eq!(groups[1]["value"], "liked");
    assert_eq!(groups[1]["count"], 1);

    assert_eq!(payload["counts"]["albums"], 4);
    assert_eq!(payload["counts"]["records"], 4);
    assert_eq!(payload["counts"]["roots"], 2);
    assert_eq!(payload["counts"]["errors"], 0);
    assert_eq!(payload["counts"]["warnings"], 6);

    let output = albums_command(&vault)
        .arg("check")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run check json");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse check");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["counts"]["errors"], 0);
    assert_eq!(payload["counts"]["warnings"], 6);
}

#[test]
fn album_schema_json_reports_attributes_usage_lines_and_fingerprint() {
    let (_temp, vault) = vault_fixture("albums-schema-json");
    write_note(&vault, "body.md", area_note());
    write_note(&vault, "config.yml", "records: {}\n");
    let album = concat!(
        "---\n",
        "type: [[album]]\n",
        "parent: \"[[body]]\"\n",
        "summary: Places to eat.\n",
        "record:\n",
        "  noun: restaurant\n",
        "  label: Restaurant\n",
        "  plural: restaurants\n",
        "  dated_notes: true\n",
        "  fields:\n",
        "    status:\n",
        "      required: true\n",
        "      values:\n",
        "        to_try: To try\n",
        "        liked: Liked\n",
        "    category:\n",
        "      suggest: true\n",
        "      default: Ramen\n",
        "    rating:\n",
        "      values: number\n",
        "      min: 0\n",
        "      max: 5\n",
        "      default: 0\n",
        "  legacy:\n",
        "    - lid\n",
        "    - source_*\n",
        "---\n",
        "\n# Restaurants\n",
    );
    write_note(&vault, "eat.md", album);
    write_note(
        &vault,
        "eat/good.md",
        "---\nstatus: to_try\ncategory: Ramen\nrating: 0\n---\n# Good\n",
    );
    write_note(
        &vault,
        "eat/invalid.md",
        "---\nstatus: retired\ncategory: \"\"\nrating: 9\n---\n# Invalid\n",
    );
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken.\nrecord: []\n---\n# Broken\n",
    );

    let output = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("run albums list JSON");
    let payload = json(&output);
    let eat = album_by_stem(&payload, "eat");
    assert_eq!(eat["dated_notes"], true);
    assert_eq!(eat["legacy"], serde_json::json!(["lid", "source_*"]));
    let fingerprint = eat["schema_fingerprint"].as_str().unwrap();
    assert!(fingerprint.starts_with("sha256:"));
    assert_eq!(fingerprint.len(), "sha256:".len() + 64);

    let fields = eat["fields"].as_array().unwrap();
    let status = &fields[0];
    let expected_status_line = album
        .lines()
        .position(|line| line == "    status:")
        .unwrap()
        + 1;
    assert_eq!(status["line"], expected_status_line);
    assert_eq!(status["filled"], 2);
    assert_eq!(status["suggest"], false);
    assert!(status.get("default").is_none());
    assert!(status.get("min").is_none());
    assert!(status.get("max").is_none());
    assert_eq!(status["choices"][0]["count"], 1);
    assert_eq!(status["choices"][1]["count"], 0);

    let category = &fields[1];
    assert_eq!(category["filled"], 1);
    assert_eq!(category["suggest"], true);
    assert_eq!(category["default"], "Ramen");
    let rating = &fields[2];
    assert_eq!(rating["filled"], 2);
    assert_eq!(rating["default"].as_f64(), Some(0.0));
    assert_eq!(rating["min"].as_f64(), Some(0.0));
    assert_eq!(rating["max"].as_f64(), Some(5.0));

    let broken = album_by_stem(&payload, "broken");
    assert_eq!(broken["dated_notes"], Value::Null);
    assert_eq!(broken["legacy"], Value::Null);
    assert_eq!(broken["schema_fingerprint"], Value::Null);
    assert_eq!(broken["fields"], serde_json::json!([]));

    let show = bob_command()
        .args(["records", "show", "eat", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", vault.join("config.yml"))
        .env("NO_COLOR", "1")
        .env("BOB_NOW", "2026-11-10 12:00:00")
        .output()
        .expect("run records show JSON");
    assert!(show.status.success(), "{}", format_output(&show));
    let shown = json(&show);
    assert_eq!(shown["fields"][0]["choices"], status["choices"]);
    assert_eq!(shown["fields"][0]["line"], status["line"]);
    assert_eq!(shown["fields"][0]["filled"], status["filled"]);
    assert_eq!(shown["fields"][2]["default"], rating["default"]);
}

#[test]
fn albums_bare_defaults_to_list() {
    let (_temp, vault) = vault_fixture("albums-bare");
    four_valid_albums(&vault);
    let bare = albums_command(&vault).output().expect("bare");
    let explicit = albums_command(&vault).arg("list").output().expect("list");
    assert!(bare.status.success(), "{}", format_output(&bare));
    assert!(explicit.status.success(), "{}", format_output(&explicit));
    assert_eq!(stdout(&bare), stdout(&explicit));
}

#[test]
fn albums_human_list_groups_by_root_and_shows_freshness() {
    let (_temp, vault) = vault_fixture("albums-human");
    four_valid_albums(&vault);
    let output = albums_command(&vault)
        .arg("list")
        .output()
        .expect("human list");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert_text_order(
        &text,
        &["body", "Eat", "Movies", "Podcasts", "mind", "Books"],
    );
    assert!(text.contains("eat/"), "text:\n{text}");
    assert!(text.contains("🔖 2 to try"), "text:\n{text}");
    assert!(text.contains("💚 1"), "text:\n{text}");
    assert!(text.contains("⚠️ 1"), "text:\n{text}");
    assert!(text.contains("2d"), "freshness uses BOB_NOW:\n{text}");
    assert!(text.contains("~1"), "missing index warns:\n{text}");
    assert!(
        text.contains("catalog body.md — missing (3 albums)"),
        "text:\n{text}"
    );
    assert!(text.contains("bob albums sync"), "text:\n{text}");
    assert!(!text.contains("No albums."), "text:\n{text}");
}

#[test]
fn albums_root_filter_accepts_md_suffix() {
    let (_temp, vault) = vault_fixture("albums-filter");
    four_valid_albums(&vault);
    let output = albums_command(&vault)
        .args(["list", "-r", "body.md", "-f", "json"])
        .output()
        .expect("filtered");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    let stems: Vec<&str> = payload["albums"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["stem"].as_str().unwrap())
        .collect();
    assert_eq!(stems, ["eat", "movies", "podcasts"]);
    assert_eq!(payload["counts"]["albums"], 3);
    assert_eq!(payload["counts"]["roots"], 1);
    assert_eq!(payload["catalogs"].as_array().unwrap().len(), 1);
    assert_eq!(payload["catalogs"][0]["path"], "body.md");
    assert_eq!(payload["counts"]["warnings"], 4);
}

#[test]
fn album_catalogs_follow_project_roots_and_filter_together() {
    let (_temp, vault) = vault_fixture("albums-catalog-roots");
    write_note(&vault, "body.md", area_note());
    write_note(&vault, "projects/outer.md", &project_note("body", false));
    write_note(
        &vault,
        "projects/middle.md",
        &project_note("projects/outer", false),
    );
    write_note(
        &vault,
        "projects/boundary.md",
        &project_note("projects/middle", true),
    );
    write_note(
        &vault,
        "projects/leaf.md",
        &project_note("projects/boundary", false),
    );
    write_note(
        &vault,
        "body_album.md",
        &album(
            "body",
            "Body Album",
            "Body collection.",
            "body_item",
            "",
            "",
        ),
    );
    write_note(
        &vault,
        "middle_album.md",
        &album(
            "projects/middle",
            "Middle Album",
            "Middle collection.",
            "middle_item",
            "",
            "",
        ),
    );
    write_note(
        &vault,
        "boundary_album.md",
        &album(
            "projects/boundary",
            "Boundary Album",
            "Boundary collection.",
            "boundary_item",
            "",
            "",
        ),
    );
    write_note(
        &vault,
        "leaf_album.md",
        &album(
            "projects/leaf",
            "Leaf Album",
            "Leaf collection.",
            "leaf_item",
            "",
            "",
        ),
    );

    let output = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("list albums and catalogs");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload = json(&output);
    let catalogs = payload["catalogs"].as_array().unwrap();
    let rows = catalogs
        .iter()
        .map(|catalog| {
            (
                catalog["path"].as_str().unwrap(),
                catalog["root"].as_str().unwrap(),
                catalog["albums"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            ("body.md", "body", 1),
            ("projects/boundary.md", "boundary", 2),
            ("projects/leaf.md", "boundary", 1),
            ("projects/middle.md", "outer", 1),
            ("projects/outer.md", "outer", 1),
        ]
    );

    let filtered = albums_command(&vault)
        .args(["list", "-r", "outer", "-f", "json"])
        .output()
        .expect("root-filter list");
    assert!(filtered.status.success(), "{}", format_output(&filtered));
    let filtered = json(&filtered);
    assert_eq!(filtered["counts"]["albums"], 1);
    assert_eq!(filtered["counts"]["roots"], 1);
    assert_eq!(
        filtered["catalogs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|catalog| catalog["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["projects/middle.md", "projects/outer.md"]
    );
}

#[test]
fn albums_list_reports_catalog_only_damage_and_stale_empty_hosts() {
    let (_temp, vault) = vault_fixture("albums-catalog-damage");
    write_note(
        &vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums\n> - partial\n",
    );

    let listed = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("list damaged catalog");
    assert_eq!(listed.status.code(), Some(0), "{}", format_output(&listed));
    let payload = json(&listed);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["counts"]["albums"], 0);
    assert_eq!(payload["counts"]["errors"], 1);
    assert_eq!(payload["catalogs"][0]["state"], "damaged");
    assert_eq!(
        payload["catalogs"][0]["issues"][0]["code"],
        "album_catalog_damaged"
    );

    let checked = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check damaged catalog");
    assert_eq!(
        checked.status.code(),
        Some(1),
        "{}",
        format_output(&checked)
    );

    let (_stale_temp, stale_vault) =
        vault_fixture("albums-catalog-empty-stale");
    write_note(
        &stale_vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums]\n>\n> - old row\n",
    );
    let list = albums_command(&stale_vault)
        .arg("list")
        .output()
        .expect("list empty stale catalog");
    assert!(list.status.success(), "{}", format_output(&list));
    let text = stdout(&list);
    assert!(text.contains("No albums."), "text:\n{text}");
    assert!(
        text.contains("catalog body.md — stale (0 albums)"),
        "text:\n{text}"
    );
    assert!(text.contains("bob albums sync"), "text:\n{text}");
}

#[test]
fn structurally_owned_schema_invalid_album_remains_in_catalog() {
    let (_temp, vault) = vault_fixture("albums-catalog-invalid-schema");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken.\nrecord:\n  fields: {}\n---\n\n# Broken\n",
    );
    write_note(
        &vault,
        "notes.md",
        "---\ntype: [[notebook]]\nparent: \"[[body]]\"\nsummary: Notes.\n---\n\n# Notes\n",
    );
    write_note(
        &vault,
        "orphan.md",
        &album("notes", "Orphan", "No area owner.", "orphan_record", "", ""),
    );
    let listed = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("list invalid schema");
    assert_eq!(listed.status.code(), Some(0), "{}", format_output(&listed));
    let payload = json(&listed);
    assert_eq!(payload["catalogs"][0]["path"], "body.md");
    assert_eq!(payload["catalogs"][0]["albums"], 1);
    assert_eq!(payload["counts"]["errors"].as_u64().unwrap(), 4);
    let issue_codes = payload["albums"][0]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["code"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(issue_codes.contains(&"album_schema_invalid"));
    assert!(issue_codes.contains(&"album_index_damaged"));
    let orphan_codes = album_by_stem(&payload, "orphan")["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["code"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(orphan_codes.contains(&"album_parent_kind"));
    assert!(orphan_codes.contains(&"album_index_damaged"));

    let checked = albums_command(&vault)
        .arg("check")
        .output()
        .expect("human check invalid album and catalog");
    assert_eq!(
        checked.status.code(),
        Some(1),
        "{}",
        format_output(&checked)
    );
    let text = stdout(&checked);
    assert!(
        text.find("broken.md — Broken").unwrap()
            < text.find("body.md — catalog").unwrap(),
        "album findings should precede host catalog findings:\n{text}"
    );
}

#[test]
fn albums_bob_dir_flag_selects_the_vault() {
    let (_temp, vault) = vault_fixture("albums-bob-dir");
    four_valid_albums(&vault);
    let output = bob_command()
        .args(["albums", "list", "-b"])
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("NO_COLOR", "1")
        .output()
        .expect("list -b");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["counts"]["albums"], 4);
}

#[test]
fn albums_check_reports_finding_codes() {
    let (_temp, vault) = vault_fixture("albums-findings");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "notes.md",
        "---\nparent: \"[[body]]\"\ntype: \"[[notebook]]\"\nsummary: Notes.\n---\n\n# Notes\n",
    );
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n\n# Broken\n",
    );
    write_note(
        &vault,
        "cards.md",
        &album("body", "Cards", "Card games.", "card", "", "    suit: {}\n"),
    );
    write_note(
        &vault,
        "deck.md",
        &album(
            "body",
            "Deck",
            "Another card album.",
            "card",
            "",
            "    rank: {}\n",
        ),
    );
    write_note(
        &vault,
        "years.md",
        &album(
            "body",
            "Years",
            "Numeric year.",
            "yearbook",
            "",
            "    year: {values: number}\n",
        ),
    );
    write_note(
        &vault,
        "vintage.md",
        &album(
            "body",
            "Vintage",
            "Text year.",
            "vintage",
            "",
            "    year: {}\n",
        ),
    );
    write_note(
        &vault,
        "under_notes.md",
        &album(
            "notes",
            "Wrong parent",
            "Album under a notebook.",
            "wrong_kind",
            "",
            "",
        ),
    );
    write_note(
        &vault,
        "plain.md",
        "---\ntype: [[album]]\nparent: [[body]]\nsummary: Unquoted parent.\nrecord: {noun: plain}\n---\n\n# Plain\n",
    );
    fs::create_dir_all(vault.join("sub")).expect("sub");
    write_note(
        &vault,
        "sub/nested.md",
        &album("body", "Nested", "Not at the vault root.", "nested", "", ""),
    );

    let output = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check json");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(output.status.code(), Some(1));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["schema_version"], 1);
    let codes = |stem: &str| -> Vec<String> {
        album_by_stem(&payload, stem)["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["code"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(codes("broken").contains(&"album_schema_invalid".to_owned()));
    assert!(codes("cards").contains(&"album_noun_duplicate".to_owned()));
    assert!(codes("deck").contains(&"album_noun_duplicate".to_owned()));
    assert!(codes("years").contains(&"album_property_conflict".to_owned()));
    assert!(codes("vintage").contains(&"album_property_conflict".to_owned()));
    assert!(codes("under_notes").contains(&"album_parent_kind".to_owned()));
    assert!(codes("plain").contains(&"album_parent_unquoted".to_owned()));
    assert!(codes("nested").contains(&"album_not_at_root".to_owned()));
}

#[test]
fn albums_check_warning_only_exits_zero() {
    let (_temp, vault) = vault_fixture("albums-warning");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "long.md",
        &album("body", "Long", &"x".repeat(150), "longform", "", ""),
    );
    let output = albums_command(&vault).arg("check").output().expect("check");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("1 albums"), "text:\n{text}");
    assert!(text.contains("warnings"), "text:\n{text}");
}

#[test]
fn albums_list_ignores_findings_for_exit() {
    let (_temp, vault) = vault_fixture("albums-list-errors");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n\n# Broken\n",
    );
    let output = albums_command(&vault).arg("list").output().expect("list");
    assert!(output.status.success(), "{}", format_output(&output));
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn albums_io_failure_is_not_empty_success() {
    let temp = TempDir::new("albums-io");
    let missing = temp.path().join("no-such-vault");
    let human = bob_command()
        .args(["albums", "list", "-b"])
        .arg(&missing)
        .env("NO_COLOR", "1")
        .output()
        .expect("list missing vault");
    assert_eq!(human.status.code(), Some(1), "{}", format_output(&human));
    assert!(
        !stdout(&human).contains("No albums."),
        "I/O failure must not look like an empty vault:\n{}",
        format_output(&human)
    );
    assert!(
        stderr(&human).contains("bob albums:"),
        "expected I/O error on stderr:\n{}",
        stderr(&human)
    );

    let json = bob_command()
        .args(["albums", "list", "-b"])
        .arg(&missing)
        .args(["-f", "json"])
        .env("NO_COLOR", "1")
        .output()
        .expect("list json missing vault");
    assert_eq!(json.status.code(), Some(1), "{}", format_output(&json));
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse json");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], false);
}

#[test]
fn albums_vault_bytes_unchanged_by_list_and_check() {
    let (_temp, vault) = vault_fixture("albums-clean");
    four_valid_albums(&vault);
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n\n# Broken\n",
    );
    write_note(&vault, "broken/good.md", "---\ntitle: Good\n---\n# Good\n");
    write_note(&vault, "broken/bad.md", "---\n[not: mapping\n---\n# Bad\n");
    init_vault_git(&vault);
    let snapshot = vault_state(&vault);
    for args in [
        &["list"][..],
        &["check"][..],
        &["list", "-f", "json"][..],
        &["check", "-f", "json"][..],
    ] {
        let mut command = albums_command(&vault);
        command.args(args);
        let output = command.output().expect("run");
        let code = output.status.code();
        assert!(
            code == Some(0) || code == Some(1),
            "{}",
            format_output(&output)
        );
    }
    assert_eq!(vault_state(&vault), snapshot);
}

#[test]
fn albums_list_places_invalid_schemas_in_the_unowned_group() {
    let (_temp, vault) = vault_fixture("albums-invalid-group");
    four_valid_albums(&vault);
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n\n# Broken\n",
    );
    let output = albums_command(&vault).arg("list").output().expect("list");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert_text_order(&text, &["body", "Eat", "invalid / unowned", "Broken"]);
    let body_block = text.split("invalid / unowned").next().unwrap();
    assert!(
        body_block.contains("Eat"),
        "healthy stay under root:\n{text}"
    );
    assert!(
        !body_block.contains("Broken"),
        "invalid schema leaves the healthy root group:\n{text}"
    );
}

#[test]
fn albums_broken_album_members_still_count() {
    let (_temp, vault) = vault_fixture("albums-broken-members");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n\n# Broken\n",
    );
    write_note(&vault, "broken/good.md", "---\ntitle: Good\n---\n# Good\n");
    write_note(&vault, "broken/bad.md", "---\n[not: mapping\n---\n# Bad\n");
    let output = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("list json");
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    let broken = album_by_stem(&payload, "broken");
    assert_eq!(broken["records"], 2);
    assert_eq!(broken["root"], "body");
    assert_eq!(broken["index"]["state"], "damaged");
    assert!(broken["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "album_schema_invalid"));
}

#[test]
fn albums_child_reparent_keeps_the_child_path() {
    let (_temp, vault) = vault_fixture("albums-child-path");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "cards.md",
        &album("body", "Cards", "Card games.", "card", "", ""),
    );
    write_note(
        &vault,
        "stray.md",
        "---\ntype: [[record]]\nparent: \"[[cards]]\"\n---\n# Stray\n",
    );
    let json = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check json");
    assert_eq!(json.status.code(), Some(0), "{}", format_output(&json));
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse");
    let issue = album_by_stem(&payload, "cards")["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "album_child_reparent")
        .expect("child reparent");
    assert_eq!(issue["path"], "stray.md");
    assert_eq!(issue["severity"], "info");
    assert!(issue["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "cards/"));

    let human = albums_command(&vault).arg("check").output().expect("check");
    let text = stdout(&human);
    assert!(text.contains("stray.md"), "human names the child:\n{text}");
    assert!(
        text.contains("cards/"),
        "human suggests the owner folder:\n{text}"
    );
}

#[test]
fn albums_schema_suggestion_and_healthy_neighbour_are_isolated() {
    let (_temp, vault) = vault_fixture("albums-suggestion");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "eat.md",
        &album(
            "body",
            "Eat",
            "Places to eat.",
            "restaurant",
            "",
            restaurant_fields(),
        ),
    );
    write_note(
        &vault,
        "typo.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Typo option\nrecord:\n  noun: sample\n  fields:\n    location:\n      requred: true\n---\n\n# Typo\n",
    );
    let json = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check json");
    assert_eq!(json.status.code(), Some(1), "{}", format_output(&json));
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    let eat = album_by_stem(&payload, "eat");
    assert!(
        eat["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue["code"] == "album_index_stale"),
        "healthy neighbour keeps only index warnings:\n{eat}"
    );
    let typo = album_by_stem(&payload, "typo");
    let issue = typo["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "album_schema_invalid")
        .expect("schema issue");
    assert_eq!(issue["path"], "typo.md");
    assert_eq!(issue["key_path"], "record.fields.location.requred");
    assert!(issue["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "required"));

    let human = albums_command(&vault).arg("check").output().expect("check");
    let text = stdout(&human);
    assert!(
        text.contains("requred"),
        "human keeps the key path:\n{text}"
    );
    assert!(
        text.contains("required"),
        "human prints the suggestion:\n{text}"
    );
}

#[test]
fn albums_non_directory_folder_is_runtime_failure() {
    let (_temp, vault) = vault_fixture("albums-not-dir");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "eat.md",
        &album(
            "body",
            "Eat",
            "Places to eat.",
            "restaurant",
            "",
            restaurant_fields(),
        ),
    );
    write_file(&vault.join("eat"), "this is a file, not a folder\n");
    let json = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("list json");
    assert_eq!(json.status.code(), Some(1), "{}", format_output(&json));
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    assert!(
        stderr(&json).contains("bob albums:"),
        "folder I/O on stderr:\n{}",
        stderr(&json)
    );

    let human = albums_command(&vault).arg("list").output().expect("list");
    assert_eq!(human.status.code(), Some(1), "{}", format_output(&human));
    assert!(
        !stdout(&human).contains("No albums."),
        "runtime failure is not an empty vault:\n{}",
        format_output(&human)
    );
}

#[cfg(unix)]
#[test]
fn albums_folder_symlink_escape_is_runtime_failure() {
    let temp = TempDir::new("albums-symlink");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).expect("outside dir");
    write_note(&vault, "body.md", area_note());
    write_note(
        &vault,
        "eat.md",
        &album(
            "body",
            "Eat",
            "Places to eat.",
            "restaurant",
            "",
            restaurant_fields(),
        ),
    );
    std::os::unix::fs::symlink(&outside, vault.join("eat")).expect("symlink");
    let json = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check json");
    assert_eq!(json.status.code(), Some(1), "{}", format_output(&json));
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
}

#[test]
fn albums_help_lists_options_alphabetically() {
    let list = bob_command()
        .args(["albums", "list", "--help"])
        .output()
        .expect("list help");
    assert_success(&list);
    let help = stdout(&list);
    assert!(help.contains("read-only"), "help:\n{help}");
    assert_text_order(
        &help,
        &["-b, --bob-dir", "-f, --format", "-h, --help", "-r, --root"],
    );

    let check = bob_command()
        .args(["albums", "check", "--help"])
        .output()
        .expect("check help");
    assert_success(&check);
    let help = stdout(&check);
    assert_text_order(&help, &["-b, --bob-dir", "-f, --format", "-h, --help"]);
    assert!(
        !help.contains("--root"),
        "check has no root filter:\n{help}"
    );
}

#[test]
fn albums_completion_offers_members_and_roots() {
    let (_temp, vault) = vault_fixture("albums-complete");
    four_valid_albums(&vault);

    let members = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete albums");
    assert_success(&members);
    let members_out = stdout(&members);
    let names: Vec<&str> = members_out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(names.contains(&"check"), "members: {names:?}");
    assert!(names.contains(&"list"), "members: {names:?}");

    let roots = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "list",
            "-r",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete roots");
    assert_success(&roots);
    let roots_out = stdout(&roots);
    let names: Vec<&str> = roots_out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(names.contains(&"body"), "roots: {names:?}\n{roots_out}");
    assert!(names.contains(&"mind"), "roots: {names:?}\n{roots_out}");
}

fn eat_records_index() -> &'static str {
    "\
## Records

> [!records] 4 restaurants · 🔖 2 to try · 💚 1 liked · ⚠️ 1 need a status
>
> - **🔖 To try** · 2
>   - [[eat/ani_ramen_house|Ani Ramen House]] — Ramen · Clark
>   - [[eat/bubble_tea|Bubble Tea]] — Ramen · Cranford
> - **💚 Liked** · 1
>   - [[eat/cafe|Cafe]] — Ramen · Clark
> - **⚠️ Needs a status** · 1
>   - [[eat/unknown|Unknown]] — Ramen · Clark
"
}

fn write_eat_with_index(vault: &Path, index: &str) {
    four_valid_albums(vault);
    let mut contents = album(
        "body",
        "Eat",
        "Places to eat.",
        "restaurant",
        "emoji: 🍽️\nindex_by: location\n",
        restaurant_fields(),
    );
    contents.push('\n');
    contents.push_str(index);
    write_note(vault, "eat.md", &contents);
}

#[test]
fn albums_index_states_cover_current_missing_stale_and_damaged() {
    let (_temp, vault) = vault_fixture("albums-index-states");
    write_eat_with_index(&vault, eat_records_index());
    let current = albums_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("current json");
    assert_eq!(
        current.status.code(),
        Some(0),
        "{}",
        format_output(&current)
    );
    let payload: Value =
        serde_json::from_str(stdout(&current).trim()).expect("parse");
    let eat = album_by_stem(&payload, "eat");
    assert_eq!(eat["index"]["state"], "current");
    assert!(
        !eat["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "album_index_stale"
                || issue["code"] == "album_index_damaged"),
        "current index is quiet:\n{eat}"
    );
    assert_eq!(
        album_by_stem(&payload, "movies")["index"]["state"],
        "missing"
    );

    write_eat_with_index(
        &vault,
        "## Records\n\n> [!records] hand edited\n>\n> - stale\n",
    );
    let stale = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("stale json");
    assert_eq!(stale.status.code(), Some(0), "{}", format_output(&stale));
    let payload: Value =
        serde_json::from_str(stdout(&stale).trim()).expect("parse");
    assert_eq!(payload["ok"], true);
    let eat = album_by_stem(&payload, "eat");
    assert_eq!(eat["index"]["state"], "stale");
    let issue = eat["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "album_index_stale")
        .expect("stale warning");
    assert_eq!(issue["severity"], "warning");
    assert!(issue["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "bob albums sync"));

    let human = albums_command(&vault).arg("check").output().expect("check");
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    let text = stdout(&human);
    assert!(text.contains("album_index_stale"), "human:\n{text}");
    assert!(text.contains("bob albums sync"), "human:\n{text}");

    fs::create_dir_all(vault.join("eat/extra.md")).expect("non-file occupant");
    let damaged = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("damaged json");
    assert_eq!(
        damaged.status.code(),
        Some(1),
        "{}",
        format_output(&damaged)
    );
    let payload: Value =
        serde_json::from_str(stdout(&damaged).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    let eat = album_by_stem(&payload, "eat");
    assert_eq!(eat["index"]["state"], "damaged");
    assert_eq!(eat["records"], 4);
    assert!(eat["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "album_index_damaged"));
}

#[test]
fn albums_index_warning_only_check_exits_zero() {
    let (_temp, vault) = vault_fixture("albums-index-warning");
    four_valid_albums(&vault);
    let output = albums_command(&vault).arg("check").output().expect("check");
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let json = albums_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check json");
    let payload: Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["counts"]["errors"], 0);
    assert!(payload["counts"]["warnings"].as_u64().unwrap() >= 4);
}

fn init_vault_git(vault: &Path) {
    git_in(vault, ["init", "-q"]);
    git_in(vault, ["symbolic-ref", "HEAD", "refs/heads/master"]);
    configure_test_git_identity(vault);
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", "album fixture"]);
}

fn vault_state(root: &Path) -> VaultState {
    VaultState {
        paths: collect_paths(root),
        files: snapshot_tree(root),
        head: stdout(&git_in(root, ["rev-parse", "HEAD"])),
        index: stdout(&git_in(root, ["diff-index", "--cached", "HEAD"])),
        worktree: stdout(&git_in(root, ["status", "--porcelain=v1"])),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct VaultState {
    paths: Vec<PathBuf>,
    files: Vec<(PathBuf, String)>,
    head: String,
    index: String,
    worktree: String,
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    files.sort();
    files
}

fn collect_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_path_entries(root, root, &mut paths);
    paths.sort();
    paths
}

fn collect_path_entries(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) {
    for entry in fs::read_dir(directory).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if relative.starts_with(".git") {
            continue;
        }
        paths.push(relative);
        if path.is_dir() {
            collect_path_entries(root, &path, paths);
        }
    }
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(PathBuf, String)>,
) {
    for entry in fs::read_dir(directory).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if relative.starts_with(".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            files.push((
                relative,
                fs::read_to_string(&path).expect("read file"),
            ));
        }
    }
}
