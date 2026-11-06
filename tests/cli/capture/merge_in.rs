//! Atomic capture merge-in (`<`) execution through the explicit merge engine,
//! plus incomplete-source feedback and the stdin shell guard.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Stdio;

const NOW: &str = "2026-11-05 09:30:00";

fn area_with_tasks(tasks: &str) -> String {
    format!("---\ntype: \"[[area]]\"\n---\n## Tasks\n{tasks}")
}

fn enable_blocked_status(vault: &Path) {
    let path = vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    write_file(
        &path,
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
    );
}

fn setup_merge_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("inbox.md"),
        &area_with_tasks("- [ ] #task Existing target ^t\n"),
    );
    write_file(
        &vault.join("a.md"),
        &area_with_tasks(
            "- [ ] #task Fix the flaky login test ^f\n\t- source note a\n",
        ),
    );
    write_file(
        &vault.join("b.md"),
        &area_with_tasks(
            "- [ ] #task Login timeout too short ^b\n\t- source note b\n",
        ),
    );
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    (temp, vault, day_file)
}

fn snapshot(vault: &Path) -> Vec<(String, String)> {
    fn collect(
        root: &Path,
        directory: &Path,
        files: &mut Vec<(String, String)>,
    ) {
        for entry in std::fs::read_dir(directory).expect("read vault directory")
        {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                collect(root, &path, files);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let name = path
                    .strip_prefix(root)
                    .expect("path stays under vault")
                    .to_string_lossy()
                    .replace('\\', "/");
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                files.push((name, body));
            }
        }
    }
    let mut files = Vec::new();
    collect(vault, vault, &mut files);
    files.sort();
    files
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    draft: &str,
    dry_run: bool,
) -> std::process::Output {
    capture_with(vault, day_file, draft, dry_run, None, None)
}

fn capture_with(
    vault: &Path,
    day_file: &Path,
    draft: &str,
    dry_run: bool,
    pull_in: Option<&str>,
    config: Option<&Path>,
) -> std::process::Output {
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
    if let Some(pull_in) = pull_in {
        command.arg("--pull-in").arg(pull_in);
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_PRIORITY_ROLL_SEED", "merge-in-acceptance");
    if let Some(config) = config {
        command.env("BOB_CONFIG_FILE", config);
    }
    command.output().expect("run capture")
}

fn capture_human(
    vault: &Path,
    day_file: &Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture human")
}

fn capture_human_flags(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run capture human")
}

fn output_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(output))
        })
}

fn item(result: &serde_json::Value) -> &serde_json::Value {
    result
        .get("captures")
        .and_then(serde_json::Value::as_array)
        .and_then(|captures| captures.first())
        .unwrap_or(result)
}

