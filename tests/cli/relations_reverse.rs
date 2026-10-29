//! Reverse relation-label renames: previewed, atomic edge reversal.
//!
//! Covers `bob relations rename OLD NEW -r/--reverse` (RV10): dry-run equals
//! apply, one revertable commit, minting, merge/conflict, closing-label
//! safety, mirrors, reconcile no-op, and CLI surfaces.

use crate::support::*;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

fn reverse_command(vault: &Path, cache: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", cache.join("bob_sync.lock"))
        .env("BOB_DAY_FILE", cache.join("2026-10-28.md"))
        .env("BOB_NOW", "2026-10-28 12:00:00");
    command
}

fn reverse_json(
    vault: &Path,
    cache: &Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let output = reverse_command(vault, cache)
        .args(["relations", "rename"])
        .args(args)
        .args(["-f", "json"])
        .output()
        .expect("bob relations rename --reverse");
    let stdout = stdout(&output);
    let value = serde_json::from_slice(stdout.as_bytes())
        .or_else(|_| serde_json::from_str(stdout.trim()))
        .expect("reverse JSON");
    (output, value)
}

fn note_snapshot(vault: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, at: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
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

fn seed_basic(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("a.md"),
        "- [ ] Task A #task ^a\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    write_file(&vault.join("b.md"), "- [ ] Task B #task ^b\n");
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

/// The approved migration shape: 14 OLD edges, 5 active-to-active,
/// 2 active-to-archive, 7 archive-to-archive, multi-entry lines, active
/// mirrors, existing LINKS/LINKED FROM on destinations, unrelated text.
fn seed_migration(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    // Active notes: 5 active-to-active edges.
    write_file(
        &vault.join("bob.md"),
        concat!(
            "- [ ] Feature one #task ^feat1\n",
            "  - 🪢 **LINKS:** old [[sase#^ref1]] • old [[sase#^ref2]]\n",
            "- [ ] Feature two #task ^feat2\n",
            "  - 🪢 **LINKS:** old [[sase#^ref3]]\n",
            "- [ ] Feature three #task ^feat3\n",
            "  - 🪢 **LINKS:** old [[sase#^ref1]]\n",
            "- [ ] Unrelated #task ^unrelated\n",
            "  Some prose that stays.\n",
        ),
    );
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] Ref one #ref #task ^ref1\n",
            "  - 🪢 **LINKED FROM:** [[bob#^feat1]] (old)\n",
            "- [ ] Ref two #ref #task ^ref2\n",
            "- [ ] Ref three #ref #task ^ref3\n",
            "- [ ] Ref five #ref #task ^ref5\n",
            "  - 🪢 **LINKS:** old [[sase#^ref1]]\n",
            "- [x] Done ref #ref #task ^ref4\n",
        ),
    );
    // Active-to-archive: two edges from active into done/.
    write_file(
        &vault.join("active2.md"),
        concat!(
            "- [ ] Active four #task ^act4\n",
            "  - 🪢 **LINKS:** old [[done/bob_done#^arch1]]\n",
            "- [ ] Active five #task ^act5\n",
            "  - 🪢 **LINKS:** old [[done/bob_done#^arch2]]\n",
        ),
    );
    // Archive notes: 7 archive-to-archive edges.
    write_file(
        &vault.join("done/bob_done.md"),
        concat!(
            "- [x] Arch one #task ^arch1\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch3]]\n",
            "- [x] Arch two #task ^arch2\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch4]]\n",
            "- [x] Arch three #task ^arch5\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch3]] • old [[done/sase_done#^arch4]]\n",
            "- [x] Arch four #task ^arch6\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch4]]\n",
            "- [x] Arch five #task ^arch7\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch3]]\n",
            "- [x] Arch six #task ^arch8\n",
            "  - 🪢 **LINKS:** old [[done/sase_done#^arch4]]\n",
        ),
    );
    write_file(
        &vault.join("done/sase_done.md"),
        concat!(
            "- [x] Done ref three #task ^arch3\n",
            "- [x] Done ref four #task ^arch4\n",
        ),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

#[test]
fn reverse_dry_run_writes_nothing_and_apply_reverts_as_one_commit() {
    let temp = TempDir::new("bob-cli-relations-reverse-basic");
    let (vault, cache) = seed_basic(temp.path());
    let original = note_snapshot(&vault);

    let (dry, preview) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_success(&dry);
    assert_eq!(preview["reversed"], true);
    assert_eq!(preview["mode"], "dry_run");
    assert_eq!(preview["moves"].as_array().unwrap().len(), 1);
    assert_eq!(preview["moves"][0]["action"], "moved");
    assert_eq!(preview["moves"][0]["source"]["path"], "a.md");
    assert_eq!(preview["moves"][0]["target"]["path"], "b.md");
    assert_eq!(preview["moves"][0]["source"]["line"], 1);
    assert_eq!(preview["from"]["key"], "old");
    assert!(preview["minted_block_ids"].as_array().unwrap().is_empty());
    assert_eq!(note_snapshot(&vault), original);
    assert_eq!(
        stdout(&git_in(&vault, ["log", "--oneline"]))
            .lines()
            .count(),
        1
    );

    let (applied, report) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&applied);
    assert_eq!(report["mode"], "write");
    assert_eq!(report["reversed"], true);
    assert_eq!(
        report["commit"]["subject"],
        "bob relations rename: old → new (reversed)"
    );
    assert!(fs::read_to_string(vault.join("b.md"))
        .unwrap()
        .contains("LINKS:** new [[a#^a]]"));
    // S loses its OLD entry: LINKS line removed when it empties.
    assert!(!fs::read_to_string(vault.join("a.md"))
        .unwrap()
        .contains("LINKS:** old"));

    git_in(&vault, ["revert", "--no-edit", "HEAD"]);
    assert_eq!(note_snapshot(&vault), original);

    // Second run after a non-symmetric apply refuses unknown_label.
    let (again, _) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&again);
    let (second, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(value["code"], "unknown_label");
}

