//! Capture `~` / `~~` complete-label commits (`docs/task-relations.md` §18,
//! plan:202610/complete_label_capture.md).
//!
//! Every test that commits a complete-label draft runs it dry and for
//! real and asserts the two agree except for the bob-authored
//! would-forms, so dry-run parity is checked, not assumed.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Output;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";

const CONFIG: &str = concat!(
    "relations:\n",
    "  cancel_labels:\n",
    "    - duplicate of\n",
    "    - superseded by\n",
    "  complete_labels:\n",
    "    - implemented by\n",
    "    - verified by\n",
);

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

fn capture_json(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
) -> serde_json::Value {
    capture_json_with(vault, day, config, draft, &[])
}

fn capture_json_with(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    extra: &[&str],
) -> serde_json::Value {
    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .arg("-f")
            .arg("json");
        command.args(extra);
        if dry_run {
            command.arg("--dry-run");
        }
        command.arg("--").arg(draft);
        command
            .env("BOB_DAY_FILE", day)
            .env("BOB_NOW", NOW)
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
        .replace("would complete", "completed")
        .replace("would strike", "struck")
        .replace("would unblock", "unblocked")
        .replace("would cancel", "cancelled")
        .replace("would remove", "removed");
    let masked: serde_json::Value =
        serde_json::from_str(&masked_text).expect("normalized dry JSON");
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for would-forms"
    );
    real
}

fn capture_fail_code_with(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    extra: &[&str],
) -> (String, String, Output) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    command.args(extra);
    command.arg("--").arg(draft);
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
        output,
    )
}

fn capture_fail_code(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
) -> (String, String) {
    let (code, message, _) =
        capture_fail_code_with(vault, day, config, draft, &[]);
    (code, message)
}

fn capture_human(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    dry_run: bool,
) -> Output {
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

// Forward complete: an existing owner links with a complete label, so
// the source is Done, today's link is struck, and the dependent is
// offered.
#[test]
fn complete_forward_completes_strikes_and_offers() {
    let (_temp, vault, _, config) = setup("bob-cli-complete-forward");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
            "- [?] #task Needs stocks [dependsOn:: cash__sell-stocks] [id:: cash__needs] ^needs-stocks\n",
        ),
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
    let day = vault.join("today.md");

    let json = capture_json_with(
        &vault,
        &day,
        &config,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"implemented by\"",
        &["-p", "all"],
    );
    assert_eq!(json["kind"], "task_dependency");
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["source"]["path"], "cash.md");
    assert_eq!(complete["source"]["line"], 1);
    assert_eq!(complete["source"]["block_id"], "sell-stocks");
    assert_eq!(complete["source"]["text"], "Sell stocks");
    assert_eq!(complete["target"]["path"], "dev.md");
    assert_eq!(complete["target"]["block_id"], "sell-stocks-now");
    assert_eq!(complete["labels"], serde_json::json!(["implemented by"]));
    assert_eq!(complete["action"], "added");
    assert_eq!(complete["previous_status"], " ");
    assert_eq!(complete["previous_status_name"], "Todo");
    assert!(complete.get("project_open_tasks").is_none(), "{complete}");
    assert_eq!(complete["task_complete"]["action"], "completed");
    assert_eq!(complete["task_complete"]["block_id"], "sell-stocks");
    assert_eq!(
        complete["task_complete"]["unblocked"][0]["block_id"],
        "needs-stocks"
    );
    assert_eq!(
        complete["task_complete"]["unblocked"][0]["offer"]["selected"],
        true
    );
    assert!(
        complete["message"]
            .as_str()
            .unwrap_or_default()
            .contains("completed “Sell stocks”")
            && complete["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Todo → Done"),
        "{complete}"
    );
    assert_eq!(json["warnings"][0]["code"], "relation_completed_source");
    assert_eq!(json["warnings"][0]["label"], "implemented by");
    assert_eq!(json["pull_in"]["choice"], "all");
    assert_eq!(json["pull_in"]["offer_count"], 1);

    let cash = read(&vault, "cash.md");
    assert!(
        cash.contains("- [x] #task Sell stocks [id:: cash__sell-stocks]  [completion:: 2026-10-14] ^sell-stocks\n")
            && cash.contains("\t- 🪢 **LINKS:** implemented by [[dev#^sell-stocks-now]]\n"),
        "source Done bytes:\n{cash}"
    );
    let today = read(&vault, "today.md");
    assert!(
        today.contains("~~[[cash#^sell-stocks]]~~")
            && today.contains("[[cash#^needs-stocks]]"),
        "dedicated link is struck and the successor is pulled in:\n{today}"
    );
}

