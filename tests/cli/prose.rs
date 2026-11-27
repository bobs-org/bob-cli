//! CLI coverage for article formatting, reporting, and completion.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

fn prose_named_command(fixture: &Fixture, name: &str) -> Command {
    let mut command = bob_command();
    command
        .arg(name)
        .env("BOB_DIR", &fixture.vault)
        .env("NO_COLOR", "1");
    command
}

fn article_command(fixture: &Fixture) -> Command {
    prose_named_command(fixture, "articles")
}

fn prose_command(fixture: &Fixture) -> Command {
    prose_named_command(fixture, "prose")
}

fn extract_json(
    fixture: &Fixture,
    section: &str,
    extra_args: &[&str],
) -> Output {
    prose_command(fixture)
        .args(["extract", "owner", "-s", section, "-f", "json"])
        .args(extra_args)
        .output()
        .expect("run prose extract")
}

fn assert_extract_refusal(
    code: &str,
    host: &str,
    section: &str,
    extra_args: &[&str],
    extra_files: &[(&str, &str)],
) {
    let fixture = fixture();
    write_file(&fixture.vault.join("owner.md"), host);
    for (path, contents) in extra_files {
        write_file(&fixture.vault.join(path), contents);
    }
    let output = extract_json(&fixture, section, extra_args);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let value = json(&output);
    assert_eq!(value["status"], "refused", "{value}");
    let refusal = value["refusal"].as_object().expect("refusal object");
    for key in ["code", "message", "hint", "path", "line", "next"] {
        assert!(refusal.contains_key(key), "missing {key}: {value}");
    }
    assert_eq!(refusal["code"], code, "{value}");
    assert!(refusal["message"].as_str().is_some(), "{value}");
    assert!(refusal["hint"].as_str().is_some(), "{value}");
}

fn run_with_stdin(command: &mut Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command with stdin");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for command")
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn complete(vault: &Path, words: &[&str]) -> Output {
    let mut command = bob_completion_command();
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
    assert!(text.contains("Prose notes"), "{text}");
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
        article_command(&fixture).args(["format", "-s", "draft", "-f", "json"]),
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
fn all_writes_good_articles_when_another_article_is_refused() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("holds-tasks.md"),
        "---\ntype: [[article]]\n---\n\n- [ ] #task wait\n",
    );
    write_file(&fixture.vault.join("good.md"), ARTICLE);

    let output = article_command(&fixture)
        .args(["format", "-a", "-f", "json"])
        .output()
        .expect("format all with one refusal");

    assert_eq!(output.status.code(), Some(2));
    let value = json(&output);
    let good = value["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|note| note["path"] == "good.md")
        .expect("good article result");
    let refused = value["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|note| note["path"] == "holds-tasks.md")
        .expect("refused article result");
    assert_eq!(good["status"], "changed");
    assert_eq!(refused["status"], "skipped");
    assert_eq!(refused["reason"]["code"], "holds-tasks");
    assert!(refused["reason"]["message"]
        .as_str()
        .unwrap()
        .contains("#task on line 5"));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("good.md")).unwrap(),
        FORMATTED
    );
}

#[test]
fn non_utf8_markdown_does_not_break_list_or_format_all() {
    let fixture = fixture();
    write_file(&fixture.vault.join("good.md"), ARTICLE);
    fs::write(fixture.vault.join("binary.md"), [0xff, 0xfe, 0x00])
        .expect("write non-UTF-8 markdown");

    let list = article_command(&fixture)
        .output()
        .expect("list articles with non-UTF-8 note");
    assert_success(&list);
    assert!(stdout(&list).contains("good"));

    let format = article_command(&fixture)
        .args(["format", "-a"])
        .output()
        .expect("format articles with non-UTF-8 note");
    assert_success(&format);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("good.md")).unwrap(),
        FORMATTED
    );
}

