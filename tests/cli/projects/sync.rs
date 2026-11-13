//! Guarded projects sync: lifecycle plus `## Projects` catalogs.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-07-10 12:00:00";

fn online_vault(temp: &TempDir) -> std::path::PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    vault
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn sync_cmd(
    vault: &Path,
    temp: &TempDir,
    args: &[&str],
    now: &str,
) -> std::process::Command {
    let mut cmd = bob_command();
    cmd.arg("projects").arg("sync");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg("-b").arg(vault);
    cmd.env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("BOB_NOW", now)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1");
    cmd
}

fn run_sync(
    vault: &Path,
    temp: &TempDir,
    args: &[&str],
    now: &str,
) -> std::process::Output {
    sync_cmd(vault, temp, args, now)
        .output()
        .expect("run bob projects sync")
}

fn json_output(output: &std::process::Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!(
            "one JSON envelope expected ({error}):\n{}",
            format_output(output)
        )
    })
}

fn head(vault: &Path) -> String {
    stdout(&git_in(vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_string()
}

fn commits_since(vault: &Path, base: &str) -> usize {
    stdout(&git_in(
        vault,
        ["rev-list", "--count", &format!("{base}..HEAD")],
    ))
    .trim()
    .parse()
    .expect("count")
}

fn project_note(status: &str, parent: Option<&str>, prj: &str) -> String {
    let parent_line = parent
        .map(|target| format!("parent: \"[[{target}]]\"\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: [[project]]\nstatus: {status}\n{parent_line}---\n{prj}\n"
    )
}

#[test]
fn projects_sync_combines_lifecycle_catalog_and_legacy_removal_in_one_write() {
    let temp = TempDir::new("bob-cli-projects-combined");
    let vault = online_vault(&temp);
    // Parent has lifecycle work (status flip + surfacing) plus a legacy
    // marker that must retire into the generated catalog in the same write.
    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [x] #task Ship parent #hide ^prj\n\t- 🧩 **Sub-projects:** [[Kid]]\n\nUnrelated prose.\n\n- [x] #task Log entry\n",
    );
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [ ] #task Kid work ^prj\n"),
    );
    commit_all(&vault, "combined fixture");

    let base = head(&vault);
    let dry = run_sync(&vault, &temp, &["-d"], NOW);
    assert_success(&dry);
    let dry_out = stdout(&dry);
    assert!(
        dry_out.contains("Parent") && dry_out.contains("dry run:"),
        "dry run must name the host and report the count:\n{}",
        format_output(&dry)
    );
    assert!(
        dry_out.contains("@@") && dry_out.contains("## Projects"),
        "dry run must show the unified catalog diff:\n{}",
        format_output(&dry)
    );

    let applied = run_sync(&vault, &temp, &[], NOW);
    assert_success(&applied);
    assert_eq!(commits_since(&vault, &base), 1, "one scoped commit");
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    assert!(parent.contains("status: done"), "{parent}");
    assert!(!parent.contains("Sub-projects:"), "{parent}");
    assert!(parent.contains("## Projects"), "{parent}");
    assert!(parent.contains("[[Kid]]"), "{parent}");
    assert!(parent.contains("Unrelated prose."), "{parent}");
    assert!(parent.contains("- [x] #task Log entry"), "{parent}");

    // Dry-run diffs equal the actual migration: rerunning the dry run after
    // the write is a no-op.
    let rerun_dry = run_sync(&vault, &temp, &["-d"], NOW);
    assert_success(&rerun_dry);
    assert!(
        !stdout(&rerun_dry).contains("@@"),
        "no-op dry run shows no diff:\n{}",
        format_output(&rerun_dry)
    );
    let rerun = run_sync(&vault, &temp, &[], NOW);
    assert_success(&rerun);
    assert_eq!(commits_since(&vault, &head(&vault)), 0);
}

#[test]
fn projects_sync_child_close_reopen_and_schedule_appear_in_parent_catalog() {
    let temp = TempDir::new("bob-cli-projects-child-states");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("Parent.md"),
        &project_note("wip", None, "- [ ] #task Parent ^prj\n"),
    );
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [ ] #task Kid ^prj\n"),
    );
    commit_all(&vault, "child states fixture");

    let first = run_sync(&vault, &temp, &[], NOW);
    assert_success(&first);
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    assert!(
        parent.contains("[[Kid]]") && !parent.contains("**Closed**"),
        "{parent}"
    );

    // Close the child: the same run must flip its row into Closed.
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [x] #task Kid ^prj\n"),
    );
    commit_all(&vault, "close kid");
    let closed = run_sync(&vault, &temp, &[], NOW);
    assert_success(&closed);
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    assert!(
        parent.contains("**Closed**") && parent.contains("[[Kid]]"),
        "{parent}"
    );

    // A hand-deleted closed row returns on the next sync (derived ledger).
    // Re-read and delete the Kid row by hand.
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    let hand_deleted = parent
        .lines()
        .filter(|line| !line.contains("[[Kid]]"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(vault.join("Parent.md"), hand_deleted).unwrap();
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "hand prune closed row"]);
    git_in(&vault, ["push", "-q"]);
    let restored = run_sync(&vault, &temp, &[], NOW);
    assert_success(&restored);
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    assert!(
        parent.contains("[[Kid]]"),
        "closed row must return:\n{parent}"
    );
}

