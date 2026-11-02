//! Keep-moving capture integration (`docs/task-dependencies.md` §12.15,
//! KM1–KM32; `plan:202611/leaf_keep_moving_rust.md` phases `capture` and
//! `finish`).
//!
//! Focused CLI coverage for `!`, Pomodoro completion, retargeted close,
//! and nested complete labels: separate array shape, all/none/key/number
//! picks, mixed groups including project-created rows, folded counts,
//! no-candidate silence, both kill switches, write-free
//! `--require-pull-in-choice` (KM30), and unavailable-snapshot behavior
//! (KM31) through a controlled invalid-UTF-8 fixture.
//!
//! The `finish` phase adds the multi-item net pass (KM21), the approved
//! human copy, the cheap-gate wiring (KM32), and the stale-pick refusals
//! the Rust side has for the cycler's re-plan case (KM24).

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-10-05 09:30:00";

fn vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

fn area() -> String {
    "---\ntype: [[area]]\n---\n".to_string()
}

fn top_project() -> String {
    "---\ntype: [[project]]\nparent: [[dev]]\nstatus: wip\n---\n- [ ] #task #prj Sase ^prj\n"
        .to_string()
}

/// Leaf `sase_badges.md` with predecessor `^t`, `^prj`, and residents in
/// file order Ready, Next, Pending to prove lane sorting (P → N → R).
fn badges(extra: &str) -> String {
    format!(
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [ ] #task Draft the badge spec [id:: t] ^t\n",
            "- [ ] #task #prj Badges ^prj\n",
            "- [ ] #task Write badge docs [id:: a] ^a\n",
            "- [*] #task Wire badges into the README [id:: b] ^b\n",
            "- [/] #task Ship the badge renderer [id:: p] ^p\n",
            "{extra}"
        ),
        extra = extra
    )
}

fn leaf_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    leaf_vault_extra(name, "")
}

fn leaf_vault_extra(name: &str, extra: &str) -> (TempDir, PathBuf, PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("dev.md"), &area());
    write_file(&vault.join("sase.md"), &top_project());
    write_file(&vault.join("sase_badges.md"), &badges(extra));
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    (temp, vault, day_file)
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
) -> serde_json::Value {
    let output = capture_output(vault, day_file, flags, text, false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_output(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
    human: bool,
) -> Output {
    capture_output_with_config(vault, day_file, flags, text, human, None)
}

fn capture_output_with_config(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
    human: bool,
    config: Option<&Path>,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    if let Some(path) = config {
        command.env("BOB_CONFIG_FILE", path);
    }
    command
        .args(flags)
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture")
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read vault dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string();
                entries.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

// KM1: `!` closes T; keep-moving offers P, B, A in lane order with the
// separate array shape, trigger, group, project, and predecessors.
#[test]
fn km1_bang_reports_separate_array_in_lane_order() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km1-array");
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    // Unblocked stays empty; keep-moving carries the group. (KM1)
    assert_eq!(json["task_complete"]["unblocked"], serde_json::json!([]));
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving array");
    assert_eq!(moving.len(), 3, "{moving:?}");
    for row in moving {
        assert_eq!(row["trigger"], "last_planned");
        assert_eq!(row["offer"]["group"], "keep_moving");
        assert_eq!(row["offer"]["project"], "sase_badges.md");
        assert_eq!(
            row["unblocked_by"],
            serde_json::json!([{
                "note_path": "sase_badges.md",
                "block_id": "t",
                "text": "Draft the badge spec",
            }])
        );
        assert_eq!(row["not_linked"], "not_selected");
        assert!(row.get("link").is_none() || row["link"].is_null());
    }
    // Lane order: Pending, Next, Ready by offer number. (KM1)
    let mut by_n = moving.to_vec();
    by_n.sort_by_key(|row| row["offer"]["n"].as_u64().unwrap_or(0));
    let ids: Vec<_> = by_n
        .iter()
        .map(|row| row["block_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["p", "b", "a"], "{by_n:?}");
    // Destination is T's slot in FIX.
    for row in moving {
        assert_eq!(row["offer"]["destination"]["entry_name"], "FIX");
        assert_eq!(row["offer"]["destination"]["entry_role"], "slot");
    }
    // Counts include keep-moving; folds exist with additive keys.
    assert_eq!(json["pull_in"]["offer_count"], 3);
    assert_eq!(json["pull_in"]["picked_count"], 0);
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["group"], "keep_moving");
    assert_eq!(groups[0]["project"], "sase_badges.md");
    // Dry run writes nothing.
    let after = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    assert!(
        after.contains("- [ ] #task Draft the badge spec"),
        "{after}"
    );
    assert!(!after.contains("[fresh::"), "{after}");
}

// Picks: `-p all` links every row as Next with mints, stamps, and links;
// `-p none` writes nothing.
#[test]
fn km_picks_all_and_none() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-picks");
    let json =
        capture_json(&vault, &day_file, &["-p", "all"], "!sase_badges:t");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    assert_eq!(moving.len(), 3);
    for row in moving {
        assert_eq!(row["offer"]["selected"], true);
        assert!(row.get("not_linked").is_none() || row["not_linked"].is_null());
        let link = row.get("link").expect("picked link");
        assert_eq!(link["entry_name"], "FIX");
    }
    // Picks raise Ready to Next; Pending and Next stay. (KM1)
    let note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    assert!(
        note.contains("- [/] #task Ship the badge renderer"),
        "{note}"
    );
    assert!(note.contains("- [*] #task Write badge docs"), "{note}");
    assert!(note.contains("[fresh:: 2026-10-05]"), "{note}");
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(day.contains("sase_badges#^p"), "{day}");
    assert_eq!(json["pull_in"]["picked_count"], 3);
    assert_eq!(
        json["day_file"],
        serde_json::json!(day_file.display().to_string())
    );

    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-none");
    let json =
        capture_json(&vault, &day_file, &["-p", "none"], "!sase_badges:t");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    assert_eq!(moving.len(), 3);
    for row in moving {
        assert_eq!(row["offer"]["selected"], false);
        assert_eq!(row["not_linked"], "not_selected");
    }
    let note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    assert!(!note.contains("[fresh::"), "{note}");
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(!day.contains("sase_badges#^p"), "{day}");
}

// Number and stable-key picks share one number space with unblocked rows.
#[test]
fn km_number_and_key_picks() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-number");
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let first_n = dry["task_complete"]["keep_moving"][0]["offer"]["n"]
        .as_u64()
        .expect("n");
    let first_key = dry["task_complete"]["keep_moving"][0]["offer"]["key"]
        .as_str()
        .expect("key")
        .to_string();
    // Numeric pick maps through the probe to the same key.
    let pick = first_n.to_string();
    let by_number =
        capture_json(&vault, &day_file, &["-p", &pick], "!sase_badges:t");
    let picked: Vec<_> = by_number["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving")
        .iter()
        .filter(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(picked.len(), 1, "{picked:?}");
    assert_eq!(picked[0]["offer"]["key"], first_key);

    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-key");
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let key = dry["task_complete"]["keep_moving"][1]["offer"]["key"]
        .as_str()
        .expect("key")
        .to_string();
    let by_key =
        capture_json(&vault, &day_file, &["-p", &key], "!sase_badges:t");
    let picked: Vec<_> = by_key["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving")
        .iter()
        .filter(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0]["offer"]["key"], key);
    // Stale keys refuse.
    let output = capture_output(
        &vault,
        &day_file,
        &["-p", "sase_badges.md#^no-such-id"],
        "!sase_badges:t",
        false,
    );
    assert!(!output.status.success());
}

// Folded counts: blocked, scheduled-later, and hidden residents fold by
// group and project instead of offering. (KM13, KM17)
#[test]
fn km_folds_blocked_scheduled_hidden() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km-folds",
        concat!(
            "- [?] #task Needs dep [dependsOn:: p] [id:: dep] ^dep\n",
            "- [ ] #task Future work [scheduled:: 2026-10-06] [id:: fut] ^fut\n",
            "- [ ] #task Hidden gem #hide [id:: hid] ^hid\n",
        ),
    );
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    // Only P, B, A are selectable; the three extra residents fold.
    assert_eq!(moving.len(), 3, "{moving:?}");
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    let fold = groups
        .iter()
        .find(|group| group["group"] == "keep_moving")
        .expect("keep-moving group");
    assert_eq!(fold["project"], "sase_badges.md");
    // Blocked counts the open-prerequisite row; hidden counts #hide;
    // scheduled counts the future row. (KM17)
    assert_eq!(fold["folded"]["blocked"], 1, "{fold}");
    assert_eq!(fold["folded"]["scheduled"], 1, "{fold}");
    assert_eq!(fold["folded"]["hidden"], 1, "{fold}");
}

