//! Completion editor, cursor and empty-success paths.

use crate::support::*;
use std::fs;

#[test]
fn capture_complete_pomodoro_name_json_lists_named_and_nameable_rows() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-name");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "\t- [[dev#^focus]]\n",
            "- [ ] () — BUGS\n",
            "- [ ] () — MEMORY\n",
            "- [ ] ()\n",
            "- [x] () — DONE\n",
        ),
    );

    let draft = "x @dev:some-id#";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run Pomodoro-name completion");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-complete stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["context"], "pomodoro_name");
    assert_eq!(
        json["replacement"],
        serde_json::json!({
            "start": draft.find('#').expect("hash") + 1,
            "end": draft.len()
        })
    );
    let candidates = json["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 3, "{json}");

    assert_eq!(candidates[0]["replacement"], "memory");
    assert_eq!(candidates[0]["name"], "MEMORY");
    assert_eq!(candidates[0]["requires_name"], false);
    assert!(candidates[0].get("creates_pomodoro").is_none());
    assert_eq!(candidates[0]["line"], 2);
    assert_eq!(candidates[0]["state"], "open");
    assert_eq!(candidates[0]["status_symbol"], " ");
    assert_eq!(candidates[0]["time_range"], "0900-0930");
    assert_eq!(candidates[0]["placeholder"], false);
    assert_eq!(candidates[0]["is_current"], true);
    assert_eq!(candidates[0]["child_count"], 1);
    assert_eq!(candidates[0]["match_count"], 2);
    assert!(candidates[0]["ref"].as_str().expect("ref").contains(':'));

    assert_eq!(candidates[1]["replacement"], "bugs");
    assert_eq!(candidates[1]["name"], "BUGS");
    assert_eq!(candidates[1]["match_count"], 1);

    assert_eq!(candidates[2]["replacement"], "");
    assert!(candidates[2]["name"].is_null());
    assert_eq!(candidates[2]["requires_name"], true);
    assert_eq!(candidates[2]["placeholder"], true);
    assert_eq!(candidates[2]["match_count"], 1);
    assert!(candidates[2].get("creates_pomodoro").is_none());
    assert!(json.get("warnings").is_none(), "{json}");
}

