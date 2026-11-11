//! Notebook note writes through the guarded capture transaction.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-11-09 12:00:00";
const DATE: &str = "2026-11-09";

fn vault_with_notebook(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# Developer tools\n\n## Git\n\n- Existing note [created::2026-11-08] ^existing\n",
    );
    vault
}

fn run_capture(
    vault: &std::path::Path,
    draft: &str,
    extra: &[&str],
    write: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if !write {
        command.arg("--dry-run");
    }
    for arg in extra {
        command.arg(arg);
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0");
    command.output().expect("run capture")
}

fn dry_single(vault: &std::path::Path, draft: &str) -> Value {
    let output = run_capture(vault, draft, &[], false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn read_notebook(vault: &std::path::Path) -> String {
    fs::read_to_string(vault.join("dev_tools.md")).expect("read notebook")
}

#[test]
fn test_top_level_note_appends_before_first_h2() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let before = read_notebook(&vault);
    let json = dry_single(&vault, "Try mergiraf @dev_tools");
    assert_eq!(json["kind"], "notebook_note");
    assert_eq!(json["notebook"]["route"], "dev_tools");
    assert_eq!(json["notebook"]["title"], "Developer tools");
    assert_eq!(json["notebook"]["section"], Value::Null);
    assert_eq!(
        json["task_line"],
        format!("- Try mergiraf [created::{DATE}]")
    );
    // Dry run preserves bytes.
    assert_eq!(read_notebook(&vault), before);
    let output = run_capture(&vault, "Try mergiraf @dev_tools", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(
        after.contains(&format!("- Try mergiraf [created::{DATE}]\n\n## Git"))
    );
}

#[test]
fn test_section_exact_and_near_miss_and_create() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Try mergiraf @dev_tools#git");
    assert_eq!(json["notebook"]["section"], "Git");
    assert_eq!(json["notebook"]["section_action"], "existing");
    let output = run_capture(&vault, "Try mergiraf @dev_tools#gi", &[], false);
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    assert_eq!(value["code"], "notebook_section_near_miss");
    let json = dry_single(&vault, "Try mergiraf @dev_tools#merge-tools");
    assert_eq!(json["notebook"]["section"], "Merge Tools");
    assert_eq!(json["notebook"]["section_action"], "create");
    let output =
        run_capture(&vault, "Try mergiraf @dev_tools#merge-tools", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("## Merge Tools"));
    assert!(after.contains(&format!("- Try mergiraf [created::{DATE}]")));
}

#[test]
fn test_named_note_and_section_named_note() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Try mergiraf @dev_tools^mergiraf");
    assert_eq!(json["notebook"]["block_id"], "mergiraf");
    assert_eq!(
        json["task_line"],
        format!("- Try mergiraf [created::{DATE}] ^mergiraf")
    );
    let json = dry_single(&vault, "Try mergiraf @dev_tools#git^mergiraf");
    assert_eq!(json["notebook"]["section"], "Git");
    assert_eq!(json["notebook"]["block_id"], "mergiraf");
}

#[test]
fn test_sub_note_appends_unstamped_after_parent() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Faster with -j @dev_tools+existing");
    assert_eq!(json["kind"], "notebook_sub_note");
    assert_eq!(json["notebook"]["parent_block_id"], "existing");
    let output =
        run_capture(&vault, "Faster with -j @dev_tools+existing", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("- Faster with -j"));
    assert!(!after.contains("Faster with -j [created::"));
}

#[test]
fn test_forced_route_section_and_task() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "text", &["-r", "dev_tools"], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "notebook_note");
    let output = run_capture(
        &vault,
        "text",
        &["-r", "dev_tools", "--section", "Git"],
        false,
    );
    assert_success(&output);
    let output = run_capture(&vault, "parent @dev_tools^myparent", &[], true);
    assert_success(&output);
    let output = run_capture(
        &vault,
        "child text",
        &["-r", "dev_tools", "-t", "myparent"],
        false,
    );
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "notebook_sub_note");
    assert_eq!(json["notebook"]["parent_block_id"], "myparent");
}