// No-candidate silence: closing the last task with no residents left
// reports an empty keep-moving array and no group. (KM13)
#[test]
fn km_no_candidates_is_silent() {
    let (temp, vault, day_file) = vault("bob-cli-km-silent");
    write_file(&vault.join("dev.md"), &area());
    write_file(&vault.join("sase.md"), &top_project());
    // All remaining residents fold (blocked/hidden/future): no selectable
    // rows, so the group is omitted entirely. (KM13)
    write_file(
        &vault.join("sase_badges.md"),
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n- [ ] #task Only task [id:: t] ^t\n- [ ] #task #prj Badges ^prj\n- [?] #task Stuck [id:: stuck] ^stuck\n- [ ] #task Hidden gem #hide [id:: hid] ^hid\n- [ ] #task Future work [scheduled:: 2026-10-06] [id:: fut] ^fut\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    let _ = temp;
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert_eq!(json["task_complete"]["keep_moving"], serde_json::json!([]));
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
}

// Both kill switches suppress the group with an empty array. (KM19)
#[test]
fn km_kill_switches_suppress_offers() {
    for (key, value) in [
        ("plan.keep_moving", "false"),
        ("plan.link_unblocked", "false"),
    ] {
        let (_temp, vault, day_file) = leaf_vault("bob-cli-km-switch");
        let dir = vault.parent().expect("temp dir").to_path_buf();
        let config = dir.join("config.yml");
        let _ = (key, value);
        let setting = key.rsplit('.').next().unwrap_or(key);
        write_file(&config, &format!("plan:\n  {setting}: false\n"));
        let output = capture_output_with_config(
            &vault,
            &day_file,
            &["--dry-run"],
            "!sase_badges:t",
            false,
            Some(&config),
        );
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(
            json["task_complete"]["keep_moving"],
            serde_json::json!([]),
            "{key}"
        );
        assert_eq!(json["pull_in"]["offer_count"], 0, "{key}");
    }
}

// KM30: only keep-moving offers plus `--require-pull-in-choice` without
// `-p` is a write-free refusal listing them.
#[test]
fn km30_require_choice_refuses_write_free() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km30");
    let before = vault_bytes(&vault);
    let output = capture_output(
        &vault,
        &day_file,
        &["--require-pull-in-choice"],
        "!sase_badges:t",
        false,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "pull_in_choice_required");
    assert_eq!(json["pull_in"]["offer_count"], 3);
    let offers = json["offers"].as_array().expect("offers");
    assert_eq!(offers.len(), 3);
    assert!(offers
        .iter()
        .all(|row| row["offer"]["group"] == "keep_moving"));
    assert_eq!(vault_bytes(&vault), before);
}

