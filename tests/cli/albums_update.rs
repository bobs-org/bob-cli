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
    // `index_by` lives at the album top level, not under `record:`.
    // Add an unused `location` field so both `index_by` and `index_facets`
    // have a real unused target; `status` stays used to prove the index
    // guard runs before the usage guard.
    let with_location =
        fs::read_to_string(vault.join("eat.md")).unwrap().replace(
            "    status:\n",
            "    location:\n      label: Location\n    status:\n",
        );
    write(&vault, "eat.md", &with_location);
    commit_all(&vault, "add location field");
    let _ = run(&vault, &temp, &["sync", "-f", "json"]);
    let indexed = fs::read_to_string(vault.join("eat.md")).unwrap().replace(
        "record:\n",
        "index_by: status\nindex_facets:\n  - location\nrecord:\n",
    );
    write(&vault, "eat.md", &indexed);
    commit_all(&vault, "add index refs");
    let _ = run(&vault, &temp, &["sync", "-f", "json"]);
    let indexed_head = head(&vault);
    let expect_indexed = fingerprint(&vault, &temp);
    for field in ["status", "location"] {
        let body = envelope(
            &expect_indexed,
            &format!(r#"{{"op":"field.remove","field":"{field}"}}"#),
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
                write_envelope(&temp, "idx.json", &body).to_str().unwrap(),
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{field}: {}",
            format_output(&output)
        );
        let payload = json(&output);
        assert_eq!(payload["error"]["code"], "index_reference", "{payload}");
        assert_eq!(
            payload["error"]["field"],
            serde_json::json!(field),
            "{payload}"
        );
    }
    assert_eq!(
        subject(&vault),
        "add index refs",
        "index refusals commit nothing new"
    );
    assert_eq!(head(&vault), indexed_head);
    let _ = before;
}

#[test]
fn grouping_prefers_status_and_reports_regrouping() {
    let temp = TempDir::new("album-update-grouping");
    let vault = fixture(&temp);
    // Add an unused `category` choices field after `status`.
    let expect = fingerprint(&vault, &temp);
    let body = envelope(
        &expect,
        r#"{"op":"field.add","label":"Category","kind":"choices","position":1,"choices":[{"label":"Family"}]}"#,
    );
    let added = run(
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
    assert_eq!(added.status.code(), Some(0), "{}", format_output(&added));
    // Move `category` before `status`; the authoritative group stays `status`.
    let expect_moved = json(&added)["after_fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = envelope(
        &expect_moved,
        r#"{"op":"field.move","field":"category","position":0}"#,
    );
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
            write_envelope(&temp, "move.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["group_before"], serde_json::json!("status"));
    assert_eq!(payload["group_after"], serde_json::json!("status"));
}

#[test]
fn nonempty_cancelling_batch_is_a_no_op_without_lock_or_commit() {
    let temp = TempDir::new("album-update-nonempty-noop");
    let vault = fixture(&temp);
    let before_head = head(&vault);
    let before_bytes = fs::read(vault.join("eat.md")).unwrap();
    let expect = fingerprint(&vault, &temp);
    // Album label is explicit `Restaurant`; setting it away and back
    // restores the initial schema and order, so the batch must return
    // original bytes and no edit, even with a held lock and broken remote.
    let body = envelope(
        &expect,
        r#"{"op":"album.set","set":{"label":"Temporary"}},{"op":"album.set","set":{"label":"Restaurant"}}"#,
    );
    let path = write_envelope(&temp, "noop.json", &body);
    let _held = hold_lock(&temp);
    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["changed"], false);
    assert_eq!(payload["changes"], serde_json::json!([]));
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before_bytes);
    assert_eq!(head(&vault), before_head);
    let remote = temp.path().join("remote.git");
    fs::remove_dir_all(&remote).expect("break remote");
    let output = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json(&output)["changed"], false);
}

