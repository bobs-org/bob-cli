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

fn snapshot_all(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("vault entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(vault)
                    .expect("vault-relative")
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&path).expect("read vault file");
                entries.push((relative, bytes));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn capture_json_with(
    vault: &Path,
    day: &Path,
    config: &Path,
    draft: &str,
    extra: &[&str],
) -> serde_json::Value {
    let before_dry = snapshot_all(vault);
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
    assert_eq!(
        snapshot_all(vault),
        before_dry,
        "dry run must create and write nothing"
    );
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

// Every open status completes; a custom status links only with its date kept.
#[test]
fn complete_open_statuses_complete_and_custom_links_only() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-statuses");
    write_file(
        &vault.join("a.md"),
        concat!(
            "- [ ] #task Ready [id:: a__ready] ^ready\n",
            "- [*] #task Next [id:: a__next] ^next\n",
            "- [/] #task Prog [id:: a__prog] ^prog\n",
            "- [?] #task Blocked [id:: a__blocked] ^blocked\n",
            "- [>] #task Custom ^custom\n",
        ),
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    for (block, symbol) in [
        ("ready", " "),
        ("next", "*"),
        ("prog", "/"),
        ("blocked", "?"),
    ] {
        let json = capture_json(
            &vault,
            &day,
            &config,
            &format!("@a+{block} ~b:beta|\"implemented by\""),
        );
        assert_eq!(
            json["relation_updates"][0]["complete_effect"], "completed",
            "{block}: {json}"
        );
        let complete = &json["relation_completes"][0];
        assert_eq!(complete["previous_status"], symbol, "{block}: {complete}");
        assert_eq!(
            complete["task_complete"]["action"], "completed",
            "{block}: {complete}"
        );
    }
    let note = read(&vault, "a.md");
    for block in ["ready", "next", "prog", "blocked"] {
        assert!(
            note.contains("- [x] #task"),
            "open source {block} is Done:\n{note}"
        );
    }

    let custom = capture_json(
        &vault,
        &day,
        &config,
        "@a+custom ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        custom["relation_updates"][0]["complete_effect"], "already_closed",
        "{custom}"
    );
    assert!(custom.get("relation_completes").is_none(), "{custom}");
    let note = read(&vault, "a.md");
    assert!(
        note.contains("- [>] #task Custom ^custom\n"),
        "custom status keeps its bytes:\n{note}"
    );
}

// Relabel into a complete label completes; relabel away leaves Done;
// an unchanged retry (even after a manual reopen) never completes;
// inverse labels never complete.
#[test]
fn complete_relabel_unchanged_inverse() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-relabel");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let plain = capture_json(&vault, &day, &config, "@a+alpha ~b:beta|see");
    assert!(plain.get("relation_completes").is_none(), "{plain}");
    assert!(read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"));

    let relabel = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert_eq!(relabel["relation_updates"][0]["action"], "relabeled");
    assert_eq!(
        relabel["relation_updates"][0]["complete_effect"], "completed",
        "{relabel}"
    );
    assert!(read(&vault, "a.md").starts_with("- [x] #task Alpha"));

    capture_json(&vault, &day, &config, "@a+alpha ~b:beta|see");
    assert!(
        read(&vault, "a.md").starts_with("- [x] #task Alpha"),
        "relabel away leaves the source Done:\n{}",
        read(&vault, "a.md")
    );

    // Manually reopen with the same edge still present: an unchanged
    // retry must not complete again.
    let reopened = read(&vault, "a.md").replacen(
        "- [x] #task Alpha",
        "- [ ] #task Alpha",
        1,
    );
    write_file(&vault.join("a.md"), &reopened);
    let retry = capture_json(&vault, &day, &config, "@a+alpha ~b:beta|see");
    assert_eq!(retry["relation_updates"][0]["action"], "unchanged");
    assert!(retry.get("relation_completes").is_none(), "{retry}");
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha"),
        "unchanged retry leaves the reopened source open:\n{}",
        read(&vault, "a.md")
    );

    // Inverse labels never complete.
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");
    let inverse =
        capture_json(&vault, &day, &config, "@a+alpha ~c:gamma|implements");
    assert!(inverse.get("relation_completes").is_none(), "{inverse}");
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha"),
        "inverse label leaves the source open:\n{}",
        read(&vault, "a.md")
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

// A `^prj` source completes like `!`: residents stay open and the copy
// names how many.
#[test]
fn complete_prj_source_keeps_residents() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-prj");
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

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@s+prj ~b:beta|\"implemented by\"",
    );
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["project_open_tasks"], 2, "{complete}");
    assert!(
        complete["message"]
            .as_str()
            .unwrap_or_default()
            .contains("its 2 open tasks stay open"),
        "{complete}"
    );

    let note = read(&vault, "s.md");
    assert!(
        note.contains("- [x] #task Ship it [id:: s__prj]"),
        "the project completes:\n{note}"
    );
    assert!(
        note.contains("- [ ] #task Resident one ^res-one\n"),
        "residents stay open:\n{note}"
    );
}