#[test]
fn unresolved_notes_show_nearby_and_ambiguous_candidates() {
    let missing_fixture = fixture();
    write_file(&missing_fixture.vault.join("draft.md"), ARTICLE);
    let missing = article_command(&missing_fixture)
        .args(["format", "drfat"])
        .output()
        .expect("resolve a nearby missing note");
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        stderr(&missing).contains("near matches: draft"),
        "{}",
        stderr(&missing)
    );

    let ambiguous_fixture = fixture();
    write_file(&ambiguous_fixture.vault.join("a/note.md"), ARTICLE);
    write_file(&ambiguous_fixture.vault.join("b/note.md"), ARTICLE);
    let ambiguous = article_command(&ambiguous_fixture)
        .args(["format", "note"])
        .output()
        .expect("resolve an ambiguous note");
    assert_eq!(ambiguous.status.code(), Some(2));
    let message = stderr(&ambiguous);
    assert!(message.contains("is ambiguous"), "{message}");
    assert!(message.contains("a/note.md"), "{message}");
    assert!(message.contains("b/note.md"), "{message}");
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
            "prose",
            "format",
            "--bob-dir",
            fixture.vault.to_str().unwrap(),
            "",
        ],
    );

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "draft\tprose note · 2 sections\tprose\tspace\n"
    );
}

