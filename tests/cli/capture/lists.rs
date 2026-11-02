//! Capture lists: several tasks or notes under one lone `@file` header.
//!
//! A lone `@file` (or `@file#section`) header line followed by bullets files
//! each first-level bullet as its own item, routed by the header. These are
//! the phase `list-capture` contract vectors: the worked example, list/batch
//! equivalence, `:` link guards, duplicate IDs, the refusal table (parse and
//! execution agree on every code), header variants, whole-item forms staying
//! text, atomicity, project-note parity, and unchanged forms.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-11-02 10:00:00";

fn vault_with_dev(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Tasks\n");
    vault
}

fn day_file(vault: &std::path::Path) -> std::path::PathBuf {
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    day
}

/// Run `bob capture` (dry-run JSON unless `write` is set) over `draft`.
fn run_capture(
    vault: &std::path::Path,
    day: &std::path::Path,
    draft: &str,
    write: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if !write {
        command.arg("--dry-run");
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0");
    command.output().expect("run capture")
}

fn dry_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    draft: &str,
) -> Value {
    let output = run_capture(vault, day, draft, false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn run_parse(draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

/// Every diagnostic code on the top-level item and each `items[]` entry.
fn parse_codes(json: &Value) -> Vec<String> {
    let mut codes = json["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(items) = json["items"].as_array() {
        for item in items {
            codes.extend(
                item["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|diagnostic| diagnostic["code"].as_str())
                    .map(str::to_string),
            );
        }
    }
    codes.sort();
    codes.dedup();
    codes
}

/// Assert `bob capture` refuses `draft` with `code`, writes nothing, and
/// `capture-parse` reports the same `code` as a diagnostic.
fn assert_refusal(
    vault: &std::path::Path,
    day: &std::path::Path,
    before: &[(std::path::PathBuf, String)],
    draft: &str,
    code: &str,
) {
    let output = run_capture(vault, day, draft, false);
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected usage refusal:\n{}",
        format_output(&output)
    );
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], code, "{json}");
    for (path, contents) in before {
        assert_eq!(
            fs::read_to_string(path).expect("read vault file"),
            *contents,
            "refused capture must not write {}",
            path.display()
        );
    }
    let parse = run_parse(draft);
    assert!(
        parse_codes(&parse).contains(&code.to_string()),
        "capture-parse must report `{code}` too:\n{}",
        serde_json::to_string_pretty(&parse).expect("pretty parse")
    );
}

#[test]
fn capture_list_worked_example_writes_both_files_byte_for_byte() {
    // The plan's worked example: three bullets under `@dev`, one linked
    // with `:id`, one named with `^id` and scheduled, one plain. The dev
    // note gains three tasks; today's daily note gains the `[[dev#^…]]`
    // link under the Pomodoro the default type places.
    let temp = TempDir::new("bob-cli-capture-list-worked-example");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    write_file(
        &vault.join("dev.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [ ] #task Existing thing [created::2026-10-01] ^existing\n",
        ),
    );
    let draft = concat!(
        "@dev\n",
        "- Fix flaky gkeep test :fix-flaky-gkeep\n",
        "  - repro on CI first\n",
        "- Write release notes ^release-notes s:2\n",
        "- Ping Alice about the cutover",
    );

    let json = dry_json(&vault, &day, draft);
    assert_eq!(json["ok"], true);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 3);
    for (index, capture) in captures.iter().enumerate() {
        let position = index as u64 + 1;
        assert_eq!(
            capture["list"],
            serde_json::json!({
                "kind": "tasks",
                "route": "dev",
                "section": null,
                "header_line": 1,
                "position": position,
                "count": 3,
            }),
            "{capture}"
        );
    }
    assert_eq!(captures[0]["kind"], "pomodoro_task");
    assert_eq!(captures[0]["block_id"], "fix-flaky-gkeep");
    assert_eq!(captures[1]["kind"], "task");
    assert_eq!(captures[1]["block_id"], "release-notes");

    let write_output = run_capture(&vault, &day, draft, true);
    assert_success(&write_output);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [ ] #task Existing thing [created::2026-10-01] ^existing\n",
            "- [*] #task Fix flaky gkeep test [created::2026-11-02] ^fix-flaky-gkeep\n",
            "\t- repro on CI first\n",
            "- [?] #task Write release notes [created::2026-11-02] [scheduled::2026-11-04] ^release-notes\n",
            "- [ ] #task Ping Alice about the cutover [created::2026-11-02]\n",
        )
    );
    let day_after = fs::read_to_string(&day).expect("read day");
    assert!(
        day_after.contains("[[dev#^fix-flaky-gkeep]]"),
        "daily note must link the Next task:\n{day_after}"
    );
    assert!(
        !day_after.contains("[[dev#^release-notes]]"),
        "the scheduled task stays unlinked:\n{day_after}"
    );

    // The human run leads with one `list` summary line per list.
    let mut human = bob_command();
    human
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("@dev\n- Fresh task")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0");
    let human_output = human.output().expect("run human capture");
    assert_success(&human_output);
    assert!(
        stdout(&human_output).contains("list  dev.md · 1 task"),
        "human summary names the list:\n{}",
        format_output(&human_output)
    );
}

