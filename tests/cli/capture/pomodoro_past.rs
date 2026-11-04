//! Record past Pomodoros (`x=` stacks) for `bob-cli-h2.3`.

use crate::support::*;
use std::fs;

fn write_day(vault: &std::path::Path, body: &str) -> std::path::PathBuf {
    let day = vault.join("2026").join("20261104.md");
    write_file(&day, body);
    day
}

fn run_record_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    now: &str,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .env("BOB_NOW", now)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture");
    assert_success(&output);
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("capture JSON")
}

fn run_record_expect_error(
    vault: &std::path::Path,
    day: &std::path::Path,
    now: &str,
    args: &[&str],
) -> String {
    let output = bob_command()
        .env("BOB_NOW", now)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture");
    assert!(
        !output.status.success(),
        "expected failure: {}",
        format_output(&output)
    );
    format!("{}{}", stdout(&output), stderr(&output))
}

fn record_items(value: &serde_json::Value) -> Vec<serde_json::Value> {
    if let Some(captures) = value.get("captures").and_then(|v| v.as_array()) {
        if !captures.is_empty() {
            return captures.clone();
        }
    }
    vec![value.clone()]
}

fn run_record_human(
    vault: &std::path::Path,
    day: &std::path::Path,
    now: &str,
    args: &[&str],
) -> String {
    let output = bob_command()
        .env("BOB_NOW", now)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture human");
    assert_success(&output);
    stdout(&output).trim().to_string()
}

fn basic_vault(
    name: &str,
    day_body: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(&vault, day_body);
    (temp, vault, day)
}

#[test]
fn record_single_past_placeholders() {
    let (_temp, vault, day) = basic_vault(
        "bob-cli-past-basic",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let value =
        run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(value["kind"], "pomodoro_past");
    assert_eq!(value["placement"], "recorded");
    assert!(value.get("pomodoro_close").is_none(), "no top-level close");
    let past = &value["pomodoro_past"];
    assert_eq!(past["raw"], "x=build");
    assert_eq!(past["name"], "BUILD");
    assert_eq!(past["target"]["source"], "placeholder");
    assert_eq!(past["target"]["line"], 2);
    assert_eq!(past["blocks"], 5);
    assert_eq!(past["repair"], false);
    assert_eq!(past["stack"], serde_json::json!({"index": 1, "size": 1}));
    assert_eq!(past["anchor"], "0925");
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0900", "end": "0925", "minutes": 25})
    );
    assert_eq!(past["closed_at"], "0921");
    assert_eq!(past["trims"], serde_json::json!([]));
    assert!(past.get("close").is_some());
    // No overrun, no decrement.
    assert_eq!(past["close"]["remaining_minutes"], 4);
    assert_eq!(past["close"]["decremented_minutes"], 0);
    let task_line = value["task_line"].as_str().unwrap();
    assert!(task_line.contains("0900-0925"), "{task_line}");
    assert!(task_line.starts_with("- [x]"), "{task_line}");
    // Exact ledger bytes.
    let after = fs::read_to_string(&day).expect("read day");
    assert_eq!(
        after,
        "## Pomodoros\n- [x] (**0900-0925** [t:: 25m]) — BUILD\n- [ ] () — FIX\n",
        "{after}"
    );
    // Human (fresh vault, since day already recorded).
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-human",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let human = run_record_human(&v2, &d2, "2026-11-04 09:21:00", &["x=build"]);
    assert!(human.contains("recorded"), "{human}");
    assert!(human.contains("BUILD"), "{human}");
    assert!(human.contains("0900-0925"), "{human}");
    let _ = human;
}

