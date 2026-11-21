//! `bob task rename-drawer`: dry-run-first rename of task drawers and `^prj`
//! sections, with heading-link repair.

use crate::support::*;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// An open task whose REQUIREMENTS meets an existing NOTES, and a done
/// task with a lone REQUIREMENTS drawer.
const BOB: &str = "\
## Tasks
- [ ] #task Ship the exporter ^export
\t- REQUIREMENTS
\t\t- Handles supported inputs ^inputs
\t- NOTES
\t\t- Morgan reported the issue.
- [x] #task Shipped ✅ 2026-11-20
\t- REQUIREMENTS
\t\t- [x] checkbox bytes stay
";

const BOB_RENAMED: &str = "\
## Tasks
- [ ] #task Ship the exporter ^export
\t- NOTES
\t\t- Handles supported inputs ^inputs
\t\t- Morgan reported the issue.
- [x] #task Shipped ✅ 2026-11-20
\t- NOTES
\t\t- [x] checkbox bytes stay
";

const DONE: &str = "\
- [x] #task Archived ✅ 2026-01-02
\t- REQUIREMENTS
\t\t- archived entry
";

const PROJ: &str = "\
---
type: \"[[project]]\"
---
# Proj

- [ ] #task Finish proj ^prj

## Requirements

- Inspired by [[bob]]!
";

