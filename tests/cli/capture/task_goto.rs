//! Whole-draft `/query` Go To grammar: parse, refusal, rollback, candidates.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn t3(token: &str) -> String {
    format!(
        "`{token}` opens the Go To picker; pick a task to open it in Obsidian — it never captures"
    )
}

fn write_worked_example(vault: &Path) -> PathBuf {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Call the bank [created::2026-09-29]\n",
    );
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Errands\n- [?] #task Book dentist [scheduled::2026-10-03]\n",
    );
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Polish capture picker ^polish\n\t- [ ] #task Tune fuzzy weights\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n## Bugs\n- [*] #task Fix deep bug ^deep-fix\n- [ ] #task Fix flaky gkeep test\n- [x] #task Old fix ^old-fix\n## Writing\n- [/] #task Draft outline ^outline\n- [ ] #task Ship blog post #now ^blog\n- [-] #task Dropped idea\n",
    );
    write_file(
        &vault.join("archive.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Leftover ^leftover\n",
    );
    write_file(&vault.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
    let day_file = vault.join("2026/20260930.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n",
    );
    day_file
}

fn snapshot_vault(vault: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut names: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read vault dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        names.sort();
        for path in names {
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).expect("read vault file");
                entries.push((path, bytes));
            }
        }
    }
    entries.sort();
    entries
}

fn capture_output(
    vault: &Path,
    day_file: &Path,
    args: &[&str],
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob capture")
}

fn complete_json(
    vault: &Path,
    day_file: &Path,
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run goto completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

#[test]
fn capture_goto_query_fails_with_the_t3_teaching_error() {
    let temp = TempDir::new("bob-cli-capture-task-goto");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("/dee")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run goto query capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!(
            "bob capture: capture item 1 starting on line 1: {}",
            t3("/dee")
        ),
        "unexpected teaching error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "an unfinished query must not create a junk inbox task"
    );
}

#[test]
fn capture_goto_refusals_cover_slash_forms_and_leave_the_vault_untouched() {
    let temp = TempDir::new("bob-cli-capture-task-goto-refusals");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);
    let before = snapshot_vault(&vault);

    // `capture` and `--dry-run` refuse every claim row; forced flags
    // change nothing. `/a&b` proves the refusal runs before the `&` scan.
    let cases: Vec<Vec<&str>> = vec![
        vec!["/"],
        vec!["/fix"],
        vec!["/a&b"],
        vec!["-r", "inbox", "/fix"],
    ];
    for args in &cases {
        let output = capture_output(&vault, &day_file, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "a picker query is a usage error ({args:?}):\n{}",
            format_output(&output)
        );
        let token = args.last().expect("token");
        assert_eq!(
            stderr(&output).trim(),
            format!(
                "bob capture: capture item 1 starting on line 1: {}",
                t3(token)
            ),
            "unexpected teaching error ({args:?}):\n{}",
            format_output(&output)
        );
        let dry_run: Vec<&str> = std::iter::once("--dry-run")
            .chain(args.iter().copied())
            .collect();
        let output = capture_output(&vault, &day_file, &dry_run);
        assert_eq!(
            output.status.code(),
            Some(2),
            "a dry run refuses too ({args:?}):\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(&t3(token)),
            "dry run teaches Go To ({args:?}):\n{}",
            format_output(&output)
        );
        assert_eq!(
            snapshot_vault(&vault),
            before,
            "a refusal must not change vault bytes ({args:?})"
        );
    }
}

#[test]
fn capture_batch_with_a_slash_item_captures_both_items_as_usual() {
    // Unlike `:`, the `/` claim is per draft: a second item keeps both
    // items prose, and both capture as usual.
    let temp = TempDir::new("bob-cli-capture-task-goto-batch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15");
    let output = run_with_stdin(&mut command, "Buy milk\n\n/fix\n");

    assert_success(&output);
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(inbox.contains("Buy milk"), "{inbox}");
    assert!(inbox.contains("/fix"), "{inbox}");
}