// Repair 1: same-note dependent on completed T plus external q must not
// recover or link from an incomplete graph. D carries
// `[dependsOn:: t,q]` with q open in q.md; the poisoned snapshot makes the
// dependents data unavailable. Old code offered D as a legacy hand-off
// while hiding its row. New code suppresses all legacy writes while KM31
// keep-moving rows survive: A offers and applies, D folds blocked.
#[test]
fn km31_same_note_dependent_suppressed_while_safe_rows_offer() {
    let (_t1, v1, d1) = vault("bob-cli-km31-legacy");
    write_file(&v1.join("dev.md"), &area());
    write_file(&v1.join("sase.md"), &top_project());
    let badges_before = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [ ] #task Draft the badge spec [id:: t] ^t\n",
        "- [ ] #task #prj Badges ^prj\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
        "- [?] #task Needs both [dependsOn:: t,q] [id:: d] ^d\n",
    );
    write_file(&v1.join("sase_badges.md"), badges_before);
    write_file(
        &v1.join("q.md"),
        "- [ ] #task External prereq [id:: q] ^q\n",
    );
    write_file(
        &v1.join("other.md"),
        "- [?] #task Outside follows [dependsOn:: t] [id:: f] ^f\n",
    );
    write_file(
        &d1,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    poison_snapshot(&v1);
    let d_line = "- [?] #task Needs both [dependsOn:: t,q] [id:: d] ^d";
    let dry = capture_json(&v1, &d1, &["--dry-run"], "!sase_badges:t");
    assert_eq!(dry["task_complete"]["unblocked_check"], "unavailable");
    assert_eq!(dry["task_complete"]["unblocked"], serde_json::json!([]));
    assert_eq!(dry["task_complete"]["still_blocked"], serde_json::json!([]));
    let moving = dry["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    assert_eq!(moving.len(), 1, "{moving:?}");
    assert_eq!(moving[0]["block_id"], "a");
    let groups = dry["pull_in"]["groups"].as_array().expect("groups");
    let fold = groups
        .iter()
        .find(|group| group["group"] == "keep_moving")
        .expect("keep-moving group");
    assert_eq!(fold["folded"]["blocked"], 1, "{fold}");
    assert!(!dry.to_string().contains("\"block_id\":\"d\""), "{dry}");
    assert!(!dry.to_string().contains("\"block_id\":\"f\""), "{dry}");
    let real = capture_json(&v1, &d1, &[], "!sase_badges:t");
    assert_eq!(real["task_complete"]["unblocked_check"], "unavailable");
    assert_eq!(
        real["task_complete"]["keep_moving"],
        dry["task_complete"]["keep_moving"]
    );
    let note = fs::read_to_string(v1.join("sase_badges.md")).unwrap();
    assert!(note.contains(d_line), "{note}");
    assert!(!note.contains("[fresh::"), "{note}");
    let day = fs::read_to_string(&d1).unwrap();
    assert!(!day.contains("sase_badges#^d"), "{day}");
    assert!(!day.contains("other#^f"), "{day}");
    let (_t2, v2, d2) = vault("bob-cli-km31-pick");
    write_file(&v2.join("dev.md"), &area());
    write_file(&v2.join("sase.md"), &top_project());
    write_file(&v2.join("sase_badges.md"), badges_before);
    write_file(
        &v2.join("q.md"),
        "- [ ] #task External prereq [id:: q] ^q\n",
    );
    write_file(
        &v2.join("other.md"),
        "- [?] #task Outside follows [dependsOn:: t] [id:: f] ^f\n",
    );
    write_file(
        &d2,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    poison_snapshot(&v2);
    let dry_all =
        capture_json(&v2, &d2, &["--dry-run", "-p", "all"], "!sase_badges:t");
    assert_eq!(dry_all["task_complete"]["unblocked_check"], "unavailable");
    let real_all = capture_json(&v2, &d2, &["-p", "all"], "!sase_badges:t");
    assert_eq!(
        dry_all["task_complete"]["keep_moving"],
        real_all["task_complete"]["keep_moving"]
    );
    assert_eq!(dry_all["pull_in"], real_all["pull_in"]);
    let note2 = fs::read_to_string(v2.join("sase_badges.md")).unwrap();
    assert!(note2.contains(d_line), "{note2}");
    assert!(note2.contains("- [*] #task Write badge docs"), "{note2}");
    let day2 = fs::read_to_string(&d2).unwrap();
    assert!(day2.contains("sase_badges#^a"), "{day2}");
    assert!(!day2.contains("sase_badges#^d"), "{day2}");
    let (_t3, v3, d3) = vault("bob-cli-km31-pomo");
    write_file(&v3.join("dev.md"), &area());
    write_file(&v3.join("sase.md"), &top_project());
    write_file(&v3.join("sase_badges.md"), badges_before);
    write_file(
        &v3.join("q.md"),
        "- [ ] #task External prereq [id:: q] ^q\n",
    );
    write_file(
        &d3,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase_badges#^t]]\n- [ ] () \u{2014} FIX\n",
    );
    poison_snapshot(&v3);
    let pomo = capture_json(&v3, &d3, &["--dry-run"], "x!");
    assert_eq!(pomo["pomodoro_close"]["unblocked_check"], "unavailable");
    assert_eq!(pomo["pomodoro_close"]["unblocked"], serde_json::json!([]));
    assert_eq!(
        pomo["pomodoro_close"]["still_blocked"],
        serde_json::json!([])
    );
    let (_t4, v4, d4) = vault("bob-cli-km31-pullin");
    write_file(&v4.join("dev.md"), &area());
    write_file(
        &v4.join("solo.md"),
        "---\ntype: [[project]]\nparent: [[dev]]\nstatus: wip\n---\n- [ ] #task Solo work [id:: t] ^t\n- [ ] #task #prj Solo ^prj\n",
    );
    write_file(
        &d4,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[solo#^t]]\n",
    );
    poison_snapshot(&v4);
    let solo = capture_json(&v4, &d4, &["--dry-run"], "!solo:t");
    assert_eq!(solo["task_complete"]["unblocked_check"], "unavailable");
    assert_eq!(solo["task_complete"]["unblocked"], serde_json::json!([]));
    assert_eq!(
        solo["task_complete"]["still_blocked"],
        serde_json::json!([])
    );
    assert_eq!(solo["task_complete"]["keep_moving"], serde_json::json!([]));
}

// KM31: a marker-positive invalid-UTF-8 note makes the dependents
// snapshot unavailable; safe rows still offer while the
// dependency-bearing candidate folds blocked.
#[test]
fn km31_unavailable_snapshot_keeps_safe_rows() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km31",
        "- [?] #task Needs dep [dependsOn:: p] [id:: dep] ^dep\n",
    );
    // Poison note: contains `dependsOn` but is not valid UTF-8, so the
    // prefiltered snapshot records it as failed without file-permission
    // tricks. It is never staged, so completeness stays unavailable.
    fs::write(
        vault.join("poison.md"),
        [
            b"- [?] #task Poison [dependsOn:: p] [id:: q]\n".as_slice(),
            &[0xff],
        ]
        .concat(),
    )
    .expect("write poison");
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert_eq!(json["task_complete"]["unblocked_check"], "unavailable");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    // P, B, A stay selectable; the dependsOn row folds blocked. (KM31)
    assert_eq!(moving.len(), 3, "{moving:?}");
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    let fold = groups
        .iter()
        .find(|group| group["group"] == "keep_moving")
        .expect("keep-moving group");
    assert_eq!(fold["folded"]["blocked"], 1, "{fold}");
}

// Dry-run leaves the fixture unchanged and agrees with the real run for
// the same flags.
#[test]
fn km_dry_run_matches_real_run() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-dry");
    let before = vault_bytes(&vault);
    let dry = capture_json(
        &vault,
        &day_file,
        &["--dry-run", "-p", "all"],
        "!sase_badges:t",
    );
    assert_eq!(vault_bytes(&vault), before);
    let real =
        capture_json(&vault, &day_file, &["-p", "all"], "!sase_badges:t");
    assert_eq!(
        dry["task_complete"]["keep_moving"],
        real["task_complete"]["keep_moving"]
    );
    assert_eq!(dry["pull_in"], real["pull_in"]);
}

// Pomodoro completion: `x!` closes the running entry and completes T;
// keep-moving offers land in the closing continuation. (KM11)
#[test]
fn km_pomodoro_close_offers_with_closing_destination() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-pomo");
    // Replace the FIX placeholder with a running BOB entry holding T.
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase_badges#^t]]\n- [ ] () \u{2014} FIX\n",
    );
    let json = capture_json(&vault, &day_file, &["--dry-run"], "x!");
    let close = &json["pomodoro_close"];
    assert_eq!(close["unblocked_check"], "checked");
    let moving = close["keep_moving"].as_array().expect("keep_moving");
    assert_eq!(moving.len(), 3, "{moving:?}");
    for row in moving {
        assert_eq!(row["trigger"], "last_planned");
        assert_eq!(row["offer"]["group"], "keep_moving");
        // Closing anchors report `closing`, or `created` when the close
        // minted its continuation placeholder. (KM11)
        let role = row["offer"]["destination"]["entry_role"]
            .as_str()
            .unwrap_or_default();
        assert!(role == "closing" || role == "created", "{row}");
    }
    assert_eq!(json["pull_in"]["offer_count"], 3);
}