#[test]
fn merge_in_creates_target_and_appends_sources_after_authored_notes() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_execute");
    let mut source_a = fs::read_to_string(vault.join("a.md")).unwrap();
    source_a = source_a.replace("^f", "[priority:: low] ^f");
    write_file(&vault.join("a.md"), &source_a);
    let draft = "New target @inbox < @a+f @b+b\n- authored note\n- #DETAILS\n  - authored drawer";
    let before = snapshot(&vault);

    let dry = capture_json(&vault, &day_file, draft, true);
    assert_success(&dry);
    let dry_json = output_json(&dry);
    let dry_item = item(&dry_json);
    assert_eq!(dry_item["kind"], "task", "{dry_json}");
    let dry_id = dry_item["block_id"]
        .as_str()
        .expect("merge-in preview has a target ID");
    assert_eq!(
        dry_item["block_link"],
        format!("[[inbox#^{dry_id}]]"),
        "{dry_json}"
    );
    assert!(dry_item["task_line"].as_str().unwrap().contains(dry_id));
    assert_eq!(
        dry_item["merges"].as_array().unwrap().len(),
        2,
        "{dry_json}"
    );
    assert!(
        dry_item["merges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["target"]["created"] == true),
        "{dry_json}"
    );
    assert_eq!(
        snapshot(&vault),
        before,
        "dry-run must not write the target"
    );

    let real = capture_json(&vault, &day_file, draft, false);
    assert_success(&real);
    let real_json = output_json(&real);
    let real_item = item(&real_json);
    assert_eq!(real_item["block_id"], dry_id, "preview ID is deterministic");
    assert_eq!(
        real_item["merges"].as_array().unwrap().len(),
        2,
        "{real_json}"
    );

    let inbox = std::fs::read_to_string(vault.join("inbox.md")).unwrap();
    let authored = inbox.find("authored note").expect("authored child lands");
    let authored_drawer = inbox
        .find("authored drawer")
        .expect("authored drawer lands");
    let source_title_a = inbox
        .find("Fix the flaky login test")
        .expect("source A title bullet lands");
    let source_a = inbox.find("source note a").expect("source A payload lands");
    let source_title_b = inbox
        .find("Login timeout too short")
        .expect("source B title bullet lands");
    let source_b = inbox.find("source note b").expect("source B payload lands");
    assert!(
        authored < authored_drawer
            && authored_drawer < source_title_a
            && source_title_a < source_a
            && source_a < source_title_b
            && source_title_b < source_b,
        "{inbox}"
    );
    assert!(
        inbox.contains("[[a#^f]] (merged into)")
            && inbox.contains("[[b#^b]] (merged into)"),
        "target keeps inverse links for each source: {inbox}"
    );
    let target_line = inbox
        .lines()
        .find(|line| line.contains(&format!("^{dry_id}")))
        .expect("new target line");
    assert!(target_line.contains("New target"), "{target_line}");
    assert!(
        !target_line.contains("priority"),
        "source fields stay on S: {target_line}"
    );
    for (path, id) in [("a.md", "f"), ("b.md", "b")] {
        let source = std::fs::read_to_string(vault.join(path)).unwrap();
        assert!(
            source.contains("#task"),
            "source tombstone remains: {source}"
        );
        assert!(
            source.contains(&format!("merged into [[inbox#^{dry_id}]]")),
            "source {id} points to the new target: {source}"
        );
    }
    let blocks = real_json["task_blocks"].as_array().unwrap();
    assert!(
        blocks.iter().any(|block| {
            block["block_id"] == dry_id
                && block["roles"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("merge_target"))
        }),
        "{real_json}"
    );
    assert_eq!(
        blocks.len(),
        3,
        "one target card and one card per source: {real_json}"
    );
    for (block_id, origin) in [("f", "a.md#^f"), ("b", "b.md#^b")] {
        let source = blocks
            .iter()
            .find(|block| block["block_id"] == block_id)
            .expect("one source card");
        assert_eq!(source["roles"], serde_json::json!(["merge_source"]));
        assert!(
            source["lines"]
                .as_array()
                .unwrap()
                .iter()
                .all(|line| line.get("origin").is_none()),
            "tombstones do not carry payload provenance: {source}"
        );
        let target = blocks
            .iter()
            .find(|block| block["block_id"] == dry_id)
            .expect("one target card");
        assert!(
            target["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line["origin"] == origin),
            "landed payload rows carry exact source provenance: {target}"
        );
    }
    let target = blocks
        .iter()
        .find(|block| block["block_id"] == dry_id)
        .unwrap();
    assert!(
        target["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|line| line["text"] == "- authored note")
            .all(|line| line.get("origin").is_none()),
        "authored rows do not inherit merge provenance: {target}"
    );
}

#[test]
fn merge_in_bytes_match_the_equivalent_two_item_transfer() {
    let (_temp_left, left_vault, left_day) =
        setup_merge_vault("merge_in_equiv_left");
    let (_temp_right, right_vault, right_day) =
        setup_merge_vault("merge_in_equiv_right");
    let left = "New target @inbox^new < @a+f @b+b\n- authored note";
    let right =
        "New target @inbox^new\n- authored note\n\n@a+f @b+b > @inbox+new";

    let left_output = capture_json(&left_vault, &left_day, left, false);
    assert_success(&left_output);
    let right_output = capture_json(&right_vault, &right_day, right, false);
    assert_success(&right_output);
    assert_eq!(snapshot(&left_vault), snapshot(&right_vault));
}

#[test]
fn merge_in_equivalence_covers_same_note_drawers_and_configured_labels() {
    let (_temp_left, left_vault, left_day) =
        setup_merge_vault("merge_in_equiv_same_left");
    let (_temp_right, right_vault, right_day) =
        setup_merge_vault("merge_in_equiv_same_right");
    enable_blocked_status(&left_vault);
    enable_blocked_status(&right_vault);
    let same_note =
        area_with_tasks("- [ ] #task Source ^f\n\t- same-note payload\n");
    write_file(&left_vault.join("a.md"), &same_note);
    write_file(&right_vault.join("a.md"), &same_note);
    let left = "New target @a^same s:3 < @a+f\n- authored note\n- #DETAILS\n  - authored drawer";
    let right = "New target @a^same s:3\n- authored note\n- #DETAILS\n  - authored drawer\n\n@a+f > @a+same";
    let a = capture_json(&left_vault, &left_day, left, false);
    let b = capture_json(&right_vault, &right_day, right, false);
    assert_success(&a);
    assert_success(&b);
    assert_eq!(snapshot(&left_vault), snapshot(&right_vault));

    let (_temp_left, left_vault, left_day) =
        setup_merge_vault("merge_in_equiv_label_left");
    let (_temp_right, right_vault, right_day) =
        setup_merge_vault("merge_in_equiv_label_right");
    let left_config = left_vault.join("config.yml");
    let right_config = right_vault.join("config.yml");
    write_file(&left_config, "relations:\n  merge_labels: [folded into]\n");
    write_file(&right_config, "relations:\n  merge_labels: [folded into]\n");
    let left = "New target @inbox^same < @a+f @b+b\n- authored note";
    let right =
        "New target @inbox^same\n- authored note\n\n@a+f @b+b > @inbox+same";
    let a = capture_with(
        &left_vault,
        &left_day,
        left,
        false,
        None,
        Some(&left_config),
    );
    let b = capture_with(
        &right_vault,
        &right_day,
        right,
        false,
        None,
        Some(&right_config),
    );
    assert_success(&a);
    assert_success(&b);
    assert_eq!(snapshot(&left_vault), snapshot(&right_vault));
    assert!(
        fs::read_to_string(left_vault.join("a.md"))
            .unwrap()
            .contains("folded into"),
        "configured label is shared by both explicit merge adapters"
    );
}

#[test]
fn merge_in_selected_and_unselected_offer_bytes_match_transfer() {
    for pick in ["none", "all"] {
        let (_temp_left, left_vault, left_day) =
            setup_merge_vault("merge_in_offer_equiv_left");
        let (_temp_right, right_vault, right_day) =
            setup_merge_vault("merge_in_offer_equiv_right");
        let ledger = concat!(
            "## Pomodoros\n\n- [ ] () — BUILD\n",
            "  - [[a#^f]]\n",
            "  - keep this slot\n",
            "  - [[b#^b]]\n",
        );
        write_file(&left_day, ledger);
        write_file(&right_day, ledger);
        let merge_in = capture_with(
            &left_vault,
            &left_day,
            "New target @inbox^target < @a+f @b+b",
            false,
            Some(pick),
            None,
        );
        let transfer = capture_with(
            &right_vault,
            &right_day,
            "New target @inbox^target\n\n@a+f @b+b > @inbox+target",
            false,
            Some(pick),
            None,
        );
        assert_success(&merge_in);
        assert_success(&transfer);
        assert_eq!(
            snapshot(&left_vault),
            snapshot(&right_vault),
            "pick={pick}"
        );
    }
}

#[test]
fn merge_in_same_note_anchor_collision_repairs_payload_and_inbound_link() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_same_note_anchor");
    write_file(
        &vault.join("a.md"),
        &area_with_tasks("- [ ] #task Source ^f\n  - source note ^detail\n"),
    );
    write_file(&vault.join("referrer.md"), "See [[a#^detail]].\n");
    let output = capture_json(
        &vault,
        &day_file,
        "New target @a^target < @a+f\n- authored note ^detail",
        false,
    );
    assert_success(&output);
    let json = output_json(&output);
    let target = json["task_blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["block_id"] == "target")
        .expect("same-note target card");
    let moved = target["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| {
            line["origin"] == "a.md#^f"
                && line["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("source note")
        })
        .expect("source child row carries its merge origin");
    let moved_text = moved["text"].as_str().unwrap();
    let moved_anchor = moved_text
        .split('^')
        .next_back()
        .expect("source child anchor");
    assert!(!moved_anchor.is_empty(), "{moved_text}");
    let target_note = fs::read_to_string(vault.join("a.md")).unwrap();
    assert_eq!(target_note.matches("^detail\n").count(), 1, "{target_note}");
    assert!(
        target_note.contains(&format!("source note ^{moved_anchor}")),
        "moved row retains its unique anchor: {target_note}"
    );
    let referrer = fs::read_to_string(vault.join("referrer.md")).unwrap();
    assert!(
        referrer.contains(&format!("[[a#^{moved_anchor}]]")),
        "inbound link follows the merged payload: {referrer}"
    );
}

#[test]
fn merge_in_empty_label_config_still_merges_with_fallback_label() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_empty_labels");
    let config = vault.join("config.yml");
    write_file(&config, "relations:\n  merge_labels: []\n");
    let output = capture_with(
        &vault,
        &day_file,
        "New target @inbox^target < @a+f",
        false,
        None,
        Some(&config),
    );
    assert_success(&output);
    assert!(fs::read_to_string(vault.join("a.md"))
        .unwrap()
        .contains("merged into [[inbox#^target]]"));
}

#[test]
fn selected_merge_in_offer_matches_two_item_transfer_without_freshness() {
    let (_temp_left, left_vault, left_day) =
        setup_merge_vault("merge_in_offer_left");
    let (_temp_right, right_vault, right_day) =
        setup_merge_vault("merge_in_offer_right");
    let day =
        "## Pomodoros\n\n- [ ] () — BUILD\n  - [[a#^f]]\n  - keep this slot\n";
    write_file(&left_day, day);
    write_file(&right_day, day);
    let merge_in = capture_with(
        &left_vault,
        &left_day,
        "New target @inbox^target < @a+f",
        false,
        Some("all"),
        None,
    );
    let two_item_transfer = capture_with(
        &right_vault,
        &right_day,
        "New target @inbox^target\n\n@a+f > @inbox+target",
        false,
        Some("all"),
        None,
    );
    assert_success(&merge_in);
    assert_success(&two_item_transfer);
    assert_eq!(snapshot(&left_vault), snapshot(&right_vault));

    let json = output_json(&merge_in);
    let target_id = item(&json)["block_id"].as_str().unwrap();
    let day = std::fs::read_to_string(&left_day).unwrap();
    assert_eq!(
        day.matches(&format!("[[inbox#^{target_id}]]")).count(),
        1,
        "{day}"
    );
    assert!(!day.contains("[[a#^f]]"), "{day}");
    let inbox = std::fs::read_to_string(left_vault.join("inbox.md")).unwrap();
    let target = inbox
        .lines()
        .find(|line| line.contains(&format!("^{target_id}")))
        .unwrap();
    assert!(target.starts_with("- [*]"), "{target}");
    assert!(
        !inbox.contains("[fresh::"),
        "new target is not freshness-stamped: {inbox}"
    );
}

#[test]
fn later_merge_in_failure_rolls_back_the_earlier_new_task() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_bulk_rollback");
    let before = snapshot(&vault);
    let output = capture_json(
        &vault,
        &day_file,
        "First task @inbox\n\nSecond < @a+missing",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(snapshot(&vault), before, "bulk vault must be unchanged");
}

#[test]
fn merge_in_invalid_second_source_refuses_without_writes() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_bad_second_source");
    let before = snapshot(&vault);
    let output = capture_json(
        &vault,
        &day_file,
        "New target @inbox^target < @a+f @b+missing",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(output_json(&output)["code"], "task_not_found");
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn merge_in_source_refusal_matrix_is_atomic() {
    let cases = [
        ("closed", "@a+closed", "merge_source_closed"),
        ("recurring", "@a+recurring", "merge_source_recurring"),
        ("project", "@a+prj", "merge_source_project"),
        ("open_child", "@a+parent", "merge_source_open_subtasks"),
        ("missing", "@a+missing", "task_not_found"),
        (
            "overlap",
            "@tree+parent @tree+child",
            "transfer_overlapping_sources",
        ),
    ];
    for (case, sources, expected_code) in cases {
        let (_temp, vault, day_file) =
            setup_merge_vault("merge_in_refusal_matrix");
        let extra = match case {
            "closed" => "- [-] #task Closed ^closed\n",
            "recurring" => "- [ ] #task Repeats [repeat:: daily] ^recurring\n",
            "project" => "- [ ] #task Project ^prj\n",
            "open_child" => {
                "- [ ] #task Parent ^parent\n  - [ ] #task Child ^child\n"
            }
            "overlap" => "",
            _ => "",
        };
        if !extra.is_empty() {
            let a = vault.join("a.md");
            let mut contents = fs::read_to_string(&a).unwrap();
            contents.push_str(extra);
            write_file(&a, &contents);
        }
        if case == "overlap" {
            write_file(
                &vault.join("tree.md"),
                &area_with_tasks(
                    "- [ ] #task Parent ^parent\n  - [ ] #task Child ^child\n",
                ),
            );
        }
        let before = snapshot(&vault);
        let output = capture_json(
            &vault,
            &day_file,
            &format!("New target @inbox^target < {sources}"),
            false,
        );
        assert!(
            !output.status.success(),
            "{case}: {}",
            format_output(&output)
        );
        assert_eq!(output_json(&output)["code"], expected_code, "{case}");
        assert_eq!(snapshot(&vault), before, "{case} refusal must be atomic");
    }
}

#[test]
fn merge_in_repoints_shared_dependents_and_cycle_refusal_is_atomic() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_dependents");
    enable_blocked_status(&vault);
    write_file(
        &vault.join("dependent.md"),
        &area_with_tasks(concat!(
            "- [?] #task Waiting on both sources [id:: dependent] ^dependent\n",
            "  - ⛓️ **DEPENDS ON:** [[a#^f]] [[b#^b]]\n",
        )),
    );
    let output = capture_json(
        &vault,
        &day_file,
        "New target @inbox^target < @a+f @b+b",
        false,
    );
    assert_success(&output);
    let dependent = fs::read_to_string(vault.join("dependent.md")).unwrap();
    assert_eq!(
        dependent.matches("[[inbox#^target]]").count(),
        1,
        "{dependent}"
    );
    assert!(!dependent.contains("[[a#^f]]"), "{dependent}");
    assert!(!dependent.contains("[[b#^b]]"), "{dependent}");
    let json = output_json(&output);
    assert_eq!(item(&json)["merges"].as_array().unwrap().len(), 2, "{json}");

    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_shared_dependent_copy");
    enable_blocked_status(&vault);
    write_file(
        &vault.join("dependent.md"),
        &area_with_tasks(concat!(
            "- [?] #task Shared dependent ^dependent\n",
            "  - ⛓️ **DEPENDS ON:** [[a#^f]] [[b#^b]]\n",
        )),
    );
    let human = capture_human(
        &vault,
        &day_file,
        "New target @inbox^target < @a+f @b+b",
    );
    assert_success(&human);
    let human_text = String::from_utf8_lossy(&human.stdout);
    assert_eq!(
        human_text.matches("now waits on “New target”").count(),
        1,
        "shared dependent identity is presented once: {human_text}"
    );

    let (_temp, vault, day_file) = setup_merge_vault("merge_in_cycle");
    enable_blocked_status(&vault);
    write_file(
        &vault.join("dependent.md"),
        &area_with_tasks(concat!(
            "- [?] #task Waiting on source [id:: dependent] ^dependent\n",
            "  - ⛓️ **DEPENDS ON:** [[a#^f]]\n",
        )),
    );
    let before = snapshot(&vault);
    let output = capture_json(
        &vault,
        &day_file,
        "New target @inbox^target &dependent:dependent < @a+f",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    let json = output_json(&output);
    assert_eq!(json["code"], "merge_dependency_cycle", "{json}");
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn merge_in_suppresses_offers_for_blocked_and_already_linked_targets() {
    for (case, draft, existing_target_link) in [
        ("scheduled", "New target @inbox^target s:3 < @a+f", false),
        (
            "dependent_blocked",
            "New target @inbox^target &b:b < @a+f",
            false,
        ),
        ("already_linked", "New target @inbox^target < @a+f", true),
    ] {
        let (_temp, vault, day_file) =
            setup_merge_vault("merge_in_offer_suppression");
        enable_blocked_status(&vault);
        let mut day = String::from("## Pomodoros\n\n- [ ] () — BUILD\n");
        if existing_target_link {
            day.push_str("  - [[inbox#^target]]\n");
        }
        day.push_str("  - [[a#^f]]\n  - keep this slot\n");
        write_file(&day_file, &day);
        let output = capture_json(&vault, &day_file, draft, false);
        assert_success(&output);
        let json = output_json(&output);
        assert!(
            item(&json)["merges"][0]["offers"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{case}: {json}"
        );
        let after = fs::read_to_string(&day_file).unwrap();
        assert!(
            !after.contains("[[a#^f]]"),
            "{case}: source link still pruned: {after}"
        );
        assert!(
            after.contains("keep this slot"),
            "{case}: unrelated ledger child survives: {after}"
        );
        assert_eq!(
            after.contains("[[inbox#^target]]"),
            existing_target_link,
            "{case}: existing target links stay untouched: {after}"
        );
    }
}

#[test]
fn merge_in_task_list_bullets_write_each_target_and_source() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_task_list");
    let output = capture_json(
        &vault,
        &day_file,
        "@inbox\n- First target < @a+f\n- Second target < @b+b",
        false,
    );
    assert_success(&output);
    let json = output_json(&output);
    let captures = json["captures"].as_array().expect("bulk captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert!(
        captures.iter().all(|capture| capture["kind"] == "task"),
        "{json}"
    );
    let inbox = fs::read_to_string(vault.join("inbox.md")).unwrap();
    assert!(inbox.contains("First target"), "{inbox}");
    assert!(inbox.contains("Second target"), "{inbox}");
    for path in ["a.md", "b.md"] {
        let source = fs::read_to_string(vault.join(path)).unwrap();
        assert!(source.contains("[-] #task"), "{source}");
    }
}

#[test]
fn merge_in_dry_run_matches_commit_and_reconcile_is_a_noop() {
    // Offer choices, then targets that suppress the offer: scheduled and
    // dependency-Blocked new targets, and a `:id` target the capture links.
    for (case, draft, pick) in [
        ("offer none", "New target @inbox^target < @a+f", "none"),
        ("offer all", "New target @inbox^target < @a+f", "all"),
        ("scheduled", "New target @inbox^target s:3 < @a+f", "all"),
        (
            "dependency blocked",
            "New target @inbox^target &b:b < @a+f",
            "all",
        ),
        ("already linked", "New target @inbox:target < @a+f", "all"),
    ] {
        let (_temp, vault, day_file) = setup_merge_vault("merge_in_reconcile");
        enable_blocked_status(&vault);
        for (path, contents) in [
            (
                "a.md",
                "## Tasks\n- [ ] #task Fix the flaky login test ^f\n  - source note a\n",
            ),
            (
                "b.md",
                "## Tasks\n- [ ] #task Login timeout too short ^b\n  - source note b\n",
            ),
            (
                "inbox.md",
                "## Tasks\n- [ ] #task Existing target ^t\n",
            ),
        ] {
            write_file(&vault.join(path), contents);
        }
        write_file(
            &day_file,
            "## Pomodoros\n\n- [ ] () — BUILD\n  - [[a#^f]]\n  - keep this slot\n",
        );
        let before = snapshot(&vault);
        let dry =
            capture_with(&vault, &day_file, draft, true, Some(pick), None);
        assert_success(&dry);
        assert_eq!(snapshot(&vault), before, "{case}: dry-run writes nothing");
        let live =
            capture_with(&vault, &day_file, draft, false, Some(pick), None);
        assert_success(&live);
        let mut dry_json = output_json(&dry);
        dry_json["dry_run"] = false.into();
        let live_json = output_json(&live);
        assert_eq!(dry_json, live_json, "{case}");
        if !case.starts_with("offer") {
            assert!(
                item(&live_json)["merges"][0]["offers"]
                    .as_array()
                    .is_some_and(Vec::is_empty),
                "{case}: target gets no replacement offer: {live_json}"
            );
            assert!(
                !fs::read_to_string(&day_file).unwrap().contains("[[a#^f]]"),
                "{case}: the source link is still pruned"
            );
        }

        let after_capture = snapshot(&vault);
        let reconcile = bob_command()
            .args(["task", "reconcile", "--format", "json"])
            .arg("--bob-dir")
            .arg(&vault)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW)
            .output()
            .expect("run bob task reconcile");
        assert_success(&reconcile);
        assert_eq!(
            snapshot(&vault),
            after_capture,
            "{case}: reconcile is a no-op"
        );
    }
}

#[test]
fn canonical_duplicate_sources_refuse_without_writes() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_duplicate_alias");
    std::fs::create_dir_all(vault.join("areas")).unwrap();
    std::fs::rename(vault.join("a.md"), vault.join("areas/a.md")).unwrap();
    let before = snapshot(&vault);
    let output = capture_json(
        &vault,
        &day_file,
        "New target @inbox < @a+f @areas/a+f",
        false,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(output_json(&output)["code"], "duplicate_capture_target");
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn resolved_marker_conflict_reports_item_and_exact_utf8_span() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_marker_alias_span");
    std::fs::create_dir_all(vault.join("areas")).unwrap();
    std::fs::rename(vault.join("a.md"), vault.join("areas/a.md")).unwrap();
    let draft = "@inbox\n- New café 🧪 &areas/a:f < @a+f";
    let marker_start = draft.find("&areas/a:f").unwrap();
    let before = snapshot(&vault);
    let output = capture_json(&vault, &day_file, draft, false);
    assert!(!output.status.success(), "{}", format_output(&output));
    let error = output_json(&output);
    assert_eq!(error["code"], "merge_in_source_also_marked");
    assert_eq!(error["item"], 1);
    assert_eq!(
        error["span"],
        serde_json::json!({"start": marker_start, "end": marker_start + "&areas/a:f".len()})
    );
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn any_merge_label_incoming_relation_refuses_before_writes() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge_in_relation_label_refusal");
    let config = vault.join("config.yml");
    write_file(&config, "relations:\n  merge_labels: [merged into]\n");
    let draft = "New target @inbox ~~a:f|\"merged into\" < @a+f";
    let marker_start = draft.find("~~a:f|\"merged into\"").unwrap();
    let before = snapshot(&vault);
    let output =
        capture_with(&vault, &day_file, draft, false, None, Some(&config));
    assert!(!output.status.success(), "{}", format_output(&output));
    let error = output_json(&output);
    assert_eq!(error["code"], "merge_in_source_also_marked");
    assert_eq!(error["item"], 1);
    assert_eq!(
        error["span"],
        serde_json::json!({"start": marker_start, "end": marker_start + "~~a:f|\"merged into\"".len()})
    );
    assert_eq!(snapshot(&vault), before);
}

#[test]
fn every_resolved_relation_marker_conflict_refuses_without_writes() {
    let cases = [
        ("dependent", "New target @inbox &a:f < @a+f"),
        ("blocked", "New target @inbox *a:f < @a+f"),
        (
            "cancel_label",
            "New target @inbox ~a:f|\"duplicate of\" < @a+f",
        ),
        (
            "other_merge_source",
            "New target @inbox ~~b:b|\"merged into\" < @a+f",
        ),
    ];
    for (case, draft) in cases {
        let (_temp, vault, day_file) =
            setup_merge_vault("merge_in_marker_matrix");
        let config = vault.join("config.yml");
        write_file(
            &config,
            "relations:\n  cancel_labels: [duplicate of]\n  merge_labels: [merged into]\n",
        );
        let before = snapshot(&vault);
        let output =
            capture_with(&vault, &day_file, draft, false, None, Some(&config));
        assert!(
            !output.status.success(),
            "{case}: {}",
            format_output(&output)
        );
        let json = output_json(&output);
        assert_eq!(
            json["code"], "merge_in_source_also_marked",
            "{case}: {json}"
        );
        assert!(json["span"]["start"].as_u64().is_some(), "{case}: {json}");
        assert_eq!(snapshot(&vault), before, "{case} refusal is atomic");
    }
}

#[test]
fn automatic_id_retry_refuses_closed_source_without_a_second_target() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_auto_retry");
    let draft = "New target @inbox < @a+f";
    let first = capture_json(&vault, &day_file, draft, false);
    assert_success(&first);
    let before_retry = snapshot(&vault);
    let retry = capture_json(&vault, &day_file, draft, false);
    assert!(!retry.status.success(), "{}", format_output(&retry));
    let retry_json = output_json(&retry);
    assert_eq!(retry_json["code"], "merge_source_closed");
    assert!(
        retry_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("New target"),
        "retry identifies the prior merge destination: {retry_json}"
    );
    assert_eq!(snapshot(&vault), before_retry);
}

#[test]
fn explicit_id_retry_collision_refuses_without_writes() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_explicit_retry");
    let draft = "New target @inbox^fixed < @a+f";
    let first = capture_json(&vault, &day_file, draft, false);
    assert_success(&first);
    let before_retry = snapshot(&vault);
    let retry = capture_json(&vault, &day_file, draft, false);
    assert!(!retry.status.success(), "{}", format_output(&retry));
    assert_eq!(snapshot(&vault), before_retry);
}