fn capture_real(
    vault: &std::path::Path,
    day: &std::path::Path,
    draft: &str,
) -> Value {
    let output = run_capture(vault, day, draft, true);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

#[test]
fn capture_list_matches_equivalent_batch_byte_for_byte() {
    // The equivalence invariant: when no nested line starts with `#`, a
    // list writes byte-for-byte what the equivalent blank-line batch
    // writes — the same dev note and the same daily note.
    let list_draft = concat!(
        "@dev\n",
        "- Plain task\n",
        "- Named task ^named-one\n",
        "- Linked task :linked-one\n",
        "- Scheduled task s:3\n",
        "- Dependent task &dev:linked-one\n",
        "- Parent task\n",
        "  - nested child s:1",
    );
    let batch_draft = concat!(
        "Plain task @dev\n",
        "\n",
        "Named task @dev^named-one\n",
        "\n",
        "Linked task @dev:linked-one\n",
        "\n",
        "Scheduled task @dev s:3\n",
        "\n",
        "Dependent task @dev &dev:linked-one\n",
        "\n",
        "Parent task @dev\n",
        "- nested child s:1",
    );

    let temp = TempDir::new("bob-cli-capture-list-equivalence");
    let list_vault = temp.path().join("list-vault");
    fs::create_dir_all(&list_vault).expect("create list vault");
    write_file(&list_vault.join("dev.md"), "# Tasks\n");
    write_blocked_tasks_settings(&list_vault);
    let list_day = day_file(&list_vault);
    capture_real(&list_vault, &list_day, list_draft);

    let batch_vault = temp.path().join("batch-vault");
    fs::create_dir_all(&batch_vault).expect("create batch vault");
    write_file(&batch_vault.join("dev.md"), "# Tasks\n");
    write_blocked_tasks_settings(&batch_vault);
    let batch_day = day_file(&batch_vault);
    capture_real(&batch_vault, &batch_day, batch_draft);

    assert_eq!(
        fs::read_to_string(list_vault.join("dev.md")).expect("read list dev"),
        fs::read_to_string(batch_vault.join("dev.md")).expect("read batch dev"),
        "list and batch must write identical notes"
    );
    assert_eq!(
        fs::read_to_string(&list_day).expect("read list day"),
        fs::read_to_string(&batch_day).expect("read batch day"),
        "list and batch must link identically"
    );
    let dev = fs::read_to_string(list_vault.join("dev.md")).expect("read dev");
    assert!(dev.contains("- [ ] #task Plain task [created::2026-11-02]\n"));
    assert!(dev.contains(
        "- [?] #task Scheduled task [created::2026-11-02] [scheduled::2026-11-05]\n"
    ));
    assert!(dev.contains(
        "- [?] #task Parent task [created::2026-11-02] [scheduled::2026-11-03]\n\t- nested child\n"
    ));
}

#[test]
fn capture_section_list_matches_equivalent_batch_byte_for_byte() {
    let temp = TempDir::new("bob-cli-capture-section-list-equivalence");
    let list_vault = temp.path().join("list-vault");
    fs::create_dir_all(&list_vault).expect("create list vault");
    write_file(&list_vault.join("dev.md"), "# Dev\n## Ideas\nnotes\n");
    let list_day = day_file(&list_vault);
    let list_json = capture_real(
        &list_vault,
        &list_day,
        "@dev#Ideas\n- Big idea\n- Small idea",
    );
    assert_eq!(
        list_json["captures"][0]["list"],
        serde_json::json!({
            "kind": "notes",
            "route": "dev",
            "section": "Ideas",
            "header_line": 1,
            "position": 1,
            "count": 2,
        })
    );

    let batch_vault = temp.path().join("batch-vault");
    fs::create_dir_all(&batch_vault).expect("create batch vault");
    write_file(&batch_vault.join("dev.md"), "# Dev\n## Ideas\nnotes\n");
    let batch_day = day_file(&batch_vault);
    capture_real(
        &batch_vault,
        &batch_day,
        "Big idea @dev#Ideas\n\nSmall idea @dev#Ideas",
    );

    assert_eq!(
        fs::read_to_string(list_vault.join("dev.md")).expect("read list dev"),
        fs::read_to_string(batch_vault.join("dev.md")).expect("read batch dev"),
        "section list and batch must write identical notes"
    );
    assert!(fs::read_to_string(list_vault.join("dev.md"))
        .expect("read dev")
        .contains("- Small idea [created::2026-11-02]\n"));
}

#[test]
fn capture_section_list_keeps_ids_literal() {
    // Outside task bullets, ` :id` / ` ^id` stay literal text and open no
    // prompt: section bullets never take IDs.
    let temp = TempDir::new("bob-cli-capture-section-list-literal-id");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    write_file(&vault.join("dev.md"), "# Dev\n## Ideas\nnotes\n");
    let json = capture_real(
        &vault,
        &day,
        "@dev#Ideas\n- Note with trailing :keep-me\n- Other ^keep-too",
    );
    assert_eq!(json["captures"].as_array().expect("captures").len(), 2);
    let dev = fs::read_to_string(vault.join("dev.md")).expect("read dev");
    assert!(
        dev.contains("- Note with trailing :keep-me [created::2026-11-02]\n")
    );
    assert!(dev.contains("- Other ^keep-too [created::2026-11-02]\n"));
}

#[test]
fn capture_list_colon_link_guards() {
    // A future `s:<N>` on a `:` bullet files the task `[?]` and unlinked;
    // a blocked destination project does the same with `#prj_is_blocked`.
    let temp = TempDir::new("bob-cli-capture-list-link-guards");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Tasks\n");
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n",
            "\n",
            "- [ ] #task Ship it [scheduled:: 2026-12-01] ^prj\n",
        ),
    );
    let day = day_file(&vault);

    let future =
        capture_real(&vault, &day, "@dev\n- Future thing :future-thing s:5");
    assert!(future["task_line"]
        .as_str()
        .unwrap_or_default()
        .starts_with("- [?] #task Future thing"));
    assert_eq!(future["block_id"], "future-thing");
    let dev = fs::read_to_string(vault.join("dev.md")).expect("read dev");
    assert!(dev.contains(
        "- [?] #task Future thing [created::2026-11-02] [scheduled::2026-11-07] ^future-thing\n"
    ));
    let day_after_future = fs::read_to_string(&day).expect("read day");
    assert!(
        !day_after_future.contains("[[dev#^future-thing]]"),
        "a scheduled task stays unlinked:\n{day_after_future}"
    );

    let blocked = capture_real(&vault, &day, "@proj\n- New thing :new-thing");
    assert!(blocked["task_line"]
        .as_str()
        .unwrap_or_default()
        .starts_with("- [?] #task New thing"));
    assert_eq!(blocked["block_id"], "new-thing");
    let proj = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(proj.contains(
        "- [?] #task New thing [created::2026-11-02] #prj_is_blocked ^new-thing\n"
    ));
    let day_after_blocked = fs::read_to_string(&day).expect("read day");
    assert!(
        !day_after_blocked.contains("[[proj#^new-thing]]"),
        "a blocked-destination task stays unlinked:\n{day_after_blocked}"
    );
}