#[test]
fn article_help_exposes_the_cli_contract() {
    let output = prose_command(&fixture())
        .arg("--help")
        .output()
        .expect("article help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("Bare `bob prose`"), "{help}");
    assert!(help.contains("adopt"), "{help}");
    assert!(help.contains("format"), "{help}");
    assert!(help.contains("outline"), "{help}");
    assert!(help.contains("docs/prose.md"), "{help}");
    assert_text_order(
        &help,
        &["\n  adopt", "\n  format", "\n  list", "\n  outline"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn articles_alias_matches_prose_for_read_and_preview_commands() {
    let fixture = fixture();
    let prose = "---\ntype: [[prose]]\n---\n\n# Draft\n\n## 1. First\n\nTODO\n";
    write_file(&fixture.vault.join("draft.md"), prose);
    write_file(&fixture.vault.join("plain.md"), "# Plain\n\n## Section\n");

    for (args, label) in [
        (&["list"][..], "list"),
        (&["outline", "draft"][..], "outline"),
        (&["adopt", "plain"][..], "adopt preview"),
    ] {
        let mut old = article_command(&fixture);
        old.args(args);
        let mut canonical = prose_command(&fixture);
        canonical.args(args);
        let old = old.output().expect("run legacy alias");
        let canonical =
            canonical.output().expect("run canonical prose command");
        assert_eq!(
            old.status.code(),
            canonical.status.code(),
            "{label} status"
        );
        assert_eq!(stdout(&old), stdout(&canonical), "{label} stdout");
        assert_eq!(stderr(&old), stderr(&canonical), "{label} stderr");
    }

    let args = ["format", "-s", "draft", "-f", "json"];
    let mut old = article_command(&fixture);
    old.args(args);
    let mut canonical = prose_command(&fixture);
    canonical.args(args);
    let old = run_with_stdin(&mut old, prose);
    let canonical = run_with_stdin(&mut canonical, prose);
    assert_eq!(
        old.status.code(),
        canonical.status.code(),
        "stdin format status"
    );
    assert_eq!(stdout(&old), stdout(&canonical), "stdin format stdout");
    assert_eq!(stderr(&old), stderr(&canonical), "stdin format stderr");
}

#[test]
fn section_numbers_toggle_updates_metadata_and_headings_idempotently() {
    let fixture = fixture();
    let source = "---\ntype: [[prose]]\nsection_numbers: true\n---\n\n# 1. Draft\n\n## 1.1 Section\n\nTODO\n";
    write_file(&fixture.vault.join("draft.md"), source);

    let off = prose_command(&fixture)
        .args(["format", "-n", "off", "draft"])
        .output()
        .expect("turn section numbers off");
    assert_success(&off);
    let unnumbered =
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap();
    assert!(
        unnumbered.contains("section_numbers: false"),
        "{unnumbered}"
    );
    assert!(unnumbered.contains("# Draft"), "{unnumbered}");
    assert!(unnumbered.contains("## Section"), "{unnumbered}");

    let off_again = prose_command(&fixture)
        .args(["format", "-n", "off", "draft"])
        .output()
        .expect("repeat section numbers off");
    assert_success(&off_again);
    assert!(
        stdout(&off_again).contains("unchanged"),
        "{}",
        stdout(&off_again)
    );

    let on = prose_command(&fixture)
        .args(["format", "-n", "on", "draft"])
        .output()
        .expect("turn section numbers on");
    assert_success(&on);
    let numbered = fs::read_to_string(fixture.vault.join("draft.md")).unwrap();
    assert!(numbered.contains("section_numbers: true"), "{numbered}");
    assert!(numbered.contains("# 1. Draft"), "{numbered}");
    assert!(numbered.contains("## 1.1 Section"), "{numbered}");
}

#[test]
fn section_numbers_replaces_bad_metadata_and_is_in_the_stdin_plan() {
    let fixture = fixture();
    let bad =
        "---\ntype: [[prose]]\nsection_numbers: maybe\n---\n\n# 1. Draft\n";
    write_file(&fixture.vault.join("draft.md"), bad);
    let output = prose_command(&fixture)
        .args(["format", "--section-numbers", "off", "draft"])
        .output()
        .expect("replace bad section_numbers value");
    assert_success(&output);
    let written = fs::read_to_string(fixture.vault.join("draft.md")).unwrap();
    assert!(written.contains("section_numbers: false"), "{written}");
    assert!(!written.contains("section_numbers: maybe"), "{written}");
    assert!(written.contains("# Draft"), "{written}");

    let stdin_note = "---\ntype: [[prose]]\n---\n\n# 1. Draft\n";
    let mut command = prose_command(&fixture);
    command.args(["format", "-s", "draft", "-n", "off", "-f", "json"]);
    let planned = run_with_stdin(&mut command, stdin_note);
    assert_success(&planned);
    let planned = json(&planned);
    assert!(planned["notes"][0]["edits"].as_array().unwrap().len() >= 2);
    assert!(planned["notes"][0].get("formatted").is_none());

    let all_conflict = prose_command(&fixture)
        .args(["format", "-a", "-n", "off"])
        .output()
        .expect("section_numbers and all conflict");
    assert_eq!(all_conflict.status.code(), Some(2));
    let check_conflict = prose_command(&fixture)
        .args(["format", "-c", "-n", "off", "draft"])
        .output()
        .expect("section_numbers and check conflict");
    assert_eq!(check_conflict.status.code(), Some(2));
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
    assert!(text.contains("Adopted as a prose note"), "{text}");
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
    assert!(written.contains("type: \"[[prose]]\""));
    assert!(written.contains("# 1. Draft"));
    assert!(stdout(&output).contains("draft"), "{}", stdout(&output));

    let again = article_command(&fixture)
        .args(["adopt", "-w", "draft"])
        .output()
        .expect("adopt twice");
    assert_success(&again);
    assert!(
        stdout(&again).contains("already a prose note"),
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
        article_command(&fixture).args(["adopt", "-s", "plain", "-f", "json"]),
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
            .starts_with("Adopted as a prose note"),
        "{value}"
    );
    assert!(!value["notes"][0]["edits"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("plain.md")).unwrap(),
        source
    );
}

#[test]
fn inline_json_previews_parent_update_and_child_trash_without_writing() {
    let fixture = fixture();
    let parent = "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\n- [[notes/draft|Topic]]\n\n## Other\n\nKeep this text.\n";
    let child = "---\nparent: \"[[owner]]\"\ntype: \"[[prose]]\"\ncreated: 2026-11-27\ntitle: Topic\nsection_numbers: false\n---\n\n## Detail\n\nRestored prose.\n";
    write_file(&fixture.vault.join("owner.md"), parent);
    write_file(&fixture.vault.join("notes/draft.md"), child);

    let output = prose_command(&fixture)
        .args(["inline", "draft", "-f", "json"])
        .output()
        .expect("preview prose inline");

    assert_success(&output);
    let report = json(&output);
    assert_eq!(report["contract"], "prose-conversion");
    assert_eq!(report["command"], "inline");
    assert_eq!(report["status"], "preview");
    assert_eq!(report["target"]["path"], "owner.md");
    assert!(report["changes"].as_array().unwrap().iter().any(|change| {
        change["path"] == "notes/draft.md" && change["action"] == "trash"
    }));
    assert_eq!(
        fs::read_to_string(fixture.vault.join("owner.md")).unwrap(),
        parent
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("notes/draft.md")).unwrap(),
        child
    );
}

#[test]
fn inline_offline_write_trashes_child_and_commits_parent() {
    let fixture = fixture();
    let parent = "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\n- [[notes/draft|Topic]]\n\n## Other\n\nKeep this text.\n";
    let child = "---\nparent: \"[[owner]]\"\ntype: \"[[prose]]\"\ncreated: 2026-11-27\ntitle: Topic\nsection_numbers: false\n---\n\n## Detail\n\nRestored prose.\n";
    write_file(&fixture.vault.join("owner.md"), parent);
    write_file(&fixture.vault.join("notes/draft.md"), child);
    git_in(&fixture.vault, ["init", "-q"]);
    git_in(&fixture.vault, ["config", "user.email", "t@t"]);
    git_in(&fixture.vault, ["config", "user.name", "t"]);
    git_in(&fixture.vault, ["add", "-A"]);
    git_in(&fixture.vault, ["commit", "-qm", "seed"]);

    let output = prose_command(&fixture)
        .args(["inline", "draft", "-w", "-o", "-f", "json"])
        .output()
        .expect("write prose inline offline");

    assert_success(&output);
    let report = json(&output);
    assert_eq!(report["status"], "written");
    assert_eq!(report["command"], "inline");
    assert!(report["commit"]["sha"].as_str().is_some());
    let written = fs::read_to_string(fixture.vault.join("owner.md")).unwrap();
    assert!(written.contains("### Detail"), "{written}");
    assert!(written.contains("Restored prose."), "{written}");
    assert!(!fixture.vault.join("notes/draft.md").exists());
    assert!(fixture.vault.join(".trash").exists());
    assert!(
        stdout(&git_in(&fixture.vault, ["log", "-1", "--format=%s"]))
            .contains("bob prose inline: draft → owner § Topic")
    );
}

#[test]
fn extract_refusals_return_the_contract_object_through_the_cli() {
    let area = |body: &str| {
        format!(
            "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\n{body}\n\n## After\n\nKept.\n"
        )
    };

    assert_extract_refusal(
        "not-host-note",
        "---\ntype: \"[[notebook]]\"\n---\n\n## Topic\n\nText.\n",
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "no-section",
        "---\ntype: \"[[area]]\"\n---\n\n## Other\n\nText.\n",
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "ambiguous-section",
        "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\nOne.\n\n## Topic\n\nTwo.\n",
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "structural-section",
        "---\ntype: \"[[area]]\"\n---\n\n## Tasks\n\n- [ ] work\n",
        "Tasks",
        &[],
        &[],
    );
    assert_extract_refusal(
        "holds-tasks",
        &area("- [ ] #task do the work"),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "holds-anchor",
        &area("This prose owns the project anchor. ^prj"),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "conflict-markers",
        &area("<<<<<<< ours\nconflicting prose\n>>>>>>> theirs"),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "stub-section",
        &area("- [[child|Topic]]"),
        "Topic",
        &[],
        &[(
            "child.md",
            "---\nparent: \"[[owner]]\"\ntype: \"[[prose]]\"\n---\n\n## Detail\n\nText.\n",
        )],
    );
    assert_extract_refusal(
        "unclosed-fence",
        &area("```text\nnever closed"),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "setext-multiline",
        &area("First title line\nSecond title line\n---\n\nBody."),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "owned-number-heading",
        &area("### 1.5 million users\n\nBody."),
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "definition-boundary",
        "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\nThis uses [guide].\n\n## After\n\nKept.\n\n[guide]: /guide\n",
        "Topic",
        &[],
        &[],
    );
    assert_extract_refusal(
        "name-taken",
        &area("Text."),
        "Topic",
        &["-n", "already_used"],
        &[("already_used.md", "Existing note.\n")],
    );
    assert_extract_refusal(
        "bad-name",
        &area("Text."),
        "Topic",
        &["-n", "../outside"],
        &[],
    );
    assert_extract_refusal(
        "ambiguous-link",
        &area("### Shared\n\nOne.\n\n### Shared\n\nTwo."),
        "Topic",
        &[],
        &[("reader.md", "[[owner#Shared]]\n")],
    );
}

#[test]
fn extract_preview_json_human_write_and_fingerprint_follow_contract() {
    let fixture = fixture();
    let host = "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\n### Detail\n\nMoved text [[#Detail]].\n\n## Other\n\nKeep this text.\n";
    write_file(&fixture.vault.join("owner.md"), host);
    let reader = "[[owner#Detail|Read]]\n";
    write_file(&fixture.vault.join("reader.md"), reader);
    write_file(&fixture.vault.join(".bob/keep.md"), "managed bytes\n");
    write_file(&fixture.vault.join(".trash/keep.md"), "trash bytes\n");
    git_in(&fixture.vault, ["init", "-q"]);
    git_in(&fixture.vault, ["config", "user.email", "t@t"]);
    git_in(&fixture.vault, ["config", "user.name", "t"]);
    git_in(&fixture.vault, ["add", "-A"]);
    git_in(&fixture.vault, ["commit", "-qm", "seed"]);
    let initial_head = stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]));

    let preview = extract_json(&fixture, "Topic", &[]);
    assert_success(&preview);
    let value = json(&preview);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["contract"], "prose-conversion");
    assert_eq!(value["status"], "preview");
    assert!(value["changes"].as_array().is_some());
    assert!(value["changes"].as_array().unwrap().iter().all(|change| {
        change["action"].as_str().is_some() && change.get("sha256").is_some()
    }));
    assert!(value["links"].as_array().is_some());
    assert!(value["counts"].is_object());
    assert!(value["focus"]["path"].as_str().is_some());
    assert!(value["notice"].as_str().is_some());
    assert!(value["commit"].is_null());
    assert!(value["sync"]["pre"].as_str().is_some());
    assert_eq!(value["counts"]["links"], 1);
    assert!(!fixture.vault.join("owner_topic.md").exists());
    assert_eq!(
        fs::read_to_string(fixture.vault.join("owner.md")).unwrap(),
        host
    );

    let human = prose_command(&fixture)
        .args(["extract", "owner", "-s", "Topic", "-f", "human"])
        .output()
        .expect("human extract preview");
    assert_success(&human);
    assert!(stdout(&human).to_lowercase().contains("extract"));
    assert!(!fixture.vault.join("owner_topic.md").exists());

    let stale = prose_command(&fixture)
        .args([
            "extract",
            "owner",
            "-s",
            "Topic",
            "-w",
            "-o",
            "-e",
            "sha256:stale",
            "-f",
            "json",
        ])
        .output()
        .expect("stale extract write");
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(json(&stale)["refusal"]["code"], "plan-changed");
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])),
        initial_head
    );
    assert!(!fixture.vault.join("owner_topic.md").exists());

    // A linker changed after preview invalidates the complete conversion.
    write_file(&fixture.vault.join("reader.md"), "Concurrent edit.\n");
    let fingerprint = value["fingerprint"].as_str().expect("fingerprint");
    let drifted = prose_command(&fixture)
        .args([
            "extract",
            "owner",
            "-s",
            "Topic",
            "-w",
            "-o",
            "-e",
            fingerprint,
            "-f",
            "json",
        ])
        .output()
        .expect("write after linker drift");
    assert_eq!(drifted.status.code(), Some(2));
    assert_eq!(json(&drifted)["refusal"]["code"], "plan-changed");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("owner.md")).unwrap(),
        host
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("reader.md")).unwrap(),
        "Concurrent edit.\n"
    );
    assert!(!fixture.vault.join("owner_topic.md").exists());
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])),
        initial_head
    );
    write_file(&fixture.vault.join("reader.md"), reader);

    // A target appearing between preview and write is refused and preserved.
    write_file(
        &fixture.vault.join("owner_topic.md"),
        "Concurrent target.\n",
    );
    let raced = prose_command(&fixture)
        .args([
            "extract",
            "owner",
            "-s",
            "Topic",
            "-w",
            "-o",
            "-e",
            fingerprint,
            "-f",
            "json",
        ])
        .output()
        .expect("write after name collision");
    assert_eq!(raced.status.code(), Some(2));
    assert_eq!(json(&raced)["refusal"]["code"], "name-taken");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("owner_topic.md")).unwrap(),
        "Concurrent target.\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("owner.md")).unwrap(),
        host
    );
    fs::remove_file(fixture.vault.join("owner_topic.md"))
        .expect("remove racing target");

    let written = prose_command(&fixture)
        .args(["extract", "owner", "-s", "Topic", "-w", "-o", "-f", "json"])
        .output()
        .expect("write prose extract");
    assert_success(&written);
    let report = json(&written);
    assert_eq!(report["status"], "written");
    assert_eq!(
        report["commit"]["subject"],
        "bob prose extract: owner § Topic → owner_topic"
    );
    let new_note =
        fs::read_to_string(fixture.vault.join("owner_topic.md")).unwrap();
    let frontmatter = new_note.split("---").nth(1).expect("frontmatter");
    let keys = frontmatter
        .trim()
        .lines()
        .map(|line| line.split(':').next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "parent",
            "type",
            "created",
            "title",
            "extracted_from",
            "extracted_heading",
            "section_numbers"
        ]
    );
    assert!(new_note.contains("section_numbers: false"));
    assert!(fs::read_to_string(fixture.vault.join("reader.md"))
        .unwrap()
        .contains("[[owner_topic#Detail|Read]]"));
    assert_eq!(
        fs::read_to_string(fixture.vault.join(".bob/keep.md")).unwrap(),
        "managed bytes\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join(".trash/keep.md")).unwrap(),
        "trash bytes\n"
    );
}