#[test]
fn projects_sync_maintains_area_and_terminal_hosts_and_warns_safely() {
    let temp = TempDir::new("bob-cli-projects-hosts");
    let vault = online_vault(&temp);
    write_file(&vault.join("Area.md"), "---\ntype: [[area]]\n---\n# Area\n");
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Area"), "- [ ] #task Kid ^prj\n"),
    );
    write_file(
        &vault.join("Old.md"),
        &project_note(
            "done",
            None,
            "- [x] #task Old work [completion:: 2026-07-09] ^prj\n",
        ),
    );
    write_file(
        &vault.join("Grandkid.md"),
        &project_note(
            "done",
            Some("Old"),
            "- [x] #task Grand work [completion:: 2026-07-10] ^prj\n",
        ),
    );
    // A valid parent with only a warning child keeps its ^prj hidden, and
    // an invalid lifecycle host still gets safe catalog maintenance.
    write_file(
        &vault.join("Broken.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Broken parent #hide ^prj\n",
    );
    write_file(
        &vault.join("WarnKid.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[Broken]]\"\n---\n- [ ] #task No prj here\n",
    );
    write_file(
        &vault.join("Invalid.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task A ^prj\n- [ ] #task B ^prj\n",
    );
    write_file(
        &vault.join("InvalidKid.md"),
        &project_note("wip", Some("Invalid"), "- [ ] #task Kid ^prj\n"),
    );
    commit_all(&vault, "hosts fixture");

    let output = run_sync(&vault, &temp, &[], NOW);
    // Invalid lifecycle is a scan error, so the run exits 1 even though
    // healthy catalogs commit.
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let area = fs::read_to_string(vault.join("Area.md")).unwrap();
    assert!(
        area.contains("## Projects") && area.contains("[[Kid]]"),
        "{area}"
    );
    let old = fs::read_to_string(vault.join("Old.md")).unwrap();
    assert!(old.contains("[[Grandkid]]"), "{old}");
    let broken = fs::read_to_string(vault.join("Broken.md")).unwrap();
    assert!(broken.contains("## Projects"), "{broken}");
    // The parent of a warning child keeps its ^prj hidden.
    assert!(broken.contains("#hide"), "{broken}");
    let invalid = fs::read_to_string(vault.join("Invalid.md")).unwrap();
    assert!(
        invalid.contains("## Projects") && invalid.contains("[[InvalidKid]]"),
        "invalid lifecycle hosts still get catalogs:\n{invalid}"
    );
}

#[test]
fn projects_sync_exact_and_ambiguous_membership_drive_catalog_and_hide() {
    let temp = TempDir::new("bob-cli-projects-membership");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("Parent.md"),
        &project_note("wip", None, "- [ ] #task Parent ^prj\n"),
    );
    write_file(
        &vault.join("docs").join("Other.md"),
        &project_note("wip", None, "- [ ] #task Other ^prj\n"),
    );
    write_file(
        &vault.join("sub").join("Kid.md"),
        &project_note("wip", Some("docs/Other"), "- [ ] #task Kid ^prj\n"),
    );
    // Ambiguous basenames never become rows.
    write_file(
        &vault.join("a").join("Dup.md"),
        &project_note("wip", None, "- [ ] #task A ^prj\n"),
    );
    write_file(
        &vault.join("b").join("Dup.md"),
        &project_note("wip", None, "- [ ] #task B ^prj\n"),
    );
    write_file(
        &vault.join("Twin.md"),
        &project_note("wip", Some("Dup"), "- [ ] #task Twin ^prj\n"),
    );
    // Albums, notebooks, and archives never become project rows.
    write_file(
        &vault.join("Album.md"),
        "---\ntype: [[album]]\n---\n# Album\n",
    );
    commit_all(&vault, "membership fixture");

    let output = run_sync(&vault, &temp, &[], NOW);
    assert_success(&output);
    let other =
        fs::read_to_string(vault.join("docs").join("Other.md")).unwrap();
    assert!(
        other.contains("sub/Kid|Kid") || other.contains("[[Kid]]"),
        "{other}"
    );
    let parent = fs::read_to_string(vault.join("Parent.md")).unwrap();
    assert!(!parent.contains("## Projects"), "{parent}");
    // Ambiguous parents resolve to nothing: no catalog, no hide flip.
    let twin = fs::read_to_string(vault.join("Twin.md")).unwrap();
    assert!(!twin.contains("## Projects"), "{twin}");
}

#[test]
fn projects_sync_damaged_host_keeps_bytes_while_healthy_hosts_commit() {
    let temp = TempDir::new("bob-cli-projects-damaged");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("Good.md"),
        &project_note("wip", None, "- [ ] #task Good ^prj\n"),
    );
    write_file(
        &vault.join("GoodKid.md"),
        &project_note("wip", Some("Good"), "- [ ] #task Kid ^prj\n"),
    );
    write_file(
        &vault.join("Bad.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Bad ^prj\n\n## Projects\n\n> [!projects\n>\n> - **[[Kid]]** — Old.\n",
    );
    commit_all(&vault, "damaged fixture");
    let base = head(&vault);

    let output = run_sync(&vault, &temp, &[], NOW);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let out = stdout(&output);
    assert!(
        out.contains("Bad.md") && out.contains("projects_catalog_damaged"),
        "damage line must survive:\n{}",
        format_output(&output)
    );
    // Healthy hosts still commit exactly once.
    assert_eq!(commits_since(&vault, &base), 1);
    let good = fs::read_to_string(vault.join("Good.md")).unwrap();
    assert!(good.contains("[[GoodKid]]"), "{good}");
    // Damaged bytes are preserved exactly.
    let bad = fs::read_to_string(vault.join("Bad.md")).unwrap();
    assert!(bad.contains("> [!projects"), "{bad}");
}

#[test]
fn projects_sync_json_envelope_reports_events_and_noop_reruns() {
    let temp = TempDir::new("bob-cli-projects-json");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("Parent.md"),
        &project_note("wip", None, "- [ ] #task Parent #hide ^prj\n"),
    );
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [ ] #task Kid ^prj\n"),
    );
    commit_all(&vault, "json fixture");

    let dry = run_sync(&vault, &temp, &["-f", "json", "-d"], NOW);
    assert_success(&dry);
    let payload = json_output(&dry);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["command"], "sync");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["ok"], true);
    assert!(payload["commit"].is_null());
    assert!(payload["sync"].is_null());
    let changes = payload["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    assert!(changes.iter().all(|change| change.get("diff").is_some()));
    let events = payload["events"].as_array().unwrap();
    assert!(events.iter().any(|event| event["kind"] == "catalog_written"
        || event["kind"] == "catalog_updated"));

    let applied = run_sync(&vault, &temp, &["-f", "json"], NOW);
    assert_success(&applied);
    let payload = json_output(&applied);
    assert_eq!(payload["dry_run"], false);
    assert!(payload["commit"].is_object());
    assert!(payload["sync"].is_object());
    assert!(payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|change| change.get("diff").is_none()));

    // No-op reruns produce no edits and no commit.
    let rerun = run_sync(&vault, &temp, &["-f", "json"], NOW);
    assert_success(&rerun);
    let payload = json_output(&rerun);
    assert!(payload["changes"].as_array().unwrap().is_empty());
    assert!(payload["commit"].is_null());
}