#[test]
fn reverse_migration_fixture_matches_approved_counts() {
    let temp = TempDir::new("bob-cli-reverse-migration");
    let (vault, cache) = seed_migration(temp.path());
    let (dry2, preview2) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_success(&dry2);
    assert_eq!(preview2["counts"]["links"], 14);
    // 7 all-done sources.
    assert_eq!(preview2["counts"]["done_links"], 7);
    assert!(preview2["moves"].as_array().unwrap().len() == 14);
}

#[test]
fn reverse_same_note_and_cross_note_moves() {
    let temp = TempDir::new("bob-cli-reverse-same-note");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("same.md"),
        concat!(
            "- [ ] S #task ^s\n",
            "  - 🪢 **LINKS:** old [[#^t]]\n",
            "- [ ] T #task ^t\n",
        ),
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    let (dry, preview) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_success(&dry);
    assert_eq!(preview["moves"][0]["source"]["path"], "same.md");
    assert_eq!(preview["moves"][0]["target"]["path"], "same.md");
    let (applied, _) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&applied);
    let text = fs::read_to_string(vault.join("same.md")).unwrap();
    assert!(text.contains("LINKS:** new [[#^s]]"));
    assert!(!text.contains("old [["));
}

#[test]
fn reverse_mints_writable_sources_and_refuses_archived_ones() {
    let temp = TempDir::new("bob-cli-reverse-mint");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("a.md"),
        "- [ ] No id source #task\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    write_file(&vault.join("b.md"), "- [ ] Target #task ^b\n");
    write_file(
        &vault.join("done/arch.md"),
        "- [ ] Archived no id #task\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    // Archived mint refusal fires before any writes.
    let (output, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "source_needs_block_id");

    // Remove the archived offender: writable mint proceeds, preview mints
    // nothing to disk.
    fs::remove_file(vault.join("done/arch.md")).unwrap();
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "drop archive"]);
    let before = note_snapshot(&vault);
    let (dry, preview) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_success(&dry);
    assert_eq!(preview["minted_block_ids"].as_array().unwrap().len(), 1);
    assert_eq!(note_snapshot(&vault), before);
    let (applied, report) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&applied);
    assert_eq!(report["minted_block_ids"].as_array().unwrap().len(), 1);
    let minted = report["minted_block_ids"][0]["block_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fs::read_to_string(vault.join("a.md"))
        .unwrap()
        .contains(&format!("^{minted}")));
}

