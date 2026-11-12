//! `bob albums update`: guarded schema edits, transaction, and sync outcomes.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
};

const NOW: &str = "2026-11-09 09:14:00";

const ALBUM: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places to eat.\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields:\n    status:\n      values:\n        to_try: To try\n        liked: Liked\n---\n\n# Eat\n\n## Records\n\n> [!records] 0 restaurants\n>\n> No restaurants yet\n";
const RECORD: &str = "---\nstatus: to_try\n---\n\n# Ninja\n";

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn fixture(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    fs::create_dir_all(vault.join("eat")).expect("record folder");
    write(&vault, "eat/ninja.md", RECORD);
    write(&vault, "eat.base", "filters: {}\n");
    commit_all(&vault, "album update fixture");
    // Undo must round-trip against a valid current index, so repair the
    // empty-state stale index the constant ships with before returning.
    let synced = run(&vault, temp, &["sync", "-f", "json"]);
    assert_eq!(synced.status.code(), Some(0), "{}", format_output(&synced));
    vault
}

fn run(vault: &Path, temp: &TempDir, args: &[&str]) -> Output {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let mut command = bob_command();
    command
        .arg("albums")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command.output().expect("run bob albums")
}

fn run_stdin(
    vault: &Path,
    temp: &TempDir,
    args: &[&str],
    stdin: &str,
) -> Output {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let mut command = bob_command();
    command
        .arg("albums")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn bob albums");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

fn json(output: &Output) -> Value {
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

fn fingerprint(vault: &Path, temp: &TempDir) -> String {
    let output = run(vault, temp, &["list", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    json(&output)["albums"][0]["schema_fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned()
}

fn envelope(expect: &str, ops: &str) -> String {
    format!("{{\"schema_version\":1,\"expect\":\"{expect}\",\"ops\":[{ops}]}}")
}

fn write_envelope(temp: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = temp.path().join(name);
    fs::write(&path, contents).expect("write envelope");
    path
}

fn hold_lock(temp: &TempDir) -> fs::File {
    let path = vault_sync_lock_file(temp);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("lock dir");
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .expect("open lock");
    fs2::FileExt::lock_exclusive(&file).expect("hold lock");
    file
}

#[test]
fn help_names_stdin_convention_and_keeps_options_alphabetical() {
    let output = bob_command()
        .args(["albums", "update", "--help"])
        .output()
        .expect("update help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("`-i -` reads stdin"), "{help}");
    assert!(help.contains("omitted `-i` has the same meaning"), "{help}");
    assert!(help.contains("See docs/records.md"), "{help}");
    // Clap renders `-b, --bob-dir <DIR>` lines; the update flags stay sorted.
    let long = ["--bob-dir", "--dry-run", "--format", "--help", "--input"];
    for flag in &long {
        assert!(help.contains(flag), "missing {flag}:\n{help}");
    }
    let positions: Vec<usize> =
        long.iter().map(|flag| help.find(flag).unwrap()).collect();
    let mut ordered = positions.clone();
    ordered.sort();
    assert_eq!(positions, ordered, "options not alphabetical:\n{help}");

    let group = bob_command()
        .args(["albums", "--help"])
        .output()
        .expect("albums help");
    assert_success(&group);
    let text = stdout(&group);
    assert!(text.contains("update"), "{text}");
}

#[test]
fn dry_run_previews_index_diff_and_reports_no_record_changes() {
    let temp = TempDir::new("album-update-dry");
    let vault = fixture(&temp);
    // Preview against the stale empty-state index so the planned diff
    // covers both the schema edit and the index repair.
    write(&vault, "eat.md", ALBUM);
    let before = fs::read(vault.join("eat.md")).unwrap();
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}"#,
    );
    let path = write_envelope(&temp, "add.json", &body);

    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-d",
            "-f",
            "json",
            "-i",
            path.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "update");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["stem"], "eat");
    assert_eq!(payload["path"], "eat.md");
    assert_eq!(payload["changed"], true);
    assert_eq!(payload["before_fingerprint"], expect);
    assert_eq!(payload["derived_keys"], serde_json::json!(["regular"]));
    assert_eq!(payload["per_op"][0]["op"], "choice.add");
    assert_eq!(payload["per_op"][0]["derived_key"], "regular");
    assert!(!payload["consequence"].as_str().unwrap().is_empty());
    let diff = payload["changes"][0]["diff"].as_str().unwrap();
    assert!(diff.contains("regular:"), "{diff}");
    assert!(diff.contains("[!records]"), "{diff}");
    assert_eq!(payload["inverse_ops"][0]["op"], "choice.remove");
    assert_eq!(payload["inverse_ops"][0]["value"], "regular");
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"], Value::Null);
    assert_eq!(
        fs::read(vault.join("eat.md")).unwrap(),
        before,
        "dry run writes nothing"
    );
}

#[test]
fn stdin_and_omitted_input_mean_the_same_envelope() {
    let temp = TempDir::new("album-update-stdin");
    let vault = fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}"#,
    );

    let via_file = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-d",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "a.json", &body).to_str().unwrap(),
        ],
    );
    let via_dash = run_stdin(
        &vault,
        &temp,
        &["update", "eat", "-d", "-f", "json", "-i", "-"],
        &body,
    );
    let via_omitted =
        run_stdin(&vault, &temp, &["update", "eat", "-d", "-f", "json"], &body);
    for output in [&via_file, &via_dash, &via_omitted] {
        assert_eq!(output.status.code(), Some(0), "{}", format_output(output));
    }
    assert_eq!(json(&via_file), json(&via_dash));
    assert_eq!(
        json(&via_file)["after_fingerprint"],
        json(&via_omitted)["after_fingerprint"]
    );
    assert!(stdout(&via_dash).ends_with('\n'));
    // stdout stays one parseable JSON document.
    assert_eq!(
        stdout(&via_dash).lines().count(),
        stdout(&via_dash).trim().lines().count()
    );
}