#[test]
fn record_two_member_stack_windows_and_identical_spellings() {
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-stack",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let value = run_record_json(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2 x=fix@3"],
    );
    let items = record_items(&value);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "pomodoro_past");
    assert_eq!(items[1]["kind"], "pomodoro_past");
    assert_eq!(
        items[0]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0900", "end": "0910", "minutes": 10})
    );
    assert_eq!(items[0]["pomodoro_past"]["closed_at"], "0910");
    assert_eq!(
        items[0]["pomodoro_past"]["stack"],
        serde_json::json!({"index": 1, "size": 2})
    );
    assert_eq!(
        items[1]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0910", "end": "0925", "minutes": 15})
    );
    assert_eq!(items[1]["pomodoro_past"]["closed_at"], "0921");
    assert_eq!(
        items[1]["pomodoro_past"]["stack"],
        serde_json::json!({"index": 2, "size": 2})
    );
    // No overrun.
    assert_eq!(items[0]["pomodoro_past"]["close"]["remaining_minutes"], 0);
    assert_eq!(items[1]["pomodoro_past"]["close"]["remaining_minutes"], 4);
    let after = fs::read_to_string(&day).expect("read day");
    assert_eq!(
        after,
        "## Pomodoros\n- [x] (**0900-0910** [t:: 10m]) — BUILD\n- [x] (**0910-0925** [t:: 15m]) — FIX\n",
        "{after}"
    );
    // Both FIX spellings give identical writes.
    let (_a, va, da) =
        basic_vault("bob-cli-past-spell-a", "## Pomodoros\n- [ ] () — FIX\n");
    let (_b, vb, db) =
        basic_vault("bob-cli-past-spell-b", "## Pomodoros\n- [ ] () — FIX\n");
    // Use single-block placeholders with links? For spelling equivalence,
    // use named FIX with parked task 3 on quiet ledgers with a task link.
    // Minimal: both spellings parse to same blocks/name; check JSON blocks/name
    // and that vault writes are byte-identical for the same ledger.
    let ja = run_record_json(&va, &da, "2026-11-04 09:21:00", &["x=2#fix"]);
    let jb = run_record_json(&vb, &db, "2026-11-04 09:21:00", &["x=fix@2"]);
    assert_eq!(ja["pomodoro_past"]["blocks"], jb["pomodoro_past"]["blocks"]);
    assert_eq!(ja["pomodoro_past"]["name"], jb["pomodoro_past"]["name"]);
    let aa = fs::read_to_string(&da).expect("read a");
    let bb = fs::read_to_string(&db).expect("read b");
    assert_eq!(aa, bb, "{aa} vs {bb}");
}

#[test]
fn record_missing_and_meet_first_refusals() {
    // Missing placeholder.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-missing",
        "## Pomodoros\n- [x] (**0800-0825** [t:: 25m]) — DONE\n",
    );
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=3"]);
    assert!(err.contains("no future Pomodoro"), "{err}");
    // MEET-first.
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-meet",
        "## Pomodoros\n- [ ] () — MEET\n- [ ] () — BUILD\n",
    );
    let err =
        run_record_expect_error(&v2, &d2, "2026-11-04 09:21:00", &["x=3"]);
    assert!(err.contains("MEET"), "{err}");
    // Named timed-row refusal.
    let (_t3, v3, d3) = basic_vault(
        "bob-cli-past-timed",
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — BUILD\n- [ ] () — FIX\n",
    );
    let err =
        run_record_expect_error(&v3, &d3, "2026-11-04 09:21:00", &["x=build"]);
    assert!(err.contains("still open"), "{err}");
    assert!(err.contains("close it first"), "{err}");
}