// Embedded subtasks follow `!` rules through the shared Done path.
#[test]
fn complete_embedded_subtasks_follow_bang() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-subtasks");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Root [id:: sase__root] ^root\n\t- ![[sase#^sub1]]\n\t- ![[sase#^sub2]]\n- [/] #task Sub one ^sub1\n- [?] #task Sub two ^sub2\n\t- ![[sase#^sub3]]\n- [ ] #task Sub three ^sub3\n",
    );
    write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@sase+root ~t:tgt|\"implemented by\"",
    );
    let complete = &json["relation_completes"][0];
    let subtasks = complete["task_complete"]["subtasks"]
        .as_array()
        .expect("subtasks");
    assert_eq!(subtasks.len(), 1, "{complete}");
    assert_eq!(subtasks[0]["block_id"], "sub1");
    assert!(
        read(&vault, "sase.md").contains("- [ ] #task Sub three ^sub3\n"),
        "the task below a Blocked subtask stays open:\n{}",
        read(&vault, "sase.md")
    );
}

// Daily-note source, missing daily note, mirror kill switch, hidden
// labels, midnight dates, and CRLF preservation.
#[test]
fn complete_locations_mirror_hidden_midnight_crlf() {
    // Source in today's daily note: one staged file holds every write.
    let (_temp, vault, _, config) = setup("bob-cli-complete-daily-source");
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
        "@day+daytask ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert_eq!(
        json["relation_completes"][0]["task_complete"]["ledger"]["struck"],
        1
    );
    assert!(
        read(&vault, "day.md").contains("- [x] #task Day task"),
        "daily source completes in place:\n{}",
        read(&vault, "day.md")
    );

    // No daily note on disk: the complete still stages, no note created.
    let (_temp, vault, _, config) = setup("bob-cli-complete-no-day");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let missing = vault.join("2026/20261014.md");
    let json = capture_json(
        &vault,
        &missing,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert!(!missing.exists(), "capture never creates the daily note");

    // Mirror kill switch skips only the mirror.
    let (_temp, vault, day, _) = setup("bob-cli-complete-no-mirror");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  linked_from_lines: false\n  complete_labels:\n    - implemented by\n",
    );
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert_eq!(json["mirror_skipped"][0]["reason"], "disabled", "{json}");
    assert_eq!(
        read(&vault, "b.md"),
        "- [ ] #task Beta ^beta\n",
        "the mirror stays untouched"
    );

    // Hidden labels still complete.
    let (_temp, vault, day, config) = setup("bob-cli-complete-hidden");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let mut hide = bob_command();
    hide.arg("relations")
        .arg("hide")
        .arg("-b")
        .arg(&vault)
        .arg("--allow-unused")
        .arg("--")
        .arg("implemented by");
    hide.env("BOB_CONFIG_FILE", &config);
    let hide = hide.output().expect("hide the complete label");
    assert_success(&hide);
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_completes"][0]["message"],
        "✓ completed “Alpha” · Todo → Done",
        "a hidden complete label still completes: {json}"
    );

    // Midnight dates follow BOB_NOW.
    for (name, now, expected) in [
        (
            "bob-cli-complete-midnight-before",
            "2026-10-14 23:59:00",
            "2026-10-14",
        ),
        (
            "bob-cli-complete-midnight-after",
            "2026-10-15 00:05:00",
            "2026-10-15",
        ),
    ] {
        let (_temp, vault, day, config) = setup(name);
        write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
        write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("@a+alpha ~b:beta|\"implemented by\"");
        command
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", now)
            .env("BOB_CONFIG_FILE", &config);
        let output = command.output().expect("run capture");
        assert_success(&output);
        assert!(
            read(&vault, "a.md")
                .contains(&format!("[completion:: {expected}]")),
            "BOB_NOW {now} completes with {expected}:\n{}",
            read(&vault, "a.md")
        );
    }

    // CRLF preservation.
    let (_temp, vault, day, config) = setup("bob-cli-complete-crlf");
    fs::write(
        vault.join("a.md"),
        "- [ ] #task Alpha ^alpha\r\n- [ ] #task Beta ^beta\r\n",
    )
    .expect("write CRLF note");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    let raw = fs::read(vault.join("a.md")).expect("read raw note");
    assert!(
        raw.windows(2).any(|w| w == b"\r\n"),
        "CRLF line endings survive the complete"
    );
}

// The same fixture via `!` and via relation yields matching Done bytes,
// allowing only the extra relation edits and documented token differences.
#[test]
fn complete_matches_bang_bytes() {
    let build = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        write_blocked_tasks_settings(&vault);
        let day = vault.join("day.md");
        write_file(&day, DAY);
        let config = vault.join("config.yml");
        write_file(&config, CONFIG);
        write_file(
            &vault.join("cash.md"),
            concat!(
                "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
                "- [?] #task Needs stocks [dependsOn:: cash__sell-stocks] [id:: cash__needs] ^needs-stocks\n",
            ),
        );
        write_file(
            &vault.join("today.md"),
            concat!(
                "## Pomodoros\n",
                "- [ ] Morning 0900-1000\n",
                "  - [[cash#^sell-stocks]]\n",
            ),
        );
        (temp, vault, day, config)
    };

    let (_t1, v1, d1, c1) = build("bob-cli-complete-parity-bang");
    let bang =
        capture_json(&v1, &v1.join("today.md"), &c1, "!cash:sell-stocks");
    let bang_line = read(&v1, "cash.md")
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();

    let (_t2, v2, d2, c2) = build("bob-cli-complete-parity-rel");
    write_file(
        &v2.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-stocks-now\n",
    );
    let rel = capture_json(
        &v2,
        &v2.join("today.md"),
        &c2,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"implemented by\"",
    );
    let rel_line = read(&v2, "cash.md")
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let strip_links = |line: &str| {
        line.replace("  [completion:: 2026-10-14]", "")
            .replace("[completion:: 2026-10-14]", "")
    };
    // Both close the same task on the same date; the relation run adds
    // only the LINKS child (checked on later lines).
    assert!(
        bang_line.contains("- [x] #task Sell stocks")
            && bang_line.contains("[completion:: 2026-10-14]"),
        "bang closes the source:\n{bang_line}\n{bang}"
    );
    assert!(
        rel_line.contains("- [x] #task Sell stocks")
            && rel_line.contains("[completion:: 2026-10-14]"),
        "relation closes the same bytes:\n{rel_line}\n{rel}"
    );
    assert_eq!(
        strip_links(&bang_line)
            .replace("  ", " ")
            .trim()
            .to_string(),
        strip_links(&rel_line).replace("  ", " ").trim().to_string(),
        "Done bytes match apart from spacing"
    );
    let _ = (d1, d2);
}

