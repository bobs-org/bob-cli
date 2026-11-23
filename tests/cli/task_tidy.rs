//! `bob task tidy`: dry-run-first repair of open task blocks.

use crate::support::*;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// A req-owning task with a loose note and a misplaced link, then a done
/// task out of order that tidy must leave alone.
const BOB: &str = "\
## Tasks
- [ ] #task Ship the exporter ^export
\t- Morgan reported the original issue.
\t- [ ] Handles supported inputs
\t\t- [ ] Handles Windows files
\t- ⛓️ **DEPENDS ON:** [[#^parser]]
- [ ] #task Write the parser ^parser
- [x] #task Shipped ✅ 2026-11-20
\t- 🛠️ **WORK LOG**
\t\t- *2026-11-20* — done
\t- a note after the log
";

const BOB_TIDY: &str = "\
## Tasks
- [ ] #task Ship the exporter ^export
\t- ⛓️ **DEPENDS ON:** [[#^parser]]
\t- [ ] Handles supported inputs
\t\t- [ ] Handles Windows files
\t- NOTES
\t\t- Morgan reported the original issue.
- [ ] #task Write the parser ^parser
- [x] #task Shipped ✅ 2026-11-20
\t- 🛠️ **WORK LOG**
\t\t- *2026-11-20* — done
\t- a note after the log
";

/// A task with no reqs: only the log moves below the authored children.
const WORK: &str = "\
- [ ] #task Plain task
\t- 🛠️ **WORK LOG**
\t\t- *2026-11-20* — started
\t- a loose note
";

const WORK_TIDY: &str = "\
- [ ] #task Plain task
\t- a loose note
\t- 🛠️ **WORK LOG**
\t\t- *2026-11-20* — started
";

const CLEAN: &str = "- [ ] #task Already tidy\n\t- a note\n";

/// Out-of-order open blocks in archive history and templates.
const UNTOUCHABLE: &str = "- [ ] #task Old\n\t- a note\n\t- [ ] a req\n";

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

/// The sync pair also seeds `initial.md` with one canonical open task.
fn fixture(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write(&vault, "bob.md", BOB);
    write(&vault, "work.md", WORK);
    write(&vault, "clean.md", CLEAN);
    write(&vault, "done/bob_done.md", UNTOUCHABLE);
    write(&vault, "_templates/task.md", UNTOUCHABLE);
    write(&vault, "_generated/view.md", UNTOUCHABLE);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "task tidy fixture"]);
    git_in(&vault, ["push", "-q"]);
    vault
}

fn task_command(vault: &Path, temp: &TempDir, args: &[&str]) -> Command {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("home");
    let mut command = bob_command();
    command
        .args(["task", "tidy"])
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
    task_command(vault, temp, args)
        .output()
        .expect("run bob task tidy")
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
    assert_eq!(read(vault, "done/bob_done.md"), UNTOUCHABLE);
    assert_eq!(read(vault, "_templates/task.md"), UNTOUCHABLE);
    assert_eq!(read(vault, "_generated/view.md"), UNTOUCHABLE);
    assert_eq!(read(vault, "clean.md"), CLEAN);
}

#[test]
fn bare_run_is_a_dry_run_with_excerpts_and_counts() {
    let temp = TempDir::new("task-tidy-dry");
    let vault = fixture(&temp);
    let before = head(&vault);

    let human = run(&vault, &temp, &[]);
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    let text = stdout(&human);
    assert!(
        text.contains("bob.md (1 task · 1 move · 1 note filed)"),
        "{text}"
    );
    assert!(
        text.contains(
            "L2 Ship the exporter · 1 move · 1 note filed under new NOTES"
        ),
        "{text}"
    );
    assert!(text.contains("    + \t- NOTES"), "{text}");
    assert!(
        text.contains("    - \t- Morgan reported the original issue."),
        "{text}"
    );
    assert!(
        text.contains("work.md (1 task · 1 move · 0 notes filed)"),
        "{text}"
    );
    assert!(
        text.contains(
            "! dry run: 2 tasks in 2 notes · 2 moves · 1 note filed under NOTES; nothing was written. Rerun with -a to apply"
        ),
        "{text}"
    );
    assert!(!text.contains("bob_done"), "{text}");
    assert!(!text.contains("_templates"), "{text}");

    let output = run(&vault, &temp, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "tidy");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["routes"], json!([]));
    assert_eq!(
        payload["counts"],
        json!({
            "notes_scanned": 4,
            "open_tasks_scanned": 5,
            "notes_changed": 2,
            "tasks_changed": 2,
            "moves": 2,
            "notes_filed": 1,
            "notes_drawers_created": 1,
        })
    );
    assert_eq!(change_paths(&payload), ["bob.md", "work.md"]);
    let task = &payload["changes"][0]["tasks"][0];
    assert_eq!(task["line"], 2);
    assert_eq!(task["text"], "Ship the exporter");
    assert_eq!(task["block_id"], "export");
    assert_eq!(task["moves"], 1);
    assert_eq!(task["notes_filed"], 1);
    assert_eq!(task["notes_created"], true);
    assert_eq!(task["before"][1], "\t- Morgan reported the original issue.");
    assert_eq!(task["after"][4], "\t- NOTES");
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"], Value::Null);

    assert_eq!(head(&vault), before);
    assert_eq!(read(&vault, "bob.md"), BOB);
    assert_eq!(read(&vault, "work.md"), WORK);
    assert_untouched(&vault);
}

#[test]
fn apply_commits_once_and_a_rerun_reports_nothing() {
    let temp = TempDir::new("task-tidy-apply");
    let vault = fixture(&temp);
    let before = head(&vault);

    let output = run(&vault, &temp, &["-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], false);
    assert_eq!(
        payload["commit"]["subject"],
        "bob task tidy (2 tasks in 2 notes)"
    );
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert_eq!(subject(&vault), "bob task tidy (2 tasks in 2 notes)");
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["rev-list", "--count", &format!("{before}..HEAD")]
        ))
        .trim(),
        "1"
    );
    assert_eq!(head_paths(&vault), "bob.md\nwork.md");
    assert_eq!(read(&vault, "bob.md"), BOB_TIDY);
    assert_eq!(read(&vault, "work.md"), WORK_TIDY);
    assert_untouched(&vault);

    let after = head(&vault);
    let rerun = run(&vault, &temp, &["-a", "-f", "json"]);
    assert_eq!(rerun.status.code(), Some(0), "{}", format_output(&rerun));
    let payload = json_of(&rerun);
    assert_eq!(payload["changes"], json!([]));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), after);

    let human = run(&vault, &temp, &[]);
    assert_success(&human);
    assert!(
        stdout(&human).contains(
            "✓ task blocks are tidy: 5 open tasks in 4 notes scanned, 0 tidied"
        ),
        "{}",
        format_output(&human)
    );
}