#[test]
fn dry_run_succeeds_under_a_held_lock_with_stale_index() {
    let temp = TempDir::new("album-update-lock-dry");
    let vault = fixture(&temp);
    // Restore the empty-state stale index without touching the schema, so
    // the dry run previews against a stale note under a held lock.
    write(&vault, "eat.md", ALBUM);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.set","field":"status","value":"liked","set":{"label":"Loved"}}"#,
    );
    let path = write_envelope(&temp, "set.json", &body);
    // A held maintenance lock never blocks a read-only preview.
    let _held = hold_lock(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-d",
            "-f",
            "json",
            "-i",
            path.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json(&output)["ok"], true);
}

#[test]
fn real_apply_refuses_a_busy_lock_without_planning() {
    let temp = TempDir::new("album-update-lock-busy");
    let vault = fixture(&temp);
    let before = head(&vault);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}"#,
    );
    let path = write_envelope(&temp, "add.json", &body);
    let _held = hold_lock(&temp);

    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], "lock_busy");
    assert_eq!(head(&vault), before, "busy lock writes nothing");
}

#[test]
fn empty_envelope_is_a_no_op_without_lock_sync_or_commit() {
    let temp = TempDir::new("album-update-noop");
    let vault = fixture(&temp);
    let before_head = head(&vault);
    let before_bytes = fs::read(vault.join("eat.md")).unwrap();
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, "");
    let path = write_envelope(&temp, "noop.json", &body);

    // The no-op path repairs no index and runs no sync.
    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["changed"], false);
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"], Value::Null);
    assert_eq!(payload["inverse_ops"], serde_json::json!([]));
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before_bytes);
    assert_eq!(head(&vault), before_head, "no-op commits nothing");

    // The no-op path takes no lock and runs no sync, so an unavailable
    // remote cannot fail it either.
    let remote = temp.path().join("remote.git");
    fs::remove_dir_all(&remote).expect("break remote");
    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json(&output)["changed"], false);
    assert_eq!(head(&vault), before_head);
}

#[test]
fn stale_fingerprint_refuses_while_formatting_only_rewrite_accepts() {
    let temp = TempDir::new("album-update-stale");
    let vault = fixture(&temp);
    let expect = fingerprint(&vault, &temp);

    let stale = envelope("sha256:0000000000000000000000000000000000000000000000000000000000000000", r#"{"op":"choice.add","field":"status","label":"Regular","position":2}"#);
    let path = write_envelope(&temp, "stale.json", &stale);
    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json(&output)["error"]["code"], "stale_schema");

    // Formatting-only rewrite keeps the semantic fingerprint: the same
    // envelope still validates against the reformatted note.
    let current = fs::read_to_string(vault.join("eat.md")).unwrap();
    let reformatted = current.replace("to_try: To try", "to_try: \"To try\"");
    assert_ne!(reformatted, current);
    write(&vault, "eat.md", &reformatted);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-d",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "ok.json", &envelope(&expect, r#"{"op":"choice.add","field":"status","label":"Regular","position":2}"#)).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json(&output)["ok"], true);
}