// No config means a plain link; cancel-only config proves the shipped
// cancel path still works and new fields stay absent.
#[test]
fn complete_no_config_and_cancel_only_compat() {
    // No complete labels configured: same draft is a plain link.
    let temp = TempDir::new("bob-cli-complete-inert");
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
        command.arg("--").arg("@a+alpha ~b:beta|\"implemented by\"");
        command.env("BOB_DAY_FILE", &day).env("BOB_NOW", NOW);
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    let real: serde_json::Value = run(false);
    assert!(dry.get("relation_completes").is_none(), "{dry}");
    assert!(dry.get("warnings").is_none(), "{dry}");
    assert!(
        dry["relation_updates"][0].get("complete_effect").is_none(),
        "{dry}"
    );
    assert!(real.get("relation_completes").is_none(), "{real}");
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"),
        "the source stays open:\n{}",
        read(&vault, "a.md")
    );
    // Human output for the inert draft names the link and never the ✓
    // complete row; the dry run writes nothing.
    let before = snapshot_all(&vault);
    let missing_config = vault.join("missing.yml");
    let human = capture_human(
        &vault,
        &day,
        &missing_config,
        "@a+alpha ~b:beta|\"implemented by\"",
        true,
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        !text.contains("completed \u{201c}")
            && !text.contains("would complete \u{201c}"),
        "no-config human never completes:\n{text}"
    );
    assert_eq!(snapshot_all(&vault), before, "dry human writes nothing");

    // Cancel-only config: the shipped cancel path is unchanged and no
    // complete fields appear.
    let (_temp, vault, day, _) = setup("bob-cli-complete-cancel-only");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n",
    );
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
    );
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    assert!(json.get("relation_completes").is_none(), "{json}");
    assert!(
        json["relation_updates"].as_array().unwrap()[0]
            .get("complete_effect")
            .is_none(),
        "{json}"
    );
    // Cancel-only human output keeps the shipped ⊘ row and never gains a
    // ✓ complete row or a complete field.
    let (_temp, vault, day, _) = setup("bob-cli-complete-cancel-human");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let config = vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n",
    );
    let human = capture_human(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"duplicate of\"",
        false,
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains('⊘'), "cancel-only human keeps ⊘:\n{text}");
    assert!(
        !text.contains("completed \u{201c}")
            && !text.contains("would complete \u{201c}"),
        "cancel-only human never completes:\n{text}"
    );
}

// Same-note closing edges keep one source when mirror staging shifts the
// stable block's line: the first mirror insertion moves the source before
// the second edge is recorded, but grouping is by path and block ID.
#[test]
fn complete_same_note_shifted_source_groups_once() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-same-shift");
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "- [ ] #task First ^first\n",
            "- [ ] #task Second ^second\n",
            "- [ ] #task Source ^source\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@tasks+source ~tasks:first|\"implemented by\" ~tasks:second|\"verified by\"",
    );
    let completes = json["relation_completes"].as_array().expect("completes");
    assert_eq!(completes.len(), 1, "one source completes once: {json}");
    let complete = &completes[0];
    assert_eq!(
        complete["labels"],
        serde_json::json!(["implemented by", "verified by"]),
        "labels stay in authored order: {complete}"
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert_eq!(
        json["relation_updates"][1]["complete_effect"], "completed",
        "{json}"
    );
    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 2, "one warning per label: {json}");
    assert!(
        warnings[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("First"),
        "first warning names First: {warnings:?}"
    );
    assert!(
        warnings[1]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Second"),
        "second warning names Second: {warnings:?}"
    );

    let note = read(&vault, "tasks.md");
    assert!(
        note.contains(
            "- [x] #task Source  [completion:: 2026-10-14] ^source\n"
        ),
        "one Done execution:\n{note}"
    );
    assert!(
        note.contains("implemented by [[#^first]]")
            && note.contains("verified by [[#^second]]"),
        "both labels survive on the completed source:\n{note}"
    );
}