const LINKS: &str = "\
- See [[proj#Requirements]] and ![[proj#Requirements|reqs]].
- Markdown [p](proj.md#Requirements) and block [[proj#^prj]].
- Elsewhere [[area#Requirements]] stays.
- Refused [[amb#Requirements]] stays.
";

const LINKS_RENAMED: &str = "\
- See [[proj#Notes]] and ![[proj#Notes|reqs]].
- Markdown [p](proj.md#Notes) and block [[proj#^prj]].
- Elsewhere [[area#Requirements]] stays.
- Refused [[amb#Requirements]] stays.
";

/// An ordinary heading outside a project note: excluded.
const AREA: &str = "# Area\n\n## Requirements\n\n- area requirement\n";

/// A project note whose rename would leave a heading ambiguous: refused.
const AMB: &str = "\
---
type: \"[[project]]\"
---
## Requirements

- a

### Requirements
";

const TEMPLATE: &str = "---\ntype: \"[[project]]\"\n---\n## Requirements\n";
const GENERATED: &str = "- [ ] #task Generated\n\t- REQUIREMENTS\n\t\t- x\n";
const CLEAN: &str = "- [ ] #task Already clean\n\t- NOTES\n\t\t- a\n";

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

/// The sync pair also seeds `initial.md` with one open task.
fn fixture(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write(&vault, "bob.md", BOB);
    write(&vault, "done/bob_done.md", DONE);
    write(&vault, "proj.md", PROJ);
    write(&vault, "links.md", LINKS);
    write(&vault, "area.md", AREA);
    write(&vault, "amb.md", AMB);
    write(&vault, "clean.md", CLEAN);
    write(&vault, "_templates/new_project.md", TEMPLATE);
    write(&vault, "_generated/view.md", GENERATED);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "rename-drawer fixture"]);
    git_in(&vault, ["push", "-q"]);
    vault
}

fn rename_command(vault: &Path, temp: &TempDir, args: &[&str]) -> Command {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("home");
    let mut command = bob_command();
    command
        .args(["task", "rename-drawer"])
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-11-21 09:14:00")
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command
}

fn run(vault: &Path, temp: &TempDir, args: &[&str]) -> Output {
    rename_command(vault, temp, args)
        .output()
        .expect("run bob task rename-drawer")
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("expected JSON ({error}):\n{}", format_output(output))
    })
}

fn head(vault: &Path) -> String {
    stdout(&git_in(vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_owned()
}

fn subject(vault: &Path) -> String {
    stdout(&git_in(vault, ["log", "-1", "--format=%s"]))
        .trim()
        .to_owned()
}

fn head_paths(vault: &Path) -> String {
    stdout(&git_in(
        vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ))
    .trim()
    .to_owned()
}

fn read(vault: &Path, path: &str) -> String {
    fs::read_to_string(vault.join(path)).expect("read vault file")
}

fn change_paths(payload: &Value) -> Vec<String> {
    payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect()
}

fn assert_untouched(vault: &Path) {
    assert_eq!(read(vault, "area.md"), AREA);
    assert_eq!(read(vault, "amb.md"), AMB);
    assert_eq!(read(vault, "clean.md"), CLEAN);
    assert_eq!(read(vault, "_templates/new_project.md"), TEMPLATE);
    assert_eq!(read(vault, "_generated/view.md"), GENERATED);
}

#[test]
fn bare_run_is_a_dry_run_with_an_inventory() {
    let temp = TempDir::new("rename-drawer-dry");
    let vault = fixture(&temp);
    let before = head(&vault);

    let human = run(&vault, &temp, &["REQUIREMENTS", "notes"]);
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    let text = stdout(&human);
    for expected in [
        "bob.md (2 drawers · 1 merge)",
        "L2 Ship the exporter (open) · REQUIREMENTS + NOTES → NOTES (merged)",
        "    - \t- REQUIREMENTS",
        "    + \t- NOTES",
        "done/bob_done.md (1 drawer) archive",
        "proj.md (1 section)",
        "L8 ## Requirements → ## Notes",
        "links.md (3 links)",
        "L1 [[proj#Requirements]] → [[proj#Notes]]",
        "L2 (proj.md#Requirements) → (proj.md#Notes)",
        "Refused (1 note, left unchanged)",
        "! amb.md: it also has `### Requirements` on line 8, so a link to #Requirements could not be repaired safely",
        "Excluded (2, left as written)",
        "· area.md:3 `## Requirements` — an ordinary heading outside a project note",
        "· amb.md:8 `### Requirements` — not a ## section",
        "Unresolved links (1, left as written)",
        "· links.md:4 [[amb#Requirements]] — its note was refused",
        "Templates (1, reported only; templates are never edited)",
        "· _templates/new_project.md:4 `## Requirements` (heading)",
        "! dry run: 3 drawers (1 open · 1 closed · 1 archived) · 1 project section · 1 merge · 3 links repaired in 4 notes; nothing was written. Rerun with -a to apply",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(!text.contains("_generated"), "{text}");

    let output = run(&vault, &temp, &["requirements", "Notes", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "rename-drawer");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(
        payload["old"],
        json!({"input": "requirements", "key": "REQUIREMENTS"})
    );
    assert_eq!(
        payload["new"],
        json!({
            "input": "Notes",
            "key": "NOTES",
            "drawer_title": "NOTES",
            "section_title": "Notes",
        })
    );
    assert_eq!(
        payload["counts"],
        json!({
            "notes_scanned": 8,
            "notes_changed": 4,
            "open_drawers": 1,
            "closed_drawers": 1,
            "archive_drawers": 1,
            "project_sections": 1,
            "merges": 1,
            "links_repaired": 3,
            "refused": 1,
            "excluded": 2,
            "unresolved_links": 1,
            "template_hits": 1,
        })
    );
    assert_eq!(
        change_paths(&payload),
        ["bob.md", "done/bob_done.md", "links.md", "proj.md"]
    );
    let drawer = &payload["changes"][0]["drawers"][0];
    assert_eq!(drawer["line"], 2);
    assert_eq!(drawer["task"], "Ship the exporter");
    assert_eq!(drawer["block_id"], "export");
    assert_eq!(drawer["status"], "open");
    assert_eq!(drawer["from"], json!(["REQUIREMENTS", "NOTES"]));
    assert_eq!(drawer["merged"], 1);
    assert_eq!(payload["changes"][1]["archive"], true);
    assert_eq!(
        payload["changes"][3]["sections"],
        json!([{"line": 8, "from": ["Requirements"], "merged": 0}])
    );
    assert_eq!(
        payload["changes"][2]["links"][1],
        json!({
            "line": 1,
            "before": "[[proj#Requirements|reqs]]",
            "after": "[[proj#Notes|reqs]]",
        })
    );
    assert_eq!(payload["refused"][0]["path"], "amb.md");
    assert_eq!(payload["templates"][0]["kind"], "heading");
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"], Value::Null);

    assert_eq!(head(&vault), before);
    assert_eq!(read(&vault, "bob.md"), BOB);
    assert_eq!(read(&vault, "done/bob_done.md"), DONE);
    assert_eq!(read(&vault, "proj.md"), PROJ);
    assert_eq!(read(&vault, "links.md"), LINKS);
    assert_untouched(&vault);
}

#[test]
fn apply_commits_once_and_a_rerun_reports_nothing() {
    let temp = TempDir::new("rename-drawer-apply");
    let vault = fixture(&temp);
    let before = head(&vault);

    let output = run(
        &vault,
        &temp,
        &["REQUIREMENTS", "NOTES", "-a", "-f", "json"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], false);
    let expected_subject =
        "bob task rename-drawer: REQUIREMENTS → NOTES (4 notes)";
    assert_eq!(payload["commit"]["subject"], expected_subject);
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert_eq!(subject(&vault), expected_subject);
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["rev-list", "--count", &format!("{before}..HEAD")]
        ))
        .trim(),
        "1"
    );
    assert_eq!(
        head_paths(&vault),
        "bob.md\ndone/bob_done.md\nlinks.md\nproj.md"
    );
    assert_eq!(read(&vault, "bob.md"), BOB_RENAMED);
    assert_eq!(
        read(&vault, "done/bob_done.md"),
        DONE.replace("REQUIREMENTS", "NOTES")
    );
    assert_eq!(
        read(&vault, "proj.md"),
        PROJ.replace("## Requirements", "## Notes")
    );
    assert_eq!(read(&vault, "links.md"), LINKS_RENAMED);
    assert_untouched(&vault);

    let after = head(&vault);
    let rerun = run(
        &vault,
        &temp,
        &["REQUIREMENTS", "NOTES", "-a", "-f", "json"],
    );
    assert_eq!(rerun.status.code(), Some(0), "{}", format_output(&rerun));
    let payload = json_of(&rerun);
    assert_eq!(payload["changes"], json!([]));
    assert_eq!(payload["counts"]["refused"], 1);
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), after);

    let human = run(&vault, &temp, &["REQUIREMENTS", "NOTES"]);
    assert_success(&human);
    assert!(
        stdout(&human).contains(
            "✓ nothing to rename: no REQUIREMENTS drawer or section changes in 8 notes scanned"
        ),
        "{}",
        format_output(&human)
    );
}

#[test]
fn apply_prints_a_summary_per_note() {
    let temp = TempDir::new("rename-drawer-apply-human");
    let vault = fixture(&temp);

    let output = run(&vault, &temp, &["requirements", "notes", "-a"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let text = stdout(&output);
    for expected in [
        "✓ renamed REQUIREMENTS → NOTES: 3 drawers (1 open · 1 closed · 1 archived) · 1 project section · 1 merge · 3 links repaired in 4 notes (",
        "  bob.md (2 drawers · 1 merge)",
        "  links.md (3 links)",
        "Refused (1 note, left unchanged)",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn invalid_requests_exit_2_before_reading_the_vault() {
    let temp = TempDir::new("rename-drawer-refusals");
    let vault = fixture(&temp);
    let before = head(&vault);
    for (args, code) in [
        (["requirements", "work log"], "reserved_drawer"),
        (["requirements", "tasks"], "reserved_drawer"),
        (["requirements", "café"], "invalid_drawer_title"),
        (["Notes", "NOTES"], "same_drawer"),
    ] {
        let output =
            run(&vault, &temp, &[args[0], args[1], "-a", "-f", "json"]);
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        let payload = json_of(&output);
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["error"]["code"], code, "{args:?}");
    }
    let human = run(&vault, &temp, &["requirements", "WORK LOG"]);
    assert_eq!(human.status.code(), Some(2));
    assert!(
        stderr(&human).contains("✗ WORK LOG is reserved for a managed line"),
        "{}",
        format_output(&human)
    );
    assert_eq!(head(&vault), before);
    assert_eq!(read(&vault, "bob.md"), BOB);
}

#[test]
fn no_match_is_a_clean_report() {
    let temp = TempDir::new("rename-drawer-no-match");
    let vault = fixture(&temp);
    let before = head(&vault);
    let output =
        run(&vault, &temp, &["future work", "later", "-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["counts"]["notes_changed"], 0);
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(head(&vault), before);
}
