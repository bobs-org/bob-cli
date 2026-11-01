//! `^prj` drawer capture: `#drawer` on a project note's own `^prj` task
//! resolves against the note's eligible `##` sections.

use crate::support::*;
use std::fs;

const PROJECT_NOTE: &str = concat!(
    "---\n",
    "type: \"[[project]]\"\n",
    "---\n",
    "\n",
    "# Project\n",
    "\n",
    "- [ ] #task #prj Ship it ^prj\n",
    "\n",
    "## Requirements\n",
    "\n",
    "notes\n",
    "\n",
    "more notes\n",
    "\n",
    "## Pomodoros\n",
    "\n",
    "- [ ] (0900-0930) Work\n",
    "\n",
    "## Future Work\n",
    "\n",
    "- existing\n",
);

#[test]
fn capture_prj_drawer_appends_after_section_content() {
    let temp = TempDir::new("bob-cli-prj-drawer-existing");
    let vault = temp.path().join("vault");
    write_file(&vault.join("proj.md"), PROJECT_NOTE);

    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("Postgres 17 minimum @proj+prj#requirements")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("dry-run prj drawer");
    assert_success(&dry_run);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("dry JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["kind"], "sub_bullet");
    assert_eq!(dry_json["drawer"]["title"], "Requirements");
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read note"),
        PROJECT_NOTE,
        "dry-run must not write"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Postgres 17 minimum @proj+prj#requirements")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run prj drawer capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["drawer"]["title"], "Requirements");
    assert_eq!(json["block_id"], "prj");
    assert_eq!(json["task_line"], "- Postgres 17 minimum");
    assert_eq!(json["placement"], "inserted");
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read note"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "\n",
            "# Project\n",
            "\n",
            "- [ ] #task #prj Ship it ^prj\n",
            "\n",
            "## Requirements\n",
            "\n",
            "notes\n",
            "\n",
            "more notes\n",
            "- Postgres 17 minimum\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-0930) Work\n",
            "\n",
            "## Future Work\n",
            "\n",
            "- existing\n",
        )
    );

    // A trailing section appends at the end of the note.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("follow up @proj+prj#future-work")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run trailing prj drawer capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["drawer"]["title"], "Future Work");
    assert_eq!(json["placement"], "appended");
    assert!(fs::read_to_string(vault.join("proj.md"))
        .expect("read note")
        .ends_with("## Future Work\n\n- existing\n- follow up\n"));
}

#[test]
fn capture_prj_drawer_creates_missing_h2_in_title_case() {
    let temp = TempDir::new("bob-cli-prj-drawer-create");
    let vault = temp.path().join("vault");
    write_file(&vault.join("proj.md"), PROJECT_NOTE);

    // Two rows in one draft create the section once.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("first @proj+prj#new-area\n\nsecond @proj+prj#new-area")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run batch prj drawer create");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert!(captures
        .iter()
        .all(|item| item["drawer"]["title"] == "New Area"));
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read note"),
        format!("{PROJECT_NOTE}\n## New Area\n\n- first\n- second\n")
    );

    // An explicit `--drawer` title matches case insensitively and creates
    // a missing section in Title Case.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("proj")
        .arg("--task")
        .arg("prj")
        .arg("--drawer")
        .arg("tech debt")
        .arg("--")
        .arg("debt entry")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run --drawer prj create");
    assert_success(&output);
    assert!(fs::read_to_string(vault.join("proj.md"))
        .expect("read note")
        .ends_with("## Tech Debt\n\n- debt entry\n"));
}