// Mixed-direction items with the same label keep each warning on its own
// edge: Alpha completes toward Beta and Gamma completes toward Alpha, so
// Gamma's warning must name Alpha rather than recovering Beta by label.
#[test]
fn complete_mixed_direction_warnings_keep_their_targets() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-mixed-warn");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(&vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");

    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\" ~~c:gamma|\"implemented by\"",
    );
    let completes = json["relation_completes"].as_array().expect("completes");
    assert_eq!(completes.len(), 2, "{json}");
    assert_eq!(completes[0]["source"]["text"], "Alpha");
    assert_eq!(completes[0]["target"]["text"], "Beta");
    assert_eq!(completes[1]["source"]["text"], "Gamma");
    assert_eq!(completes[1]["target"]["text"], "Alpha");

    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 2, "{json}");
    assert!(
        warnings[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("“Beta”"),
        "Alpha warns toward Beta: {warnings:?}"
    );
    assert!(
        warnings[1]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("“Alpha”"),
        "Gamma warns toward Alpha, not Beta: {warnings:?}"
    );

    for dry in [true, false] {
        let (_temp, vault, day, config) = setup("bob-cli-complete-mixed-human");
        write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
        write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
        let output = capture_human(
            &vault,
            &day,
            &config,
            "@a+alpha ~b:beta|\"implemented by\"",
            dry,
        );
        assert_success(&output);
        let text = stdout(&output);
        assert!(
            text.contains("✓"),
            "human single-row names the complete (dry={dry}):\n{text}"
        );
    }
}

// Reconcile never infers Done, and a late invalid item rolls the whole
// batch back.
#[test]
fn complete_reconcile_never_infers_and_batch_atomic() {
    // A LINKS line already carrying a complete label stays open until an
    // explicit gesture: reconcile must not complete it.
    let (_temp, vault, day, config) = setup("bob-cli-complete-reconcile");
    write_file(
        &vault.join("a.md"),
        "- [ ] #task Alpha ^alpha\n\t- 🪢 **LINKS:** implemented by [[b#^beta]]\n",
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let mut reconcile = bob_command();
    reconcile
        .arg("task")
        .arg("reconcile")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json");
    reconcile
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = reconcile.output().expect("run reconcile");
    assert_success(&output);
    assert!(
        read(&vault, "a.md").starts_with("- [ ] #task Alpha ^alpha\n"),
        "reconcile never completes:\n{}",
        read(&vault, "a.md")
    );

    // A late invalid item refuses the whole batch atomically.
    let (_temp, vault, day, config) = setup("bob-cli-complete-atomic");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let before = snapshot(&vault, &["a.md", "b.md", "day.md"]);
    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "@a+alpha ~b:beta|\"implemented by\"\n\nNew thing ~a:alpha|\"implemented by\"",
    );
    assert_eq!(code, "relation_completes_new_task");
    assert_unchanged(&vault, &before);
}