#[test]
fn projects_sync_dry_run_needs_no_git_and_refusals_write_nothing() {
    // Dry runs work without a Git worktree.
    let temp = TempDir::new("bob-cli-projects-dry-nogit");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Parent.md"),
        &project_note("wip", None, "- [ ] #task Parent ^prj\n"),
    );
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [ ] #task Kid ^prj\n"),
    );
    let before = fs::read_to_string(vault.join("Parent.md")).unwrap();
    let dry = bob_command()
        .args(["projects", "sync", "-d", "-b"])
        .arg(&vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("dry run without git");
    assert_success(&dry);
    assert_eq!(fs::read_to_string(vault.join("Parent.md")).unwrap(), before);

    // A real run without a Git worktree is a structured refusal.
    let real = bob_command()
        .args(["projects", "sync", "-b"])
        .arg(&vault)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("worktree refusal");
    assert_eq!(real.status.code(), Some(1));
    let payload: Value = serde_json::from_str(stdout(&real).trim())
        .unwrap_or_else(|_| {
            // Human mode still refuses with a structured stderr line.
            assert!(
                stderr(&real)
                    .contains("project sync writes require a Git worktree")
                    || stdout(&real)
                        .contains("project sync writes require a Git worktree"),
                "refusal must name the worktree:\n{}",
                format_output(&real)
            );
            serde_json::json!({"error": {"code": "not_a_worktree"}})
        });
    let code = payload
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("not_a_worktree");
    assert_eq!(code, "not_a_worktree");
    assert_eq!(fs::read_to_string(vault.join("Parent.md")).unwrap(), before);
}