#[test]
fn capture_prj_drawer_structural_and_reserved_refuse() {
    let cases = [
        (
            "structural-pomodoros",
            "jot @proj+prj#pomodoros",
            "use `@proj#<Heading>` to capture into a project `##` section",
        ),
        (
            "structural-tasks",
            "jot @proj+prj#tasks",
            "use `@proj#<Heading>` to capture into a project `##` section",
        ),
        (
            "reserved-links",
            "jot @proj+prj#links",
            "`LINKS` is managed by Bob; name the section something else",
        ),
    ];

    for (name, draft, expected) in cases {
        for json in [false, true] {
            let temp = TempDir::new(&format!(
                "bob-cli-prj-drawer-refuse-{name}-{}",
                if json { "json" } else { "human" }
            ));
            let vault = temp.path().join("vault");
            write_file(&vault.join("proj.md"), PROJECT_NOTE);
            let mut command = bob_command();
            command.arg("capture").arg("-b").arg(&vault);
            if json {
                command.arg("-f").arg("json");
            }
            let output = command
                .arg(draft)
                .env("BOB_NOW", "2026-07-31")
                .output()
                .expect("run refusing prj drawer capture");
            assert_eq!(
                output.status.code(),
                Some(2),
                "{name} / {json}: {}",
                format_output(&output)
            );
            let error_text = if json {
                let value: serde_json::Value =
                    serde_json::from_str(stdout(&output).trim())
                        .expect("JSON error object");
                assert_eq!(value["ok"], false);
                value["error"].as_str().unwrap_or_default().to_string()
            } else {
                stderr(&output)
            };
            assert!(
                error_text.contains(expected),
                "{name} / {json}: expected {expected:?} in {error_text:?}"
            );
            assert_eq!(
                fs::read_to_string(vault.join("proj.md")).expect("read note"),
                PROJECT_NOTE,
                "{name} / {json} must leave the note unchanged"
            );
        }
    }
}

#[test]
fn capture_prj_drawer_ambiguous_and_guard_refuse_but_literal_wins() {
    let original = concat!(
        "# Dup\n",
        "\n",
        "- [ ] #task #prj Dup ^prj\n",
        "\n",
        "## Future Work\n",
        "\n",
        "- one\n",
        "\n",
        "## Future-Work\n",
        "\n",
        "- two\n",
        "\n",
        "## Requirements\n",
        "\n",
        "- req\n",
    );
    let cases = [
        (
            "ambiguous",
            "jot @dup+prj#future-work",
            "matches more than one section (lines 5, 9)",
        ),
        (
            "prefix-guard",
            "jot @dup+prj#requi",
            "did you mean `#requirements`?",
        ),
        (
            "edit-distance-guard",
            "jot @dup+prj#requirments",
            "did you mean `#requirements`?",
        ),
    ];

    for (name, draft, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-prj-drawer-guard-{name}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("dup.md"), original);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg(draft)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("run guarded prj drawer capture");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{name}: {}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(expected),
            "{name}: expected {expected:?} in {:?}",
            stderr(&output)
        );
        assert_eq!(
            fs::read_to_string(vault.join("dup.md")).expect("read note"),
            original,
            "{name} must leave the note unchanged"
        );
    }

    // A quoted literal picks one duplicate exactly.
    let temp = TempDir::new("bob-cli-prj-drawer-literal");
    let vault = temp.path().join("vault");
    write_file(&vault.join("dup.md"), original);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("jot @dup+prj#\"FUTURE-WORK\"")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run literal prj drawer capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["drawer"]["title"], "Future-Work");
    assert_eq!(
        fs::read_to_string(vault.join("dup.md")).expect("read note"),
        original.replace(
            "## Future-Work\n\n- two\n",
            "## Future-Work\n\n- two\n- jot\n"
        )
    );
}

#[test]
fn capture_prj_drawer_authored_children_and_clip_nest_under_entry() {
    let temp = TempDir::new("bob-cli-prj-drawer-composition");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_file(&vault.join("proj.md"), PROJECT_NOTE);
    write_executable(&clipboard, "#!/bin/sh\nprintf 'clip child\\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@proj+prj#requirements buy milk %c\n- authored child")
        .env("BOB_NOW", "2026-07-31")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("run composed prj drawer capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["drawer"]["title"], "Requirements");
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read note"),
        PROJECT_NOTE.replace(
            "more notes\n\n## Pomodoros",
            "more notes\n- buy milk\n\t- authored child\n\t- clip child\n\n## Pomodoros"
        )
    );
}

