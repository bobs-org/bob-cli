//! Archive health findings, binding-conflict write skips, and report carry.
use super::*;

use serde_json::Value;

use super::super::command::{execute_standalone, Hooks};
use super::super::health::{
    CODE_BINDING_CONFLICT, CODE_DUPLICATE, CODE_DUPLICATE_ID,
    CODE_OPEN_IN_ARCHIVE, CODE_ORPHAN, CODE_UNDATED,
};
use super::super::report::{render_human, render_json, Outcome, Report};

fn report_for(vault: &TempDir, dry_run: bool) -> Report {
    Report::new(vault.path().to_path_buf(), 1, dry_run)
}

fn lock_env(dir: &Path) -> bob_env::TestEnvGuard {
    bob_env::TestEnvGuard::set(&[(
        "BOB_VAULT_SYNC_LOCK_FILE",
        Some(dir.join("bob_sync.lock").as_os_str()),
    )])
}

fn run_preview(vault: &TempDir) -> Report {
    let scratch = TempDir::new("bob-cli-archive-health-lock");
    let _env = lock_env(scratch.path());
    let mut report = report_for(vault, true);
    execute_standalone(&mut report, &mut Hooks::production());
    report
}

fn json(report: &Report) -> Value {
    serde_json::from_str(&render_json(report)).expect("report is JSON")
}

fn findings<'a>(value: &'a Value, code: &str) -> Vec<&'a Value> {
    value["diagnostics"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|finding| finding["code"] == code)
        .collect()
}

fn archive_note(parent: &str, body: &str) -> String {
    format!("---\nparent: \"[[{parent}]]\"\ntype: \"[[done]]\"\n---\n\n{body}")
}

fn settings_with_custom_open() -> &'static str {
    r##"{
      "globalFilter": "#task",
      "statusSettings": {
        "coreStatuses": [
          {"symbol":" ","name":"Todo","type":"TODO"},
          {"symbol":"x","name":"Done","type":"DONE"},
          {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
          {"symbol":"*","name":"Next","type":"ON_HOLD"},
          {"symbol":"-","name":"Canceled","type":"CANCELLED"}
        ],
        "customStatuses": [
          {"symbol":"Q","name":"Queued","type":"TODO"}
        ]
      }
    }"##
}

#[test]
fn bound_noncanonical_archive_moves_to_canonical_destination() {
    let vault = TempDir::new("bob-cli-archive-health-noncanonical");
    write_file(
        &vault.path().join("projects/alpha.md"),
        "\
---
type: [[project]]
done_tasks: \"[[done/projects/alpha_old_done]]\"
---

- [x] one #task [completion:: 2026-01-01]
- [x] two #task [completion:: 2026-01-02]
",
    );
    write_file(
        &vault.path().join("done/projects/alpha_old_done.md"),
        &archive_note(
            "projects/alpha",
            "- [x] older #task [completion:: 2025-12-01]\n",
        ),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.files.len(), 1);
    // Rename-following installs the canonical destination and removes the
    // old path; the noncanonical archive is never the final destination.
    assert_eq!(
        plan.files[0].relative_archive_path,
        PathBuf::from("done/projects/alpha_done.md")
    );
    assert_eq!(
        plan.files[0].renamed_from,
        Some(PathBuf::from("done/projects/alpha_old_done.md"))
    );
    assert!(plan.files[0].archive_created);
    assert!(plan.files[0]
        .archive_contents
        .as_deref()
        .unwrap()
        .contains("one #task"));
    assert!(plan.files[0]
        .archive_contents
        .as_deref()
        .unwrap()
        .contains("older #task"));
    assert_eq!(plan.renames.len(), 1);
    assert_eq!(
        plan.renames[0].from,
        PathBuf::from("done/projects/alpha_old_done.md")
    );
    assert_eq!(
        plan.renames[0].to,
        PathBuf::from("done/projects/alpha_done.md")
    );
    assert!(!vault.path().join("done/projects/alpha_done.md").exists());
}

#[test]
fn grandfathered_untyped_source_renames_bound_noncanonical_archive() {
    let vault = TempDir::new("bob-cli-archive-health-grandfather-noncanonical");
    write_file(
        &vault.path().join("legacy.md"),
        "\
---
done_tasks: \"[[done/legacy_day_done]]\"
---

- [x] below #task [completion:: 2026-01-01]
- [ ] open #task
",
    );
    write_file(
        &vault.path().join("done/legacy_day_done.md"),
        &archive_note("legacy", "- [x] old #task [completion:: 2025-01-01]\n"),
    );

    let plan = build_collection_plan(vault.path(), 1).expect("plan");
    assert_eq!(plan.grandfathered, vec![PathBuf::from("legacy.md")]);
    assert_eq!(plan.files.len(), 1);
    assert_eq!(
        plan.files[0].relative_archive_path,
        PathBuf::from("done/legacy_done.md")
    );
    assert_eq!(
        plan.files[0].renamed_from,
        Some(PathBuf::from("done/legacy_day_done.md"))
    );
    assert_eq!(plan.files[0].task_count, 1);
    assert_eq!(plan.renames.len(), 1);
}

