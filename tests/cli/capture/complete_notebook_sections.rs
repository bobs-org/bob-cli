//! The `notebook_section_bullet` completion menu and Bob-authored
//! `place_menu` chrome: title-preferring replacements, the nested
//! continuation, provenance groups, note counts, consequences, refused
//! rows, create-row rules, and the round-trip invariant (menu, parse, and
//! dry-run agree on every enabled row).

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-11-09 12:00:00";

fn vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        concat!(
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n",
            "# Developer tools\n\n",
            "## Git\n\n",
            "- Use worktrees for investigations [created::2026-11-08] ^existing\n",
            "- Second note [created::2026-11-08]\n\n",
            "## API Design\n\n",
            "- Preserve meaning across versions [created::2026-11-08]\n\n",
            "## Docker\n\n",
            "- Pin base images by digest [created::2026-11-08]\n\n",
            "## Empty Section\n",
        ),
    );
    write_file(
        &vault.join("dup_nb.md"),
        concat!(
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\n---\n\n",
            "# Duplicates\n\n",
            "## Git\n\n",
            "- note one [created::2026-11-08]\n\n",
            "## Git\n\n",
            "- note two [created::2026-11-08]\n",
        ),
    );
    vault
}

fn complete_at(
    temp: &TempDir,
    vault: &std::path::Path,
    draft: &str,
    cursor: usize,
) -> Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("completion JSON")
}

fn end_complete(temp: &TempDir, vault: &std::path::Path, draft: &str) -> Value {
    complete_at(temp, vault, draft, draft.len())
}

fn rows(json: &Value) -> &Vec<Value> {
    json["candidates"].as_array().expect("candidates")
}

fn titles(json: &Value) -> Vec<String> {
    rows(json)
        .iter()
        .map(|row| {
            row["title"]
                .as_str()
                .unwrap_or(row["label"].as_str().unwrap_or("?"))
                .to_string()
        })
        .collect()
}

fn parse(temp: &TempDir, vault: &std::path::Path, draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DIR", vault)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("parse JSON")
}

fn dry_run(temp: &TempDir, vault: &std::path::Path, draft: &str) -> Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture dry-run");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

#[test]
fn bullet_menu_lists_notebook_sections_with_detail_and_chrome() {
    let temp = TempDir::new("bob-cli-complete-section-bullet-menu");
    let vault = vault(&temp);
    let draft = "@dev_tools\n- #";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "notebook_section_bullet", "{json}");
    assert_eq!(json["query"], "", "{json}");
    assert_eq!(json["initial_selection"], "none", "{json}");
    // The replacement covers the empty selector after `#`.
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": draft.len(), "end": draft.len()}),
        "{json}"
    );
    // An empty query keeps document order with no create row.
    assert_eq!(
        titles(&json),
        vec!["Git", "API Design", "Docker", "Empty Section"],
        "{json}"
    );
    let git = &rows(&json)[0];
    assert_eq!(git["kind"], "notebook_section", "{json}");
    assert_eq!(git["group"], "notebook", "{json}");
    assert_eq!(git["line"], 9, "{json}");
    assert_eq!(git["note_count"], 2, "{json}");
    assert_eq!(git["detail"], "2 notes", "{json}");
    assert_eq!(
        git["consequence"], "Adds after “Second note” in ## Git",
        "{json}"
    );
    // The continuation opens the first nested note on every row.
    assert_eq!(git["replacement"], "Git\n  - ", "{json}");
    assert!(git["cursor_after"].as_u64().is_some(), "{json}");
    let empty = &rows(&json)[3];
    assert_eq!(empty["detail"], "empty", "{json}");
    assert_eq!(
        empty["consequence"], "First note in ## Empty Section",
        "{json}"
    );
    let menu = &json["place_menu"];
    assert_eq!(menu["kind"], "notebook_section", "{json}");
    assert_eq!(menu["title"], "Section", "{json}");
    assert_eq!(menu["caption"], "📓 Developer tools · dev", "{json}");
    assert_eq!(
        menu["groups"],
        serde_json::json!([
            {"id": "notebook", "title": "In this notebook"},
            {"id": "draft", "title": "Earlier in this draft"},
            {"id": "new", "title": ""},
        ]),
        "{json}"
    );
    assert_eq!(menu["new_row_prefix"], "＋ ", "{json}");
    assert_eq!(menu["empty_hint"], "Type a section name", "{json}");
    assert_eq!(menu["results_label"], "sections", "{json}");
    assert_eq!(menu["footer_hint"], "⏎ choose · ⎋ close", "{json}");
}