#[test]
fn capture_complete_pomodoro_name_json_offers_a_create_row() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-create");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] () — NETWORK\n",
            "- [ ] ()\n",
            "- [x] () — BUGS\n",
        ),
    );

    let complete = |draft: &str| {
        bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(&vault)
            .arg("-c")
            .arg(draft.len().to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .env("BOB_DAY_FILE", &day_file)
            .output()
            .expect("run Pomodoro-name completion")
    };

    let novel = complete("x @dev:some-id#future");
    assert_success(&novel);
    let novel_json: serde_json::Value =
        serde_json::from_str(stdout(&novel).trim()).expect("json");
    assert_eq!(novel_json["schema_version"], 1);
    assert_eq!(novel_json["context"], "pomodoro_name");
    let novel_rows = novel_json["candidates"].as_array().expect("candidates");
    assert_eq!(novel_rows[0]["replacement"], "future");
    assert_eq!(novel_rows[0]["name"], "FUTURE");
    assert_eq!(novel_rows[0]["creates_pomodoro"], true);
    assert_eq!(novel_rows[0]["requires_name"], false);
    assert_eq!(novel_rows[0]["placeholder"], true);
    assert!(novel_rows[0].get("ref").is_none(), "{novel_json}");
    assert!(novel_rows[0].get("line").is_none(), "{novel_json}");
    assert!(
        novel_rows.iter().any(|row| row["requires_name"] == true),
        "{novel_json}"
    );

    let completed_only = complete("x @dev:some-id#bugs");
    assert_success(&completed_only);
    let completed_json: serde_json::Value =
        serde_json::from_str(stdout(&completed_only).trim()).expect("json");
    assert_eq!(completed_json["candidates"][0]["creates_pomodoro"], true);
    assert_eq!(completed_json["candidates"][0]["replacement"], "bugs");
    assert_eq!(completed_json["candidates"][0]["name"], "BUGS");

    let substring = complete("x @dev:some-id#work");
    assert_success(&substring);
    let substring_json: serde_json::Value =
        serde_json::from_str(stdout(&substring).trim()).expect("json");
    assert_eq!(substring_json["candidates"][0]["creates_pomodoro"], true);
    assert_eq!(substring_json["candidates"][0]["replacement"], "work");
    assert_eq!(substring_json["candidates"][1]["replacement"], "network");
    assert!(substring_json["candidates"][1]
        .get("creates_pomodoro")
        .is_none());
    assert_eq!(substring_json["candidates"][2]["requires_name"], true);

    for query in ["memory", "mem"] {
        let draft = format!("x @dev:some-id#{query}");
        let matched = complete(&draft);
        assert_success(&matched);
        let matched_json: serde_json::Value =
            serde_json::from_str(stdout(&matched).trim()).expect("json");
        assert_eq!(matched_json["candidates"][0]["replacement"], "memory");
        assert!(
            matched_json["candidates"]
                .as_array()
                .expect("candidates")
                .iter()
                .all(|row| row.get("creates_pomodoro").is_none()),
            "{query}: {matched_json}"
        );
    }

    let empty = complete("x @dev:some-id#");
    assert_success(&empty);
    let empty_json: serde_json::Value =
        serde_json::from_str(stdout(&empty).trim()).expect("json");
    assert!(
        empty_json["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|row| row.get("creates_pomodoro").is_none()),
        "{empty_json}"
    );

    let invalid = complete("x @dev:some-id#bad_id");
    assert_success(&invalid);
    let invalid_json: serde_json::Value =
        serde_json::from_str(stdout(&invalid).trim()).expect("json");
    assert!(
        invalid_json["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|row| row.get("creates_pomodoro").is_none()),
        "{invalid_json}"
    );

    let later = "Plan café @dev\n\nFix startup @dev:some-id#future";
    let later_output = complete(later);
    assert_success(&later_output);
    let later_json: serde_json::Value =
        serde_json::from_str(stdout(&later_output).trim()).expect("json");
    let hash = later.rfind('#').expect("hash");
    assert_eq!(
        later_json["replacement"],
        serde_json::json!({
            "start": hash + 1,
            "end": later.len()
        })
    );
    assert_eq!(later_json["candidates"][0]["creates_pomodoro"], true);
    assert_eq!(later_json["candidates"][0]["replacement"], "future");
}

#[test]
fn capture_complete_pomodoro_name_json_skips_create_when_ledger_cannot_place_it(
) {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-create-blocked");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    let draft = "x @dev:some-id#future";

    let missing_day = vault.join("2026/20260828.md");
    let missing = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &missing_day)
        .output()
        .expect("run missing-day Pomodoro completion");
    assert_success(&missing);
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json");
    assert_eq!(missing_json["candidates"], serde_json::json!([]));
    assert!(
        missing_json["warnings"][0]
            .as_str()
            .expect("warning")
            .contains("does not exist"),
        "{missing_json}"
    );

    let sectionless = vault.join("2026/20260829.md");
    write_file(&sectionless, "# Day\n");
    let no_section = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &sectionless)
        .output()
        .expect("run sectionless Pomodoro completion");
    assert_success(&no_section);
    let no_section_json: serde_json::Value =
        serde_json::from_str(stdout(&no_section).trim()).expect("json");
    assert_eq!(no_section_json["candidates"], serde_json::json!([]));
    assert!(
        no_section_json["warnings"][0]
            .as_str()
            .expect("warning")
            .contains("no Pomodoros section"),
        "{no_section_json}"
    );

    let ambiguous_day = vault.join("2026/20260830.md");
    write_file(
        &ambiguous_day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] (1000-1030) — BUGS\n",
            "- [ ] ()\n",
        ),
    );
    let ambiguous = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &ambiguous_day)
        .output()
        .expect("run ambiguous Pomodoro completion");
    assert_success(&ambiguous);
    let ambiguous_json: serde_json::Value =
        serde_json::from_str(stdout(&ambiguous).trim()).expect("json");
    assert!(
        ambiguous_json["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|row| row.get("creates_pomodoro").is_none()),
        "{ambiguous_json}"
    );
    assert!(
        ambiguous_json["warnings"][0]
            .as_str()
            .expect("warning")
            .contains("multiple open timed Pomodoros"),
        "{ambiguous_json}"
    );
}

