//! Capture-side successor jump selection and final-text landing contract
//! (`docs/task-dependencies.md` §12.24, JS1–JS10).

use crate::support::*;
use std::{fs, path::Path, process::Output};

const NOW: &str = "2026-10-05 09:30:00";

fn vault(
    name: &str,
    dependencies: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(&vault.join("deps.md"), dependencies);
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    (temp, vault, day)
}

fn one_offer_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    vault(
        name,
        "- [?] #task Alpha [dependsOn:: bar] [id:: alpha]\n- [?] #task Beta [dependsOn:: bar] [id:: beta]\n",
    )
}

fn capture_output(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
    human: bool,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture")
}

fn capture_json(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> serde_json::Value {
    let output = capture_output(vault, day, flags, draft, dry_run, false);
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("capture JSON")
}

fn capture_json_error(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
) -> (Output, serde_json::Value) {
    let output = capture_output(vault, day, flags, draft, false, false);
    assert!(!output.status.success(), "{}", format_output(&output));
    let json = serde_json::from_slice(&output.stdout).expect("error JSON");
    (output, json)
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![vault.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read vault") {
            let entry = entry.expect("vault entry");
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name() != ".obsidian" {
                    pending.push(path);
                }
            } else if path.is_file() {
                files.push((
                    path.strip_prefix(vault)
                        .expect("vault-relative path")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).expect("read vault file"),
                ));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn landing_line(
    json: &serde_json::Value,
    vault: &Path,
) -> (usize, String, String) {
    let landing = &json["jump"]["landing"];
    assert!(landing.is_object(), "expected a landing: {json}");
    let relative = landing["relative_path"].as_str().expect("relative path");
    let line = landing["line"].as_u64().expect("landing line") as usize;
    let text = fs::read_to_string(vault.join(relative)).expect("landing note");
    let physical = text
        .lines()
        .nth(line - 1)
        .expect("landing physical line")
        .to_string();
    (line, text, physical)
}

fn remove_dry_run(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("dry_run");
            for value in object.values_mut() {
                remove_dry_run(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_dry_run(value);
            }
        }
        _ => {}
    }
}

// JS1: a recovering dependent can stay unselected while its final line is
// returned as the navigation target.
#[test]
fn jump_left_lands_on_final_recovery_line() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-left");
    let human = capture_output(
        &vault,
        &day,
        &["-p", "none", "-j", "2"],
        "!sase:bar",
        true,
        true,
    );
    assert_success(&human);
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(
        human.contains("↗ jump  “Beta”  deps.md:2"),
        "human jump receipt missing: {human}"
    );
    let json = capture_json(
        &vault,
        &day,
        &["-p", "none", "-j", "2"],
        "!sase:bar",
        false,
    );
    assert_eq!(json["jump"]["outcome"], "left");
    assert_eq!(json["jump"]["n"], 2);
    assert_eq!(json["jump"]["label"], "Beta");
    assert_eq!(json["jump"]["landing"]["relative_path"], "deps.md");
    let (_, _, physical) = landing_line(&json, &vault);
    assert!(physical.contains("Beta"), "{physical}");
    assert!(physical.contains("[id:: beta]"), "{physical}");
    assert_eq!(json["pull_in"]["jump"]["supported"], true);
}

// JS2: the picked offer's minted block ID is resolved from post-pull-in text.
#[test]
fn jump_pulled_in_lands_on_minted_block_id() {
    let (_temp, vault, day) = vault(
        "bob-cli-jump-picked",
        "- [?] #task Alpha [dependsOn:: bar] [id:: alpha]\n- [?] #task Beta [dependsOn:: bar] [id:: beta]\n",
    );
    let json =
        capture_json(&vault, &day, &["-p", "2", "-j", "2"], "!sase:bar", false);
    assert_eq!(json["jump"]["outcome"], "pulled_in");
    let (_, _, physical) = landing_line(&json, &vault);
    let block_id = json["jump"]["landing"]["block_id"]
        .as_str()
        .expect("minted block ID");
    assert!(!block_id.is_empty());
    assert!(physical.contains(&format!("^{block_id}")), "{physical}");
    assert!(physical.contains("Beta"), "{physical}");
}

// JS3: a verdict is still navigable and the landing uses the final [x] line.
#[test]
fn jump_done_lands_on_closed_line() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-done");
    let json =
        capture_json(&vault, &day, &["-x", "2", "-j", "2"], "!sase:bar", false);
    assert_eq!(json["jump"]["outcome"], "done");
    let (_, _, physical) = landing_line(&json, &vault);
    assert!(physical.starts_with("- [x]"), "{physical}");
    assert!(physical.contains("Beta"), "{physical}");
}

