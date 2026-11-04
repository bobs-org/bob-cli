//! `bob attachments import` and `bob attachments origin`.

use crate::support::*;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn attachments_command(vault: &Path, cache: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
}

fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("source dir");
    fs::write(&path, bytes).expect("write source");
    path
}

fn journal_events(vault: &Path) -> Vec<Value> {
    let path = vault.join(".bob/attachments/apollo.jsonl");
    let body = fs::read_to_string(&path).expect("read journal");
    body.lines()
        .map(|line| serde_json::from_str(line).expect("journal line parses"))
        .collect()
}

fn hold_vault_lock(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("lock dir");
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("open lock");
    fs2::FileExt::try_lock_exclusive(&file).expect("take lock");
    file
}

#[test]
fn attachments_group_help_lists_members_and_options_in_order() {
    let group = bob_command()
        .args(["attachments", "--help"])
        .output()
        .expect("bob attachments --help");
    assert_success(&group);
    let help = stdout(&group);
    assert!(
        help.contains("Copy files into the vault"),
        "group about:\n{help}"
    );
    assert!(help.contains("import"), "member import:\n{help}");
    assert!(help.contains("origin"), "member origin:\n{help}");
    assert!(help.contains("bob attachments <COMMAND>"), "usage:\n{help}");
    assert!(
        help.contains("docs/attachments.md"),
        "contract link:\n{help}"
    );

    let import = bob_command()
        .args(["attachments", "import", "--help"])
        .output()
        .expect("bob attachments import --help");
    assert_success(&import);
    assert_text_order(
        &stdout(&import),
        &["-b, --bob-dir", "-f, --format", "-h, --help"],
    );
    assert!(
        !stdout(&import).contains("--via"),
        "via stays hidden:\n{}",
        stdout(&import)
    );

    let origin = bob_command()
        .args(["attachments", "origin", "--help"])
        .output()
        .expect("bob attachments origin --help");
    assert_success(&origin);
    assert_text_order(
        &stdout(&origin),
        &["-b, --bob-dir", "-f, --format", "-h, --help"],
    );

    // Bare `bob attachments` prints help like `bob task`: no flag-only
    // read-only member, so there is no default to run.
    let bare = bob_command().arg("attachments").output().expect("bare");
    assert_eq!(bare.status.code(), Some(2), "{}", format_output(&bare));
    assert!(
        stderr(&bare).contains("bob attachments <COMMAND>"),
        "bare prints group help:\n{}",
        format_output(&bare)
    );
}

