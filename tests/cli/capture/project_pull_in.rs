//! Capture-side project pull-in vectors (PP1, PP9–PP12, PP14, PP16,
//! PP21, PP23, PP24, PP26, PP27; `docs/task-dependencies.md` §12.11).

use crate::support::*;
use std::{fs, path::Path};

const NOW: &str = "2026-10-05 09:30:00";

fn make_vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

fn project(contents: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\n---\n{contents}")
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    args: &[&str],
    dry_run: bool,
) -> serde_json::Value {
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
    let output = command
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture JSON");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_human(
    vault: &Path,
    day_file: &Path,
    args: &[&str],
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command.output().expect("run bob capture human")
}

// PP1 + PP14 + PP23: an id-less task in a staged project note triggers a
// pull-in without building dependents, and sync agrees with the write.
#[test]
fn idless_last_task_close_pulls_in_and_sync_agrees() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp1");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship the last outcome ^a\n\
             - [ ] #task Ship the project #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");

    let json =
        capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["!proj:a"]);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(json["task_complete"]["unblocked_check"], "checked");
    assert_eq!(row["block_id"], "prj");
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["hide_removed"], true);
    assert_eq!(row["previous_status_symbol"], " ");
    assert_eq!(row["status_symbol"], "*");
    assert_eq!(row["link"]["block_link"], "[[proj#^prj]]");
    assert_eq!(row["unblocked_by"][0]["block_id"], "a");
    let note = fs::read_to_string(vault.join("proj.md")).expect("read project");
    assert!(
        note.contains("- [*] #task Ship the project #prj ^prj"),
        "{note}"
    );

    let sync = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run project sync dry-run");
    assert_success(&sync);
    let out = stdout(&sync);
    assert!(
        !out.contains("proj.md"),
        "sync should leave the pull-in alone:\n{out}"
    );
}

// PP9: a live R link is not duplicated, but the project still becomes Next
// and loses #hide.
#[test]
fn already_planned_pull_in_raises_project_without_duplicate_link() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp9");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship A [id:: a] ^a\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n  - [[proj#^prj]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!proj:a"], false);
    let rows = json["task_complete"]["unblocked"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["trigger"], "project_empty");
    assert_eq!(rows[0]["not_linked"], "already_planned");
    assert!(rows[0]["link"].is_null());
    assert_eq!(rows[0]["hide_removed"], true);
    assert_eq!(rows[0]["status_symbol"], "*");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(day.matches("[[proj#^prj]]").count(), 1, "{day}");
}

// PP10: without today's anchor, a hidden Ready project is surfaced and the
// human dry-run says "would surface".
#[test]
fn unplanned_pull_in_is_reported_as_surface_in_human_output() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp10");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship A [id:: a] ^a\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n");

    let before_note =
        fs::read(vault.join("proj.md")).expect("read project bytes");
    let before_day = fs::read(&day_file).expect("read day bytes");
    let dry = capture_json(&vault, &day_file, &["!proj:a"], true);
    let row = &dry["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["not_linked"], "not_planned_today");
    assert_eq!(row["hide_removed"], true);
    assert_eq!(row["previous_status_symbol"], row["status_symbol"]);
    assert!(row["link"].is_null());

    let output = capture_human(&vault, &day_file, &["!proj:a"], true);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would surface Ship project  proj.md ^prj"),
        "{out}"
    );
    assert!(out.contains("· project has no open tasks"), "{out}");
    assert!(!out.contains("· not planned today"), "{out}");
    assert_eq!(fs::read(vault.join("proj.md")).unwrap(), before_note);
    assert_eq!(fs::read(&day_file).unwrap(), before_day);
}

// PP11 and PP12: disabled linking still surfaces the task; In Progress is
// preserved when a project pull-in is linked.
#[test]
fn disabled_pull_in_surfaces_and_in_progress_pull_in_stays_in_progress() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp11");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship A [id:: a] ^a\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");
    write_file(
        &vault.join("bob-config.yml"),
        "plan:\n  link_unblocked: false\n",
    );

    let config = vault.join("bob-config.yml");
    let json = {
        let mut command = bob_command();
        let output = command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("!proj:a")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("run capture with disabled linking");
        assert_success(&output);
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .expect("capture JSON")
    };
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "disabled");
    assert_eq!(row["hide_removed"], true);
    assert_eq!(row["status_symbol"], " ");

    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp12");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship A [id:: a] ^a\n\
             - [/] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");

    let json = capture_json(&vault, &day_file, &["!proj:a"], false);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["status_symbol"], "/");
    assert_eq!(row["trigger"], "project_empty");
}

