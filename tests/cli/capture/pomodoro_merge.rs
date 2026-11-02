//! Capture `=*` merge end to end.

use crate::support::*;
use std::fs;

fn write_vault(vault: &std::path::Path, day_name: &str, day_body: &str) {
    fs::create_dir_all(vault.join("2026")).expect("create vault year");
    write_file(&vault.join(format!("2026/{day_name}")), day_body);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task Build task [type:: BUILD] ^a1\n",
            "- [ ] #task Fix task [type:: FIX] ^a2\n",
            "- [ ] #task Read task [type:: READ] ^a3\n",
        ),
    );
}

fn day_body_three() -> &'static str {
    concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] (**0900-1000**) — BUILD\n",
        "  - [[tasks#^a1]]\n",
        "- [ ] () — FIX\n",
        "  - [[tasks#^a2]]\n",
        "- [ ] () — READ\n",
        "  - [[tasks#^a3]]\n",
    )
}

#[test]
fn capture_merge_absorbs_planned_dests_into_src() {
    let temp = TempDir::new("bob-cli-capture-merge-all");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#fix#read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run merge capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_merge");
    assert_eq!(json["pomodoro_merge"]["before"], "BUILD");
    assert_eq!(json["pomodoro_merge"]["after"], "BUILD + FIX + READ");
    assert_eq!(json["pomodoro_merge"]["source_role"], "named");
    assert!(json["pomodoro_merge"].get("explicit").is_none());
    let absorbed = json["pomodoro_merge"]["absorbed"]
        .as_array()
        .expect("absorbed array");
    assert_eq!(absorbed.len(), 2);
    assert_eq!(absorbed[0]["name"], "FIX");
    assert_eq!(absorbed[0]["role"], "named");
    assert_eq!(absorbed[1]["name"], "READ");
    assert_eq!(absorbed[1]["role"], "named");
}

#[test]
fn capture_merge_writes_and_drops_duplicate_links() {
    let temp = TempDir::new("bob-cli-capture-merge-commit");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "  - [[tasks#^a2]]\n",
            "- [ ] () — FIX\n",
            "  - [[tasks#^a2]]\n",
            "  - [[tasks#^a3]]\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=*build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run merge write");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["pomodoro_merge"]["after"], "BUILD + FIX");
    let absorbed = json["pomodoro_merge"]["absorbed"]
        .as_array()
        .expect("absorbed");
    assert_eq!(absorbed[0]["dropped_duplicates"], 1);
    let day = fs::read_to_string(vault.join("2026/20261101.md")).expect("day");
    assert!(day.contains("BUILD + FIX"), "{day}");
    assert!(!day.contains("- [ ] () — FIX\n"), "{day}");
    let a2_count = day.matches("[[tasks#^a2]]").count();
    assert_eq!(a2_count, 1, "{day}");
    assert!(day.contains("[[tasks#^a3]]"), "{day}");
}

#[test]
fn capture_merge_guards_teach_precisely() {
    let temp = TempDir::new("bob-cli-capture-merge-guards");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let same = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#build")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("same pomodoro");
    assert!(!same.status.success());
    let same_json: serde_json::Value =
        serde_json::from_str(stdout(&same).trim()).expect("json");
    assert_eq!(same_json["code"], "merge_same_pomodoro");

    let timed = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*fix#build")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("timed dest");
    assert!(!timed.status.success());
    let timed_json: serde_json::Value =
        serde_json::from_str(stdout(&timed).trim()).expect("json");
    assert_eq!(timed_json["code"], "merge_timed_pomodoro");
    let err = timed_json["error"].as_str().unwrap_or("");
    assert!(err.contains("=*build#fix"), "{err}");

    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*build#nope")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("missing dest");
    assert!(!missing.status.success());
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("json");
    assert_eq!(missing_json["code"], "merge_target_missing");
}