#[test]
fn capture_list_duplicate_ids_are_refused() {
    // Two equal IDs in one list name both lines; an ID already in the
    // note fails at plan time. Nothing is written either way.
    let temp = TempDir::new("bob-cli-capture-list-duplicate-ids");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    let before = fs::read_to_string(vault.join("dev.md")).expect("read dev");

    let within = run_capture(
        &vault,
        &day,
        "@dev\n- Alpha ^same-id\n- Beta ^same-id",
        false,
    );
    assert_eq!(within.status.code(), Some(2));
    let within_json: Value =
        serde_json::from_str(stdout(&within).trim()).expect("capture JSON");
    assert_eq!(within_json["code"], "duplicate_project_task_id");
    assert!(
        within_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("already used on line 2"),
        "duplicate names both lines: {}",
        within_json["error"]
    );
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before
    );
    let parse = run_parse("@dev\n- Alpha ^same-id\n- Beta ^same-id");
    assert!(
        parse_codes(&parse).contains(&"duplicate_project_task_id".to_string()),
        "parse agrees: {}",
        serde_json::to_string_pretty(&parse).expect("pretty parse")
    );

    write_file(
        &vault.join("dev.md"),
        "# Tasks\n- [ ] #task Taken ^taken-id\n",
    );
    let against_before =
        fs::read_to_string(vault.join("dev.md")).expect("read dev");
    let against = run_capture(
        &vault,
        &day,
        "@dev\n- Other ^fresh-id\n- Clash :taken-id",
        false,
    );
    assert!(!against.status.success());
    let against_json: Value =
        serde_json::from_str(stdout(&against).trim()).expect("capture JSON");
    assert_eq!(against_json["ok"], false);
    assert!(
        against_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "duplicate against the note: {}",
        against_json["error"]
    );
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        against_before
    );
}