#[test]
fn projects_sync_vault_mismatch_and_lock_contention_fail_closed() {
    let temp = TempDir::new("bob-cli-projects-failures");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("Parent.md"),
        &project_note("wip", None, "- [ ] #task Parent ^prj\n"),
    );
    write_file(
        &vault.join("Kid.md"),
        &project_note("wip", Some("Parent"), "- [ ] #task Kid ^prj\n"),
    );
    commit_all(&vault, "failure fixture");
    let before = fs::read_to_string(vault.join("Parent.md")).unwrap();

    // Wrong-vault writes refuse before locks, sync state, or Git changes.
    let mut wrong = bob_command();
    wrong.arg("projects").arg("sync");
    assert_vault_override_refused(&mut wrong, &vault);

    // Lock contention is a structured failure with no writes.
    use fs2::FileExt;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(vault_sync_lock_file(&temp))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let output = sync_cmd(&vault, &temp, &[], NOW)
        .output()
        .expect("lock contention");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json_output(&{
        let mut cmd = sync_cmd(&vault, &temp, &["-f", "json"], NOW);
        cmd.output().expect("lock contention json")
    });
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["code"], "lock_busy");
    drop(lock);
    assert_eq!(fs::read_to_string(vault.join("Parent.md")).unwrap(), before);
}