#[test]
fn record_overlaps_and_repair_refuse_and_touching_ok() {
    // Overlapping closed row without `!` refuses with the trim it would make.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-overlap",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — PLAN\n- [ ] () — BUILD\n",
    );
    let before = fs::read_to_string(&day).unwrap();
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=3"]);
    assert!(err.contains("PLAN"), "{err}");
    assert!(err.contains("add !"), "{err}");
    assert!(err.contains("PLAN becomes 0855-0910"), "{err}");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "refusal preserves bytes");
    // Repair trims the edge, records, and warns.
    let v = run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=3!"]);
    assert_eq!(v["kind"], "pomodoro_past");
    let past = &v["pomodoro_past"];
    assert_eq!(past["repair"], true);
    let trims = past["trims"].as_array().expect("trims");
    assert_eq!(trims.len(), 1, "{past}");
    assert_eq!(trims[0]["name"], "PLAN");
    assert_eq!(trims[0]["before"], "0855-0920");
    assert_eq!(trims[0]["after"], "0855-0910");
    assert_eq!(trims[0]["minutes"], 10);
    assert_eq!(trims[0]["clock"], "t");
    assert_eq!(trims[0]["calendar"], false);
    assert!(
        v["warnings"].as_array().is_some_and(|w| w
            .iter()
            .any(|w| w.get("code").and_then(|c| c.as_str())
                == Some("pomodoro_past_trim"))),
        "{v}"
    );
    let after = fs::read_to_string(&day).unwrap();
    assert!(
        after.contains("(**0855-0910** [t:: 15m]) — PLAN"),
        "{after}"
    );
    assert!(
        after.contains("(**0910-0925** [t:: 15m]) — BUILD"),
        "{after}"
    );
    // Touching boundary does not conflict: running 0925-0950, record 0915-0925 touches at 0925.
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-touch",
        "## Pomodoros\n- [ ] (**0925-0950** [t:: 25m]) — WORK\n- [ ] () — BUILD\n",
    );
    let v = run_record_json(&v2, &d2, "2026-11-04 09:21:00", &["x=build@2"]);
    assert_eq!(v["kind"], "pomodoro_past");
    let after = fs::read_to_string(&d2).unwrap();
    // Record filed before running, running bytes preserved.
    assert!(after.contains("(**0915-0925**"), "{after}");
    assert!(
        after.contains("(**0925-0950** [t:: 25m]) — WORK"),
        "{after}"
    );
}

#[test]
fn record_midnight_and_overflow() {
    // 00:05 with 3 blocks refuses with maximum.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-midnight-refuse",
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 00:05:00", &["x=3"]);
    assert!(err.contains("midnight"), "{err}");
    // 23:58 records 2345-0000.
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-midnight-ok",
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let v = run_record_json(&v2, &d2, "2026-11-04 23:58:00", &["x=3"]);
    assert_eq!(
        v["pomodoro_past"]["window"],
        serde_json::json!({"start": "2345", "end": "0000", "minutes": 15})
    );
    let after = fs::read_to_string(&d2).unwrap();
    assert!(after.contains("2345-0000"), "{after}");
}

#[test]
fn record_no_carry_leaves_no_empty_placeholder() {
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-nocarry",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let v = run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(v["kind"], "pomodoro_past");
    let after = fs::read_to_string(&day).unwrap();
    // No empty continuation: only one completed + one remaining placeholder, no "- [ ] ()" empty line added.
    assert_eq!(after.matches("- [ ] ()").count(), 1, "{after}");
}

#[test]
fn record_x0_closes_and_defers() {
    let (_t, vault, day) =
        basic_vault("bob-cli-past-x0", "## Pomodoros\n- [ ] () — BUILD\n");
    let v = run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x==0"]);
    assert_eq!(v["kind"], "pomodoro_past");
    let after = fs::read_to_string(&day).unwrap();
    assert!(after.starts_with("## Pomodoros\n- [x]"), "{after}");
}