fn refusal_vault(
    name: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    Vec<(std::path::PathBuf, String)>,
) {
    let temp = TempDir::new(name);
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    let before = vec![(
        vault.join("dev.md"),
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
    )];
    (temp, vault, day, before)
}

#[test]
fn capture_list_empty_header_needs_list_item() {
    // A header with no bullets captures nothing: `capture-parse` reports
    // `incomplete` needing `list_item` with an empty list, and `bob
    // capture` refuses `empty_list`. This replaces today's literal `@dev`
    // inbox task.
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-empty");
    for draft in ["@dev", "@dev#Ideas", "@dev\n- "] {
        let parse = run_parse(draft);
        assert_eq!(parse["mode"], "incomplete", "{draft}");
        assert_eq!(parse["needs"], serde_json::json!(["list_item"]), "{draft}");
        assert_eq!(parse["list"]["count"], 0, "{draft}");
        assert!(
            parse["diagnostics"].as_array().unwrap().is_empty(),
            "{draft}"
        );

        let output = run_capture(&vault, &day, draft, false);
        assert_eq!(output.status.code(), Some(2), "{draft}");
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["code"], "empty_list", "{draft}");
    }
    for (path, contents) in &before {
        assert_eq!(&fs::read_to_string(path).expect("read file"), contents);
    }
}

#[test]
fn capture_list_header_marker_is_refused() {
    // A route marker plus item-wide markers on the header line: the header
    // stands alone, so the marker must move to the bullet it configures.
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-header-marker");
    assert_refusal(
        &vault,
        &day,
        &before,
        "@dev s:3\n- Call the bank",
        "list_header_marker",
    );
    assert_refusal(
        &vault,
        &day,
        &before,
        "@dev#Ideas s:3\n- Big idea",
        "list_header_marker",
    );
}

#[test]
fn capture_list_bullet_routing_markers_are_refused() {
    // Every bullet files into the header's note; anything that would route
    // one elsewhere is refused with the per-marker suggestion.
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-bullet-marker");
    for draft in [
        "@dev\n- A @cash",
        "@dev\n- A +query",
        "@dev\n- A %n",
        "@dev\n- A %l",
        "@dev\n- A !cash:x",
        "@dev#Ideas\n- A @cash",
    ] {
        assert_refusal(&vault, &day, &before, draft, "list_bullet_marker");
    }
}

#[test]
fn capture_list_template_body_header_is_refused() {
    // A header in a `%t` template body is refused in execution, and the
    // template view reports the same code so both sides agree.
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-template");
    assert_refusal(
        &vault,
        &day,
        &before,
        "@dev %t\n- detail {0}\n\nFoo",
        "template_list",
    );
}

#[test]
fn capture_list_checkbox_and_drawer_positions_are_refused() {
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-checkbox-drawer");
    for draft in ["@dev\n- [x] Done thing", "@dev\n- [?] Maybe thing"] {
        assert_refusal(&vault, &day, &before, draft, "list_bullet_checkbox");
    }
    for draft in ["@dev\n- #fix stuff", "@dev\n- #"] {
        assert_refusal(&vault, &day, &before, draft, "drawer_bullet_position");
    }
    assert_refusal(
        &vault,
        &day,
        &before,
        "@dev#Ideas\n- #fix stuff",
        "drawer_bullet_position",
    );
}

#[test]
fn capture_list_misplaced_task_id_names_the_owner_bullet() {
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-misplaced-id");
    assert_refusal(
        &vault,
        &day,
        &before,
        "@dev\n- Owner task\n  - detail :owned-id",
        "misplaced_project_task_id",
    );
}

#[test]
fn capture_list_unfinished_id_needs_block_id() {
    // A lone ` :` / ` ^` ending a bullet or a nested line is an unfinished
    // ID: `capture-parse` reports `incomplete` needing `block_id` with an
    // interactive placeholder and no diagnostic, and `bob capture` refuses.
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-unfinished-id");
    for draft in ["@dev\n- Owner task :", "@dev\n- Owner task\n  - detail :"] {
        let parse = run_parse(draft);
        assert_eq!(parse["mode"], "incomplete", "{draft}");
        assert_eq!(parse["needs"], serde_json::json!(["block_id"]), "{draft}");
        assert!(
            parse["diagnostics"].as_array().unwrap().is_empty(),
            "{draft}"
        );
        let items = parse["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "{draft}");
        assert_eq!(items[0]["mode"], "incomplete", "{draft}");
        assert_eq!(
            items[0]["needs"],
            serde_json::json!(["block_id"]),
            "{draft}"
        );

        let output = run_capture(&vault, &day, draft, false);
        assert_eq!(output.status.code(), Some(2), "{draft}");
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["code"], "invalid_project_task_id", "{draft}");
    }
    for (path, contents) in &before {
        assert_eq!(&fs::read_to_string(path).expect("read file"), contents);
    }

    for draft in [
        "@dev\n- Fix flaky gkeep test :",
        "@dev\n- Fix flaky gkeep test\n  - reproduce on CI :",
        "Finish it @cash^goog-exit@\n- Fix flaky gkeep test :",
        "Finish it @cash^goog-exit@\n- Fix flaky gkeep test\n  - reproduce on CI :",
    ] {
        let output = run_capture(&vault, &day, draft, false);
        assert_eq!(output.status.code(), Some(2), "{draft}");
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        let error = json["error"].as_str().expect("refusal text");
        assert!(error.contains("Fix flaky gkeep test"), "{error}");
        assert!(error.contains(" :fix-flaky-gkeep"), "{error}");
    }
}

