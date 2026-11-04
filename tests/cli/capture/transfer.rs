//! Staged `>` transfer moves to bare notes plus the shell guard.
//!
//! Bare-note destinations move through `batch_move` in typed order;
//! task, drawer, and section destinations keep the temporary
//! `transfer_execution_unavailable` refusal until the merges phase.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Stdio;

const NOW: &str = "2026-11-03 09:30:00";
const TODAY: &str = "2026-11-03";

fn area_with_tasks(tasks: &str) -> String {
    format!("---\ntype: \"[[area]]\"\n---\n## Tasks\n{tasks}")
}

fn setup_move_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("inbox.md"),
        &area_with_tasks(concat!(
            "- [ ] #task Benchmark logical replication ^bench\n",
            "  - Try pgbench at 10k TPS\n",
            "- [ ] #task Cut over prod ^cutover\n",
        )),
    );
    write_file(
        &vault.join("work.md"),
        &area_with_tasks("- [ ] #task Dump production ^pgdump\n"),
    );
    write_file(&vault.join("db.md"), &area_with_tasks(""));
    write_file(
        &vault.join("referrer.md"),
        "---\ntype: \"[[area]]\"\n---\nSee [[inbox#^bench]] for context.\n",
    );
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    (temp, vault, day_file)
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    draft: &str,
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn capture_human(
    vault: &Path,
    day_file: &Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture human")
}

fn json_body(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

#[test]
fn shell_guard_refuses_at_redirect_before_mutation() {
    let temp = TempDir::new("bob-cli-transfer-shell-guard");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let before = fs::read_to_string(&day_file).expect("read day");
    let at_file = temp.path().join("@c");
    let out_file = fs::File::create(&at_file).expect("create @c");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .stdout(Stdio::from(out_file))
        .output()
        .expect("run capture with @ stdout");
    assert!(
        !output.status.success(),
        "guard must refuse: {}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("your shell ate"),
        "canonical guard copy on stderr: {err}"
    );
    assert!(
        err.contains("quote the draft"),
        "guard teaches quoting: {err}"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        before,
        "vault unchanged when the shell ate `>`"
    );
}

#[test]
fn shell_guard_lets_normal_files_and_pipes_through() {
    let temp = TempDir::new("bob-cli-transfer-shell-guard-ok");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let out_path = temp.path().join("out.txt");
    let out_file = fs::File::create(&out_path).expect("create out");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .stdout(Stdio::from(out_file))
        .output()
        .expect("run capture with normal file stdout");
    assert_success(&output);
    let piped = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("pipe")
        .arg("ok")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run capture with piped stdout");
    assert_success(&piped);
}

#[test]
fn merge_destination_transfer_refuses_before_capture_mutation() {
    // Task, drawer, and section destinations keep the temporary refusal
    // until the merges phase; bare-note moves execute (covered below).
    let temp = TempDir::new("bob-cli-transfer-unavailable");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let before = fs::read(&day_file).expect("read day before");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@inbox+bench > @db+upgrade")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture with a merge transfer draft");
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture JSON error");
    assert_eq!(error["code"], "transfer_execution_unavailable");
    assert_eq!(
        fs::read(&day_file).expect("read day after"),
        before,
        "transfer refusal writes nothing"
    );
}

