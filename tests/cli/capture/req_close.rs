//! Public requirement-close CLI (`docs/task-dependencies.md` §12.5):
//! `-k/--carry`, `-x`/`-X`/`-p`/`-j` by row kind, the dual-gate
//! handshake, receipts, and legacy-client fallback.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Output;

const NOW: &str = "2026-10-05 09:30:00";
const RELATION_NOW: &str = "2026-10-14 10:20:00";
const RELATION_CONFIG: &str = concat!(
    "relations:\n",
    "  cancel_labels:\n",
    "    - duplicate of\n",
    "  complete_labels:\n",
    "    - implemented by\n",
);

fn vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

fn ship_note() -> &'static str {
    concat!(
        "- [ ] #task Ship parser [id:: ship] ^ship\n",
        "\t- [ ] Handles empty input\n",
        "\t- [ ] Handles unicode\n",
    )
}

fn ship_vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("sase.md"), ship_note());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^ship]]\n",
    );
    (temp, vault, day_file)
}

fn mixed_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] #task Ship parser [id:: ship] ^ship\n",
            "\t- [ ] Handles empty input\n",
        ),
    );
    write_file(
        &vault.join("deps.md"),
        "- [?] #task Follow up [dependsOn:: ship] [id:: follow] ^follow\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^ship]]\n",
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
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
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

fn capture_json_failing(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
) -> (Output, serde_json::Value) {
    let output = capture_output(vault, day_file, flags, text, false);
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], false);
    (output, json)
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir()
                && path.file_name().is_some_and(|name| name == ".obsidian")
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string();
                entries.push((
                    relative,
                    fs::read(&path).expect("read vault file"),
                ));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn req_groups<'a>(json: &'a serde_json::Value) -> Vec<&'a serde_json::Value> {
    json["pull_in"]["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|group| group["group"] == "requirements")
        .collect()
}

fn req_rows<'a>(json: &'a serde_json::Value) -> Vec<&'a serde_json::Value> {
    req_groups(json)
        .into_iter()
        .flat_map(|group| {
            group["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        })
        .collect()
}

fn read_sase(vault: &Path) -> String {
    fs::read_to_string(vault.join("sase.md")).expect("sase.md")
}

#[test]
fn default_met_closes_owner_and_leaves_req_bytes() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-met");
    let json = capture_json(&vault, &day_file, &[], "!sase:ship");
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 2, "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2, "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 0, "{json}");
    let groups = req_groups(&json);
    assert_eq!(groups.len(), 1, "{json}");
    assert_eq!(groups[0]["kind"], "requirements");
    assert_eq!(groups[0]["owner"]["text"], "Ship parser");
    assert_eq!(
        groups[0]["copy"]["hint"],
        "-k N own task · -X N[=why] drop · -p N pull in"
    );
    let rows = req_rows(&json);
    assert_eq!(rows.len(), 2, "{json}");
    assert_eq!(rows[0]["offer"]["n"], 1);
    assert_eq!(rows[0]["req"]["text"], "Handles empty input");
    assert_eq!(rows[1]["offer"]["n"], 2);
    assert_eq!(rows[1]["req"]["text"], "Handles unicode");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    assert_eq!(outcomes.len(), 1, "{json}");
    assert_eq!(outcomes[0]["verdict"], "met");
    assert_eq!(
        outcomes[0]["message"], "✓ 2 requirements met with “Ship parser”",
        "{json}"
    );
    let sase = read_sase(&vault);
    assert!(sase.contains("- [x] #task Ship parser"), "{sase}");
    assert!(sase.contains("[completion:: 2026-10-05]"), "{sase}");
    assert!(sase.contains("- [ ] Handles empty input"), "{sase}");
    assert!(sase.contains("- [ ] Handles unicode"), "{sase}");
    assert!(!sase.contains("[-]"), "{sase}");
    assert!(!sase.contains("[>]"), "{sase}");
}

#[test]
fn explicit_met_acknowledges_pending_reqs() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-x");
    let json = capture_json(&vault, &day_file, &["-x", "1"], "!sase:ship");
    assert_eq!(json["pull_in"]["choice"], "none", "{json}");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    assert!(outcomes.iter().any(|row| row["verdict"] == "met"), "{json}");
    let sase = read_sase(&vault);
    assert!(sase.contains("- [x] #task Ship parser"), "{sase}");
    assert!(sase.contains("- [ ] Handles empty input"), "{sase}");
    assert!(!sase.contains("[-]"), "{sase}");
}

