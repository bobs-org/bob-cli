//! Several notes on one existing task compose with every other effect of
//! the item (`plan:202611/drawer_capture_closeout.md`).
//!
//! A note-less `@file+id` with first-level bullets files each bullet as a
//! note, and the item's `&`, `*`, `~`, and `~~` modifiers still act on the
//! target. These tests pin that the multi-note path stages the same
//! relation-close effects as the single-note path, and that the parent it
//! reports (line, text, status, task block) is the final staged state.
//!
//! Every draft runs dry and for real; the two plans must agree except for
//! the bob-authored would-forms, and a dry run must write nothing.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";
const CONFIG: &str = concat!(
    "relations:\n",
    "  cancel_labels:\n",
    "    - duplicate of\n",
    "  complete_labels:\n",
    "    - implemented by\n",
);

struct Scene {
    _temp: TempDir,
    vault: PathBuf,
    day: PathBuf,
    config: PathBuf,
}

fn scene_with(name: &str, config: &str) -> Scene {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    let config_path = temp.path().join("config.yml");
    write_file(&config_path, config);
    Scene {
        _temp: temp,
        vault,
        day,
        config: config_path,
    }
}

fn scene(name: &str) -> Scene {
    scene_with(name, CONFIG)
}

fn read(scene: &Scene, relative: &str) -> String {
    fs::read_to_string(scene.vault.join(relative)).expect("read note")
}

fn snapshot_all(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read vault dir") {
            let path = entry.expect("vault entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(vault)
                    .expect("vault-relative")
                    .to_string_lossy()
                    .into_owned();
                entries.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    entries.sort();
    entries
}

fn run(
    scene: &Scene,
    draft: &str,
    dry_run: bool,
    human: bool,
    extra: &[&str],
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(&scene.vault);
    if !human {
        command.arg("-f").arg("json");
    }
    if dry_run {
        command.arg("--dry-run");
    }
    command.args(extra).arg("--").arg(draft);
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &scene.day)
        .env("BOB_CONFIG_FILE", &scene.config)
        .env("NO_COLOR", "1");
    command.output().expect("run capture")
}

fn json_of(output: &Output) -> serde_json::Value {
    assert_success(output);
    serde_json::from_str(stdout(output).trim()).expect("capture JSON")
}