#[test]
fn incomplete_merge_in_is_refused_without_writes() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_incomplete");
    let before = snapshot(&vault);
    let output = capture_json(&vault, &day_file, "New task < @a", false);
    assert!(
        !output.status.success(),
        "incomplete should refuse: {}",
        format_output(&output)
    );
    assert_eq!(output_json(&output)["code"], "incomplete_merge_in");
    assert_eq!(snapshot(&vault), before);
}

fn pending_json(
    vault: &Path,
    day_file: &Path,
    draft: &str,
) -> serde_json::Value {
    let output = capture_json(vault, day_file, draft, true);
    assert!(
        !output.status.success(),
        "incomplete merge-in should refuse: {}",
        format_output(&output)
    );
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
            .expect("dry-run JSON");
    assert_eq!(json["code"], "incomplete_merge_in", "{json}");
    json
}

#[test]
fn merge_in_pending_reports_single_complete_source_with_mergeability() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-single");
    let before = snapshot(&vault);
    let json = pending_json(&vault, &day_file, "New task < @a+f @b");
    let pending = &json["merge_in_pending"];
    assert_eq!(pending["item"], 1, "{json}");
    assert_eq!(pending["need"], "merge_in_source", "{json}");
    let sources = pending["sources"].as_array().expect("sources array");
    assert_eq!(sources.len(), 1, "{json}");
    let first = &sources[0];
    assert_eq!(first["marker"], "@a+f", "{json}");
    assert_eq!(first["block_id"], "f", "{json}");
    assert_eq!(first["path"], "a.md", "{json}");
    assert_eq!(first["title"], "Fix the flaky login test", "{json}");
    assert_eq!(first["status_name"], "Ready", "{json}");
    assert_eq!(first["mergeable"], true, "{json}");
    assert_eq!(snapshot(&vault), before, "dry run writes nothing");
}

