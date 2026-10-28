//! Capture `~` / `~~` cancel-label commits (`docs/task-relations.md` §17,
//! plan:202610/relation_cancel_labels.md phase capture-cancel).
//!
//! Every test runs the draft through dry-run and the real commit and
//! asserts the two agree except for the bob-authored would-forms, so
//! dry-run parity is checked, not assumed.

use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";

const CONFIG: &str =
    "relations:\n  cancel_labels:\n    - duplicate of\n    - superseded by\n";

fn setup(
    name: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    let config = vault.join("config.yml");
    write_file(&config, CONFIG);
    (temp, vault, day, config)
}

fn read(vault: &Path, relative: &str) -> String {
    fs::read_to_string(vault.join(relative)).expect("read note")
}

/// Run the draft dry and for real, asserting the two plans agree: the
/// staged bytes are identical and only the bob-authored cancel phrasing
/// differs (`would cancel` vs `cancelled`).
fn capture_json(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
) -> serde_json::Value {
    capture_json_with_day(vault, day, config, draft, NOW)
}

fn capture_json_with_day(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    now: &str,
) -> serde_json::Value {
    let run = |dry_run: bool| {
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
        command.arg("--").arg(draft);
        command
            .env("BOB_DAY_FILE", day)
            .env("BOB_NOW", now)
            .env("BOB_CONFIG_FILE", config);
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    let real: serde_json::Value = run(false);
    assert_eq!(real["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    if let Some(serde_json::Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = serde_json::Value::Bool(false);
        }
    }
    let masked_text = serde_json::to_string(&masked).expect("dry JSON");
    let masked_text = masked_text
        .replace("would cancel", "cancelled")
        .replace("would remove", "removed")
        .replace("would unblock", "unblocked");
    let masked: serde_json::Value =
        serde_json::from_str(&masked_text).expect("normalized dry JSON");
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for would-forms"
    );
    real
}

fn capture_fail(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft);
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("BOB_CONFIG_FILE", config);
    command.output().expect("run bob capture")
}

/// The coded refusal behind a human-format failure.
fn capture_fail_code(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
) -> (String, String) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft);
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("BOB_CONFIG_FILE", config);
    let output = command.output().expect("run bob capture");
    assert!(!output.status.success(), "refusal must fail");
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    (
        value["code"].as_str().unwrap_or_default().to_string(),
        value["error"].as_str().unwrap_or_default().to_string(),
    )
}

fn capture_human(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command.arg("--").arg(draft);
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("BOB_CONFIG_FILE", config)
        .env("NO_COLOR", "1");
    command.output().expect("run bob capture human")
}

fn snapshot(vault: &Path, relatives: &[&str]) -> Vec<(String, String)> {
    relatives
        .iter()
        .map(|relative| ((*relative).to_string(), read(vault, relative)))
        .collect()
}

fn assert_unchanged(vault: &Path, before: &[(String, String)]) {
    for (relative, contents) in before {
        assert_eq!(
            read(vault, relative),
            *contents,
            "{relative} must stay byte-identical"
        );
    }
}