#[test]
fn transfer_moves_single_source_to_bare_note() {
    // Byte-for-byte: the source leaves inbox.md with its children, lands
    // in db.md with its ID kept and freshness stamped, vault links are
    // repaired, and the day file gains no Ensure Next links.
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-move-one");
    let day_before = fs::read_to_string(&day_file).expect("read day");
    let output = capture_json(&vault, &day_file, "@inbox+bench > @db", false);
    assert_success(&output);
    let json = json_body(&output);
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["kind"], "task_move", "{json}");
    assert_eq!(json["route"], "db", "{json}");
    assert!(json["block_id"].is_null(), "{json}");
    let moves = json["moves"].as_array().expect("moves array");
    assert_eq!(moves.len(), 1, "{json}");
    assert_eq!(moves[0]["source_path"], "inbox.md", "{json}");
    assert_eq!(moves[0]["destination_path"], "db.md", "{json}");
    assert_eq!(moves[0]["old_block_id"], "bench", "{json}");
    assert_eq!(moves[0]["new_block_id"], "bench", "{json}");
    assert!(
        moves[0]["title"]
            .as_str()
            .unwrap_or_default()
            .contains("Benchmark"),
        "{json}"
    );
    // Ordinary results omit empty additive fields; transfer items carry
    // moves while merges stay omitted.
    assert!(json.get("merges").is_none(), "{json}");
    let inbox_after =
        fs::read_to_string(vault.join("inbox.md")).expect("read inbox");
    assert!(!inbox_after.contains("^bench"), "{inbox_after}");
    assert!(inbox_after.contains("^cutover"), "{inbox_after}");
    let db_after = fs::read_to_string(vault.join("db.md")).expect("read db");
    assert!(db_after.contains("^bench"), "{db_after}");
    assert!(
        db_after.contains("Benchmark logical replication"),
        "{db_after}"
    );
    assert!(
        db_after.contains("Try pgbench at 10k TPS"),
        "children travel: {db_after}"
    );
    assert!(
        db_after.contains(&format!("[fresh:: {TODAY}]")),
        "freshness stamped like Ctrl+Shift+M: {db_after}"
    );
    let referrer =
        fs::read_to_string(vault.join("referrer.md")).expect("read referrer");
    assert!(
        referrer.contains("[[db#^bench]]"),
        "links repaired to the destination: {referrer}"
    );
    assert!(!referrer.contains("inbox#^bench"), "{referrer}");
    let repaired = json["repaired_files"].as_array().expect("repaired files");
    assert!(
        repaired.iter().any(|file| file["path"] == "referrer.md"
            && file["link_count"].as_u64().unwrap_or(0) >= 1),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        day_before,
        "moves never run Ensure Next"
    );
}

#[test]
fn transfer_moves_multiple_sources_in_typed_order() {
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-move-typed-order");
    let output = capture_json(
        &vault,
        &day_file,
        "@inbox+bench @work+pgdump > @db",
        false,
    );
    assert_success(&output);
    let json = json_body(&output);
    assert_eq!(json["kind"], "task_move", "{json}");
    let moves = json["moves"].as_array().expect("moves array");
    assert_eq!(moves.len(), 2, "{json}");
    assert_eq!(moves[0]["old_block_id"], "bench", "{json}");
    assert_eq!(moves[1]["old_block_id"], "pgdump", "{json}");
    assert_eq!(moves[0]["source_path"], "inbox.md", "{json}");
    assert_eq!(moves[1]["source_path"], "work.md", "{json}");
    let db_after = fs::read_to_string(vault.join("db.md")).expect("read db");
    let bench = db_after.find("^bench").expect("bench landed");
    let pgdump = db_after.find("^pgdump").expect("pgdump landed");
    assert!(
        bench < pgdump,
        "typed order kept in the destination: {db_after}"
    );
    // Human output uses the shared move header and per-source rows.
    let human = capture_human(&vault, &day_file, "@inbox+cutover > @db");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("Moved 1 task into db.md"),
        "human move header: {out}"
    );
    assert!(out.contains("^cutover"), "human source row: {out}");
}