#[test]
fn reports_each_health_finding_and_masks_code_comments() {
    let vault = TempDir::new("bob-cli-archive-health-findings");
    write_file(
        &vault.path().join("dated.md"),
        &area_note(
            "\
- [x] missing date #task
- [x] invalid date #task [completion:: not-a-date]
- [x] emoji date #task ✅ 2026-02-03
- [x] cancelled date #task [cancelled:: 2026-02-04]
",
        ),
    );
    write_file(
        &vault.path().join("done/orphan_done.md"),
        "\
---
type: \"[[done]]\"
---

- [x] leftover #task ^once
",
    );
    write_file(
        &vault.path().join("done/clean_done.md"),
        &archive_note(
            "clean",
            "\
- [x] archived #task ^keep
```
- [ ] fenced open #task
```
%%
- [ ] commented open #task
%%
<!--
- [ ] html open #task
-->
",
        ),
    );
    write_file(
        &vault.path().join("clean.md"),
        "\
---
type: [[area]]
done_tasks: \"[[done/clean_done]]\"
---

- [x] dated #task [completion:: 2026-01-01]
- [ ] still open #task
",
    );
    write_file(
        &vault.path().join("openish.md"),
        &area_note("- [ ] live #task\n"),
    );
    write_file(
        &vault.path().join("done/openish_done.md"),
        &archive_note("openish", "- [ ] leaked #task ^open\n- [x] dup #task ^dup\n- [x] also #task ^dup\n"),
    );

    let report = run_preview(&vault);
    let value = json(&report);
    assert_eq!(value["diagnostics"]["complete"], true);
    assert_eq!(value["schema_version"], 1);

    let orphans = findings(&value, CODE_ORPHAN);
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0]["path"], "done/orphan_done.md");

    let open = findings(&value, CODE_OPEN_IN_ARCHIVE);
    assert_eq!(open.len(), 1, "{value}");
    assert_eq!(open[0]["path"], "done/openish_done.md");
    assert_eq!(open[0]["line"], 6);
    assert_eq!(open[0]["block_id"], "open");

    let dups = findings(&value, CODE_DUPLICATE_ID);
    assert_eq!(dups.len(), 1);
    assert_eq!(dups[0]["path"], "done/openish_done.md");
    assert_eq!(dups[0]["block_id"], "dup");

    let undated = findings(&value, CODE_UNDATED);
    assert_eq!(undated.len(), 2, "{value}");
    let undated_lines: Vec<u64> = undated
        .iter()
        .map(|finding| finding["line"].as_u64().unwrap())
        .collect();
    assert!(undated_lines.contains(&5), "{undated:?}");
    assert!(undated_lines.contains(&6), "{undated:?}");

    let text = render_human(&report);
    assert!(text.contains("diagnostics:"), "{text}");
    assert!(text.contains("archive_orphan:"), "{text}");
    assert!(!text.contains("fenced open"), "{text}");
}

#[test]
fn custom_status_settings_control_open_in_archive() {
    let vault = TempDir::new("bob-cli-archive-health-custom-status");
    write_file(
        &vault
            .path()
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        settings_with_custom_open(),
    );
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [ ] live #task\n"),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &archive_note("work", "- [Q] queued #task ^q\n"),
    );

    let report = run_preview(&vault);
    let value = json(&report);
    let open = findings(&value, CODE_OPEN_IN_ARCHIVE);
    assert_eq!(open.len(), 1, "{value}");
    assert_eq!(open[0]["block_id"], "q");
}