// JS4: an earlier Cancel Log insertion shifts the second close's final line.
#[test]
fn jump_cancel_lands_after_earlier_cancel_log_shift() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-cancel-shift");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n",
            "- [?] #task Beta [dependsOn:: bar] [id:: beta] ^beta\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day,
        &["-X", "1=why", "-X", "2", "-j", "2"],
        "!sase:bar",
        false,
    );
    assert_eq!(json["jump"]["outcome"], "cancelled");
    let close_line = json["offer_closes"][1]["line"]
        .as_u64()
        .expect("close line");
    let (landing, _, physical) = landing_line(&json, &vault);
    assert!(landing > close_line as usize, "line did not shift: {json}");
    assert!(physical.starts_with("- [-]"), "{physical}");
    assert!(physical.contains("Beta"), "{physical}");
}

// JS5: selecting an inbox resident and routing it makes the jump follow the
// task's destination note.
#[test]
fn jump_pulled_in_routed_offer_follows_move() {
    let temp = TempDir::new("bob-cli-jump-routed");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day = vault.join("20261005.md");
    write_file(&vault.join("inbox.md"), "---\ntype: [[inbox]]\n---\n");
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n",
    );
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n## Tasks\n",
            "- [?] #task Route me [dependsOn:: bar] [id:: mac_inbox__route-task] ^route-task\n",
        ),
    );
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let json = capture_json(
        &vault,
        &day,
        &["-p", "1", "-i", "1=health", "-j", "1"],
        "!sase:bar",
        false,
    );
    assert_eq!(json["jump"]["outcome"], "pulled_in");
    assert_eq!(json["jump"]["landing"]["relative_path"], "health.md");
    let (_, _, physical) = landing_line(&json, &vault);
    assert!(physical.contains("Route me"), "{physical}");
}

// JS6: an unknown number and stale key both refuse before any vault write.
#[test]
fn stale_jump_selectors_refuse_write_free_with_current_offers() {
    let (_temp, vault, day) = vault("bob-cli-jump-stale-number", "# Tasks\n");
    let before = vault_bytes(&vault);
    let (output, json) =
        capture_json_error(&vault, &day, &["-j", "9"], "New task");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json["code"], "offer_jump_stale");
    assert_eq!(json["offers"], serde_json::json!([]));
    assert_eq!(vault_bytes(&vault), before);

    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-stale-key");
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_error(
        &vault,
        &day,
        &["-j", "deps.md#^stale"],
        "!sase:bar",
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json["code"], "offer_jump_stale");
    assert_eq!(json["offers"].as_array().expect("offers").len(), 2);
    assert_eq!(json["pull_in"]["jump"]["supported"], true);
    assert_eq!(vault_bytes(&vault), before);
}

// JS7: the jump is assembled from the shared plan, so preview and real JSON
// agree exactly except for dry_run.
#[test]
fn jump_dry_run_matches_real_capture() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-dry-run");
    let dry =
        capture_json(&vault, &day, &["-p", "2", "-j", "2"], "!sase:bar", true);
    let real =
        capture_json(&vault, &day, &["-p", "2", "-j", "2"], "!sase:bar", false);
    let mut dry = dry;
    let mut real = real;
    remove_dry_run(&mut dry);
    remove_dry_run(&mut real);
    assert_eq!(dry, real);
}

// JS8: the jump selector is navigation only and does not satisfy the hidden
// required-pull-in-choice guard.
#[test]
fn jump_does_not_satisfy_required_pull_in_choice() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-not-a-choice");
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_error(
        &vault,
        &day,
        &["--require-pull-in-choice", "-j", "2"],
        "!sase:bar",
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["code"], "pull_in_choice_required");
    assert_eq!(vault_bytes(&vault), before);
}

// JS9: an ID-less note-pull row uses cleaned description plus its settled
// duplicate ordinal after freshness edits move or rewrite the line.
#[test]
fn jump_idless_note_pull_lands_by_cleaned_description() {
    let (_temp, vault, day) =
        vault("bob-cli-jump-idless-note-pull", "# unused\n");
    write_file(
        &vault.join("work.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n## Tasks\n",
            "- [ ] #task Write docs [type:: BUILD] [fresh:: 2026-10-01]\n",
        ),
    );
    let preview = capture_json(&vault, &day, &[], "@work", true);
    assert_eq!(preview["pull_in"]["jump"]["supported"], true);
    let key = preview["pull_in"]["groups"][0]["offers"][0]["key"]
        .as_str()
        .or_else(|| {
            preview["note_pull"]["successors"][0]["offer"]["key"].as_str()
        })
        .expect("ID-less offer key")
        .to_string();
    let json =
        capture_json(&vault, &day, &["-p", "none", "-j", &key], "@work", false);
    assert_eq!(json["jump"]["key"], key);
    assert_eq!(json["jump"]["outcome"], "left");
    let (_, _, physical) = landing_line(&json, &vault);
    assert!(physical.contains("Write docs"), "{physical}");
    assert!(
        physical.contains("[fresh::"),
        "freshness write should remain: {physical}"
    );
}

// JS10: every preview advertises jump support in the pull-in capability.
#[test]
fn previews_advertise_jump_capability() {
    let (_temp, vault, day) = one_offer_vault("bob-cli-jump-capability");
    let json = capture_json(&vault, &day, &[], "!sase:bar", true);
    assert_eq!(json["pull_in"]["jump"]["supported"], true);
}