#[test]
fn bullet_replacement_covers_only_the_selector() {
    let temp = TempDir::new("bob-cli-complete-section-bullet-range");
    let vault = vault(&temp);
    let draft = "@dev_tools\n- #gi";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "notebook_section_bullet", "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": draft.len() - 2, "end": draft.len()}),
        "the `#` stays in the draft: {json}"
    );
    // `gi` is a strict prefix of Git, so the create row stays hidden.
    assert_eq!(titles(&json), vec!["Git"], "{json}");
    assert_eq!(rows(&json)[0]["replacement"], "Git\n  - ", "{json}");
}

#[test]
fn bullet_typed_query_ranks_prefix_before_substring() {
    let temp = TempDir::new("bob-cli-complete-section-bullet-rank");
    let vault = vault(&temp);
    // `e` prefixes Empty Section, matches API Design and Docker by
    // substring, and misses Git; the prefix row leads with document order
    // breaking the substring tie. `e` prefixes a listed title, so no
    // create row.
    let draft = "@dev_tools\n- #e";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(
        titles(&json),
        vec!["Empty Section", "API Design", "Docker"],
        "{json}"
    );
    // A typed query with an enabled row keeps the default first row.
    assert!(json["initial_selection"].is_null(), "{json}");
    // `o` matches by substring only, so the create row joins last.
    let draft = "@dev_tools\n- #o";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(
        titles(&json),
        vec!["Docker", "Empty Section", "O"],
        "{json}"
    );
}

#[test]
fn bullet_create_row_preselects_only_when_clean() {
    let temp = TempDir::new("bob-cli-complete-section-create-row");
    let vault = vault(&temp);
    // A lone clean create row is preselected (preselect_clean).
    let draft = "@dev_tools\n- #wireguard";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(titles(&json), vec!["Wireguard"], "{json}");
    assert!(json["initial_selection"].is_null(), "{json}");
    let create = &rows(&json)[0];
    assert_eq!(create["kind"], "notebook_section_create", "{json}");
    assert_eq!(create["group"], "new", "{json}");
    assert_eq!(create["creates"], true, "{json}");
    assert!(create["detail"].is_null(), "{json}");
    assert_eq!(
        create["consequence"],
        "Creates ## Wireguard at the end of Developer tools",
        "{json}"
    );
    assert_eq!(create["replacement"], "wireguard\n  - ", "{json}");
    assert!(create["cursor_after"].as_u64().is_some(), "{json}");
    // A near miss takes the quoted spelling, names the heading it almost
    // spelled, and is never preselected.
    let draft = "@dev_tools\n- #dockr";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(titles(&json), vec!["Dockr"], "{json}");
    assert_eq!(json["initial_selection"], "none", "{json}");
    let create = &rows(&json)[0];
    assert_eq!(create["replacement"], "\"Dockr\"\n  - ", "{json}");
    assert_eq!(create["detail"], "did you mean Docker?", "{json}");
    // An exact title match offers the row with no create row.
    let draft = "@dev_tools\n- #git";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(titles(&json), vec!["Git"], "{json}");
}

