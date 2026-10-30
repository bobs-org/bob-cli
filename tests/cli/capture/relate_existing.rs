//! RX execution vectors for relating existing `@route+id` tasks
//! (`plan:202610/capture_relate_existing_tasks.md` §2.7).

use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-14 10:20:00";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_ready_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Unrelated placeholder\n");
    (temp, vault, day)
}

fn write_ready_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
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

fn seed_standard(vault: &Path) {
    write_file(
        vault.join("foo.md").as_path(),
        "- [ ] #task Buy shoes ^bar\n",
    );
    write_file(
        vault.join("body.md").as_path(),
        "- [ ] #task Exercise ^excercise\n",
    );
    write_file(
        vault.join("job.md").as_path(),
        "- [*] #task Do cleanup tasks ^cleanup\n",
    );
}

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-parse json");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn parse_stdin(draft: &str) -> serde_json::Value {
    let mut command = bob_command();
    command.arg("capture-parse").arg("-f").arg("json");
    let output = run_with_stdin(&mut command, draft);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn capture_json(vault: &Path, day: &Path, draft: &str) -> serde_json::Value {
    capture_json_dry_run_matches_real(vault, day, NOW, &[draft])
}

fn capture_human(
    vault: &Path,
    day: &Path,
    draft: &str,
) -> std::process::Output {
    capture_human_flags(vault, day, &[], draft)
}

fn capture_human_flags(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault);
    command.args(flags);
    command.arg("--").arg(draft);
    command.output().expect("run bob capture human")
}

fn capture_dry_human(vault: &Path, day: &Path, draft: &str) -> String {
    let output = capture_human_flags(vault, day, &["--dry-run"], draft);
    assert_success(&output);
    stdout(&output)
}

fn capture_fail(vault: &Path, day: &Path, draft: &str) -> std::process::Output {
    capture_fail_flags(vault, day, &[], draft)
}

fn capture_fail_flags(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault);
    command.args(flags);
    command.arg("--").arg(draft);
    command.output().expect("run bob capture")
}

fn capture_stdin(
    vault: &Path,
    day: &Path,
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault);
    run_with_stdin(&mut command, draft)
}

fn read(vault: &Path, relative: &str) -> String {
    fs::read_to_string(vault.join(relative)).expect("read note")
}

fn task_line(note: &str, block_id: &str) -> String {
    note.lines()
        .find(|line| line.contains(&format!("^{block_id}")))
        .unwrap_or(note)
        .to_string()
}

fn assert_existing_owner(
    json: &serde_json::Value,
    route: &str,
    block_id: &str,
) {
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": route,
            "block_id": block_id,
        }),
        "{json}"
    );
}

fn assert_exercise_depends_on_shoes(vault: &Path) {
    let body = read(vault, "body.md");
    assert!(
        body.contains("[dependsOn:: foo__bar]"),
        "Exercise depends on Buy shoes:\n{body}"
    );
    assert!(
        body.contains("⛓️ **DEPENDS ON:** [[foo#^bar]]"),
        "managed Depends-On line:\n{body}"
    );
    assert!(
        !body.contains("\n- \n") && !body.lines().any(|line| line == "- "),
        "no empty bullet:\n{body}"
    );
}

fn assert_exercise_blocked(vault: &Path) {
    let body = read(vault, "body.md");
    assert!(
        task_line(&body, "excercise").contains("- [?] #task Exercise"),
        "Exercise becomes Blocked:\n{body}"
    );
    assert_exercise_depends_on_shoes(vault);
}

fn assert_cleanup_depends_on_exercise(vault: &Path) {
    let job = read(vault, "job.md");
    assert!(
        task_line(&job, "cleanup").contains("- [?] "),
        "cleanup becomes Blocked:\n{job}"
    );
    assert!(
        job.contains("[[body#^excercise]]"),
        "cleanup depends on Exercise:\n{job}"
    );
}