#[test]
fn binding_conflict_skips_affected_pair_and_keeps_independent_work() {
    let vault = TempDir::new("bob-cli-archive-health-conflict-independent");
    write_file(
        &vault.path().join("safe.md"),
        &area_note(
            "\
- [x] safe one #task [completion:: 2026-01-01]
- [x] safe two #task [completion:: 2026-01-02]
",
        ),
    );
    write_file(
        &vault.path().join("conflict.md"),
        "\
---
type: [[area]]
done_tasks: \"[[done/other_done]]\"
---

- [x] conflict one #task [completion:: 2026-01-01]
- [x] conflict two #task [completion:: 2026-01-02]
",
    );
    write_file(
        &vault.path().join("done/conflict_done.md"),
        &archive_note(
            "conflict",
            "- [x] old #task [completion:: 2025-01-01]\n",
        ),
    );
    write_file(
        &vault.path().join("other.md"),
        &area_note("- [ ] other live #task\n"),
    );
    write_file(
        &vault.path().join("done/other_done.md"),
        &archive_note(
            "other",
            "- [x] other archived #task [completion:: 2025-01-01]\n",
        ),
    );

    let report = run_preview(&vault);
    let value = json(&report);
    assert_eq!(report.exit.code, 1);
    assert_eq!(report.exit.reason, Some("archive_binding_conflict"));
    assert_eq!(report.exit.outcome, Outcome::Preview);
    let conflicts = findings(&value, CODE_BINDING_CONFLICT);
    assert!(!conflicts.is_empty(), "{value}");
    let moves = value["moves"].as_array().expect("moves");
    assert!(
        moves.iter().any(|entry| entry["source"] == "safe.md"),
        "{moves:?}"
    );
    assert!(
        moves.iter().all(|entry| entry["source"] != "conflict.md"),
        "{moves:?}"
    );
}

#[test]
fn duplicate_archives_list_claimants_and_block_that_source() {
    let vault = TempDir::new("bob-cli-archive-health-duplicate");
    write_file(
        &vault.path().join("shared.md"),
        "\
---
type: [[area]]
done_tasks: \"[[done/shared_done]]\"
---

- [x] one #task [completion:: 2026-01-01]
- [x] two #task [completion:: 2026-01-02]
",
    );
    write_file(
        &vault.path().join("done/shared_done.md"),
        &archive_note(
            "shared",
            "- [x] first #task [completion:: 2025-01-01]\n",
        ),
    );
    write_file(
        &vault.path().join("done/shared_also_done.md"),
        "\
---
parent: \"[[shared]]\"
type: \"[[done]]\"
---

- [x] second #task [completion:: 2025-01-02]
",
    );
    write_file(
        &vault.path().join("safe.md"),
        &area_note(
            "\
- [x] safe one #task [completion:: 2026-01-01]
- [x] safe two #task [completion:: 2026-01-02]
",
        ),
    );

    let report = run_preview(&vault);
    let value = json(&report);
    let dups = findings(&value, CODE_DUPLICATE);
    assert_eq!(dups.len(), 1, "{value}");
    assert_eq!(dups[0]["path"], "shared.md");
    let related = dups[0]["related"].as_array().expect("related");
    assert!(
        related.iter().any(|path| path == "done/shared_done.md"),
        "{related:?}"
    );
    assert!(
        related
            .iter()
            .any(|path| path == "done/shared_also_done.md"),
        "{related:?}"
    );
    let moves = value["moves"].as_array().expect("moves");
    assert!(
        moves.iter().all(|entry| entry["source"] != "shared.md"),
        "{moves:?}"
    );
    assert!(
        moves.iter().any(|entry| entry["source"] == "safe.md"),
        "{moves:?}"
    );
    assert_eq!(report.exit.code, 1);
}

#[test]
fn refused_plan_retains_diagnostics() {
    let vault = TempDir::new("bob-cli-archive-health-refused");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] one #task [completion:: 2026-01-01]
- [x] two #task [completion:: 2026-01-02]
",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &archive_note("work", "- [ ] open archived #task ^open\n"),
    );

    let scratch = TempDir::new("bob-cli-archive-health-refused-lock");
    let _env = lock_env(scratch.path());
    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut Hooks::production());
    assert_eq!(report.exit.reason, Some("plan_refused"));
    let value = json(&report);
    assert_eq!(value["diagnostics"]["complete"], true);
    let open = findings(&value, CODE_OPEN_IN_ARCHIVE);
    assert_eq!(open.len(), 1, "{value}");
    assert_eq!(value["moves"], serde_json::json!([]));
}