#[test]
fn capture_complete_task_section_json_covers_components_and_warnings() {
    let temp = TempDir::new("bob-cli-capture-complete-task-section");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("foo.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        ),
    );

    let raw = "note @foo+bar#req";
    let hash = raw.find('#').expect("hash");
    let plus = raw.find('+').expect("plus");
    let at = raw.find('@').expect("at");

    let section = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(raw.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(raw)
        .output()
        .expect("section complete");
    assert_success(&section);
    let section_json: serde_json::Value =
        serde_json::from_str(stdout(&section).trim()).expect("json");
    assert_eq!(section_json["context"], "task_section");
    assert_eq!(
        section_json["replacement"],
        serde_json::json!({"start": hash + 1, "end": raw.len()})
    );
    assert_eq!(section_json["candidates"][0]["replacement"], "requirements");
    assert_eq!(section_json["candidates"][0]["title"], "REQUIREMENTS");
    assert_eq!(section_json["candidates"][0]["slug"], "requirements");
    assert_eq!(section_json["candidates"][0]["route"], "foo");
    assert_eq!(section_json["candidates"][0]["block_id"], "bar");
    assert_eq!(section_json["candidates"][0]["text"], "Parent task");

    let empty_selector = "note @foo+bar#";
    let bare = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(empty_selector.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(empty_selector)
        .output()
        .expect("bare hash complete");
    assert_success(&bare);
    let bare_json: serde_json::Value =
        serde_json::from_str(stdout(&bare).trim()).expect("json");
    assert_eq!(bare_json["context"], "task_section");
    let titles: Vec<&str> = bare_json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| candidate["title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles, ["REQUIREMENTS", "FUTURE WORK"]);
    assert_eq!(bare_json["candidates"][1]["replacement"], "future-work");

    let empty_id = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("11")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+#")
        .output()
        .expect("empty block id");
    assert_success(&empty_id);
    let empty_id_json: serde_json::Value =
        serde_json::from_str(stdout(&empty_id).trim()).expect("json");
    assert_eq!(empty_id_json["context"], "task_section");
    assert_eq!(
        empty_id_json["candidates"]
            .as_array()
            .expect("candidates")
            .len(),
        0
    );
    assert!(empty_id_json.get("warnings").is_none());

    let missing = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("18")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+missing#")
        .output()
        .expect("missing parent");
    assert_success(&missing);
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json");
    assert_eq!(missing_json["context"], "task_section");
    assert_eq!(
        missing_json["candidates"]
            .as_array()
            .expect("candidates")
            .len(),
        0
    );
    assert_eq!(
        missing_json["warnings"][0],
        "no task with block ID ^missing in foo.md"
    );

    let route = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg((at + 3).to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(raw)
        .output()
        .expect("route component");
    assert_success(&route);
    let route_json: serde_json::Value =
        serde_json::from_str(stdout(&route).trim()).expect("json");
    assert_eq!(route_json["context"], "route");
    assert_eq!(route_json["candidates"][0]["route"], "foo");

    let task = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg((plus + 2).to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(raw)
        .output()
        .expect("task component");
    assert_success(&task);
    let task_json: serde_json::Value =
        serde_json::from_str(stdout(&task).trim()).expect("json");
    assert_eq!(task_json["context"], "task");
    assert_eq!(task_json["candidates"][0]["block_id"], "bar");
}

#[test]
fn capture_complete_wikilink_note_json_returns_replacement_and_cursor_after() {
    let temp = TempDir::new("bob-cli-capture-complete-wikilink-note");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Artificial Intelligence.md"),
        "---\naliases: [AI]\n---\n# Design\n",
    );

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("4")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("[[AI")
        .output()
        .expect("run bob capture-complete wikilink note json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "wikilink_note");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 2, "end": 4})
    );
    assert!(json.get("warnings").is_none());
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["replacement"], "Artificial Intelligence|AI]]");
    assert_eq!(candidates[0]["cursor_after"], 30);
    assert_eq!(candidates[0]["path"], "Artificial Intelligence.md");
    assert_eq!(candidates[0]["name"], "Artificial Intelligence");
    assert_eq!(candidates[0]["alias"], "AI");
    assert_eq!(candidates[0]["match_kind"], "exact_alias");
}

#[test]
fn capture_complete_wikilink_same_note_heading_uses_capture_route() {
    let temp = TempDir::new("bob-cli-capture-complete-wikilink-heading");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "# Design\n# Decision Log\n");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("16")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@sase task [[#De")
        .output()
        .expect("run bob capture-complete wikilink heading json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "wikilink_heading");
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate["heading"].as_str().expect("heading"))
            .collect::<Vec<_>>(),
        vec!["Design", "Decision Log"]
    );
    assert_eq!(candidates[0]["replacement"], "Design]]");
    assert_eq!(candidates[0]["path"], "sase.md");
}

#[test]
fn capture_complete_missing_note_behind_a_resolved_route_is_an_empty_success() {
    let temp = TempDir::new("bob-cli-capture-complete-missing-note");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("12")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Idea @notes#")
        .output()
        .expect("run bob capture-complete missing note");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "section");
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn capture_complete_cursor_in_body_text_is_an_empty_success() {
    let temp = TempDir::new("bob-cli-capture-complete-body");
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(temp.path())
        .arg("-c")
        .arg("4")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("buy milk @groceries")
        .output()
        .expect("run bob capture-complete body text");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert!(json["context"].is_null());
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 4, "end": 4})
    );
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn capture_complete_bare_hash_marker_is_an_empty_success() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-note");
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(temp.path())
        .arg("-c")
        .arg("10")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("jot this #")
        .output()
        .expect("run bob capture-complete on a trailing #");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert!(json["context"].is_null());
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 10})
    );
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn capture_complete_empty_text_defaults_to_an_empty_draft() {
    let temp = TempDir::new("bob-cli-capture-complete-empty-text");
    let output = run_with_stdin(
        bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(temp.path())
            .arg("-c")
            .arg("0")
            .arg("-f")
            .arg("json"),
        "",
    );

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert!(json["context"].is_null());
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn capture_complete_rejects_a_cursor_outside_the_text() {
    let temp = TempDir::new("bob-cli-capture-complete-cursor-range");
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(temp.path())
        .arg("-c")
        .arg("99")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@ca")
        .output()
        .expect("run bob capture-complete out-of-range cursor");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).is_empty(),
        "JSON failures keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error string")
            .contains("UTF-8 byte boundary"),
        "unexpected capture-complete failure JSON: {json}"
    );
}

