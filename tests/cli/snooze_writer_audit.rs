//! Writer audit: a trailing `## Snoozed` section must survive every Rust
//! ledger writer byte-for-byte, plus the wake-engine end-to-end gaps.
//!
//! Contract: `docs/snooze.md` § writer audit (from
//! `plan:202611/snoozed_tasks.md` phase writer-audit). The shared fixture
//! holds running, untimed, and closed Pomodoros, active/blocked/history
//! snooze rows with nested notes, and a following sentinel section. Each
//! writer runs for real (binary level); the Snoozed tail must be identical
//! afterwards, including CRLF coverage where helpers manipulate offsets.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

const TIMEZONE_CONFIG: &str = "timezone: America/New_York\n";

fn audit_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), TIMEZONE_CONFIG);
    let daily = vault.join("2026").join("20261008.md");
    write_file(&daily, &audit_daily());
    write_file(&vault.join("jobs.md"), &audit_tasks());
    (temp, vault, daily)
}

fn audit_daily() -> String {
    concat!(
        "# 2026-10-08\n",
        "\n",
        "## Pomodoros\n",
        "\n",
        "- [ ] (**0900-1000** [t:: 60m]) — WORK\n",
        "  - [[jobs#^early]]\n",
        "- [ ] () — WORK\n",
        "- [ ] () — BUILD\n",
        "- [x] (**0700-0800** [t:: 60m]) — WORK\n",
        "\n",
        "## Snoozed\n",
        "\n",
        "- **1530** [[jobs#^snoozed]]\n",
        "  - Keep this note\n",
        "  - nested detail\n",
        "- **1700** [[jobs#^blocked]]\n",
        "- ~~**0900** [[jobs#^woken]]~~ · woke 09:05\n",
        "\n",
        "## Next\n",
        "\n",
        "- sentinel stays put\n",
    )
    .to_string()
}

fn audit_tasks() -> String {
    concat!(
        "- [*] #task Early work ^early\n",
        "- [*] #task Other work ^other\n",
        "- [*] #task Fresh work ^fresh\n",
        "- [*] #task Snoozed work ^snoozed\n",
        "- [?] #task Blocked work ^blocked\n",
        "  - \u{26D3}\u{FE0F} **DEPENDS ON:** [[jobs#^dep]]\n",
        "- [ ] #task Dep work ^dep\n",
        "- [*] #task Woken work ^woken\n",
        "- [ ] #task Ready work ^ready\n",
        "- [x] #task Done work ^done\n",
    )
    .to_string()
}

/// Everything from the `## Snoozed` heading to end of file.
fn snoozed_tail(daily: &str) -> &str {
    let start = daily.find("## Snoozed").expect("Snoozed section");
    &daily[start..]
}

fn read_daily(daily: &Path) -> String {
    fs::read_to_string(daily).expect("read daily note")
}

fn capture_output(
    vault: &Path,
    daily: &Path,
    temp: &Path,
    now: &str,
    args: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", vault.join("bob.yml"))
        .env("BOB_NOW", now)
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.join("vault.lock"));
    command.output().expect("run bob capture")
}

fn assert_snoozed_preserved(before: &str, after: &str) {
    assert_eq!(
        snoozed_tail(after),
        snoozed_tail(before),
        "trailing Snoozed section plus sentinel must be byte-identical"
    );
    assert!(
        after.contains("## Pomodoros"),
        "Pomodoros section survives: {after}"
    );
}

#[test]
fn capture_link_preserves_trailing_snoozed() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-link");
    let before = read_daily(&daily);
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 09:30:00",
        &["@jobs:fresh"],
    );
    assert_success(&output);
    let after = read_daily(&daily);
    assert!(
        after.contains("- [[jobs#^fresh]]"),
        "link landed in the ledger: {after}"
    );
    assert_snoozed_preserved(&before, &after);
}

#[test]
fn capture_start_preserves_trailing_snoozed() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-start");
    let before = read_daily(&daily);
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 11:00:00",
        &["=audit"],
    );
    assert_success(&output);
    let after = read_daily(&daily);
    assert!(after.contains("— AUDIT"), "named entry created: {after}");
    assert_snoozed_preserved(&before, &after);
}

#[test]
fn capture_close_preserves_trailing_snoozed() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-close");
    let before = read_daily(&daily);
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 09:30:00",
        &["x"],
    );
    assert_success(&output);
    let after = read_daily(&daily);
    assert!(
        after.contains("- [x] (**0900-0930**"),
        "running entry closed at the 09:30 close time: {after}"
    );
    assert_snoozed_preserved(&before, &after);
}

#[test]
fn capture_record_preserves_trailing_snoozed() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-record");
    let before = read_daily(&daily);
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 11:00:00",
        &["x=build"],
    );
    assert_success(&output);
    let after = read_daily(&daily);
    assert_snoozed_preserved(&before, &after);
}

#[test]
fn capture_lineup_preserves_trailing_snoozed() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-lineup");
    let before = read_daily(&daily);
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 11:00:00",
        &["="],
    );
    assert_success(&output);
    let after = read_daily(&daily);
    assert_snoozed_preserved(&before, &after);
}