#[test]
fn test_list_and_global_inheritance() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "@dev_tools\n- a\n- b", &[], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    let output = run_capture(
        &vault,
        "https://example.com/global-note\n\n@@dev_tools",
        &[],
        false,
    );
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "notebook_note");
    assert_eq!(json["text"], "https://example.com/global-note");
    assert!(captures.iter().all(|item| item["kind"] == "notebook_note"));
    let output =
        run_capture(&vault, "Idea A\n\nIdea B\n\n@@dev_tools", &[], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
}

#[test]
fn test_url_is_plain_note_with_no_ref_job() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "https://example.com/post @dev_tools");
    assert_eq!(json["kind"], "notebook_note");
    assert_eq!(json["text"], "https://example.com/post");
}

#[test]
fn test_mixed_task_and_note_is_atomic() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let before_notebook = read_notebook(&vault);
    let before_dev =
        fs::read_to_string(vault.join("dev.md")).expect("read dev");
    let output = run_capture(
        &vault,
        "Good task @dev\n\nBad note @dev_tools#gi",
        &[],
        true,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(read_notebook(&vault), before_notebook);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before_dev
    );
}

#[test]
fn test_duplicate_ids_refuse() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "dup @dev_tools^existing", &[], false);
    // Duplicate block IDs are IO-guarded, like ordinary captures.
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn test_later_items_see_earlier_additions() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(
        &vault,
        "First @dev_tools^newid\n\nChild @dev_tools+newid",
        &[],
        true,
    );
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("^newid"));
    assert!(after.contains("- Child"));
}

#[test]
fn test_fenced_ids_are_not_parents() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# Developer tools\n\n```\n- Fake ^fenced\n```\n\n- Real [created::2026-11-08] ^real\n",
    );
    let output = run_capture(&vault, "child @dev_tools+fenced", &[], false);
    assert_eq!(output.status.code(), Some(2));
    let json = dry_single(&vault, "child @dev_tools+real");
    assert_eq!(json["notebook"]["parent_block_id"], "real");
}

#[test]
fn test_notebook_insertion_preserves_line_endings_and_final_newline() {
    for (ending, has_final_newline) in
        [("\n", true), ("\r\n", true), ("\r\n", false)]
    {
        let temp = TempDir::new("bob-cli-notebook-line-endings");
        let vault = vault_with_notebook(&temp);
        let before = format!(
            "---{ending}type: \"[[notebook]]\"{ending}---{ending}{ending}# Notes{ending}{ending}## Git{ending}{ending}- Existing ^existing{ending}"
        );
        let before = if has_final_newline {
            before
        } else {
            before.trim_end_matches(ending).to_string()
        };
        fs::write(vault.join("dev_tools.md"), before.as_bytes())
            .expect("write notebook bytes");
        let output = run_capture(&vault, "New note @dev_tools", &[], true);
        assert_success(&output);
        let after = fs::read(vault.join("dev_tools.md")).expect("read bytes");
        let text = String::from_utf8(after.clone()).expect("UTF-8 notebook");
        assert!(text.contains(&format!(
            "- New note [created::{DATE}]{ending}{ending}## Git"
        )));
        assert_eq!(
            after.windows(2).filter(|bytes| *bytes == b"\r\n").count(),
            text.matches("\r\n").count()
        );
        if ending == "\r\n" {
            assert!(!text.replace("\r\n", "").contains('\n'));
        } else {
            assert!(!text.contains('\r'));
        }
        assert_eq!(text.ends_with(ending), has_final_newline);
    }
}

