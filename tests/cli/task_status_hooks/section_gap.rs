//! The gap after a grouped Tasks section across archive cycles.

use crate::support::*;
use std::fs;
use std::path::Path;

struct Vault {
    root: std::path::PathBuf,
    daily: std::path::PathBuf,
    state: std::path::PathBuf,
    lock: std::path::PathBuf,
    cache: std::path::PathBuf,
}

impl Vault {
    fn reconcile(&self) {
        let output = bob_command()
            .args(["task", "reconcile"])
            .arg("--bob-dir")
            .arg(&self.root)
            .env("BOB_DAY_FILE", &self.daily)
            .env("BOB_VAULT_SYNC_LOCK_FILE", &self.lock)
            .env("XDG_STATE_HOME", &self.state)
            .output()
            .expect("run bob task reconcile");
        assert_success(&output);
    }

    fn archive(&self) {
        let output = bob_command()
            .args(["task", "archive", "--threshold=1"])
            .env("BOB_DIR", &self.root)
            .env("XDG_CACHE_HOME", &self.cache)
            .output()
            .expect("run bob task archive");
        assert_success(&output);
    }
}

/// Counts the blank lines directly above the `heading` line.
fn blank_lines_before(contents: &str, heading: &str) -> usize {
    let lines: Vec<&str> = contents.lines().collect();
    let index = lines
        .iter()
        .position(|line| *line == heading)
        .unwrap_or_else(|| panic!("missing {heading:?} in:\n{contents}"));
    lines[..index]
        .iter()
        .rev()
        .take_while(|line| line.trim().is_empty())
        .count()
}

/// Appends a closed task to the Tasks intake, which sits above the first
/// generated group once the section is decorated.
fn close_a_new_task(note: &Path, cycle: usize) {
    let contents = fs::read_to_string(note).expect("read project note");
    let anchor = if contents.contains("### Next & In Progress\n") {
        "### Next & In Progress\n"
    } else {
        "- [*] #task Keep going ^next\n"
    };
    let closed = format!("- [x] #task Finished round {cycle} ^done{cycle}\n\n");
    write_file(
        note,
        &contents.replacen(anchor, &format!("{closed}{anchor}"), 1),
    );
}

#[test]
fn reconcile_keeps_one_blank_line_after_tasks_across_archive_cycles() {
    let temp = TempDir::new("bob-cli-task-status-hooks-section-gap");
    let vault = Vault {
        root: temp.path().join("vault"),
        daily: temp.path().join("vault/2026/20260710.md"),
        state: temp.path().join("state"),
        lock: temp.path().join("task-status-hooks.lock"),
        cache: temp.path().join("cache"),
    };
    let note = vault.root.join("alpha.md");

    write_blocked_tasks_settings(&vault.root);
    write_file(
        &vault.daily,
        concat!("# Today\n\n", "## Pomodoros\n\n", "- [ ] Current\n"),
    );
    write_file(
        &note,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Alpha\n\n",
            "## Tasks\n\n",
            "- [*] #task Keep going ^next\n",
            "- [x] #task Finished round zero ^done\n",
            "\n",
            "## Future Work\n\n",
            "Later ideas.\n",
        ),
    );

    for cycle in 1..=3 {
        close_a_new_task(&note, cycle);

        vault.reconcile();
        let closed = fs::read_to_string(&note).expect("read after close");
        assert!(closed.contains("### Done & Canceled"), "{closed}");
        assert_eq!(
            blank_lines_before(&closed, "## Future Work"),
            1,
            "cycle {cycle}: reconcile after the close grew the gap:\n{closed}"
        );

        vault.archive();
        let archived = fs::read_to_string(&note).expect("read after archive");
        assert!(!archived.contains("- [x] #task"), "{archived}");

        vault.reconcile();
        let healed = fs::read_to_string(&note).expect("read after reconcile");
        assert!(!healed.contains("### Done & Canceled"), "{healed}");
        assert_eq!(
            blank_lines_before(&healed, "## Future Work"),
            1,
            "cycle {cycle}: reconcile after the archive grew the gap:\n{healed}"
        );
        assert!(healed.contains("- [*] #task Keep going ^next"), "{healed}");
        assert!(healed.ends_with("## Future Work\n\nLater ideas.\n"));
    }
}