#[test]
fn merge_in_pending_preserves_typed_order_across_complete_sources() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-order");
    let json = pending_json(&vault, &day_file, "New task < @b+b @a+f @c");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 2, "{json}");
    assert_eq!(sources[0]["marker"], "@b+b", "{json}");
    assert_eq!(sources[1]["marker"], "@a+f", "{json}");
    assert_eq!(sources[0]["path"], "b.md", "{json}");
    assert_eq!(sources[1]["path"], "a.md", "{json}");
}

#[test]
fn merge_in_pending_reports_source_level_errors() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-errors");
    let json = pending_json(&vault, &day_file, "New task < @a+typo @b");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 1, "{json}");
    assert_eq!(sources[0]["marker"], "@a+typo", "{json}");
    assert!(
        sources[0]["error"]["code"].is_string(),
        "missing task reports code: {json}"
    );
    assert!(
        sources[0]["error"]["message"].is_string(),
        "missing task reports message: {json}"
    );

    let missing = pending_json(&vault, &day_file, "New task < @missing+f @b");
    let missing_sources = missing["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(missing_sources.len(), 1, "{missing}");
    assert!(missing_sources[0]["error"]["code"].is_string(), "{missing}");
}

#[test]
fn merge_in_pending_reports_closed_source_as_unmergeable() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-closed");
    write_file(
        &vault.join("a.md"),
        &area_with_tasks("- [x] #task Fix the flaky login test ^f\n"),
    );
    let json = pending_json(&vault, &day_file, "New task < @a+f @b");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 1, "{json}");
    assert_eq!(sources[0]["mergeable"], false, "{json}");
    assert_eq!(
        sources[0]["mergeable_code"], "merge_source_closed",
        "{json}"
    );
}