// Retargeted close: T linked under MEET offers into MEET's slot.
#[test]
fn km_retargeted_slot_follows_link_entry() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-retarget");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} MEET\n  - [[sase_badges#^t]]\n- [ ] () \u{2014} FIX\n",
    );
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    assert_eq!(moving.len(), 3);
    for row in moving {
        assert_eq!(row["offer"]["destination"]["entry_name"], "MEET");
        assert_eq!(row["offer"]["destination"]["entry_role"], "slot");
    }
}

// Nested complete labels carry the separate keep-moving array.
#[test]
fn km_nested_complete_labels_carry_keep_moving_array() {
    // A relation draft that completes nothing still reports nested
    // summaries with the additive array present (possibly empty).
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-nested");
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    // Whole-item shape is already pinned above; nested summaries share
    // the contract when they appear.
    assert!(json["task_complete"].get("keep_moving").is_some());
    if let Some(completes) =
        json.get("relation_completes").and_then(|v| v.as_array())
    {
        for complete in completes {
            assert!(complete["task_complete"].get("keep_moving").is_some());
        }
    }
}

// Mixed groups: a direct dependent stays in `hand_off` while leaf
// residents offer as keep-moving after it. (dedup: hand_off wins)
#[test]
fn km_mixed_hand_off_orders_first() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-mixed");
    write_file(
        &vault.join("other.md"),
        "- [?] #task Follows badges [dependsOn:: t] [id:: f] ^f\n",
    );
    // T needs an [id::] for the dependent to match; badges already has
    // [id:: t], and the other note carries the dependent edge.
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let unblocked = json["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    let moving = json["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    assert_eq!(unblocked.len(), 1, "{unblocked:?}");
    assert_eq!(unblocked[0]["offer"]["group"], "hand_off");
    assert_eq!(moving.len(), 3);
    // Draft numbering puts keep-moving after hand-off.
    let hand_n = unblocked[0]["offer"]["n"].as_u64().expect("n");
    let moving_ns: Vec<u64> = moving
        .iter()
        .map(|row| row["offer"]["n"].as_u64().expect("n"))
        .collect();
    assert!(
        moving_ns.iter().all(|n| *n > hand_n),
        "{hand_n} {moving_ns:?}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 4);
}

// ---------------------------------------------------------------------
// Finish phase: net batch semantics (KM21)
// ---------------------------------------------------------------------

/// Blocked-aware Tasks settings so dependency drafts (`^note:id &note:id`)
/// can re-block a candidate.
fn blocked_leaf_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let (temp, vault, day_file) = leaf_vault(name);
    write_blocked_tasks_settings(&vault);
    (temp, vault, day_file)
}