/// Dry-run then commit, asserting the dry run wrote nothing and that the
/// two plans agree except for the would-forms. Returns the real result.
fn capture_pair(
    scene: &Scene,
    draft: &str,
    extra: &[&str],
) -> serde_json::Value {
    let before = snapshot_all(&scene.vault);
    let dry = json_of(&run(scene, draft, true, false, extra));
    assert_eq!(
        snapshot_all(&scene.vault),
        before,
        "dry run must create and write nothing"
    );
    assert_eq!(dry["dry_run"], true);
    let real = json_of(&run(scene, draft, false, false, extra));
    assert_eq!(real["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    let text = serde_json::to_string(&masked)
        .expect("dry JSON")
        .replace("would complete", "completed")
        .replace("would cancel", "cancelled")
        .replace("would strike", "struck")
        .replace("would unblock", "unblocked")
        .replace("would remove", "removed");
    let masked: serde_json::Value =
        serde_json::from_str(&text).expect("normalized dry JSON");
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for would-forms"
    );
    real
}

/// The coded refusal for a draft, asserting the vault stays byte-identical.
fn refusal(scene: &Scene, draft: &str) -> (String, String) {
    let before = snapshot_all(&scene.vault);
    for dry_run in [true, false] {
        let output = run(scene, draft, dry_run, false, &[]);
        assert!(!output.status.success(), "refusal must fail");
        let value: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("error JSON");
        assert_eq!(
            snapshot_all(&scene.vault),
            before,
            "a refusal must leave every byte alone (dry_run={dry_run})"
        );
        if !dry_run {
            return (
                value["code"].as_str().unwrap_or_default().to_string(),
                value["error"].as_str().unwrap_or_default().to_string(),
            );
        }
    }
    unreachable!("the loop returns on the committing run")
}

/// 1-based line of the task carrying `^block_id` in `file`.
fn line_of(file: &str, block_id: &str) -> usize {
    let suffix = format!("^{block_id}");
    file.lines()
        .position(|line| line.ends_with(&suffix))
        .unwrap_or_else(|| panic!("^{block_id} missing from:\n{file}"))
        + 1
}

/// The checkbox symbol of the task carrying `^block_id`.
fn symbol_of(file: &str, block_id: &str) -> char {
    let line = file
        .lines()
        .nth(line_of(file, block_id) - 1)
        .expect("task line");
    let open = line.find("- [").expect("checkbox") + 3;
    line[open..].chars().next().expect("symbol")
}

/// Every report of the parent agrees with the file bytes: the item's
/// `parent_line` and status fields, and the batch `task_blocks` entry.
fn assert_parent_agrees(json: &serde_json::Value, file: &str, block_id: &str) {
    let line = line_of(file, block_id);
    let symbol = symbol_of(file, block_id).to_string();
    assert_eq!(
        json["parent_line"], line,
        "parent_line vs bytes:\n{file}\n{json}"
    );
    assert_eq!(
        json["parent_status_symbol"], symbol,
        "parent_status_symbol vs bytes:\n{file}\n{json}"
    );
    let block = json["task_blocks"]
        .as_array()
        .expect("task_blocks")
        .iter()
        .find(|block| block["block_id"] == block_id)
        .unwrap_or_else(|| panic!("no task block for ^{block_id}:\n{json}"));
    assert_eq!(block["line"], line, "task block line vs bytes:\n{block}");
    assert_eq!(
        block["status_symbol"], symbol,
        "task block status:\n{block}"
    );
}

// ---------------------------------------------------------------------------
// Closing labels act on the owner (`~`) and the picked source (`~~`).
// ---------------------------------------------------------------------------

const COMPLETE_DRAFT: &str =
    "@foo+parent ~other:other|\"implemented by\"\n- first\n- second";

fn parent_and_other(scene: &Scene) {
    write_file(&scene.vault.join("foo.md"), "- [ ] #task Parent ^parent\n");
    write_file(&scene.vault.join("other.md"), "- [ ] #task Other ^other\n");
}

#[test]
fn complete_label_completes_the_owner_beside_the_notes() {
    let scene = scene("bob-cli-compose-complete");
    parent_and_other(&scene);

    let human = run(&scene, COMPLETE_DRAFT, true, true, &[]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("under [x] Parent")
            && text.contains("· 2 notes")
            && text.contains("🪢 would link to")
            && text.contains("✓ would complete “Parent”"),
        "dry human names the final state, the notes, and the complete:\n{text}"
    );

    let json = capture_pair(&scene, COMPLETE_DRAFT, &[]);
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["notes"].as_array().expect("notes").len(), 2, "{json}");
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "completed",
        "{json}"
    );
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["task_complete"]["action"], "completed", "{json}");
    assert_eq!(complete["source"]["path"], "foo.md");
    assert_eq!(complete["source"]["block_id"], "parent");
    assert_eq!(complete["target"]["block_id"], "other");
    assert_eq!(complete["labels"], serde_json::json!(["implemented by"]));
    assert_eq!(complete["previous_status"], " ");
    assert_eq!(json["warnings"][0]["code"], "relation_completed_source");
    assert_eq!(json["parent_status_symbol"], "x", "{json}");
    assert_eq!(json["parent_status_name"], "Done", "{json}");

    let foo = read(&scene, "foo.md");
    assert_eq!(
        foo,
        concat!(
            "- [x] #task Parent  [completion:: 2026-10-14] ^parent\n",
            "\t- 🪢 **LINKS:** implemented by [[other#^other]]\n",
            "\t- first\n",
            "\t- second\n",
        ),
        "owner Done bytes with both notes filed"
    );
    assert_parent_agrees(&json, &foo, "parent");
}