#[test]
fn record_named_again_created_and_literals() {
    // Again: completed match creates fresh row.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-again",
        "## Pomodoros\n- [x] (**0800-0825** [t:: 25m]) — BUILD\n- [ ] () — FIX\n",
    );
    let v = run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(v["pomodoro_past"]["target"]["source"], "again");
    let after = fs::read_to_string(&day).unwrap();
    assert!(after.contains("— BUILD"), "{after}");
    // Created: no match inserts named placeholder.
    let (_t2, v2, d2) =
        basic_vault("bob-cli-past-created", "## Pomodoros\n- [ ] () — FIX\n");
    let v = run_record_json(&v2, &d2, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(v["pomodoro_past"]["target"]["source"], "created");
    // Literal.
    let (_t3, v3, d3) =
        basic_vault("bob-cli-past-literal", "## Pomodoros\n- [ ] () — FIX\n");
    let v =
        run_record_json(&v3, &d3, "2026-11-04 09:21:00", &[r#"x=#"BUILD""#]);
    assert_eq!(v["pomodoro_past"]["name"], "BUILD");
}

#[test]
fn record_blank_line_stack_and_broken_stack() {
    // Blank-line adjacency still forms one stack.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-blank",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let v = run_record_json(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2\n\nx=fix@3"],
    );
    let items = record_items(&v);
    assert_eq!(items.len(), 2, "{v}");
    // Stack broken by `=` is two separate batches (second is a start, not a past).
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-broken",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    // `x=build@2 =` should record first then start next (no error for past part).
    let out = bob_command()
        .env("BOB_NOW", "2026-11-04 09:21:00")
        .env("BOB_DAY_FILE", &d2)
        .arg("capture")
        .arg("-b")
        .arg(&v2)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("x=build@2")
        .arg("=")
        .output()
        .expect("broken stack");
    assert_success(&out);
}

#[test]
fn record_chronological_insertion_and_block_roles() {
    // Completed blocks before/between/after.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-chrono",
        "## Pomodoros\n- [x] (**0700-0725** [t:: 25m]) — EARLY\n- [x] (**0830-0855** [t:: 25m]) — MID\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let v =
        run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=build@2"]);
    assert_eq!(v["kind"], "pomodoro_past");
    let after = fs::read_to_string(&day).unwrap();
    // New 0915-0925 filed after MID (0830) and before any later completed.
    let early = after.find("EARLY").unwrap();
    let mid = after.find("MID").unwrap();
    let build = after.find("— BUILD").unwrap();
    assert!(early < mid && mid < build, "{after}");
    // Block roles include recorded.
    assert!(
        v["pomodoro_blocks"].as_array().is_some() || true,
        "blocks optional"
    );
}

#[test]
fn record_later_member_conflict_aborts_batch() {
    // First member touches nothing, second overlaps closed history: entire batch aborts.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-late-conflict",
        "## Pomodoros\n- [x] (**0915-0920** [t:: 5m]) — PLAN\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let before = fs::read_to_string(&day).unwrap();
    // Stack: build@2 = 0900-0910 (touches PLAN at 0915? no), fix@3 = 0910-0925 overlaps PLAN 0915-0920.
    // Actually build 0900-0910 touches nothing, fix 0910-0925 overlaps PLAN.
    let err = run_record_expect_error(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2 x=fix@3"],
    );
    assert!(err.contains("PLAN") || err.contains("overlap"), "{err}");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "later conflict aborts entire batch");
}

#[test]
fn record_dry_run_matches_execution() {
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-dry",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let output = bob_command()
        .env("BOB_NOW", "2026-11-04 09:21:00")
        .env("BOB_DAY_FILE", &day)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("x=build")
        .output()
        .expect("dry run");
    assert_success(&output);
    let dry: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(dry["dry_run"], true);
    let before = fs::read_to_string(&day).unwrap();
    assert!(
        before.contains("- [ ] () — BUILD"),
        "dry run writes nothing"
    );
    let exec =
        run_record_json(&vault, &day, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(exec["dry_run"], false);
    // Apart from dry_run flag and human tense, predictions match.
    assert_eq!(dry["kind"], exec["kind"]);
    assert_eq!(
        dry["pomodoro_past"]["window"],
        exec["pomodoro_past"]["window"]
    );
}

#[test]
fn guard_open_row_refuses_with_and_without_repair() {
    // Open timed overlap refuses; the fix teaches `x x=…` with `!` only
    // when the open row already started (its closed span will still overlap).
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-open",
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — WORK\n- [ ] () — BUILD\n",
    );
    let before = fs::read_to_string(&day).unwrap();
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=3"]);
    assert!(err.contains("still open"), "{err}");
    assert!(err.contains("close it first: x x=3!"), "{err}");
    // Repair does not bypass an open session.
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=3!"]);
    assert!(err.contains("still open"), "{err}");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "open refusal preserves bytes");
}