#[test]
fn drop_rewrites_req_and_optional_cancel_log() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-drop");
    let json = capture_json(
        &vault,
        &day_file,
        &["-X", "1=duplicate effort"],
        "!sase:ship",
    );
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    let drop = outcomes
        .iter()
        .find(|row| row["verdict"] == "drop")
        .expect("drop outcome");
    assert_eq!(drop["n"], 1);
    assert_eq!(drop["text"], "Handles empty input");
    assert_eq!(drop["reason"], "duplicate effort");
    assert_eq!(
        drop["message"], "⊘ Dropped “Handles empty input” · “duplicate effort”",
        "{json}"
    );
    let sase = read_sase(&vault);
    assert!(sase.contains("[-] Handles empty input"), "{sase}");
    assert!(sase.contains("❌ **CANCEL LOG**"), "{sase}");
    assert!(sase.contains("*2026-10-05* — duplicate effort"), "{sase}");
    assert!(sase.contains("- [ ] Handles unicode"), "{sase}");
}

#[test]
fn carry_creates_own_task_without_pulling_in() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-carry");
    let json = capture_json(&vault, &day_file, &["-k", "1"], "!sase:ship");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    let carry = outcomes
        .iter()
        .find(|row| row["verdict"] == "carry")
        .expect("carry outcome");
    assert_eq!(carry["pulled"], false);
    assert_eq!(
        carry["message"], "→ “Handles empty input” is its own task now",
        "{json}"
    );
    assert!(carry.get("carried").is_some(), "{carry}");
    assert!(!carry["carried"]["block_id"]
        .as_str()
        .unwrap_or("")
        .is_empty());
    let sase = read_sase(&vault);
    assert!(sase.contains("[>]"), "{sase}");
    assert!(sase.contains("#task Handles empty input"), "{sase}");
    assert!(!sase.contains("[fresh::"), "{sase}");
    assert!(!sase.contains("[unblocked::"), "{sase}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(
        !day.contains("Handles empty input"),
        "carry without pull-in stays unlinked:\n{day}"
    );
}

#[test]
fn pull_in_on_req_is_carry_pulled() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-pulled");
    let json = capture_json(&vault, &day_file, &["-p", "1"], "!sase:ship");
    assert_eq!(json["pull_in"]["choice"], "subset", "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 1, "{json}");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    let pulled = outcomes
        .iter()
        .find(|row| row["verdict"] == "carry_pulled")
        .expect("carry_pulled outcome");
    assert_eq!(pulled["pulled"], true);
    assert_eq!(
        pulled["message"],
        "→ “Handles empty input” is its own task now · pulled in",
        "{json}"
    );
    let sase = read_sase(&vault);
    assert!(sase.contains("[>]"), "{sase}");
    assert!(
        sase.contains("- [*] #task Handles empty input")
            || sase.contains("[*] #task Handles empty input"),
        "{sase}"
    );
    assert!(!sase.contains("[fresh::"), "{sase}");
    assert!(!sase.contains("[unblocked::"), "{sase}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(
        day.contains("Handles empty input") || day.contains("[[sase#^"),
        "carry-pulled links the new task:\n{day}"
    );
}

#[test]
fn mark_all_never_selects_requirement_rows() {
    let (_temp, vault, day_file) = mixed_vault("bob-cli-req-close-all");
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!sase:ship");
    assert_eq!(json["pull_in"]["choice"], "all", "{json}");
    let rows = req_rows(&json);
    assert_eq!(rows.len(), 1, "{json}");
    assert_eq!(rows[0]["offer"]["n"], 1);
    assert_eq!(rows[0]["offer"]["selected"], false);
    let follow = json
        .pointer("/task_complete/unblocked")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .find(|row| row["block_id"] == "follow")
        .expect("follow offer");
    assert_eq!(follow["offer"]["n"], 2, "{json}");
    assert_eq!(follow["offer"]["selected"], true, "{json}");
    let sase = read_sase(&vault);
    assert!(sase.contains("- [ ] Handles empty input"), "{sase}");
    assert!(!sase.contains("[>]"), "{sase}");
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    assert!(deps.contains("- [*] #task Follow up"), "{deps}");
}

#[test]
fn carry_plus_pull_in_on_same_req_conflicts_write_free() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-conflict");
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_failing(
        &vault,
        &day_file,
        &["-k", "1", "-p", "1"],
        "!sase:ship",
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "offer_verdict_conflict", "{json}");
    assert_eq!(vault_bytes(&vault), before, "conflict writes nothing");
}

#[test]
fn dual_gate_pending_reqs_demand_acknowledgement() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-choice");
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_failing(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "--req-choice"],
        "!sase:ship",
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "pull_in_choice_required", "{json}");
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 2, "{json}");
    assert_eq!(vault_bytes(&vault), before);
}

