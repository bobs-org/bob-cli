//! CLI coverage for article formatting, reporting, and completion.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const ARTICLE: &str = "---\ntype: \"[[article]]\"\n---\n\n[[#9. First]]\n\n# Draft\n\n## 9. First\n\nTODO\n";
const FORMATTED: &str = "---\ntype: \"[[article]]\"\n---\n\n[[#1.1 First]]\n\n# 1. Draft\n\n## 1.1 First\n\nTODO\n";

struct Fixture {
    _temp: TempDir,
    vault: PathBuf,
}

fn fixture() -> Fixture {
    let temp = TempDir::new("bob-cli-articles");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    Fixture { _temp: temp, vault }
}

fn article_command(fixture: &Fixture) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("articles")
        .env("BOB_DIR", &fixture.vault)
        .env("NO_COLOR", "1");
    command
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn complete(vault: &Path, words: &[&str]) -> Output {
    let mut command = bob_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1");
    for word in words {
        command.arg(word);
    }
    command.output().expect("run article completion")
}

#[test]
fn bare_articles_lists_only_marked_notes_and_honors_exclusions() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);
    write_file(&fixture.vault.join("plain.md"), "# Not an article\n");
    write_file(&fixture.vault.join("_templates/hidden.md"), ARTICLE);
    write_file(&fixture.vault.join(".obsidian/hidden.md"), ARTICLE);

    let output = article_command(&fixture)
        .output()
        .expect("run bob articles");

    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Articles"), "{text}");
    assert!(text.contains("draft"), "{text}");
    assert!(text.contains("2"), "{text}");
    assert!(!text.contains("plain"), "{text}");
    assert!(!text.contains("hidden"), "{text}");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn list_json_keeps_report_fields_but_omits_edits() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let output = article_command(&fixture)
        .args(["list", "-f", "json"])
        .output()
        .expect("run article list JSON");

    assert_success(&output);
    let value = json(&output);
    assert_eq!(value["summary"]["articles"], 1);
    assert_eq!(value["notes"][0]["status"], "changed");
    assert_eq!(
        value["notes"][0]["outline"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(value["notes"][0].get("edits").is_none());
    assert!(value["notes"][0]["findings"].is_array());
}

#[test]
fn format_writes_atomically_and_check_is_idempotent() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let output = article_command(&fixture)
        .args(["format", "draft"])
        .output()
        .expect("format article");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        FORMATTED
    );
    assert!(stdout(&output).contains("draft ·"), "{}", stdout(&output));

    let check = article_command(&fixture)
        .args(["format", "-c", "draft"])
        .output()
        .expect("check formatted article");
    assert_success(&check);
    assert!(stdout(&check).is_empty(), "{}", stdout(&check));
}

#[test]
fn check_and_dry_run_report_changes_without_writing() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let check = article_command(&fixture)
        .args(["format", "-c", "draft"])
        .output()
        .expect("check article");
    assert_eq!(check.status.code(), Some(1));
    assert!(stdout(&check).contains("needs formatting"));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        ARTICLE
    );

    let dry_run = article_command(&fixture)
        .args(["format", "-d", "draft"])
        .output()
        .expect("dry-run article");
    assert_success(&dry_run);
    assert!(stdout(&dry_run).contains("--- a/draft.md"));
    assert!(stdout(&dry_run).contains("+## 1.1 First"));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        ARTICLE
    );
}

#[test]
fn stdin_json_returns_plan_without_writing() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let output = run_with_stdin(
        &mut article_command(&fixture)
            .args(["format", "-s", "draft", "-f", "json"]),
        ARTICLE,
    );

    assert_success(&output);
    let value = json(&output);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["notes"][0]["status"], "changed");
    assert!(value["notes"][0]["edits"].is_array());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        ARTICLE
    );
}

#[test]
fn all_formats_every_article_and_skips_untyped_notes() {
    let fixture = fixture();
    write_file(&fixture.vault.join("a.md"), ARTICLE);
    write_file(&fixture.vault.join("b.md"), FORMATTED);
    write_file(&fixture.vault.join("plain.md"), "# Plain\n");
    write_file(&fixture.vault.join("_generated/hidden.md"), ARTICLE);

    let output = article_command(&fixture)
        .args(["format", "--all", "-f", "json"])
        .output()
        .expect("format all articles");

    assert_success(&output);
    let value = json(&output);
    assert_eq!(value["summary"]["articles"], 2);
    assert_eq!(value["summary"]["changed"], 1);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("a.md")).unwrap(),
        FORMATTED
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("b.md")).unwrap(),
        FORMATTED
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("_generated/hidden.md")).unwrap(),
        ARTICLE
    );
}

#[test]
fn named_non_article_reports_refusal_in_json() {
    let fixture = fixture();
    write_file(&fixture.vault.join("plain.md"), "# Plain\n");

    let output = article_command(&fixture)
        .args(["format", "plain", "-f", "json"])
        .output()
        .expect("format a non-article");

    assert_eq!(output.status.code(), Some(2));
    let value = json(&output);
    assert_eq!(value["notes"][0]["status"], "skipped");
    assert_eq!(value["notes"][0]["reason"]["code"], "not-article");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("plain.md")).unwrap(),
        "# Plain\n"
    );
}