#[test]
fn prose_extract_no_section_dispatches_to_inline_for_parented_prose() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("owner.md"),
        "---\ntype: \"[[area]]\"\n---\n\n## Topic\n\n- [[draft|Topic]]\n",
    );
    write_file(
        &fixture.vault.join("draft.md"),
        "---\nparent: \"[[owner]]\"\ntype: \"[[prose]]\"\n---\n\n## Detail\n\nText.\n",
    );
    let output = prose_command(&fixture)
        .args(["extract", "draft", "-l", "1", "-f", "json"])
        .output()
        .expect("toggle dispatch preview");
    assert_eq!(output.status.code(), Some(2));
    let value = json(&output);
    assert_eq!(value["refusal"]["code"], "no-section");
    assert_eq!(value["refusal"]["next"]["command"], "inline");
    assert_eq!(value["refusal"]["next"]["note"], "draft.md");
}

#[test]
fn extract_reformats_numbered_prose_host_and_disables_numbers_in_child() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("owner.md"),
        "---\ntype: \"[[prose]]\"\nsection_numbers: true\n---\n\n# 9. Owner\n\n## 9.8 Topic\n\n### 9.8.4 Detail\n\nMoved text.\n\n## 9.9 Other\n\nKept text.\n",
    );
    let output = prose_command(&fixture)
        .args(["extract", "owner", "-s", "Topic", "-f", "json"])
        .output()
        .expect("preview numbered extraction");
    assert_success(&output);
    let report = json(&output);
    let host_after = report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "owner.md")
        .expect("host after-image")["diff"]
        .as_str()
        .unwrap();
    assert!(host_after.contains("+## 1.1 Topic"), "{host_after}");
    assert!(host_after.contains("+## 1.2 Other"), "{host_after}");
    let extracted = report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "owner_topic.md")
        .expect("child after-image");
    assert!(extracted["diff"]
        .as_str()
        .unwrap()
        .contains("section_numbers: false"));
    assert!(extracted["diff"].as_str().unwrap().contains("## Detail"));
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

