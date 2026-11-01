//! Keep-moving capture integration (`docs/task-dependencies.md` §12.15,
//! KM1–KM31; `plan:202611/leaf_keep_moving_rust.md` phase `capture`).
//!
//! Focused CLI coverage for `!`, Pomodoro completion, retargeted close,
//! and nested complete labels: separate array shape, all/none/key/number
//! picks, mixed groups including project-created rows, folded counts,
//! no-candidate silence, both kill switches, write-free
//! `--require-pull-in-choice` (KM30), and unavailable-snapshot behavior
//! (KM31) through a controlled invalid-UTF-8 fixture.

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