// Forward cancel: an existing owner links with a cancel label, so the
// source cancels, today's link prunes, and the dependent recovers.
#[test]
fn cancel_forward_cancels_prunes_and_recovers() {
    let (_temp, vault, _, config) = setup("bob-cli-cancel-forward");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
    );
    write_file(
        &vault.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-stocks-now\n",
    );
    write_file(
        &vault.join("today.md"),
        concat!(
            "## Pomodoros\n",
            "- [ ] Morning 0900-1000\n",
            "  - [[cash#^sell-stocks]]\n",
        ),
    );
    write_file(
        &vault.join("c.md"),
        concat!(
            "- [?] #task Needs stocks [fresh:: 2026-10-14] [dependsOn:: cash__sell-stocks] ^needs-stocks\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^sell-stocks]]\n",
        ),
    );
    let day = vault.join("today.md");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"duplicate of\"",
    );
    assert_eq!(json["kind"], "task_dependency");
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["source"]["path"], "cash.md");
    assert_eq!(cancel["source"]["line"], 1);
    assert_eq!(cancel["source"]["block_id"], "sell-stocks");
    assert_eq!(cancel["source"]["text"], "Sell stocks");
    assert_eq!(cancel["target"]["path"], "dev.md");
    assert_eq!(cancel["target"]["block_id"], "sell-stocks-now");
    assert_eq!(cancel["labels"], serde_json::json!(["duplicate of"]));
    assert_eq!(cancel["action"], "added");
    assert_eq!(cancel["previous_status"], " ");
    assert_eq!(cancel["previous_status_name"], "Todo");
    assert!(cancel.get("project_open_tasks").is_none(), "{cancel}");
    assert_eq!(cancel["cancel_log"], "none");
    assert_eq!(cancel["pomodoro_links_removed"], 1);
    assert_eq!(cancel["recovered"][0]["path"], "c.md");
    assert_eq!(cancel["recovered"][0]["line"], 1);
    assert_eq!(cancel["recovered"][0]["block_id"], "needs-stocks");
    assert_eq!(cancel["recovered"][0]["text"], "Needs stocks");
    assert_eq!(
        cancel["message"],
        "⊘ cancelled “Sell stocks” · Todo → Cancelled · removed 1 Pomodoro link · unblocked 1",
        "{cancel}"
    );
    assert_eq!(json["warnings"][0]["code"], "relation_cancelled_source");
    assert_eq!(json["warnings"][0]["label"], "duplicate of");
    assert_eq!(
        json["warnings"][0]["message"],
        "⊘ cancelled “Sell stocks” (“duplicate of” “Sell stocks now”)",
        "{json}"
    );

    assert_eq!(
        read(&vault, "cash.md"),
        concat!(
            "- [-] #task Sell stocks [id:: cash__sell-stocks] [cancelled:: 2026-10-14] ^sell-stocks\n",
            "\t- 🪢 **LINKS:** duplicate of [[dev#^sell-stocks-now]]\n",
        ),
        "source cancel bytes"
    );
    assert_eq!(
        read(&vault, "today.md"),
        "## Pomodoros\n- [ ] Morning 0900-1000\n",
        "dedicated Pomodoro bullet prunes with its subtree"
    );
    assert_eq!(
        read(&vault, "c.md"),
        concat!(
            "- [ ] #task Needs stocks [fresh:: 2026-10-14] [unblocked:: 2026-10-14] [dependsOn:: cash__sell-stocks] ^needs-stocks\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^sell-stocks]]\n",
        ),
        "dependent recovers with an unblocked stamp"
    );
    assert!(
        read(&vault, "dev.md").contains(
            "- 🪢 **LINKED FROM:** [[cash#^sell-stocks]] (duplicate of)\n"
        ),
        "mirror still writes:\n{}",
        read(&vault, "dev.md")
    );
}

// The human renderer prints one bob-authored ⊘ row and never repeats
// the structured warning on stderr.
#[test]
fn cancel_human_names_the_cancel_once() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-human");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let dry = capture_human(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
        true,
    );
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(
        text.contains("🪢 would link to") && text.contains("⊘ would cancel"),
        "dry human names the link and the cancel:\n{text}"
    );
    assert!(
        !stderr(&dry).contains("relation_cancelled_source"),
        "the ⊘ row replaces the stderr warning:\n{}",
        stderr(&dry)
    );

    let real = capture_human(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
        false,
    );
    assert_success(&real);
    let text = stdout(&real);
    assert!(
        text.contains("🪢 links to") && text.contains("⊘ cancelled"),
        "real human names the link and the cancel:\n{text}"
    );
    assert!(
        !stderr(&real).contains("warning:"),
        "no stderr warning when the ⊘ row covers it:\n{}",
        stderr(&real)
    );
}

// Reverse cancel: a new task retires an existing task, so the picked
// source cancels, the new task survives, and a project dependent of the
// source recovers with an unblocked stamp.
#[test]
fn cancel_reverse_retires_old_task() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-reverse");
    write_file(
        &vault.join("sase_blog.md"),
        concat!(
            "- [ ] #task Write introduction [id:: sase_blog__write-intro] ^write-intro\n",
            "- [?] #task Ship v1 draft [fresh:: 2026-10-14] [dependsOn:: sase_blog__write-intro] ^v1-draft\n",
            "\t- ⛓️ **DEPENDS ON:** [[sase_blog#^write-intro]]\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "Write v2 intro ~~sase_blog:write-intro|\"superseded by\"",
    );
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("~~ mints the new task id")
        .to_string();
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["source"]["path"], "sase_blog.md");
    assert_eq!(cancel["source"]["block_id"], "write-intro");
    assert_eq!(cancel["source"]["text"], "Write introduction");
    assert_eq!(cancel["target"]["path"], "mac_inbox.md");
    assert_eq!(cancel["target"]["block_id"], minted);
    assert_eq!(cancel["labels"], serde_json::json!(["superseded by"]));
    assert_eq!(cancel["action"], "added");
    assert_eq!(cancel["previous_status"], " ");
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    assert_eq!(
        cancel["recovered"][0]["block_id"], "v1-draft",
        "the project draft recovers: {cancel}"
    );

    let blog = read(&vault, "sase_blog.md");
    assert!(
        blog.contains("- [-] #task Write introduction [id:: sase_blog__write-intro] [cancelled:: 2026-10-14] ^write-intro\n"),
        "old task cancels:\n{blog}"
    );
    assert!(
        blog.contains(
            "- [ ] #task Ship v1 draft [fresh:: 2026-10-14] [unblocked:: 2026-10-14] [dependsOn:: sase_blog__write-intro] ^v1-draft\n"
        ),
        "dependent recovers with a stamp:\n{blog}"
    );
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains(&format!(
            "- [ ] #task Write v2 intro [created::2026-10-14] ^{minted}\n"
        )),
        "the new task survives:\n{inbox}"
    );
}