// #ref sources, both same-note orders, archived and Cancelled targets:
// the source still completes while the target keeps its status and bytes.
#[test]
fn complete_ref_same_note_archived_cancelled_targets() {
    // #ref-tagged source completes like any other open task.
    let (_temp, vault, day, config) = setup("bob-cli-complete-ref-source");
    write_file(
        &vault.join("a.md"),
        "- [ ] #task #ref Read paper ^ref-read\n",
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+ref-read ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert!(
        read(&vault, "a.md")
            .contains("- [x] #task #ref Read paper  [completion:: 2026-10-14] ^ref-read\n"),
        "{}",
        read(&vault, "a.md")
    );

    // Same-note source above its targets, then below them: both complete.
    for (name, contents) in [
        (
            "bob-cli-complete-same-above",
            "- [ ] #task Source ^source\n- [ ] #task First ^first\n- [ ] #task Second ^second\n",
        ),
        (
            "bob-cli-complete-same-below",
            "- [ ] #task First ^first\n- [ ] #task Second ^second\n- [ ] #task Source ^source\n",
        ),
    ] {
        let (_temp, vault, day, config) = setup(name);
        write_file(&vault.join("tasks.md"), contents);
        let json = capture_json(
            &vault,
            &day,
            &config,
            "@tasks+source ~tasks:first|\"implemented by\"",
        );
        assert_eq!(
            json["relation_updates"][0]["complete_effect"], "completed",
            "{name}: {json}"
        );
        assert!(
            read(&vault, "tasks.md").contains(
                "- [x] #task Source  [completion:: 2026-10-14] ^source\n"
            ),
            "{name}:\n{}",
            read(&vault, "tasks.md")
        );
    }

    // Archived targets keep the mirror skipped and their own bytes.
    let (_temp, vault, day, config) = setup("bob-cli-complete-archived");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    fs::create_dir_all(vault.join("done")).expect("create done");
    write_file(&vault.join("done/b.md"), "- [x] #task Old done ^old\n");
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~done/b:old|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert_eq!(json["mirror_skipped"][0]["reason"], "archived_target");
    assert_eq!(
        read(&vault, "done/b.md"),
        "- [x] #task Old done ^old\n",
        "archived target is untouched"
    );
    assert!(
        read(&vault, "a.md").starts_with("- [x] #task Alpha"),
        "source completes:\n{}",
        read(&vault, "a.md")
    );

    // Cancelled targets are allowed: the source completes, the target date
    // and status stay exactly as they were.
    let (_temp, vault, day, config) = setup("bob-cli-complete-cancelled");
    write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(
        &vault.join("c.md"),
        "- [-] #task Gone [cancelled:: 2026-10-01] ^gone\n",
    );
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@a+alpha ~c:gone|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    assert_eq!(
        read(&vault, "c.md"),
        "- [-] #task Gone [cancelled:: 2026-10-01] ^gone\n\t- 🪢 **LINKED FROM:** [[a#^alpha]] (implemented by)\n",
        "cancelled target keeps status/date, gains only the mirror:\n{}",
        read(&vault, "c.md")
    );
}

// Typed `||` completes, one-off hides ride only on the `~~` success path,
// an existing completion date is preserved, all recurrence spellings
// refuse, and a modifier on the new `~~` target is not a source conflict.
#[test]
fn complete_oneoff_hide_date_recurrence_new_target() {
    // `||` on the `~~` form completes and records one hide; dry runs plan
    // the hide but write no journal.
    let (_temp, vault, day, config) = setup("bob-cli-complete-oneoff");
    write_file(
        &vault.join("sase_blog.md"),
        "- [ ] #task Write introduction [id:: sase_blog__write-intro] ^write-intro\n",
    );
    let mut dry = bob_command();
    dry.arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("Write v2 intro ~~sase_blog:write-intro||\"implemented by\"");
    dry.env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = dry.output().expect("dry capture");
    assert_success(&output);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(
        dry_json["relation_updates"][0]["complete_effect"], "completed",
        "{dry_json}"
    );
    assert_eq!(
        dry_json["relation_label_hides"][0]["label"], "implemented by",
        "{dry_json}"
    );
    assert!(
        !vault.join(".bob").exists(),
        "dry run writes no hide journal"
    );
    let json = capture_json(
        &vault,
        &day,
        &config,
        "Write v2 redo ~~sase_blog:write-intro||\"implemented by\"",
    );
    assert_eq!(json["relation_label_hides"][0]["action"], "hide", "{json}");

    // A failed batch with a one-off label writes no hide journal event:
    // the whole commit refuses and `.bob` never appears.
    let (_temp, vault, day, config) = setup("bob-cli-complete-oneoff-fail");
    write_file(&vault.join("s.md"), "- [ ] #task Source ^src\n");
    write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
    let before = snapshot_all(&vault);
    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New intro ~~s:src||\"implemented by\"\n\nNew thing ~s:src|\"implemented by\"",
    );
    assert_eq!(code, "relation_completes_new_task");
    assert_eq!(snapshot_all(&vault), before, "failed batch writes nothing");
    assert!(
        !vault.join(".bob").exists(),
        "no hide journal event on refusal"
    );

    // An existing completion date on an already-Done source is preserved.
    let (_temp, vault, day, config) = setup("bob-cli-complete-keep-date");
    write_file(
        &vault.join("a.md"),
        "- [x] #task Done [completion:: 2026-10-01] ^done\n",
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
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
    assert!(
        read(&vault, "a.md").contains("[completion:: 2026-10-01]"),
        "original date survives:\n{}",
        read(&vault, "a.md")
    );
    assert!(
        !read(&vault, "a.md").contains("2026-10-14"),
        "no new date is stamped:\n{}",
        read(&vault, "a.md")
    );

    // Every supported recurrence spelling refuses with the coded hint.
    for (name, line) in [
        (
            "bob-cli-complete-recur-bracket",
            "- [ ] #task R [repeat:: every week] ^r\n",
        ),
        (
            "bob-cli-complete-recur-paren",
            "- [ ] #task R (repeat:: every week) ^r\n",
        ),
        ("bob-cli-complete-recur-emoji", "- [ ] #task R 🔁 ^r\n"),
    ] {
        let (_temp, vault, day, config) = setup(name);
        write_file(&vault.join("a.md"), line);
        write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
        let before = snapshot(&vault, &["a.md", "b.md", "day.md"]);
        let (code, message) = capture_fail_code(
            &vault,
            &day,
            &config,
            "@a+r ~b:beta|\"implemented by\"",
        );
        assert_eq!(code, "relation_completes_recurring_task", "{name}");
        assert!(message.contains("repeats"), "{name}: {message}");
        assert_unchanged(&vault, &before);
    }

    // A `*` naming the picked `~~` source conflicts; the `@note+id`
    // existing-owner form itself is the relation and never conflicts.
    let (_temp, vault, day, config) = setup("bob-cli-complete-newtgt");
    write_file(&vault.join("s.md"), "- [ ] #task Source ^src\n");
    write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
    let before = snapshot_all(&vault);
    let (code, _) = capture_fail_code(
        &vault,
        &day,
        &config,
        "New thing ~~s:src|\"implemented by\" *s:src",
    );
    assert_eq!(code, "relation_complete_conflicts_with_item_action");
    assert_eq!(snapshot_all(&vault), before, "conflict writes nothing");

    // Positive case: a modifier on the new `~~` target is not a source
    // conflict — the dependency lands on the new task and the picked
    // source still completes.
    let (_temp, vault, day, config) = setup("bob-cli-complete-newtgt-ok");
    write_file(&vault.join("s.md"), "- [ ] #task Source ^src\n");
    write_file(&vault.join("t.md"), "- [ ] #task Dep ^dep\n");
    let json = capture_json(
        &vault,
        &day,
        &config,
        "New task ~~s:src|\"implemented by\" &t:dep",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );

    // Ordinary whole-item grammar refusals keep their own codes: an `!`
    // with a suffix fails as a malformed `!`, never as a closing-label
    // conflict, and the `@note+id` owner form stays the relation itself.
    let (_temp, vault, day, config) = setup("bob-cli-complete-grammar");
    write_file(&vault.join("s.md"), "- [ ] #task Source ^src\n");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let before = snapshot_all(&vault);
    let (code, message) = capture_fail_code(&vault, &day, &config, "!s:src=x");
    assert_ne!(code, "relation_complete_conflicts_with_item_action");
    assert!(
        message.contains("must be the whole capture item"),
        "ordinary `!` grammar refusal: {message}"
    );
    assert_eq!(
        snapshot_all(&vault),
        before,
        "grammar refusal writes nothing"
    );
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@s+src ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "the `@note+id` owner form is the relation itself: {json}"
    );
}

