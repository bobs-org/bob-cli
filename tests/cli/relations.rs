//! `bob relations` list: catalog, hide/unhide journal, and fold.

use crate::support::*;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn write_relation_vault(root: &Path) {
    write_file(
        &root.join("work.md"),
        "- [ ] Ship it #task ^a\n  - 🪢 **LINKS:** inspired by [[other#^x]] • implements [[other#^y]]\n",
    );
    write_file(
        &root.join("more.md"),
        "- [x] Done thing #task ^b\n  - 🪢 **LINKS:** Inspired By [[other#^x]] • duplicate of [[other#^z]]\n",
    );
    write_file(
        &root.join("done/old_done.md"),
        "- [x] Archived #task ^c\n  - 🪢 **LINKS:** implements [[other#^y]]\n",
    );
}

fn relations_command(vault: &Path, cache: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
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
fn relations_help_is_in_tasks_and_projects_and_lists_options() {
    let output = bob_command()
        .args(["relations", "--help"])
        .output()
        .expect("bob relations --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("List relation labels in the vault"),
        "group about:\n{help}"
    );
    assert!(
        help.contains("Bare `bob relations` runs the read-only list"),
        "default list:\n{help}"
    );
    assert!(help.contains("list"), "member list:\n{help}");
    assert!(help.contains("rename"), "member rename:\n{help}");
    assert!(help.contains("hide"), "member hide:\n{help}");
    assert!(help.contains("unhide"), "member unhide:\n{help}");
    assert!(
        help.contains("docs/task-relations.md"),
        "contract link:\n{help}"
    );

    let list = bob_command()
        .args(["relations", "list", "--help"])
        .output()
        .expect("bob relations list --help");
    assert_success(&list);
    assert_text_order(
        &stdout(&list),
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-H, --hidden",
            "-n, --limit",
            "-q, --query",
        ],
    );

    for verb in ["hide", "unhide"] {
        let output = bob_command()
            .args(["relations", verb, "--help"])
            .output()
            .unwrap_or_else(|error| {
                panic!("bob relations {verb} --help: {error}")
            });
        assert_success(&output);
        assert_text_order(
            &stdout(&output),
            &["-b, --bob-dir", "-f, --format", "-h, --help"],
        );
    }

    let rename = bob_command()
        .args(["relations", "rename", "--help"])
        .output()
        .expect("bob relations rename --help");
    assert_success(&rename);
    assert_text_order(
        &stdout(&rename),
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-m, --merge",
            "-o, --offline",
        ],
    );
    assert!(stdout(&rename).contains("<OLD> <NEW>"));
}

#[test]
fn relations_lists_catalog_counts_on_scratch_vault() {
    let temp = TempDir::new("bob-cli-relations-list");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);

    let output = relations_command(&vault, &cache)
        .args(["relations", "-f", "json"])
        .output()
        .expect("bob relations -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON parses");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["counts"]["labels"], 3);
    assert_eq!(value["counts"]["links"], 5);
    assert_eq!(value["counts"]["hidden"], 0);
    let labels = value["labels"].as_array().expect("labels array");
    assert_eq!(labels.len(), 3);
    // Most-used first: implements and inspired by tie at 2, key order wins.
    assert_eq!(labels[0]["key"], "implements");
    assert_eq!(labels[0]["count"], 2);
    assert_eq!(labels[1]["key"], "inspired by");
    assert_eq!(labels[1]["count"], 2);
    assert_eq!(labels[2]["key"], "duplicate of");
    assert_eq!(labels[2]["inverse"], "duplicated by");
    assert_eq!(labels[2]["replacement"], "|\"duplicate of\"");
    assert_eq!(labels[2]["hide_replacement"], "||\"duplicate of\"");
    assert_eq!(
        value["renamed_reappeared"].as_array().expect("array").len(),
        0
    );

    let human = relations_command(&vault, &cache)
        .args(["relations"])
        .output()
        .expect("bob relations human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("RELATIONS"), "heading:\n{text}");
    assert!(text.contains("3 labels"), "counts:\n{text}");
    assert!(text.contains("5 links"), "counts:\n{text}");
    assert!(text.contains("Inspired By"), "row:\n{text}");
    assert!(!text.contains("◌ hidden"), "no hidden rows:\n{text}");
}