/// The keep-moving block ids of capture `index`, ordered by offer number.
fn moving_ids(json: &serde_json::Value, index: usize) -> Vec<String> {
    let mut rows: Vec<&serde_json::Value> = json["captures"][index]
        ["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving array")
        .iter()
        .collect();
    rows.sort_by_key(|row| row["offer"]["n"].as_u64().unwrap_or(0));
    rows.iter()
        .map(|row| row["block_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn keep_moving_reason_lines(json: &serde_json::Value) -> Vec<String> {
    json["pomodoro_blocks"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .flat_map(|block| {
                    block["lines"].as_array().cloned().unwrap_or_default()
                })
                .filter(|line| line["reason"] == "keep_moving")
                .map(|line| {
                    line["text"].as_str().unwrap_or_default().trim().to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Real and dry runs of one draft agree on the net result.
fn assert_net_dry_matches_real(
    name: &str,
    setup: impl Fn(&str) -> (TempDir, PathBuf, PathBuf),
    flags: &[&str],
    draft: &str,
) -> serde_json::Value {
    let (_temp, vault, day_file) = setup(&format!("{name}-dry"));
    let before = vault_bytes(&vault);
    let mut dry_flags = vec!["--dry-run"];
    dry_flags.extend_from_slice(flags);
    let dry = capture_json(&vault, &day_file, &dry_flags, draft);
    assert_eq!(vault_bytes(&vault), before, "dry run writes nothing");
    let (_temp2, vault2, day_file2) = setup(&format!("{name}-real"));
    let real = capture_json(&vault2, &day_file2, flags, draft);
    for index in 0..dry["captures"].as_array().map_or(0, Vec::len) {
        assert_eq!(
            dry["captures"][index]["task_complete"]["keep_moving"],
            real["captures"][index]["task_complete"]["keep_moving"],
            "capture {index} keep_moving"
        );
    }
    assert_eq!(dry["pull_in"], real["pull_in"]);
    assert_eq!(
        keep_moving_reason_lines(&dry),
        keep_moving_reason_lines(&real)
    );
    real
}

// KM21: `!N:t` then a later item linking unpicked candidate A drops N's
// whole group; the later link itself is untouched and nothing else is
// written for the dropped offers.
#[test]
fn km21_later_link_of_candidate_drops_group() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-link",
        leaf_vault,
        &["-p", "none"],
        "!sase_badges:t\n\n@sase_badges:a",
    );
    assert_eq!(moving_ids(&json, 0), Vec::<String>::new(), "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_link");
}

// KM21: the dropped group is gone from files too: later link kept, no
// stamps or links for the unpicked rows.
#[test]
fn km21_later_link_keeps_only_the_explicit_link_on_disk() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km21-disk");
    let json = capture_json(
        &vault,
        &day_file,
        &["-p", "none"],
        "!sase_badges:t\n\n@sase_badges:a",
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
    let day = fs::read_to_string(&day_file).unwrap();
    assert_eq!(day.matches("sase_badges#^a").count(), 1, "{day}");
    assert!(!day.contains("sase_badges#^b"), "{day}");
    assert!(!day.contains("sase_badges#^p"), "{day}");
    let note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    // The explicit link raises A to Next; B and P are untouched.
    assert!(note.contains("- [*] #task Write badge docs"), "{note}");
    assert!(
        note.contains("- [*] #task Wire badges into the README [id:: b] ^b"),
        "{note}"
    );
    assert!(!note.contains("[fresh:: 2026-10-05] [id:: b]"), "{note}");
}

// KM21: a later item completing B drops only B's row; remaining numbers
// close up and a numeric pick addresses the survivors.
#[test]
fn km21_later_completion_drops_only_that_row() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-complete",
        leaf_vault,
        &["-p", "none"],
        "!sase_badges:t\n\n!sase_badges:b",
    );
    assert_eq!(moving_ids(&json, 0), vec!["p", "a"], "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    let ns: Vec<u64> = json["captures"][0]["task_complete"]["keep_moving"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["offer"]["n"].as_u64().unwrap())
        .collect();
    let mut sorted = ns.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![1, 2], "no gap left by the dropped row");

    // `-p 2` addresses the second survivor (A), not the dropped B.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km21-complete-num");
    let picked = capture_json(
        &vault,
        &day_file,
        &["-p", "2"],
        "!sase_badges:t\n\n!sase_badges:b",
    );
    let rows = picked["captures"][0]["task_complete"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    let selected: Vec<_> = rows
        .iter()
        .filter(|row| row["offer"]["selected"] == true)
        .map(|row| row["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(selected, vec!["a"], "{rows:?}");
    assert_eq!(picked["pull_in"]["picked_count"], 1);
    let day = fs::read_to_string(&day_file).unwrap();
    assert!(day.contains("sase_badges#^a"), "{day}");
    assert!(!day.contains("sase_badges#^p"), "{day}");
}

// KM21: picks the originating close linked survive several-at-once, and
// a later explicit link of an already-picked row changes nothing.
#[test]
fn km21_picks_survive_a_later_link_of_a_picked_row() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-picked",
        leaf_vault,
        &["-p", "all"],
        "!sase_badges:t\n\n@sase_badges:a",
    );
    assert_eq!(moving_ids(&json, 0), vec!["p", "b", "a"], "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 3);
    assert_eq!(json["pull_in"]["offer_count"], 3);
    for row in json["captures"][0]["task_complete"]["keep_moving"]
        .as_array()
        .unwrap()
    {
        assert!(row.get("link").is_some_and(|link| !link.is_null()));
    }
    // Link reasons stay `keep_moving` for every surviving pick.
    let reasons = keep_moving_reason_lines(&json);
    assert_eq!(reasons.len(), 3, "{reasons:?}");
}

// KM21: a later completion of one picked row drops just that row; the
// other picks keep their rows and link reasons.
#[test]
fn km21_later_completion_of_a_picked_row_keeps_other_picks() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-picked-complete",
        leaf_vault,
        &["-p", "all"],
        "!sase_badges:t\n\n!sase_badges:a",
    );
    assert_eq!(moving_ids(&json, 0), vec!["p", "b"], "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
    assert_eq!(json["pull_in"]["picked_count"], 2);
    let reasons = keep_moving_reason_lines(&json);
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(
        reasons.iter().all(|line| !line.contains("^a")),
        "{reasons:?}"
    );
}

// KM21: a later item linking a DIFFERENT open resident drops the whole
// group, picks included; the picks' task edits and links stay written.
#[test]
fn km21_later_link_of_other_resident_drops_picked_group() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km21-other");
    let json = capture_json(
        &vault,
        &day_file,
        &["-p", "all"],
        "!sase_badges:t\n\n@sase_badges:prj",
    );
    assert_eq!(moving_ids(&json, 0), Vec::<String>::new(), "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert_eq!(json["pull_in"]["picked_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
    assert!(keep_moving_reason_lines(&json).is_empty(), "{json}");
    // Net reporting never undoes the originating close's legitimate
    // edits: the picks stay linked and stamped.
    let day = fs::read_to_string(&day_file).unwrap();
    for id in ["a", "b", "p", "prj"] {
        assert!(day.contains(&format!("sase_badges#^{id}")), "{id}: {day}");
    }
    let note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    assert!(note.contains("[fresh:: 2026-10-05]"), "{note}");
}

// KM21: a later close of N's `^prj` drops the group.
#[test]
fn km21_later_close_of_prj_drops_group() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-close-prj",
        leaf_vault,
        &["-p", "none"],
        "!sase_badges:t\n\n!sase_badges:prj",
    );
    assert_eq!(moving_ids(&json, 0), Vec::<String>::new(), "{json}");
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
}

// KM21: a later item that re-blocks a candidate drops only that row.
#[test]
fn km21_later_reblock_drops_only_that_row() {
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-reblock",
        blocked_leaf_vault,
        &["-p", "none"],
        "!sase_badges:t\n\n^sase_badges:a &sase_badges:p",
    );
    assert_eq!(moving_ids(&json, 0), vec!["p", "b"], "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
}

// KM21 + the folds: dropping every surviving row also drops the group's
// folded counts, so no stale `groups` entry remains.
#[test]
fn km21_all_rows_dropped_leaves_no_stale_fold_counts() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km21-folds",
        "- [?] #task Stuck [id:: stuck] ^stuck\n",
    );
    let json = capture_json(
        &vault,
        &day_file,
        &["--dry-run"],
        "!sase_badges:t\n\n!sase_badges:p\n\n!sase_badges:b\n\n!sase_badges:a",
    );
    assert_eq!(moving_ids(&json, 0), Vec::<String>::new(), "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
}

// KM21 + KM22: a later link in one project drops only that project's
// group; the other leaf project's group survives untouched.
#[test]
fn km21_multiple_project_groups_drop_independently() {
    let setup = |name: &str| {
        let (temp, vault, day_file) = leaf_vault(name);
        write_file(
            &vault.join("sase_docs.md"),
            concat!(
                "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
                "- [ ] #task Outline docs [id:: u] ^u\n",
                "- [ ] #task #prj Docs ^prj\n",
                "- [ ] #task Write intro [id:: d1] ^d1\n",
                "- [ ] #task Write guide [id:: d2] ^d2\n",
            ),
        );
        write_file(
            &day_file,
            concat!(
                "## Pomodoros\n- [ ] () \u{2014} FIX\n",
                "  - [[sase_badges#^t]]\n  - [[sase_docs#^u]]\n"
            ),
        );
        (temp, vault, day_file)
    };
    let json = assert_net_dry_matches_real(
        "bob-cli-km21-two",
        setup,
        &["-p", "none"],
        "!sase_badges:t\n\n!sase_docs:u\n\n@sase_docs:d1",
    );
    assert_eq!(moving_ids(&json, 0), vec!["p", "b", "a"], "{json}");
    assert_eq!(moving_ids(&json, 1), Vec::<String>::new(), "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 3);
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["project"], "sase_badges.md");
}