#[test]
fn capture_list_invalid_task_ids_are_refused() {
    let (_temp, vault, day, before) =
        refusal_vault("bob-cli-capture-list-invalid-id");
    for draft in [
        "@dev\n- Bad :bad!id",
        "@dev\n- Bad :prj",
        "@dev\n- :orphan-id",
        "@dev\n- :dee",
    ] {
        assert_refusal(&vault, &day, &before, draft, "invalid_project_task_id");
    }
}

#[test]
fn capture_list_whole_item_forms_stay_text() {
    // Whole-item forms do not apply to list bullets: each bullet is always
    // task text with trailing markers, exactly like any later line.
    let temp = TempDir::new("bob-cli-capture-list-whole-item-forms");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    for (draft, body) in [
        ("@dev\n- x", "x"),
        ("@dev\n- =", "="),
        ("@dev\n- +2", "+2"),
        (
            "@dev\n- https://example.com/task",
            "https://example.com/task",
        ),
    ] {
        let parse = run_parse(draft);
        let items = parse["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "{draft}");
        assert_eq!(items[0]["mode"], "task", "{draft}");
        assert_eq!(items[0]["body"], body, "{draft}");
    }
    // A caret link never applies either: `^dev:foo` lexes as an ID token,
    // so the ID-charset rule refuses it in both sides.
    assert_refusal(
        &vault,
        &day,
        &[(
            vault.join("dev.md"),
            fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        )],
        "@dev\n- ^dev:foo",
        "invalid_project_task_id",
    );

    let json = capture_real(&vault, &day, "@dev\n- x");
    assert_eq!(json["text"], "x");
    assert!(fs::read_to_string(vault.join("dev.md"))
        .expect("read dev")
        .contains("- [ ] #task x [created::2026-11-02]\n"));
}

#[test]
fn capture_list_bare_at_header_refuses_incomplete_route() {
    // A bare `@` header with bullets is an unfinished route: `capture-parse`
    // reports `incomplete` needing `route`, and `bob capture` refuses with
    // the incomplete-route copy instead of filing literally. Forced
    // destinations keep their literal behavior (covered elsewhere).
    let parse = run_parse("@\n- A task");
    assert_eq!(parse["mode"], "incomplete");
    assert_eq!(parse["needs"], serde_json::json!(["route"]));

    let temp = TempDir::new("bob-cli-capture-list-bare-at");
    let vault = vault_with_dev(&temp);
    write_file(&vault.join("mac_inbox.md"), "# Tasks\n");
    let day = day_file(&vault);
    let before_dev =
        fs::read_to_string(vault.join("dev.md")).expect("read dev");
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    let before_day = fs::read_to_string(&day).expect("read day");
    let output = run_capture(&vault, &day, "@\n- A task", false);
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected refusal:\n{}",
        format_output(&output)
    );
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "incomplete_route", "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before_dev,
        "refusal must not write dev.md"
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        before_inbox,
        "refusal must not write inbox"
    );
    assert_eq!(
        fs::read_to_string(&day).expect("read day"),
        before_day,
        "refusal must not write day"
    );
}

