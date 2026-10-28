//! Shared PT7/PT8 vectors, exercised through the real completion and capture APIs.
use crate::support::*;
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn command(root: &Path) -> Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", root.join("vault"))
        .env("BOB_DAY_FILE", root.join("vault/2026/20261024.md"))
        .env("BOB_NOW", "2026-10-24 08:00:00")
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("BOB_CONFIG_FILE", root.join("config.yml"));
    command
}

fn fixture(root: &Path, case: &Value) {
    let vault = root.join("vault");
    write_capture_task_settings(&vault);
    let mut config = String::from("plan:\n  max_themes: 30\n");
    if let Some(configured) = case.get("configured") {
        config.push_str("themes:\n");
        match configured {
            Value::Object(map) => {
                for (name, color) in map {
                    let slot = color.as_str().unwrap_or("auto");
                    config.push_str(&format!("  {name}: {slot}\n"));
                }
            }
            Value::Array(entries) => {
                for entry in entries {
                    let name = entry["name"].as_str().unwrap();
                    let slot = entry["color"].as_str().unwrap_or("auto");
                    config.push_str(&format!("  {name}: {slot}\n"));
                }
            }
            _ => {}
        }
    }
    write_file(&root.join("config.yml"), &config);
    write_file(&vault.join("inbox_t.md"), "- [ ] #task Test task ^foo1\n");
    let today = case["today"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    write_file(
        &vault.join("2026/20261024.md"),
        &format!("## Pomodoros\n{today}\n"),
    );
    if let Some(history) = case["history"].as_object() {
        for (date, names) in history {
            let rows = names
                .as_array()
                .unwrap()
                .iter()
                .map(|name| format!("- [x] () — {}\n", name.as_str().unwrap()))
                .collect::<String>();
            write_file(
                &vault.join(format!("2026/{}.md", date.replace('-', ""))),
                &format!("## Pomodoros\n{rows}"),
            );
        }
    }
    let events = case["hidden"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|key| {
            json!({
        "v":1,"ts":"2026-10-24T07:00:00Z","host":"test","kind":"theme.hide",
        "key":key,"name":key.as_str().unwrap().to_uppercase(),"via":"bob-cli"
    }).to_string()+"\n"
        })
        .collect::<String>();
    write_file(&vault.join(".bob/themes/test.jsonl"), &events);
}

fn complete(root: &Path, draft: &str) -> Value {
    let output = command(root)
        .args([
            "capture-complete",
            "-f",
            "json",
            "-c",
            &draft.len().to_string(),
            "--",
            draft,
        ])
        .output()
        .unwrap();
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn theme_menu_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../fixtures/pomodoro_themes/vectors.json"
    ))
    .unwrap();
    for case in vectors["menu"].as_array().unwrap() {
        let temp = TempDir::new("bob-theme-menu");
        fixture(temp.path(), case);
        let journal = temp.path().join("vault/.bob/themes/test.jsonl");
        let before = fs::read(&journal).unwrap();
        let result = complete(temp.path(), case["draft"].as_str().unwrap());
        assert_eq!(result["schema_version"], 1);
        let rows = result["candidates"].as_array().unwrap();
        let expected = case["expected"].as_array().unwrap();
        assert_eq!(rows.len(), expected.len(), "{}: {rows:?}", case["name"]);
        for (row, expected) in rows.iter().zip(expected) {
            for (key, value) in expected.as_object().unwrap() {
                assert_eq!(&row[key], value, "{} {key}: {row}", case["name"]);
            }
            if row["name"].is_string() {
                assert!(!row["themes"].as_array().unwrap().is_empty(), "{row}");
            }
            if row["origin"] == "history" {
                assert_eq!(row["creates_pomodoro"], true);
                assert!(row.get("ref").is_none() && row.get("line").is_none());
                if row.get("last_used").is_some() {
                    assert!(row["last_used"].is_string());
                    assert!(row["days_used"].as_u64().unwrap() > 0);
                } else {
                    assert!(row.get("days_used").is_none());
                }
                assert!(row["plan_themes_after"].is_number());
                assert_eq!(row["plan_themes_cap"], 30);
            }
            if case.get("configured").is_some()
                && row.get("origin").is_none()
                && row["creates_pomodoro"] == true
            {
                assert!(
                    row["themes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|theme| theme["configured"] == false),
                    "{} create row should be unconfigured: {row}",
                    case["name"]
                );
            }
        }
        assert_eq!(fs::read(&journal).unwrap(), before);
    }
}