// Numeric probe/rerun and stable-key picks agree; invalid/stale picks and
// required-choice refuse without writes; later items filter the offer set;
// unpicked recovery never gains a link.
#[test]
fn complete_offer_probe_rerun_keys_and_net_filter() {
    let build = |name: &str| {
        let (temp, vault, _, config) = setup(name);
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
        write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
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
        (temp, vault, day, config)
    };
    let draft = "Implementer ~~one:first|\"implemented by\" ~~two:second|\"verified by\"";

    // Probe with -p all: two offers share one stable number space.
    let (_t, vault, day, config) = build("bob-cli-complete-probe");
    let probed =
        capture_json_with(&vault, &day, &config, draft, &["-p", "all"]);
    assert_eq!(probed["pull_in"]["offer_count"], 2);
    let completes = probed["relation_completes"]
        .as_array()
        .expect("relation_completes");
    let first_unblocked = &completes[0]["task_complete"]["unblocked"][0];
    let second_unblocked = &completes[1]["task_complete"]["unblocked"][0];
    let first_n = first_unblocked["offer"]["n"]
        .as_u64()
        .expect("first offer number");
    let second_n = second_unblocked["offer"]["n"]
        .as_u64()
        .expect("second offer number");
    assert_ne!(first_n, second_n, "one number space: {probed}");
    assert_eq!(
        first_unblocked["offer"]["key"], "one.md#^needs-first",
        "{probed}"
    );
    assert_eq!(
        second_unblocked["offer"]["key"], "two.md#^needs-second",
        "{probed}"
    );

    // Numeric rerun selects the same link as the probe.
    let (_t, vault, day, config) = build("bob-cli-complete-rerun");
    let rerun = capture_json_with(
        &vault,
        &day,
        &config,
        draft,
        &["-p", &first_n.to_string()],
    );
    assert_eq!(rerun["pull_in"]["picked_count"], 1, "{rerun}");
    let rerun_completes =
        rerun["relation_completes"].as_array().expect("completes");
    assert_eq!(
        rerun_completes[0]["task_complete"]["unblocked"][0]["offer"]
            ["selected"],
        true
    );
    assert_eq!(
        rerun_completes[0]["task_complete"]["unblocked"][0]["block_id"],
        "needs-first"
    );
    assert_eq!(
        rerun_completes[1]["task_complete"]["unblocked"][0]["not_linked"],
        "not_selected"
    );

    // Stable-key pick agrees with the probe on keys, ordering, and links.
    let (_t, vault, day, config) = build("bob-cli-complete-skey");
    let by_key = capture_json_with(
        &vault,
        &day,
        &config,
        draft,
        &["-p", "one.md#^needs-first"],
    );
    assert_eq!(
        by_key["pull_in"]["picked_count"], 1,
        "stable-key pick selects one: {by_key}"
    );
    let keyed = by_key["relation_completes"].as_array().expect("completes");
    assert_eq!(
        keyed[0]["task_complete"]["unblocked"][0]["offer"]["selected"],
        true
    );
    assert_eq!(
        keyed[0]["task_complete"]["unblocked"][0]["offer"]["n"], first_n,
        "stable-key pick keeps the probe number: {by_key}"
    );

    // Invalid and stale picks refuse without writing anything.
    for (name, pick) in [
        ("bob-cli-complete-bad-pick", "99"),
        ("bob-cli-complete-stale-key", "missing.md#^nope"),
    ] {
        let (_t, vault, day, config) = build(name);
        let before = snapshot_all(&vault);
        let (code, message, _) =
            capture_fail_code_with(&vault, &day, &config, draft, &["-p", pick]);
        assert!(
            message.contains("pull-in pick"),
            "{name}: invalid pick names the pick: {message} ({code})"
        );
        assert_eq!(snapshot_all(&vault), before, "{name}: writes nothing");
    }

    // Required-choice without an answer refuses without writes.
    let (_t, vault, day, config) = build("bob-cli-complete-require2");
    let before = snapshot_all(&vault);
    let (code, _, _) = capture_fail_code_with(
        &vault,
        &day,
        &config,
        draft,
        &["--require-pull-in-choice"],
    );
    assert_eq!(code, "pull_in_choice_required");
    assert_eq!(
        snapshot_all(&vault),
        before,
        "require-choice writes nothing"
    );

    // A later item that completes the offered task filters it out of the
    // final set; the survivor keeps its link reason and unpicked recovery
    // never gains a Task Link.
    let (_t, vault, day, config) = build("bob-cli-complete-netfilter");
    let json = capture_json_with(
        &vault,
        &day,
        &config,
        &format!("{draft}\n\n!one:needs-first"),
        &["-p", "all"],
    );
    let today = read(&vault, "today.md");
    assert!(
        !today.contains("[[one#^needs-first]]")
            || json["pull_in"]["offer_count"].as_u64().unwrap_or(2) < 2,
        "closed offer leaves the final set: {json}\n{today}"
    );
    let none = capture_json_with(&vault, &day, &config, draft, &["-p", "none"]);
    assert_eq!(none["pull_in"]["picked_count"], 0);
    let today = read(&vault, "today.md");
    assert!(
        !today.contains("[[one#^needs-first]]")
            || today.contains("~~[[one#^first]]~~"),
        "unpicked recovery gains no link:\n{today}"
    );
}