#[test]
fn capture_list_quoted_section_headers_match_exactly() {
    // Quoted one-word and multi-word section headers decode through the
    // exact-name lexer, preserve exact-match semantics into execution,
    // and agree between parse and execution. Unquoted stays prefix.
    // Two adjacent quoted headers split, forced stays literal, template
    // refuses, UTF-8/CRLF offsets hold, and list writes equal ordinary
    // batch writes including missing-section fallback.
    let temp = TempDir::new("bob-cli-capture-list-quoted");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        concat!(
            "# Tasks\n",
            "## Ideas Extra\n",
            "Existing extra\n",
            "## Ideas\n",
            "Existing\n",
            "## Release Ideas\n",
            "Existing release\n",
            "## Caf\u{00e9}\n",
            "Existing cafe\n",
        ),
    );
    let day = day_file(&vault);

    // One-word quoted decodes and reports the list contract.
    let parse = run_parse("@dev#\"Ideas\"\n- Big idea");
    assert_eq!(parse["mode"], "bullet", "{parse}");
    assert_eq!(parse["section"], "Ideas", "{parse}");
    assert_eq!(parse["list"]["section"], "Ideas", "{parse}");
    assert_eq!(
        parse["header_range"],
        serde_json::json!({"start": 0, "end": 12}),
        "{parse}"
    );
    let items = parse["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["section"], "Ideas");
    assert_eq!(items[0]["list"]["section"], "Ideas");

    // Multi-word quoted spans the literal including spaces.
    let multi = run_parse("@dev#\"Release Ideas\"\n- Big idea");
    assert_eq!(multi["section"], "Release Ideas", "{multi}");
    assert_eq!(multi["list"]["section"], "Release Ideas", "{multi}");
    assert_eq!(
        multi["header_range"],
        serde_json::json!({"start": 0, "end": 20}),
        "{multi}"
    );
    let spans = multi["spans"].as_array().expect("spans");
    assert!(
        spans
            .iter()
            .any(|s| s["kind"] == "route" && s["start"] == 0 && s["end"] == 4),
        "{spans:?}"
    );
    assert!(
        spans.iter().any(|s| s["kind"] == "section"
            && s["start"] == 5
            && s["end"] == 20),
        "{spans:?}"
    );

    // Exact versus prefix: unquoted `#Ideas` prefix-matches the first
    // `Ideas*` heading (`Ideas Extra`), while quoted `#"Ideas"` matches
    // exactly `Ideas`. Both list and ordinary agree.
    let list_extra = dry_json(&vault, &day, "@dev#Ideas\n- Prefix task");
    let list_exact = dry_json(&vault, &day, "@dev#\"Ideas\"\n- Exact task");
    // Dry-run does not reveal placement, so compare real writes.
    let temp_prefix = TempDir::new("bob-cli-capture-list-quoted-prefix");
    let prefix_vault = temp_prefix.path().join("vault");
    fs::create_dir_all(&prefix_vault).expect("create vault");
    write_file(
        &prefix_vault.join("dev.md"),
        "# Tasks\n## Ideas Extra\nX\n## Ideas\nY\n",
    );
    let prefix_day = day_file(&prefix_vault);
    capture_real(&prefix_vault, &prefix_day, "@dev#Ideas\n- Prefix task");
    let prefix_contents =
        fs::read_to_string(prefix_vault.join("dev.md")).expect("read");
    let extra_pos = prefix_contents
        .find("## Ideas Extra")
        .expect("extra heading");
    let ideas_pos = prefix_contents.find("## Ideas\n").expect("ideas heading");
    let task_pos = prefix_contents.find("Prefix task").expect("task");
    assert!(extra_pos < task_pos && task_pos < ideas_pos, "unquoted prefix should land under first prefix match (Ideas Extra):\n{prefix_contents}");

    let temp_exact = TempDir::new("bob-cli-capture-list-quoted-exact");
    let exact_vault = temp_exact.path().join("vault");
    fs::create_dir_all(&exact_vault).expect("create vault");
    write_file(
        &exact_vault.join("dev.md"),
        "# Tasks\n## Ideas Extra\nX\n## Ideas\nY\n",
    );
    let exact_day = day_file(&exact_vault);
    capture_real(&exact_vault, &exact_day, "@dev#\"Ideas\"\n- Exact task");
    let exact_contents =
        fs::read_to_string(exact_vault.join("dev.md")).expect("read");
    let extra_pos = exact_contents
        .find("## Ideas Extra")
        .expect("extra heading");
    let ideas_pos = exact_contents.find("## Ideas\n").expect("ideas heading");
    let task_pos = exact_contents.find("Exact task").expect("task");
    assert!(
        ideas_pos < task_pos,
        "quoted exact should land under exact heading:\n{exact_contents}"
    );
    assert!(
        !(extra_pos < task_pos && task_pos < ideas_pos),
        "exact must not take prefix match:\n{exact_contents}"
    );

    // List writes equal the ordinary batch writes, including fallback.
    for (list_draft, batch_draft) in [
        (
            "@dev#\"Release Ideas\"\n- Big idea",
            "Big idea @dev#\"Release Ideas\"",
        ),
        ("@dev#\"Ideas\"\n- Big idea", "Big idea @dev#\"Ideas\""),
        ("@dev#\"Missing\"\n- Big idea", "Big idea @dev#\"Missing\""),
    ] {
        let list_temp = TempDir::new("bob-cli-capture-list-quoted-equiv-list");
        let list_v = list_temp.path().join("vault");
        fs::create_dir_all(&list_v).expect("create vault");
        write_file(
            &list_v.join("dev.md"),
            "# Tasks\n## Ideas\nY\n## Release Ideas\nZ\n",
        );
        let list_d = day_file(&list_v);
        capture_real(&list_v, &list_d, list_draft);

        let batch_temp =
            TempDir::new("bob-cli-capture-list-quoted-equiv-batch");
        let batch_v = batch_temp.path().join("vault");
        fs::create_dir_all(&batch_v).expect("create vault");
        write_file(
            &batch_v.join("dev.md"),
            "# Tasks\n## Ideas\nY\n## Release Ideas\nZ\n",
        );
        let batch_d = day_file(&batch_v);
        capture_real(&batch_v, &batch_d, batch_draft);

        assert_eq!(
            fs::read_to_string(list_v.join("dev.md")).expect("read list"),
            fs::read_to_string(batch_v.join("dev.md")).expect("read batch"),
            "list vs batch mismatch for {list_draft:?}"
        );
    }
    let _ = (list_extra, list_exact);

    // Two adjacent quoted headers split into two lists.
    let two_vault = temp.path().join("vault-two");
    fs::create_dir_all(&two_vault).expect("create vault");
    write_file(&two_vault.join("dev.md"), "# Tasks\n## Ideas\nY\n");
    write_file(&two_vault.join("cash.md"), "# Tasks\n");
    let two_day = day_file(&two_vault);
    let two_json = capture_real(
        &two_vault,
        &two_day,
        "@dev#\"Ideas\"\n- A task\n@cash\n- B task",
    );
    let caps = two_json["captures"].as_array().expect("captures");
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0]["list"]["section"], "Ideas");
    assert_eq!(caps[1]["route"], "cash");

    // UTF-8 section decodes and offsets land on char boundaries.
    let utf = run_parse("@dev#\"Caf\u{00e9}\"\n- Big idea");
    assert_eq!(utf["section"], "Caf\u{00e9}", "{utf}");
    assert_eq!(
        utf["header_range"],
        serde_json::json!({"start": 0, "end": 12}),
        "{utf}"
    );

    // CRLF line endings keep header ranges byte-exact.
    let crlf = run_parse("@dev#\"Release Ideas\"\r\n- Big idea");
    assert_eq!(crlf["list"]["section"], "Release Ideas", "{crlf}");
    assert_eq!(
        crlf["header_range"],
        serde_json::json!({"start": 0, "end": 20}),
        "{crlf}"
    );
    let crlf_items = crlf["items"].as_array().expect("items");
    assert_eq!(crlf_items[0]["range"]["start"], 22, "{crlf}");

    // Forced destinations keep `@` tokens literal (no list).
    let mut forced = bob_command();
    forced
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--route")
        .arg("dev")
        .arg("--")
        .arg("@dev#\"Ideas\"\n- Big idea")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0");
    let forced_out = forced.output().expect("run forced capture");
    assert_success(&forced_out);
    let forced_json: Value =
        serde_json::from_str(stdout(&forced_out).trim()).expect("forced JSON");
    assert!(
        forced_json.get("list").is_none() || forced_json["list"].is_null(),
        "forced must not split lists: {forced_json}"
    );
    assert_eq!(forced_json["text"], "@dev#\"Ideas\"", "{forced_json}");

    // Template bodies refuse quoted headers with the same code.
    let (_temp, tvault, tday, tbefore) =
        refusal_vault("bob-cli-capture-list-quoted-template");
    assert_refusal(
        &tvault,
        &tday,
        &tbefore,
        "@dev#\"Ideas\" %t\n- detail {0}\n\nFoo",
        "template_list",
    );
}