#[test]
fn bullet_menu_round_trips_through_parse_and_dry_run() {
    let temp = TempDir::new("bob-cli-complete-section-round-trip");
    let vault = vault(&temp);
    // Every enabled row in both notebook contexts splices to a draft
    // whose parse span and dry-run filing report the advertised section
    // and action.
    let mut drafts = vec![
        "@dev_tools\n- #".to_string(),
        "@dev_tools\n- #o".to_string(),
        "@dev_tools\n- #wireguard".to_string(),
        "More @dev_tools#".to_string(),
    ];
    // A `%n` draft contributes its staged draft-group row.
    drafts
        .push("Nvim notes @dev^nvim@ %n\n- KEYMAPS\n  - x\n- #key".to_string());
    for draft in drafts {
        let json = end_complete(&temp, &vault, &draft);
        let replacement = json["replacement"].clone();
        let (start, end) = (
            replacement["start"].as_u64().unwrap() as usize,
            replacement["end"].as_u64().unwrap() as usize,
        );
        for row in rows(&json).clone() {
            if row["refused"].as_str().is_some() {
                continue;
            }
            let title = row["title"].as_str().expect("row title").to_string();
            let is_create = row["kind"] == "notebook_section_create";
            // Fill the appended continuation with a probe note; glued rows
            // splice bare.
            let inserted = row["replacement"].as_str().unwrap();
            let inserted = inserted
                .strip_suffix("\n  - ")
                .map(|stem| format!("{stem}\n  - probe note"))
                .unwrap_or_else(|| inserted.to_string());
            let spliced =
                format!("{}{}{}", &draft[..start], inserted, &draft[end..]);
            let parsed = parse(&temp, &vault, &spliced);
            let spans: Vec<&Value> = parsed["spans"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|span| span["kind"] == "section")
                .collect();
            assert_eq!(spans.len(), 1, "{spliced}: {parsed}");
            assert_eq!(spans[0]["title"], title, "{spliced}: {parsed}");
            let captured = dry_run(&temp, &vault, &spliced);
            if captured.get("kind") == Some(&Value::from("notebook_create")) {
                // `%n` files the note into the seeded section of the new
                // notebook body.
                let bodies: Vec<&Value> =
                    captured["note_files"].as_array().unwrap().iter().collect();
                assert!(
                    bodies.iter().any(|file| file["body"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("## {title}"))),
                    "{spliced}: {captured}"
                );
                continue;
            }
            assert_eq!(
                captured["notebook"]["section"], title,
                "{spliced}: {captured}"
            );
            assert_eq!(
                captured["notebook"]["section_action"],
                if is_create { "create" } else { "existing" },
                "{spliced}: {captured}"
            );
            assert_eq!(
                spans[0]["section_action"],
                if is_create { "create" } else { "existing" },
                "{spliced}: {parsed}"
            );
        }
    }
}

#[test]
fn bullet_staged_view_lists_earlier_draft_sections() {
    let temp = TempDir::new("bob-cli-complete-section-staged");
    let vault = vault(&temp);
    // An earlier bullet in the same list appears under `draft`.
    let draft = "@dev_tools\n- #wireguard\n  - Set up tunnel\n- #wir";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "notebook_section_bullet", "{json}");
    // `wir` is a strict prefix of the staged title, so no create row.
    assert_eq!(titles(&json), vec!["Wireguard"], "{json}");
    let row = &rows(&json)[0];
    assert_eq!(row["group"], "draft", "{json}");
    assert_eq!(row["detail"], "new", "{json}");
    assert_eq!(
        row["consequence"], "Adds to ## Wireguard, new in this draft",
        "{json}"
    );
    assert!(row.get("line").is_none_or(Value::is_null), "{json}");
    // An earlier `%n` seed appears in a later glued menu.
    let draft = "Nvim notes @dev^nvim@ %n\n- KEYMAPS\n  - x\n\nMore @dev_nvim#";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "section", "{json}");
    assert_eq!(titles(&json), vec!["Keymaps"], "{json}");
    let row = &rows(&json)[0];
    assert_eq!(row["group"], "draft", "{json}");
    // A failed earlier bullet contributes no staged section.
    let draft = "@dev_tools\n- #wireguard oops\n- #";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(
        titles(&json),
        vec!["Git", "API Design", "Docker", "Empty Section"],
        "{json}"
    );
}