// RX1
#[test]
fn rx1_existing_owner_ampersand_either_order() {
    for draft in ["&foo:bar @body+excercise", "@body+excercise &foo:bar"] {
        let parsed = parse_json(draft);
        assert_eq!(parsed["mode"], "task_dependency", "{draft}");
        assert_eq!(parsed["body"], "", "{draft}");
        assert_existing_owner(&parsed, "body", "excercise");
        assert_eq!(parsed["dependencies"][0]["raw"], "&foo:bar", "{draft}");

        let (_temp, vault, day) = setup("bob-cli-rx1");
        seed_standard(&vault);
        let out = capture_dry_human(&vault, &day, draft);
        assert!(out.contains("would update  body.md"), "{draft}: {out}");
        assert!(out.contains("[ ] → [?]  Exercise"), "{draft}: {out}");
        assert!(
            out.contains("would now depend on \"Buy shoes\""),
            "{draft}: {out}"
        );

        let json = capture_json(&vault, &day, draft);
        assert_eq!(json["kind"], "task_dependency", "{draft}: {json}");
        assert_eq!(json["block_id"], "excercise", "{draft}");
        assert_eq!(json["text"], "Exercise", "{draft}");
        assert_exercise_blocked(&vault);
    }
}

// RX2
#[test]
fn rx2_existing_owner_star() {
    let draft = "@body+excercise *job:cleanup";
    let parsed = parse_json(draft);
    assert_eq!(parsed["mode"], "task_dependency");
    assert_existing_owner(&parsed, "body", "excercise");
    assert_eq!(parsed["blocked_tasks"][0]["raw"], "*job:cleanup");

    let (_temp, vault, day) = setup("bob-cli-rx2");
    seed_standard(&vault);
    let out = capture_dry_human(&vault, &day, draft);
    assert!(out.contains("would update  body.md"), "{out}");
    assert!(out.contains("[ ] → [*]  Exercise"), "{out}");
    assert!(
        out.contains("would now block \"Do cleanup tasks\""),
        "{out}"
    );

    let json = capture_json(&vault, &day, draft);
    assert_eq!(json["kind"], "task_dependency", "{json}");
    assert_cleanup_depends_on_exercise(&vault);
    let body = read(&vault, "body.md");
    assert!(
        task_line(&body, "excercise").contains("- [*] "),
        "Exercise takes Next from cleanup:\n{body}"
    );
}

// RX3
#[test]
fn rx3_existing_owner_both_directions() {
    let draft = "@body+excercise &foo:bar *job:cleanup";
    let parsed = parse_json(draft);
    assert_eq!(parsed["mode"], "task_dependency");
    assert_eq!(parsed["dependencies"][0]["raw"], "&foo:bar");
    assert_eq!(parsed["blocked_tasks"][0]["raw"], "*job:cleanup");

    let (_temp, vault, day) = setup("bob-cli-rx3");
    seed_standard(&vault);
    let out = capture_dry_human(&vault, &day, draft);
    assert!(out.contains("would update  body.md"), "{out}");
    assert!(out.contains("would now depend on \"Buy shoes\""), "{out}");
    assert!(
        out.contains("would now block \"Do cleanup tasks\""),
        "{out}"
    );

    let json = capture_json(&vault, &day, draft);
    assert_eq!(json["kind"], "task_dependency", "{json}");
    assert_exercise_depends_on_shoes(&vault);
    assert_cleanup_depends_on_exercise(&vault);
}

// RX4
#[test]
fn rx4_caret_alias_executes_like_plus_owner() {
    for (draft, star) in [
        ("^body:excercise &foo:bar", false),
        ("&foo:bar ^body:excercise", false),
        ("^body:excercise *job:cleanup", true),
    ] {
        let parsed = parse_json(draft);
        assert_eq!(parsed["mode"], "task_dependency", "{draft}");
        assert_existing_owner(&parsed, "body", "excercise");

        let (_temp, vault, day) = setup("bob-cli-rx4");
        seed_standard(&vault);
        let json = capture_json(&vault, &day, draft);
        assert_eq!(json["kind"], "task_dependency", "{draft}: {json}");
        if star {
            assert_cleanup_depends_on_exercise(&vault);
        } else {
            assert_exercise_blocked(&vault);
        }
    }
}

// RX5
#[test]
fn rx5_colon_alias_still_owns() {
    let draft = "@body:excercise &foo:bar";
    let parsed = parse_json(draft);
    assert_eq!(parsed["mode"], "task_dependency");
    assert_existing_owner(&parsed, "body", "excercise");

    let (_temp, vault, day) = setup("bob-cli-rx5");
    seed_standard(&vault);
    let json = capture_json(&vault, &day, draft);
    assert_eq!(json["kind"], "task_dependency", "{json}");
    assert_exercise_blocked(&vault);
}

