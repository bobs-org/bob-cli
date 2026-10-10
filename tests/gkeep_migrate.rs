//! `bob gkeep migrate-markers` offline migration integration tests.
//!
//! No fake adapter, no network, no config: migration operates on vault
//! Markdown only.

mod gkeep_support;

use std::{fs, path::Path, process::Output};

use gkeep_support::{stderr, stdout, GkeepEnv, TempDir};

fn run_migrate(env: &GkeepEnv, args: &[&str]) -> Output {
    let mut cmd = env.command();
    cmd.env("TZ", "UTC").arg("gkeep").arg("migrate-markers");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.output().expect("run bob gkeep migrate-markers")
}

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn store_files(vault: &Path) -> Vec<std::path::PathBuf> {
    let dir = vault.join(".bob/gkeep/imports");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    out.sort();
    out
}

#[test]
fn dry_run_reports_without_writing_or_metadata() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-dry");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Call dentist [created::2026-09-27]\n\t%%gkeep:v1:note-1:0123456789ab%%\n",
    );
    let out = run_migrate(&env, &["-d"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stdout(&out) + &stderr(&out);
    assert!(body.contains("note-1"), "{body}");
    // Zero writes: the marker is still there and no store exists.
    let after =
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read");
    assert!(after.contains("%%gkeep:v1:note-1:"), "{after}");
    assert!(
        store_files(env.vault()).is_empty(),
        "dry-run writes no metadata"
    );
}

#[test]
fn removes_standalone_markers_and_creates_verified_receipts() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-standalone");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Call dentist [created::2026-09-27]\n\t%%gkeep:v1:note-1:0123456789ab%%\n",
    );
    let out = run_migrate(&env, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    let after =
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read");
    assert!(!after.contains("%%gkeep:"), "{after}");
    assert!(after.contains("Call dentist"), "{after}");
    let files = store_files(env.vault());
    assert_eq!(files.len(), 1);
    let tx: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&files[0]).expect("read tx"))
            .expect("tx json");
    assert_eq!(tx["schema_version"], serde_json::json!(1));
    assert_eq!(tx["destination"], serde_json::json!("gkeep_inbox.md"));
    let entries = tx["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], serde_json::json!("note-1"));
    assert_eq!(entries[0]["fp"], serde_json::json!("0123456789ab"));
    assert_eq!(entries[0]["state"], serde_json::json!("verified"));
    assert!(entries[0].get("intended").is_none());

    // Rerun after success performs no writes and creates no commit.
    let second = run_migrate(&env, &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert_eq!(
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read"),
        after,
        "no-op rerun"
    );
    assert_eq!(store_files(env.vault()).len(), 1, "no extra import");
}

#[test]
fn source_child_keeps_link_and_removes_only_marker() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-source");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Old task [created::2026-09-20]\n\t- Source: [Google Keep](https://keep.google.com/) \xc2\xb7 2026-09-20 08:00 %%gkeep:v1:note-2:ffffffffffff%%\n",
    );
    let out = run_migrate(&env, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let after =
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read");
    assert!(!after.contains("%%gkeep:"), "{after}");
    assert!(
        after.contains("[Google Keep](https://keep.google.com/)"),
        "{after}"
    );
    assert!(after.contains("- Source:"), "{after}");
}

#[test]
fn malformed_ambiguous_and_code_examples_are_retained() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-retain");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Keep me [created::2026-09-27]\n\t- See %%gkeep:v1:note-1:0123456789AB%% (uppercase, malformed)\n- [ ] #task Conflict [created::2026-09-27]\n\t%%gkeep:v1:a:0123456789ab%%\n\t%%gkeep:v1:b:ffffffffffff%%\n",
    );
    write(
        &env.vault().join("notes.md"),
        b"```\n%%gkeep:v1:note-9:000000000000%%\n```\n",
    );
    let out = run_migrate(&env, &[]);
    // Ambiguous block retained: exit is still 0 when nothing else fails?
    // Conflicting markers in one block are skips, not failures.
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let inbox =
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read");
    assert!(
        inbox.contains("0123456789AB"),
        "malformed retained:\n{inbox}"
    );
    assert!(
        inbox.contains("%%gkeep:v1:a:"),
        "ambiguous retained:\n{inbox}"
    );
    assert!(
        inbox.contains("%%gkeep:v1:b:"),
        "ambiguous retained:\n{inbox}"
    );
    let notes = fs::read_to_string(env.vault().join("notes.md")).expect("read");
    assert!(
        notes.contains("%%gkeep:v1:note-9:"),
        "code retained:\n{notes}"
    );
}

#[test]
fn done_tasks_migrated_and_excluded_dirs_skipped() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-done");
    write(
        &env.vault().join("done/triaged.md"),
        b"- [x] #task Old [created::2026-09-20]\n\t%%gkeep:v1:note-2:ffffffffffff%%\n",
    );
    write(
        &env.vault().join(".obsidian/config.md"),
        b"%%gkeep:v1:note-9:000000000000%%\n",
    );
    let out = run_migrate(&env, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let done =
        fs::read_to_string(env.vault().join("done/triaged.md")).expect("read");
    assert!(!done.contains("%%gkeep:"), "{done}");
    let excluded = fs::read_to_string(env.vault().join(".obsidian/config.md"))
        .expect("read");
    assert!(excluded.contains("%%gkeep:v1:note-9:"), "excluded skipped");
}