#[test]
fn capture_complete_rejects_a_cursor_that_splits_a_multibyte_character() {
    let temp = TempDir::new("bob-cli-capture-complete-cursor-boundary");
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(temp.path())
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("\u{e9}x")
        .output()
        .expect("run bob capture-complete mid-character cursor");

    assert_eq!(output.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete failure JSON");
    assert_eq!(json["ok"], false);
}

#[test]
fn capture_complete_reports_utf8_byte_offsets() {
    let text = "caf\u{e9} \u{1f680} @Cash+goog-exit";
    let temp = TempDir::new("bob-cli-capture-complete-utf8");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(
        vault.join("cash.md"),
        "# Tasks\n- [ ] #task Google exit ^goog-exit\n",
    )
    .expect("write cash.md");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("21")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-complete utf8");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["context"], "task");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 17, "end": 26})
    );
    assert_eq!(&text[17..26], "goog-exit");
}

#[test]
fn capture_complete_human_output_is_plain_and_concise() {
    let temp = TempDir::new("bob-cli-capture-complete-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(vault.join("cash.md"), "---\ntype: [[area]]\n---\n")
        .expect("write cash.md");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("--")
        .arg("@")
        .output()
        .expect("run bob capture-complete human");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-complete stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert_text_order(&out, &["route", "replacement", "Candidates", "cash"]);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_complete_discovery_failure_reports_an_actionable_error() {
    let temp = TempDir::new("bob-cli-capture-complete-discovery-failure");
    let missing_vault = temp.path().join("missing-vault");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&missing_vault)
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@")
        .output()
        .expect("run bob capture-complete discovery failure");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing vault should be an IO failure:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("failed to read directory")),
        "unexpected capture-complete failure JSON: {json}"
    );
}