// The human renderer prints one bob-authored ✓ row and never repeats
// the structured warning on stderr.
#[test]
fn complete_human_names_the_complete_once() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-human");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let dry = capture_human(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
        true,
    );
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(
        text.contains("🪢 would link to") && text.contains("✓ would complete"),
        "dry human names the link and the complete:\n{text}"
    );
    assert!(
        !stderr(&dry).contains("relation_completed_source"),
        "the ✓ row replaces the stderr warning:\n{}",
        stderr(&dry)
    );

    let real = capture_human(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
        false,
    );
    assert_success(&real);
    let text = stdout(&real);
    assert!(
        text.contains("🪢 links to") && text.contains("✓ completed"),
        "real human names the link and the complete:\n{text}"
    );
    assert!(
        !stderr(&real).contains("warning:"),
        "no stderr warning when the ✓ row covers it:\n{}",
        stderr(&real)
    );
}

// Reverse complete: a new task finishes an existing task, so the picked
// source is Done and the new task survives.
#[test]
fn complete_reverse_completes_old_task() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-reverse");
    write_file(
        &vault.join("sase_blog.md"),
        "- [ ] #task Write introduction [id:: sase_blog__write-intro] ^write-intro\n",
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "Write v2 intro ~~sase_blog:write-intro|\"implemented by\"",
    );
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("~~ mints the new task id")
        .to_string();
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["source"]["path"], "sase_blog.md");
    assert_eq!(complete["source"]["block_id"], "write-intro");
    assert_eq!(complete["source"]["text"], "Write introduction");
    assert_eq!(complete["target"]["path"], "mac_inbox.md");
    assert_eq!(complete["target"]["block_id"], minted);
    assert_eq!(complete["labels"], serde_json::json!(["implemented by"]));
    assert_eq!(complete["action"], "added");
    assert_eq!(complete["previous_status"], " ");
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );

    let blog = read(&vault, "sase_blog.md");
    assert!(
        blog.contains("- [x] #task Write introduction [id:: sase_blog__write-intro]  [completion:: 2026-10-14] ^write-intro\n"),
        "old task is Done:\n{blog}"
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
fn complete_refusals_are_coded_and_write_nothing() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-refusals");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(
        &vault.join("r.md"),
        "- [ ] #task Repeat [repeat:: every week] ^rep\n",
    );
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");
    let before = snapshot(&vault, &["a.md", "b.md", "r.md", "c.md", "day.md"]);

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New thing ~a:alpha|\"implemented by\"",
    );
    assert_eq!(code, "relation_completes_new_task");
    assert!(
        message.contains("is new") && message.contains("use ~~"),
        "new-task refusal hints ~~: {message}"
    );

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@r+rep ~a:alpha|\"implemented by\"",
    );
    assert_eq!(code, "relation_completes_recurring_task");
    assert!(
        message.contains("repeats"),
        "recurring refusal names the fix: {message}"
    );

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@b+beta &a:alpha ~a:alpha|\"implemented by\"",
    );
    assert_eq!(code, "relation_complete_conflicts_with_item_action");
    assert!(
        message.contains("also uses &"),
        "conflict refusal names the marker: {message}"
    );

    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New thing ~~a:alpha|\"implemented by\" *a:alpha",
    );
    assert_eq!(
        code, "relation_complete_conflicts_with_item_action",
        "a * naming the picked source conflicts too"
    );

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\" ~c:gamma|\"implemented by\"",
    );
    assert_eq!(code, "relation_close_effects_conflict");
    assert!(
        message.contains("can't be both cancelled and completed"),
        "mixed-effect refusal names the contradiction: {message}"
    );

    assert_unchanged(&vault, &before);
}