// RX6
#[test]
fn rx6_inherited_owner_writes_edge_only() {
    for draft in [
        "@@body+excercise\n&foo:bar\n",
        "@@body+excercise\n\n&foo:bar\n",
    ] {
        let parsed = parse_stdin(draft);
        assert_eq!(parsed["mode"], "task_dependency", "{draft:?}");
        assert_eq!(parsed["dependency_target"]["inherited"], true, "{draft:?}");
        assert_eq!(parsed["body"], "", "{draft:?}");

        let (_temp, vault, day) = setup("bob-cli-rx6");
        seed_standard(&vault);
        let output = capture_stdin(&vault, &day, draft);
        assert_success(&output);
        assert_exercise_blocked(&vault);
        let out = stdout(&output);
        assert!(out.contains("updated  body.md"), "{draft:?}: {out}");
        assert!(
            out.contains("now depends on \"Buy shoes\""),
            "{draft:?}: {out}"
        );
    }
}

// RX7
#[test]
fn rx7_closed_star_owner_is_refused() {
    let message_done = "task ^old in foo.md is Done; a finished task cannot block another task (reopen it first)";
    let message_canceled = "task ^old in foo.md is Canceled; a finished task cannot block another task (reopen it first)";
    for (status, message) in [('x', message_done), ('-', message_canceled)] {
        for draft in ["@foo+old *job:cleanup", "Note @foo+old *job:cleanup"] {
            let (_temp, vault, day) = setup("bob-cli-rx7");
            write_file(
                &vault.join("foo.md"),
                &format!("- [{status}] #task Old work ^old\n"),
            );
            write_file(
                &vault.join("job.md"),
                "- [*] #task Do cleanup tasks ^cleanup\n",
            );
            let before_foo = read(&vault, "foo.md");
            let before_job = read(&vault, "job.md");
            let output = capture_fail(&vault, &day, draft);
            assert!(
                !output.status.success(),
                "draft {draft:?} status {status} must fail:\n{}",
                format_output(&output)
            );
            assert!(
                stderr(&output).contains(message),
                "draft {draft:?} status {status}:\n{}",
                format_output(&output)
            );
            assert_eq!(read(&vault, "foo.md"), before_foo, "{draft}");
            assert_eq!(read(&vault, "job.md"), before_job, "{draft}");
        }
    }

    let message_forced = "task ^old in old.md is Done; a finished task cannot block another task (reopen it first)";
    let (_temp, vault, day) = setup("bob-cli-rx7-forced");
    write_file(&vault.join("old.md"), "- [x] #task Old thing ^old\n");
    write_file(
        &vault.join("job.md"),
        "- [*] #task Do cleanup tasks ^cleanup\n",
    );
    let before_old = read(&vault, "old.md");
    let before_job = read(&vault, "job.md");
    let output = capture_fail_flags(
        &vault,
        &day,
        &["--route", "old", "--task", "old"],
        "Note *job:cleanup",
    );
    assert!(
        !output.status.success(),
        "forced closed * owner must fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(message_forced),
        "{}",
        format_output(&output)
    );
    assert_eq!(read(&vault, "old.md"), before_old);
    assert_eq!(read(&vault, "job.md"), before_job);
}