#[test]
fn multi_note_completion_equals_the_same_line_note_completion() {
    let multi = scene("bob-cli-compose-complete-multi");
    parent_and_other(&multi);
    let single = scene("bob-cli-compose-complete-single");
    parent_and_other(&single);

    let multi_json = capture_pair(&multi, COMPLETE_DRAFT, &[]);
    let single_json = capture_pair(
        &single,
        "@foo+parent a note ~other:other|\"implemented by\"",
        &[],
    );
    for field in ["relation_updates", "relation_completes", "warnings"] {
        assert_eq!(
            multi_json[field], single_json[field],
            "{field} must not depend on the note form"
        );
    }
    assert_eq!(
        read(&multi, "other.md"),
        read(&single, "other.md"),
        "the link target's bytes must not depend on the note form"
    );
}

#[test]
fn complete_label_runs_the_ledger_and_successor_effects() {
    let scene = scene("bob-cli-compose-complete-ledger");
    write_file(
        &scene.vault.join("cash.md"),
        concat!(
            "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
            "- [?] #task Needs stocks [dependsOn:: cash__sell-stocks] [id:: cash__needs] ^needs-stocks\n",
        ),
    );
    write_file(
        &scene.vault.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-stocks-now\n",
    );
    write_file(
        &scene.vault.join("today.md"),
        concat!(
            "## Pomodoros\n",
            "- [ ] Morning 0900-1000\n",
            "  - [[cash#^sell-stocks]]\n",
        ),
    );
    let scene = Scene {
        day: scene.vault.join("today.md"),
        ..scene
    };

    let json = capture_pair(
        &scene,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"implemented by\"\n- first\n- second",
        &["-p", "all"],
    );
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["task_complete"]["action"], "completed", "{json}");
    assert_eq!(
        complete["task_complete"]["ledger"]["struck"], 1,
        "today's dedicated link is struck: {json}"
    );
    assert_eq!(
        complete["task_complete"]["unblocked"][0]["block_id"], "needs-stocks",
        "{json}"
    );
    assert_eq!(
        complete["task_complete"]["unblocked"][0]["offer"]["selected"], true,
        "{json}"
    );
    assert_eq!(json["pull_in"]["choice"], "all", "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 1, "{json}");
    assert!(
        json["day_file"]
            .as_str()
            .is_some_and(|day| day.ends_with("today.md")),
        "the closing day file is reported: {json}"
    );
    let today = read(&scene, "today.md");
    assert!(
        today.contains("~~[[cash#^sell-stocks]]~~")
            && today.contains("[[cash#^needs-stocks]]"),
        "the link is struck and the successor is pulled in:\n{today}"
    );
    let cash = read(&scene, "cash.md");
    assert!(
        cash.contains("- [x] #task Sell stocks")
            && cash.contains("\t- first\n")
            && cash.contains("\t- second\n"),
        "owner Done with both notes:\n{cash}"
    );
    assert_parent_agrees(&json, &cash, "sell-stocks");
}

#[test]
fn inverse_complete_label_completes_the_picked_source_not_the_owner() {
    let scene = scene("bob-cli-compose-complete-inverse");
    parent_and_other(&scene);

    let json = capture_pair(
        &scene,
        "@foo+parent ~~other:other|\"implemented by\"\n- first\n- second",
        &[],
    );
    let complete = &json["relation_completes"][0];
    assert_eq!(complete["task_complete"]["action"], "completed", "{json}");
    assert_eq!(complete["source"]["path"], "other.md");
    assert_eq!(complete["source"]["block_id"], "other");
    assert_eq!(complete["target"]["path"], "foo.md");
    assert_eq!(complete["target"]["block_id"], "parent");
    let other = read(&scene, "other.md");
    assert!(
        other.contains("- [x] #task Other")
            && other.contains("[completion:: 2026-10-14]"),
        "the picked source is Done:\n{other}"
    );
    let foo = read(&scene, "foo.md");
    assert!(
        foo.starts_with("- [ ] #task Parent ^parent\n")
            && foo.contains("\t- first\n")
            && foo.contains("\t- second\n"),
        "the owner stays open and keeps its notes:\n{foo}"
    );
    assert_eq!(json["parent_status_symbol"], " ", "{json}");
    assert_parent_agrees(&json, &foo, "parent");
}