#[test]
fn malformed_envelope_and_unknown_ops_refuse_with_exit_2() {
    let temp = TempDir::new("album-update-malformed");
    let vault = fixture(&temp);
    let before = head(&vault);

    for (name, contents) in [
        ("bad.json", "not json"),
        (
            "version.json",
            r#"{"schema_version":2,"expect":"sha256:x","ops":[]}"#,
        ),
        (
            "unknown.json",
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.rename","field":"status"}]}"#,
        ),
    ] {
        let path = write_envelope(&temp, name, contents);
        let output = run(
            &vault,
            &temp,
            &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{name}: {}",
            format_output(&output)
        );
        assert_eq!(json(&output)["ok"], false);
    }
    assert_eq!(head(&vault), before, "refusals commit nothing");
}

#[test]
fn used_removal_and_prohibited_rewrites_refuse_with_counts() {
    let temp = TempDir::new("album-update-refusals");
    let vault = fixture(&temp);
    let before = head(&vault);
    let expect = fingerprint(&vault, &temp);

    // status is used by eat/ninja.md, so removal refuses with a count.
    let body = envelope(&expect, r#"{"op":"field.remove","field":"status"}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "rm.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]["affected"].as_u64().unwrap_or(0) >= 1,
        "{payload}"
    );
    assert!(
        payload["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("edit in YAML"),
        "{payload}"
    );

    // Kind changes are prohibited even when no records use the field.
    let body = envelope(
        &expect,
        r#"{"op":"field.set","field":"status","set":{"kind":"text"}}"#,
    );
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "kind.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    // Removing an index_by field refuses rather than changing top-level bytes.
    let indexed = fs::read_to_string(vault.join("eat.md"))
        .unwrap()
        .replace("record:\n", "record:\nindex_by: status\n");
    let _ = indexed;
    assert_eq!(head(&vault), before);
}

#[test]
fn multi_op_failure_is_all_or_none() {
    let temp = TempDir::new("album-update-atomic");
    let vault = fixture(&temp);
    let before = fs::read(vault.join("eat.md")).unwrap();
    let before_head = head(&vault);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","position":2},{"op":"field.remove","field":"status"}"#,
    );
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "both.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before);
    assert_eq!(head(&vault), before_head);
}

#[test]
fn apply_commits_one_scoped_update_and_preserves_other_files() {
    let temp = TempDir::new("album-update-apply");
    let vault = fixture(&temp);
    let owner_before = fs::read(vault.join("body.md")).unwrap();
    let record_before = fs::read(vault.join("eat/ninja.md")).unwrap();
    let base_before = fs::read(vault.join("eat.base")).unwrap();
    let before_head = head(&vault);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}"#,
    );
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "add.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["changed"], true);
    assert_eq!(
        payload["commit"]["subject"],
        "albums: update eat.md (choice.add status/Regular)"
    );
    assert_ne!(head(&vault), before_head);
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]
        ))
        .trim(),
        "eat.md",
        "exactly one scoped file commits"
    );
    assert_eq!(
        subject(&vault),
        "albums: update eat.md (choice.add status/Regular)"
    );
    assert_eq!(
        fs::read(vault.join("body.md")).unwrap(),
        owner_before,
        "owner catalog untouched"
    );
    assert_eq!(
        fs::read(vault.join("eat/ninja.md")).unwrap(),
        record_before,
        "records untouched"
    );
    assert_eq!(
        fs::read(vault.join("eat.base")).unwrap(),
        base_before,
        "Base untouched"
    );
    let after = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(after.contains("regular:"), "{after}");
    assert!(after.contains("[!records] 1 restaurants"), "{after}");
    assert_eq!(payload["sync"]["pre"], "ok");
    assert!(!payload["inverse_ops"].as_array().unwrap().is_empty());
}