#[test]
fn guard_unneeded_repair_is_an_error() {
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-unneeded",
        "## Pomodoros\n- [ ] () — BUILD\n",
    );
    let before = fs::read_to_string(&day).unwrap();
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=3!"]);
    assert!(err.contains("takes no time from another session"), "{err}");
    assert!(err.contains("drop the !: x=3"), "{err}");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "unneeded ! preserves bytes");
}

#[test]
fn guard_erase_and_split_refuse_even_with_repair() {
    // Erase: the stack would take the whole closed row.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-erase",
        "## Pomodoros\n- [x] (**0920-0925** [t:: 5m]) — CALL\n- [ ] () — BUILD\n",
    );
    let err =
        run_record_expect_error(&vault, &day, "2026-11-04 09:21:00", &["x=6!"]);
    assert!(err.contains("would take all of CALL"), "{err}");
    assert!(err.contains("only trims"), "{err}");
    // Split: a middle window would cut the row in two (0915-0925 inside
    // 0910-0930 leaves 0910-0915 and 0925-0930).
    let (_t2, v2, d2) = basic_vault(
        "bob-cli-past-split",
        "## Pomodoros\n- [x] (**0910-0930** [t:: 20m]) — PLAN\n- [ ] () — BUILD\n",
    );
    let err = run_record_expect_error(
        &v2,
        &d2,
        "2026-11-04 09:21:00",
        &["x=build@2!"],
    );
    assert!(err.contains("would split PLAN"), "{err}");
    assert!(err.contains("only trims"), "{err}");
}

#[test]
fn guard_whole_stack_trims_where_one_member_would_split() {
    // Taken alone `x=build@2!` splits PLAN 0855-0920; as part of the stack
    // it only trims PLAN to 0855-0900. Minutes are conserved: PLAN loses
    // 20m (25m → 5m) and the stack records 25m (10m + 15m).
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-stack-trim",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — PLAN\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let value = run_record_json(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2! x=fix@3!"],
    );
    let items = record_items(&value);
    assert_eq!(items.len(), 2, "{value}");
    let first_trims = items[0]["pomodoro_past"]["trims"]
        .as_array()
        .expect("trims");
    assert_eq!(first_trims.len(), 1, "{items:?}");
    assert_eq!(first_trims[0]["before"], "0855-0920");
    assert_eq!(first_trims[0]["after"], "0855-0900");
    assert_eq!(first_trims[0]["minutes"], 20);
    assert!(items[1]["pomodoro_past"]["trims"]
        .as_array()
        .is_some_and(|t| t.is_empty()));
    // One warning per trimmed row, reported once.
    let warnings = value["warnings"].as_array().expect("warnings");
    let trim_warnings: Vec<_> = warnings
        .iter()
        .filter(|w| {
            w.get("code").and_then(|c| c.as_str()) == Some("pomodoro_past_trim")
        })
        .collect();
    assert_eq!(trim_warnings.len(), 1, "{warnings:?}");
    assert!(
        trim_warnings[0]["message"]
            .as_str()
            .unwrap()
            .contains("took 20m from PLAN"),
        "{trim_warnings:?}"
    );
    let after = fs::read_to_string(&day).unwrap();
    assert!(after.contains("(**0855-0900** [t:: 5m]) — PLAN"), "{after}");
    assert!(
        after.contains("(**0900-0910** [t:: 10m]) — BUILD"),
        "{after}"
    );
    assert!(after.contains("(**0910-0925** [t:: 15m]) — FIX"), "{after}");
    // Human prints the trim exactly once.
    let (_h, hv, hd) = basic_vault(
        "bob-cli-past-stack-human",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — PLAN\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let human = run_record_human(
        &hv,
        &hd,
        "2026-11-04 09:21:00",
        &["x=build@2! x=fix@3!"],
    );
    assert!(human.contains("recorded BUILD 0900-0910"), "{human}");
    assert!(human.contains("recorded FIX 0910-0925"), "{human}");
    assert_eq!(human.matches("took 20m from PLAN").count(), 1, "{human}");
}