#[test]
fn bullet_menu_refuses_ambiguous_duplicates() {
    let temp = TempDir::new("bob-cli-complete-section-refused");
    let vault = vault(&temp);
    let draft = "@dup_nb\n- #git";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "notebook_section_bullet", "{json}");
    // Both duplicates stay listed as refused rows; the exact match
    // suppresses the create row and nothing is preselected.
    assert_eq!(rows(&json).len(), 2, "{json}");
    assert_eq!(json["initial_selection"], "none", "{json}");
    for row in rows(&json) {
        assert_eq!(row["title"], "Git", "{json}");
        assert!(
            row["refused"]
                .as_str()
                .unwrap()
                .contains("2 sections titled"),
            "{json}"
        );
    }
}

#[test]
fn section_menu_warns_without_rows_on_read_failure() {
    let temp = TempDir::new("bob-cli-complete-section-read-failure");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    // A directory at the note path fails the read without going missing.
    fs::create_dir_all(vault.join("ghost_nb.md")).expect("create dir note");
    let draft = "More @ghost_nb#x";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "section", "{json}");
    assert!(rows(&json).is_empty(), "{json}");
    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{json}");
    assert!(warnings[0].as_str().unwrap().contains("ghost_nb"), "{json}");
}

#[test]
fn place_menu_covers_drawer_glued_and_bullet_menus() {
    let temp = TempDir::new("bob-cli-complete-place-menu");
    let vault = vault(&temp);
    // The glued notebook menu carries notebook chrome without a
    // continuation.
    let draft = "More @dev_tools#";
    let json = end_complete(&temp, &vault, draft);
    assert_eq!(json["context"], "section", "{json}");
    assert_eq!(json["initial_selection"], "none", "{json}");
    let menu = &json["place_menu"];
    assert_eq!(menu["kind"], "notebook_section", "{json}");
    assert_eq!(menu["caption"], "📓 Developer tools · dev", "{json}");
    assert!(
        rows(&json)
            .iter()
            .all(|row| row.get("cursor_after").is_none_or(Value::is_null)),
        "glued rows carry no continuation: {json}"
    );
    // `%n` captions the new notebook.
    let draft = "Nvim notes @dev^nvim@ %n\n- KEYMAPS\n  - x\n- #key";
    let json = end_complete(&temp, &vault, draft);
    let menu = &json["place_menu"];
    assert_eq!(menu["caption"], "📓 New notebook · Nvim · dev", "{json}");
}

#[test]
fn bullet_menu_opens_nowhere_else() {
    let temp = TempDir::new("bob-cli-complete-section-negatives");
    let vault = vault(&temp);
    // Under one notebook note the bullet is opaque note detail.
    let draft = "Text @dev_tools\n- #";
    let json = end_complete(&temp, &vault, draft);
    assert!(json["context"].is_null(), "{json}");
    assert!(json.get("place_menu").is_none_or(Value::is_null), "{json}");
    // Under an `@nb#Section` header the bullet is refused, not completed.
    let draft = "List @dev_tools#git\n- #";
    let json = end_complete(&temp, &vault, draft);
    assert!(json["context"].is_null(), "{json}");
    // A nested bullet never opens the place menu.
    let draft = "@dev_tools\n- #git\n  - note\n  - #";
    let json = complete_at(&temp, &vault, draft, draft.len());
    assert_ne!(json["context"], "notebook_section_bullet", "{json}");
    assert_ne!(json["context"], "drawer", "{json}");
    assert!(json.get("place_menu").is_none_or(Value::is_null), "{json}");
}