#[test]
fn merge_in_human_output_prints_new_target_header_once() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_human_header");
    let before = snapshot(&vault);
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("New target @inbox^target < @a+f @b+b")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run dry-run capture");
    assert_success(&dry);
    assert_eq!(snapshot(&vault), before, "dry run writes nothing");
    let dry_text = String::from_utf8_lossy(&dry.stdout);
    assert!(dry_text.contains("- [ ] #task New target"), "{dry_text}");
    assert_eq!(
        dry_text
            .matches("⤵ Would merge 2 tasks into new “New target” · inbox.md")
            .count(),
        1,
        "{dry_text}"
    );

    let live =
        capture_human(&vault, &day_file, "New target @inbox^target < @a+f");
    assert_success(&live);
    let text = String::from_utf8_lossy(&live.stdout);
    assert!(text.contains("- [ ] #task New target"), "{text}");
    assert_eq!(text.matches('⤵').count(), 1, "one merge block: {text}");
    assert!(
        text.contains("⤵ Merged 1 task into new “New target” · inbox.md"),
        "{text}"
    );
    assert!(
        text.contains("Fix the flaky login test")
            && text.contains("→ Cancelled · 1 note moved"),
        "{text}"
    );
}

#[test]
fn merge_in_human_offer_prints_the_verdict_receipt() {
    for (dry_run, expected) in [
        (true, "✓ would complete “Existing target” · Ready → Done"),
        (false, "✓ completed “Existing target” · Ready → Done"),
    ] {
        let name = if dry_run {
            "merge_in_verdict_receipt_dry"
        } else {
            "merge_in_verdict_receipt_live"
        };
        let (_temp, vault, day_file) = setup_merge_vault(name);
        write_file(
            &day_file,
            "## Pomodoros\n\n- [ ] () — BUILD\n  - [[a#^f]]\n  - keep this slot\n  - [[b#^b]]\n",
        );
        let output = capture_human_flags(
            &vault,
            &day_file,
            &["-x", "1"],
            "@a+f @b+b > @inbox+t",
            dry_run,
        );
        assert_success(&output);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(&format!("offer 1  {expected}")), "{text}",);
        assert!(!text.contains("(-p 1)"), "{text}");
        assert!(!text.contains("route with -i"), "{text}");
    }
}