#[test]
fn capture_list_header_starts_item_without_blank_line() {
    // A column-zero header line starts a new list even with no blank line
    // before it: two headers file into two notes with global `i/N`.
    let temp = TempDir::new("bob-cli-capture-list-header-starts-item");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Tasks\n");
    write_file(&vault.join("cash.md"), "# Tasks\n");
    let day = day_file(&vault);

    let json = capture_real(&vault, &day, "@dev\n- A task\n@cash\n- B task");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["route"], "dev");
    assert_eq!(captures[0]["list"]["position"], 1);
    assert_eq!(captures[1]["route"], "cash");
    assert_eq!(captures[1]["list"]["position"], 1);
    assert_eq!(captures[1]["list"]["header_line"], 3);
    assert!(fs::read_to_string(vault.join("dev.md"))
        .expect("read dev")
        .contains("- [ ] #task A task [created::2026-11-02]\n"));
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read cash")
        .contains("- [ ] #task B task [created::2026-11-02]\n"));
}

#[test]
fn capture_list_later_item_failure_rolls_everything_back() {
    // Capture is atomic: a later bullet's refusal writes nothing, and a
    // later item's refusal rolls back the whole batch.
    let temp = TempDir::new("bob-cli-capture-list-atomicity");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Tasks\n");
    let day = day_file(&vault);
    let before = fs::read_to_string(vault.join("dev.md")).expect("read dev");

    let bad_bullet =
        run_capture(&vault, &day, "@dev\n- Good task\n- Bad @cash", false);
    assert_eq!(bad_bullet.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before
    );

    // A duplicate block ID across items fails the batch and writes
    // nothing: the first item's staged write rolls back too.
    let bad_item = run_capture(
        &vault,
        &day,
        "@dev\n- Good task\n\nSecond @dev^x1\n\nThird @dev^x1",
        false,
    );
    assert!(!bad_item.status.success());
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before,
        "a later item's failure rolls the list back"
    );

    // A plan-time failure in a later item (duplicate block ID against the
    // staged first item) rolls the staged write back too.
    let staged = run_capture(
        &vault,
        &day,
        "@dev\n- First ^staged-id\n\nSecond @dev^staged-id",
        false,
    );
    assert!(!staged.status.success());
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before,
        "a staged duplicate rolls back"
    );
}