#[test]
fn apply_prints_a_colored_summary_per_note() {
    let temp = TempDir::new("task-tidy-apply-human");
    let vault = fixture(&temp);

    let output = run(&vault, &temp, &["-a"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(
        text.contains(
            "✓ tidied 2 tasks in 2 notes · 2 moves · 1 note filed under NOTES ("
        ),
        "{text}"
    );
    assert!(
        text.contains("  bob.md (1 task · 1 move · 1 note filed)"),
        "{text}"
    );
    assert!(
        text.contains("  work.md (1 task · 1 move · 0 notes filed)"),
        "{text}"
    );
}

#[test]
fn routes_narrow_the_scope_and_bad_routes_exit_2() {
    let temp = TempDir::new("task-tidy-route");
    let vault = fixture(&temp);
    let before = head(&vault);

    let unknown = run(&vault, &temp, &["nope", "-a", "-f", "json"]);
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "{}",
        format_output(&unknown)
    );
    let payload = json_of(&unknown);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["code"], "unknown_route");
    assert_eq!(head(&vault), before);

    let archived = run(&vault, &temp, &["done/bob_done"]);
    assert_eq!(
        archived.status.code(),
        Some(2),
        "{}",
        format_output(&archived)
    );
    assert!(
        stderr(&archived)
            .contains("route `done/bob_done` names done/bob_done.md"),
        "{}",
        format_output(&archived)
    );

    let template = run(&vault, &temp, &["_templates/task.md", "-f", "json"]);
    assert_eq!(template.status.code(), Some(2));
    assert_eq!(json_of(&template)["error"]["code"], "route_out_of_scope");

    let output = run(&vault, &temp, &["@work", "-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["routes"], json!(["@work"]));
    assert_eq!(payload["counts"]["notes_scanned"], 1);
    assert_eq!(
        payload["commit"]["subject"],
        "bob task tidy (1 task in 1 note)"
    );
    assert_eq!(head_paths(&vault), "work.md");
    assert_eq!(read(&vault, "bob.md"), BOB);
    assert_eq!(read(&vault, "work.md"), WORK_TIDY);
}

#[test]
fn a_req_owning_prj_files_loose_notes_under_the_notes_section() {
    let temp = TempDir::new("task-tidy-prj-notes");
    let vault = fixture(&temp);
    write(
        &vault,
        "pk.md",
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pk root ^prj\n",
            "\t- [ ] hand req\n",
            "\t- loose\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
        ),
    );
    git_in(&vault, ["add", "pk.md"]);
    git_in(&vault, ["commit", "-q", "-m", "add pk"]);
    git_in(&vault, ["push", "-q"]);

    let output = run(&vault, &temp, &["@pk", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["counts"]["notes_changed"], 1);
    assert_eq!(payload["counts"]["notes_filed"], 1);
    assert_eq!(payload["counts"]["notes_drawers_created"], 1);
    let task = &payload["changes"][0]["tasks"][0];
    assert_eq!(task["notes_filed"], 1, "{payload}");
    assert_eq!(task["notes_created"], true, "{payload}");
    assert_eq!(
        task["after"],
        json!(["- [ ] #task #prj Pk root ^prj", "\t- [ ] hand req"]),
        "{payload}"
    );

    let apply = run(&vault, &temp, &["@pk", "-a", "-f", "json"]);
    assert_eq!(apply.status.code(), Some(0), "{}", format_output(&apply));
    assert_eq!(
        read(&vault, "pk.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pk root ^prj\n",
            "\t- [ ] hand req\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
            "\n",
            "## Notes\n",
            "\n",
            "- loose\n",
        )
    );

    let rerun = run(&vault, &temp, &["@pk", "-f", "json"]);
    assert_eq!(rerun.status.code(), Some(0), "{}", format_output(&rerun));
    let payload = json_of(&rerun);
    assert_eq!(payload["counts"]["notes_changed"], 0, "{payload}");
    assert_eq!(payload["changes"], json!([]));
}

#[test]
fn a_clean_route_writes_and_commits_nothing() {
    let temp = TempDir::new("task-tidy-clean-noop");
    let vault = fixture(&temp);
    let before = head(&vault);
    let output = run(&vault, &temp, &["clean", "-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json_of(&output);
    assert_eq!(payload["counts"]["notes_changed"], 0);
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), before);
}