#[test]
fn merge_in_pending_mergeability_matches_execution_refusals() {
    let (_temp, vault, day_file) =
        setup_merge_vault("merge-in-pending-engine-parity");
    let first =
        capture_json(&vault, &day_file, "First target @inbox < @a+f", false);
    assert_success(&first);
    write_file(
        &vault.join("b.md"),
        &area_with_tasks(
            "- [ ] #task Login timeout too short ^b\n\t- [ ] #task Open child\n",
        ),
    );

    let json = pending_json(&vault, &day_file, "Second < @a+f @b+b @c");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 2, "{json}");
    for (source, draft) in [
        (&sources[0], "Second @inbox < @a+f"),
        (&sources[1], "Second @inbox < @b+b"),
    ] {
        assert_eq!(source["mergeable"], false, "{json}");
        let execution = capture_json(&vault, &day_file, draft, true);
        assert!(!execution.status.success(), "{}", format_output(&execution));
        let refusal = output_json(&execution);
        assert_eq!(source["mergeable_code"], refusal["code"], "{refusal}");
        let message = source["mergeable_message"].as_str().unwrap_or_default();
        assert!(!message.is_empty(), "{json}");
        assert!(
            refusal["error"]
                .as_str()
                .unwrap_or_default()
                .ends_with(message),
            "pending copy matches execution: {message} vs {refusal}"
        );
    }
    assert!(
        sources[0]["mergeable_message"]
            .as_str()
            .unwrap_or_default()
            .contains("already merged into “First target”"),
        "{json}"
    );
    assert_eq!(sources[1]["mergeable_code"], "merge_source_open_subtasks");
}