#[test]
fn cancel_label_cancels_the_owner_beside_the_notes() {
    let scene = scene("bob-cli-compose-cancel");
    write_file(
        &scene.vault.join("cash.md"),
        "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
    );
    write_file(
        &scene.vault.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-stocks-now\n",
    );
    write_file(
        &scene.vault.join("today.md"),
        concat!(
            "## Pomodoros\n",
            "- [ ] Morning 0900-1000\n",
            "  - [[cash#^sell-stocks]]\n",
        ),
    );
    write_file(
        &scene.vault.join("c.md"),
        concat!(
            "- [?] #task Needs stocks [fresh:: 2026-10-14] [dependsOn:: cash__sell-stocks] ^needs-stocks\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^sell-stocks]]\n",
        ),
    );
    let scene = Scene {
        day: scene.vault.join("today.md"),
        ..scene
    };

    let json = capture_pair(
        &scene,
        "@cash+sell-stocks ~dev:sell-stocks-now|\"duplicate of\"\n- first\n- second",
        &[],
    );
    assert_eq!(
        json["relation_updates"][0]["cancel_effect"], "cancelled",
        "{json}"
    );
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["source"]["block_id"], "sell-stocks", "{json}");
    assert_eq!(cancel["pomodoro_links_removed"], 1, "{json}");
    assert_eq!(cancel["recovered"][0]["block_id"], "needs-stocks", "{json}");
    assert_eq!(json["warnings"][0]["code"], "relation_cancelled_source");
    assert_eq!(json["parent_status_symbol"], "-", "{json}");
    assert_eq!(json["parent_status_name"], "Canceled", "{json}");

    let cash = read(&scene, "cash.md");
    assert_eq!(
        cash,
        concat!(
            "- [-] #task Sell stocks [id:: cash__sell-stocks] [cancelled:: 2026-10-14] ^sell-stocks\n",
            "\t- 🪢 **LINKS:** duplicate of [[dev#^sell-stocks-now]]\n",
            "\t- first\n",
            "\t- second\n",
        ),
        "owner cancelled with both notes filed"
    );
    assert_eq!(
        read(&scene, "today.md"),
        "## Pomodoros\n- [ ] Morning 0900-1000\n",
        "the dedicated Pomodoro link prunes"
    );
    assert!(
        read(&scene, "c.md").contains("- [ ] #task Needs stocks"),
        "the dependent recovers:\n{}",
        read(&scene, "c.md")
    );
    assert_parent_agrees(&json, &cash, "sell-stocks");
}

#[test]
fn inverse_cancel_label_cancels_the_picked_source_not_the_owner() {
    let scene = scene("bob-cli-compose-cancel-inverse");
    parent_and_other(&scene);

    let json = capture_pair(
        &scene,
        "@foo+parent ~~other:other|\"duplicate of\"\n- first\n- second",
        &[],
    );
    let cancel = &json["relation_cancels"][0];
    assert_eq!(cancel["source"]["path"], "other.md", "{json}");
    assert_eq!(cancel["source"]["block_id"], "other");
    assert_eq!(cancel["target"]["block_id"], "parent");
    assert!(
        read(&scene, "other.md").contains("- [-] #task Other"),
        "the picked source is cancelled:\n{}",
        read(&scene, "other.md")
    );
    let foo = read(&scene, "foo.md");
    assert_eq!(json["parent_status_symbol"], " ", "{json}");
    assert!(foo.contains("\t- first\n") && foo.contains("\t- second\n"));
    assert_parent_agrees(&json, &foo, "parent");
}