// Done and Cancelled sources link only: no status change, no new date,
// and the muted already-closed row.
#[test]
fn complete_already_closed_links_only() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-closed");
    write_file(
        &vault.join("a.md"),
        concat!(
            "- [x] #task Done deed ^done\n",
            "- [-] #task Old news [cancelled:: 2026-10-01] ^old\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let dry_done = capture_human(
        &vault,
        &day,
        &config,
        "@a+done ~b:beta|\"implemented by\"",
        true,
    );
    assert_success(&dry_done);
    let done_text = stdout(&dry_done);
    assert!(
        done_text.contains("Already Done · link only")
            && !done_text.contains('✓'),
        "muted already-closed line has no ✓:\n{done_text}"
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+done ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "already_closed",
        "{json}"
    );
    assert!(json.get("relation_completes").is_none(), "{json}");
    assert!(json.get("warnings").is_none(), "{json}");

    let dry_old = capture_human(
        &vault,
        &day,
        &config,
        "@a+old ~b:beta|\"verified by\"",
        true,
    );
    assert_success(&dry_old);
    let old_text = stdout(&dry_old);
    assert!(
        old_text.contains("Stays Cancelled · link only")
            && !old_text.contains('✓'),
        "muted already-cancelled line has no ✓:\n{old_text}"
    );

    let second =
        capture_json(&vault, &day, &config, "@a+old ~b:beta|\"verified by\"");
    assert!(second.get("relation_completes").is_none(), "{second}");
    assert_eq!(
        second["relation_updates"][0]["complete_effect"], "already_closed",
        "{second}"
    );

    assert_eq!(
        read(&vault, "a.md"),
        concat!(
            "- [x] #task Done deed ^done\n",
            "\t- 🪢 **LINKS:** implemented by [[b#^beta]]\n",
            "- [-] #task Old news [cancelled:: 2026-10-01] ^old\n",
            "\t- 🪢 **LINKS:** verified by [[b#^beta]]\n",
        ),
        "closed sources keep every status byte"
    );
}

// Two complete edges on one source finish it once.
#[test]
fn complete_two_edges_one_source_once() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-two-edges");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\" ~c:gamma|\"verified by\"",
    );
    assert_eq!(json["relation_completes"].as_array().unwrap().len(), 1);
    let complete = &json["relation_completes"][0];
    assert_eq!(
        complete["labels"],
        serde_json::json!(["implemented by", "verified by"]),
        "{complete}"
    );
    let note = read(&vault, "a.md");
    assert!(
        note.starts_with(
            "- [x] #task Alpha  [completion:: 2026-10-14] ^alpha\n"
        ),
        "one complete:\n{note}"
    );
}