// A dependent recovered by a complete-label close routes out of its inbox:
// explicit destinations move it, keep leaves it, and the guard refuses a
// choiceless batch without writing.
#[test]
fn complete_inbox_routed_recovery() {
    use std::process::Command as StdCommand;
    let build = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        write_blocked_tasks_settings(&vault);
        write_file(&vault.join("inbox.md"), "# Inbox\n");
        write_file(
            &vault.join("dev.md"),
            "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Prereq [id:: dev__prereq] ^prereq\n",
        );
        write_file(
            &vault.join("mac_inbox.md"),
            "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n- [?] #task Inbox dependent [dependsOn:: dev__prereq] [id:: mac_inbox__dep] ^dep\n",
        );
        write_file(&vault.join("t.md"), "- [ ] #task Target ^tgt\n");
        write_file(
            &vault.join("health.md"),
            "---\ntype: [[area]]\n---\n## Tasks\n",
        );
        let day = vault.join("20261024.md");
        write_file(&day, "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n");
        let config = vault.join("config.yml");
        write_file(&config, CONFIG);
        (temp, vault, day, config)
    };
    let run = |vault: &Path,
               day: &Path,
               config: &Path,
               extra: &[&str],
               draft: &str|
     -> serde_json::Value {
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
            .env("BOB_DAY_FILE", day)
            .env("BOB_NOW", "2026-10-24 09:30:00")
            .env("BOB_CONFIG_FILE", config);
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };

    // Explicit destination: the recovered inbox task moves home with a
    // repaired link, while the successor offer is picked.
    let (_t, vault, day, config) = build("bob-cli-complete-inbox-move");
    let json = run(
        &vault,
        &day,
        &config,
        &["-p", "all", "-i", "1=health"],
        "@dev+prereq ~t:tgt|\"implemented by\"",
    );
    let candidates = json["inbox_route"]["candidates"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 1, "{json}");
    assert_eq!(candidates[0]["outcome"], "moved", "{candidates:?}");
    assert_eq!(candidates[0]["moved"]["destination"], "health.md");
    let health = read(&vault, "health.md");
    assert!(health.contains("^dep"), "routed task bytes:\n{health}");
    assert!(
        !read(&vault, "mac_inbox.md").contains("^dep"),
        "inbox no longer holds it"
    );
    assert!(
        json["pull_in"]["picked_count"].as_u64().unwrap_or(0) > 0,
        "successor picked alongside the route: {json}"
    );

    // Explicit keep: the recovered task stays with its stamps intact.
    let (_t, vault, day, config) = build("bob-cli-complete-inbox-keep");
    let json = run(
        &vault,
        &day,
        &config,
        &["-p", "all", "-i", "keep"],
        "@dev+prereq ~t:tgt|\"implemented by\"",
    );
    assert_eq!(
        json["inbox_route"]["candidates"][0]["outcome"], "kept",
        "{json}"
    );
    assert!(
        read(&vault, "mac_inbox.md").contains("^dep"),
        "kept task stays in the inbox"
    );

    // No route answer with a pending candidate refuses without writing.
    let (_t, vault, day, config) = build("bob-cli-complete-inbox-guard");
    let before = snapshot_all(&vault);
    let mut guard = StdCommand::new(bob_command().get_program());
    guard
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--require-inbox-route-choice")
        .arg("--")
        .arg("@dev+prereq ~t:tgt|\"implemented by\"");
    guard
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", "2026-10-24 09:30:00")
        .env("BOB_CONFIG_FILE", &config);
    let output = guard.output().expect("guard capture");
    assert!(!output.status.success(), "guard refuses");
    assert_eq!(snapshot_all(&vault), before, "guard writes nothing");
}

