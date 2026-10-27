//! Saved sources under `.bob/` are never scanned as vault notes.

use std::fs;
use std::path::Path;

use crate::support::*;

const SAVED_REL: &str = ".bob/ref-sources/chat/x--0123456789ab/source.md";

const SAVED_SOURCE: &str = "\
---
type: [[project]]
status: wip
---

#project

- [x] shipped #task ^done1
- [?] #task keep this open ^open1
[[area#^blk]]
\t- Source: Google Keep · 2026-09-20 08:00 %%gkeep:v1:saved-source:0123456789ab%%
";

fn saved_source_path(vault: &Path) -> std::path::PathBuf {
    vault.join(SAVED_REL)
}

#[test]
fn bob_dir_saved_sources_are_ignored_by_note_walkers() {
    let temp = TempDir::new("bob-cli-bob-dir-walkers");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20261020.md");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("area.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task ^blk\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(
        &vault.join("gkeep_inbox.md"),
        "- [ ] #task Inbox copy [created::2026-09-20]\n\
         \t- Source: Google Keep · 2026-09-20 08:00 %%gkeep:v1:saved-source:0123456789ab%%\n",
    );
    write_file(&saved_source_path(&vault), SAVED_SOURCE);
    let saved_before =
        fs::read(saved_source_path(&vault)).expect("read saved source");

    let archive = bob_command()
        .args(["task", "archive", "--threshold", "1"])
        .env("BOB_DIR", &vault)
        .env("HOME", temp.path().join("home"))
        .output()
        .expect("run bob task archive");
    assert_success(&archive);
    assert_eq!(
        fs::read(saved_source_path(&vault)).expect("read after archive"),
        saved_before,
        "archive must leave the saved source byte-identical:\n{}",
        format_output(&archive)
    );
    assert!(
        !vault
            .join("done/.bob/ref-sources/chat/x--0123456789ab/source_done.md")
            .exists(),
        "archive must not collect the saved source"
    );

    let moved = bob_command()
        .args(["task", "move", "-f", "json", "area", "dest", "-b", "blk"])
        .env("BOB_DIR", &vault)
        .env("HOME", temp.path().join("home"))
        .output()
        .expect("run bob task move");
    assert_success(&moved);
    assert_eq!(
        fs::read(saved_source_path(&vault)).expect("read after move"),
        saved_before,
        "link repair must leave the saved source byte-identical:\n{}",
        format_output(&moved)
    );
    assert!(
        fs::read_to_string(vault.join("dest.md"))
            .expect("read dest")
            .contains("Move this task"),
        "the real task still moves:\n{}",
        format_output(&moved)
    );

    let projects = bob_command()
        .args(["projects", "list", "-b"])
        .arg(&vault)
        .output()
        .expect("run bob projects list");
    assert_success(&projects);
    let projects_out = stdout(&projects);
    assert!(
        !projects_out.contains("ref-sources")
            && !projects_out.contains("0123456789ab")
            && !projects_out.contains("keep this open"),
        "project scan must skip the saved source:\n{}",
        format_output(&projects)
    );

    let ready = bob_command()
        .args(["ready", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-20")
        .output()
        .expect("run bob ready");
    assert_success(&ready);
    let ready_json: serde_json::Value =
        serde_json::from_str(stdout(&ready).trim()).expect("ready JSON");
    let ready_paths: Vec<&str> = ready_json["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .filter_map(|note| note["path"].as_str())
        .collect();
    assert!(
        ready_paths.iter().all(|path| !path.contains(".bob/")),
        "ready scan must skip .bob notes: {ready_paths:?}"
    );

    let reconcile = bob_command()
        .args(["task", "reconcile", "--format", "json", "-r", "0"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-10-20")
        .output()
        .expect("run bob task reconcile");
    assert_success(&reconcile);
    assert_eq!(
        fs::read(saved_source_path(&vault)).expect("read after reconcile"),
        saved_before,
        "task-status hooks must leave the saved source byte-identical:\n{}",
        format_output(&reconcile)
    );

    let dataview = bob_command()
        .args(["query", "-b"])
        .arg(&vault)
        .args(["-S", "-q", "LIST FROM #project"])
        .output()
        .expect("run bob query LIST FROM #project");
    assert_success(&dataview);
    assert_eq!(
        stdout(&dataview),
        "",
        "Dataview must not see the saved source:\n{}",
        format_output(&dataview)
    );

    let tasks = bob_command()
        .args(["query", "-b"])
        .arg(&vault)
        .args(["--tasks", ""])
        .output()
        .expect("run bob query --tasks");
    assert_success(&tasks);
    let tasks_out = stdout(&tasks);
    assert!(
        !tasks_out.contains("shipped") && !tasks_out.contains("keep this open"),
        "Tasks query must skip the saved source:\n{}",
        format_output(&tasks)
    );

    let gkeep = bob_command()
        .args(["gkeep", "list", "-s", "vault", "-f", "json", "-b"])
        .arg(&vault)
        .output()
        .expect("run bob gkeep list");
    assert_success(&gkeep);
    let gkeep_json: serde_json::Value =
        serde_json::from_str(stdout(&gkeep).trim()).expect("gkeep JSON");
    assert_eq!(
        gkeep_json["summary"]["duplicates"],
        0,
        "gkeep ledger must ignore the saved-source marker:\n{}",
        format_output(&gkeep)
    );
    assert_eq!(
        fs::read(saved_source_path(&vault)).expect("read after gkeep list"),
        saved_before,
        "gkeep list must not rewrite the saved source"
    );
}
