//! Vault-aware value kinds: partial-parse context, read-only
//! providers behind the 150 ms deadline, and the read-only
//! enforcement test.
//!
//! Every case runs the built binary against a small fixture vault,
//! so the goldens cover context parsing, the providers, and the
//! presenter together. `BOB_NOW` pins the daily note the Pomodoro
//! provider reads.

use crate::support::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Instant;

const BOB_NOW: &str = "2026-06-01 09:10:01";

struct Fixture {
    _temp: TempDir,
    vault: PathBuf,
    repo: PathBuf,
    config: PathBuf,
}

/// A vault with one area (`cash`), one project (`dev`), open tasks
/// with block IDs, task sections, and a daily note with open
/// Pomodoros; plus a plugins repo checkout and a priority config.
fn fixture() -> Fixture {
    let temp = TempDir::new("bob-cli-complete-vault");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Cash\n",
            "\n",
            "## Shopping\n",
            "\n",
            "## Ideas\n",
            "\n",
            "- [ ] #task Buy oat milk ^buy-milk\n",
            "- [ ] #task Fix the sink ^fix-sink\n",
            "  - FIRST STEPS\n",
            "    - gather parts\n",
            "  - SECOND STEPS\n",
            "- [x] #task Done already ^done-old\n",
        ),
    );
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: active\n",
            "---\n",
            "# Dev\n",
            "\n",
            "- [ ] #task Ship the release ^ship-it\n",
        ),
    );
    write_file(
        &vault.join("2026/20260601.md"),
        concat!(
            "# 2026-06-01\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-0930) Morning pages\n",
            "- [ ] () — DEEP WORK\n",
            "- [x] (0700-0730) Done early\n",
        ),
    );
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("plugins/alpha")).expect("repo alpha");
    fs::create_dir_all(repo.join("plugins/beta")).expect("repo beta");
    write_file(&repo.join("plugins/README.md"), "# plugins\n");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        concat!(
            "properties:\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
            "      - label: P2\n",
            "        value: medium\n",
            "        min_days: 8\n",
            "        max_days: 30\n",
        ),
    );
    Fixture {
        _temp: temp,
        vault,
        repo,
        config,
    }
}

/// Run `bob __complete` over shell words with the fixture vault.
fn complete(fixture: &Fixture, words: &[&str]) -> Output {
    complete_with_env(fixture, words, &[])
}

fn complete_with_env(
    fixture: &Fixture,
    words: &[&str],
    env: &[(&str, &str)],
) -> Output {
    let mut command = bob_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_CONFIG_FILE", &fixture.config);
    for (key, value) in env {
        command.env(key, value);
    }
    for word in words {
        command.arg(word);
    }
    command.output().expect("run bob __complete")
}

fn rows(output: &Output) -> Vec<Vec<String>> {
    stdout(output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

fn directives(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| line.starts_with('!'))
        .map(str::to_string)
        .collect()
}

fn values(output: &Output) -> Vec<String> {
    rows(output).iter().map(|row| row[0].clone()).collect()
}

#[test]
fn routes_are_grouped_in_scan_order() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "--route", ""]);

    assert_success(&output);
    assert!(stderr(&output).is_empty());
    assert_eq!(
        stdout(&output),
        concat!(
            "mac_inbox\tinbox · default capture target\tinbox\tspace\n",
            "cash\tarea\tareas\tspace\n",
            "dev\tproject · active\tprojects\tspace\n",
        )
    );
}

#[test]
fn sections_follow_long_short_and_attached_route_forms() {
    let fixture = fixture();
    let expected = concat!(
        "Cash\tH1\tsections in cash\tspace\n",
        "Shopping\tH2\tsections in cash\tspace\n",
        "Ideas\tH2\tsections in cash\tspace\n",
    );
    for words in [
        vec!["bob", "capture", "--route", "cash", "--section", ""],
        vec!["bob", "capture", "-rcash", "--section", ""],
        vec!["bob", "capture", "--route=cash", "--section", ""],
    ] {
        let output = complete(&fixture, &words);
        assert_success(&output);
        assert!(stderr(&output).is_empty(), "words: {words:?}");
        assert_eq!(stdout(&output), expected, "words: {words:?}");
    }
}

#[test]
fn tasks_offer_open_block_ids_with_text() {
    let fixture = fixture();
    for words in [
        vec!["bob", "capture", "--route", "cash", "--task", ""],
        vec!["bob", "capture", "--route=cash", "--task", ""],
        vec![
            "bob",
            "capture-task-sections",
            "--route",
            "cash",
            "--block-id",
            "",
        ],
    ] {
        let output = complete(&fixture, &words);
        assert_success(&output);
        assert!(stderr(&output).is_empty(), "words: {words:?}");
        let lines = rows(&output);
        assert_eq!(lines.len(), 2, "words: {words:?}: {lines:?}");
        assert_eq!(lines[0][0], "buy-milk");
        assert_eq!(lines[1][0], "fix-sink");
        for line in &lines {
            assert_eq!(line[2], "tasks in cash", "words: {words:?}");
            assert!(!line[1].is_empty(), "description: {line:?}");
        }
        assert!(
            lines[0][1].contains("oat milk"),
            "unexpected description: {:?}",
            lines[0][1]
        );
    }
}