#[test]
fn capture_complete_never_creates_the_vault_directory() {
    let temp = TempDir::new("bob-cli-capture-complete-no-mutations");
    let missing = temp.path().join("missing-vault");

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&missing)
        .arg("-c")
        .arg("12")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Idea @notes#")
        .output()
        .expect("run bob capture-complete section against a missing vault");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-complete JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "section");
    assert_eq!(json["candidates"], serde_json::json!([]));
    assert!(
        !missing.exists(),
        "capture-complete must not create the vault directory"
    );
}

#[test]
fn capture_complete_pomodoro_ranges_preserve_start_suffix() {
    let temp = TempDir::new("bob-cli-capture-complete-start-suffix");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n- [ ] () — FOCUS\n",
    );
    let complete = |draft: &str, cursor: usize| {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(&vault)
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .env("BOB_DAY_FILE", &day_file)
            .output()
            .expect("run capture-complete");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("complete JSON")
    };

    // A cursor inside the name completes just the name; splicing the first
    // candidate into the reported range keeps the typed suffix.
    let draft = "Do work @sase:outline#bu=-2";
    let suffix_start = draft.find('=').expect("suffix");
    let name = complete(draft, suffix_start - 1);
    assert_eq!(name["context"], "pomodoro_name");
    assert_eq!(
        name["replacement"],
        serde_json::json!({ "start": 22, "end": suffix_start })
    );
    let first = name["candidates"][0]["replacement"]
        .as_str()
        .expect("candidate text");
    let accepted =
        format!("{}{}{}", &draft[..22], first, &draft[suffix_start..]);
    assert!(
        accepted.ends_with("=-2"),
        "accepting a name must keep the suffix: {accepted}"
    );
    assert!(accepted.starts_with("Do work @sase:outline#"), "{accepted}");

    // A cursor inside the suffix offers no completion.
    let in_suffix = complete(draft, suffix_start + 1);
    assert!(in_suffix["context"].is_null(), "{in_suffix}");
    assert_eq!(
        in_suffix["candidates"],
        serde_json::json!([]),
        "{in_suffix}"
    );

    // Block-ID completion behaves the same way.
    let block_draft = "Do work @sase:out=3";
    let block_suffix = block_draft.find('=').expect("suffix");
    let block = complete(block_draft, block_suffix);
    assert_eq!(block["context"], "pomodoro_block_id");
    assert_eq!(
        block["replacement"],
        serde_json::json!({ "start": 14, "end": block_suffix })
    );
    let at_end = complete(block_draft, block_draft.len());
    assert!(at_end["context"].is_null(), "{at_end}");
}

#[test]
fn capture_complete_pomodoro_close_protocol() {
    // An empty success inside `=x`, and whenever the cursor is inside a
    // `=x` suffix.
    for (text, cursor) in [("=x", 1), ("=x", 2), ("^r:id=x", 6)] {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-complete");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("complete JSON");
        assert_eq!(json["ok"], true, "{text} {cursor}");
        assert_eq!(
            json["candidates"],
            serde_json::json!([]),
            "{text} {cursor}"
        );
    }
}

#[test]
fn capture_complete_pomodoro_close_selection_protocol() {
    // A cursor anywhere inside `=x…`, including the task-number lists and
    // a dangling separator, returns an empty success.
    for (text, cursor) in [
        ("=x1,3!2", 1),
        ("=x1,3!2", 3),
        ("=x1,3!2", 5),
        ("=x1,3!2", 7),
        ("=x1,", 4),
        ("=x!", 3),
        ("^r:id=x1", 8),
        ("^r:id=x1,", 9),
        ("Text @r:id=x1!2", 14),
    ] {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-complete");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("complete JSON");
        assert_eq!(json["ok"], true, "{text} {cursor}");
        assert_eq!(
            json["candidates"],
            serde_json::json!([]),
            "{text} {cursor}"
        );
    }
}