#[test]
fn reverse_merge_and_conflict_and_same_key() {
    let temp = TempDir::new("bob-cli-reverse-merge");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("a.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    write_file(
        &vault.join("b.md"),
        "- [ ] B #task ^b\n  - 🪢 **LINKS:** new [[a#^a]]\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);

    // Existing NEW key requires --merge.
    let (output, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "label_exists");
    let (dry, preview) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d", "-m"]);
    assert_success(&dry);
    assert_eq!(preview["moves"][0]["action"], "merged");

    // Different-label conflict refuses.
    write_file(
        &vault.join("b.md"),
        "- [ ] B #task ^b\n  - 🪢 **LINKS:** other [[a#^a]]\n",
    );
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "conflict"]);
    let (conflict, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d", "-m"]);
    assert_eq!(conflict.status.code(), Some(1));
    assert_eq!(value["code"], "reverse_conflict");

    // Same-key reversal needs --merge and reports moved.
    let temp2 = TempDir::new("bob-cli-reverse-same-key");
    let vault2 = temp2.path().join("vault");
    let cache2 = temp2.path().join("cache");
    fs::create_dir_all(&cache2).unwrap();
    write_file(
        &vault2.join("a.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    write_file(&vault2.join("b.md"), "- [ ] B #task ^b\n");
    git_in(&vault2, ["init", "-q"]);
    git_in(&vault2, ["config", "user.email", "t@t"]);
    git_in(&vault2, ["config", "user.name", "t"]);
    git_in(&vault2, ["add", "-A"]);
    git_in(&vault2, ["commit", "-qm", "init"]);
    let (needs_merge, value) =
        reverse_json(&vault2, &cache2, &["old", "old", "--reverse", "-d"]);
    assert_eq!(needs_merge.status.code(), Some(1));
    assert_eq!(value["code"], "label_exists");
    let (dry, preview) = reverse_json(
        &vault2,
        &cache2,
        &["old", "old", "--reverse", "-d", "-m"],
    );
    assert_success(&dry);
    assert_eq!(preview["reversed"], true);
}

#[test]
fn reverse_refuses_unresolved_ambiguous_and_self_targets() {
    let temp = TempDir::new("bob-cli-reverse-unresolved");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("a.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[missing#^zzz]]\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    let before = note_snapshot(&vault);
    let (output, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "unresolved_target");
    assert_eq!(note_snapshot(&vault), before);

    // Self-link refuses as unresolved.
    write_file(
        &vault.join("a.md"),
        "- [ ] A #task ^a\n  - 🪢 **LINKS:** old [[a#^a]]\n",
    );
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "self"]);
    let (output, value) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "unresolved_target");
}

#[test]
fn reverse_never_closes_and_reports_stay_open() {
    let temp = TempDir::new("bob-cli-reverse-closing");
    let (vault, cache) = seed_basic(temp.path());
    let config = cache.join("closing.yml");
    write_file(&config, "relations:\n  complete_labels:\n    - new\n");
    let run = |args: &[&str]| {
        reverse_command(&vault, &cache)
            .env("BOB_CONFIG_FILE", &config)
            .args(["relations", "rename"])
            .args(args)
            .args(["-f", "json"])
            .output()
            .expect("reverse with closing config")
    };
    let output = run(&["old", "new", "--reverse", "-d"]);
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("JSON");
    assert!(value["warnings"].as_array().unwrap().iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("complete label in config.yml")
    }));
    let before = note_snapshot(&vault);
    let output = run(&["old", "new", "--reverse", "-o"]);
    assert_success(&output);
    // No status bytes change: no completion stamps, no checkbox flips.
    for (relative, bytes) in note_snapshot(&vault) {
        let text = String::from_utf8(bytes).unwrap();
        if before.contains_key(&relative) {
            for line in text.lines() {
                assert!(!line.contains("[completion::"));
            }
        }
    }
    assert!(fs::read_to_string(vault.join("a.md"))
        .unwrap()
        .contains("- [ ] Task A #task"));
}