#[test]
fn attachments_import_copies_files_and_records_origins() {
    let temp = TempDir::new("bob-cli-attachments-import");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let src = temp.path().join("src");
    fs::create_dir_all(&vault).expect("vault");
    let shot = write_source(&src, "shot.png", b"shot-bytes");
    let doc = write_source(&src, "memo.txt", b"memo-bytes");

    let output = attachments_command(&vault, &cache)
        .args(["attachments", "import", "-f", "json"])
        .arg("--")
        .arg(&shot)
        .arg(&doc)
        .output()
        .expect("import");
    assert_success(&output);
    let report: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("import JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(report["schema_version"], json!(1));
    let attachments = report["attachments"].as_array().expect("attachments");
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0]["saved"], json!("img/shot.png"));
    assert_eq!(attachments[0]["kind"], json!("image"));
    assert_eq!(attachments[0]["reused"], json!(false));
    assert_eq!(attachments[0]["embed"], json!("![[img/shot.png|400]]"));
    assert_eq!(attachments[0]["source"], json!(shot.display().to_string()));
    assert_eq!(attachments[1]["saved"], json!("file/memo.txt"));
    assert_eq!(attachments[1]["kind"], json!("file"));
    assert_eq!(attachments[1]["embed"], json!("[[file/memo.txt]]"));

    assert_eq!(
        fs::read(vault.join("img/shot.png")).expect("vault copy"),
        b"shot-bytes"
    );
    let events = journal_events(&vault);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], json!("attachment.import"));
    assert_eq!(events[0]["saved"], json!("img/shot.png"));
    assert_eq!(events[0]["source"], json!(shot.display().to_string()));
    assert_eq!(events[0]["via"], json!("cli"));
    assert_eq!(events[0]["v"], json!(1));
    assert_eq!(events[0]["host"], json!("apollo"));

    // A second import of the same file is reused and still recorded.
    let output = attachments_command(&vault, &cache)
        .arg("attachments")
        .arg("import")
        .arg("--")
        .arg(&shot)
        .output()
        .expect("reimport");
    assert_success(&output);
    let line = stdout(&output).trim().to_string();
    assert!(
        line.starts_with("reused") && line.ends_with(" → img/shot.png"),
        "human reuse line:\n{}",
        format_output(&output)
    );
    assert_eq!(journal_events(&vault).len(), 3);

    // A conflicting name with different bytes gains a hash suffix, and
    // the internal client id lands on the event.
    let other = temp.path().join("other");
    let clash = write_source(&other, "shot.png", b"other-bytes");
    let output = attachments_command(&vault, &cache)
        .args(["attachments", "import", "-f", "json", "--via", "zoom"])
        .arg("--")
        .arg(&clash)
        .output()
        .expect("clash");
    assert_success(&output);
    let report: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("clash JSON: {error}\n{}", format_output(&output))
        });
    let saved = report["attachments"][0]["saved"]
        .as_str()
        .expect("saved")
        .to_string();
    assert!(
        saved.starts_with("img/shot-") && saved.ends_with(".png"),
        "hash suffix: {saved}"
    );
    let events = journal_events(&vault);
    assert_eq!(events.len(), 4);
    assert_eq!(events[3]["saved"], json!(saved));
    assert_eq!(events[3]["via"], json!("zoom"));
}

#[test]
fn attachments_import_classifies_like_clipboard_lines() {
    let temp = TempDir::new("bob-cli-attachments-classify");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let home = temp.path().join("home");
    fs::create_dir_all(&vault).expect("vault");
    fs::create_dir_all(&home).expect("home");
    let shot = write_source(&home, "Mixed Case image.png", b"spaced");

    let uri = format!("file://{}", shot.display()).replace(' ', "%20");
    // Quoted, unwrapped URI, and angle-wrapped paths resolve through the
    // shared classifier used by `%c`.
    for (raw, reused) in [
        (format!("'{}'", shot.display()), false),
        (uri.clone(), true),
        (format!("<{}>", shot.display()), true),
        (format!("<{uri}>"), true),
    ] {
        let output = attachments_command(&vault, &cache)
            .args(["attachments", "import", "-f", "json"])
            .arg("--")
            .arg(&raw)
            .env("HOME", &home)
            .output()
            .expect("classified import");
        assert_success(&output);
        let report: Value = serde_json::from_str(stdout(&output).trim())
            .unwrap_or_else(|error| {
                panic!("classify JSON: {error}\n{}", format_output(&output))
            });
        assert_eq!(
            report["attachments"][0]["source"],
            json!(shot.display().to_string()),
            "raw {raw}"
        );
        assert_eq!(
            report["attachments"][0]["display"],
            json!("~/Mixed Case image.png"),
            "raw {raw}"
        );
        assert_eq!(
            report["attachments"][0]["reused"],
            json!(reused),
            "raw {raw}"
        );
    }

    let origin = attachments_command(&vault, &cache)
        .args(["attachments", "origin", "-f", "json", "--"])
        .arg("img/Mixed Case image.png")
        .env("HOME", &home)
        .output()
        .expect("resolve wrapped import origin");
    assert_success(&origin);
    let report: Value = serde_json::from_str(stdout(&origin).trim())
        .unwrap_or_else(|error| {
            panic!("origin JSON: {error}\n{}", format_output(&origin))
        });
    assert_eq!(
        report["origin"]["source"],
        json!(shot.display().to_string())
    );
    assert_eq!(report["origin"]["display"], json!("~/Mixed Case image.png"));
    assert_eq!(report["origin"]["state"], json!("present"));
}