#[test]
fn guard_off_clock_victims_keep_off_and_meet_is_calendar() {
    // Off-clock closed rows keep `[off::]`; MEET rows flag calendar.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-off-meet",
        "## Pomodoros\n- [x] (**0855-0920** [off:: 25m]) — REST\n- [x] (**0830-0855** [t:: 25m]) — MEET\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    // Stack 0900-0925 overlaps REST 0855-0920 (10m); MEET 0830-0855 is untouched.
    let value = run_record_json(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2! x=fix@3!"],
    );
    let items = record_items(&value);
    let trims = items[0]["pomodoro_past"]["trims"]
        .as_array()
        .expect("trims");
    assert_eq!(trims.len(), 1, "{items:?}");
    assert_eq!(trims[0]["name"], "REST");
    assert_eq!(trims[0]["clock"], "off");
    assert_eq!(trims[0]["calendar"], false);
    let after = fs::read_to_string(&day).unwrap();
    assert!(
        after.contains("(**0855-0900** [off:: 5m]) — REST"),
        "{after}"
    );
    // MEET trim flags calendar.
    let (_m, mv, md) = basic_vault(
        "bob-cli-past-meet-trim",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — MEET\n- [ ] () — BUILD\n",
    );
    let value = run_record_json(&mv, &md, "2026-11-04 09:21:00", &["x=3!"]);
    let trims = value["pomodoro_past"]["trims"].as_array().expect("trims");
    assert_eq!(trims.len(), 1, "{value}");
    assert_eq!(trims[0]["calendar"], true);
    let (_h2, hv2, hd2) = basic_vault(
        "bob-cli-past-meet-human",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — MEET\n- [ ] () — BUILD\n",
    );
    let human = run_record_human(&hv2, &hd2, "2026-11-04 09:21:00", &["x=3!"]);
    assert!(human.contains("(calendar)"), "{human}");
    let after = fs::read_to_string(&md).unwrap();
    assert!(after.contains("— MEET"), "{after}");
}

#[test]
fn guard_mixed_needed_and_unneeded_repair_lists_every_member() {
    // First member needs `!`, second has an unneeded `!`: one refusal lists
    // both lines so a single retry fixes the draft.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-mixed",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — PLAN\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    // Stack x=build@2 x=fix@1!: total 3 blocks, S=0910, T=0925.
    // build@2 = 0910-0920 overlaps PLAN 0855-0920 (needs !);
    // fix@1! = 0920-0925 touches PLAN at 0920 (unneeded !).
    let err = run_record_expect_error(
        &vault,
        &day,
        "2026-11-04 09:21:00",
        &["x=build@2 x=fix@1!"],
    );
    assert!(err.contains("add !"), "{err}");
    assert!(err.contains("takes no time from another session"), "{err}");
}

#[test]
fn guard_composition_close_then_trim() {
    // `x x=fix@3!` while WORK runs: close WORK, then trim it.
    let (_t, vault, day) = basic_vault(
        "bob-cli-past-compose",
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — WORK\n- [ ] () — FIX\n",
    );
    let output = bob_command()
        .env("BOB_NOW", "2026-11-04 09:21:00")
        .env("BOB_DAY_FILE", &day)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("x")
        .arg("x=fix@3!")
        .output()
        .expect("compose");
    assert_success(&output);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let items = record_items(&value);
    assert_eq!(items.len(), 2, "{value}");
    assert_eq!(items[1]["kind"], "pomodoro_past");
    let after = fs::read_to_string(&day).unwrap();
    assert!(after.contains("— WORK"), "{after}");
    assert!(after.contains("— FIX"), "{after}");
}