// PP8 / reporting: project-empty still-blocked rows retain the complete line
// and human output puts the empty-project context before the reason.
#[test]
fn still_blocked_project_row_has_trigger_and_context_before_reason() {
    let (_temp, vault, day_file) =
        make_vault("bob-cli-project-pull-in-still-blocked");
    write_file(
        &vault.join("prereq.md"),
        "- [ ] #task A [id:: a] ^a\n- [ ] #task Other [id:: other] ^other\n",
    );
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [?] #task Ship project #prj #hide [dependsOn:: a, other] [id:: prj] ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — FIX\n  - [[prereq#^a]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!prereq:a"], false);
    let row = &json["task_complete"]["still_blocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["reason"], "waits_on");
    assert_eq!(row["waits_on"], 1);
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read project"),
        project(
            "- [?] #task Ship project #prj #hide [dependsOn:: a, other] [id:: prj] ^prj\n"
        )
    );
}

// PP16: closing an embedded project task joins the closing continuation.
#[test]
fn pomodoro_close_pulls_in_project_at_continuation() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp16");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship A ^a\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — BOB\n  - ![[proj#^a]]\n- [ ] () — FIX\n",
    );

    let json = capture_json(&vault, &day_file, &["=x"], false);
    let close = &json["pomodoro_close"];
    let row = &close["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["hide_removed"], true);
    assert_eq!(row["link"]["entry_name"], "BOB");
    assert_eq!(row["link"]["next_up"], true);
    assert_eq!(row["link"]["block_link"], "[[proj#^prj]]");
    assert_eq!(close["next_pomodoro"]["name"], "BOB");
}

// PP21: only the item that empties N reports R; a later close of R removes
// an earlier pull-in row in the net batch pass.
#[test]
fn multi_item_project_pull_ins_obey_surface_and_net_pass() {
    let (_temp, vault, day_file) =
        make_vault("bob-cli-project-pull-in-pp21-first");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task A [id:: a] ^a\n\
             - [ ] #task B [id:: b] ^b\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n  - [[proj#^b]]\n",
    );
    let json = capture_json(&vault, &day_file, &["!proj:a\n\n!proj:b"], false);
    assert_eq!(
        json["captures"][0]["task_complete"]["unblocked"],
        serde_json::json!([])
    );
    assert_eq!(
        json["captures"][1]["task_complete"]["unblocked"][0]["trigger"],
        "project_empty"
    );

    let (_temp, vault, day_file) =
        make_vault("bob-cli-project-pull-in-pp21-net");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task A [id:: a] ^a\n\
             - [ ] #task Ship project #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");
    let json =
        capture_json(&vault, &day_file, &["!proj:a\n\n!proj:prj"], false);
    assert_eq!(
        json["captures"][0]["task_complete"]["unblocked"],
        serde_json::json!([])
    );
}

// PP24: the last blocker can pull in R from another note, as long as N is
// empty. PP27: a hidden Blocked R that recovers Ready gets a tickler stamp
// after #hide is removed when it is not linked.
#[test]
fn external_blocker_pull_in_and_unplanned_ready_recovery() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp24");
    write_file(&vault.join("prereq.md"), "- [ ] #task P [id:: p] ^p\n");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [?] #task Ship project #prj #hide [dependsOn:: p] [id:: prj] ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — FIX\n  - [[prereq#^p]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!prereq:p"], false);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["link"]["block_link"], "[[proj#^prj]]");
    assert_eq!(row["status_symbol"], "*");

    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp27");
    write_file(&vault.join("prereq.md"), "- [ ] #task P [id:: p] ^p\n");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [?] #task Ship project #prj #hide [fresh:: 2026-10-01] [dependsOn:: p] [id:: prj] ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n");

    let json = capture_json(&vault, &day_file, &["!prereq:p"], false);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["not_linked"], "not_planned_today");
    assert_eq!(row["status_symbol"], " ");
    assert_eq!(row["hide_removed"], true);
    assert!(fs::read_to_string(vault.join("proj.md"))
        .expect("read project")
        .contains("[unblocked:: 2026-10-05]"));
}

