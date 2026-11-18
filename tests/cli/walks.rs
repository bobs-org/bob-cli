//! `bob walks`: s/w/b attention, conformance vectors, and goldens.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-11-17T12:00:00";
const DATE: &str = "2026-11-17";

const BANK_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    groceries: Groceries\n---\n\n# 🏦 Bank\n";

fn vault_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("vault")
}

fn empty_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    temp
}

fn write_tasks(vault: &Path, body: &str) {
    write_file(&vault.join("tasks.md"), body);
}

fn write_bank_month(vault: &Path, month: &str, body: &str) {
    write_file(&vault.join(format!("bank/{month}.md")), body);
}

fn walks_command(temp: &TempDir, args: &[&str]) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("walks")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1");
    for arg in args {
        command.arg(arg);
    }
    command
}

fn walks_json(temp: &TempDir) -> (std::process::Output, Value) {
    let output = walks_command(temp, &["-f", "json"])
        .output()
        .expect("run bob walks -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("walks JSON");
    (output, value)
}

fn family<'a>(value: &'a Value, letter: &str) -> &'a Value {
    value["walks"]
        .as_array()
        .expect("walks array")
        .iter()
        .find(|entry| entry["family"] == letter)
        .unwrap_or_else(|| panic!("missing family {letter}"))
}

fn check_golden(name: &str, value: &Value) {
    let pretty =
        format!("{}\n", serde_json::to_string_pretty(value).expect("pretty"));
    let golden = fixture(&format!("walks/{name}"));
    if std::env::var("BOB_UPDATE_GOLDENS").is_ok() {
        fs::create_dir_all(golden.parent().expect("golden parent"))
            .expect("create golden dir");
        fs::write(&golden, &pretty).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(&golden).unwrap_or_else(|error| {
        panic!("read committed golden {}: {error}", golden.display())
    });
    assert_eq!(pretty, expected, "golden {name} changed");
}

fn n_lines(prefix: &str, status: &str, count: usize, extra: &str) -> String {
    (1..=count)
        .map(|i| format!("- [{status}] #task {prefix} {i}{extra}\n"))
        .collect()
}

#[test]
fn json_reports_schema_and_family_order() {
    let temp = empty_vault("bob-cli-walks-empty");
    let (_, value) = walks_json(&temp);
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["date"], DATE);
    let walks = value["walks"].as_array().expect("walks");
    assert_eq!(walks.len(), 3);
    assert_eq!(walks[0]["family"], "s");
    assert_eq!(walks[0]["name"], "review");
    assert_eq!(walks[1]["family"], "w");
    assert_eq!(walks[1]["name"], "lanes");
    assert_eq!(walks[2]["family"], "b");
    assert_eq!(walks[2]["name"], "bank");
    assert_eq!(walks[0]["available"], true);
    assert_eq!(walks[0]["attention"], false);
    assert!(walks[0]["tier"].is_null());
    assert!(walks[0]["label"].is_null());
    assert_eq!(walks[0]["counts"]["walk"], 0);
    assert_eq!(walks[0]["counts"]["commitments"], 0);
    assert_eq!(walks[2]["available"], true);
    assert_eq!(walks[2]["counts"]["waiting"], 0);
    assert!(walks[2]["oldest_waiting"].is_null());
}

#[test]
fn s1_only_rotten_due_lights_rotten() {
    let temp = empty_vault("bob-cli-walks-s1");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task Rotten only [fresh:: 2026-10-01]\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "rotten");
    assert_eq!(s["counts"]["walk"], 1);
    assert_eq!(s["counts"]["commitments"], 0);
    assert_eq!(
        s["label"],
        "Review: 1 due · commitments done — ROTTEN first"
    );
}