#[test]
fn dual_gate_none_or_explicit_verdict_acknowledges() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-choice-none");
    let json = capture_json(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "--req-choice", "-p", "none"],
        "!sase:ship",
    );
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["pull_in"]["choice"], "none", "{json}");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    assert_eq!(outcomes[0]["verdict"], "met");

    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-choice-x");
    let json = capture_json(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "--req-choice", "-x", "1"],
        "!sase:ship",
    );
    assert_eq!(json["ok"], true, "{json}");
}

#[test]
fn dual_gate_jump_alone_is_not_acknowledgement() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-jump");
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_failing(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "--req-choice", "-j", "1"],
        "!sase:ship",
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "pull_in_choice_required", "{json}");
    assert_eq!(vault_bytes(&vault), before);
}

#[test]
fn legacy_client_keeps_offer_numbering_and_met_default() {
    let (_temp, vault, day_file) = mixed_vault("bob-cli-req-close-legacy");
    let json = capture_json(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "-p", "1"],
        "!sase:ship",
    );
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 1, "{json}");
    let groups = req_groups(&json);
    assert!(
        groups.is_empty(),
        "legacy clients must not see requirement groups: {json}"
    );
    let follow = json
        .pointer("/task_complete/unblocked")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .find(|row| row["block_id"] == "follow")
        .expect("follow offer");
    assert_eq!(follow["offer"]["n"], 1, "{json}");
    assert_eq!(follow["offer"]["selected"], true, "{json}");
    let sase = read_sase(&vault);
    assert!(sase.contains("- [ ] Handles empty input"), "{sase}");
    assert!(!sase.contains("[-]"), "{sase}");
    assert!(!sase.contains("[>]"), "{sase}");
}

#[test]
fn mixed_card_numbers_requirements_first() {
    let (_temp, vault, day_file) = mixed_vault("bob-cli-req-close-mixed");
    let json = capture_json(&vault, &day_file, &["--dry-run"], "!sase:ship");
    let rows = req_rows(&json);
    assert_eq!(rows[0]["offer"]["n"], 1, "{json}");
    let follow = json
        .pointer("/task_complete/unblocked")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .find(|row| row["block_id"] == "follow")
        .expect("follow offer");
    assert_eq!(follow["offer"]["n"], 2, "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2, "{json}");
}

#[test]
fn keyed_and_numbered_flags_select_the_same_row() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-keyed");
    let preview = capture_json(&vault, &day_file, &["--dry-run"], "!sase:ship");
    let key = preview["pull_in"]["groups"][0]["rows"][0]["offer"]["key"]
        .as_str()
        .expect("req key")
        .to_string();
    let numbered = capture_json(
        &vault,
        &day_file,
        &["--dry-run", "-k", "1"],
        "!sase:ship",
    );
    let keyed = capture_json(
        &vault,
        &day_file,
        &["--dry-run", "-k", &key],
        "!sase:ship",
    );
    let numbered_carry = numbered["req_outcomes"]
        .as_array()
        .expect("numbered outcomes")
        .iter()
        .find(|row| row["verdict"] == "carry")
        .expect("numbered carry");
    let keyed_carry = keyed["req_outcomes"]
        .as_array()
        .expect("keyed outcomes")
        .iter()
        .find(|row| row["verdict"] == "carry")
        .expect("keyed carry");
    assert_eq!(numbered_carry["key"], keyed_carry["key"], "{numbered}");
}

#[test]
fn stale_keyed_replay_in_a_second_process_refuses() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-stale");
    let preview = capture_json(&vault, &day_file, &["--dry-run"], "!sase:ship");
    let key = preview["pull_in"]["groups"][0]["rows"][0]["offer"]["key"]
        .as_str()
        .expect("req key")
        .to_string();
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] #task Ship parser [id:: ship] ^ship\n",
            "\t- [ ] Handles empty input now\n",
            "\t- [ ] Handles unicode\n",
        ),
    );
    let before = vault_bytes(&vault);
    let (output, json) =
        capture_json_failing(&vault, &day_file, &["-k", &key], "!sase:ship");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "req_close_stale", "{json}");
    assert!(json.get("current").is_some(), "{json}");
    assert_eq!(vault_bytes(&vault), before);
}