#[test]
fn unconfigured_labels_keep_the_owner_open_and_only_link() {
    let scene = scene_with("bob-cli-compose-unconfigured", "relations: {}\n");
    parent_and_other(&scene);

    let json = capture_pair(&scene, COMPLETE_DRAFT, &[]);
    assert!(json.get("relation_completes").is_none(), "{json}");
    assert!(json.get("relation_cancels").is_none(), "{json}");
    assert!(json.get("warnings").is_none(), "{json}");
    assert_eq!(json["parent_status_symbol"], " ", "{json}");
    let foo = read(&scene, "foo.md");
    assert_eq!(
        foo,
        concat!(
            "- [ ] #task Parent ^parent\n",
            "\t- 🪢 **LINKS:** implemented by [[other#^other]]\n",
            "\t- first\n",
            "\t- second\n",
        )
    );
    assert_parent_agrees(&json, &foo, "parent");
}

#[test]
fn closing_refusals_are_coded_and_roll_everything_back() {
    let scene = scene("bob-cli-compose-refusals");
    write_file(&scene.vault.join("a.md"), "- [ ] #task Alpha ^alpha\n");
    write_file(&scene.vault.join("b.md"), "- [ ] #task Beta ^beta\n");
    write_file(&scene.vault.join("c.md"), "- [ ] #task Gamma ^gamma\n");
    write_file(
        &scene.vault.join("r.md"),
        "- [ ] #task Repeat [repeat:: every week] ^rep\n",
    );

    let (code, message) = refusal(
        &scene,
        "@b+beta &a:alpha ~a:alpha|\"implemented by\"\n- first\n- second",
    );
    assert_eq!(code, "relation_complete_conflicts_with_item_action");
    assert!(message.contains("also uses &"), "{message}");

    let (code, _) = refusal(
        &scene,
        "@b+beta *a:alpha ~~a:alpha|\"duplicate of\"\n- first",
    );
    assert_eq!(code, "relation_cancel_conflicts_with_item_action");

    let (code, message) = refusal(
        &scene,
        "@r+rep ~a:alpha|\"implemented by\"\n- first\n- second",
    );
    assert_eq!(code, "relation_completes_recurring_task");
    assert!(message.contains("repeats"), "{message}");

    let (code, message) = refusal(
        &scene,
        "@a+alpha ~b:beta|\"duplicate of\" ~c:gamma|\"implemented by\"\n- first",
    );
    assert_eq!(code, "relation_close_effects_conflict");
    assert!(
        message.contains("can't be both cancelled and completed"),
        "{message}"
    );

    // A good multi-note item cannot survive a later refusal in the draft.
    let (code, _) = refusal(
        &scene,
        "@a+alpha ~b:beta|\"implemented by\"\n- first\n- second\n\n@r+rep ~c:gamma|\"implemented by\"",
    );
    assert_eq!(code, "relation_completes_recurring_task");
    assert!(
        !read(&scene, "a.md").contains("first"),
        "the earlier item's notes must roll back"
    );
}

#[test]
fn an_already_closed_owner_links_only_and_still_files_the_notes() {
    let scene = scene("bob-cli-compose-closed");
    write_file(&scene.vault.join("a.md"), "- [x] #task Done deed ^done\n");
    write_file(&scene.vault.join("b.md"), "- [ ] #task Beta ^beta\n");

    let json = capture_pair(
        &scene,
        "@a+done ~b:beta|\"implemented by\"\n- first\n- second",
        &[],
    );
    assert_eq!(
        json["relation_updates"][0]["complete_effect"], "already_closed",
        "{json}"
    );
    assert!(json.get("relation_completes").is_none(), "{json}");
    let a = read(&scene, "a.md");
    assert_eq!(
        a,
        concat!(
            "- [x] #task Done deed ^done\n",
            "\t- 🪢 **LINKS:** implemented by [[b#^beta]]\n",
            "\t- first\n",
            "\t- second\n",
        )
    );
    assert_parent_agrees(&json, &a, "done");
}

// ---------------------------------------------------------------------------
// The reported parent is the final staged parent.
// ---------------------------------------------------------------------------