#[test]
fn task_sections_offer_exact_titles() {
    let fixture = fixture();
    let output = complete(
        &fixture,
        &[
            "bob",
            "capture",
            "--route",
            "cash",
            "--task",
            "fix-sink",
            "--task-section",
            "",
        ],
    );

    assert_success(&output);
    assert!(stderr(&output).is_empty());
    assert_eq!(
        stdout(&output),
        concat!(
            "FIRST STEPS\t1 item\ttask sections\tspace\n",
            "SECOND STEPS\t\ttask sections\tspace\n",
        )
    );
}

#[test]
fn pomodoro_refs_offer_open_entries() {
    let fixture = fixture();
    let output = complete(
        &fixture,
        &["bob", "capture-pomodoro-name", "--pomodoro-ref", ""],
    );

    assert_success(&output);
    assert!(stderr(&output).is_empty());
    let first = capture_pomodoro_ref("- [ ] (0900-0930) Morning pages", 5);
    let second = capture_pomodoro_ref("- [ ] () — DEEP WORK", 6);
    assert_eq!(
        stdout(&output),
        format!(
            "{first}\t0900-0930 · open\topen Pomodoros\tspace\n\
             {second}\tDEEP WORK\topen Pomodoros\tspace\n",
        )
    );
}

#[test]
fn plugins_come_from_the_repo_checkout() {
    let fixture = fixture();
    let repo = path_str(&fixture.repo).to_string();
    let output = complete(
        &fixture,
        &[
            "bob",
            "plugins",
            "sync",
            "--repo",
            repo.as_str(),
            "--plugin",
            "",
        ],
    );

    assert_success(&output);
    assert!(stderr(&output).is_empty());
    assert_eq!(
        stdout(&output),
        "alpha\t\tplugins\tspace\nbeta\t\tplugins\tspace\n",
    );
}

#[test]
fn levels_come_from_config_in_order() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "task", "reroll", "--level", ""]);

    assert_success(&output);
    assert!(stderr(&output).is_empty());
    assert_eq!(
        stdout(&output),
        "P1\t2–7 days\tlabel\tspace\nP2\t8–30 days\tlabel\tspace\n",
    );
}

#[test]
fn vault_notes_answer_files_in() {
    let fixture = fixture();
    for words in [
        vec!["bob", "query", "--tasks-note", ""],
        vec!["bob", "query", "--origin", ""],
    ] {
        let output = complete(&fixture, &words);
        assert_success(&output);
        assert!(stderr(&output).is_empty(), "words: {words:?}");
        assert_eq!(
            directives(&output),
            vec![format!("!files-in {}\t*.md", fixture.vault.display())]
        );
        assert!(values(&output).is_empty());
    }
}

#[test]
fn missing_prerequisites_name_the_flag() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "--section", ""]);
    assert_success(&output);
    assert_eq!(stdout(&output), "!message pass --route first\n");

    let output = complete(
        &fixture,
        &["bob", "capture", "--route", "cash", "--task-section", ""],
    );
    assert_success(&output);
    assert_eq!(stdout(&output), "!message pass --task first\n");
}

#[test]
fn unknown_route_offers_nothing() {
    let fixture = fixture();
    let output = complete(
        &fixture,
        &["bob", "capture", "--route", "nope", "--section", ""],
    );
    assert_success(&output);
    assert!(stdout(&output).is_empty());
}

#[test]
fn deadline_override_keeps_working() {
    let fixture = fixture();
    for deadline in ["5000", "abc", "0"] {
        let output = complete_with_env(
            &fixture,
            &["bob", "capture", "--route", ""],
            &[("BOB_COMPLETE_DEADLINE_MS", deadline)],
        );
        assert_success(&output);
        assert!(
            values(&output).contains(&"cash".to_string()),
            "deadline {deadline}: {}",
            stdout(&output)
        );
    }
}

#[test]
fn vault_slots_stay_fast() {
    let fixture = fixture();
    let mut elapsed_ms = Vec::new();
    for _ in 0..11 {
        let started = Instant::now();
        let output = complete(
            &fixture,
            &["bob", "capture", "--route", "cash", "--task", ""],
        );
        assert_success(&output);
        elapsed_ms.push(started.elapsed().as_millis());
    }
    elapsed_ms.sort_unstable();
    let p50 = elapsed_ms[elapsed_ms.len() / 2];
    // Warn-only: vault slots should answer p50 ≤ 30 ms on the
    // fixture. This never fails; it records the number.
    if p50 > 30 {
        eprintln!("warning: vault completion p50 is {p50}ms (> 30ms)");
    }
}