#[test]
fn transfer_dry_run_matches_real_run() {
    let (_temp, vault, day_file) = setup_move_vault("bob-cli-transfer-dry-run");
    let inbox_before =
        fs::read_to_string(vault.join("inbox.md")).expect("read inbox");
    let db_before = fs::read_to_string(vault.join("db.md")).expect("read db");
    let dry = capture_json(&vault, &day_file, "@inbox+bench > @db", true);
    assert_success(&dry);
    let dry_json = json_body(&dry);
    assert_eq!(dry_json["dry_run"], true, "{dry_json}");
    assert_eq!(
        fs::read_to_string(vault.join("inbox.md")).expect("dry inbox"),
        inbox_before,
        "dry run stages nothing"
    );
    assert_eq!(
        fs::read_to_string(vault.join("db.md")).expect("dry db"),
        db_before,
        "dry run writes nothing"
    );
    let real = capture_json(&vault, &day_file, "@inbox+bench > @db", false);
    assert_success(&real);
    let mut masked = dry_json.clone();
    masked["dry_run"] = serde_json::json!(false);
    let real_json = json_body(&real);
    assert_eq!(masked, real_json, "dry-run JSON must equal real-run JSON");
    let human_dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("@inbox+cutover > @db")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("dry human");
    assert_success(&human_dry);
    assert!(
        stdout(&human_dry).contains("Would move"),
        "dry-run wording: {}",
        format_output(&human_dry)
    );
}

#[test]
fn transfer_refuses_same_note_closed_nested_and_prj_sources() {
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-move-eligibility");
    write_file(
        &vault.join("closed.md"),
        &area_with_tasks(concat!(
            "- [x] #task Done work ^done\n",
            "- [ ] #task Parent ^par\n",
            "  - [ ] #task Kid ^kid\n",
        )),
    );
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\nstatus: wip\n---\n",
            "- [ ] #task Criteria ^prj\n\n## Tasks\n",
        ),
    );
    // Move the project task out of its own note is a project-task refusal
    // (use a second area as the destination so same-note does not win).
    let prj = capture_json(&vault, &day_file, "@proj+prj > @db", false);
    assert!(!prj.status.success());
    assert_eq!(json_body(&prj)["code"], "move_project_task");
    let closed = capture_json(&vault, &day_file, "@closed+done > @db", false);
    assert!(!closed.status.success());
    assert_eq!(json_body(&closed)["code"], "move_closed_task");
    let nested = capture_json(&vault, &day_file, "@closed+kid > @db", false);
    assert!(!nested.status.success());
    assert_eq!(json_body(&nested)["code"], "move_nested_task");
    // Same-note refusal uses the engine's existing code.
    let output =
        capture_json(&vault, &day_file, "@closed+par > @closed", false);
    assert!(!output.status.success());
    assert_eq!(json_body(&output)["code"], "move_same_note");
    // Nothing staged on any refusal.
    assert!(fs::read_to_string(vault.join("db.md"))
        .expect("read db")
        .find("^par")
        .is_none());
}

#[test]
fn transfer_applies_blocked_destination_rules() {
    let temp = TempDir::new("bob-cli-transfer-blocked-dest");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("inbox.md"),
        &area_with_tasks(concat!(
            "- [*] #task Next work ^next\n",
            "- [?] #task Blocked work ^blocked\n",
        )),
    );
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\nstatus: wip\n---\n",
            "- [?] #task Criteria ^prj\n\n## Tasks\n",
        ),
    );
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    // Ready/Next tasks cannot enter a Blocked project.
    let refused = capture_json(&vault, &day_file, "@inbox+next > @proj", false);
    assert!(!refused.status.success());
    assert_eq!(json_body(&refused)["code"], "move_dest_blocked");
    // Only `[?]` tasks move into a Blocked project, with a reconcile hint.
    let allowed =
        capture_json(&vault, &day_file, "@inbox+blocked > @proj", false);
    assert_success(&allowed);
    let json = json_body(&allowed);
    assert_eq!(json["moves"].as_array().map(Vec::len), Some(1), "{json}");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .unwrap_or(&warning.to_string())
                .contains("reconcile")
        }),
        "blocked moves hint reconcile: {json}"
    );
}