#[test]
fn theme_literal_vectors_round_trip_through_capture() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../fixtures/pomodoro_themes/vectors.json"
    ))
    .unwrap();
    for case in vectors["literal"].as_array().unwrap() {
        let temp = TempDir::new("bob-theme-literal");
        let mut case = case.clone();
        case["history"] = json!({"2026-10-23":[case["theme"]]});
        fixture(temp.path(), &case);
        let draft = case["draft"].as_str().unwrap();
        let result = complete(temp.path(), draft);
        let row = result["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == case["theme"])
            .expect("theme row");
        assert_eq!(row["replacement"], case["replacement"], "{}", case["name"]);
        let start = result["replacement"]["start"].as_u64().unwrap() as usize;
        let end = result["replacement"]["end"].as_u64().unwrap() as usize;
        let accepted = format!(
            "{}{}{}",
            &draft[..start],
            row["replacement"].as_str().unwrap(),
            &draft[end..]
        );
        let output = command(temp.path())
            .args(["capture", "-d", "--no-clip", "-f", "json", "--", &accepted])
            .output()
            .unwrap();
        assert_success(&output);
        let captured: Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(
            captured["pomodoro_name"], case["theme"],
            "{accepted}: {captured}"
        );
        assert_eq!(
            fs::read_to_string(
                temp.path().join("vault/.bob/themes/test.jsonl")
            )
            .unwrap(),
            ""
        );
    }
}

#[test]
fn theme_recent_shell_description_and_quoted_replacement() {
    let temp = TempDir::new("bob-theme-shell");
    fixture(
        temp.path(),
        &json!({"today":[], "history":{"2026-10-23":["DEEP WORK"]}}),
    );
    let output = command(temp.path())
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "capture",
            "=#deep",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("recent · Oct 23 · 1 day"), "{text}");
    assert!(text.contains("DEEP WORK"), "{text}");
}

#[test]
fn configured_shell_describes_never_used_and_unconfigured_create() {
    let temp = TempDir::new("bob-theme-shell-configured");
    fixture(
        temp.path(),
        &json!({
            "today": [],
            "history": {"2026-10-23": ["BUILD"]},
            "hidden": [],
            "configured": [
                {"name": "BUILD", "color": "yellow"},
                {"name": "WRITE", "color": "violet"}
            ]
        }),
    );
    let never_used = command(temp.path())
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "capture",
            "=#",
        ])
        .output()
        .unwrap();
    assert_success(&never_used);
    let never_text = stdout(&never_used);
    assert!(
        never_text.contains("your theme · never used"),
        "{never_text}"
    );
    assert!(
        never_text.to_ascii_lowercase().contains("write"),
        "{never_text}"
    );

    let create = command(temp.path())
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "capture",
            "=#snack",
        ])
        .output()
        .unwrap();
    assert_success(&create);
    let create_text = stdout(&create);
    assert!(
        create_text.contains("new · not in your themes"),
        "{create_text}"
    );
}

#[test]
fn accepting_hidden_theme_keeps_preferences_and_today_row() {
    let temp = TempDir::new("bob-theme-hidden-accept");
    fixture(
        temp.path(),
        &json!({"today":[], "history":{"2026-10-23":["FIXES"]}, "hidden":["fixes"]}),
    );
    let journal = temp.path().join("vault/.bob/themes/test.jsonl");
    let before = fs::read(&journal).unwrap();
    let draft = "=#fixes";
    let result = complete(temp.path(), draft);
    assert_eq!(result["candidates"][0]["hidden"], true);
    let output = command(temp.path())
        .args(["capture", "--no-clip", "-f", "json", "--", draft])
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(fs::read(&journal).unwrap(), before);
    let after = complete(temp.path(), draft);
    let rows = after["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "FIXES");
    assert!(rows[0].get("origin").is_none());
    assert!(rows[0].get("ref").is_some());
}