#[test]
fn s2_only_post_due_lights_post() {
    let temp = empty_vault("bob-cli-walks-s2");
    write_tasks(
        &vault_dir(&temp),
        "- [?] #task #gtd #post Closeout [scheduled:: 2026-11-17]\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "post");
    assert_eq!(s["counts"]["walk"], 1);
    assert_eq!(s["counts"]["commitments"], 0);
    assert_eq!(s["label"], "Review: 1 due · commitments done — POST first");
}

#[test]
fn s3_new_and_rotten_lights_new() {
    let temp = empty_vault("bob-cli-walks-s3");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task New capture\n\
         - [ ] #task Rotten bread [fresh:: 2026-10-01]\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "new");
    assert_eq!(s["counts"]["walk"], 2);
    assert_eq!(s["counts"]["commitments"], 1);
    assert_eq!(s["label"], "Review: 2 due · 1 commitment — NEW first");
}

#[test]
fn s6_rotten_and_post_lights_rotten() {
    let temp = empty_vault("bob-cli-walks-s6");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task Rotten one [fresh:: 2026-10-01]\n\
         - [ ] #task Rotten two [fresh:: 2026-10-02]\n\
         - [?] #task #gtd #post Closeout [scheduled:: 2026-11-17]\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "rotten");
    assert_eq!(s["counts"]["walk"], 3);
    assert_eq!(s["counts"]["commitments"], 0);
    assert_eq!(
        s["label"],
        "Review: 3 due · commitments done — ROTTEN first"
    );
}

#[test]
fn s7_nothing_due_is_clear() {
    let temp = empty_vault("bob-cli-walks-s7");
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], false);
    assert!(s["tier"].is_null());
    assert!(s["label"].is_null());
    assert_eq!(s["counts"]["walk"], 0);
    assert_eq!(s["counts"]["commitments"], 0);
    let output = walks_command(&temp, &[]).output().expect("run bob walks");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("s · Review clear"), "{text}");
}

#[test]
fn s4_pre_beats_next() {
    let temp = empty_vault("bob-cli-walks-s4");
    write_tasks(
        &vault_dir(&temp),
        "- [?] #task #gtd #pre Morning [scheduled:: 2026-11-17]\n\
         - [*] #task Next one\n\
         - [*] #task Next two\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "pre");
    assert_eq!(s["counts"]["commitments"], 3);
    assert_eq!(s["label"], "Review: 3 due · 3 commitments — PRE first");
}

#[test]
fn s5_tickler_label_ends_ticks_first() {
    let temp = empty_vault("bob-cli-walks-s5");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task Deferred one [fresh:: 2026-11-10] [scheduled:: 2026-11-16]\n\
         - [ ] #task Deferred two [fresh:: 2026-11-10] [scheduled:: 2026-11-16]\n",
    );
    let (_, value) = walks_json(&temp);
    let s = family(&value, "s");
    assert_eq!(s["attention"], true);
    assert_eq!(s["tier"], "tickler");
    assert_eq!(s["counts"]["walk"], 2);
    let label = s["label"].as_str().expect("label");
    assert!(label.ends_with("TICKS first"), "tickler label: {label}");
}

#[test]
fn human_post_only_shows_post_bullet() {
    let temp = empty_vault("bob-cli-walks-human-post");
    write_tasks(
        &vault_dir(&temp),
        "- [?] #task #gtd #post Closeout [scheduled:: 2026-11-17]\n",
    );
    let output = walks_command(&temp, &[]).output().expect("run bob walks");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("s ● Review: 1 due"), "{text}");
    assert!(text.contains("POST first"), "{text}");
}

#[test]
fn w1_pending_over_cap_is_on() {
    let temp = empty_vault("bob-cli-walks-w1");
    write_tasks(
        &vault_dir(&temp),
        &n_lines("Pending", "/", 11, " [fresh:: 2026-11-17]"),
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["attention"], true);
    assert_eq!(w["tier"], "over");
    assert_eq!(w["label"], "Lanes over limit: WIP 11/10");
    assert_eq!(w["lanes"][0]["lane"], "pending");
    assert_eq!(w["lanes"][0]["count"], 11);
    assert_eq!(w["lanes"][0]["cap"], 10);
    assert_eq!(w["lanes"][0]["over"], true);
    assert_eq!(w["counts"]["excess"], 1);
}

#[test]
fn w2_pending_at_cap_is_off() {
    let temp = empty_vault("bob-cli-walks-w2");
    write_tasks(
        &vault_dir(&temp),
        &n_lines("Pending", "/", 10, " [fresh:: 2026-11-17]"),
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["attention"], false);
    assert_eq!(w["counts"]["excess"], 0);
    assert_eq!(w["lanes"][0]["count"], 10);
    assert_eq!(w["lanes"][0]["over"], false);
}