#[test]
fn reverse_crlf_and_no_final_newline_preserved() {
    let temp = TempDir::new("bob-cli-reverse-endings");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&vault).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        vault.join("a.md"),
        "- [ ] A #task ^a\r\n  - 🪢 **LINKS:** old [[b#^b]]\r\n",
    )
    .unwrap();
    fs::write(vault.join("b.md"), "- [ ] B #task ^b").unwrap();
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    let (applied, _) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&applied);
    let raw = fs::read(vault.join("a.md")).unwrap();
    assert!(raw.windows(2).any(|pair| pair == b"\r\n"));
    let raw_b = fs::read(vault.join("b.md")).unwrap();
    assert!(!raw_b.ends_with(b"\n"));
}

#[test]
fn reverse_help_orders_flag_and_completion_inherits_it() {
    let temp = TempDir::new("bob-cli-reverse-help");
    let (vault, cache) = seed_basic(temp.path());
    let help = reverse_command(&vault, &cache)
        .args(["relations", "rename", "--help"])
        .output()
        .unwrap();
    assert_success(&help);
    let text = String::from_utf8(help.stdout).unwrap();
    let offline = text.find("-o, --offline").expect("offline flag");
    let reverse = text.find("-r, --reverse").expect("reverse flag");
    assert!(offline < reverse);
    // Completion inherits the flag through the shared build_cli_rename
    // descriptor (used by the completion tree).
    let complete = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "relations",
            "rename",
            "--",
        ])
        .output()
        .unwrap();
    assert_success(&complete);
    let stdout = stdout(&complete);
    assert!(stdout.contains("--reverse"));
}

#[test]
fn reverse_with_mirrors_disabled_writes_no_mirrors_but_still_mints() {
    let temp = TempDir::new("bob-cli-reverse-no-mirrors");
    let (vault, cache) = seed_basic(temp.path());
    let config = cache.join("no-mirrors.yml");
    write_file(&config, "relations:\n  linked_from_lines: false\n");
    let run = |args: &[&str]| {
        reverse_command(&vault, &cache)
            .env("BOB_CONFIG_FILE", &config)
            .args(["relations", "rename"])
            .args(args)
            .args(["-f", "json"])
            .output()
            .expect("reverse with mirrors disabled")
    };
    let output = run(&["old", "new", "--reverse", "-o"]);
    assert_success(&output);
    let text_a = fs::read_to_string(vault.join("a.md")).unwrap();
    assert!(!text_a.contains("LINKED FROM"));
    let text_b = fs::read_to_string(vault.join("b.md")).unwrap();
    assert!(text_b.contains("LINKS:** new [[a#^a]]"));
}

#[test]
fn reverse_previous_daily_moves_with_existing_ids() {
    let temp = TempDir::new("bob-cli-reverse-prev-daily");
    let vault = temp.path().join("vault");
    let cache = temp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    // Previous daily note (before BOB_DAY_FILE's 2026-10-28 anchor).
    write_file(
        &vault.join("2026/20261027.md"),
        "- [ ] Daily task #task ^daily\n  - 🪢 **LINKS:** old [[b#^b]]\n",
    );
    write_file(&vault.join("b.md"), "- [ ] Target #task ^b\n");
    write_file(&vault.join("2026/20261028.md"), "# Daily\n");
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    let (dry, preview) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-d"]);
    assert_success(&dry);
    assert_eq!(preview["moves"][0]["source"]["path"], "2026/20261027.md");
}

#[test]
fn reverse_reconcile_is_noop_after_apply() {
    let temp = TempDir::new("bob-cli-reverse-reconcile");
    let (vault, cache) = seed_basic(temp.path());
    let (applied, _) =
        reverse_json(&vault, &cache, &["old", "new", "--reverse", "-o"]);
    assert_success(&applied);
    // Reconcile must make no additional relation changes: run it on the
    // post-apply vault with a real daily note present.
    let daily = vault.join("2026/20261028.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let output = bob_command()
        .args(["task", "reconcile", "--format", "json"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"))
        .env("XDG_CACHE_HOME", &cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_VAULT_SYNC_LOCK_FILE", cache.join("bob_sync.lock"))
        .output()
        .expect("bob task reconcile");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("reconcile JSON");
    assert!(
        value["linked_from_line_updates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "reconcile made relation changes after reverse: {value}"
    );
}