#[test]
fn capture_parse_goto_query_reports_incomplete_task_goto() {
    for (text, end) in [("/", 1), ("/fix", 4)] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run bob capture-parse json");

        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        assert_eq!(json["ok"], true);
        assert_eq!(json["mode"], "incomplete");
        assert_eq!(json["needs"], serde_json::json!(["task_goto"]));
        assert_eq!(json["body"], "");
        assert!(json["route"].is_null(), "{json}");
        assert!(json["section"].is_null(), "{json}");
        assert!(json["block_id"].is_null(), "{json}");
        assert_eq!(
            json["spans"],
            serde_json::json!([{"start": 0, "end": end, "kind": "interactive_placeholder"}]),
            "{json}"
        );
        assert_eq!(json["diagnostics"], serde_json::json!([]), "{json}");
    }
}

#[test]
fn capture_parse_goto_prose_rows_keep_their_current_parse() {
    // A second `/`, several tokens, child lines, sibling items, and a `@@`
    // declaration never claim Go To.
    for text in ["//", "/Users/x", "/r/rust", "/a b", "Buy /x"] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run bob capture-parse json");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        assert_ne!(
            json["needs"],
            serde_json::json!(["task_goto"]),
            "{text}: {json}"
        );
    }
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("/a b")
        .output()
        .expect("run bob capture-parse json");
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "/a b", "{json}");

    for text in ["/x\n- x", "/x\n\nBuy milk", "@@work\n/x"] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run bob capture-parse json");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        let needs = json.get("needs").cloned().unwrap_or_default();
        assert_ne!(needs, serde_json::json!(["task_goto"]), "{text:?}: {json}");
    }
}

#[test]
fn capture_complete_goto_query_reports_task_goto_without_query_or_picker() {
    let temp = TempDir::new("bob-cli-capture-task-goto-complete");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    for (text, cursor, end) in [("/", 1, 1), ("/sas", 4, 4)] {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-b")
            .arg(&vault)
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run bob capture-complete json");

        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-complete JSON");
        assert_eq!(json["ok"], true);
        assert_eq!(json["context"], "task_goto");
        assert_eq!(
            json["replacement"],
            serde_json::json!({"start": 0, "end": end}),
            "{json}"
        );
        assert!(json.get("query").is_none(), "{json}");
        assert!(json.get("picker").is_none(), "{json}");
        assert_eq!(json["candidates"], serde_json::json!([]), "{json}");
    }
}