// KM22: one close draining two leaf projects reports two groups, ordered
// by anchor then path, both in the one keep-moving array.
#[test]
fn km22_two_projects_two_groups_in_order() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km22");
    write_file(
        &vault.join("sase_docs.md"),
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [ ] #task Outline docs [id:: u] ^u\n",
            "- [ ] #task #prj Docs ^prj\n",
            "- [ ] #task Write intro [id:: d1] ^d1\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n",
            "  - [[sase_badges#^t]]\n  - [[sase_docs#^u]]\n",
            "- [ ] () \u{2014} FIX\n"
        ),
    );
    let json = capture_json(&vault, &day_file, &["--dry-run"], "x!");
    let rows = json["pomodoro_close"]["keep_moving"]
        .as_array()
        .expect("keep_moving");
    let mut ordered: Vec<(u64, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row["offer"]["n"].as_u64().unwrap(),
                row["offer"]["project"].as_str().unwrap().to_string(),
                row["block_id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    ordered.sort();
    let projects: Vec<&str> = ordered
        .iter()
        .map(|(_, project, _)| project.as_str())
        .collect();
    assert_eq!(
        projects,
        vec![
            "sase_badges.md",
            "sase_badges.md",
            "sase_badges.md",
            "sase_docs.md"
        ],
        "{ordered:?}"
    );
    assert_eq!(json["pull_in"]["groups"].as_array().unwrap().len(), 2);
}

// KM21 through a nested complete label: the relation's own summary
// carries the group, and a later link in the same draft drops it.
#[test]
fn km21_nested_complete_label_group_and_net_drop() {
    let setup = |name: &str| {
        let (temp, vault, day_file) = blocked_leaf_vault(name);
        write_file(
            &vault.join("other.md"),
            "- [ ] #task Implement badges [id:: x] ^x\n",
        );
        (temp, vault, day_file)
    };
    let config = |vault: &Path| {
        let path = vault.parent().unwrap().join("relations.yml");
        write_file(
            &path,
            "relations:\n  complete_labels:\n    - implemented by\n",
        );
        path
    };
    let (_temp, vault, day_file) = setup("bob-cli-km21-nested");
    let config_path = config(&vault);
    let output = capture_output_with_config(
        &vault,
        &day_file,
        &["--dry-run"],
        "@sase_badges+t ~other:x|\"implemented by\"",
        false,
        Some(&config_path),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let nested = json["relation_completes"][0]["task_complete"]["keep_moving"]
        .as_array()
        .expect("nested keep_moving");
    assert_eq!(nested.len(), 3, "{nested:?}");
    assert_eq!(json["pull_in"]["offer_count"], 3);
    assert_eq!(json["pull_in"]["groups"][0]["group"], "keep_moving");

    // A later item linking a resident of N drops the nested group too.
    let (_temp2, vault2, day_file2) = setup("bob-cli-km21-nested-net");
    let config_path2 = config(&vault2);
    let output = capture_output_with_config(
        &vault2,
        &day_file2,
        &["--dry-run"],
        "@sase_badges+t ~other:x|\"implemented by\"\n\n@sase_badges:a",
        false,
        Some(&config_path2),
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let nested = json["captures"][0]["relation_completes"][0]["task_complete"]
        ["keep_moving"]
        .as_array()
        .expect("nested keep_moving");
    assert!(nested.is_empty(), "{nested:?}");
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
}

// The note-less `@file+id` multi-note form runs the same closing-label
// engine as a single note: the completed owner carries its keep-moving
// group, and a later link in the draft nets it away.
#[test]
fn km21_multi_note_complete_label_matches_the_single_note_group() {
    let setup = |name: &str| {
        let (temp, vault, day_file) = blocked_leaf_vault(name);
        write_file(
            &vault.join("other.md"),
            "- [ ] #task Implement badges [id:: x] ^x\n",
        );
        let config = vault.parent().unwrap().join("relations.yml");
        write_file(
            &config,
            "relations:\n  complete_labels:\n    - implemented by\n",
        );
        (temp, vault, day_file, config)
    };
    let dry = |vault: &Path, day_file: &Path, config: &Path, text: &str| {
        let output = capture_output_with_config(
            vault,
            day_file,
            &["--dry-run"],
            text,
            false,
            Some(config),
        );
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("json")
    };

    let (_temp, vault, day_file, config) = setup("bob-cli-km21-multi-note");
    let single = dry(
        &vault,
        &day_file,
        &config,
        // One note plus a nested bullet adds the same two lines under the
        // task as two notes, so later rows report identical line numbers.
        "@sase_badges+t one note ~other:x|\"implemented by\"\n- nested",
    );
    let multi = dry(
        &vault,
        &day_file,
        &config,
        "@sase_badges+t ~other:x|\"implemented by\"\n- first\n- second",
    );
    let nested = multi["relation_completes"][0]["task_complete"]["keep_moving"]
        .as_array()
        .expect("nested keep_moving");
    assert_eq!(nested.len(), 3, "{nested:?}");
    assert_eq!(
        multi["relation_completes"][0]["task_complete"],
        single["relation_completes"][0]["task_complete"],
        "multi-note completion must equal the single-note completion"
    );
    assert_eq!(multi["pull_in"], single["pull_in"]);
    assert_eq!(multi["pull_in"]["offer_count"], 3);
    assert_eq!(multi["pull_in"]["groups"][0]["group"], "keep_moving");
    assert_eq!(multi["parent_status_symbol"], "x", "{multi}");
    assert_eq!(multi["parent_status_name"], "Done", "{multi}");
    assert_eq!(multi["notes"].as_array().expect("notes").len(), 2);

    // A later item linking a resident drops the nested group too.
    let (_temp2, vault2, day_file2, config2) =
        setup("bob-cli-km21-multi-note-net");
    let netted = dry(
        &vault2,
        &day_file2,
        &config2,
        "@sase_badges+t ~other:x|\"implemented by\"\n- first\n- second\n\n@sase_badges:a",
    );
    let nested = netted["captures"][0]["relation_completes"][0]
        ["task_complete"]["keep_moving"]
        .as_array()
        .expect("nested keep_moving");
    assert!(nested.is_empty(), "{nested:?}");
    assert_eq!(netted["pull_in"]["offer_count"], 0);
    assert!(netted["pull_in"].get("groups").is_none(), "{netted}");
}

// ---------------------------------------------------------------------
// Finish phase: approved human copy (§12.15)
// ---------------------------------------------------------------------

fn human_stdout(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
) -> String {
    let output = capture_output(vault, day_file, flags, text, true);
    assert_success(&output);
    stdout(&output)
}

#[test]
fn human_dry_run_leads_with_why_and_folds() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km-human-dry",
        concat!(
            "- [?] #task Needs dep [dependsOn:: p] [id:: dep] ^dep\n",
            "- [?] #task Stuck [id:: stuck] ^stuck\n",
            "- [ ] #task Future work [scheduled:: 2026-10-06] [id:: fut] ^fut\n",
            "- [ ] #task Hidden gem #hide [id:: hid] ^hid\n",
        ),
    );
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let lines: Vec<&str> = out.lines().collect();
    let header = lines
        .iter()
        .position(|line| {
            *line == "  Keep sase_badges moving \u{b7} Draft the badge spec was its last task planned today (ready tasks, not dependents)"
        })
        .unwrap_or_else(|| panic!("header missing:\n{out}"));
    assert_eq!(
        lines[header + 1],
        "  offer 1  Pending Ship the badge renderer  sase_badges.md ^p \u{2192} FIX"
    );
    assert_eq!(
        lines[header + 2],
        "  offer 2  Next Wire badges into the README  sase_badges.md ^b \u{2192} FIX"
    );
    assert_eq!(
        lines[header + 3],
        "  offer 3  Ready Write badge docs  sase_badges.md ^a \u{2192} FIX"
    );
    assert_eq!(
        lines[header + 4],
        "  not offered  2 blocked \u{b7} 1 scheduled later \u{b7} 1 hidden"
    );
    assert_eq!(lines[header + 5], "  -p 1,2,3 \u{b7} -p all");
    // The group never borrows dependency wording or the unlock glyph.
    assert!(!out.contains("unblocked"), "{out}");
    assert!(!out.contains('\u{1f513}'), "{out}");
    // Keep-moving rows use lane names, never raw marks (ordinary completion
    // keeps its own `[ ] → [x]` formatting).
    for line in &lines[header + 1..=header + 3] {
        assert!(!line.contains("[/]"), "{line}");
        assert!(!line.contains("[*]"), "{line}");
        assert!(!line.contains("[ ]"), "{line}");
    }
    assert!(out.contains("Pending"), "{out}");
    assert!(out.contains("Next"), "{out}");
    assert!(out.contains("Ready"), "{out}");
}

#[test]
fn human_fold_row_omits_zero_parts_and_empty_groups() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km-human-fold",
        "- [ ] #task Hidden gem #hide [id:: hid] ^hid\n",
    );
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert!(out.contains("  not offered  1 hidden\n"), "{out}");
    assert!(!out.contains("blocked"), "{out}");
    assert!(!out.contains("scheduled later"), "{out}");

    // No fold, no `not offered` row at all.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-human-nofold");
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert!(!out.contains("not offered"), "{out}");

    // Nothing selectable: no header and no fold-only group.
    let (_temp, vault, day_file) = self::vault("bob-cli-km-human-silent");
    write_file(&vault.join("dev.md"), &area());
    write_file(&vault.join("sase.md"), &top_project());
    write_file(
        &vault.join("sase_badges.md"),
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n- [ ] #task Only task [id:: t] ^t\n- [ ] #task #prj Badges ^prj\n- [?] #task Stuck [id:: stuck] ^stuck\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert!(!out.contains("Keep "), "{out}");
    assert!(!out.contains("not offered"), "{out}");
}