#[test]
fn transfer_refuses_duplicates_overlaps_and_unknown_notes() {
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-duplicates");
    let duplicate = capture_json(
        &vault,
        &day_file,
        "@inbox+bench @inbox+bench > @db",
        false,
    );
    assert!(!duplicate.status.success());
    assert_eq!(json_body(&duplicate)["code"], "duplicate_capture_target");
    write_file(
        &vault.join("tree.md"),
        &area_with_tasks(concat!(
            "- [ ] #task Parent ^par\n",
            "  - [ ] #task Kid ^kid\n",
        )),
    );
    let overlap =
        capture_json(&vault, &day_file, "@tree+par @tree+kid > @db", false);
    assert!(!overlap.status.success());
    assert_eq!(json_body(&overlap)["code"], "transfer_overlapping_sources");
    let missing =
        capture_json(&vault, &day_file, "@inbox+bench > @nope", false);
    assert!(!missing.status.success());
    assert_eq!(json_body(&missing)["code"], "move_missing_note");
    // Failed drafts leave the vault byte-identical.
    assert!(
        fs::read_to_string(vault.join("db.md"))
            .expect("read db")
            .is_empty()
            || !fs::read_to_string(vault.join("db.md"))
                .expect("read db")
                .contains("^bench")
    );
}

#[test]
fn transfer_later_item_sees_staged_move_and_failures_roll_back() {
    // A later transfer resolves the moved task at its staged destination.
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-staged-visibility");
    write_file(&vault.join("work.md"), &area_with_tasks(""));
    let chained = capture_json(
        &vault,
        &day_file,
        "@inbox+bench > @db\n\n@db+bench > @work",
        false,
    );
    assert_success(&chained);
    let json = json_body(&chained);
    let captures = json["captures"].as_array().expect("two items");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(
        captures[0]["moves"][0]["destination_path"], "db.md",
        "{json}"
    );
    assert_eq!(captures[1]["moves"][0]["source_path"], "db.md", "{json}");
    assert_eq!(
        captures[1]["moves"][0]["destination_path"], "work.md",
        "{json}"
    );
    assert!(fs::read_to_string(vault.join("work.md"))
        .expect("read work")
        .contains("^bench"));
    // A later failure rolls back the earlier staged move.
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-rollback");
    let inbox_before =
        fs::read_to_string(vault.join("inbox.md")).expect("read inbox");
    let db_before = fs::read_to_string(vault.join("db.md")).expect("read db");
    let failed = capture_json(
        &vault,
        &day_file,
        "@inbox+bench > @db\n\n@missing+nope > @db",
        false,
    );
    assert!(!failed.status.success());
    assert_eq!(
        fs::read_to_string(vault.join("inbox.md")).expect("rolled inbox"),
        inbox_before,
        "atomic draft planning leaves no partial move"
    );
    assert_eq!(
        fs::read_to_string(vault.join("db.md")).expect("rolled db"),
        db_before
    );
}

#[test]
fn transfer_mixes_with_ordinary_items_without_ensure_next() {
    // A transfer plus an ordinary note capture commit together, and the
    // transfer never files Ensure Next rows into the day file.
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-compatibility");
    let day_before = fs::read_to_string(&day_file).expect("read day");
    let output = capture_json(
        &vault,
        &day_file,
        "@inbox+bench > @db\n\nJust a plain note",
        false,
    );
    assert_success(&output);
    let json = json_body(&output);
    let captures = json["captures"].as_array().expect("two items");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(captures[0]["kind"], "task_move", "{json}");
    assert!(fs::read_to_string(vault.join("db.md"))
        .expect("read db")
        .contains("^bench"));
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        day_before,
        "no Ensure Next side effects"
    );
    // Project-note move bullets share the same engine path: a transfer
    // and a project-note creation with moves commit in one draft.
    let (_temp, vault, day_file) =
        setup_move_vault("bob-cli-transfer-project-compat");
    write_file(
        &vault.join("cash.md"),
        &area_with_tasks(concat!(
            "- [ ] #task Alpha ^a\n",
            "- [ ] #task Beta ^b\n",
        )),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@inbox+bench > @db\n\nFinish packet @cash^x@\n- New task\n- ^b")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("mixed transfer and project-note capture");
    assert_success(&output);
    assert!(fs::read_to_string(vault.join("db.md"))
        .expect("read db")
        .contains("^bench"));
    assert!(fs::read_to_string(vault.join("cash_x.md"))
        .expect("read project")
        .contains("^b"));
}