#[test]
fn attachments_import_rejects_bad_paths_with_coded_errors() {
    let temp = TempDir::new("bob-cli-attachments-errors");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let src = temp.path().join("src");
    fs::create_dir_all(&vault).expect("vault");
    fs::create_dir_all(&src).expect("src");

    for (args, code, message) in [
        (
            vec!["/definitely/missing/x.png".to_string()],
            "missing",
            "attachment does not exist",
        ),
        (
            vec![src.display().to_string()],
            "not_a_file",
            "attachment is not a file",
        ),
        (
            vec!["relative/x.png".to_string()],
            "not_absolute",
            "attachment is not an absolute path",
        ),
    ] {
        let output = attachments_command(&vault, &cache)
            .arg("attachments")
            .arg("import")
            .arg("-f")
            .arg("json")
            .arg("--")
            .args(&args)
            .output()
            .expect("bad import");
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        let error: Value = serde_json::from_str(stdout(&output).trim())
            .unwrap_or_else(|error| {
                panic!("error JSON: {error}\n{}", format_output(&output))
            });
        assert_eq!(error["ok"], json!(false));
        assert_eq!(error["code"], json!(code));
        assert!(
            error["message"].as_str().unwrap_or("").contains(message),
            "message for {code}:\n{}",
            format_output(&output)
        );
        assert!(
            !vault.join(".bob/attachments").exists()
                || journal_events_opt(&vault).is_empty(),
            "failed import writes no journal"
        );
    }

    let many: Vec<String> =
        (0..11).map(|index| format!("/tmp/x{index}.png")).collect();
    let output = attachments_command(&vault, &cache)
        .arg("attachments")
        .arg("import")
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(&many)
        .output()
        .expect("too many");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let error: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("too_many JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(error["code"], json!("too_many"));
}

fn journal_events_opt(vault: &Path) -> Vec<Value> {
    let path = vault.join(".bob/attachments/apollo.jsonl");
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines()
        .map(|line| serde_json::from_str(line).expect("journal line parses"))
        .collect()
}

#[test]
fn attachments_import_lock_busy_and_cleanup_on_journal_failure() {
    let temp = TempDir::new("bob-cli-attachments-lock");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let src = temp.path().join("src");
    let isolation = temp.path().join("iso");
    fs::create_dir_all(&vault).expect("vault");
    let shot = write_source(&src, "shot.png", b"shot");

    let lock = isolation.join("busy.lock");
    let _held = hold_vault_lock(&lock);
    let output = attachments_command(&vault, &cache)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["attachments", "import", "-f", "json"])
        .arg("--")
        .arg(&shot)
        .output()
        .expect("locked import");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let error: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("lock JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(error["code"], json!("lock_busy"));
    assert!(
        !vault.join("img/shot.png").exists(),
        "locked import writes no file"
    );
    drop(_held);

    // A journal append failure removes the files the import created.
    let journal_host = vault.join(".bob/attachments/apollo.jsonl");
    fs::create_dir_all(journal_host.parent().expect("parent"))
        .expect("journal dir");
    fs::create_dir_all(&journal_host).expect("journal path is a directory");
    let output = attachments_command(&vault, &cache)
        .args(["attachments", "import", "-f", "json"])
        .arg("--")
        .arg(&shot)
        .output()
        .expect("journal-failed import");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let error: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("io JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(error["code"], json!("io"));
    assert!(
        !vault.join("img/shot.png").exists(),
        "journal failure removes the created copy"
    );
}

#[test]
fn attachments_origin_resolves_recorded_copies() {
    let temp = TempDir::new("bob-cli-attachments-origin");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let src = temp.path().join("src");
    fs::create_dir_all(&vault).expect("vault");
    let shot = write_source(&src, "shot.png", b"shot");
    fs::write(vault.join("img-plain.png"), b"plain").expect("plain");

    let import = attachments_command(&vault, &cache)
        .args(["attachments", "import", "-f", "json"])
        .arg("--")
        .arg(&shot)
        .output()
        .expect("import");
    assert_success(&import);

    let output = attachments_command(&vault, &cache)
        .args(["attachments", "origin", "-f", "json"])
        .arg("--")
        .arg("img/shot.png")
        .output()
        .expect("origin json");
    assert_success(&output);
    let report: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("origin JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(report["schema_version"], json!(1));
    assert_eq!(report["path"], json!("img/shot.png"));
    assert_eq!(
        report["origin"]["source"],
        json!(shot.display().to_string())
    );
    assert_eq!(report["origin"]["state"], json!("present"));
    assert_eq!(report["origin"]["host"], json!("apollo"));
    assert_eq!(report["origin"]["via"], json!("cli"));
    assert!(report["origin"]["ts"].is_string());
    assert_eq!(report["warnings"], json!([]));

    let output = attachments_command(&vault, &cache)
        .arg("attachments")
        .arg("origin")
        .arg("--")
        .arg("img/shot.png")
        .output()
        .expect("origin human");
    assert_success(&output);
    assert!(
        stdout(&output).contains("present · apollo"),
        "human origin:\n{}",
        format_output(&output)
    );

    let output = attachments_command(&vault, &cache)
        .args(["attachments", "origin", "-f", "json"])
        .arg("--")
        .arg("img-plain.png")
        .output()
        .expect("null origin");
    assert_success(&output);
    let report: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("null JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(report["origin"], json!(null));

    let output = attachments_command(&vault, &cache)
        .arg("attachments")
        .arg("origin")
        .arg("--")
        .arg("img-plain.png")
        .output()
        .expect("null human");
    assert_success(&output);
    assert_eq!(
        stdout(&output).trim(),
        "img-plain.png has no recorded origin"
    );

    let output = attachments_command(&vault, &cache)
        .args(["attachments", "origin", "-f", "json"])
        .arg("--")
        .arg("img/ghost.png")
        .output()
        .expect("missing origin");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let error: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("not_found JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(error["code"], json!("not_found"));
}

#[test]
fn attachments_capture_clip_records_origins_and_keeps_json_stable() {
    let temp = TempDir::new("bob-cli-attachments-capture");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    let src = temp.path().join("src");
    fs::create_dir_all(&vault).expect("vault");
    let shot = write_source(&src, "clip.png", b"clip-bytes");
    write_file(
        &vault.join("work.md"),
        "# Work\n## Tasks\n- [ ] #task Existing\n",
    );
    let clipboard = temp.path().join("clipboard.sh");
    write_executable(
        &clipboard,
        &format!("#!/bin/sh\nprintf '%s\\n' '<{}>'\n", shot.display()),
    );

    let output = attachments_command(&vault, &cache)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["file clip", "%c", "@work"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-11-03 12:00:00")
        .output()
        .expect("capture");
    assert_success(&output);
    let capture: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("capture JSON: {error}\n{}", format_output(&output))
        });
    let attachments = capture["clip"]["attachments"]
        .as_array()
        .expect("clip attachments");
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0],
        json!({
            "source": shot.display().to_string(),
            "saved": "img/clip.png",
            "kind": "image",
            "reused": false,
        }),
        "capture JSON stays byte-identical: no new attachment fields"
    );

    let events = journal_events(&vault);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["via"], json!("capture"));
    assert_eq!(events[0]["saved"], json!("img/clip.png"));

    // A dry run plans the same copy but appends nothing.
    let output = attachments_command(&vault, &cache)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .args(["dry clip", "%c", "@work"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-11-03 12:00:00")
        .output()
        .expect("dry run");
    assert_success(&output);
    assert_eq!(journal_events(&vault).len(), 1, "dry run appends nothing");
}
