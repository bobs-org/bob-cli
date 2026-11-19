//! Guarded `bob periods update` and daily `bob periods show` contract.

use crate::support::*;
use fs2::FileExt;
use serde_json::{json, Value};
use std::fs;

const NOW: &str = "2026-11-19 12:00:00";

fn vault(tag: &str) -> TempDir {
    TempDir::new(&format!("bob-cli-periods-update-{tag}"))
}

fn day_path(vault: &std::path::Path) -> std::path::PathBuf {
    vault.join("2026/20261118.md")
}

fn write_day(vault: &std::path::Path, contents: &str) {
    let path = day_path(vault);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn update(
    vault: &std::path::Path,
    subject: &str,
    envelope: &str,
    extra: &[&str],
) -> std::process::Output {
    let input = vault.join("update-envelope.json");
    fs::write(&input, envelope).unwrap();
    let mut command = bob_command();
    command
        .args(["periods", "update", subject, "-i"])
        .arg(input)
        .args(["-f", "json"])
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.join("vault.lock"))
        .env("NO_COLOR", "1")
        .args(extra);
    command.output().expect("run bob periods update")
}

fn report(output: &std::process::Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("invalid JSON ({error}):\n{}", format_output(output))
    })
}

fn current_fingerprint(vault: &std::path::Path) -> String {
    let output = update(
        vault,
        "yesterday",
        r#"{"schema_version":1,"ops":[]}"#,
        &["-n"],
    );
    assert_success(&output);
    report(&output)["fingerprint_before"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn every_public_operation_applies_in_order_and_unsets_empty_fields() {
    let vault = vault("ops");
    write_day(
        vault.path(),
        "---\nintentions: [old, older]\nsummary: Draft\nsummary_digest: sha256:1111111111111111\nsummary_drafted: 2026-11-19\n---\nbody\n",
    );
    let expect = current_fingerprint(vault.path());
    let output = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version": 1,
            "expect": expect,
            "ops": [
                {"op":"intentions.set","values":["one","two","three"]},
                {"op":"intentions.insert","index":1,"value":"inserted"},
                {"op":"intentions.replace","index":2,"value":"replaced"},
                {"op":"intentions.move","from":3,"to":0},
                {"op":"intentions.remove","index":2},
                {"op":"reflection.set","value":"First paragraph\n\nSecond paragraph"},
                {"op":"summary.set","value":"Bryan's account"}
            ]
        })
        .to_string(),
        &[],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(
        body["after"]["intentions"],
        json!(["three", "one", "replaced"])
    );
    assert_eq!(body["after"]["summary"], "Bryan's account");
    assert_eq!(
        body["after"]["reflection"],
        "First paragraph\n\nSecond paragraph"
    );
    assert_eq!(body["after"]["summary_state"], "edited");
    let note = fs::read_to_string(day_path(vault.path())).unwrap();
    assert!(!note.contains("summary_digest:"));
    assert!(!note.contains("summary_drafted:"));

    let expect = body["fingerprint_after"].as_str().unwrap();
    let unset = update(
        vault.path(),
        "20261118",
        &json!({
            "schema_version": 1,
            "expect": expect,
            "ops": [
                {"op":"intentions.set","values":[]},
                {"op":"reflection.unset"},
                {"op":"summary.unset"}
            ]
        })
        .to_string(),
        &[],
    );
    assert_success(&unset);
    let unset_note = fs::read_to_string(day_path(vault.path())).unwrap();
    assert!(!unset_note.contains("intentions:"));
    assert!(!unset_note.contains("summary:"));
    assert!(!unset_note.contains("reflection:"));
}

#[test]
fn inverse_ops_restore_values_and_summary_draft_stamps() {
    let vault = vault("undo");
    write_day(
        vault.path(),
        "---\nintentions: [one, two]\nsummary: Agent draft\nsummary_digest: sha256:1111111111111111\nsummary_drafted: 2026-11-19\nreflection: Existing reflection\n---\nbody\n",
    );
    let expect = current_fingerprint(vault.path());
    let changed = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":expect,
            "ops":[
                {"op":"intentions.move","from":0,"to":1},
                {"op":"summary.set","value":"Edited by Bryan"},
                {"op":"reflection.set","value":"New reflection"}
            ]
        })
        .to_string(),
        &[],
    );
    assert_success(&changed);
    let changed_body = report(&changed);
    let undo = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":changed_body["fingerprint_after"],
            "ops":changed_body["inverse_ops"],
        })
        .to_string(),
        &[],
    );
    assert_success(&undo);
    let restored = report(&undo);
    assert_eq!(restored["after"]["intentions"], json!(["one", "two"]));
    assert_eq!(restored["after"]["summary"], "Agent draft");
    assert_eq!(restored["after"]["reflection"], "Existing reflection");
    assert!(fs::read_to_string(day_path(vault.path()))
        .unwrap()
        .contains("summary_digest: sha256:1111111111111111"));
}