#[test]
fn merge_in_pending_finds_the_failing_bulk_item() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-bulk");
    let json = pending_json(
        &vault,
        &day_file,
        "First task @inbox\n\nSecond < @a+f @b",
    );
    assert_eq!(json["merge_in_pending"]["item"], 2, "{json}");
    assert_eq!(
        json["merge_in_pending"]["sources"][0]["marker"], "@a+f",
        "{json}"
    );
}

#[test]
fn merge_in_pending_finds_failing_list_bullet() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-list");
    let json = pending_json(&vault, &day_file, "@inbox\n- Second < @a+f @b");
    assert_eq!(json["merge_in_pending"]["item"], 1, "{json}");
    assert_eq!(
        json["merge_in_pending"]["sources"][0]["marker"], "@a+f",
        "{json}"
    );
}

#[test]
fn merge_in_pending_is_dry_run_json_only() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-commit");
    let real = capture_json(&vault, &day_file, "New task < @a+f @b", false);
    assert!(!real.status.success(), "{}", format_output(&real));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real.stdout).trim())
            .expect("commit JSON");
    assert_eq!(json["code"], "incomplete_merge_in", "{json}");
    assert!(
        json.get("merge_in_pending").is_none(),
        "commit failures carry no pending context: {json}"
    );

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("New task < @a+f @b")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("human incomplete");
    assert!(!human.status.success(), "{}", format_output(&human));
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("pick tasks to merge in"), "{stderr}");
    assert!(!stderr.contains("merge_in_pending"), "{stderr}");
}