#[test]
fn outline_json_and_human_show_numbering_and_stubs() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let json_output = article_command(&fixture)
        .args(["outline", "draft", "-f", "json"])
        .output()
        .expect("outline JSON");
    assert_success(&json_output);
    let outline = json(&json_output);
    assert_eq!(outline["title"], "draft");
    assert_eq!(outline["sections"][1]["number"], "1.1");
    assert_eq!(outline["sections"][1]["stub"], true);

    let human = article_command(&fixture)
        .args(["outline", "draft"])
        .output()
        .expect("outline human");
    assert_success(&human);
    assert!(stdout(&human).contains("1.1"));
    assert!(stdout(&human).contains("TODO"));
}

#[test]
fn article_note_completion_shows_section_count() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);

    let output = complete(
        &fixture.vault,
        &[
            "bob",
            "articles",
            "format",
            "--bob-dir",
            fixture.vault.to_str().unwrap(),
            "",
        ],
    );

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "draft\tarticle · 2 sections\tarticles\tspace\n"
    );
}

#[test]
fn article_help_exposes_the_cli_contract() {
    let output = article_command(&fixture())
        .arg("--help")
        .output()
        .expect("article help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("Bare `bob articles`"), "{help}");
    assert!(help.contains("adopt"), "{help}");
    assert!(help.contains("format"), "{help}");
    assert!(help.contains("outline"), "{help}");
    assert!(help.contains("docs/articles.md"), "{help}");
    assert_text_order(
        &help,
        &["\n  adopt", "\n  format", "\n  list", "\n  outline"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn adopt_previews_without_writing_and_reports_replaced_numbers() {
    let fixture = fixture();
    let source = "---\nparent: \"[[home]]\"\n---\n\n# Draft\n\n## 3. Reasons\n";
    write_file(&fixture.vault.join("draft.md"), source);

    let output = article_command(&fixture)
        .args(["adopt", "draft"])
        .output()
        .expect("preview adopt");

    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Adopted as an article"), "{text}");
    assert!(text.contains("Numbers replaced"), "{text}");
    assert!(text.contains("## 3. Reasons"), "{text}");
    assert!(text.contains("1.1"), "{text}");
    assert!(
        text.contains("Preview only · rerun with -w to write"),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        source
    );
}

#[test]
fn adopt_write_inserts_type_and_adopting_twice_is_a_noop() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("draft.md"),
        "---\nparent: \"[[home]]\"\n---\n\n# Draft\n\n## First\n",
    );

    let output = article_command(&fixture)
        .args(["adopt", "-w", "draft"])
        .output()
        .expect("write adopt");
    assert_success(&output);
    let written = fs::read_to_string(fixture.vault.join("draft.md")).unwrap();
    assert!(written.contains("type: \"[[article]]\""));
    assert!(written.contains("# 1. Draft"));
    assert!(stdout(&output).contains("draft"), "{}", stdout(&output));

    let again = article_command(&fixture)
        .args(["adopt", "-w", "draft"])
        .output()
        .expect("adopt twice");
    assert_success(&again);
    assert!(
        stdout(&again).contains("already an article"),
        "{}",
        stdout(&again)
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        written
    );
}

#[test]
fn adopt_stdin_json_returns_plan_without_writing() {
    let fixture = fixture();
    let source = "# Title\n";
    write_file(&fixture.vault.join("plain.md"), source);

    let output = run_with_stdin(
        &mut article_command(&fixture)
            .args(["adopt", "-s", "plain", "-f", "json"]),
        source,
    );

    assert_success(&output);
    let value = json(&output);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["notes"][0]["status"], "changed");
    assert!(
        value["notes"][0]["notice"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Adopted as an article"),
        "{value}"
    );
    assert!(!value["notes"][0]["edits"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("plain.md")).unwrap(),
        source
    );
}

#[test]
fn adopt_refuses_other_types_and_tasks() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("meal.md"),
        "---\ntype: \"[[restaurant]]\"\n---\n\n# Dinner\n",
    );
    write_file(
        &fixture.vault.join("todo.md"),
        "---\nparent: \"[[home]]\"\n---\n\n- [ ] #task pack\n",
    );

    let restaurant = article_command(&fixture)
        .args(["adopt", "meal", "-f", "json"])
        .output()
        .expect("adopt restaurant");
    assert_eq!(restaurant.status.code(), Some(2));
    let value = json(&restaurant);
    assert_eq!(value["notes"][0]["status"], "skipped");
    assert_eq!(value["notes"][0]["reason"]["code"], "mixed-type");
    assert_eq!(
        value["notes"][0]["reason"]["message"],
        "note already has type [[restaurant]]"
    );

    let tasks = article_command(&fixture)
        .args(["adopt", "todo", "-f", "json"])
        .output()
        .expect("adopt tasks");
    assert_eq!(tasks.status.code(), Some(2));
    let value = json(&tasks);
    assert_eq!(value["notes"][0]["reason"]["code"], "holds-tasks");
}

#[test]
fn adopt_help_lists_options_alphabetically() {
    let output = article_command(&fixture())
        .args(["adopt", "--help"])
        .output()
        .expect("adopt help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Insert type:") && help.contains("--write"),
        "{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-f, --format",
            "-s, --stdin",
            "-w, --write",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn adopt_note_completion_is_a_vault_note() {
    let fixture = fixture();
    write_file(&fixture.vault.join("plain.md"), "# Plain\n");

    let output = complete(
        &fixture.vault,
        &[
            "bob",
            "articles",
            "adopt",
            "--bob-dir",
            fixture.vault.to_str().unwrap(),
            "",
        ],
    );

    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("!files-in"), "{text}");
    assert!(text.contains("*.md"), "{text}");
}
