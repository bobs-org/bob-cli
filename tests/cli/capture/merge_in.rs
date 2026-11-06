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
        .env("BOB_NOW", NOW);
    if let Some(config) = config {
        command.env("BOB_CONFIG_FILE", config);
    }
    command.output().expect("run capture")
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
    let draft = "New target @inbox < @a+f @b+b\n- authored note";
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
    let source_a = inbox.find("source note a").expect("source A payload lands");
    let source_b = inbox.find("source note b").expect("source B payload lands");
    assert!(authored < source_a && source_a < source_b, "{inbox}");
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
fn automatic_id_retry_refuses_closed_source_without_a_second_target() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_auto_retry");
    let draft = "New target @inbox < @a+f";
    let first = capture_json(&vault, &day_file, draft, false);
    assert_success(&first);
    let before_retry = snapshot(&vault);
    let retry = capture_json(&vault, &day_file, draft, false);
    assert!(!retry.status.success(), "{}", format_output(&retry));
    assert_eq!(output_json(&retry)["code"], "merge_source_closed");
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