// RX8
#[test]
fn rx8_unresolved_plus_selector_never_creates_a_task() {
    let message_plus =
        "`+` needs a task selection; choose the task to relate before capture";
    let message_plus_ex =
        "`+ex` needs a task selection; choose the task to relate before capture";
    for (draft, message) in [
        ("+ &foo:bar", message_plus),
        ("+ex &foo:bar", message_plus_ex),
        ("&foo:bar +", message_plus),
        ("*job:cleanup +ex", message_plus_ex),
    ] {
        let parsed = parse_json(draft);
        assert_eq!(parsed["mode"], "incomplete", "{draft}");
        assert_eq!(
            parsed["needs"],
            serde_json::json!(["task_parent"]),
            "{draft}"
        );
        assert_eq!(parsed["diagnostics"], serde_json::json!([]), "{draft}");
        assert_eq!(parsed["body"], "", "{draft}");

        let (_temp, vault, day) = setup("bob-cli-rx8");
        seed_standard(&vault);
        let before_foo = read(&vault, "foo.md");
        let before_body = read(&vault, "body.md");
        let before_job = read(&vault, "job.md");
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "draft {draft:?} must fail:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(message),
            "draft {draft:?}:\n{}",
            format_output(&output)
        );
        assert_eq!(read(&vault, "foo.md"), before_foo, "{draft}");
        assert_eq!(read(&vault, "body.md"), before_body, "{draft}");
        assert_eq!(read(&vault, "job.md"), before_job, "{draft}");
        assert!(
            !vault.join("mac_inbox.md").exists(),
            "never create a +… inbox task for {draft:?}"
        );
    }
}

// RX9
#[test]
fn rx9_relationship_needs_text_in_parse_and_execute() {
    let cases = [
        (
            "@body+excercise &foo:bar $2",
            "`$2` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar $2`)",
        ),
        (
            "@body+excercise &foo:bar s:3",
            "`s:3` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar s:3`)",
        ),
        (
            "@body+excercise &foo:bar %",
            "`%` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar %`)",
        ),
        (
            "@body+excercise &foo:bar $2 s:3",
            "`$2` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar $2`)",
        ),
    ];
    for (draft, expected) in cases {
        let parsed = parse_json(draft);
        assert_eq!(
            parsed["diagnostics"][0]["code"], "relationship_needs_text",
            "{draft}"
        );
        let parse_message = parsed["diagnostics"][0]["message"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(parse_message, expected, "{draft}");
        let (_temp, vault, day) = setup("bob-cli-rx9");
        seed_standard(&vault);
        let before = read(&vault, "body.md");
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "{draft}: {}",
            format_output(&output)
        );
        let execute_stderr = stderr(&output);
        assert!(
            execute_stderr.contains(parse_message),
            "parse and execute must name the same token for {draft}:\n{}",
            format_output(&output)
        );
        assert_eq!(read(&vault, "body.md"), before, "{draft}");
    }
}

// RX10
#[test]
fn rx10_child_bullets_need_text() {
    let draft = "@body+excercise &foo:bar\n- note\n";
    let parsed = parse_stdin(draft);
    assert_eq!(parsed["diagnostics"][0]["code"], "relationship_needs_text");
    assert!(parsed["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("`child bullets` needs task text"));

    let (_temp, vault, day) = setup("bob-cli-rx10");
    seed_standard(&vault);
    let before = read(&vault, "body.md");
    let output = capture_stdin(&vault, &day, draft);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("`child bullets` needs task text"),
        "{}",
        format_output(&output)
    );
    assert_eq!(read(&vault, "body.md"), before);
}

// RX11
#[test]
fn rx11_cycle_path_starts_at_new_edge_source() {
    let draft = "@body+excercise &foo:bar *foo:bar";
    let (_temp, vault, day) = setup("bob-cli-rx11");
    seed_standard(&vault);
    let before_body = read(&vault, "body.md");
    let before_foo = read(&vault, "foo.md");
    let output = capture_fail(&vault, &day, draft);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        stderr(&output)
            .contains("foo.md^bar -> body.md^excercise -> foo.md^bar"),
        "{}",
        format_output(&output)
    );
    assert!(
        !stderr(&output).contains("foo.md^bar -> foo.md^bar"),
        "cycle must not duplicate the final hop:\n{}",
        format_output(&output)
    );
    assert_eq!(read(&vault, "body.md"), before_body);
    assert_eq!(read(&vault, "foo.md"), before_foo);
}