#[test]
fn test_fenced_headings_do_not_bound_notebook_sections() {
    let temp = TempDir::new("bob-cli-notebook-fenced-headings");
    let vault = vault_with_notebook(&temp);
    let before = "---\ntype: \"[[notebook]]\"\n---\n\n# Notes\n\n```md\n## Fake\n- Fake ^fenced\n```\n\n## Git\n\n- Existing ^existing\n".to_string();
    fs::write(vault.join("dev_tools.md"), before).expect("write notebook");
    let output = run_capture(&vault, "New note @dev_tools", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("- New note [created::2026-11-09]\n\n## Git"));
    assert!(after.contains("```md\n## Fake\n- Fake ^fenced\n```"));
}

#[test]
fn test_notebook_section_stops_at_the_next_real_h1() {
    let temp = TempDir::new("bob-cli-notebook-section-h1");
    let vault = vault_with_notebook(&temp);
    fs::write(
        vault.join("dev_tools.md"),
        "---\ntype: \"[[notebook]]\"\n---\n\n# Notes\n\n## Git\n\n- Existing ^existing\n\n# Next topic\n\n- Keep below ^below\n",
    )
    .expect("write notebook");
    let output = run_capture(&vault, "New note @dev_tools#Git", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains(
        "- Existing ^existing\n\n- New note [created::2026-11-09]\n\n# Next topic"
    ));
}

#[test]
fn test_nested_parent_and_authored_sub_bullets_keep_depth() {
    let temp = TempDir::new("bob-cli-notebook-nested-parent");
    let vault = vault_with_notebook(&temp);
    let contents = "---\ntype: \"[[notebook]]\"\n---\n\n# Notes\n\n- Grandparent ^grand\n  - Intermediate\n    - Parent #task ^parent\n      - Existing detail\n  - Sibling ^sibling\n";
    fs::write(vault.join("dev_tools.md"), contents).expect("write notebook");
    let output = run_capture(
        &vault,
        "Child @dev_tools+parent\n- first\n  - nested\n",
        &[],
        true,
    );
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains(
        "    - Parent #task ^parent\n      - Existing detail\n      - Child\n        - first\n          - nested"
    ));
    assert!(after.contains("  - Sibling ^sibling"));
}

#[test]
fn test_duplicate_anchors_and_invalid_parents_are_rejected() {
    let temp = TempDir::new("bob-cli-notebook-anchor-rules");
    let vault = vault_with_notebook(&temp);
    let original = read_notebook(&vault);
    fs::write(
        vault.join("dev_tools.md"),
        original.replace(
            "## Git\n\n- Existing note [created::2026-11-08] ^existing\n",
            "## Heading ^same\n\n- Existing note ^same\n- [ ] Task ^task\n- Prose mentions #task ^prose\n- Literal `^inline` and [[linked#^wiki]] ^real\n\n## Another heading ^heading\n",
        ),
    )
    .expect("write notebook");
    let duplicate = run_capture(&vault, "Child @dev_tools+same", &[], false);
    assert_eq!(duplicate.status.code(), Some(1));
    for id in ["task", "heading"] {
        let output =
            run_capture(&vault, &format!("Child @dev_tools+{id}"), &[], false);
        assert_eq!(output.status.code(), Some(2), "{id}");
    }
    let prose = dry_single(&vault, "Child @dev_tools+prose");
    assert_eq!(prose["notebook"]["parent_block_id"], "prose");
    let real = dry_single(&vault, "Child @dev_tools+real");
    assert_eq!(real["notebook"]["parent_block_id"], "real");
}

#[test]
fn test_capture_parse_uses_shared_notebook_wire_span_labels() {
    let temp = TempDir::new("bob-cli-notebook-span-labels");
    let vault = vault_with_notebook(&temp);
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Keep this @dev_tools#Git^idea")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    let spans = value["spans"].as_array().expect("spans");
    for (index, label) in
        ["route", "section", "task_block_id"].iter().enumerate()
    {
        assert_eq!(spans[index]["kind"], *label);
        assert!(spans[index]["notebook"].as_bool().unwrap());
    }
    assert_eq!(spans.len(), 3);

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Child @dev_tools+existing")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run capture-parse for sub-note");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    let span = value["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .find(|span| span["kind"] == "sub_bullet_block_id")
        .expect("parent block ID span");
    assert!(span["notebook"].as_bool().unwrap());

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Keep this @dev#Git^idea")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run capture-parse for area note");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    let spans = value["spans"].as_array().expect("spans");
    assert_eq!(spans[0]["kind"], "route");
    assert_eq!(spans[1]["kind"], "section");
    assert_eq!(spans[2]["kind"], "task_block_id");
    assert!(spans.iter().all(|span| span.get("notebook").is_none()));
}