#[test]
fn w3_today_linked_pending_is_not_counted() {
    let temp = empty_vault("bob-cli-walks-w3");
    let vault = vault_dir(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [/] #task Linked pending [fresh:: 2026-11-17] ^today-pending\n\
         - [/] #task Other pending [fresh:: 2026-11-17]\n",
    );
    write_file(
        &vault.join("2026/20261117.md"),
        "# 2026-11-17\n\n## Pomodoros\n\n- [ ] (0900-0930) — REVIEW\n  - [[tasks#^today-pending]]\n",
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["lanes"][0]["count"], 1);
    assert_eq!(w["attention"], false);
    assert_eq!(w["counts"]["excess"], 0);
}

#[test]
fn w4_unstamped_ready_is_not_in_ready_lane() {
    let temp = empty_vault("bob-cli-walks-w4");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task New capture\n\
         - [ ] #task Fresh one [fresh:: 2026-11-15]\n",
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["lanes"][2]["lane"], "ready");
    assert_eq!(w["lanes"][2]["count"], 1);
}

#[test]
fn w5_rotten_ready_is_not_in_ready_lane() {
    let temp = empty_vault("bob-cli-walks-w5");
    write_tasks(
        &vault_dir(&temp),
        "- [ ] #task Rotten bread [fresh:: 2026-10-01]\n\
         - [ ] #task Fresh one [fresh:: 2026-11-15]\n",
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["lanes"][2]["count"], 1);
}

#[test]
fn w6_dash_md_rows_are_not_counted() {
    let temp = empty_vault("bob-cli-walks-w6");
    let vault = vault_dir(&temp);
    write_file(
        &vault.join("dash.md"),
        "- [/] #task Dash pending [fresh:: 2026-11-17]\n\
         - [*] #task Dash next\n\
         - [ ] #task Dash ready [fresh:: 2026-11-15]\n",
    );
    write_tasks(
        &vault,
        "- [/] #task Real pending [fresh:: 2026-11-17]\n\
         - [ ] #task Real ready [fresh:: 2026-11-15]\n",
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["lanes"][0]["count"], 1);
    assert_eq!(w["lanes"][1]["count"], 0);
    assert_eq!(w["lanes"][2]["count"], 1);
}

#[test]
fn w7_excluded_rows_are_not_counted() {
    let temp = empty_vault("bob-cli-walks-w7");
    let vault = vault_dir(&temp);
    write_tasks(
        &vault,
        "- [/] #task Counted pending [fresh:: 2026-11-17]\n\
         - [/] #task Hidden pending #hide [fresh:: 2026-11-17]\n\
         - [/] #task Future pending [fresh:: 2026-11-17] [scheduled:: 2026-12-01]\n\
         - [ ] #task Root [fresh:: 2026-11-15] [id:: block-root]\n\
         - [/] #task Blocked pending [fresh:: 2026-11-17] [dependsOn:: block-root]\n",
    );
    write_file(
        &vault.join("_templates/tpl.md"),
        "- [/] #task Templated pending [fresh:: 2026-11-17]\n",
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["lanes"][0]["count"], 1);
}

#[test]
fn w8_ready_over_cap_is_on() {
    let temp = empty_vault("bob-cli-walks-w8");
    write_tasks(
        &vault_dir(&temp),
        &n_lines("Ready", " ", 101, " [fresh:: 2026-11-15]"),
    );
    let (_, value) = walks_json(&temp);
    let w = family(&value, "w");
    assert_eq!(w["attention"], true);
    assert_eq!(w["label"], "Lanes over limit: READY 101/100");
    assert_eq!(w["lanes"][2]["count"], 101);
    assert_eq!(w["lanes"][2]["over"], true);
    assert_eq!(w["counts"]["excess"], 1);
}