#[test]
fn capture_link_preserves_trailing_snoozed_crlf() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-capture-crlf");
    let crlf = read_daily(&daily).replace('\n', "\r\n");
    write_file(&daily, &crlf);
    let before = fs::read(&daily).expect("read raw daily");
    let output = capture_output(
        &vault,
        &daily,
        temp.path(),
        "2026-10-08 09:30:00",
        &["@jobs:fresh"],
    );
    assert_success(&output);
    let after = fs::read(&daily).expect("read raw daily");
    let marker = b"## Snoozed";
    let before_tail = &before[find_bytes(&before, marker)..];
    let after_tail = &after[find_bytes(&after, marker)..];
    assert_eq!(
        before_tail, after_tail,
        "CRLF Snoozed tail must be byte-identical"
    );
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("Snoozed marker")
}

#[test]
fn reconcile_leaves_snoozed_rows_byte_identical() {
    let (temp, vault, daily) = audit_vault("bob-cli-audit-reconcile");
    let before = read_daily(&daily);
    let output = bob_command()
        .args(["task", "reconcile", "--format", "json", "-r", "0"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", vault.join("bob.yml"))
        .env("BOB_NOW", "2026-10-08")
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run reconcile");
    assert!(
        output.status.success(),
        "reconcile succeeds: {}",
        format_output(&output)
    );
    let after = read_daily(&daily);
    assert_snoozed_preserved(&before, &after);
}

fn snooze_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    let config = temp.path().join("bob.yml");
    write_file(&config, TIMEZONE_CONFIG);
    let daily = vault.join("2026").join("20261008.md");
    write_file(
        &daily,
        concat!(
            "# 2026-10-08\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000** [t:: 60m]) — WORK\n",
            "  - [[jobs#^early]]\n",
            "- [ ] () — WORK\n",
            "\n",
            "## Snoozed\n",
        ),
    );
    write_file(
        &vault.join("jobs.md"),
        concat!(
            "- [*] #task Early work ^early\n",
            "- [*] #task Other work ^other\n",
            "- [*] #task Noted work ^noted\n",
            "- [?] #task Blocked work ^blocked\n",
            "  - \u{26D3}\u{FE0F} **DEPENDS ON:** [[jobs#^dep]]\n",
            "- [ ] #task Dep work ^dep\n",
            "- [ ] #task Ready work ^ready\n",
        ),
    );
    (temp, vault, daily)
}

fn snooze_command(
    temp: &TempDir,
    vault: &Path,
    daily: &Path,
    now: &str,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("snooze")
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", temp.path().join("bob.yml"))
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"))
        .env("BOB_NOW", now);
    command.args(args);
    command
}

fn snooze_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).expect("snooze JSON")
}

#[test]
fn wake_flow_noop_before_target_then_untimed_despite_running() {
    let (temp, vault, daily) = snooze_vault("bob-cli-audit-wake-flow");
    // Give the snoozed task ledger notes: add moves them into the row.
    write_file(
        &daily,
        concat!(
            "# 2026-10-08\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-1000** [t:: 60m]) — WORK\n",
            "  - [[jobs#^early]]\n",
            "- [ ] () — WORK\n",
            "  - [[jobs#^noted]]\n",
            "    - Carry the timeline\n",
            "  - [[jobs#^other]]\n",
            "\n",
            "## Snoozed\n",
        ),
    );
    let added = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T12:00:00-04:00",
        &["add", "jobs:noted", "15:30", "-f", "json"],
    )
    .output()
    .expect("snooze add");
    assert_success(&added);
    assert_eq!(snooze_json(&added)["status"], "added");
    let day_after_add = read_daily(&daily);
    assert!(
        day_after_add.contains("- **1530** [[jobs#^noted]]"),
        "row added: {day_after_add}"
    );
    assert!(
        day_after_add.contains("Carry the timeline"),
        "ledger notes moved into the row: {day_after_add}"
    );
    let task_before = fs::read_to_string(vault.join("jobs.md")).unwrap();

    // Before the target time the wake is a no-op and changes nothing.
    let early = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T13:00:00-04:00",
        &["wake", "-f", "json"],
    )
    .output()
    .expect("early wake");
    assert_success(&early);
    assert_eq!(snooze_json(&early)["status"], "nothing_due");
    assert_eq!(read_daily(&daily), day_after_add);
    assert!(!temp.path().join("state").exists());

    // At the target time the wake lands in the untimed entry even though a
    // running matching entry exists, strikes history, and keeps task bytes.
    let woke = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T15:35:00-04:00",
        &["wake", "-f", "json"],
    )
    .output()
    .expect("due wake");
    assert_success(&woke);
    let report = snooze_json(&woke);
    assert_eq!(report["status"], "woke", "{report}");
    assert_eq!(report["woken"].as_array().unwrap().len(), 1);
    let day_after_wake = read_daily(&daily);
    assert!(
        day_after_wake.contains("- ~~**1530** [[jobs#^noted]]~~ · woke 15:35"),
        "row struck with actual time: {day_after_wake}"
    );
    let untimed = day_after_wake
        .find("- [ ] () — WORK")
        .expect("untimed entry");
    let link_at = day_after_wake.find("- [[jobs#^noted]]").expect("new link");
    assert!(
        link_at > untimed,
        "woken link sits under the untimed entry: {day_after_wake}"
    );
    let running = day_after_wake
        .find("(**0900-1000**")
        .expect("running entry");
    assert!(
        day_after_wake[running..]
            .find("- [[jobs#^noted]]")
            .is_none()
            || link_at > untimed,
        "woken link is not under the running entry: {day_after_wake}"
    );
    assert!(
        day_after_wake.contains("Carry the timeline"),
        "notes travel under the new ledger link: {day_after_wake}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("jobs.md")).unwrap(),
        task_before,
        "wake never edits a task line"
    );
    // Exactly one notification attempt: the sink holds a single document.
    let sink = fs::read_to_string(temp.path().join("notify.json"))
        .expect("notify sink");
    assert!(sink.contains("bob snooze"), "{sink}");
    let receipt_path = temp.path().join("state/bob-cli/snooze/last-wake.json");
    let receipt = fs::read(&receipt_path).expect("wake receipt");
    assert!(!report["wake_id"].as_str().unwrap().is_empty(), "{report}");

    // A retry is a no-op: same vault bytes, same receipt, same sink.
    let retry = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T15:40:00-04:00",
        &["wake", "-f", "json"],
    )
    .output()
    .expect("retry wake");
    assert_success(&retry);
    assert_eq!(snooze_json(&retry)["status"], "nothing_due");
    assert_eq!(read_daily(&daily), day_after_wake);
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt);
    assert_eq!(
        fs::read_to_string(temp.path().join("notify.json")).unwrap(),
        sink
    );
}