#[test]
fn noop_and_dry_run_include_diagnostics_without_writes() {
    let vault = TempDir::new("bob-cli-archive-health-noop");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [ ] live #task\n"),
    );
    write_file(
        &vault.path().join("done/stray_done.md"),
        "\
---
type: \"[[done]]\"
---

- [x] stray #task
",
    );
    let before = fs::read(vault.path().join("work.md")).unwrap();
    let before_archive =
        fs::read(vault.path().join("done/stray_done.md")).unwrap();

    let preview = run_preview(&vault);
    assert_eq!(preview.exit.outcome, Outcome::Preview);
    assert_eq!(preview.exit.code, 0);
    let value = json(&preview);
    assert_eq!(findings(&value, CODE_ORPHAN).len(), 1);
    assert_eq!(fs::read(vault.path().join("work.md")).unwrap(), before);
    assert_eq!(
        fs::read(vault.path().join("done/stray_done.md")).unwrap(),
        before_archive
    );

    let scratch = TempDir::new("bob-cli-archive-health-noop-lock");
    let _env = lock_env(scratch.path());
    let mut noop = Report::new(vault.path().to_path_buf(), 10, false);
    execute_standalone(&mut noop, &mut Hooks::production());
    assert_eq!(noop.exit.outcome, Outcome::Noop);
    assert_eq!(noop.exit.code, 0);
    assert_eq!(findings(&json(&noop), CODE_ORPHAN).len(), 1);
}

#[test]
fn undated_rows_count_below_threshold() {
    let vault = TempDir::new("bob-cli-archive-health-undated-threshold");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [x] lonely #task\n- [ ] live #task\n"),
    );
    let plan = build_collection_plan(vault.path(), 10).expect("plan");
    assert!(plan.files.is_empty());

    let scratch = TempDir::new("bob-cli-archive-health-undated-lock");
    let _env = lock_env(scratch.path());
    let mut report = Report::new(vault.path().to_path_buf(), 10, true);
    execute_standalone(&mut report, &mut Hooks::production());
    let value = json(&report);
    let undated = findings(&value, CODE_UNDATED);
    assert_eq!(undated.len(), 1, "{value:?}");
    assert_eq!(undated[0]["path"], "work.md");
}

#[test]
fn capture_failure_reports_incomplete_diagnostics() {
    let report = Report::new(PathBuf::from("/tmp/vault"), 10, true);
    let value = json(&report);
    assert_eq!(value["diagnostics"]["complete"], false);
    assert_eq!(value["diagnostics"]["findings"], serde_json::json!([]));
    for code in [
        CODE_ORPHAN,
        CODE_DUPLICATE,
        CODE_OPEN_IN_ARCHIVE,
        CODE_DUPLICATE_ID,
        CODE_UNDATED,
        CODE_BINDING_CONFLICT,
    ] {
        assert_eq!(value["diagnostics"]["counts"][code], 0, "{value}");
    }
    let text = render_human(&report);
    assert!(text.contains("incomplete census"), "{text}");
}

#[test]
fn duplicate_holds_alongside_binding_conflict_keep_independent_work() {
    let vault = TempDir::new("bob-cli-archive-health-holds-and-conflict");
    write_file(
        &vault.path().join("safe.md"),
        &area_note(
            "\
- [x] safe one #task [completion:: 2026-01-01]
- [x] safe two #task [completion:: 2026-01-02]
",
        ),
    );
    write_file(
        &vault.path().join("holds.md"),
        &area_note(
            "\
- [x] first #task [completion:: 2026-01-01] ^dup
- [x] second #task [completion:: 2026-01-02] ^dup
",
        ),
    );
    write_file(
        &vault.path().join("conflict.md"),
        "\
---
type: [[area]]
done_tasks: \"[[done/other_done]]\"
---

- [x] conflict one #task [completion:: 2026-01-01]
- [x] conflict two #task [completion:: 2026-01-02]
",
    );
    write_file(
        &vault.path().join("done/conflict_done.md"),
        &archive_note(
            "conflict",
            "- [x] old #task [completion:: 2025-01-01]\n",
        ),
    );
    write_file(
        &vault.path().join("other.md"),
        &area_note("- [ ] other live #task\n"),
    );
    write_file(
        &vault.path().join("done/other_done.md"),
        &archive_note(
            "other",
            "- [x] other archived #task [completion:: 2025-01-01]\n",
        ),
    );

    let report = run_preview(&vault);
    let value = json(&report);
    assert_eq!(report.exit.code, 1);
    assert_eq!(report.exit.reason, Some("archive_binding_conflict"));
    let holds = value["holds"].as_array().expect("holds");
    assert!(
        holds.iter().any(|hold| hold["path"] == "holds.md"
            && hold["reason"] == "duplicate_block_id"),
        "{holds:?}"
    );
    let moves = value["moves"].as_array().expect("moves");
    assert!(
        moves.iter().any(|entry| entry["source"] == "safe.md"),
        "{moves:?}"
    );
    assert!(
        moves.iter().all(|entry| {
            entry["source"] != "conflict.md" && entry["source"] != "holds.md"
        }),
        "{moves:?}"
    );
}