#[test]
fn capture_merge_already_part_repeated_and_meet_guards_teach() {
    let temp = TempDir::new("bob-cli-capture-merge-more-guards");
    let vault = temp.path().join("vault");
    let day_rel = "2026/20261101.md";
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000**) — BUILD + FIX\n",
            "  - [[tasks#^a1]]\n",
            "  - [[tasks#^a2]]\n",
            "- [ ] () — READ\n",
            "  - [[tasks#^a3]]\n",
            "- [ ] () — MEET\n",
        ),
    );
    let before = fs::read_to_string(vault.join(day_rel)).expect("day");
    let refusal = |token: &str, code: &str, teaches: &str| {
        for dry_run in [true, false] {
            let output = merge_capture(&vault, token, dry_run);
            assert!(!output.status.success(), "{token} should refuse");
            let json: serde_json::Value =
                serde_json::from_str(stdout(&output).trim()).expect("json");
            assert_eq!(json["code"], code, "{token}: {json}");
            let error = json["error"].as_str().unwrap_or_default();
            assert!(error.contains(teaches), "{token}: {error}");
        }
    };

    refusal(
        "=*build#fix",
        "merge_already_part",
        "FIX is already part of BUILD + FIX",
    );
    refusal(
        "=*build#read#read",
        "merge_dest_repeated",
        "READ is named twice",
    );
    refusal("=*build#meet", "merge_meet", "MEET entries never merge");
    refusal("=*meet#read", "merge_meet", "MEET entries never merge");

    let after = fs::read_to_string(vault.join(day_rel)).expect("day");
    assert_eq!(after, before, "refused merges must leave the ledger");
}

#[test]
fn capture_merge_human_output_names_absorbed() {
    let temp = TempDir::new("bob-cli-capture-merge-human");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=*build#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("human merge");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("would merge") || text.contains("merged"),
        "{text}"
    );
    assert!(text.contains("FIX"), "{text}");
    assert!(text.contains("BUILD + FIX"), "{text}");
    assert!(
        !text.contains("same as"),
        "fully typed merge stays unchanged: {text}"
    );
}

#[test]
fn bare_merge_absorbs_the_next_planned_and_reports_its_spelling() {
    let temp = TempDir::new("bob-cli-capture-merge-next");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("bare merge capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["before"], "BUILD");
    assert_eq!(merge["after"], "BUILD + FIX");
    assert_eq!(merge["source_role"], "current");
    assert_eq!(merge["explicit"], "=*build#fix");
    assert_eq!(merge["absorbed"][0]["name"], "FIX");
    assert_eq!(merge["absorbed"][0]["role"], "next");

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("human bare merge");
    assert_success(&human);
    assert!(
        stdout(&human)
            .contains("same as =*build#fix · current BUILD, next FIX"),
        "{}",
        stdout(&human)
    );

    let named_destination = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*#fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("implied source merge");
    assert_success(&named_destination);
    let named_json: serde_json::Value =
        serde_json::from_str(stdout(&named_destination).trim()).expect("json");
    assert_eq!(named_json["pomodoro_merge"]["source_role"], "current");
    assert_eq!(named_json["pomodoro_merge"]["explicit"], "=*build#fix");
    assert_eq!(named_json["pomodoro_merge"]["absorbed"][0]["role"], "named");
}

#[test]
fn repeated_bare_merge_grows_one_planned_entry_at_a_time() {
    let temp = TempDir::new("bob-cli-capture-merge-next-twice");
    let vault = temp.path().join("vault");
    write_vault(&vault, "20261101.md", day_body_three());
    for expected in ["BUILD + FIX", "BUILD + FIX + READ"] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("=*")
            .env("BOB_NOW", "2026-11-01 09:30")
            .output()
            .expect("write bare merge");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["pomodoro_merge"]["after"], expected);
    }
}

#[test]
fn bare_merge_uses_next_up_without_a_running_session_and_skips_meet() {
    let temp = TempDir::new("bob-cli-capture-merge-next-up");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — PLAN\n",
            "- [ ] () — MEET\n",
            "- [ ] () — FIX\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("next-up merge");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["source_role"], "next_up");
    assert_eq!(merge["before"], "PLAN");
    assert_eq!(merge["after"], "PLAN + FIX");
    assert_eq!(merge["absorbed"][0]["role"], "next");
}