// Numeric / key / none / all pull-in, plus `--require-pull-in-choice`.
#[test]
fn complete_offers_join_existing_pull_in() {
    let setup_offers = |name: &str| {
        let (temp, vault, _, config) = setup(name);
        write_file(
            &vault.join("sase.md"),
            concat!(
                "- [ ] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
                "- [?] #task Re-launch failed agents [dependsOn:: fix-apollo] [id:: relaunch-agents] ^relaunch-failed-agents\n",
            ),
        );
        write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
        write_file(
            &vault.join("today.md"),
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
                "  - [[sase#^other]]\n",
                "- [ ] () \u{2014} FIX\n",
                "  - [[sase#^fix-apollo]]\n",
            ),
        );
        write_file(&vault.join("other.md"), "- [ ] #task Other live ^other\n");
        let day = vault.join("today.md");
        (temp, vault, day, config)
    };

    let (_temp, vault, day, config) = setup_offers("bob-cli-complete-p-all");
    let all = capture_json_with(
        &vault,
        &day,
        &config,
        "@sase+fix-apollo ~t:tgt|\"implemented by\"",
        &["-p", "all"],
    );
    assert_eq!(all["pull_in"]["choice"], "all");
    assert_eq!(all["pull_in"]["offer_count"], 1);
    assert_eq!(
        all["relation_completes"][0]["task_complete"]["unblocked"][0]["offer"]
            ["selected"],
        true
    );

    let (_temp, vault, day, config) = setup_offers("bob-cli-complete-p-none");
    let none = capture_json_with(
        &vault,
        &day,
        &config,
        "@sase+fix-apollo ~t:tgt|\"implemented by\"",
        &["-p", "none"],
    );
    assert_eq!(none["pull_in"]["choice"], "none");
    assert_eq!(none["pull_in"]["picked_count"], 0);
    assert_eq!(
        none["relation_completes"][0]["task_complete"]["unblocked"][0]
            ["not_linked"],
        "not_selected"
    );

    let (_temp, vault, day, config) = setup_offers("bob-cli-complete-p-n");
    let by_n = capture_json_with(
        &vault,
        &day,
        &config,
        "@sase+fix-apollo ~t:tgt|\"implemented by\"",
        &["-p", "1"],
    );
    assert_eq!(by_n["pull_in"]["choice"], "subset");
    assert_eq!(by_n["pull_in"]["picked_count"], 1);

    let (_temp, vault, day, config) = setup_offers("bob-cli-complete-p-key");
    let by_key = capture_json_with(
        &vault,
        &day,
        &config,
        "@sase+fix-apollo ~t:tgt|\"implemented by\"",
        &["-p", "sase.md#^relaunch-failed-agents"],
    );
    assert_eq!(by_key["pull_in"]["choice"], "subset");
    assert_eq!(
        by_key["relation_completes"][0]["task_complete"]["unblocked"][0]
            ["block_id"],
        "relaunch-failed-agents"
    );

    let (_temp, vault, day, config) = setup_offers("bob-cli-complete-require");
    let before = snapshot(
        &vault,
        &["sase.md", "t.md", "today.md", "other.md", "config.yml"],
    );
    let (code, message, output) = capture_fail_code_with(
        &vault,
        &day,
        &config,
        "@sase+fix-apollo ~t:tgt|\"implemented by\"",
        &["--require-pull-in-choice"],
    );
    assert_eq!(code, "pull_in_choice_required");
    assert!(
        message.contains("successor offers"),
        "require-choice names the offers: {message}"
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(json["pull_in"]["offer_count"], 1);
    assert_unchanged(&vault, &before);
}

// One item closing two sources with offers shares one number space.
#[test]
fn complete_one_item_two_sources_with_offers() {
    let (_temp, vault, _, config) = setup("bob-cli-complete-two-sources");
    write_file(
        &vault.join("one.md"),
        concat!(
            "- [ ] #task First [id:: one__first] ^first\n",
            "- [?] #task Needs first [dependsOn:: one__first] [id:: one__needs] ^needs-first\n",
        ),
    );
    write_file(
        &vault.join("two.md"),
        concat!(
            "- [ ] #task Second [id:: two__second] ^second\n",
            "- [?] #task Needs second [dependsOn:: two__second] [id:: two__needs] ^needs-second\n",
        ),
    );
    write_file(
        &vault.join("today.md"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () \u{2014} ONE\n",
            "  - [[one#^first]]\n",
            "- [ ] () \u{2014} TWO\n",
            "  - [[two#^second]]\n",
        ),
    );
    let day = vault.join("today.md");

    let json = capture_json_with(
        &vault,
        &day,
        &config,
        "Implementer ~~one:first|\"implemented by\" ~~two:second|\"verified by\"",
        &["-p", "all"],
    );
    let completes = json["relation_completes"].as_array().expect("completes");
    assert_eq!(completes.len(), 2, "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
    assert_eq!(json["pull_in"]["picked_count"], 2);
    let n1 = completes[0]["task_complete"]["unblocked"][0]["offer"]["n"]
        .as_u64()
        .expect("n1");
    let n2 = completes[1]["task_complete"]["unblocked"][0]["offer"]["n"]
        .as_u64()
        .expect("n2");
    assert_ne!(n1, n2, "nested completions share one number space: {json}");
}

// Mixed `!` and relation offers share draft numbering.
#[test]
fn complete_mixed_bang_and_relation_offers() {
    let (_temp, vault, _, config) = setup("bob-cli-complete-mixed-bang");
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n",
            "- [?] #task Re-launch [dependsOn:: fix-apollo] [id:: relaunch] ^relaunch\n",
        ),
    );
    write_file(
        &vault.join("other.md"),
        concat!(
            "- [ ] #task Spec [id:: other__spec] ^spec\n",
            "- [?] #task Needs spec [dependsOn:: other__spec] [id:: other__needs] ^needs-spec\n",
        ),
    );
    write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
    write_file(
        &vault.join("today.md"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () \u{2014} FIX\n",
            "  - [[sase#^fix-apollo]]\n",
            "- [ ] () \u{2014} SPEC\n",
            "  - [[other#^spec]]\n",
        ),
    );
    let day = vault.join("today.md");

    let json = capture_json_with(
        &vault,
        &day,
        &config,
        "!sase:fix-apollo\n\n@other+spec ~t:tgt|\"implemented by\"",
        &["-p", "all"],
    );
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 2);
    assert_eq!(json["pull_in"]["picked_count"], 2);
    assert!(captures[0].get("task_complete").is_some(), "{json}");
    assert!(captures[1].get("relation_completes").is_some(), "{json}");
}