#[test]
fn human_pick_suffix_and_not_pulled_in() {
    let (_temp, vault, day_file) = leaf_vault_extra(
        "bob-cli-km-human-pick",
        "- [ ] #task Write the changelog entry\n",
    );
    let out = human_stdout(&vault, &day_file, &["-p", "all"], "!sase_badges:t");
    assert!(out.starts_with("\u{2713} completed"), "{out}");
    assert!(
        out.contains(
            "linked Ready \u{2192} Next Write badge docs  sase_badges.md ^a \u{2192} FIX (next up) \u{b7} keeps sase_badges moving"
        ),
        "{out}"
    );
    // A minted ID is reported after the suffix.
    assert!(
        out.contains("keeps sase_badges moving \u{b7} added ^write-changelog"),
        "{out}"
    );
    assert!(!out.contains("unblocked"), "{out}");
    // Keep-moving picked rows use lane names; ordinary completion keeps its
    // own `[ ] → [x]` mark on the first line.
    for line in out.lines().filter(|line| line.contains("keeps ")) {
        assert!(!line.contains("[ ]"), "{line}");
        assert!(!line.contains("[*]"), "{line}");
        assert!(!line.contains("[/]"), "{line}");
    }
    assert!(out.contains("Ready"), "{out}");
    assert!(out.contains("Next"), "{out}");

    // A partial pick marks the rest as not pulled in; an explicit none
    // writes no task fields and prints no link.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-human-none");
    let before_note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    let out =
        human_stdout(&vault, &day_file, &["-p", "none"], "!sase_badges:t");
    assert!(
        out.contains("sase_badges.md ^p \u{2192} FIX \u{b7} not pulled in"),
        "{out}"
    );
    assert!(!out.contains("linked"), "{out}");
    let note = fs::read_to_string(vault.join("sase_badges.md")).unwrap();
    assert_eq!(
        note,
        before_note.replacen(
            "- [ ] #task Draft the badge spec [id:: t] ^t",
            "- [x] #task Draft the badge spec [id:: t]  [completion:: 2026-10-05] ^t",
            1
        )
    );
}

#[test]
fn human_pomodoro_close_and_caption_for_several_predecessors() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-human-pomo");
    write_file(
        &vault.join("sase_badges.md"),
        &badges(
            "- [ ] #task Review the badge spec with the whole design team at the offsite [id:: t2] ^t2\n",
        ),
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n",
            "  - [[sase_badges#^t]]\n  - [[sase_badges#^t2]]\n",
            "- [ ] () \u{2014} FIX\n"
        ),
    );
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "x!");
    assert!(
        out.contains(
            "  Keep sase_badges moving \u{b7} Draft the badge spec and 1 more were its last tasks planned today (ready tasks, not dependents)\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "  offer 1  Pending Ship the badge renderer  sase_badges.md ^p"
        ),
        "{out}"
    );
    assert!(!out.contains("unblocked"), "{out}");
    // Keep-moving offer lines use lane names; ordinary close keeps its marks.
    for line in out.lines().filter(|line| line.contains("offer 1")) {
        assert!(!line.contains("[/]"), "{line}");
        assert!(!line.contains("[*]"), "{line}");
        assert!(!line.contains("[ ]"), "{line}");
    }
}

#[test]
fn human_caption_truncates_the_predecessor_to_48_characters() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-human-trunc");
    write_file(
        &vault.join("sase_badges.md"),
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [ ] #task Review the badge spec with the whole design team at the offsite [id:: t] ^t\n",
            "- [ ] #task #prj Badges ^prj\n",
            "- [ ] #task Write badge docs [id:: a] ^a\n",
        ),
    );
    let out = human_stdout(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let truncated = "Review the badge spec with the whole design tea\u{2026}";
    assert_eq!(truncated.chars().count(), 48);
    assert!(
        out.contains(&format!(
            "  Keep sase_badges moving \u{b7} {truncated} was its last task planned today"
        )),
        "{out}"
    );
}

#[test]
fn human_mixed_hand_off_refusal_never_labels_keep_moving_unblocked() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km-human-mixed");
    write_file(
        &vault.join("other.md"),
        "- [?] #task Follows badges [dependsOn:: t] [id:: f] ^f\n",
    );
    let before = vault_bytes(&vault);
    let output = capture_output(
        &vault,
        &day_file,
        &["--require-pull-in-choice"],
        "!sase_badges:t",
        true,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(vault_bytes(&vault), before, "refusal is write-free");
    let out = stdout(&output);
    let hand_offs = out.find("  Hand-offs\n").expect("hand-off group");
    let keep = out
        .find("  Keep sase_badges moving \u{b7} Draft the badge spec was its last task planned today (ready tasks, not dependents)\n")
        .expect("keep-moving group");
    assert!(hand_offs < keep, "{out}");
    // Keep-moving rows (offer lines under the Keep header) use lane names,
    // never raw marks or unblocked wording. Ordinary completion and legacy
    // hand-off rows keep their own formatting.
    let keep_section = &out[keep..];
    for line in keep_section.lines().filter(|line| line.contains("offer")) {
        assert!(!line.contains("unblocked"), "{line}");
        assert!(!line.contains("[/]"), "{line}");
        assert!(!line.contains("[*]"), "{line}");
        assert!(!line.contains("[ ]"), "{line}");
    }
    // Keep-moving rows carry lane names.
    assert!(keep_section.contains("Pending"), "{out}");
    assert!(keep_section.contains("Next"), "{out}");
    assert!(keep_section.contains("Ready"), "{out}");
    assert!(
        out.contains(
            "offer 1  unblocked [?] \u{2192} [ ] Follows badges  other.md ^f"
        ),
        "{out}"
    );
    assert!(!out.contains('\u{1f513}'), "{out}");
}