// RX12
#[test]
fn rx12_sub_bullet_prints_children_then_relationship() {
    let draft = "Waiting @body+excercise &foo:bar\n- detail\n";
    let (_temp, vault, day) = setup("bob-cli-rx12");
    seed_standard(&vault);
    let output = capture_stdin(&vault, &day, draft);
    assert_success(&output);
    let body = read(&vault, "body.md");
    assert!(body.contains("Waiting"), "{body}");
    assert!(body.contains("detail"), "{body}");
    assert_exercise_blocked(&vault);

    let out = stdout(&output);
    let waiting = out
        .find("Waiting")
        .unwrap_or_else(|| panic!("bullet in:\n{out}"));
    let detail = out
        .find("detail")
        .unwrap_or_else(|| panic!("child in:\n{out}"));
    let chain = out
        .find("now depends on")
        .unwrap_or_else(|| panic!("relationship in:\n{out}"));
    assert!(
        waiting < detail && detail < chain,
        "relationship rows follow the bullet and child:\n{out}"
    );
    assert!(
        out.contains("under [ ] → [?] Exercise")
            || out.contains("under [ ] → [?]  Exercise"),
        "under row shows the parent status change:\n{out}"
    );

    let star_draft = "Note @plain+plain *job:cleanup";
    let (_temp, vault, day) = setup("bob-cli-rx12-star");
    write_file(&vault.join("plain.md"), "- [ ] #task Plain ^plain\n");
    write_file(
        &vault.join("job.md"),
        "- [*] #task Do cleanup tasks ^cleanup\n",
    );
    let dry = capture_human_flags(&vault, &day, &["--dry-run"], star_draft);
    assert_success(&dry);
    let dry_out = stdout(&dry);
    assert!(
        dry_out.contains("under [ ] → [*] Plain")
            || dry_out.contains("under [ ] → [*]  Plain"),
        "dry-run under row shows the * parent promotion:\n{dry_out}"
    );
    let live = capture_human(&vault, &day, star_draft);
    assert_success(&live);
    let live_out = stdout(&live);
    assert!(
        live_out.contains("under [ ] → [*] Plain")
            || live_out.contains("under [ ] → [*]  Plain"),
        "live under row shows the * parent promotion:\n{live_out}"
    );
}

// RX13
#[test]
fn rx13_explicit_and_project_owners_keep_must_be_bare() {
    for draft in [
        "@body+excercise! *job:cleanup",
        "^body:excercise=x &foo:bar",
    ] {
        let parsed = parse_json(draft);
        assert!(
            parsed["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["message"]
                    .as_str()
                    .unwrap()
                    .contains("must be bare")),
            "{draft}: {parsed}"
        );
        let (_temp, vault, day) = setup("bob-cli-rx13");
        seed_standard(&vault);
        let before = read(&vault, "body.md");
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "{draft}: {}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("must be bare"),
            "{draft}: {}",
            format_output(&output)
        );
        assert_eq!(read(&vault, "body.md"), before, "{draft}");
    }
}

// RX14
#[test]
fn rx14_in_progress_owner_consumes_running_link() {
    let draft = "@body+excercise &foo:bar";
    let (_temp, vault, day) = setup("bob-cli-rx14");
    write_file(&vault.join("foo.md"), "- [ ] #task Buy shoes ^bar\n");
    write_file(&vault.join("body.md"), "- [/] #task Exercise ^excercise\n");
    write_file(
        &day,
        "## Pomodoros\n- [ ] (1010-1035) — BODY\n  - [[body#^excercise]]\n",
    );
    let out = capture_dry_human(&vault, &day, draft);
    assert!(out.contains("would update  body.md"), "{out}");
    assert!(out.contains("[/] → [?]  Exercise"), "{out}");
    assert!(
        out.contains(
            "would now depend on \"Buy shoes\"  foo.md ^bar  [ ] → [/] In Progress"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "would remove its Task Link from BODY 10:10–10:35 (running)"
        ),
        "{out}"
    );

    let json = capture_json(&vault, &day, draft);
    assert_eq!(json["kind"], "task_dependency", "{json}");
    let prereq = &json["dependency_update"]["prerequisites"][0];
    assert_eq!(prereq["status_symbol"], "/", "{json}");
    assert_eq!(prereq["status_name"], "In Progress", "{json}");
    assert_eq!(prereq["previous_status"], " ", "{json}");
    assert_eq!(prereq["previous_status_name"], "Ready", "{json}");
    assert!(prereq.get("already_present").is_none(), "{json}");
    assert!(
        !json["consumed_links"].as_array().unwrap().is_empty(),
        "{json}"
    );
    let body = read(&vault, "body.md");
    assert!(
        task_line(&body, "excercise").contains("- [?] #task Exercise"),
        "{body}"
    );
    let day_text = fs::read_to_string(&day).expect("day");
    assert!(
        !day_text.contains("[[body#^excercise]]"),
        "consumed the running link:\n{day_text}"
    );
}