// PP26: a due project schedule recounts an ordinary hidden open task as
// remaining work; without that schedule, the hidden task does not prevent
// the pull-in.
#[test]
fn due_project_schedule_applies_sync_hidden_task_recount() {
    let setup = |name: &str, scheduled: bool| {
        let (_temp, vault, day_file) = make_vault(name);
        let project_line = if scheduled {
            "- [ ] #task Ship project #prj #hide [scheduled:: 2026-10-05] ^prj\n"
        } else {
            "- [ ] #task Ship project #prj #hide ^prj\n"
        };
        write_file(
            &vault.join("proj.md"),
            &project(&format!(
                "- [ ] #task A [id:: a] ^a\n{project_line}- [ ] #task Hidden remaining #hide ^b\n"
            )),
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n",
        );
        (_temp, vault, day_file)
    };

    let (_temp, vault, day_file) =
        setup("bob-cli-project-pull-in-pp26-dated", true);
    let json = capture_json(&vault, &day_file, &["!proj:a"], false);
    assert_eq!(json["task_complete"]["unblocked"], serde_json::json!([]));

    let (_temp, vault, day_file) =
        setup("bob-cli-project-pull-in-pp26-open", false);
    let json = capture_json(&vault, &day_file, &["!proj:a"], false);
    assert_eq!(
        json["task_complete"]["unblocked"][0]["trigger"],
        "project_empty"
    );
}

// R1 (PP22/PP23): a live pull-in with punctuation-bounded `#hide` agrees
// with `bob projects sync --dry-run` (no event for N).
#[test]
fn punctuation_hide_pull_in_and_sync_agrees() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-r1");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task Ship the last outcome ^a\n\
             - [ ] #task Ship the project #prj #hide, ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");

    let json =
        capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["!proj:a"]);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["hide_removed"], true);
    assert_eq!(row["block_id"], "prj");
    assert!(row["link"].is_object());

    let sync = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run project sync dry-run");
    assert_success(&sync);
    let out = stdout(&sync);
    assert!(
        !out.contains("proj.md"),
        "sync should leave the pull-in alone:\n{out}"
    );
}

// R2: a `[?]` resident whose prerequisite is still open elsewhere stays
// `[?]` instead of recovering to `[ ]`.
#[test]
fn stale_blocked_resident_stays_blocked() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-r2");
    write_file(
        &vault.join("other.md"),
        "- [ ] #task Ext [id:: ext-id] ^ext\n",
    );
    write_file(
        &vault.join("proj.md"),
        &project(concat!(
            "- [ ] #task Fix pump ^a\n",
            "- [?] #task Ship it! #prj #hide ^prj\n",
            "- [?] #task Help #prj_is_blocked #hide [dependsOn:: ext-id] ^help\n",
        )),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");

    let _json = capture_json(&vault, &day_file, &["!proj:a"], false);
    let note = fs::read_to_string(vault.join("proj.md")).expect("read project");
    let resident = note
        .lines()
        .find(|line| line.contains("^help"))
        .expect("resident line");
    assert!(resident.starts_with("- [?] "), "{resident}");
    assert!(resident.contains("#hide"), "{resident}");
    assert!(!resident.contains("[unblocked::"), "{resident}");
}

fn child_project(parent: &str, contents: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\nparent: \"[[{parent}]]\"\n---\n{contents}")
}