/// Completion never writes: run every vault slot against a
/// write-locked vault with a recording fake `git` on PATH and empty
/// XDG directories, then prove the tree, `git`, and XDG are
/// untouched.
#[test]
fn completion_is_read_only() {
    let fixture = fixture();
    let snapshot = tree_hashes(&fixture.vault);

    // Write-lock the vault (best effort outside unix).
    lock_tree(&fixture.vault);

    let probe = TempDir::new("bob-cli-complete-readonly");
    let git_log = probe.path().join("git-calls.log");
    let fake_bin = probe.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("fake bin");
    write_executable(
        &fake_bin.join("git"),
        &format!(
            "#!/bin/sh\necho called >> {}\nexit 1\n",
            shell_single_quote(path_str(&git_log))
        ),
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let xdg_data = probe.path().join("data");
    let xdg_cache = probe.path().join("cache");
    let xdg_config = probe.path().join("config");
    let home = probe.path().join("home");
    for directory in [&xdg_data, &xdg_cache, &xdg_config, &home] {
        fs::create_dir_all(directory).expect("xdg dir");
    }

    let repo = path_str(&fixture.repo).to_string();
    let slots: Vec<Vec<String>> = vec![
        vec!["bob", "capture", "--route", ""],
        vec!["bob", "capture", "--route", "cash", "--section", ""],
        vec!["bob", "capture", "--route", "cash", "--task", ""],
        vec![
            "bob",
            "capture",
            "--route",
            "cash",
            "--task",
            "fix-sink",
            "--task-section",
            "",
        ],
        vec![
            "bob",
            "capture-task-sections",
            "--route",
            "cash",
            "--block-id",
            "",
        ],
        vec!["bob", "capture-pomodoro-name", "--pomodoro-ref", ""],
        vec![
            "bob",
            "plugins",
            "sync",
            "--repo",
            repo.as_str(),
            "--plugin",
            "",
        ],
        vec!["bob", "task", "reroll", "--level", ""],
        vec!["bob", "query", "--tasks-note", ""],
        vec!["bob", "query", "--origin", ""],
        vec!["bob", "capture", "--route=cash", "--task", ""],
        vec!["bob", "capture", "fix", ""],
        vec!["bob", "capture", "fix", "it", "@"],
        vec!["bob", "capture", "fix", "it", "@dev:"],
        vec!["bob", "capture", "fix", "it", "@cash#"],
        vec!["bob", "capture", "@cash+fix-sink#"],
        vec!["bob", "capture", "^"],
        vec!["bob", "capture", "=#"],
        vec!["bob", "capture-parse", "fix", "it", "@dev:"],
        vec!["bob", "capture-rewrite", "fix", "it", "@dev:"],
    ]
    .into_iter()
    .map(|words| words.iter().map(|word| word.to_string()).collect())
    .collect();

    for words in &slots {
        let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let mut command = bob_command();
        command
            .arg("__complete")
            .arg("zsh")
            .arg("--protocol")
            .arg("1")
            .arg("--")
            .env("BOB_DIR", &fixture.vault)
            .env("BOB_NOW", BOB_NOW)
            .env("BOB_CONFIG_FILE", &fixture.config)
            .env("PATH", &path)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &xdg_data)
            .env("XDG_CACHE_HOME", &xdg_cache)
            .env("XDG_CONFIG_HOME", &xdg_config);
        for word in &word_refs {
            command.arg(word);
        }
        let output = command.output().expect("run bob __complete");
        assert_success(&output);
        assert!(
            stderr(&output).is_empty(),
            "words {words:?}: {}",
            stderr(&output)
        );
    }

    assert_eq!(
        tree_hashes(&fixture.vault),
        snapshot,
        "completion modified the vault"
    );
    assert!(!git_log.exists(), "completion spawned git");
    for directory in [&xdg_data, &xdg_cache, &xdg_config, &home] {
        assert!(
            is_empty_dir(directory),
            "completion wrote to {}",
            directory.display()
        );
    }
}

fn tree_hashes(root: &Path) -> HashMap<PathBuf, String> {
    let mut hashes = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory).expect("read vault directory");
        for entry in entries {
            let entry = entry.expect("vault entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("vault prefix")
                    .to_path_buf();
                hashes.insert(relative, sha256_file(&path));
            }
        }
    }
    hashes
}

#[cfg(unix)]
fn lock_tree(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = fs::read_dir(&path).expect("read lock directory");
        for entry in entries {
            let entry = entry.expect("lock entry");
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else {
                set_mode(&entry_path, 0o444);
            }
        }
    }
    set_mode(root, 0o555);
}

#[cfg(not(unix))]
fn lock_tree(_root: &Path) {}

fn is_empty_dir(directory: &Path) -> bool {
    let mut stack = vec![directory.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else {
                return false;
            }
        }
    }
    true
}