#[test]
fn relations_hide_unhide_round_trip_folds_across_hosts() {
    let temp = TempDir::new("bob-cli-relations-round-trip");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);

    let hide = relations_command(&vault, &cache)
        .args(["relations", "hide", "inspired by", "-f", "json"])
        .output()
        .expect("hide");
    assert_success(&hide);
    let value: Value = serde_json::from_str(stdout(&hide).trim()).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["hidden"], json!(["Inspired By"]));

    // A second host file unhides; the later event wins the fold.
    fs::create_dir_all(vault.join(".bob/relations")).unwrap();
    fs::write(
        vault.join(".bob/relations/mac.jsonl"),
        "{\"v\":1,\"ts\":\"2099-01-01T00:00:00+00:00\",\"host\":\"mac\",\"kind\":\"relation_label.unhide\",\"key\":\"inspired by\",\"name\":\"inspired by\",\"via\":\"test\"}\n",
    )
    .unwrap();

    let list = relations_command(&vault, &cache)
        .args(["relations", "-f", "json"])
        .output()
        .expect("list after cross-host unhide");
    assert_success(&list);
    let value: Value = serde_json::from_str(stdout(&list).trim()).unwrap();
    assert_eq!(value["counts"]["hidden"], 0);

    // Hiding twice is an unchanged no-op that writes nothing.
    let hide = relations_command(&vault, &cache)
        .args(["relations", "hide", "implements", "-f", "json"])
        .output()
        .expect("hide implements");
    assert_success(&hide);
    let again = relations_command(&vault, &cache)
        .args(["relations", "hide", "implements", "-f", "json"])
        .output()
        .expect("hide implements again");
    assert_eq!(again.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&again).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unchanged");

    let unhide = relations_command(&vault, &cache)
        .args(["relations", "unhide", "implements", "-f", "json"])
        .output()
        .expect("unhide implements");
    assert_success(&unhide);
    let value: Value = serde_json::from_str(stdout(&unhide).trim()).unwrap();
    assert_eq!(value["unhidden"], json!(["implements"]));
}

#[test]
fn relations_hide_refuses_unknown_and_invalid_labels() {
    let temp = TempDir::new("bob-cli-relations-refusals");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);

    let unknown = relations_command(&vault, &cache)
        .args(["relations", "hide", "nope", "-f", "json"])
        .output()
        .expect("hide unknown");
    assert_eq!(unknown.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&unknown).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unknown_label");

    // --allow-unused hides a label the catalog has not seen yet; it
    // stays listable at count 0 so it can be found and unhidden.
    let fresh = relations_command(&vault, &cache)
        .args(["relations", "hide", "brand new", "--allow-unused"])
        .output()
        .expect("hide allow-unused");
    assert_success(&fresh);
    let list = relations_command(&vault, &cache)
        .args(["relations", "-a", "-f", "json"])
        .output()
        .expect("list all");
    assert_success(&list);
    let value: Value = serde_json::from_str(stdout(&list).trim()).unwrap();
    let row = value["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .find(|row| row["key"] == "brand new")
        .expect("journal-only row")
        .clone();
    assert_eq!(row["count"], 0);
    assert_eq!(row["hidden"], true);

    let invalid = relations_command(&vault, &cache)
        .args(["relations", "hide", "bad (label)", "-f", "json"])
        .output()
        .expect("hide invalid");
    assert_eq!(invalid.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&invalid).trim()).unwrap();
    assert_eq!(value["code"], "invalid_label");
}