#[test]
fn capture_complete_pomodoro_start_name_json_covers_empty_and_new_queries() {
    let temp = TempDir::new("bob-cli-capture-complete-start-name");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "  - [[sase#^plan-day]]\n",
            "- [ ] () — BUGS\n",
            "  - [[sase#^deep-fix]]\n",
            "- [ ] () — DEEP WORK\n",
            "  - [[bob#^outline]]\n",
            "  - [[bob#^draft]]\n",
            "- [ ] ()\n",
            "  - [[bob#^inbox-zero]]\n",
        ),
    );

    let complete = |draft: &str| {
        bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(&vault)
            .arg("-c")
            .arg(draft.len().to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .env("BOB_DAY_FILE", &day_file)
            .output()
            .expect("run start-name completion")
    };

    let empty = complete("=#");
    assert_success(&empty);
    let empty_json: serde_json::Value =
        serde_json::from_str(stdout(&empty).trim()).expect("json");
    assert_eq!(empty_json["context"], "pomodoro_start_name");
    assert_eq!(
        empty_json["replacement"],
        serde_json::json!({ "start": 2, "end": 2 })
    );
    let rows = empty_json["candidates"].as_array().expect("candidates");
    let replacements = rows
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect::<Vec<_>>();
    assert_eq!(replacements, vec!["bugs", "deep-work", "plan", ""]);
    assert_eq!(rows[0]["next_up"], true);
    assert_eq!(rows[0]["name"], "BUGS");
    assert_eq!(rows[2]["state"], "completed");
    assert_eq!(rows[2]["time_range"], "0830-0855");
    assert_eq!(rows[2]["creates_pomodoro"], true);
    assert_eq!(rows[3]["requires_name"], true);
    assert!(empty_json.get("warnings").is_none(), "{empty_json}");

    let novel = complete("=#rev");
    assert_success(&novel);
    let novel_json: serde_json::Value =
        serde_json::from_str(stdout(&novel).trim()).expect("json");
    assert_eq!(novel_json["context"], "pomodoro_start_name");
    assert_eq!(
        novel_json["replacement"],
        serde_json::json!({ "start": 2, "end": 5 })
    );
    let novel_rows = novel_json["candidates"].as_array().expect("candidates");
    assert_eq!(novel_rows[0]["replacement"], "rev");
    assert_eq!(novel_rows[0]["name"], "REV");
    assert_eq!(novel_rows[0]["creates_pomodoro"], true);
    assert!(novel_rows[0].get("next_up").is_none(), "{novel_json}");
    assert!(novel_rows[0].get("ref").is_none(), "{novel_json}");

    // A cursor on the suffix side of `#` stays an empty success.
    let at_hash = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=#rev")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run start-name completion at hash");
    assert_success(&at_hash);
    let at_hash_json: serde_json::Value =
        serde_json::from_str(stdout(&at_hash).trim()).expect("json");
    assert!(at_hash_json["context"].is_null(), "{at_hash_json}");
    assert_eq!(
        at_hash_json["candidates"],
        serde_json::json!([]),
        "{at_hash_json}"
    );
}

#[test]
fn capture_complete_pomodoro_start_name_human_labels_rows() {
    let temp = TempDir::new("bob-cli-capture-complete-start-name-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "- [ ] () — BUGS\n",
            "  - [[sase#^deep-fix]]\n",
            "- [ ] ()\n",
        ),
    );

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("2")
        .arg("--")
        .arg("=#")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run start-name human completion");
    assert_success(&output);
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "pomodoro_start_name",
            "next up · 1 link",
            "again · last 0830-0855",
            "name it · Empty",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_complete_pomodoro_start_drop_protocol() {
    // A cursor anywhere inside a start `~<K>` drop part, including a
    // dangling separator, returns an empty success.
    for (text, cursor) in [
        ("=~2", 1),
        ("=~2", 2),
        ("=~2", 3),
        ("=~", 2),
        ("=~2,", 4),
        ("=3#bugs~1", 9),
        ("=3#bugs~", 8),
    ] {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-complete");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("complete JSON");
        assert_eq!(json["ok"], true, "{text} {cursor}");
        assert_eq!(
            json["candidates"],
            serde_json::json!([]),
            "{text} {cursor}"
        );
    }

    // The named replacement stops before `~`, so accepting a name keeps a
    // typed drop list.
    let output = bob_command()
        .arg("capture-complete")
        .arg("-c")
        .arg("6")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=3#bugs~1")
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "pomodoro_start_name", "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": 3, "end": 7 }),
        "{json}"
    );
}