// Cross-item mixed effects refuse in both orders; later staged closed
// status does not mask the contradiction as already_closed.
#[test]
fn complete_cross_item_mixed_effects_conflict() {
    let (_temp, vault, day, config) =
        setup("bob-cli-complete-cross-item-conflict");
    write_file(&vault.join("s.md"), "- [ ] #task Source ^src\n");
    write_file(&vault.join("t1.md"), "- [ ] #task One ^one\n");
    write_file(&vault.join("t2.md"), "- [ ] #task Two ^two\n");
    let before = snapshot(&vault, &["s.md", "t1.md", "t2.md", "day.md"]);

    let (code, message) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@s+src ~t1:one|\"duplicate of\"\n\n@s+src ~t2:two|\"implemented by\"",
    );
    assert_eq!(code, "relation_close_effects_conflict");
    assert!(
        message.contains("can't be both cancelled and completed"),
        "{message}"
    );
    assert_unchanged(&vault, &before);

    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@s+src ~t2:two|\"implemented by\"\n\n@s+src ~t1:one|\"duplicate of\"",
    );
    assert_eq!(code, "relation_close_effects_conflict");
    assert_unchanged(&vault, &before);
}

// Distinct sources may mix cancel and complete in one draft.
#[test]
fn complete_distinct_sources_mixed_cancel_and_complete() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-distinct-mix");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");
    write_file(&vault.join("d.md"), "- [ ] #task Delta ^delta\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"\n\n@c+gamma ~d:delta|\"implemented by\"",
    );
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(
        captures[0]["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    assert_eq!(
        captures[1]["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert!(
        read(&vault, "a.md")
            .starts_with("- [-] #task Alpha [cancelled:: 2026-10-14] ^alpha\n"),
        "{}",
        read(&vault, "a.md")
    );
    assert!(
        read(&vault, "c.md").starts_with(
            "- [x] #task Gamma  [completion:: 2026-10-14] ^gamma\n"
        ),
        "{}",
        read(&vault, "c.md")
    );
}

// A label listed in both lists closes nothing and warns once.
#[test]
fn complete_overlap_label_closes_nothing() {
    let (_temp, vault, day, _) = setup("bob-cli-complete-overlap");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(
        &config,
        concat!(
            "relations:\n",
            "  cancel_labels:\n",
            "    - implemented by\n",
            "  complete_labels:\n",
            "    - implemented by\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert!(json.get("relation_cancels").is_none(), "{json}");
    assert!(json.get("relation_completes").is_none(), "{json}");
    assert!(
        json["relation_updates"][0].get("cancel_effect").is_none(),
        "{json}"
    );
    assert!(
        json["relation_updates"][0].get("complete_effect").is_none(),
        "{json}"
    );
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("both cancel_labels and complete_labels")),
        "overlap warning surfaces: {json}"
    );
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"),
        "the source stays open:\n{}",
        read(&vault, "a.md")
    );
}