#[test]
fn all_nine_ops_accept_hostile_unicode_labels() {
    let temp = TempDir::new("album-update-nine");
    let vault = fixture(&temp);
    let mut expect = fingerprint(&vault, &temp);
    // Unicode labels exercise derived keys, emoji splitting, and quoting.
    // Each step applies so the next `expect` matches the moved vault.
    let unicode = "Café 🍰 naïve";
    let mut run_step = |index: usize, op: &str| -> Value {
        let body = envelope(&expect, op);
        let output = run(
            &vault,
            &temp,
            &[
                "update",
                "eat",
                "-f",
                "json",
                "-i",
                write_envelope(&temp, &format!("step{index}.json"), &body)
                    .to_str()
                    .unwrap(),
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "step {index} {op}: {}",
            format_output(&output)
        );
        let payload = json(&output);
        assert_eq!(payload["ok"], true, "step {index}: {payload}");
        expect = payload["after_fingerprint"].as_str().unwrap().to_owned();
        payload
    };
    run_step(
        0,
        &format!(r#"{{"op":"album.set","set":{{"label":"{unicode} Eats"}}}}"#),
    );
    run_step(
        1,
        &format!(
            r#"{{"op":"field.add","label":"{unicode} Notes","position":1}}"#
        ),
    );
    run_step(
        2,
        r#"{"op":"field.set","field":"status","set":{"suggest":true}}"#,
    );
    run_step(3, r#"{"op":"field.move","field":"status","position":1}"#);
    let added = run_step(
        4,
        &format!(
            r#"{{"op":"choice.add","field":"status","label":"{unicode} Regular","emoji":"🔁","position":2}}"#
        ),
    );
    let value = added["derived_keys"][0].as_str().unwrap().to_owned();
    assert!(!value.is_empty(), "unicode choice derives a key: {added}");
    run_step(
        5,
        &format!(
            r#"{{"op":"choice.set","field":"status","value":"{value}","set":{{"label":"{unicode} Regular!"}}}}"#
        ),
    );
    run_step(
        6,
        &format!(
            r#"{{"op":"choice.move","field":"status","value":"{value}","position":0}}"#
        ),
    );
    run_step(
        7,
        &format!(
            r#"{{"op":"choice.remove","field":"status","value":"{value}"}}"#
        ),
    );
    // The Unicode field key derives from its label; remove it by that key.
    let field_key = {
        let payload =
            run_step(8, r#"{"op":"field.move","field":"status","position":0}"#);
        // Derive the Unicode field key from the last list report.
        let _ = payload;
        // `normalize_stem` lowercases and strips accents/punctuation.
        // Read the current album to find the derived key.
        let list = run(&vault, &temp, &["list", "-f", "json"]);
        let albums = json(&list)["albums"].as_array().unwrap().clone();
        let fields = albums[0]["fields"].as_array().unwrap().clone();
        fields
            .iter()
            .find(|field| {
                field["label"].as_str().unwrap().contains("naïve Notes")
            })
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    run_step(
        9,
        &format!(r#"{{"op":"field.remove","field":"{field_key}"}}"#),
    );
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
    assert!(after.contains("[!records] 1 restaurant"), "{after}");
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

// ---------------------------------------------------------------------------
// walk_by.set: reversible grouped walk configuration
// ---------------------------------------------------------------------------

fn walk_fixture(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    fs::create_dir_all(vault.join("eat")).expect("record folder");
    write(&vault, "eat/aaa.md", "---\nstatus: to_try\n---\n\n# AAA\n");
    write(&vault, "eat/mmm.md", "---\nstatus: liked\n---\n\n# MMM\n");
    write(&vault, "eat/zzz.md", "---\nstatus: to_try\n---\n\n# ZZZ\n");
    write(&vault, "eat.base", "filters: {}\n");
    commit_all(&vault, "walk_by fixture");
    let synced = run(&vault, temp, &["sync", "-f", "json"]);
    assert_eq!(synced.status.code(), Some(0), "{}", format_output(&synced));
    vault
}

fn list_json(vault: &Path, temp: &TempDir) -> Value {
    let output = run(vault, temp, &["list", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    json(&output)
}

fn walk_by_of(payload: &Value) -> Option<String> {
    payload["albums"][0]["walk_by"].as_str().map(str::to_owned)
}

#[test]
fn walk_by_set_and_unset_rewrites_grouped_walk_lines() {
    let temp = TempDir::new("album-update-walk-set");
    let vault = walk_fixture(&temp);
    let before_fp = fingerprint(&vault, &temp);
    assert_eq!(
        list_json(&vault, &temp)["albums"][0]["walk_by"],
        Value::Null
    );

    // Set walk_by to status: one scoped commit with album + walk lines.
    let body = envelope(&before_fp, r#"{"op":"walk_by.set","field":"status"}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["changed"], true);
    let after_fp = payload["after_fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(after_fp, before_fp, "grouping binds into the fingerprint");
    let album_text = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(album_text.contains("walk_by: status"), "{album_text}");
    // Grouped order: to_try (AAA, ZZZ) then liked (MMM).
    let listed = list_json(&vault, &temp);
    assert_eq!(walk_by_of(&listed), Some("status".to_owned()));
    assert_eq!(
        listed["albums"][0]["walk"]["order"],
        serde_json::json!(["aaa", "zzz", "mmm"]),
        "group-major title order"
    );
    assert_eq!(listed["albums"][0]["walk"]["state"], "current");
    assert_eq!(
        listed["albums"][0]["walk"]["groups"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Exactly one scoped commit contains the album and all affected records.
    let files = stdout(&git_in(
        &vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ));
    let mut names: Vec<&str> = files.lines().collect();
    names.sort();
    assert!(names.contains(&"eat.md"), "album commits: {names:?}");
    assert!(
        names.contains(&"eat/aaa.md"),
        "walk lines commit: {names:?}"
    );
    assert!(
        !names.contains(&"body.md"),
        "unrelated files excluded: {names:?}"
    );
    assert!(
        !names.contains(&"eat.base"),
        "unrelated files excluded: {names:?}"
    );

    // Unset returns to flat title order and identical fingerprint base.
    let body = envelope(&after_fp, r#"{"op":"walk_by.set","field":null}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "unset.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["changed"], true);
    let album_text = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(!album_text.contains("walk_by"), "{album_text}");
    let listed = list_json(&vault, &temp);
    assert_eq!(listed["albums"][0]["walk_by"], Value::Null);
    assert_eq!(
        listed["albums"][0]["walk"]["order"],
        serde_json::json!(["aaa", "mmm", "zzz"]),
        "flat title order returns"
    );
    assert_eq!(
        payload["after_fingerprint"].as_str().unwrap(),
        before_fp,
        "unset restores the byte-identical schema hash"
    );
}

#[test]
fn walk_by_set_noop_and_invalid_selections_refuse() {
    let temp = TempDir::new("album-update-walk-invalid");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);

    // Setting the same unset value twice: second is a semantic no-op.
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":null}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "noop.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(json(&output)["changed"], false);

    // Unknown field.
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"missing"}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad1.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stdout(&output).contains("walk_by names undeclared field"),
        "{}",
        format_output(&output)
    );

    // Non-choices field: add a text field, then select it.
    let body = envelope(
        &expect,
        r#"{"op":"field.add","label":"Note","kind":"text"}"#,
    );
    let added = run(
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
    assert_eq!(added.status.code(), Some(0), "{}", format_output(&added));
    let after = json(&added)["after_fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = envelope(&after, r#"{"op":"walk_by.set","field":"note"}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad2.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stdout(&output)
            .contains("walk_by must name a single-value choices field"),
        "{}",
        format_output(&output)
    );

    // Empty field name.
    let body = envelope(&after, r#"{"op":"walk_by.set","field":"  "}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad3.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stdout(&output).contains("walk_by must be a non-empty field name"),
        "{}",
        format_output(&output)
    );

    // Wrong type.
    let body = envelope(&after, r#"{"op":"walk_by.set","field":42}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad4.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    // Missing key.
    let body = format!("{{\"schema_version\":1,\"expect\":\"{after}\",\"ops\":[{{\"op\":\"walk_by.set\"}}]}}");
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad5.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    // Unknown op key.
    let body =
        envelope(&after, r#"{"op":"walk_by.set","field":"status","extra":1}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "bad6.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn walk_by_guards_field_removal_and_allows_unset_then_remove() {
    let temp = TempDir::new("album-update-walk-guard");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    // Add an unused choices field and select it for the walk.
    let body = envelope(
        &expect,
        r#"{"op":"field.add","label":"Section","kind":"choices","choices":[{"label":"Intro"}]}"#,
    );
    let added = run(
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
    assert_eq!(added.status.code(), Some(0), "{}", format_output(&added));
    let after_add = json(&added)["after_fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let body =
        envelope(&after_add, r#"{"op":"walk_by.set","field":"section"}"#);
    let set = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(set.status.code(), Some(0), "{}", format_output(&set));
    let after_set =
        json(&set)["after_fingerprint"].as_str().unwrap().to_owned();

    // Removing the selected field refuses, even though it is unused.
    let body =
        envelope(&after_set, r#"{"op":"field.remove","field":"section"}"#);
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
    assert!(
        stdout(&output).contains("named by walk_by"),
        "{}",
        format_output(&output)
    );

    // Unsetting before removing proceeds when the field is unused.
    let body = envelope(
        &after_set,
        r#"{"op":"walk_by.set","field":null},{"op":"field.remove","field":"section"}"#,
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
            write_envelope(&temp, "unset_rm.json", &body)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let album_text = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(!album_text.contains("walk_by"), "{album_text}");
    assert!(!album_text.contains("section"), "{album_text}");
}

#[test]
fn walk_by_choice_reorder_replans_but_label_edits_do_not() {
    let temp = TempDir::new("album-update-walk-reorder");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(
        &vault,
        "eat.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places to eat.\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields:\n    status:\n      values:\n        to_try: To try\n        liked: Liked\n        archived: Archived\n---\n\n# Eat\n\n## Records\n\n> [!records] 0 restaurants\n>\n> No restaurants yet\n",
    );
    fs::create_dir_all(vault.join("eat")).expect("record folder");
    write(&vault, "eat/aaa.md", "---\nstatus: to_try\n---\n\n# AAA\n");
    write(&vault, "eat/mmm.md", "---\nstatus: liked\n---\n\n# MMM\n");
    write(
        &vault,
        "eat/zzz.md",
        "---\nstatus: archived\n---\n\n# ZZZ\n",
    );
    write(&vault, "eat.base", "filters: {}\n");
    commit_all(&vault, "walk_by three-group fixture");
    let synced = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(synced.status.code(), Some(0), "{}", format_output(&synced));
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"status"}"#);
    let set = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(set.status.code(), Some(0), "{}", format_output(&set));
    let after_set =
        json(&set)["after_fingerprint"].as_str().unwrap().to_owned();
    let order_before =
        list_json(&vault, &temp)["albums"][0]["walk"]["order"].clone();
    assert_eq!(order_before, serde_json::json!(["aaa", "mmm", "zzz"]));

    // Reordering the selected choices regroups the walk: moving the middle
    // choice last changes every walk line (not a pure ring rotation).
    let body = envelope(
        &after_set,
        r#"{"op":"choice.move","field":"status","value":"liked","position":2}"#,
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
            write_envelope(&temp, "move.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["changed"], true);
    let order_after =
        list_json(&vault, &temp)["albums"][0]["walk"]["order"].clone();
    assert_ne!(order_after, order_before, "choice order regroups the walk");
    assert_eq!(order_after, serde_json::json!(["aaa", "zzz", "mmm"]));
    let files = stdout(&git_in(
        &vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ));
    assert!(
        files.contains("eat/"),
        "choice reorder commits walk lines: {files}"
    );

    // A display-label change keeps the same choice values, so no walk repair.
    let after_move = payload["after_fingerprint"].as_str().unwrap().to_owned();
    let before_records: Vec<String> = ["aaa", "mmm", "zzz"]
        .iter()
        .map(|stem| {
            fs::read_to_string(vault.join(format!("eat/{stem}.md"))).unwrap()
        })
        .collect();
    let body = envelope(
        &after_move,
        r#"{"op":"choice.set","field":"status","value":"liked","set":{"label":"Loved"}}"#,
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
            write_envelope(&temp, "label.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let files = stdout(&git_in(
        &vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ));
    assert_eq!(
        files.trim(),
        "eat.md",
        "label edit commits only the album: {files}"
    );
    for (stem, before) in
        ["aaa", "mmm", "zzz"].iter().zip(before_records.iter())
    {
        assert_eq!(
            &fs::read_to_string(vault.join(format!("eat/{stem}.md"))).unwrap(),
            before,
            "label edit leaves {stem} walk lines alone"
        );
    }
}

#[test]
fn walk_by_unrelated_edits_do_not_repair_stale_walk() {
    let temp = TempDir::new("album-update-walk-stale");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"status"}"#);
    let set = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(set.status.code(), Some(0), "{}", format_output(&set));
    let after_set =
        json(&set)["after_fingerprint"].as_str().unwrap().to_owned();

    // Break one walk line by hand and commit the damage.
    let stale = fs::read_to_string(vault.join("eat/aaa.md")).unwrap();
    let broken = stale.replacen("[[eat|", "[[eat| STALE ", 1);
    assert_ne!(broken, stale, "fixture walk line must exist");
    write(&vault, "eat/aaa.md", &broken);
    commit_all(&vault, "break one walk line");

    // An unrelated schema edit must not repair the stale walk line.
    let body = envelope(
        &after_set,
        r#"{"op":"album.set","set":{"label":"Eateries"}}"#,
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
            write_envelope(&temp, "unrelated.json", &body)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(vault.join("eat/aaa.md")).unwrap(),
        broken,
        "unrelated edits never repair walk lines"
    );
    let files = stdout(&git_in(
        &vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ));
    assert_eq!(files.trim(), "eat.md", "only the album commits: {files}");
}

#[test]
fn walk_by_inverse_round_trips_and_multi_op_batches_are_atomic() {
    let temp = TempDir::new("album-update-walk-inverse");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let original = fs::read_to_string(vault.join("eat.md")).unwrap();

    // Set then invert through the reported inverse ops.
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"status"}"#);
    let set = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(set.status.code(), Some(0), "{}", format_output(&set));
    let payload = json(&set);
    let after_set = payload["after_fingerprint"].as_str().unwrap().to_owned();
    assert!(!payload["inverse_ops"].as_array().unwrap().is_empty());
    let inverse = payload["inverse_ops"].clone();
    let undo_body = serde_json::json!({
        "schema_version": 1,
        "expect": after_set,
        "ops": inverse,
    })
    .to_string();
    let output = run(
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
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(vault.join("eat.md")).unwrap(), original);

    // A batch that ends with a refused removal writes nothing.
    let expect_now = fingerprint(&vault, &temp);
    let body = envelope(
        &expect_now,
        r#"{"op":"walk_by.set","field":"status"},{"op":"field.remove","field":"status"}"#,
    );
    let before_head = head(&vault);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "atomic.json", &body)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(head(&vault), before_head, "failed batches write nothing");
}

#[test]
fn walk_by_stale_fingerprint_and_dry_run_purity() {
    let temp = TempDir::new("album-update-walk-stale-fp");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"status"}"#);
    let path = write_envelope(&temp, "set.json", &body);

    // Dry run previews album + walk diffs but writes nothing.
    let before_bytes: Vec<Vec<u8>> =
        ["eat.md", "eat/aaa.md", "eat/mmm.md", "eat/zzz.md"]
            .iter()
            .map(|name| fs::read(vault.join(name)).unwrap())
            .collect();
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
    assert_eq!(payload["changed"], true);
    assert!(
        payload["changes"].as_array().unwrap().len() >= 2,
        "dry run lists walk lines: {}",
        payload["changes"]
    );
    for (name, before) in ["eat.md", "eat/aaa.md", "eat/mmm.md", "eat/zzz.md"]
        .iter()
        .zip(before_bytes.iter())
    {
        assert_eq!(
            &fs::read(vault.join(name)).unwrap(),
            before,
            "dry run writes nothing to {name}"
        );
    }

    // A real apply followed by a stale-expect retry refuses.
    let applied = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        format_output(&applied)
    );
    let stale = run(
        &vault,
        &temp,
        &["update", "eat", "-f", "json", "-i", path.to_str().unwrap()],
    );
    assert_eq!(stale.status.code(), Some(2), "{}", format_output(&stale));
    assert!(
        stdout(&stale).contains("stale_schema")
            || format_output(&stale).contains("stale"),
        "{}",
        format_output(&stale)
    );
}

#[test]
fn walk_by_restore_whitelist_rejects_unrelated_and_stale_spans() {
    let temp = TempDir::new("album-update-walk-restore");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let before_head = head(&vault);

    // Malicious restore: walk_by.set must not restore nested record keys.
    let malicious = format!(
        "{{\"schema_version\":1,\"expect\":\"{expect}\",\"ops\":[{{\"op\":\"walk_by.set\",\"field\":\"status\",\"restore\":{{\"spans\":[{{\"key_path\":\"record.label\",\"expected\":null,\"prior\":\"walk_by: status\"}}]}}}}]}}"
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
            write_envelope(&temp, "malicious.json", &malicious)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        format_output(&output).contains("schema_restore_invalid"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        head(&vault),
        before_head,
        "malicious restore writes nothing"
    );

    // Stale restore: expected bytes do not match the current walk_by span.
    let stale_restore = format!(
        "{{\"schema_version\":1,\"expect\":\"{expect}\",\"ops\":[{{\"op\":\"walk_by.set\",\"field\":\"status\",\"restore\":{{\"spans\":[{{\"key_path\":\"walk_by\",\"expected\":\"walk_by: bogus\",\"prior\":\"walk_by: status\"}}]}}}}]}}"
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
            write_envelope(&temp, "stale_restore.json", &stale_restore)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        format_output(&output).contains("schema_restore_stale")
            || format_output(&output).contains("schema_restore_invalid"),
        "{}",
        format_output(&output)
    );
    assert_eq!(head(&vault), before_head, "stale restore writes nothing");
}

#[test]
fn walk_by_quoted_comment_matches_plain_selection_fingerprint() {
    let temp = TempDir::new("album-update-walk-quoted");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":"status"}"#);
    let set = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "set.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(set.status.code(), Some(0), "{}", format_output(&set));
    let after = json(&set)["after_fingerprint"].as_str().unwrap().to_owned();
    let quoted = fs::read_to_string(vault.join("eat.md"))
        .unwrap()
        .replace("walk_by: status", "walk_by: \"status\" # authored");
    fs::write(vault.join("eat.md"), quoted).unwrap();
    let listed = run(&vault, &temp, &["list", "-f", "json"]);
    assert_eq!(listed.status.code(), Some(0), "{}", format_output(&listed));
    assert_eq!(
        json(&listed)["albums"][0]["schema_fingerprint"]
            .as_str()
            .unwrap(),
        after,
        "quoted commented walk_by shares the effective fingerprint"
    );
    assert_eq!(
        json(&listed)["albums"][0]["walk_by"].as_str(),
        Some("status")
    );
}

#[test]
fn walk_by_explicit_unset_removes_invalid_authored_key() {
    let temp = TempDir::new("album-update-walk-invalid-unset");
    let vault = walk_fixture(&temp);
    let original = fs::read_to_string(vault.join("eat.md")).unwrap();
    let invalid = original.replace(
        "summary: Places to eat.\n",
        "summary: Places to eat.\nwalk_by: missing\n",
    );
    fs::write(vault.join("eat.md"), &invalid).unwrap();
    let expect = fingerprint(&vault, &temp);
    assert_eq!(expect, fingerprint(&vault, &temp));
    let body = envelope(&expect, r#"{"op":"walk_by.set","field":null}"#);
    let output = run(
        &vault,
        &temp,
        &[
            "update",
            "eat",
            "-f",
            "json",
            "-i",
            write_envelope(&temp, "unset.json", &body).to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["changed"], true);
    let after = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert!(!after.contains("walk_by"), "{after}");
    let inverse = payload["inverse_ops"].clone();
    let undo_body = serde_json::json!({
        "schema_version": 1,
        "expect": payload["after_fingerprint"],
        "ops": inverse,
    })
    .to_string();
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
        fs::read_to_string(vault.join("eat.md")).unwrap(),
        invalid,
        "unset inverse restores the original authored slot"
    );
}

#[test]
fn walk_by_mismatching_restore_writes_nothing() {
    let temp = TempDir::new("album-update-walk-mismatch-restore");
    let vault = walk_fixture(&temp);
    let expect = fingerprint(&vault, &temp);
    let before_head = head(&vault);
    let malicious = format!(
        "{{\"schema_version\":1,\"expect\":\"{expect}\",\"ops\":[{{\"op\":\"walk_by.set\",\"field\":\"status\",\"restore\":{{\"spans\":[{{\"key_path\":\"walk_by\",\"expected\":null,\"prior\":\"walk_by: bogus\\n\"}}]}}}}]}}"
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
            write_envelope(&temp, "mismatch.json", &malicious)
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        format_output(&output).contains("span_semantic_mismatch")
            || format_output(&output).contains("walk_by"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        head(&vault),
        before_head,
        "mismatching restore writes nothing"
    );
}