// PP29 + PP34 + PP36 + dry-run: closing K's `^prj` pulls in the parent
// N's R (`trigger: project_empty`, `unblocked_by: [K ^prj]`, row text
// drops the tags); `--dry-run` writes nothing; and `bob projects sync
// --dry-run` then only flips K's marker entry, never N's `#hide`.
#[test]
fn subproject_close_pulls_in_parent_and_sync_agrees() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp29");
    write_file(
        &vault.join("k.md"),
        &child_project("n-proj", "- [ ] #task Kid work ^prj\n"),
    );
    write_file(
        &vault.join("n-proj.md"),
        &project(
            "- [ ] #task Parent work #prj #hide ^prj\n  - 🧩 **Sub-projects:** [[k]]\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[k#^prj]]\n");

    let before_k = fs::read(vault.join("k.md")).expect("read k bytes");
    let before_n = fs::read(vault.join("n-proj.md")).expect("read n bytes");
    let before_day = fs::read(&day_file).expect("read day bytes");
    let dry = capture_json(&vault, &day_file, &["!k:prj"], true);
    let row = &dry["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert_eq!(row["block_id"], "prj");
    assert_eq!(row["note_path"], "n-proj.md");
    assert_eq!(row["previous_status_symbol"], " ");
    assert_eq!(row["status_symbol"], "*");
    assert_eq!(row["text"], "Parent work");
    assert_eq!(row["link"]["block_link"], "[[n-proj#^prj]]");
    assert_eq!(row["unblocked_by"][0]["note_path"], "k.md");
    assert_eq!(row["unblocked_by"][0]["block_id"], "prj");
    assert_eq!(fs::read(vault.join("k.md")).unwrap(), before_k);
    assert_eq!(fs::read(vault.join("n-proj.md")).unwrap(), before_n);
    assert_eq!(fs::read(&day_file).unwrap(), before_day);

    let human = capture_human(&vault, &day_file, &["!k:prj"], true);
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("Parent work"), "{out}");
    assert!(out.contains("· project has no open tasks"), "{out}");

    let json = capture_json(&vault, &day_file, &["!k:prj"], false);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["trigger"], "project_empty");
    assert!(row["link"].is_object());
    let k = fs::read_to_string(vault.join("k.md")).expect("read k");
    assert!(k.contains("- [x] #task Kid work"), "{k}");
    assert!(k.contains("^prj"), "{k}");
    let n = fs::read_to_string(vault.join("n-proj.md")).expect("read n");
    assert!(n.contains("- [*] #task Parent work #prj ^prj"), "{n}");
    assert!(!n.contains("#hide"), "{n}");

    let sync = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run project sync dry-run");
    assert_success(&sync);
    let out = stdout(&sync);
    assert!(
        out.contains("update [[k]] on ^prj"),
        "sync flips only K's marker entry:\n{out}"
    );
    assert!(
        !out.contains("#hide"),
        "sync never touches N's #hide:\n{out}"
    );
}

// PP35 (SL20): a draft that closes K's `^prj` and then N's pulled-in R
// drops the pull-in row in the net batch pass.
#[test]
fn parent_pull_in_row_drops_when_parent_prj_also_closes() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp35");
    write_file(
        &vault.join("k.md"),
        &child_project("n-proj", "- [ ] #task Kid work ^prj\n"),
    );
    write_file(
        &vault.join("n-proj.md"),
        &project(
            "- [ ] #task Parent work #prj #hide ^prj\n  - 🧩 **Sub-projects:** [[k]]\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[k#^prj]]\n");

    let json =
        capture_json(&vault, &day_file, &["!k:prj\n\n!n-proj:prj"], false);
    assert_eq!(
        json["captures"][0]["task_complete"]["unblocked"],
        serde_json::json!([]),
        "net pass drops the pulled-in row once R closes"
    );
    let n = fs::read_to_string(vault.join("n-proj.md")).expect("read n");
    assert!(n.contains("- [x] #task Parent work #prj"), "{n}");
    assert!(n.contains("^prj"), "{n}");
}

// PP32: closing a task in N when N's R is tagged (N inherits) leaves R
// `[?]` with `still_blocked{reason: project, project: parent(N)}` and
// the row text drops the tag.
#[test]
fn tagged_parent_prj_close_stays_blocked_naming_grandparent() {
    let (_temp, vault, day_file) = make_vault("bob-cli-project-pull-in-pp32");
    write_file(
        &vault.join("grand.md"),
        &project("- [ ] #task Grand chore [id:: chore] ^chore\n"),
    );
    write_file(
        &vault.join("proj.md"),
        &child_project(
            "grand",
            "- [ ] #task Fix pump [id:: fix-a] ^a-fix\n- [?] #task Ship it! #prj #hide #prj_is_blocked ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a-fix]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!proj:a-fix"], false);
    let complete = &json["task_complete"];
    assert_eq!(complete["unblocked"], serde_json::json!([]));
    let still = complete["still_blocked"].as_array().expect("still_blocked");
    assert_eq!(still.len(), 1);
    assert_eq!(still[0]["block_id"], "prj");
    assert_eq!(still[0]["reason"], "project");
    assert_eq!(still[0]["project"], "grand.md");
    assert_eq!(still[0]["trigger"], "project_empty");
    assert_eq!(still[0]["text"], "Ship it!");
    let note = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(
        note.contains("- [?] #task Ship it! #prj #hide #prj_is_blocked ^prj"),
        "R stays byte-identical:\n{note}"
    );
}