#[test]
fn capture_prj_drawer_separate_type_and_plain_child_still_work() {
    let temp = TempDir::new("bob-cli-prj-drawer-retype");
    let vault = temp.path().join("vault");
    write_file(&vault.join("proj.md"), PROJECT_NOTE);
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n",
        ),
    );

    // A separate `#type` retypes in the same transaction as the H2 write.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Postgres 17 @proj+prj#requirements #fix")
        .env("BOB_NOW", "2026-07-31")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run prj drawer retype capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["drawer"]["title"], "Requirements");
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    let note = fs::read_to_string(vault.join("proj.md")).expect("read note");
    assert!(note.contains("more notes\n- Postgres 17\n"), "{note}");
    assert!(note.contains("[type:: FIX]"), "{note}");

    // A plain child with a separate `#type` follows the existing project
    // typing rules, unchanged by drawers.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("plain child @proj+prj #fix")
        .env("BOB_NOW", "2026-07-31")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run plain prj child capture");
    assert_success(&output);
}

#[test]
fn capture_complete_prj_drawer_menu_lists_h2_sections() {
    let temp = TempDir::new("bob-cli-prj-drawer-menu");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Ship it ^prj\n",
            "\n",
            "## Requirements\n",
            "\n",
            "- one\n",
            "- two\n",
            "  - nested\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-0930) Work\n",
            "\n",
            "## Q&A\n",
            "\n",
            "prose\n",
        ),
    );

    let draft = "jot @proj+prj#";
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
        .output()
        .expect("prj drawer menu");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("menu JSON");
    assert_eq!(json["context"], "drawer");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let titles: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate["title"].as_str().expect("title"))
        .collect();
    // Structural `## Pomodoros` is excluded; the rest list in order.
    assert_eq!(titles, ["Requirements", "Q&A"]);
    assert_eq!(json["candidates"][0]["group"], "task");
    assert_eq!(json["candidates"][0]["replacement"], "requirements");
    assert_eq!(json["candidates"][0]["entry_count"], 2);
    assert_eq!(
        json["candidates"][0]["consequence"],
        "Adds to ## Requirements"
    );
    // `Q&A` has no slug spelling, so the row carries a quoted literal.
    assert_eq!(json["candidates"][1]["replacement"], "\"Q&A\"");
    assert_eq!(json["candidates"][1]["entry_count"], 0);
    assert_eq!(json["target"]["block_id"], "prj");

    // A typed query filters by slug.
    let draft = "jot @proj+prj#req";
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
        .output()
        .expect("filtered prj drawer menu");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("menu JSON");
    assert_eq!(
        json["candidates"]
            .as_array()
            .expect("candidates array")
            .len(),
        1
    );
    assert_eq!(json["candidates"][0]["title"], "Requirements");
}

#[test]
fn capture_drawers_lists_h2_sections_for_prj_parent() {
    let temp = TempDir::new("bob-cli-prj-drawers-command");
    let vault = temp.path().join("vault");
    write_file(&vault.join("proj.md"), PROJECT_NOTE);

    let output = bob_command()
        .arg("capture-drawers")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("proj")
        .arg("-i")
        .arg("prj")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run capture-drawers on ^prj");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("drawers JSON");
    assert_eq!(json["ok"], true);
    let drawers = json["drawers"].as_array().expect("drawers array");
    let titles: Vec<&str> = drawers
        .iter()
        .map(|drawer| drawer["title"].as_str().expect("title"))
        .collect();
    // Structural `## Pomodoros` is excluded; the rest list in order.
    assert_eq!(titles, ["Requirements", "Future Work"]);
    assert_eq!(drawers[0]["key"], "REQUIREMENTS");
    assert_eq!(drawers[0]["entry_count"], 0);
    assert_eq!(drawers[1]["entry_count"], 1);
}

#[test]
fn capture_prj_drawer_promotion_round_trip() {
    // A drawer capture writes a plain entry under an ALL-CAPS child; the
    // plugins promote that drawer to a `## Title Case` section with the
    // entry copied verbatim. A later `@proj+prj#same-key` capture appends
    // after the promoted section's existing content.
    let temp = TempDir::new("bob-cli-prj-drawer-round-trip");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "\n",
            "# Project\n",
            "\n",
            "- [ ] #task #prj Ship it ^prj\n",
            "\n",
            "## Requirements\n",
            "\n",
            "- Postgres 17 minimum\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("follow up @proj+prj#requirements")
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("run round-trip prj drawer capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["drawer"]["title"], "Requirements");
    assert!(fs::read_to_string(vault.join("proj.md"))
        .expect("read note")
        .ends_with("## Requirements\n\n- Postgres 17 minimum\n- follow up\n"));
}