#[test]
fn explicit_source_uses_the_next_entry_below_it_without_wrapping() {
    let temp = TempDir::new("bob-cli-capture-merge-below-source");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — BUILD\n",
            "- [ ] () — FIX\n",
            "- [ ] () — READ\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*fix")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("explicit source bare merge");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["before"], "FIX");
    assert_eq!(merge["after"], "FIX + READ");
    assert_eq!(merge["source_role"], "named");
    assert_eq!(merge["absorbed"][0]["name"], "READ");
    assert_eq!(merge["absorbed"][0]["role"], "next");
}

#[test]
fn bare_merge_teaches_missing_next_entry_and_omits_unsafe_spelling() {
    let temp = TempDir::new("bob-cli-capture-merge-next-missing");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!("## Pomodoros\n\n", "- [ ] () — FIX\n", "- [ ] () — READ\n",),
    );
    let missing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("missing next entry");
    assert!(!missing.status.success());
    let error: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("error JSON");
    assert_eq!(error["code"], "merge_next_missing");
    assert!(error["error"].as_str().unwrap().contains("`=*read#fix`"));

    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — FIX\n",
            "- [ ] () — READ\n",
            "- [ ] () — MEET\n",
        ),
    );
    let meet_only_below = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*read")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("MEET passed during missing merge");
    assert!(!meet_only_below.status.success());
    let meet_json: serde_json::Value =
        serde_json::from_str(stdout(&meet_only_below).trim()).expect("json");
    assert!(meet_json["error"]
        .as_str()
        .unwrap()
        .contains("MEET never merges"));

    write_vault(&vault, "20261101.md", "## Pomodoros\n\n- [ ] () — PLAN\n");
    let no_other_planned = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("no next planned entry");
    assert!(!no_other_planned.status.success());
    let no_other_json: serde_json::Value =
        serde_json::from_str(stdout(&no_other_planned).trim()).expect("json");
    assert_eq!(no_other_json["code"], "merge_next_missing");
    assert!(no_other_json["error"]
        .as_str()
        .unwrap()
        .ends_with("Nothing planned below PLAN to merge"));

    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — FIX\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "- [ ] () — FIX\n",
        ),
    );
    let ambiguous_name = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("=*")
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("duplicate destination name");
    assert_success(&ambiguous_name);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&ambiguous_name).trim())
            .expect("capture JSON");
    assert!(json["pomodoro_merge"].get("explicit").is_none());
}

fn merge_capture(
    vault: &std::path::Path,
    token: &str,
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
        .arg(token)
        .env("BOB_NOW", "2026-11-01 09:30")
        .output()
        .expect("run merge capture")
}