#[test]
fn capture_list_project_note_parity_vectors() {
    // Under `project_note_parity`, project-note task bullets share the list
    // ID grammar: the ID token is recognized anywhere in the bullet's
    // trailing marker run (a mid-run `s:2` configures the task instead of
    // staying literal), and a lone ` :` on a nested line is an unfinished
    // ID for the owner.
    let input =
        "Finish it @cash^goog-exit@#admin\n- Draft the memo s:2 :draft-memo";
    let parse = run_parse(input);
    assert_eq!(parse["mode"], "pomodoro_project_note");
    assert_eq!(
        parse["sub_bullets"],
        serde_json::json!(["Draft the memo"]),
        "markers leave the body"
    );
    assert_eq!(
        parse["sub_bullet_task_ids"],
        serde_json::json!([{ "block_id": "draft-memo", "link": true }])
    );
    let kinds: Vec<&str> = parse["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|span| span["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"schedule"), "{parse}");
    assert!(kinds.contains(&"project_task_link_marker"), "{parse}");
    assert!(kinds.contains(&"project_task_block_id"), "{parse}");
    let memo = input.find(":draft-memo").unwrap();
    assert!(
        parse["spans"].as_array().unwrap().iter().any(|span| {
            span["start"] == memo
                && span["end"] == memo + 1
                && span["kind"] == "project_task_link_marker"
        }),
        "{parse}"
    );

    let nested =
        "Finish it @cash^goog-exit@#admin\n- Draft the memo\n  - detail :";
    let nested_parse = run_parse(nested);
    assert_eq!(nested_parse["mode"], "incomplete", "{nested}");
    assert_eq!(
        nested_parse["needs"],
        serde_json::json!(["block_id"]),
        "{nested}"
    );
    assert!(
        nested_parse["diagnostics"].as_array().unwrap().is_empty(),
        "{nested}"
    );

    let temp = TempDir::new("bob-cli-capture-list-parity-exec");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    let output = run_capture(&vault, &day, nested, false);
    assert_eq!(output.status.code(), Some(2), "{nested}");
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["code"], "invalid_project_task_id", "{nested}: {json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("needs a task ID"),
        "the nested refusal teaches the fix: {}",
        json["error"]
    );
}

#[test]
fn capture_list_unchanged_forms_keep_old_behavior() {
    // `@route+id` + bullets is still the multi-note form, `@@route` is
    // still a draft-wide declaration, and body text beside the marker is
    // still one task with children. None of them grows a `list` object.
    let plus = run_parse("@dev+proj\n- A task");
    assert_eq!(plus["mode"], "sub_bullet");
    assert_eq!(plus["route"], "dev");
    assert!(plus.get("list").is_none() || plus["list"].is_null());

    let global = run_parse("@@dev\n- A task");
    assert_eq!(global["route"], "dev");
    assert!(global["global_destination"]["route"] == "dev");
    assert!(global.get("list").is_none() || global["list"].is_null());

    let temp = TempDir::new("bob-cli-capture-list-unchanged");
    let vault = vault_with_dev(&temp);
    let day = day_file(&vault);
    let body = run_parse("Plan trip @dev\n- pack bags");
    assert_eq!(body["mode"], "task");
    assert_eq!(body["body"], "Plan trip");
    assert!(body.get("list").is_none() || body["list"].is_null());
    let json = capture_real(&vault, &day, "Plan trip @dev\n- pack bags");
    assert_eq!(json["text"], "Plan trip");
    assert!(fs::read_to_string(vault.join("dev.md"))
        .expect("read dev")
        .contains(
            "- [ ] #task Plan trip [created::2026-11-02]\n\t- pack bags\n"
        ));
}