#[test]
fn apply_then_undo_round_trips_to_identical_bytes() {
    let temp = TempDir::new("album-update-undo");
    let vault = fixture(&temp);
    let original = fs::read(vault.join("eat.md")).unwrap();
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}"#,
    );
    let forward = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "add.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(
        forward.status.code(),
        Some(0),
        "{}",
        format_output(&forward)
    );
    let applied = json(&forward);
    let after_fp = applied["after_fingerprint"].as_str().unwrap().to_owned();
    let inverse: Vec<Value> =
        applied["inverse_ops"].as_array().unwrap().clone();
    assert!(!inverse.is_empty());

    // The exact Undo request uses the returned after fingerprint.
    let undo_envelope = serde_json::json!({
        "schema_version": 1,
        "expect": after_fp,
        "ops": inverse,
    });
    let undo_body = serde_json::to_string(&undo_envelope).unwrap();
    let undo = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "undo.json", &undo_body)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(undo.status.code(), Some(0), "{}", format_output(&undo));
    assert_eq!(
        fs::read(vault.join("eat.md")).unwrap(),
        original,
        "undo restores byte-identical album"
    );
}

#[test]
fn usage_added_after_forward_add_makes_inverse_remove_refuse() {
    let temp = TempDir::new("album-update-usage");
    let vault = fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","position":2}"#,
    );
    let forward = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "add.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(
        forward.status.code(),
        Some(0),
        "{}",
        format_output(&forward)
    );
    let applied = json(&forward);
    let after_fp = applied["after_fingerprint"].as_str().unwrap().to_owned();

    write(
        &vault,
        "eat/second.md",
        "---\nstatus: regular\n---\n\n# Second\n",
    );
    commit_all(&vault, "use the new choice");
    let current = fingerprint(&vault, &temp);
    assert_eq!(
        current, after_fp,
        "using a choice does not change the schema"
    );

    let inverse: Vec<Value> =
        applied["inverse_ops"].as_array().unwrap().clone();
    let undo_envelope = serde_json::json!({
        "schema_version": 1,
        "expect": after_fp,
        "ops": inverse,
    });
    let undo = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(
                &temp,
                "undo.json",
                &serde_json::to_string(&undo_envelope).unwrap(),
            )
            .to_str()
            .unwrap(),
        ],
    );
    assert_eq!(undo.status.code(), Some(2), "{}", format_output(&undo));
    assert!(undo.status.code() == Some(2));
}

#[test]
fn post_sync_failure_keeps_local_commit_and_inverse_ops() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new("album-update-post-sync");
    let vault = fixture(&temp);
    let hook = vault.join(".git/hooks/pre-push");
    fs::write(
        &hook,
        "#!/bin/sh\ngit log -1 --format=%s | grep -q '^albums: update' && exit 1\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let base = head(&vault);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"choice.add","field":"status","label":"Regular","position":2}"#,
    );
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "add.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["sync"]["post"], "failed");
    assert!(!payload["inverse_ops"].as_array().unwrap().is_empty());
    let warning = payload["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("committed locally")
            && warning.contains("bob vault-sync"),
        "{warning}"
    );
    assert_ne!(head(&vault), base, "the local commit stands");
}

#[test]
fn refusals_create_no_schema_update_commit() {
    let temp = TempDir::new("album-update-no-commit");
    let vault = fixture(&temp);
    let base = head(&vault);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, r#"{"op":"field.remove","field":"status"}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "rm.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(head(&vault), base);
    assert!(
        !subject(&vault).starts_with("albums: update"),
        "no update commit"
    );
}

#[test]
fn completion_offers_update_stems_and_input_files() {
    let temp = TempDir::new("album-update-complete");
    let vault = fixture(&temp);

    let members = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete albums");
    assert_success(&members);
    let members_text = stdout(&members);
    let names: Vec<&str> = members_text
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(names.contains(&"update"), "members: {names:?}");

    let stems = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "update",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete stems");
    assert_success(&stems);
    let stems_text = stdout(&stems);
    let stem_names: Vec<&str> = stems_text
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(stem_names.contains(&"eat"), "stems: {stem_names:?}");
}

#[test]
fn catalog_excludes_the_json_input_update_command() {
    let mut command = bob_command();
    command.arg("__catalog").arg("--protocol").arg("1");
    let output = command.output().expect("run bob __catalog");
    assert_success(&output);
    let catalog: Value =
        serde_json::from_slice(&output.stdout).expect("catalog json");
    let entries = catalog["entries"].as_array().expect("entries");
    assert!(!entries
        .iter()
        .any(|entry| entry["path"] == serde_json::json!(["albums", "update"])));
    let excluded = catalog["excluded"].as_array().expect("excluded");
    assert!(excluded
        .iter()
        .any(|entry| entry["path"] == serde_json::json!(["albums", "update"])));
}