fn merge_json(
    vault: &std::path::Path,
    token: &str,
    dry_run: bool,
) -> serde_json::Value {
    let output = merge_capture(vault, token, dry_run);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn assert_dry_run_leaves_ledger(
    vault: &std::path::Path,
    day_rel: &str,
    token: &str,
) -> (String, serde_json::Value) {
    let day_file = vault.join(day_rel);
    let before = fs::read_to_string(&day_file).expect("day before dry-run");
    let json = merge_json(vault, token, true);
    let after = fs::read_to_string(&day_file).expect("day after dry-run");
    assert_eq!(after, before, "dry-run must leave the ledger unchanged");
    (before, json)
}

#[test]
fn bare_merge_absorbs_the_below_duplicate_and_leaves_the_above_row() {
    let temp = TempDir::new("bob-cli-capture-merge-dup-identity");
    let vault = temp.path().join("vault");
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] () — FIX\n",
        "  - above child\n",
        "- [ ] (**0900-1000**) — BUILD\n",
        "  - [[tasks#^a1]]\n",
        "- [ ] () — FIX\n",
        "  - below child\n",
    );
    write_vault(&vault, "20261101.md", day);
    let day_rel = "2026/20261101.md";
    let day_file = vault.join(day_rel);
    let (before, dry) = assert_dry_run_leaves_ledger(&vault, day_rel, "=*");
    let merge = &dry["pomodoro_merge"];
    assert_eq!(merge["before"], "BUILD");
    assert_eq!(merge["after"], "BUILD + FIX");
    assert_eq!(merge["source_role"], "current");
    assert!(merge.get("explicit").is_none(), "{merge}");
    assert_eq!(merge["entry_line"], 5);
    assert_eq!(merge["absorbed"][0]["name"], "FIX");
    assert_eq!(merge["absorbed"][0]["role"], "next");
    assert_eq!(merge["absorbed"][0]["from_line"], 7);
    assert_eq!(dry["task_line"], "- [ ] (**0900-1000**) — BUILD + FIX");

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-01 09:30",
        &["=*"],
    );
    assert_eq!(json["pomodoro_merge"]["absorbed"][0]["from_line"], 7);
    let after = fs::read_to_string(&day_file).expect("day after write");
    assert_ne!(after, before);
    assert!(
        after.contains("- [ ] () — FIX\n  - above child\n"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] (**0900-1000**) — BUILD + FIX\n  - [[tasks#^a1]]\n  - below child\n"),
        "{after}"
    );
    assert!(!after.contains("- below child\n- [ ] () — FIX"), "{after}");
    assert_eq!(after.matches("- [ ] () — FIX").count(), 1, "{after}");
}

#[test]
fn bare_merge_leaves_a_closed_duplicate_above_the_source() {
    let temp = TempDir::new("bob-cli-capture-merge-closed-dup");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] () — FIX\n",
            "  - closed child\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "- [ ] () — FIX\n",
            "  - open child\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-01 09:30",
        &["=*"],
    );
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["after"], "BUILD + FIX");
    assert_eq!(merge["absorbed"][0]["from_line"], 7);
    let after = fs::read_to_string(&day_file).expect("day");
    assert!(
        after.contains("- [x] () — FIX\n  - closed child\n"),
        "{after}"
    );
    assert!(after.contains("  - open child\n"), "{after}");
    assert!(!after.contains("- [ ] () — FIX\n"), "{after}");
}

#[test]
fn merge_keeps_later_rows_when_children_shift_onto_the_dest_line() {
    let temp = TempDir::new("bob-cli-capture-merge-shift-later");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "- [ ] () — FIX\n",
            "  - fix one\n",
            "  - fix two\n",
            "- [ ] () — READ\n",
            "  - [[tasks#^a3]]\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-01 09:30",
        &["=*"],
    );
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["after"], "BUILD + FIX");
    assert_eq!(merge["absorbed"][0]["from_line"], 5);
    assert_eq!(merge["absorbed"][0]["moved_lines"], 2);
    let after = fs::read_to_string(&day_file).expect("day");
    assert!(
        after.contains("- [ ] (**0900-1000**) — BUILD + FIX\n  - [[tasks#^a1]]\n  - fix one\n  - fix two\n"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] () — READ\n  - [[tasks#^a3]]\n"),
        "{after}"
    );
    assert!(!after.contains("- [ ] () — FIX\n"), "{after}");
}

#[test]
fn explicit_dest_above_the_source_and_multiple_named_dests_keep_identity() {
    let temp = TempDir::new("bob-cli-capture-merge-above-and-many");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] () — FIX\n",
            "  - above child\n",
            "- [ ] (**0900-1000**) — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "- [ ] () — READ\n",
            "  - [[tasks#^a3]]\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-01 09:30",
        &["=*build#fix#read"],
    );
    let merge = &json["pomodoro_merge"];
    assert_eq!(merge["after"], "BUILD + FIX + READ");
    assert_eq!(merge["absorbed"][0]["name"], "FIX");
    assert_eq!(merge["absorbed"][0]["from_line"], 3);
    assert_eq!(merge["absorbed"][1]["name"], "READ");
    assert_eq!(merge["absorbed"][1]["from_line"], 7);
    let after = fs::read_to_string(&day_file).expect("day");
    assert!(
        after.contains("- [ ] (**0900-1000**) — BUILD + FIX + READ\n  - [[tasks#^a1]]\n  - above child\n  - [[tasks#^a3]]\n"),
        "{after}"
    );
    assert!(!after.contains("- [ ] () — FIX\n"), "{after}");
    assert!(!after.contains("- [ ] () — READ\n"), "{after}");
}

