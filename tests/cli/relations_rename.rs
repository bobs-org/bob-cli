//! bob relations rename: strict, previewed, single-commit label rewrite.

use crate::support::*;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

#[test]
fn wrong_vault_refuses_before_syncing_or_renaming() {
    let temp = TempDir::new("bob-cli-relations-rename-wrong-vault");
    let (vault, _) = seed_vault(temp.path());
    assert_vault_override_refused(
        bob_command().args(["relations", "rename", "old", "new"]),
        &vault,
    );
}

fn seed_vault(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("source.md"),
        concat!(
            "- [ ] Source #task ^source [priority:: high] [scheduled:: 2026-10-28]\n",
            "  - 🪢 **LINKS:** old [[target#^target]] • stay [[target#^other]]\n",
            "- [ ] Untagged source\n",
            "  - 🪢 **LINKS:** old [[target#^other]]\n",
            "\n~~~md\n",
            "- [ ] Fenced #task ^fenced\n",
            "  - 🪢 **LINKS:** old [[target#^other]]\n",
            "~~~\n",
        ),
    );
    write_file(
        &vault.join("target.md"),
        concat!(
            "- [ ] Target #task ^target\n",
            "  - 🪢 **LINKED FROM:** [[source#^source]] (old)\n",
            "- [ ] Other target #task ^other\n",
        ),
    );
    write_file(
        &vault.join("done/archive.md"),
        "- [x] Archived #task ^archived\n  - 🪢 **LINKS:** old [[target#^target]]\n",
    );
    write_file(
        &vault.join("2026/20261023.md"),
        "- [ ] Previous daily #task ^daily\n  - 🪢 **LINKS:** old [[target#^target]]\n",
    );
    write_file(
        &vault.join("projects/nested.md"),
        "- [ ] Nested project task #task ^nested\n  - 🪢 **LINKS:** old [[target#^target]]\n",
    );
    write_file(
        &vault.join(".bob/relations/apollo.jsonl"),
        "{\"v\":1,\"ts\":\"2020-01-01T00:00:00Z\",\"host\":\"apollo\",\"kind\":\"relation_label.hide\",\"key\":\"old\",\"name\":\"old\",\"via\":\"test\"}\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

fn relation_command(vault: &Path, cache: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", cache.join("bob_sync.lock"));
    command
}

fn rename_json(
    vault: &Path,
    cache: &Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let output = relation_command(vault, cache)
        .args(["relations", "rename"])
        .args(args)
        .args(["-f", "json"])
        .output()
        .expect("bob relations rename");
    let value = serde_json::from_slice(stdout(&output).as_bytes())
        .or_else(|_| serde_json::from_str(stdout(&output).trim()))
        .expect("rename JSON");
    (output, value)
}

fn note_snapshot(vault: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, at: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, result);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                result.insert(relative, fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(vault, vault, &mut result);
    result
}

#[test]
fn relations_rename_previews_rewrites_every_scope_and_reverts_as_one_commit() {
    let temp = TempDir::new("bob-cli-relations-rename");
    let (vault, cache) = seed_vault(temp.path());
    let original = note_snapshot(&vault);
    let original_journal =
        fs::read(vault.join(".bob/relations/apollo.jsonl")).unwrap();

    let (dry_run, preview) =
        rename_json(&vault, &cache, &["old", "prompted by", "-d"]);
    assert_success(&dry_run);
    assert_eq!(preview["mode"], "dry_run");
    assert_eq!(preview["from"]["key"], "old");
    assert_eq!(preview["to"]["key"], "prompted by");
    assert_eq!(preview["counts"]["links"], 4);
    assert_eq!(preview["counts"]["mirrors"], 1);
    assert_eq!(preview["counts"]["done_links"], 1);
    assert_eq!(preview["state"]["hidden_before"], true);
    assert_eq!(preview["state"]["hidden_after"], true);
    assert_eq!(preview["state"]["moved"], true);
    assert_eq!(preview["replacement"], "|\"prompted by\"");
    assert_eq!(preview["hide_replacement"], "||\"prompted by\"");
    assert!(preview["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "done/archive.md"));
    assert!(preview["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "2026/20261023.md"));
    assert_eq!(note_snapshot(&vault), original);
    assert_eq!(
        fs::read(vault.join(".bob/relations/apollo.jsonl")).unwrap(),
        original_journal
    );
    assert_eq!(
        stdout(&git_in(&vault, ["log", "--oneline"]))
            .lines()
            .count(),
        1
    );

    let (applied, report) =
        rename_json(&vault, &cache, &["old", "prompted by", "-o"]);
    assert_success(&applied);
    assert_eq!(report["mode"], "write");
    assert_eq!(
        report["commit"]["subject"],
        "bob relations rename: old → prompted by"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["log", "--oneline"]))
            .lines()
            .count(),
        2
    );
    assert!(fs::read_to_string(vault.join("source.md"))
        .unwrap()
        .contains("LINKS:** prompted by [[target#^target]] • stay"));
    assert!(fs::read_to_string(vault.join("target.md"))
        .unwrap()
        .contains("LINKED FROM:** [[source#^source]] (prompted by)"));
    assert!(fs::read_to_string(vault.join("done/archive.md"))
        .unwrap()
        .contains("LINKS:** prompted by"));
    assert!(fs::read_to_string(vault.join("2026/20261023.md"))
        .unwrap()
        .contains("LINKS:** prompted by"));
    assert!(fs::read_to_string(vault.join("projects/nested.md"))
        .unwrap()
        .contains("LINKS:** prompted by"));
    let source = fs::read_to_string(vault.join("source.md")).unwrap();
    assert!(source.contains("[priority:: high] [scheduled:: 2026-10-28]"));
    assert!(source.contains("#task ^source"));
    assert!(source.contains("Untagged source\n  - 🪢 **LINKS:** old"));
    assert!(source.contains("Fenced #task ^fenced\n  - 🪢 **LINKS:** old"));

    let journal =
        fs::read_to_string(vault.join(".bob/relations/apollo.jsonl")).unwrap();
    let event: Value =
        serde_json::from_str(journal.lines().last().unwrap()).unwrap();
    assert_eq!(event["kind"], "relation_label.rename");
    assert_eq!(event["from"], "old");
    assert_eq!(event["to"], "prompted by");
    let listed = relation_command(&vault, &cache)
        .args(["relations", "-a", "-f", "json"])
        .output()
        .unwrap();
    assert_success(&listed);
    let listed: Value = serde_json::from_str(stdout(&listed).trim()).unwrap();
    let target = listed["labels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "prompted by")
        .unwrap();
    assert_eq!(target["hidden"], true);
    assert_eq!(target["name"], "prompted by");

    git_in(&vault, ["revert", "--no-edit", "HEAD"]);
    assert_eq!(note_snapshot(&vault), original);
    assert_eq!(
        fs::read(vault.join(".bob/relations/apollo.jsonl")).unwrap(),
        original_journal
    );
    assert!(stdout(&git_in(&vault, ["status", "--porcelain"])).is_empty());
}

#[test]
fn relations_rename_applies_case_only_and_requires_merge_for_existing_key() {
    let temp = TempDir::new("bob-cli-relations-rename-rules");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("one.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[x#^x]] • newer [[x#^y]]\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (exists, value) = rename_json(&vault, &cache, &["old", "newer", "-d"]);
    assert_eq!(exists.status.code(), Some(1));
    assert_eq!(value["code"], "label_exists");

    let (merge, preview) =
        rename_json(&vault, &cache, &["old", "newer", "-d", "--merge"]);
    assert_success(&merge);
    assert_eq!(preview["merge"], true);
    assert_eq!(preview["counts"]["links"], 1);
    assert_eq!(
        preview["files"][0]["lines"][0]["after"],
        "  - 🪢 **LINKS:** newer [[x#^x]] • newer [[x#^y]]"
    );

    let (same, value) = rename_json(&vault, &cache, &["old", "old", "-d"]);
    assert_eq!(same.status.code(), Some(1));
    assert_eq!(value["code"], "unchanged");

    let (case_only, preview) =
        rename_json(&vault, &cache, &["old", "OLD", "-d"]);
    assert_success(&case_only);
    assert_eq!(preview["from"]["key"], "old");
    assert_eq!(preview["to"]["key"], "old");
    assert!(preview["files"][0]["lines"][0]["after"]
        .as_str()
        .unwrap()
        .contains("OLD [[x#^x]]"));
}

#[test]
fn relations_rename_refuses_malformed_matching_lines() {
    let temp = TempDir::new("bob-cli-relations-rename-malformed");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("one.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[x#^x]] trailing prose\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (output, value) = rename_json(&vault, &cache, &["old", "new", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "malformed_links_line");
    assert!(value["message"].as_str().unwrap().contains("one.md:2"));
}

#[test]
fn relations_list_reports_a_label_used_again_after_rename() {
    let temp = TempDir::new("bob-cli-relations-rename-reappeared");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("one.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[x#^x]]\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    let (rename, _) = rename_json(&vault, &cache, &["old", "new", "-o"]);
    assert_success(&rename);
    write_file(
        &vault.join("offline-host.md"),
        "- [ ] B #task ^b\n  - 🪢 **LINKS:** old [[x#^y]]\n",
    );
    let listed = relation_command(&vault, &cache)
        .args(["relations", "-a", "-f", "json"])
        .output()
        .unwrap();
    assert_success(&listed);
    let value: Value = serde_json::from_str(stdout(&listed).trim()).unwrap();
    assert_eq!(value["renamed_reappeared"][0]["key"], "old");
    assert_eq!(value["renamed_reappeared"][0]["renamed_to"], "new");
    assert!(fs::read_to_string(vault.join("offline-host.md"))
        .unwrap()
        .contains("LINKS:** old"));
}