#[test]
fn w9_excess_sums_every_over_lane() {
    let temp = empty_vault("bob-cli-walks-w9");
    let vault = vault_dir(&temp);
    write_tasks(
        &vault,
        &format!(
            "{}{}",
            n_lines("Pending", "/", 11, " [fresh:: 2026-11-17]"),
            n_lines("Next", "*", 22, ""),
        ),
    );
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_next: 20\n");
    let output = walks_command(&temp, &["-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run bob walks");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("walks JSON");
    let w = family(&value, "w");
    assert_eq!(w["attention"], true);
    assert_eq!(w["label"], "Lanes over limit: WIP 11/10 · NEXT 22/20");
    assert_eq!(w["lanes"][1]["cap"], 20);
    assert_eq!(w["counts"]["excess"], 3);
}

#[test]
fn b1_open_checkbox_is_waiting() {
    let temp = empty_vault("bob-cli-walks-b1");
    let vault = vault_dir(&temp);
    write_file(&vault.join("bank.md"), BANK_HUB);
    write_bank_month(
        &vault,
        "2026-11",
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\n---\n\n# 🏦 November 2026\n\n## Tue Nov 17\n\n- [ ] $10.00 Groceries [cat:: groceries]\n",
    );
    let (_, value) = walks_json(&temp);
    let b = family(&value, "b");
    assert_eq!(b["available"], true, "b1 family: {b}");
    assert_eq!(b["attention"], true, "b1 family: {b}");
    assert_eq!(b["tier"], "waiting");
    assert_eq!(b["counts"]["waiting"], 1);
    assert_eq!(b["oldest_waiting"], "2026-11-17");
    assert_eq!(b["label"], "Bank: 1 waiting · oldest Tue Nov 17");
}

#[test]
fn b2_acknowledged_boxes_are_not_waiting() {
    let temp = empty_vault("bob-cli-walks-b2");
    let vault = vault_dir(&temp);
    write_file(&vault.join("bank.md"), BANK_HUB);
    write_bank_month(
        &vault,
        "2026-11",
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\n---\n\n# 🏦 November 2026\n\n## Tue Nov 17\n\n- [x] $10.00 Groceries [cat:: groceries]\n- [X] $4.00 Coffee [cat:: groceries]\n",
    );
    let (_, value) = walks_json(&temp);
    let b = family(&value, "b");
    assert_eq!(b["attention"], false, "b2 family: {b}");
    assert_eq!(b["counts"]["waiting"], 0, "b2 family: {b}");
}

#[test]
fn b3_oldest_waiting_comes_from_the_earlier_month() {
    let temp = empty_vault("bob-cli-walks-b3");
    let vault = vault_dir(&temp);
    write_file(&vault.join("bank.md"), BANK_HUB);
    write_bank_month(
        &vault,
        "2026-10",
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-10\n---\n\n# 🏦 October 2026\n\n## Sat Oct 3\n\n- [ ] $8.00 Market [cat:: groceries]\n",
    );
    write_bank_month(
        &vault,
        "2026-11",
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\n---\n\n# 🏦 November 2026\n\n## Tue Nov 17\n\n- [ ] $10.00 Groceries [cat:: groceries]\n",
    );
    let (_, value) = walks_json(&temp);
    let b = family(&value, "b");
    assert_eq!(b["counts"]["waiting"], 2);
    assert_eq!(b["oldest_waiting"], "2026-10-03");
    let label = b["label"].as_str().expect("label");
    assert!(label.contains("oldest Sat Oct 3"), "oldest label: {label}");
}

#[test]
fn b4_invalid_bank_leaves_s_and_w_available() {
    let temp = empty_vault("bob-cli-walks-b4");
    let vault = vault_dir(&temp);
    write_file(
        &vault.join("bank.md"),
        "---\nbank:\n  accounts: nope\n---\n\n# Bank\n",
    );
    write_tasks(&vault, "- [*] #task Next one\n");
    let (_, value) = walks_json(&temp);
    assert_eq!(value["ok"], true);
    let s = family(&value, "s");
    let w = family(&value, "w");
    let b = family(&value, "b");
    assert_eq!(s["available"], true);
    assert_eq!(s["attention"], true);
    assert_eq!(w["available"], true);
    assert_eq!(b["available"], false);
    assert_eq!(b["attention"], false);
    assert!(b["tier"].is_null());
    assert!(b["label"].is_null());
    assert!(
        b["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "bank reason: {value}"
    );
}

#[test]
fn invalid_plan_config_makes_w_unavailable() {
    let temp = empty_vault("bob-cli-walks-plan-invalid");
    write_tasks(&vault_dir(&temp), "- [*] #task Next one\n");
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_pending: 0\n");
    let output = walks_command(&temp, &["-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run bob walks");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let s = family(&value, "s");
    let w = family(&value, "w");
    assert_eq!(s["available"], true);
    assert_eq!(w["available"], false);
    assert!(
        w["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("max_pending")),
        "plan reason: {value}"
    );
}

#[test]
fn walk_counts_match_freshness_list() {
    let temp = empty_vault("bob-cli-walks-parity");
    write_tasks(
        &vault_dir(&temp),
        "- [?] #task #gtd #pre Morning [scheduled:: 2026-11-17]\n\
         - [ ] #task New capture\n\
         - [*] #task Next one\n\
         - [ ] #task Deferred [fresh:: 2026-11-10] [scheduled:: 2026-11-16]\n\
         - [ ] #task Rotten bread [fresh:: 2026-10-01]\n\
         - [?] #task #gtd #post Closeout [scheduled:: 2026-11-17]\n",
    );
    let (_, walks) = walks_json(&temp);
    let mut freshness = bob_command();
    let output = freshness
        .arg("freshness")
        .arg("list")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob freshness list");
    assert_success(&output);
    let freshness: Value =
        serde_json::from_str(stdout(&output).trim()).expect("freshness json");
    let s = family(&walks, "s");
    assert_eq!(s["counts"]["walk"], freshness["counts"]["walk"]);
    let by_tier = &freshness["counts"]["by_tier"];
    let commitments = by_tier["pre"].as_u64().unwrap()
        + by_tier["new"].as_u64().unwrap()
        + by_tier["projects"].as_u64().unwrap()
        + by_tier["pending"].as_u64().unwrap()
        + by_tier["next"].as_u64().unwrap()
        + by_tier["recurring"].as_u64().unwrap()
        + by_tier["tickler"].as_u64().unwrap();
    assert_eq!(s["counts"]["commitments"], commitments);
}

#[test]
fn human_output_covers_attention_calm_and_unavailable() {
    let temp = empty_vault("bob-cli-walks-human");
    let vault = vault_dir(&temp);
    write_tasks(&vault, "- [*] #task Next one\n");
    write_file(
        &vault.join("bank.md"),
        "---\nbank:\n  accounts: nope\n---\n\n# Bank\n",
    );
    let output = walks_command(&temp, &[]).output().expect("run bob walks");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("s ● Review:"), "{text}");
    assert!(text.contains("NEXT first"), "{text}");
    assert!(text.contains("w · Lanes within limits"), "{text}");
    assert!(text.contains("b ? unavailable —"), "{text}");
}

#[test]
fn missing_vault_is_an_unversioned_error() {
    let output = bob_command()
        .arg("walks")
        .arg("-f")
        .arg("json")
        .arg("-b")
        .arg("/definitely/missing/bob-walks-vault")
        .output()
        .expect("run bob walks missing vault");
    assert_eq!(output.status.code(), Some(1));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("error json");
    assert_eq!(value["ok"], false);
    assert!(value.get("schema_version").is_none());
    assert!(value["error"]
        .as_str()
        .is_some_and(|error| { error.contains("not a directory") }));
}

#[test]
fn help_names_the_contract() {
    let output = bob_command()
        .arg("walks")
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob walks --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("docs/walks.md"), "{help}");
    assert!(help.contains("read-only"), "{help}");
    assert!(help.contains("s (any review row"), "{help}");
}

#[test]
fn goldens_clear_all_and_bank_unavailable() {
    let clear = empty_vault("bob-cli-walks-golden-clear");
    let (_, clear_json) = walks_json(&clear);
    check_golden("walks-clear.json", &clear_json);

    let all = empty_vault("bob-cli-walks-golden-all");
    let vault = vault_dir(&all);
    write_tasks(
        &vault,
        &format!(
            "{}{}",
            n_lines("Next", "*", 16, ""),
            n_lines("Ready", " ", 3, " [fresh:: 2026-11-15]"),
        ),
    );
    write_file(&vault.join("bank.md"), BANK_HUB);
    write_bank_month(
        &vault,
        "2026-11",
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\n---\n\n# 🏦 November 2026\n\n## Tue Nov 3\n\n\
         - [ ] $10.00 Groceries [cat:: groceries]\n\
         - [ ] $4.00 Coffee [cat:: groceries]\n\
         - [ ] $8.00 Market [cat:: groceries]\n",
    );
    let (_, all_json) = walks_json(&all);
    let s = family(&all_json, "s");
    let w = family(&all_json, "w");
    let b = family(&all_json, "b");
    assert_eq!(s["tier"], "next");
    assert_eq!(w["tier"], "over");
    assert_eq!(b["attention"], true);
    check_golden("walks-all.json", &all_json);

    let unavailable = empty_vault("bob-cli-walks-golden-bank");
    write_file(
        &vault_dir(&unavailable).join("bank.md"),
        "---\nbank:\n  accounts: nope\n---\n\n# Bank\n",
    );
    let (_, unavailable_json) = walks_json(&unavailable);
    check_golden("walks-bank-unavailable.json", &unavailable_json);
}