#[test]
fn test_notebook_checkbox_guard_covers_generated_authored_and_clip_rows() {
    let temp = TempDir::new("bob-cli-notebook-checkbox-guard");
    let vault = vault_with_notebook(&temp);
    for draft in [
        "[ ] generated checkbox @dev_tools",
        "Note @dev_tools\n- [ ] authored checkbox",
        "@dev_tools#Git\n- [ ] list checkbox",
    ] {
        let before = read_notebook(&vault);
        let output = run_capture(&vault, draft, &[], true);
        assert_eq!(output.status.code(), Some(2), "{draft}");
        assert_eq!(read_notebook(&vault), before, "{draft}");
    }

    let clipboard = temp.path().join("clipboard");
    write_executable(
        &clipboard,
        "#!/bin/sh\nprintf '%s\\n' '- [ ] clipped checkbox'\n",
    );
    let before = read_notebook(&vault);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--clip")
        .arg("--")
        .arg("Note @dev_tools")
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("run notebook clipboard capture");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(read_notebook(&vault), before);
}

fn vault_with_duplicate_git(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# Developer tools\n\n## Git\n\n- First [created::2026-11-08]\n\n## Git\n\n- Second [created::2026-11-08]\n",
    );
    vault
}

#[test]
fn test_ambiguous_duplicate_section_is_refused() {
    let temp = TempDir::new("bob-cli-capture-notebook-ambiguous");
    let vault = vault_with_duplicate_git(&temp);
    let draft = "Try mergiraf @dev_tools#git";
    let parse = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DIR", &vault)
        .output()
        .expect("run capture-parse");
    assert_success(&parse);
    let parsed: Value =
        serde_json::from_str(stdout(&parse).trim()).expect("parse JSON");
    let diagnostic = parsed["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|entry| entry["code"] == "notebook_section_ambiguous")
        .expect("ambiguous diagnostic");
    let message = diagnostic["message"].as_str().expect("message");
    assert!(
        message.contains("dev_tools.md has 2 sections titled “Git”"),
        "{message}"
    );

    let complete = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture-complete");
    assert_success(&complete);
    let menu: Value =
        serde_json::from_str(stdout(&complete).trim()).expect("complete JSON");
    let rows = menu["candidates"].as_array().expect("candidates");
    let refused: Vec<&Value> =
        rows.iter().filter(|row| row["title"] == "Git").collect();
    assert_eq!(refused.len(), 2, "{rows:?}");
    for row in refused {
        assert!(
            row["refused"]
                .as_str()
                .is_some_and(|text| text.contains("rename one")),
            "{row}"
        );
    }

    let dry = run_capture(&vault, draft, &[], false);
    assert_eq!(dry.status.code(), Some(2));
    let refusal: Value =
        serde_json::from_str(stdout(&dry).trim()).expect("refusal JSON");
    assert_eq!(refusal["code"], "notebook_section_ambiguous");
    let before = read_notebook(&vault);
    let write = run_capture(&vault, draft, &[], true);
    assert_eq!(write.status.code(), Some(2));
    assert_eq!(read_notebook(&vault), before);
}

#[test]
fn test_notebook_list_summary_says_notes() {
    let temp = TempDir::new("bob-cli-capture-notebook-list-noun");
    let vault = vault_with_notebook(&temp);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("@dev_tools\n- first idea\n- second idea")
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0")
        .output()
        .expect("run human notebook list");
    assert_success(&output);
    assert!(
        stdout(&output).contains("list  dev_tools.md · 2 notes"),
        "notebook lists count notes:\n{}",
        format_output(&output)
    );
}

#[test]
fn test_section_bullets_file_nested_notes_and_keep_top_siblings() {
    let temp = TempDir::new("bob-cli-capture-notebook-section-bullets");
    let vault = vault_with_notebook(&temp);
    let before = read_notebook(&vault);
    let draft = "@dev_tools\n- #Git\n  - Use worktrees ^worktrees\n- Prefer fd over find\n  - Keep this detail with the top note";
    let json = dry_json_captures(&vault, draft);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["text"], "Use worktrees");
    assert_eq!(captures[0]["notebook"]["section"], "Git");
    assert_eq!(captures[0]["notebook"]["section_action"], "existing");
    assert_eq!(captures[0]["notebook"]["block_id"], "worktrees");
    assert_eq!(captures[0]["fanout"]["marker"], "#Git");
    assert_eq!(captures[0]["fanout"]["position"], 1);
    assert_eq!(captures[0]["fanout"]["count"], 1);
    assert_eq!(captures[1]["text"], "Prefer fd over find");
    assert!(captures[1]["notebook"]["section"].is_null());
    assert_eq!(captures[1]["list"]["count"], 2);
    assert_eq!(read_notebook(&vault), before);

    let output = run_capture(&vault, draft, &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    let prefer = after.find("- Prefer fd over find").expect("top note");
    let git = after.find("## Git").expect("git heading");
    let worktrees = after.find("- Use worktrees").expect("git note");
    assert!(prefer < git, "plain siblings stay in the top region");
    assert!(git < worktrees);
    assert!(after.contains("^worktrees"));
    assert!(after.contains("Keep this detail with the top note"));
}

#[test]
fn test_section_bullet_parse_needs_and_refusals() {
    let temp = TempDir::new("bob-cli-capture-notebook-section-parse");
    let vault = vault_with_notebook(&temp);
    let parse = |draft: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .env("BOB_DIR", &vault)
            .output()
            .expect("run capture-parse");
        assert_success(&output);
        serde_json::from_str::<Value>(stdout(&output).trim())
            .expect("parse JSON")
    };

    let bare = parse("@dev_tools\n- #");
    assert_eq!(bare["grammar"], 26);
    assert_eq!(bare["mode"], "incomplete");
    assert_eq!(bare["needs"], serde_json::json!(["section"]));
    assert!(bare["diagnostics"]
        .as_array()
        .is_none_or(|rows| rows.is_empty()));
    assert!(bare["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|span| { span["kind"] == "interactive_placeholder" }));

    let empty = parse("@dev_tools\n- #Git");
    assert_eq!(empty["mode"], "incomplete");
    assert_eq!(empty["needs"], serde_json::json!(["section_notes"]));
    let section = empty["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|span| span["kind"] == "section")
        .expect("section span");
    assert_eq!(section["title"], "Git");
    assert_eq!(section["section_action"], "existing");

    let before = read_notebook(&vault);
    for (draft, code) in [
        ("@dev_tools\n- #Git leftover", "section_bullet_text"),
        (
            "@dev_tools#Git\n- #Docker\n  - x",
            "section_bullet_position",
        ),
        (
            "@dev_tools\n- #Git\n  - one line\n    - too deep",
            "section_bullet_depth",
        ),
    ] {
        let dry = run_capture(&vault, draft, &[], false);
        assert_eq!(dry.status.code(), Some(2), "{draft}");
        let value: Value =
            serde_json::from_str(stdout(&dry).trim()).expect("refusal");
        assert_eq!(value["code"], code, "{draft}");
        let write = run_capture(&vault, draft, &[], true);
        assert_eq!(write.status.code(), Some(2), "{draft}");
        assert_eq!(read_notebook(&vault), before, "{draft}");
    }
}

fn dry_json_captures(vault: &std::path::Path, draft: &str) -> Value {
    let output = run_capture(vault, draft, &[], false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}