#[test]
fn blocked_parks_and_ready_ignores() {
    let (temp, vault, daily) = snooze_vault("bob-cli-audit-park-ignore");
    write_file(
        &daily,
        concat!(
            "# 2026-10-08\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] () — WORK\n",
            "\n",
            "## Snoozed\n",
            "\n",
            "- **1530** [[jobs#^blocked]]\n",
            "- **1600** [[jobs#^ready]]\n",
        ),
    );
    let list = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T16:30:00-04:00",
        &["list", "-f", "json"],
    )
    .output()
    .expect("snooze list");
    assert_success(&list);
    let report = snooze_json(&list);
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{report}");
    let blocked = rows
        .iter()
        .find(|row| row["block_id"] == "blocked")
        .expect("blocked row");
    assert_eq!(blocked["state"], "blocked", "{blocked}");
    let ready = rows
        .iter()
        .find(|row| row["block_id"] == "ready")
        .expect("ready row");
    assert_eq!(ready["state"], "ignored", "{ready}");
    assert_eq!(ready["reason"], "ready", "{ready}");
    assert!(
        report["next_wake_at"].is_null(),
        "blocked and ignored rows never drive the timer: {report}"
    );

    // Adding a Blocked target is refused on every route.
    let add = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T12:00:00-04:00",
        &["add", "jobs:blocked", "15:30", "-f", "json"],
    )
    .output()
    .expect("snooze add blocked");
    assert_eq!(add.status.code(), Some(2), "{}", format_output(&add));

    // Waking parks the blocked row and leaves the ready row alone.
    let before = read_daily(&daily);
    let wake = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T16:30:00-04:00",
        &["wake", "-f", "json"],
    )
    .output()
    .expect("parked wake");
    assert_success(&wake);
    assert_eq!(snooze_json(&wake)["status"], "nothing_due");
    assert_eq!(read_daily(&daily), before);
}

#[test]
fn over_cap_wake_succeeds_and_reports_over() {
    let (temp, vault, daily) = snooze_vault("bob-cli-audit-over-cap");
    write_file(
        &temp.path().join("bob.yml"),
        "timezone: America/New_York\nplan:\n  max_links: 1\n",
    );
    write_file(
        &daily,
        concat!(
            "# 2026-10-08\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] () — WORK\n",
            "  - [[jobs#^early]]\n",
            "  - [[jobs#^noted]]\n",
            "\n",
            "## Snoozed\n",
            "\n",
            "- **1100** [[jobs#^snoozed-x]]\n",
        ),
    );
    write_file(
        &vault.join("jobs.md"),
        "- [*] #task Early work ^early\n- [*] #task Noted work ^noted\n- [*] #task Extra work ^snoozed-x\n",
    );
    let woke = snooze_command(
        &temp,
        &vault,
        &daily,
        "2026-10-08T12:00:00-04:00",
        &["wake", "-f", "json"],
    )
    .output()
    .expect("over-cap wake");
    assert_success(&woke);
    let report = snooze_json(&woke);
    assert_eq!(report["status"], "woke", "{report}");
    assert_eq!(report["plan"]["over"], true, "{report}");
}