#[test]
fn amp_reports_the_blocked_parent_everywhere() {
    let scene = scene("bob-cli-compose-amp");
    parent_and_other(&scene);
    let draft = "@foo+parent &other:other\n- first\n- second";

    let human = run(&scene, draft, true, true, &[]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("under [ ] → [?] Parent"),
        "human names the parent's real transition:\n{text}"
    );

    let json = capture_pair(&scene, draft, &[]);
    assert_eq!(json["parent_status_symbol"], "?", "{json}");
    assert_eq!(json["parent_status_name"], "Blocked", "{json}");
    assert_eq!(json["dependency_update"]["dependent_status"], "?");
    assert_eq!(json["parent_text"], "Parent");
    let foo = read(&scene, "foo.md");
    assert!(
        foo.starts_with("- [?] #task Parent"),
        "file bytes agree:\n{foo}"
    );
    assert_parent_agrees(&json, &foo, "parent");
}

#[test]
fn star_reports_the_committed_parent_everywhere() {
    let scene = scene("bob-cli-compose-star");
    write_file(&scene.vault.join("foo.md"), "- [ ] #task Parent ^parent\n");
    write_file(&scene.vault.join("other.md"), "- [*] #task Other ^other\n");
    let draft = "@foo+parent *other:other\n- first\n- second";

    let json = capture_pair(&scene, draft, &[]);
    // The target is Next, so the parent it now blocks is raised to Next.
    assert_eq!(json["parent_status_symbol"], "*", "{json}");
    assert_eq!(json["parent_status_name"], "Next", "{json}");
    let foo = read(&scene, "foo.md");
    assert!(foo.starts_with("- [*] #task Parent"), "{foo}");
    assert_parent_agrees(&json, &foo, "parent");
    assert!(
        read(&scene, "other.md").starts_with("- [?] #task Other"),
        "the starred target is blocked by the parent:\n{}",
        read(&scene, "other.md")
    );
}

#[test]
fn same_note_mirrors_do_not_leave_stale_parent_lines() {
    // `&foo:other` stages a BLOCKS mirror under `other`, which sits above
    // the parent, so the parent moves down a line after its notes landed.
    let scene = scene("bob-cli-compose-shift-amp");
    write_file(
        &scene.vault.join("foo.md"),
        "- [ ] #task Other ^other\n- [ ] #task Parent ^parent\n",
    );
    let json =
        capture_pair(&scene, "@foo+parent &foo:other\n- first\n- second", &[]);
    let foo = read(&scene, "foo.md");
    assert_eq!(line_of(&foo, "parent"), 3, "the mirror shifted the parent");
    assert_eq!(json["parent_line"], 3, "{json}");
    assert_parent_agrees(&json, &foo, "parent");

    // A relation's LINKED FROM mirror and a completion shift it the same way.
    let scene = self::scene("bob-cli-compose-shift-complete");
    write_file(
        &scene.vault.join("foo.md"),
        "- [ ] #task Other ^other\n- [ ] #task Parent ^parent\n",
    );
    let json = capture_pair(
        &scene,
        "@foo+parent ~foo:other|\"implemented by\"\n- first\n- second",
        &[],
    );
    let foo = read(&scene, "foo.md");
    assert!(foo.contains("[x] #task Parent"), "{foo}");
    assert_parent_agrees(&json, &foo, "parent");
    assert_eq!(json["parent_line"], line_of(&foo, "parent"), "{json}");

    // Cancelling an earlier task in the same note adds LINKS/log lines
    // above the parent too.
    let scene = self::scene("bob-cli-compose-shift-cancel");
    write_file(
        &scene.vault.join("foo.md"),
        "- [ ] #task Above ^above\n- [ ] #task Parent ^parent\n",
    );
    let json = capture_pair(
        &scene,
        "@foo+parent ~~foo:above|\"duplicate of\"\n- first\n- second",
        &[],
    );
    let foo = read(&scene, "foo.md");
    assert!(foo.contains("[-] #task Above"), "{foo}");
    assert!(line_of(&foo, "parent") > 2, "the parent moved:\n{foo}");
    assert_parent_agrees(&json, &foo, "parent");
}