#[test]
fn relations_concurrent_hides_both_survive() {
    let temp = TempDir::new("bob-cli-relations-concurrent");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);
    let lock = temp.path().join("bob_sync.lock");

    let mut first = relations_command(&vault, &cache);
    first.env("BOB_VAULT_SYNC_LOCK_FILE", &lock).args([
        "relations",
        "hide",
        "implements",
        "-f",
        "json",
    ]);
    let mut second = relations_command(&vault, &cache);
    second.env("BOB_VAULT_SYNC_LOCK_FILE", &lock).args([
        "relations",
        "hide",
        "duplicate of",
        "-f",
        "json",
    ]);

    let handle_a =
        std::thread::spawn(move || first.output().expect("hide first"));
    let handle_b =
        std::thread::spawn(move || second.output().expect("hide second"));
    let out_a = handle_a.join().expect("join first");
    let out_b = handle_b.join().expect("join second");
    assert_success(&out_a);
    assert_success(&out_b);

    let journal = fs::read_to_string(vault.join(".bob/relations/apollo.jsonl"))
        .expect("journal");
    assert!(journal.contains("\"key\":\"implements\""), "{journal}");
    assert!(journal.contains("\"key\":\"duplicate of\""), "{journal}");
}

#[test]
fn relations_hide_lock_busy_when_maintenance_lock_held() {
    let temp = TempDir::new("bob-cli-relations-lock-busy");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);
    let lock = temp.path().join("bob_sync.lock");
    let _held = hold_vault_lock(&lock);

    let output = relations_command(&vault, &cache)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .args(["relations", "hide", "implements", "-f", "json"])
        .output()
        .expect("lock busy");
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "lock_busy");
    assert!(!vault.join(".bob/relations/apollo.jsonl").exists());
}

#[test]
fn relations_list_marks_cancel_labels_and_overlays_zero_use_configured() {
    let temp = TempDir::new("bob-cli-relations-list-cancel");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("work.md"),
        "- [ ] Ship it #task ^a\n  - 🪢 **LINKS:** duplicate of [[other#^x]]\n",
    );
    write_file(&vault.join("other.md"), "- [ ] Other #task ^x\n");
    let config = cache.join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n    - superseded by\n",
    );
    let mut json_command = relations_command(&vault, &cache);
    json_command.env("BOB_CONFIG_FILE", &config).args([
        "relations",
        "-f",
        "json",
    ]);
    let output = json_command.output().expect("cancel list json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    assert_eq!(value["cancel_labels"], 2);
    let labels = value["labels"].as_array().expect("labels");
    // The used cancel label carries the flag without `configured`.
    let used = labels
        .iter()
        .find(|row| row["key"] == "duplicate of")
        .expect("used cancel row");
    assert_eq!(used["cancels_source"], true);
    assert!(used.get("configured").is_none());
    // The default list excludes the zero-use configured label.
    assert!(!labels.iter().any(|row| row["key"] == "superseded by"));

    // `-a` includes the zero-use configured label as an overlay.
    let output = relations_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["relations", "-a", "-f", "json"])
        .output()
        .expect("cancel list all json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    let overlay = value["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .find(|row| row["key"] == "superseded by")
        .expect("overlay row");
    assert_eq!(overlay["cancels_source"], true);
    assert_eq!(overlay["configured"], true);
    assert_eq!(overlay["count"], 0);

    // The human header counts cancel labels and rows name the cue.
    let human = relations_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["relations", "-a"])
        .output()
        .expect("cancel list human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("2 cancel"), "header:\n{text}");
    assert!(text.contains("⊘ cancels source"), "row cue:\n{text}");
}

#[test]
fn relations_list_warns_once_for_invalid_cancel_config() {
    let temp = TempDir::new("bob-cli-relations-list-cancel-warn");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_relation_vault(&vault);
    let config = cache.join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n    - 42\n",
    );
    let output = relations_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["relations", "-f", "json"])
        .output()
        .expect("warn list json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("list JSON");
    let warnings = value["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0]
        .as_str()
        .expect("warning text")
        .contains("relations.cancel_labels"));

    let human = relations_command(&vault, &cache)
        .env("BOB_CONFIG_FILE", &config)
        .args(["relations"])
        .output()
        .expect("warn list human");
    assert_success(&human);
    let errors = stderr(&human);
    assert_eq!(
        errors.matches("relations.cancel_labels").count(),
        1,
        "warning prints once:\n{errors}"
    );
}
