//! Guarded `bob props show` and `bob props update` contract.

use crate::support::*;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const NOTE: &str =
    "---\ntitle: Groceries\ncount: 3\ndone: false\n---\n\n# Groceries\n";

#[path = "props_contract.rs"]
mod contract;

fn vault(tag: &str) -> TempDir {
    TempDir::new(&format!("bob-cli-props-{tag}"))
}

fn write_note(vault: &Path, rel: &str, contents: &str) {
    let path = vault.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).expect("write note");
}

fn show(vault: &Path, note: &str, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command
        .args(["props", "show", note, "-f", "json"])
        .env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.join("vault.lock"))
        .env("NO_COLOR", "1")
        .args(extra);
    command.output().expect("run bob props show")
}

fn update(
    vault: &Path,
    note: &str,
    envelope: &str,
    extra: &[&str],
) -> std::process::Output {
    let input = vault.join("update-envelope.json");
    fs::write(&input, envelope).expect("write envelope");
    let mut command = bob_command();
    command
        .args(["props", "update", note, "-i"])
        .arg(&input)
        .env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.join("vault.lock"))
        .env("NO_COLOR", "1")
        .args(extra);
    command.output().expect("run bob props update")
}

fn report(output: &std::process::Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("invalid JSON ({error}):\n{}", format_output(output))
    })
}

fn fingerprint(vault: &Path, note: &str) -> String {
    let output = show(vault, note, &[]);
    assert_success(&output);
    report(&output)["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned()
}

#[test]
fn props_retired_project_counts_are_deletable_without_changing_other_bytes() {
    let vault = vault("retired-counts");
    for note in ["project.md", "area.md", "done/archived.md"] {
        let kind = if note == "area.md" { "area" } else { "project" };
        let before = format!(
            "---\r\ntype: '[[{kind}]]'\r\nparent: '[[dev]]' # keep\r\nstatus: wip\r\ntask_count: 3\r\nopen_task_count: 1\r\ncustom: [a, b]\r\n---\r\nBody\r\n"
        );
        write_note(vault.path(), note, &before);
        let envelope = json!({
            "schema_version": 1,
            "expect": fingerprint(vault.path(), note),
            "ops": [
                {"op": "prop.unset", "key": "task_count"},
                {"op": "prop.unset", "key": "open_task_count"}
            ]
        })
        .to_string();
        let preview =
            update(vault.path(), note, &envelope, &["-d", "-f", "json"]);
        assert_success(&preview);
        assert_eq!(
            fs::read_to_string(vault.path().join(note)).unwrap(),
            before
        );
        let applied = update(vault.path(), note, &envelope, &["-f", "json"]);
        assert_success(&applied);
        assert_eq!(
            fs::read_to_string(vault.path().join(note)).unwrap(),
            before
                .replace("task_count: 3\r\n", "")
                .replace("open_task_count: 1\r\n", "")
        );
        let after = report(&applied);
        let undo = json!({
            "schema_version": 1,
            "expect": after["fingerprint_after"],
            "ops": after["inverse_ops"]
        })
        .to_string();
        assert_success(&update(vault.path(), note, &undo, &[]));
        assert_eq!(
            fs::read_to_string(vault.path().join(note)).unwrap(),
            before
        );
    }
}

fn hold_lock(vault: &Path) -> fs::File {
    let path = vault.join("vault.lock");
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

fn mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).expect("stat").modified().expect("mtime")
}

#[test]
fn show_reports_ordered_properties_with_capability_and_fingerprint() {
    let vault = vault("show");
    write_note(vault.path(), "notes/groceries.md", NOTE);

    let output = show(vault.path(), "notes/groceries", &[]);
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["ok"], true);
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["path"], "notes/groceries.md");
    assert_eq!(body["note_kind"], "note");
    assert_eq!(body["capabilities"], json!(["props_v1"]));
    assert!(
        body["fingerprint"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")),
        "fingerprint:\n{}",
        format_output(&output)
    );
    let keys: Vec<&str> = body["properties"]
        .as_array()
        .expect("properties")
        .iter()
        .map(|property| property["key"].as_str().expect("key"))
        .collect();
    assert_eq!(keys, vec!["title", "count", "done"]);
    assert_eq!(body["properties"][0]["line"], 2);
    assert_eq!(body["properties"][0]["kind"], "text");
    assert_eq!(body["properties"][1]["kind"], "number");
    assert_eq!(body["properties"][2]["kind"], "checkbox");
    assert_eq!(body["frontmatter"]["state"], "ok");

    // The stem form and the absolute vault path resolve to the same note.
    let stem = show(vault.path(), "notes/groceries.md", &[]);
    assert_success(&stem);
    assert_eq!(report(&stem)["path"], "notes/groceries.md");
    let absolute = vault
        .path()
        .join("notes/groceries.md")
        .display()
        .to_string();
    let via_absolute = show(vault.path(), &absolute, &[]);
    assert_success(&via_absolute);
    assert_eq!(report(&via_absolute)["path"], "notes/groceries.md");
}