// ---------------------------------------------------------------------
// Finish phase: gates (KM32) and the cycler's re-plan equivalent (KM24)
// ---------------------------------------------------------------------

/// A marker-positive invalid-UTF-8 note makes any dependents snapshot
/// unavailable, so a run that reports `unblocked_check: "checked"` never
/// tried to build one.
fn poison_snapshot(vault: &Path) {
    fs::write(
        vault.join("poison.md"),
        [
            b"- [?] #task Poison [dependsOn:: p] [id:: q]\n".as_slice(),
            &[0xff],
        ]
        .concat(),
    )
    .expect("write poison");
}

// KM32 / G1-G3: a close outside any project note adds no keep-moving
// work and builds no snapshot.
#[test]
fn km32_close_outside_a_project_note_does_no_added_work() {
    let (_temp, vault, day_file) = vault("bob-cli-km32-plain");
    write_file(&vault.join("scratch.md"), "- [ ] #task Plain chore ^c\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[scratch#^c]]\n",
    );
    poison_snapshot(&vault);
    let json = capture_json(&vault, &day_file, &["--dry-run"], "!scratch:c");
    assert_eq!(json["task_complete"]["keep_moving"], serde_json::json!([]));
    assert_eq!(json["task_complete"]["unblocked_check"], "checked");
}

// G3: the baseline-day byte check rejects an unlinked project close
// before any snapshot is built; the same close linked today does build it.
#[test]
fn km32_unlinked_project_close_builds_no_snapshot() {
    let idless = concat!(
        "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
        "- [ ] #task Draft the badge spec ^t\n",
        "- [ ] #task #prj Badges ^prj\n",
        "- [ ] #task Write badge docs [id:: a] ^a\n",
    );
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km32-unlinked");
    write_file(&vault.join("sase_badges.md"), idless);
    write_file(&day_file, "## Pomodoros\n- [ ] () \u{2014} FIX\n");
    poison_snapshot(&vault);
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert_eq!(json["task_complete"]["keep_moving"], serde_json::json!([]));
    assert_eq!(
        json["task_complete"]["unblocked_check"], "checked",
        "G3 rejected before any snapshot work: {json}"
    );

    // A triggering close with no `[dependsOn::]` in the project has no
    // prerequisite to prove closed, so it builds no snapshot either and
    // still offers its safe row.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km32-linked");
    write_file(&vault.join("sase_badges.md"), idless);
    poison_snapshot(&vault);
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert_eq!(json["task_complete"]["unblocked_check"], "checked");
    assert_eq!(
        json["task_complete"]["keep_moving"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Control: a candidate that carries `[dependsOn::]` needs the proof,
    // so the poisoned vault makes the snapshot unavailable (KM31): the
    // dependency-bearing row folds and the safe row still offers.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km32-proof");
    write_file(
        &vault.join("sase_badges.md"),
        &format!(
            "{idless}- [ ] #task Needs proof [dependsOn:: z] [id:: pr] ^pr\n"
        ),
    );
    poison_snapshot(&vault);
    let json =
        capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    assert_eq!(json["task_complete"]["unblocked_check"], "unavailable");
    assert_eq!(
        json["task_complete"]["keep_moving"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// Both kill switches gate the trigger itself, so a disabled group builds
// no snapshot and reads no dependency data (KM19 + KM32).
#[test]
fn km19_kill_switch_adds_no_snapshot_work() {
    for setting in ["keep_moving", "link_unblocked"] {
        let (_temp, vault, day_file) = leaf_vault("bob-cli-km19-nowork");
        // An id-less predecessor, so only keep-moving could want a snapshot.
        write_file(
            &vault.join("sase_badges.md"),
            concat!(
                "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
                "- [ ] #task Draft the badge spec ^t\n",
                "- [ ] #task #prj Badges ^prj\n",
                "- [ ] #task Needs proof [dependsOn:: z] [id:: pr] ^pr\n",
            ),
        );
        poison_snapshot(&vault);
        let config = vault.parent().expect("temp dir").join("config.yml");
        write_file(&config, &format!("plan:\n  {setting}: false\n"));
        let output = capture_output_with_config(
            &vault,
            &day_file,
            &["--dry-run"],
            "!sase_badges:t",
            false,
            Some(&config),
        );
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(
            json["task_complete"]["unblocked_check"], "checked",
            "{setting}: {json}"
        );
        assert_eq!(
            json["task_complete"]["keep_moving"],
            serde_json::json!([]),
            "{setting}"
        );
    }
}

// KM24 (Rust equivalent): a candidate that became Blocked or linked before
// apply is planned fresh, so its old pick key is stale and refused with
// the vault untouched.
#[test]
fn km24_stale_pick_keys_refuse_after_a_replan() {
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km24-blocked");
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let key_a = dry["task_complete"]["keep_moving"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["block_id"] == "a")
        .expect("row a")["offer"]["key"]
        .as_str()
        .unwrap()
        .to_string();
    // A became Blocked after the picker rendered.
    let note = vault.join("sase_badges.md");
    let text = fs::read_to_string(&note).unwrap().replace(
        "- [ ] #task Write badge docs [id:: a] ^a",
        "- [?] #task Write badge docs [dependsOn:: p] [id:: a] ^a",
    );
    write_file(&note, &text);
    let before = vault_bytes(&vault);
    let output = capture_output(
        &vault,
        &day_file,
        &["-p", &key_a],
        "!sase_badges:t",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(vault_bytes(&vault), before, "stale pick writes nothing");

    // A linked before apply: another planned N task suppresses the group
    // (K4), so every old key is stale.
    let (_temp, vault, day_file) = leaf_vault("bob-cli-km24-linked");
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase_badges:t");
    let key_b = dry["task_complete"]["keep_moving"][0]["offer"]["key"]
        .as_str()
        .unwrap()
        .to_string();
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n  - [[sase_badges#^a]]\n",
    );
    let before = vault_bytes(&vault);
    let output = capture_output(
        &vault,
        &day_file,
        &["-p", &key_b],
        "!sase_badges:t",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(vault_bytes(&vault), before, "stale pick writes nothing");
}