#[test]
fn complete_goto_candidates_carry_note_path_and_target_without_link_keys() {
    let temp = TempDir::new("bob-cli-complete-task-goto-keys");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("/")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run goto completion");
    assert_success(&output);
    let raw = stdout(&output).to_string();
    let json: serde_json::Value = serde_json::from_str(raw.trim())
        .unwrap_or_else(|error| {
            panic!("completion stdout should be JSON: {error}\n{raw}")
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "task_goto");
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert_eq!(candidates.len(), 8, "{json}");

    // Keys appear in the §4 contract order in the raw JSON (serde emits
    // struct fields in order); the parsed map below only checks the set.
    let ordered = [
        "replacement",
        "ref",
        "route",
        "note_kind",
        "note_path",
        "target",
        "block_id",
        "status_symbol",
        "status_name",
        "status_type",
        "text",
        "section",
        "depth",
        "line",
        "group",
        "scheduled",
        "pomodoro",
    ];
    let body = raw
        .split_once("\"candidates\":")
        .expect("candidates array")
        .1;
    let mut cursor = 0;
    for key in ordered {
        let needle = format!("\"{key}\":");
        let at = body[cursor..].find(&needle).unwrap_or_else(|| {
            panic!("missing key {key} in candidate order: {body}")
        });
        cursor += at + needle.len();
    }

    for candidate in candidates {
        let mut keys: Vec<&str> = candidate
            .as_object()
            .expect("candidate object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = ordered.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{candidate}");
        assert_eq!(candidate["replacement"], "", "{candidate}");
    }

    // Area, project, and inbox rows carry exact vault-relative paths and
    // absolute targets.
    let by_text = |text: &str| {
        candidates
            .iter()
            .find(|candidate| candidate["text"] == text)
            .unwrap_or_else(|| panic!("missing row {text:?}: {json}"))
            .clone()
    };
    let bank = by_text("Call the bank");
    assert_eq!(bank["route"], "mac_inbox");
    assert_eq!(bank["note_kind"], "inbox");
    assert_eq!(bank["note_path"], "mac_inbox.md");
    assert_eq!(
        bank["target"],
        vault.join("mac_inbox.md").display().to_string()
    );
    let dentist = by_text("Book dentist");
    assert_eq!(dentist["route"], "health");
    assert_eq!(dentist["note_kind"], "area");
    assert_eq!(dentist["note_path"], "health.md");
    assert_eq!(
        dentist["target"],
        vault.join("health.md").display().to_string()
    );
    assert_eq!(dentist["scheduled"], "2026-10-03");
    assert!(dentist.get("pulls_forward").is_none(), "{dentist}");
    let bug = by_text("Fix deep bug");
    assert_eq!(bug["route"], "sase");
    assert_eq!(bug["note_kind"], "project");
    assert_eq!(bug["note_path"], "sase.md");
    assert_eq!(bug["target"], vault.join("sase.md").display().to_string());
    assert_eq!(bug["block_id"], "deep-fix");

    // ID-less rows list with a null block ID and no Add block ID metadata.
    let flaky = by_text("Fix flaky gkeep test");
    assert_eq!(flaky["block_id"], serde_json::Value::Null);
    assert!(flaky.get("requires_block_id").is_none(), "{flaky}");
    assert!(flaky.get("block_id_suggestions").is_none(), "{flaky}");
}

#[test]
fn complete_goto_route_ref_sequence_equals_task_link() {
    let temp = TempDir::new("bob-cli-complete-task-goto-parity");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    for (goto_text, goto_cursor, link_text, link_cursor) in [
        ("/", 1, ":", 1),
        ("/sas", 4, ":sas", 4),
        ("/fix", 4, ":fix", 4),
    ] {
        let goto = complete_json(&vault, &day_file, goto_cursor, goto_text);
        let link = complete_json(&vault, &day_file, link_cursor, link_text);
        let sequence = |value: &serde_json::Value| {
            value["candidates"]
                .as_array()
                .expect("candidates array")
                .iter()
                .map(|candidate| {
                    (
                        candidate["route"].as_str().expect("route").to_string(),
                        candidate["ref"].as_str().expect("ref").to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sequence(&goto),
            sequence(&link),
            "goto and link disagree on {goto_text:?}"
        );
    }
}

#[test]
fn complete_goto_human_lines_show_note_path_and_line() {
    let temp = TempDir::new("bob-cli-complete-task-goto-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    let human = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("--")
        .arg("/")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run human goto completion");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("sase.md:6"), "{out}");
    assert!(out.contains("[*] Fix deep bug  · sase.md:6"), "{out}");
    assert!(
        out.contains("[ ] Fix flaky gkeep test  · sase.md:7"),
        "{out}"
    );
    assert!(out.contains("[ ] Call the bank  · mac_inbox.md:4"), "{out}");
    assert!(!out.contains("needs ID"), "{out}");
    assert!(out.contains("8 candidates"), "{out}");
}

#[test]
fn shell_completion_serves_no_goto_rows() {
    let temp = TempDir::new("bob-cli-complete-task-goto-shell");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let _day_file = write_worked_example(&vault);

    let output = bob_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .arg("/")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob __complete");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        !out.contains("Fix deep bug"),
        "nothing is insertable: {out}"
    );
    assert!(
        !out.contains("Call the bank"),
        "nothing is insertable: {out}"
    );
}