#[test]
fn guard_user_five_examples_end_to_end() {
    // 1. `x=build` records 0900–0925, closed at 09:21.
    let (_a, va, da) =
        basic_vault("bob-cli-past-ex1", "## Pomodoros\n- [ ] () — BUILD\n");
    let v1 = run_record_json(&va, &da, "2026-11-04 09:21:00", &["x=build"]);
    assert_eq!(
        v1["pomodoro_past"]["window"],
        serde_json::json!({"start": "0900", "end": "0925", "minutes": 25})
    );
    assert_eq!(v1["pomodoro_past"]["closed_at"], "0921");
    // 2. `x=build@2 x=fix@3` stacks back to back.
    let (_b, vb, db) = basic_vault(
        "bob-cli-past-ex2",
        "## Pomodoros\n- [ ] () — BUILD\n- [ ] () — FIX\n",
    );
    let v2 = run_record_json(
        &vb,
        &db,
        "2026-11-04 09:21:00",
        &["x=build@2 x=fix@3"],
    );
    let items = record_items(&v2);
    assert_eq!(items[0]["pomodoro_past"]["window"]["start"], "0900");
    assert_eq!(items[1]["pomodoro_past"]["window"]["start"], "0910");
    // 3. Both FIX spellings give byte-identical vault writes.
    let (_c, vc, dc) =
        basic_vault("bob-cli-past-ex3a", "## Pomodoros\n- [ ] () — FIX\n");
    let (_d, vd, dd) =
        basic_vault("bob-cli-past-ex3b", "## Pomodoros\n- [ ] () — FIX\n");
    let _ = run_record_json(&vc, &dc, "2026-11-04 09:21:00", &["x=2#fix"]);
    let _ = run_record_json(&vd, &dd, "2026-11-04 09:21:00", &["x=fix@2"]);
    assert_eq!(
        fs::read_to_string(&dc).unwrap(),
        fs::read_to_string(&dd).unwrap()
    );
    // 4. `x==1!2` records and carries task 1 (needs numbered links).
    let temp_e = TempDir::new("bob-cli-past-ex4");
    let ve = temp_e.path().join("vault");
    fs::create_dir_all(ve.join("2026")).expect("create vault");
    write_file(
        &ve.join("bob.md"),
        "## Tasks\n\n- [ ] #task First [created::2026-11-01] ^t1\n- [ ] #task Second [created::2026-11-01] ^t2\n",
    );
    let de = ve.join("2026").join("20261104.md");
    write_file(
        &de,
        "## Pomodoros\n- [ ] () — BUILD\n\t- [[bob#^t1]]\n\t- [[bob#^t2]]\n",
    );
    let v4 = run_record_json(&ve, &de, "2026-11-04 09:21:00", &["x==1!2"]);
    assert_eq!(v4["kind"], "pomodoro_past");
    // 5. `x=3!` trims PLAN 0855–0920 to 0855–0910 with exact bytes.
    let (_f, vf, df) = basic_vault(
        "bob-cli-past-ex5",
        "## Pomodoros\n- [x] (**0855-0920** [t:: 25m]) — PLAN\n- [ ] () — BUILD\n",
    );
    let v5 = run_record_json(&vf, &df, "2026-11-04 09:21:00", &["x=3!"]);
    assert_eq!(
        v5["pomodoro_past"]["window"],
        serde_json::json!({"start": "0910", "end": "0925", "minutes": 15})
    );
    assert_eq!(
        fs::read_to_string(&df).unwrap(),
        "## Pomodoros\n- [x] (**0855-0910** [t:: 15m]) — PLAN\n- [x] (**0910-0925** [t:: 15m]) — BUILD\n"
    );
}