#[test]
fn merge_next_missing_puts_meet_in_the_first_clause() {
    let temp = TempDir::new("bob-cli-capture-merge-meet-clause");
    let vault = temp.path().join("vault");

    write_vault(
        &vault,
        "20261101.md",
        concat!("## Pomodoros\n\n", "- [ ] () — PLAN\n", "- [ ] () — MEET\n"),
    );
    let plan_then_meet = merge_capture(&vault, "=*", true);
    assert!(!plan_then_meet.status.success());
    let plan_json: serde_json::Value =
        serde_json::from_str(stdout(&plan_then_meet).trim()).expect("json");
    assert_eq!(plan_json["code"], "merge_next_missing");
    assert!(
        plan_json["error"].as_str().unwrap().ends_with(
            "Nothing planned below PLAN to merge (MEET never merges)"
        ),
        "{}",
        plan_json["error"]
    );

    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n\n",
            "- [ ] () — FIX\n",
            "- [ ] () — READ\n",
            "- [ ] () — MEET\n",
        ),
    );
    let read_then_meet = merge_capture(&vault, "=*read", true);
    assert!(!read_then_meet.status.success());
    let read_json: serde_json::Value =
        serde_json::from_str(stdout(&read_then_meet).trim()).expect("json");
    assert_eq!(read_json["code"], "merge_next_missing");
    assert!(
        read_json["error"].as_str().unwrap().ends_with(
            "Nothing planned below READ to merge (MEET never merges); name one to absorb (for example `=*read#fix`)"
        ),
        "{}",
        read_json["error"]
    );

    write_vault(&vault, "20261101.md", "## Pomodoros\n\n- [ ] () — PLAN\n");
    let lone_plan = merge_capture(&vault, "=*", true);
    assert!(!lone_plan.status.success());
    let lone_json: serde_json::Value =
        serde_json::from_str(stdout(&lone_plan).trim()).expect("json");
    assert_eq!(lone_json["code"], "merge_next_missing");
    assert!(
        lone_json["error"]
            .as_str()
            .unwrap()
            .ends_with("Nothing planned below PLAN to merge"),
        "{}",
        lone_json["error"]
    );
}

#[test]
fn carried_meet_at_the_end_refuses_a_second_bare_merge() {
    let temp = TempDir::new("bob-cli-capture-merge-carried-meet");
    let vault = temp.path().join("vault");
    write_vault(
        &vault,
        "20261101.md",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0800-0815** [off:: 15m]) — MEET\n",
            "  - ~~[[#^swings]]~~\n",
            "- [ ] () — BUILD\n",
            "  - [[tasks#^a1]]\n",
            "- [ ] () — FIX\n",
            "  - [[tasks#^a2]]\n",
            "- [ ] () — MEET\n",
            "  - [[#^lincoln]]\n",
        ),
    );
    let day_file = vault.join("2026/20261101.md");
    let first = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-11-01 09:30",
        &["=*"],
    );
    assert_eq!(first["pomodoro_merge"]["after"], "BUILD + FIX");
    assert_eq!(first["pomodoro_merge"]["absorbed"][0]["name"], "FIX");
    let after_first = fs::read_to_string(&day_file).expect("day after first");
    assert!(
        after_first.contains("- [ ] () — MEET\n  - [[#^lincoln]]\n"),
        "{after_first}"
    );

    let before_second = after_first.clone();
    let second = merge_capture(&vault, "=*", true);
    assert!(!second.status.success());
    let error: serde_json::Value =
        serde_json::from_str(stdout(&second).trim()).expect("error JSON");
    assert_eq!(error["code"], "merge_next_missing");
    assert!(
        error["error"].as_str().unwrap().ends_with(
            "Nothing planned below BUILD + FIX to merge (MEET never merges)"
        ),
        "{}",
        error["error"]
    );
    let after_second = fs::read_to_string(&day_file).expect("day after refuse");
    assert_eq!(after_second, before_second);
    assert!(
        after_second.contains("- [ ] () — MEET\n  - [[#^lincoln]]\n"),
        "{after_second}"
    );
    assert!(after_second.contains("~~[[#^swings]]~~"), "{after_second}");
}