#[test]
fn format_rewrites_inbound_links_in_two_other_notes() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);
    write_file(
        &fixture.vault.join("journal.md"),
        "See [[draft#9. First]].\n",
    );
    write_file(
        &fixture.vault.join("log.md"),
        "[First](draft.md#9.%20First)\n",
    );

    let output = article_command(&fixture)
        .args(["format", "draft"])
        .output()
        .expect("format with inbound");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        FORMATTED
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("journal.md")).unwrap(),
        "See [[draft#1.1 First]].\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("log.md")).unwrap(),
        "[First](draft.md#1.1%20First)\n"
    );
}

#[test]
fn format_rewrites_inbound_embeds() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);
    write_file(&fixture.vault.join("journal.md"), "![[draft#9. First]]\n");

    let output = article_command(&fixture)
        .args(["format", "draft"])
        .output()
        .expect("format with inbound embed");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("journal.md")).unwrap(),
        "![[draft#1.1 First]]\n"
    );
}

#[test]
fn format_reports_unrepairable_inbound_and_resolves_duplicate_titles() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("note.md"),
        "---\ntype: \"[[article]]\"\n---\n\n## 9. Examples\n\n## 8. Examples\n",
    );
    write_file(&fixture.vault.join("a.md"), "[[note#9. Examples]]\n");
    write_file(&fixture.vault.join("b.md"), "[[note#Examples]]\n");
    write_file(&fixture.vault.join("gone.md"), "[[note#Missing]]\n");

    let output = article_command(&fixture)
        .args(["format", "note", "-f", "json"])
        .output()
        .expect("format duplicates");
    assert_success(&output);
    let value = json(&output);
    let inbound = value["notes"][0]["inbound"].as_array().unwrap();
    assert!(
        inbound.iter().any(|item| {
            item["path"] == "a.md"
                && item["after"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("[[note#1. Examples]]")
        }),
        "{value}"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("a.md")).unwrap(),
        "[[note#1. Examples]]\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("b.md")).unwrap(),
        "[[note#Examples]]\n"
    );
    let findings = value["notes"][0]["findings"].as_array().unwrap();
    assert!(
        findings.iter().any(|finding| {
            finding["code"] == "inbound-heading-link"
                && finding["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("b.md:1")
        }),
        "{value}"
    );
    assert!(
        findings.iter().any(|finding| {
            finding["code"] == "inbound-heading-link"
                && finding["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("gone.md:1")
        }),
        "{value}"
    );
}

#[test]
fn stdin_json_fills_inbound_entries_without_writing() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);
    write_file(
        &fixture.vault.join("journal.md"),
        "See [[draft#9. First]].\n",
    );

    let output = run_with_stdin(
        article_command(&fixture).args(["format", "-s", "draft", "-f", "json"]),
        ARTICLE,
    );
    assert_success(&output);
    let value = json(&output);
    let inbound = &value["notes"][0]["inbound"][0];
    assert_eq!(inbound["path"], "journal.md");
    assert_eq!(inbound["line"], 1);
    assert!(inbound["sha256"].as_str().is_some());
    assert_eq!(inbound["before"], "See [[draft#9. First]].");
    assert_eq!(inbound["after"], "See [[draft#1.1 First]].");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("journal.md")).unwrap(),
        "See [[draft#9. First]].\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("draft.md")).unwrap(),
        ARTICLE
    );
}

#[test]
fn dry_run_shows_inbound_diffs_without_writing() {
    let fixture = fixture();
    write_file(&fixture.vault.join("draft.md"), ARTICLE);
    write_file(
        &fixture.vault.join("journal.md"),
        "See [[draft#9. First]].\n",
    );

    let output = article_command(&fixture)
        .args(["format", "-d", "draft"])
        .output()
        .expect("dry-run inbound");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("--- a/journal.md"), "{text}");
    assert!(text.contains("+See [[draft#1.1 First]]."), "{text}");
    assert_eq!(
        fs::read_to_string(fixture.vault.join("journal.md")).unwrap(),
        "See [[draft#9. First]].\n"
    );
}