#[test]
fn dry_run_uses_would_forms_and_writes_nothing() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-dry");
    let before = vault_bytes(&vault);
    let dry = capture_json(
        &vault,
        &day_file,
        &["--dry-run", "-X", "1"],
        "!sase:ship",
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(vault_bytes(&vault), before, "dry run writes nothing");
    let drop = dry["req_outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .find(|row| row["verdict"] == "drop")
        .expect("drop");
    assert_eq!(drop["message"], "would drop “Handles empty input”", "{dry}");
    let met = dry["req_outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .find(|row| row["verdict"] == "met")
        .expect("met");
    assert!(
        met["message"]
            .as_str()
            .unwrap_or("")
            .starts_with("would meet"),
        "{dry}"
    );
}

#[test]
fn no_req_task_advertises_zero_pending() {
    let (_temp, vault, day_file) = vault("bob-cli-req-close-none");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n");
    let json = capture_json(&vault, &day_file, &[], "!sase:bar");
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 0, "{json}");
    assert!(req_groups(&json).is_empty(), "{json}");
    assert!(
        json.get("req_outcomes").is_none()
            || json["req_outcomes"]
                .as_array()
                .is_some_and(|rows| rows.is_empty()),
        "{json}"
    );
}

#[test]
fn offer_close_keeps_ordinary_numbering_when_owner_has_no_reqs() {
    let (_temp, vault, day_file) = vault("bob-cli-req-close-offer");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n",
            "- [?] #task Beta [dependsOn:: bar] [id:: beta] ^beta\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let json = capture_json(&vault, &day_file, &["-X", "1"], "!sase:bar");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 0, "{json}");
    assert_eq!(json["offer_closes"][0]["outcome"], "cancelled", "{json}");
    assert_eq!(json["offer_closes"][0]["n"], 1);
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    assert!(deps.contains("- [-] #task Alpha"), "{deps}");
}

#[test]
fn multi_owner_done_reports_each_forest() {
    let (_temp, vault, day_file) = vault("bob-cli-req-close-owners");
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] #task Ship parser [id:: ship] ^ship\n",
            "\t- [ ] Handles empty input\n",
            "- [ ] #task Other [id:: other] ^other\n",
            "\t- [ ] Other req\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n");
    let json =
        capture_json(&vault, &day_file, &[], "!sase:ship\n\n!sase:other");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 2, "{json}");
    assert_eq!(req_groups(&json).len(), 2, "{json}");
    let outcomes = json["req_outcomes"].as_array().expect("req_outcomes");
    assert_eq!(outcomes.len(), 2, "{json}");
}

#[test]
fn pomodoro_close_audits_linked_task_reqs() {
    let (_temp, vault, day_file) = vault("bob-cli-req-close-pom");
    write_file(&vault.join("sase.md"), ship_note());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n  - [[sase#^ship]]\n- [ ] () — FIX\n",
    );
    let json = capture_json(&vault, &day_file, &[], "x!");
    assert_eq!(json["kind"], "pomodoro_close", "{json}");
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 2, "{json}");
    assert_eq!(req_groups(&json).len(), 1, "{json}");
    let sase = read_sase(&vault);
    assert!(sase.contains("- [x] #task Ship parser"), "{sase}");
    assert!(sase.contains("- [ ] Handles empty input"), "{sase}");
}

#[test]
fn relation_complete_audits_source_reqs() {
    let temp = TempDir::new("bob-cli-req-close-rel");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Unrelated placeholder\n");
    let config = vault.join("config.yml");
    write_file(&config, RELATION_CONFIG);
    write_file(
        &vault.join("a.md"),
        concat!(
            "- [ ] #task Alpha ^alpha\n",
            "\t- [ ] Handles empty input\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("@a+alpha ~b:beta|\"implemented by\"")
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", RELATION_NOW)
            .env("BOB_CONFIG_FILE", &config);
        command.output().expect("run relation complete")
    };
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["pull_in"]["reqs"]["supported"], true, "{json}");
    assert_eq!(json["pull_in"]["reqs"]["pending"], 1, "{json}");
    assert_eq!(req_groups(&json).len(), 1, "{json}");
    let a = fs::read_to_string(vault.join("a.md")).expect("a.md");
    assert!(a.contains("- [x] #task Alpha"), "{a}");
    assert!(a.contains("- [ ] Handles empty input"), "{a}");
}

#[test]
fn human_preview_prints_requirements_first() {
    let (_temp, vault, day_file) = ship_vault("bob-cli-req-close-human");
    let output = capture_output(&vault, &day_file, &[], "!sase:ship", true);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Requirements · “Ship parser”"), "{text}");
    assert!(
        text.contains("-k N own task · -X N[=why] drop · -p N pull in"),
        "{text}"
    );
    assert!(text.contains("Handles empty input"), "{text}");
    let req_at = text.find("Handles empty input").expect("req row");
    let met_at = text
        .find("requirements met with “Ship parser”")
        .expect("met receipt");
    assert!(req_at < met_at, "rows print before receipts:\n{text}");
}