// A composed relation completion preserves an authored theme field byte
// for byte, exactly as the shared Done engine does for `!`.
#[test]
fn complete_relation_preserves_authored_theme() {
    let (_temp, vault, day, config) = setup("bob-cli-complete-theme");
    write_file(
        &vault.join("s.md"),
        "- [ ] #task Ship it (theme::  WRITE) ^ship\n",
    );
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    let json = capture_json(
        &vault,
        &day,
        &config,
        "@s+ship ~b:beta|\"implemented by\"",
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    let note = read(&vault, "s.md");
    assert!(
        note.contains("- [x] #task Ship it (theme::  WRITE)  [completion:: 2026-10-14] ^ship\n"),
        "authored theme survives the composed close:\n{note}"
    );
    assert_eq!(
        note.matches("(theme::  WRITE)").count(),
        1,
        "theme appears exactly once:\n{note}"
    );
}

// A mixed `!`, `x!`, and relation draft shares one offer number space:
// the task completion, the pomodoro close, and the relation close all
// report into the same pull-in set.
#[test]
fn complete_mixed_bang_pomodoro_relation_offers() {
    let (_temp, vault, day_file) =
        super::pomodoro_close::close_worked_vault("bob-cli-complete-mixed-x");
    write_file(&vault.join("config.yml"), CONFIG);
    write_file(
        &vault.join("extra.md"),
        "- [ ] #task Extra ^extra\n- [ ] #task Tgt ^tgt\n",
    );
    let config = vault.join("config.yml");
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-p")
        .arg("all")
        .arg("--")
        .arg("!bob:ready\n\nx!1\n\n@extra+extra ~extra:tgt|\"implemented by\"");
    command
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("mixed capture");
    assert_success(&output);
    let mixed: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("mixed JSON");
    let captures = mixed["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 3, "three items commit together: {mixed}");
    assert!(captures[0].get("task_complete").is_some(), "{mixed}");
    assert!(captures[1].get("pomodoro_close").is_some(), "{mixed}");
    assert!(captures[2].get("relation_completes").is_some(), "{mixed}");
    let offer_count = mixed["pull_in"]["offer_count"].as_u64().unwrap_or(0);
    let picked_count = mixed["pull_in"]["picked_count"].as_u64().unwrap_or(0);
    assert_eq!(
        offer_count, picked_count,
        "-p all picks every shared offer: {mixed}"
    );
}

// Composed `!` parity across tree, ledger, recovery, and offers; exact CRLF
// bytes; and midnight dates under a configured New York day zone.
#[test]
fn complete_bang_parity_ledger_crlf_midnight_ny() {
    // Same fixture via `!` and via relation: tree, ledger, recovery, and
    // offer results agree, not only the root Done line.
    let build = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        write_blocked_tasks_settings(&vault);
        let day = vault.join("day.md");
        write_file(&day, DAY);
        let config = vault.join("config.yml");
        write_file(&config, CONFIG);
        write_file(
            &vault.join("cash.md"),
            concat!(
                "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
                "- [?] #task Needs stocks [dependsOn:: cash__sell-stocks] [id:: cash__needs] ^needs-stocks\n",
            ),
        );
        write_file(
            &vault.join("today.md"),
            concat!(
                "## Pomodoros\n",
                "- [ ] Morning 0900-1000\n",
                "  - [[cash#^sell-stocks]]\n",
            ),
        );
        (temp, vault, day, config)
    };
    let (_t1, v1, _, c1) = build("bob-cli-complete-parity-full-bang");
    let bang =
        capture_json(&v1, &v1.join("today.md"), &c1, "!cash:sell-stocks");
    let (_t2, v2, _, c2) = build("bob-cli-complete-parity-full-rel");
    write_file(
        &v2.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-stocks-now\n",
    );
    let rel = capture_json(
        &v2,
        &v2.join("today.md"),
        &c2,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"implemented by\"",
    );
    let bang_complete = &bang["task_complete"];
    let rel_complete = &rel["relation_completes"][0]["task_complete"];
    for field in ["action", "block_id", "completion_date", "text"] {
        assert_eq!(
            bang_complete[field], rel_complete[field],
            "task_complete.{field} agrees: {bang_complete} vs {rel_complete}"
        );
    }
    assert_eq!(
        bang_complete["ledger"], rel_complete["ledger"],
        "ledger strikes agree"
    );
    // Recovery agrees up to the extra LINKS child the relation run stages
    // under the source, which shifts the recovered row's line by one.
    let mut bang_unblocked = bang_complete["unblocked"].clone();
    let mut rel_unblocked = rel_complete["unblocked"].clone();
    for rows in [&mut bang_unblocked, &mut rel_unblocked] {
        if let Some(items) = rows.as_array_mut() {
            for item in items {
                item["line"] = serde_json::Value::from(0);
            }
        }
    }
    assert_eq!(bang_unblocked, rel_unblocked, "recovery agrees");
    assert_eq!(
        bang_complete["subtasks"], rel_complete["subtasks"],
        "tree subtasks agree"
    );
    assert_eq!(
        bang["pull_in"]["offer_count"], rel["pull_in"]["offer_count"],
        "offer counts agree: {bang} vs {rel}"
    );

    // CRLF: the affected note keeps exact bytes, endings included.
    let (_temp, vault, day, config) = setup("bob-cli-complete-crlf-exact");
    fs::write(
        vault.join("a.md"),
        "- [ ] #task Alpha ^alpha\r\n- [ ] #task Beta ^beta\r\n",
    )
    .expect("write CRLF note");
    write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    capture_json(&vault, &day, &config, "@a+alpha ~b:beta|\"implemented by\"");
    let raw = fs::read(vault.join("a.md")).expect("read raw note");
    let expected = "- [x] #task Alpha  [completion:: 2026-10-14] ^alpha\r\n\t- \u{1faa2} **LINKS:** implemented by [[b#^beta]]\r\n- [ ] #task Beta ^beta\r\n";
    assert_eq!(
        String::from_utf8_lossy(&raw),
        expected,
        "exact CRLF bytes preserved"
    );

    // Midnight under a configured New York day zone with controlled time.
    for (name, now, expected) in [
        (
            "bob-cli-complete-midnight-ny-before",
            "2026-10-14 23:59:00",
            "2026-10-14",
        ),
        (
            "bob-cli-complete-midnight-ny-after",
            "2026-10-15 00:05:00",
            "2026-10-15",
        ),
    ] {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        write_blocked_tasks_settings(&vault);
        let day = vault.join("day.md");
        write_file(&day, DAY);
        let config = vault.join("config.yml");
        write_file(
            &config,
            concat!(
                "timezone: America/New_York\n",
                "relations:\n",
                "  cancel_labels:\n",
                "    - duplicate of\n",
                "  complete_labels:\n",
                "    - implemented by\n",
            ),
        );
        write_file(&vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
        write_file(&vault.join("b.md"), "- [ ] #task Beta ^beta\n");
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("@a+alpha ~b:beta|\"implemented by\"");
        command
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", now)
            .env("BOB_CONFIG_FILE", &config);
        // The configured day zone is America/New_York; the controlled
        // clock pins the completion date on either side of midnight.
        let output = command.output().expect("run capture");
        assert_success(&output);
        assert!(
            read(&vault, "a.md").contains(&format!("[completion:: {expected}]")),
            "BOB_NOW {now} completes with {expected} under America/New_York:\n{}",
            read(&vault, "a.md")
        );
    }
}