#[test]
fn fingerprint_ignores_body_only_changes_but_rejects_changed_fields() {
    let vault = vault("stale");
    write_day(vault.path(), "---\nintentions: [one]\n---\nfirst body\n");
    let original_expect = current_fingerprint(vault.path());
    let path = day_path(vault.path());
    fs::write(&path, "---\nintentions: [one]\n---\nchanged task body\n")
        .unwrap();
    let applied = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":original_expect,
            "ops":[{"op":"reflection.set","value":"Meaning"}]
        })
        .to_string(),
        &[],
    );
    assert_success(&applied);
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("changed task body"));

    let stale_expect = report(&applied)["fingerprint_before"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(
        &path,
        "---\nintentions: [changed]\nreflection: Meaning\n---\nbody\n",
    )
    .unwrap();
    let stale = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":stale_expect,
            "ops":[{"op":"summary.set","value":"A summary"}]
        })
        .to_string(),
        &[],
    );
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(report(&stale)["error"]["code"], "stale_note");
}

#[test]
fn malformed_intentions_can_be_replaced_but_not_individually_edited() {
    let vault = vault("malformed");
    write_day(vault.path(), "---\nintentions: one\n---\nbody\n");
    let expect = current_fingerprint(vault.path());
    let refused = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":expect,
            "ops":[{"op":"intentions.remove","index":0}]
        })
        .to_string(),
        &[],
    );
    assert_eq!(report(&refused)["error"]["code"], "malformed_field");

    let expect = current_fingerprint(vault.path());
    let fixed = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":expect,
            "ops":[{"op":"intentions.set","values":["repaired"]}]
        })
        .to_string(),
        &[],
    );
    assert_success(&fixed);
    assert_eq!(report(&fixed)["after"]["intentions"], json!(["repaired"]));
}

#[test]
fn dry_run_without_expect_returns_the_current_fields_and_fingerprint() {
    let vault = vault("dry-run");
    write_day(vault.path(), "---\nintentions: [one]\n---\nbody\n");
    let output = update(
        vault.path(),
        "yesterday",
        r#"{"schema_version":1,"ops":[]}"#,
        &["-n"],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["fingerprint_before"], body["fingerprint_after"]);
    assert_eq!(body["changed"], false);
}

#[test]
fn update_preserves_bom_crlf_and_unowned_yaml() {
    let vault = vault("crlf-bom");
    write_day(
        vault.path(),
        "\u{feff}---\r\nother: preserve # comment\r\nintentions: [old] # keep list comment\r\n---\r\nbody stays\r\n",
    );
    let expect = current_fingerprint(vault.path());
    let updated = update(
        vault.path(),
        "yesterday",
        &json!({
            "schema_version":1,
            "expect":expect,
            "ops":[{"op":"intentions.set","values":["[[note]]","new"]}]
        })
        .to_string(),
        &[],
    );
    assert_success(&updated);
    let note = fs::read(day_path(vault.path())).unwrap();
    let note = String::from_utf8(note).unwrap();
    assert!(note.starts_with("\u{feff}---\r\n"));
    assert!(note.contains("other: preserve # comment\r\n"));
    assert!(note.contains(
        "intentions: # keep list comment\r\n  - \"[[note]]\"\r\n  - new\r\n"
    ));
    assert!(note.ends_with("body stays\r\n"));
}

#[test]
fn missing_daily_note_and_busy_lock_are_typed_refusals() {
    let vault = vault("refusals");
    let missing = update(
        vault.path(),
        "yesterday",
        r#"{"schema_version":1,"expect":"sha256:missing","ops":[]}"#,
        &[],
    );
    assert_eq!(report(&missing)["error"]["code"], "missing_note");
    assert!(report(&missing)["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("Obsidian"));

    let missing_period = update(
        vault.path(),
        "2026-W47",
        r#"{"schema_version":1,"expect":"sha256:missing","ops":[]}"#,
        &[],
    );
    assert_eq!(report(&missing_period)["error"]["code"], "missing_note");
    assert!(report(&missing_period)["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("bob periods ensure 2026-W47"));

    write_day(vault.path(), "---\nintentions: [one]\n---\nbody\n");
    let expect = current_fingerprint(vault.path());
    let lock_path = vault.path().join("vault.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock.lock_exclusive().unwrap();
    let busy = update(
        vault.path(),
        "yesterday",
        &json!({"schema_version":1,"expect":expect,"ops":[{"op":"summary.set","value":"summary"}] }).to_string(),
        &[],
    );
    let _ = FileExt::unlock(&lock);
    assert_eq!(busy.status.code(), Some(1));
    assert_eq!(report(&busy)["error"]["code"], "lock_busy");
}

#[test]
fn show_accepts_daily_subjects_and_returns_all_three_fields_and_state() {
    let vault = vault("show-day");
    write_day(
        vault.path(),
        "---\nintentions: [\"Ship [[record-walk]]\"]\nsummary: Shipped the rail.\nsummary_digest: sha256:1111111111111111\nreflection: Felt focused.\n---\nbody\n",
    );
    let mut command = bob_command();
    command
        .args(["periods", "show", "yesterday", "-f", "json"])
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    let output = command.output().unwrap();
    assert_success(&output);
    let body: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(body["subject"]["id"], "2026-11-18");
    assert_eq!(body["subject"]["path"], "2026/20261118.md");
    assert_eq!(body["intentions"][0], "Ship [[record-walk]]");
    assert_eq!(body["summary"], "Shipped the rail.");
    assert_eq!(body["reflection"], "Felt focused.");
    assert_eq!(body["review_state"], "reflected");

    let mut human = bob_command();
    human
        .args(["periods", "show", "20261118"])
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    let human = human.output().unwrap();
    assert_success(&human);
    assert!(stdout(&human).contains("2026/20261118.md"));
    assert!(stdout(&human).contains("Review state  reflected"));
}