// Each refusal is coded, names the fix, and writes nothing.
#[test]
fn cancel_refusals_are_coded_and_write_nothing() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-refusals");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(
        &vault.join("r.md"),
        "- [ ] #task Repeat [repeat:: every week] ^rep\n",
    );
    let before = snapshot(&vault, &["a.md", "b.md", "r.md", "day.md"]);

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New thing ~a:alpha|\"duplicate of\"",
    );
    assert_eq!(code, "relation_cancels_new_task");
    assert!(
        message.contains("is new") && message.contains("use ~~"),
        "new-task refusal hints ~~: {message}"
    );

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@r+rep ~a:alpha|\"duplicate of\"",
    );
    assert_eq!(code, "relation_cancels_recurring_task");
    assert!(
        message.contains("repeats"),
        "recurring refusal names the fix: {message}"
    );

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@b+beta &a:alpha ~a:alpha|\"duplicate of\"",
    );
    assert_eq!(code, "relation_cancel_conflicts_with_item_action");
    assert!(
        message.contains("also uses &"),
        "conflict refusal names the marker: {message}"
    );

    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New thing ~~a:alpha|\"duplicate of\" *a:alpha",
    );
    assert_eq!(
        code, "relation_cancel_conflicts_with_item_action",
        "a * naming the picked source conflicts too"
    );

    assert_unchanged(&vault, &before);
    let output = capture_fail(
        &vault,
        &day,
        &config,
        "New thing ~a:alpha|\"duplicate of\"",
    );
    assert!(!output.status.success());
    assert_unchanged(&vault, &before);
}

// Done and Cancelled sources link only: no status change, no new date,
// and the muted already-closed row.
#[test]
fn cancel_already_closed_links_only() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-closed");
    write_file(
        &vault.join("a.md"),
        concat!(
            "- [x] #task Done deed ^done\n",
            "- [-] #task Old news [cancelled:: 2026-10-01] ^old\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let json =
        capture_json(&vault, &day, &config, "@a+done ~b:beta|\"duplicate of\"");
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "already_closed",
        "{json}"
    );
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["previous_status"], "x");
    assert_eq!(cancel["previous_status_name"], "Done");
    assert_eq!(cancel["message"], "⊘ Stays Done · link only");
    assert_eq!(cancel["pomodoro_links_removed"], 0);
    assert!(cancel["recovered"].as_array().unwrap().is_empty());
    assert!(json.get("warnings").is_none(), "{json}");

    let second =
        capture_json(&vault, &day, &config, "@a+old ~b:beta|\"superseded by\"");
    assert_eq!(
        second["relation_cancels"][0]["message"],
        "⊘ Already Cancelled · link only",
        "{second}"
    );

    assert_eq!(
        read(&vault, "a.md"),
        concat!(
            "- [x] #task Done deed ^done\n",
            "\t- 🪢 **LINKS:** duplicate of [[b#^beta]]\n",
            "- [-] #task Old news [cancelled:: 2026-10-01] ^old\n",
            "\t- 🪢 **LINKS:** superseded by [[b#^beta]]\n",
        ),
        "closed sources keep every status byte"
    );
}

// Re-running the same cancel draft is a no-op the second time.
#[test]
fn cancel_unchanged_rerun_is_noop() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-rerun");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let first = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(first["relation_updates"][0]["action"], "added");
    let after_first = snapshot(&vault, &["a.md", "b.md", "day.md"]);

    let second = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(second["relation_updates"][0]["action"], "unchanged");
    assert!(second.get("relation_cancels").is_none(), "{second}");
    assert!(second.get("warnings").is_none(), "{second}");
    assert_unchanged(&vault, &after_first);
}