#[test]
fn shell_ate_merge_in_arrow_refuses_argv_with_at_stdin_redirect() {
    let temp = TempDir::new("bob-cli-merge-in-shell-guard");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let before = fs::read_to_string(&day_file).expect("read day");
    let at_file = temp.path().join("@a+x");
    fs::write(&at_file, "stale stdin").expect("write @ stdin");
    let stdin = fs::File::open(&at_file).expect("open @ stdin");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("New task")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(stdin))
        .output()
        .expect("run capture with @ stdin");
    assert!(
        !output.status.success(),
        "guard must refuse: {}",
        format_output(&output)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("your shell ate"), "{err}");
    assert!(err.contains("quote the draft"), "{err}");
    assert!(err.contains("New task < @a+x"), "{err}");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        before,
        "vault unchanged when the shell ate `<`"
    );

    // JSON failures carry the machine code.
    let json_stdin = fs::File::open(&at_file).expect("reopen @ stdin");
    let json_out = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("New task")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(json_stdin))
        .output()
        .expect("json guard");
    assert!(!json_out.status.success(), "{}", format_output(&json_out));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&json_out.stdout).trim())
            .expect("guard JSON");
    assert_eq!(json["code"], "shell_ate_merge_in_arrow", "{json}");
}

#[test]
fn shell_ate_merge_in_allows_plain_files_pipes_and_stdin_capture() {
    let temp = TempDir::new("bob-cli-merge-in-shell-guard-ok");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");

    // A plain (non-`@`) regular stdin file with argv does not trigger.
    let plain = temp.path().join("plain.md");
    fs::write(&plain, "stale stdin").expect("write plain stdin");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(fs::File::open(&plain).expect("open plain")))
        .output()
        .expect("run capture with plain stdin");
    assert_success(&output);

    // Piped stdin (not a regular file) with argv does not trigger.
    let piped = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("pipe ok")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture with piped stdin");
    assert_success(&piped);

    // Normal stdin capture without argv reads the draft from stdin: an
    // `@`-named stdin file is the draft source, not a shell-eaten `<`.
    let draft_file = temp.path().join("@draft");
    fs::write(&draft_file, "stdin task").expect("write draft stdin");
    let from_stdin = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(
            fs::File::open(&draft_file).expect("open draft"),
        ))
        .output()
        .expect("run stdin capture");
    assert_success(&from_stdin);
}