// RX15
#[test]
fn rx15_repeat_is_already_present_noop() {
    let (_temp, vault, day) = setup("bob-cli-rx15");
    seed_standard(&vault);
    assert_success(&capture_human(&vault, &day, "&foo:bar @body+excercise"));
    let before = read(&vault, "body.md");
    let repeat = capture_human(&vault, &day, "&foo:bar @body+excercise");
    assert_success(&repeat);
    assert_eq!(read(&vault, "body.md"), before);
    let out = stdout(&repeat);
    assert!(out.contains("already depends on \"Buy shoes\""), "{out}");
    assert!(
        !out.contains('→'),
        "already-present has no status arrow:\n{out}"
    );
    let repeat_json = capture_json(&vault, &day, "&foo:bar @body+excercise");
    let shoes = &repeat_json["dependency_update"]["prerequisites"][0];
    assert_eq!(shoes["already_present"], true, "{repeat_json}");
    assert!(shoes.get("previous_status").is_none(), "{repeat_json}");

    write_file(
        &vault.join("foo.md"),
        "- [ ] #task Buy shoes ^bar\n- [ ] #task Buy socks ^socks\n",
    );
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");
    assert_success(&capture_human(&vault, &day, "@body+excercise &foo:bar"));
    let mixed_draft = "@body+excercise &foo:bar &foo:socks";
    let mixed_json_output = capture_human_flags(
        &vault,
        &day,
        &["-f", "json", "--dry-run"],
        mixed_draft,
    );
    assert_success(&mixed_json_output);
    let mixed_json: serde_json::Value =
        serde_json::from_str(stdout(&mixed_json_output).trim())
            .expect("mixed dry-run JSON");
    let mixed_prereqs = mixed_json["dependency_update"]["prerequisites"]
        .as_array()
        .expect("prerequisites");
    let by_id = |id: &str| {
        mixed_prereqs
            .iter()
            .find(|row| row["block_id"] == id)
            .unwrap_or_else(|| panic!("missing {id} in {mixed_json}"))
    };
    assert_eq!(by_id("bar")["already_present"], true, "{mixed_json}");
    assert!(
        by_id("socks").get("already_present").is_none(),
        "{mixed_json}"
    );
    let mixed = capture_human(&vault, &day, mixed_draft);
    assert_success(&mixed);
    let mixed_out = stdout(&mixed);
    assert!(
        mixed_out.contains("already depends on \"Buy shoes\""),
        "{mixed_out}"
    );
    assert!(
        mixed_out.contains("now depends on \"Buy socks\""),
        "{mixed_out}"
    );

    write_file(
        &vault.join("job.md"),
        "- [*] #task Do cleanup tasks ^cleanup\n",
    );
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");
    assert_success(&capture_human(
        &vault,
        &day,
        "@body+excercise *job:cleanup",
    ));
    let before_job = read(&vault, "job.md");
    let star = capture_human(&vault, &day, "@body+excercise *job:cleanup");
    assert_success(&star);
    assert_eq!(read(&vault, "job.md"), before_job);
    assert!(
        stdout(&star).contains("already blocks \"Do cleanup tasks\""),
        "{}",
        stdout(&star)
    );
}

// RX16
#[test]
fn rx16_new_task_prints_per_prerequisite_row() {
    let draft = "Buy milk &foo:bar";
    let parsed = parse_json(draft);
    assert_eq!(parsed["mode"], "task");
    assert_eq!(
        parsed["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );

    let (_temp, vault, day) = setup("bob-cli-rx16");
    seed_standard(&vault);
    let output = capture_human(&vault, &day, draft);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("⛓ depends on \"Buy shoes\"  foo.md ^bar  [ ] Ready"),
        "{out}"
    );
    assert!(
        !out.contains('→'),
        "new-task prerequisite with no transfer has no status arrow:\n{out}"
    );
    let inbox = read(&vault, "mac_inbox.md");
    assert!(inbox.contains("Buy milk"), "{inbox}");
    assert!(inbox.contains("[[foo#^bar]]"), "{inbox}");
}