#[test]
fn json_shape_and_line_endings_preserved() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-json");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task CRLF [created::2026-09-27]\r\n\t%%gkeep:v1:note-1:0123456789ab%%\r\n",
    );
    let out = run_migrate(&env, &["-f", "json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["schema_version"], serde_json::json!(1));
    assert_eq!(doc["ok"], serde_json::json!(true));
    assert_eq!(doc["dry_run"], serde_json::json!(false));
    let raw = fs::read(env.vault().join("gkeep_inbox.md")).expect("read raw");
    assert!(raw.windows(2).any(|pair| pair == b"\r\n"), "CRLF preserved");
    for (i, byte) in raw.iter().enumerate() {
        if *byte == b'\n' {
            assert!(i > 0 && raw[i - 1] == b'\r', "bare LF at {i}");
        }
    }
    assert!(!String::from_utf8_lossy(&raw).contains("%%gkeep:"));
}

#[test]
fn needs_no_config_or_adapter() {
    let dir = TempDir::new("bob-cli-gkeep-migrate-noconfig");
    let vault = dir.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");
    write(
        &vault.join("gkeep_inbox.md"),
        b"- [ ] #task Solo [created::2026-09-27]\n\t%%gkeep:v1:note-1:0123456789ab%%\n",
    );
    let mut cmd = std::process::Command::new(gkeep_support::BOB_BIN);
    cmd.env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", dir.path().join("missing.yml"))
        .env_remove("BOB_GKEEP_ADAPTER")
        .env("TZ", "UTC")
        .arg("gkeep")
        .arg("migrate-markers");
    let out = cmd.output().expect("run migrate");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let after = fs::read_to_string(vault.join("gkeep_inbox.md")).expect("read");
    assert!(!after.contains("%%gkeep:"));
}

fn write_allowlist(vault: &Path, extra: &str) {
    fs::write(
        vault.join(".gitignore"),
        format!("{}{extra}", gkeep_support::production_allowlist()),
    )
    .expect("gitignore");
}

#[test]
fn restrictive_allowlist_leaves_markers_and_evidence_untouched() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-ignore");
    gkeep_support::init_git(env.vault());
    write_allowlist(env.vault(), "");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Call dentist [created::2026-09-27]\n\t%%gkeep:v1:note-1:0123456789ab%%\n",
    );
    write(
        &env.vault().join(".bob/gkeep/imports/preexisting.json"),
        br#"{
  "schema_version": 1,
  "transaction_id": "preexisting01",
  "destination": "other.md",
  "before_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "after_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "baseline_counts": {},
  "entries": [{
    "id": "note-9",
    "fp": "ffffffffffff",
    "path": "other.md",
    "block_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "state": "verified",
    "dest_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  }]
}
"#,
    );
    gkeep_support::git(
        env.vault(),
        &["add", "-f", ".gitignore", "gkeep_inbox.md"],
    );
    gkeep_support::git(env.vault(), &["commit", "-m", "before"]);
    let before_note =
        fs::read(env.vault().join("gkeep_inbox.md")).expect("note");
    let before_receipt =
        fs::read(env.vault().join(".bob/gkeep/imports/preexisting.json"))
            .expect("receipt");
    let out = run_migrate(&env, &["-f", "json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(doc["ok"], false);
    assert_eq!(
        fs::read(env.vault().join("gkeep_inbox.md")).expect("note after"),
        before_note
    );
    assert_eq!(
        fs::read(env.vault().join(".bob/gkeep/imports/preexisting.json"))
            .expect("receipt after"),
        before_receipt
    );
    assert_eq!(store_files(env.vault()).len(), 1);
}

#[test]
fn allowlist_migrates_markers_and_reuses_uncommitted_receipt() {
    let env = GkeepEnv::new("bob-cli-gkeep-migrate-allow");
    gkeep_support::init_git(env.vault());
    write_allowlist(env.vault(), "!/.bob/gkeep/imports/*.json\n");
    write(
        &env.vault().join("gkeep_inbox.md"),
        b"- [ ] #task Call dentist [created::2026-09-27]\n\t%%gkeep:v1:note-1:0123456789ab%%\n",
    );
    gkeep_support::git(env.vault(), &["add", ".gitignore", "gkeep_inbox.md"]);
    gkeep_support::git(env.vault(), &["commit", "-m", "before"]);
    let first = run_migrate(&env, &[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let after =
        fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read");
    assert!(!after.contains("%%gkeep:"), "{after}");
    let files = store_files(env.vault());
    assert_eq!(files.len(), 1);
    let names = gkeep_support::git_log_names(env.vault());
    assert!(
        names.iter().any(|name| name == "gkeep_inbox.md"),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with(".bob/gkeep/imports/")),
        "{names:?}"
    );

    write(
        &env.vault().join("second.md"),
        b"- [ ] #task Hardware [created::2026-09-26]\n\t%%gkeep:v1:note-2:ffffffffffff%%\n",
    );
    let prepared = run_migrate(&env, &["-C"]);
    assert_eq!(prepared.status.code(), Some(0), "{}", stderr(&prepared));
    let second_note =
        fs::read_to_string(env.vault().join("second.md")).expect("second");
    assert!(!second_note.contains("%%gkeep:"), "{second_note}");
    assert_eq!(store_files(env.vault()).len(), 2);
    write(
        &env.vault().join("second.md"),
        b"- [ ] #task Hardware [created::2026-09-26]\n\t%%gkeep:v1:note-2:ffffffffffff%%\n",
    );
    let reused = run_migrate(&env, &[]);
    assert_eq!(reused.status.code(), Some(0), "{}", stderr(&reused));
    assert_eq!(store_files(env.vault()).len(), 2, "reused existing receipt");
    let names = gkeep_support::git_log_names(env.vault());
    assert!(
        names.iter().any(|name| name == "second.md"),
        "reused receipt still committed with the note: {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with(".bob/gkeep/imports/")),
        "{names:?}"
    );
}