#[test]
fn canonical_record_fields_route_to_the_card_and_refuse_generic_writes() {
    let vault = vault("canonical-record");
    write_note(vault.path(), "body.md", "---\ntype: [[area]]\n---\n");
    write_note(
        vault.path(),
        "eat.md",
        "---\ntype: [[album]]\nparent: [[body]]\nrecord:\n  noun: restaurant\n  fields:\n    rating:\n      values: number\n---\n",
    );
    let record = "---\ntype: \"[[record]]\"\nparent: \"[[eat]]\"\ntitle: Ramen\nrating: 5\n---\n\n# Ramen\n";
    write_note(vault.path(), "eat/ramen.md", record);
    let output = show(vault.path(), "eat/ramen.md", &[]);
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["note_kind"], "record");
    let rating = body["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "rating")
        .unwrap();
    assert_eq!(rating["role"], "card");
    assert_eq!(rating["owner"], "record");
    assert_eq!(rating["editor"], "==");

    let output = update(
        vault.path(),
        "eat/ramen.md",
        &json!({
            "schema_version": 1,
            "expect": body["fingerprint"],
            "ops": [{"op": "prop.set", "key": "rating", "value": 4}]
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_eq!(report(&output)["error"]["code"], "prop_owned");
    assert_eq!(
        fs::read_to_string(vault.path().join("eat/ramen.md")).unwrap(),
        record
    );
}

#[test]
fn help_lists_options_alphabetically_with_short_aliases() {
    for (command, options) in [
        ("show", vec!["-b, --bob-dir", "-f, --format", "-h, --help"]),
        (
            "update",
            vec![
                "-b, --bob-dir",
                "-d, --dry-run",
                "-f, --format",
                "-h, --help",
                "-i, --input",
            ],
        ),
    ] {
        let output = bob_command()
            .args(["props", command, "--help"])
            .output()
            .expect("run bob props help");
        assert_success(&output);
        let help = stdout(&output);
        assert_text_order(&help, &options);
        assert!(
            help.contains("Output format: text or json"),
            "help advertises text|json:\n{help}"
        );
        assert!(
            help.contains("[possible values: text, json]"),
            "human alias stays hidden:\n{help}"
        );
        assert_stdout_has_no_ansi(&output);
    }
    let output = bob_command()
        .arg("props")
        .arg("--help")
        .output()
        .expect("run bob props --help");
    assert_success(&output);
    assert_text_order(&stdout(&output), &["\n  show ", "\n  update "]);
}

#[test]
fn show_human_lists_properties_and_key_roles() {
    let vault = vault("human");
    write_note(vault.path(), "notes/groceries.md", NOTE);

    let output = bob_command()
        .args(["props", "show", "notes/groceries.md"])
        .env("BOB_DIR", vault.path())
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.path().join("vault.lock"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob props show");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("notes/groceries.md"), "text:\n{text}");
    assert!(text.contains("title"), "text:\n{text}");
    assert!(text.contains("Groceries"), "text:\n{text}");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn show_absent_frontmatter_has_no_properties_and_empty_fingerprint() {
    let vault = vault("absent");
    write_note(vault.path(), "notes/plain.md", "# Plain\n");

    let output = show(vault.path(), "notes/plain.md", &[]);
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["frontmatter"]["state"], "absent");
    assert_eq!(body["properties"], json!([]));
    assert!(
        body["fingerprint"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")),
        "empty mapping still fingerprints:\n{}",
        format_output(&output)
    );
}

#[test]
fn show_damaged_yaml_reports_recovery_while_update_refuses() {
    let vault = vault("damaged");
    write_note(vault.path(), "notes/broken.md", "---\nname: [\n---\nbody\n");

    let output = show(vault.path(), "notes/broken.md", &[]);
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["frontmatter"]["state"], "invalid");
    assert_eq!(body["fingerprint"], Value::Null);

    let expect = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let output = update(
        vault.path(),
        "notes/broken.md",
        &json!({
            "schema_version": 1,
            "expect": expect,
            "ops": [{"op": "prop.set", "key": "name", "value": "fixed"}],
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let body = report(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "prop_invalid_yaml");
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/broken.md")).unwrap(),
        "---\nname: [\n---\nbody\n",
        "refused batch leaves bytes unchanged"
    );
}

#[test]
fn dry_run_previews_without_writing_and_apply_writes_atomically() {
    let vault = vault("dry-run");
    let note = vault.path().join("notes/groceries.md");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let before_mtime = mtime(&note);
    let expect = fingerprint(vault.path(), "notes/groceries.md");

    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [
            {"op": "prop.set", "key": "count", "value": 4},
            {"op": "prop.set", "key": "aisle", "value": "produce", "after": "count"},
        ],
    })
    .to_string();
    let output = update(
        vault.path(),
        "notes/groceries.md",
        &envelope,
        &["-n", "-f", "json"],
    );
    // `-n` is not the props dry-run flag (`-d`), so clap must refuse it.
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    let output = update(
        vault.path(),
        "notes/groceries.md",
        &envelope,
        &["-d", "-f", "json"],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["ok"], true);
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["changed"], true);
    assert_eq!(body["path"], "notes/groceries.md");
    assert!(!body["inverse_ops"].as_array().expect("inverses").is_empty());
    assert!(!body["changes"].as_array().expect("changes").is_empty());
    assert_eq!(
        fs::read_to_string(&note).unwrap(),
        NOTE,
        "dry run writes nothing"
    );
    assert_eq!(mtime(&note), before_mtime, "dry run keeps mtime");

    let output = update(
        vault.path(),
        "notes/groceries.md",
        &envelope,
        &["-f", "json"],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["dry_run"], false);
    assert_eq!(body["changed"], true);
    let after = fs::read_to_string(&note).unwrap();
    assert!(after.contains("count: 4"), "applied:\n{after}");
    assert!(after.contains("aisle: produce"), "applied:\n{after}");
    assert!(after.ends_with("# Groceries\n"), "body kept:\n{after}");

    // The returned inverse ops replay to the exact original bytes.
    let inverses = body["inverse_ops"].clone();
    let reverted = update(
        vault.path(),
        "notes/groceries.md",
        &json!({
            "schema_version": 1,
            "expect": body["fingerprint_after"],
            "ops": inverses,
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_success(&reverted);
    assert_eq!(
        fs::read_to_string(&note).unwrap(),
        NOTE,
        "inverse ops restore exact bytes"
    );
}

#[test]
fn noop_update_writes_nothing_and_keeps_mtime() {
    let vault = vault("noop");
    let note = vault.path().join("notes/groceries.md");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let before_mtime = mtime(&note);
    let expect = fingerprint(vault.path(), "notes/groceries.md");

    let output = update(
        vault.path(),
        "notes/groceries.md",
        &json!({"schema_version": 1, "expect": expect, "ops": []}).to_string(),
        &["-f", "json"],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["changed"], false);
    assert_eq!(body["inverse_ops"], json!([]));
    assert_eq!(body["changes"], json!([]));
    assert_eq!(mtime(&note), before_mtime, "no-op keeps mtime");
}

#[test]
fn path_safety_rejects_traversal_outside_and_non_markdown() {
    let vault = vault("paths");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    write_note(vault.path(), "data.txt", "not markdown\n");
    let outside = vault.path().join("outside.md");
    fs::write(&outside, NOTE).expect("outside note");

    let nearby = vault
        .path()
        .join("outside.md")
        .display()
        .to_string()
        .replace("bob-cli-props-paths", "bob-cli-props-other");
    for note in ["../outside.md", "notes/../../outside.md", nearby.as_str()] {
        let output = show(vault.path(), note, &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "note {note}:\n{}",
            format_output(&output)
        );
    }
    let output = show(vault.path(), "data.txt", &[]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "prop_not_markdown",
        "{}",
        format_output(&output)
    );

    let output = show(vault.path(), "notes/missing.md", &[]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "prop_missing",
        "{}",
        format_output(&output)
    );
}

#[test]
#[cfg(unix)]
fn symlink_escape_is_refused() {
    let vault = vault("symlink");
    let elsewhere = TempDir::new("bob-cli-props-symlink-elsewhere");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let outside = elsewhere.path().join("outside-real.md");
    fs::write(&outside, NOTE).expect("outside note");
    std::os::unix::fs::symlink(&outside, vault.path().join("notes/link.md"))
        .expect("symlink");

    let output = show(vault.path(), "notes/link.md", &[]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "prop_symlink_escape",
        "{}",
        format_output(&output)
    );
}

#[test]
fn typed_refusals_keep_bytes_and_report_op_index() {
    let vault = vault("refusals");
    write_note(vault.path(), "notes/groceries.md", NOTE);

    // Malformed envelope.
    let output = update(
        vault.path(),
        "notes/groceries.md",
        "not json",
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "envelope_malformed",
        "{}",
        format_output(&output)
    );

    // Stale fingerprint.
    let output = update(
        vault.path(),
        "notes/groceries.md",
        &json!({
            "schema_version": 1,
            "expect": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "ops": [{"op": "prop.set", "key": "count", "value": 9}],
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "prop_stale",
        "{}",
        format_output(&output)
    );

    // Protected identity key reports its op index.
    let expect = fingerprint(vault.path(), "notes/groceries.md");
    let output = update(
        vault.path(),
        "notes/groceries.md",
        &json!({
            "schema_version": 1,
            "expect": expect,
            "ops": [
                {"op": "prop.set", "key": "count", "value": 9},
                {"op": "prop.set", "key": "type", "value": "other"},
            ],
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let body = report(&output);
    assert_eq!(body["error"]["code"], "prop_identity");
    assert_eq!(body["error"]["op_index"], 1);
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap(),
        NOTE,
        "refused batch leaves bytes unchanged"
    );
}

#[test]
fn lock_contention_reports_busy_without_writing() {
    let vault = vault("lock-busy");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let _held = hold_lock(vault.path());
    let expect = fingerprint(vault.path(), "notes/groceries.md");

    let output = update(
        vault.path(),
        "notes/groceries.md",
        &json!({
            "schema_version": 1,
            "expect": expect,
            "ops": [{"op": "prop.set", "key": "count", "value": 9}],
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(
        report(&output)["error"]["code"],
        "lock_busy",
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap(),
        NOTE,
        "busy lock writes nothing"
    );
}

#[test]
fn apply_replans_fresh_and_preserves_concurrent_body_edit() {
    let vault = vault("replan");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let expect = fingerprint(vault.path(), "notes/groceries.md");
    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [{"op": "prop.set", "key": "count", "value": 7}],
    })
    .to_string();
    let input = vault.path().join("update-envelope.json");
    fs::write(&input, &envelope).expect("write envelope");

    let held = hold_lock(vault.path());
    let vault_path = vault.path().to_owned();
    let input_path = input.clone();
    let child = std::thread::spawn(move || {
        bob_command()
            .args(["props", "update", "notes/groceries.md", "-i"])
            .arg(&input_path)
            .arg("-f")
            .arg("json")
            .env("BOB_DIR", &vault_path)
            .env("BOB_VAULT_SYNC_LOCK_FILE", vault_path.join("vault.lock"))
            .env("NO_COLOR", "1")
            .output()
            .expect("run bob props update")
    });
    // Let the child block on the held lock, then change only the body.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let concurrent =
        "---\ntitle: Groceries\ncount: 3\ndone: false\n---\n\n# Groceries\n\nBuy milk.\n";
    fs::write(vault.path().join("notes/groceries.md"), concurrent)
        .expect("concurrent body edit");
    drop(held);
    let output = child.join().expect("child");
    assert_success(&output);
    let final_note =
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap();
    assert!(final_note.contains("count: 7"), "replanned:\n{final_note}");
    assert!(final_note.contains("Buy milk."), "body kept:\n{final_note}");
}

#[test]
fn format_text_human_and_json_are_accepted() {
    let vault = vault("format");
    write_note(vault.path(), "notes/groceries.md", NOTE);

    let text = bob_command()
        .args(["props", "show", "notes/groceries.md", "-f", "text"])
        .env("BOB_DIR", vault.path())
        .env("NO_COLOR", "1")
        .output()
        .expect("show -f text");
    assert_success(&text);
    let text_out = stdout(&text);
    assert!(text_out.contains("Properties"), "text:\n{text_out}");

    let human = bob_command()
        .args(["props", "show", "notes/groceries.md", "-f", "human"])
        .env("BOB_DIR", vault.path())
        .env("NO_COLOR", "1")
        .output()
        .expect("show -f human");
    assert_success(&human);
    assert_eq!(stdout(&human), text_out);

    let json = show(vault.path(), "notes/groceries.md", &[]);
    assert_success(&json);
    assert_eq!(report(&json)["ok"], true);
}

#[test]
fn stdin_envelope_applies_and_six_ops_undo() {
    let vault = vault("stdin-ops");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let expect = fingerprint(vault.path(), "notes/groceries.md");
    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [
            {"op": "prop.set", "key": "aisle", "value": "produce", "after": "count"},
            {"op": "prop.set_yaml", "key": "notes", "yaml": "keep"},
            {"op": "prop.rename", "key": "aisle", "to": "section"},
            {"op": "prop.move", "key": "section", "before": "title"},
            {"op": "prop.unset", "key": "done"},
            {"op": "prop.restore", "key": "done", "raw": "done: false\n"},
        ],
    })
    .to_string();

    let mut preview = bob_command();
    preview
        .args([
            "props",
            "update",
            "notes/groceries.md",
            "-i",
            "-",
            "-d",
            "-f",
            "json",
        ])
        .env("BOB_DIR", vault.path())
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.path().join("vault.lock"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = preview.spawn().expect("spawn dry-run");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(envelope.as_bytes())
        .expect("write envelope");
    let output = child.wait_with_output().expect("dry-run");
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["dry_run"], true);
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap(),
        NOTE
    );

    let output = update(
        vault.path(),
        "notes/groceries.md",
        &envelope,
        &["-f", "json"],
    );
    assert_success(&output);
    let body = report(&output);
    assert_eq!(body["changed"], true);
    let applied =
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap();
    assert!(applied.contains("section: produce"), "applied:\n{applied}");
    assert!(applied.contains("notes: keep"), "applied:\n{applied}");
    assert!(applied.contains("done: false"), "applied:\n{applied}");

    let reverted = update(
        vault.path(),
        "notes/groceries.md",
        &json!({
            "schema_version": 1,
            "expect": body["fingerprint_after"],
            "ops": body["inverse_ops"],
        })
        .to_string(),
        &["-f", "json"],
    );
    assert_success(&reverted);
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap(),
        NOTE
    );
}

#[test]
fn absolute_note_with_relative_bob_dir_stays_confined() {
    let vault = vault("abs-rel");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let parent = vault.path().parent().expect("parent");
    let name = vault.path().file_name().expect("name");
    let absolute = vault
        .path()
        .join("notes/groceries.md")
        .display()
        .to_string();
    let output = bob_command()
        .current_dir(parent)
        .args([
            "props",
            "show",
            &absolute,
            "-b",
            name.to_str().expect("utf8"),
            "-f",
            "json",
        ])
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.path().join("vault.lock"))
        .env("NO_COLOR", "1")
        .output()
        .expect("show absolute + relative -b");
    assert_success(&output);
    assert_eq!(report(&output)["path"], "notes/groceries.md");
}

#[cfg(unix)]
fn wait_until_child_opens_lock(pid: u32, lock: &Path) {
    use std::os::unix::fs::MetadataExt;
    let want = fs::metadata(lock).ok();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let (Ok(entries), Some(want)) =
            (fs::read_dir(format!("/proc/{pid}/fd")), want.as_ref())
        {
            for entry in entries.flatten() {
                if fs::metadata(entry.path()).ok().is_some_and(|meta| {
                    meta.dev() == want.dev() && meta.ino() == want.ino()
                }) {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("child {pid} never opened {}", lock.display());
}

#[cfg(unix)]
fn spawn_update(
    vault: &Path,
    note: &str,
    envelope: &str,
) -> std::process::Child {
    let input = vault.join("update-envelope.json");
    fs::write(&input, envelope).expect("write envelope");
    bob_command()
        .args(["props", "update", note, "-i"])
        .arg(&input)
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault.join("vault.lock"))
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn update")
}

#[test]
#[cfg(unix)]
fn lock_wait_preserves_formatting_only_frontmatter_edit() {
    let vault = vault("lock-format");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let expect = fingerprint(vault.path(), "notes/groceries.md");
    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [{"op": "prop.set", "key": "count", "value": 7}],
    })
    .to_string();
    let held = hold_lock(vault.path());
    let child = spawn_update(vault.path(), "notes/groceries.md", &envelope);
    wait_until_child_opens_lock(child.id(), &vault.path().join("vault.lock"));
    let concurrent =
        "---\ntitle: Groceries\ncount: 3\ndone: false # keep\n---\n\n# Groceries\n";
    fs::write(vault.path().join("notes/groceries.md"), concurrent)
        .expect("formatting-only edit");
    drop(held);
    let output = child.wait_with_output().expect("wait");
    assert_success(&output);
    let final_note =
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap();
    assert!(final_note.contains("count: 7"), "applied:\n{final_note}");
    assert!(
        final_note.contains("done: false # keep"),
        "formatting kept:\n{final_note}"
    );
}

#[test]
#[cfg(unix)]
fn lock_wait_semantic_edit_is_stale() {
    let vault = vault("lock-stale");
    write_note(vault.path(), "notes/groceries.md", NOTE);
    let expect = fingerprint(vault.path(), "notes/groceries.md");
    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [{"op": "prop.set", "key": "count", "value": 7}],
    })
    .to_string();
    let held = hold_lock(vault.path());
    let child = spawn_update(vault.path(), "notes/groceries.md", &envelope);
    wait_until_child_opens_lock(child.id(), &vault.path().join("vault.lock"));
    fs::write(
        vault.path().join("notes/groceries.md"),
        "---\ntitle: Groceries\ncount: 9\ndone: false\n---\n\n# Groceries\n",
    )
    .expect("semantic edit");
    drop(held);
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(report(&output)["error"]["code"], "prop_stale");
}

#[test]
#[cfg(unix)]
fn lock_wait_schema_change_owns_a_previously_loose_field() {
    let vault = vault("lock-schema");
    write_note(
        vault.path(),
        "body.md",
        "---\ntype: [[area]]\n---\n# Body\n",
    );
    write_note(
        vault.path(),
        "eat.md",
        "---\ntype: [[album]]\nparent: [[body]]\ntitle: Restaurants\nrecord:\n  noun: restaurant\n  fields: {}\n---\n# Restaurants\n",
    );
    let record = "---\ntype: [[restaurant]]\nparent: [[eat]]\ntitle: Ramen\nrating: 5\ncreated: 2026-11-22\n---\n# Ramen\n";
    write_note(vault.path(), "eat/ramen.md", record);
    let expect = fingerprint(vault.path(), "eat/ramen.md");
    let envelope = json!({
        "schema_version": 1,
        "expect": expect,
        "ops": [{"op": "prop.set", "key": "rating", "value": 4}],
    })
    .to_string();
    let preview = update(
        vault.path(),
        "eat/ramen.md",
        &envelope,
        &["-d", "-f", "json"],
    );
    assert_success(&preview);
    assert_eq!(report(&preview)["changed"], true);

    let held = hold_lock(vault.path());
    let child = spawn_update(vault.path(), "eat/ramen.md", &envelope);
    wait_until_child_opens_lock(child.id(), &vault.path().join("vault.lock"));
    fs::write(
        vault.path().join("eat.md"),
        "---\ntype: [[album]]\nparent: [[body]]\ntitle: Restaurants\nrecord:\n  noun: restaurant\n  fields:\n    rating:\n      values: number\n---\n# Restaurants\n",
    )
    .expect("schema change");
    drop(held);
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(report(&output)["error"]["code"], "prop_owned");
    assert_eq!(
        fs::read_to_string(vault.path().join("eat/ramen.md")).unwrap(),
        record
    );
}