// Relabeling into a cancel label cancels; relabeling away (or unlinking
// by relabel) leaves the source Cancelled.
#[test]
fn cancel_relabel_into_and_away() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-relabel");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let plain = capture_json(&vault, &day, &config, "@a+alpha ~b:beta|see");
    assert!(plain.get("relation_cancels").is_none(), "{plain}");
    assert!(read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"));

    let relabel = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(relabel["relation_updates"][0]["action"], "relabeled");
    assert_eq!(relabel["relation_updates"][0]["previous_label"], "see");
    assert_eq!(
        relabel["relation_updates"][0]["cancel_effect"], "cancelled",
        "{relabel}"
    );
    assert!(
        read(&vault, "a.md")
            .starts_with("- [-] #task Alpha [cancelled:: 2026-10-14] ^alpha\n"),
        "relabel into a cancel label cancels:\n{}",
        read(&vault, "a.md")
    );

    capture_json(&vault, &day, &config, "@a+alpha ~b:beta|see");
    assert!(
        read(&vault, "a.md")
            .starts_with("- [-] #task Alpha [cancelled:: 2026-10-14] ^alpha\n"),
        "relabel away leaves the source Cancelled:\n{}",
        read(&vault, "a.md")
    );
}

// Two cancel edges on one source retire once, with one log line joining
// both labels in authored order.
#[test]
fn cancel_two_edges_one_source_one_log_line() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-two-edges");
    write_file(
        &vault.join("a.md"),
        concat!(
            "- [ ] #task Alpha ^alpha\n",
            "  - ❌ **CANCEL LOG**\n",
            "    - *2026-10-01* — older\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\" ~c:gamma|\"superseded by\"",
    );
    assert_eq!(json["relation_cancels"].as_array().unwrap().len(), 1);
    let cancel = &json["relation_cancels"][0];
    assert_eq!(
        cancel["labels"],
        serde_json::json!(["duplicate of", "superseded by"]),
        "{cancel}"
    );
    assert_eq!(cancel["cancel_log"], "prepended");

    assert_eq!(
        read(&vault, "a.md"),
        concat!(
            "- [-] #task Alpha [cancelled:: 2026-10-14] ^alpha\n",
            "  - ❌ **CANCEL LOG**\n",
            "    - *2026-10-14* — 🪢 duplicate of [[b#^beta]] · 🪢 superseded by [[c#^gamma]]\n",
            "    - *2026-10-01* — older\n",
            "  - 🪢 **LINKS:** duplicate of [[b#^beta]] • superseded by [[c#^gamma]]\n",
        ),
        "one cancel, one prepended log line:\n{}",
        read(&vault, "a.md")
    );
}

// The source lives in today's daily note file: one staged file holds
// the task line, the LINKS edit, the cancel, and the pruned Pomodoro
// link. The source sits above the Pomodoros section: rewriting an
// in-section task line trips the block tracker's vanished-entry guard,
// a pre-existing limitation every status gesture shares (recorded as a
// follow-up), so this test keeps the meaningful same-file coverage.
#[test]
fn cancel_source_in_daily_note_stays_one_file() {
    let (_temp, vault, _, config) = setup("bob-cli-cancel-daily-source");
    let day = vault.join("day.md");
    write_file(
        &day,
        concat!(
            "- [ ] #task Day task ^daytask\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] Morning 0900-1000\n",
            "  - [[day#^daytask]]\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@day+daytask ~b:beta|\"duplicate of\"",
    );
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    assert_eq!(json["relation_cancels"][0]["pomodoro_links_removed"], 1);

    assert_eq!(
        read(&vault, "day.md"),
        concat!(
            "- [-] #task Day task [cancelled:: 2026-10-14] ^daytask\n",
            "\t- 🪢 **LINKS:** duplicate of [[b#^beta]]\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] Morning 0900-1000\n",
        ),
        "one staged file holds every write:\n{}",
        read(&vault, "day.md")
    );
}

// No daily note on disk means zero pruned links and no created note.
#[test]
fn cancel_without_daily_note_prunes_nothing() {
    let (_temp, vault, _, config) = setup("bob-cli-cancel-no-day");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let missing = vault.join("2026/20261014.md");
    assert!(!missing.exists());

    let json = capture_json(
        &vault,
        &missing,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(json["relation_cancels"][0]["pomodoro_links_removed"], 0);
    assert!(
        read(&vault, "a.md")
            .starts_with("- [-] #task Alpha [cancelled:: 2026-10-14]"),
        "the cancel still stages:\n{}",
        read(&vault, "a.md")
    );
    assert!(!missing.exists(), "capture never creates the daily note");
}

// `linked_from_lines: false` skips only the mirror; the cancel fires.
#[test]
fn cancel_with_mirror_kill_switch_still_cancels() {
    let (_temp, vault, day, _) = setup("bob-cli-cancel-no-mirror");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  linked_from_lines: false\n  cancel_labels:\n    - duplicate of\n",
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(json["mirror_skipped"][0]["reason"], "disabled", "{json}");
    assert_eq!(
        json["relation_cancels"][0]["message"],
        "⊘ cancelled “Alpha” · Todo → Cancelled",
        "{json}"
    );
    assert_eq!(
        read(&vault, "b.md"),
        "- [ ] #task Beta ^beta\n",
        "the mirror stays untouched"
    );
}

// Hide is not disable: a hidden label and a `||` one-off label still
// cancel when typed.
#[test]
fn cancel_hidden_and_oneoff_labels_still_cancel() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-hidden");
    write_file(
        &vault.join("a.md"),
        concat!("- [ ] #task Alpha ^alpha\n", "- [ ] #task Second ^second\n"),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let mut hide = bob_command();
    hide.arg("relations")
        .arg("hide")
        .arg("-b")
        .arg(&vault)
        .arg("--allow-unused")
        .arg("--")
        .arg("duplicate of");
    hide.env("BOB_CONFIG_FILE", &config);
    let hide = hide.output().expect("hide the cancel label");
    assert_success(&hide);

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(
        json["relation_cancels"][0]["message"],
        "⊘ cancelled “Alpha” · Todo → Cancelled",
        "a hidden cancel label still cancels: {json}"
    );

    // A `||` one-off on the new-task form records the hide and still
    // cancels. (On the `@owner` form hides never reach the plan, a
    // pre-existing gap recorded as a follow-up.)
    let oneoff = capture_json(
        &vault,
        &day,
        &config,
        "New intro ~~b:beta||\"superseded by\"",
    );
    assert_eq!(
        oneoff["relation_cancels"][0]["message"],
        "⊘ cancelled “Beta” · Todo → Cancelled",
        "a one-off cancel label still cancels: {oneoff}"
    );
    assert_eq!(
        oneoff["relation_label_hides"][0]["label"], "superseded by",
        "{oneoff}"
    );
}

// A `^prj` source cancels like Task Card `x`: residents stay open and
// the copy names how many.
#[test]
fn cancel_prj_source_cancels_project_only() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-prj");
    write_file(
        &vault.join("s.md"),
        concat!(
            "- [ ] #task Ship it [id:: s__prj] ^prj\n",
            "- [ ] #task Resident one ^res-one\n",
            "- [?] #task Resident blocked [fresh:: 2026-10-14] [dependsOn:: s__prj] ^res-blocked\n",
            "\t- ⛓️ **DEPENDS ON:** [[s#^prj]]\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let json =
        capture_json(&vault, &day, &config, "@s+prj ~b:beta|\"duplicate of\"");
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["project_open_tasks"], 2);
    assert_eq!(
        cancel["message"],
        "⊘ cancelled “Ship it” · Todo → Cancelled · unblocked 1 · its 2 open tasks stay open",
        "{cancel}"
    );

    let note = read(&vault, "s.md");
    assert!(
        note.contains(
            "- [-] #task Ship it [id:: s__prj] [cancelled:: 2026-10-14] ^prj\n"
        ),
        "the project cancels:\n{note}"
    );
    assert!(
        note.contains("- [ ] #task Resident one ^res-one\n"),
        "residents stay open:\n{note}"
    );
    assert!(
        note.contains("- [ ] #task Resident blocked [fresh:: 2026-10-14] [unblocked:: 2026-10-14] [dependsOn:: s__prj] ^res-blocked\n"),
        "residents blocked only by the project recover:\n{note}"
    );
}

// A dependent that still waits on another open prerequisite stays
// Blocked and is not reported recovered.
#[test]
fn cancel_dependent_with_other_prereq_stays_blocked() {
    let (_temp, vault, day, config) = setup("bob-cli-cancel-still-blocked");
    write_file(
        &vault.join("a.md"),
        "- [ ] #task Alpha [id:: a__alpha] ^alpha\n",
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(
        &vault.join("c.md"),
        concat!(
            "- [ ] #task Other ^other\n",
            "- [?] #task Needs both ^needs-both\n",
        ),
    );
    // Two prerequisites: the `&` gesture writes one, then the second is
    // appended by a follow-up capture on the same owner.
    capture_json(&vault, &day, &config, "@c+needs-both &a:alpha");
    capture_json(&vault, &day, &config, "@c+needs-both &c:other");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert!(
        json["relation_cancels"][0]["recovered"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing recovers while another prerequisite stays open: {json}"
    );
    assert!(
        read(&vault, "c.md").contains("- [?] #task Needs both"),
        "the dependent stays Blocked:\n{}",
        read(&vault, "c.md")
    );
}

// The cancel date follows the vault day across a New York midnight.
#[test]
fn cancel_midnight_dates_follow_bob_now() {
    for (name, now, expected) in [
        (
            "bob-cli-cancel-midnight-before",
            "2026-10-14 23:59:00",
            "2026-10-14",
        ),
        (
            "bob-cli-cancel-midnight-after",
            "2026-10-15 00:05:00",
            "2026-10-15",
        ),
    ] {
        let (_temp, vault, day, config) = setup(name);
        write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
        write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
        capture_json_with_day(
            &vault,
            &day,
            &config,
            "@a+alpha ~b:beta|\"duplicate of\"",
            now,
        );
        assert!(
            read(&vault, "a.md").contains(&format!("[cancelled:: {expected}]")),
            "BOB_NOW {now} cancels with {expected}:\n{}",
            read(&vault, "a.md")
        );
    }
}

// A later item sees the staged cancel through the overlays: linking,
// ensuring Next, or completing the cancelled task is refused, and a
// two-item batch that tries is all-or-nothing.
#[test]
fn cancel_cross_item_sees_staged_cancel() {
    let (_temp, vault, _, config) = setup("bob-cli-cancel-cross-item");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(
        &vault.join("today.md"),
        "## Pomodoros\n- [ ] Morning 0900-1000\n",
    );
    let day = vault.join("today.md");

    capture_json(&vault, &day, &config, "@a+alpha ~b:beta|\"duplicate of\"");

    for draft in ["@a:alpha", "@a+alpha", "!a:alpha"] {
        let output = capture_fail(&vault, &day, &config, draft);
        assert!(
            !output.status.success(),
            "{draft} must refuse the cancelled task"
        );
        assert!(
            stderr(&output).contains("Canceled")
                || stderr(&output).contains("Unknown"),
            "{draft} refusal names the closed state:\n{}",
            stderr(&output)
        );
    }

    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let before = snapshot(&vault, &["a.md", "b.md", "today.md"]);
    let output = capture_fail(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"\n\n@a:alpha",
    );
    assert!(
        !output.status.success(),
        "a batch that links a just-cancelled task fails"
    );
    assert_unchanged(&vault, &before);
}

// With no cancel labels configured, the same draft is a plain link:
// every new field stays absent and the source stays open.
#[test]
fn cancel_inert_without_config() {
    let temp = TempDir::new("bob-cli-cancel-inert");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command.arg("--").arg("@a+alpha ~b:beta|\"duplicate of\"");
        command.env("BOB_DAY_FILE", &day).env("BOB_NOW", NOW);
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    let real: serde_json::Value = run(false);
    assert!(dry.get("relation_cancels").is_none(), "{dry}");
    assert!(dry.get("warnings").is_none(), "{dry}");
    let updates = dry["relation_updates"].as_array().expect("updates");
    assert_eq!(updates.len(), 1);
    assert!(updates[0].get("cancel_effect").is_none(), "{dry}");
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"),
        "the source stays open:\n{}",
        read(&vault, "a.md")
    );
    let _ = real;
}

// An invalid cancel_labels value warns once and cancels nothing.
#[test]
fn cancel_invalid_config_warns_and_cancels_nothing() {
    let (_temp, vault, day, _) = setup("bob-cli-cancel-bad-config");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(&config, "relations:\n  cancel_labels: duplicate of\n");

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@a+alpha ~b:beta|\"duplicate of\"");
    command
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("run capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert!(json.get("relation_cancels").is_none(), "{json}");
    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{json}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap_or_default()
            .contains("relations.cancel_labels"),
        "the warning names the key: {json}"
    );
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"),
        "nothing cancels:\n{}",
        read(&vault, "a.md")
    );
}
