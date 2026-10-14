//! Capture `*` execute vectors ST1–ST19 (`plan:202610/capture_star_blocks.md`).

use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-14 10:20:00";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Unrelated placeholder\n");
    (temp, vault, day)
}

fn write_project(vault: &Path, relative: &str, body: &str) {
    write_file(
        &vault.join(relative),
        &format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}"),
    );
}

fn capture_json(vault: &Path, day: &Path, draft: &str) -> serde_json::Value {
    capture_json_dry_run_matches_real(vault, day, NOW, &[draft])
}

fn capture_json_flags(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
) -> serde_json::Value {
    capture_json_dry_run_matches_real_flags(vault, day, NOW, flags, &[draft])
}

fn capture_fail(vault: &Path, day: &Path, draft: &str) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture")
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
        .arg("capture")
        .arg("-b")
        .arg(vault);
    command.args(flags);
    command.arg("--").arg(draft);
    command.output().expect("run bob capture human")
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
        .arg("capture")
        .arg("-b")
        .arg(vault);
    command.args(flags);
    command.arg("--").arg(draft);
    command.output().expect("run bob capture")
}

fn task_block_ids(json: &serde_json::Value) -> Vec<String> {
    json["task_blocks"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|row| row["block_id"].as_str().map(str::to_string))
        .collect()
}

fn running_day(extra_children: &str) -> String {
    format!(
        "## Pomodoros\n- [ ] (1010-1035) — JOB\n{extra_children}- [ ] Unrelated placeholder\n"
    )
}

fn task_line(note: &str, block_id: &str) -> String {
    note.lines()
        .find(|line| line.contains(&format!("^{block_id}")))
        .unwrap_or(note)
        .to_string()
}

// ST1 + ST18: new N * Ready unlinked T. Dry-run JSON matches live.
#[test]
fn st1_new_blocker_ready_unlinked() {
    let (_temp, vault, day) = setup("bob-cli-star-st1");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");

    let json = capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    assert_eq!(json["kind"], "task");
    assert_eq!(json["dependency_update"]["role"], "blocked_task");
    assert_eq!(
        json["dependency_update"], json["dependency_updates"][0],
        "{json}"
    );
    assert_eq!(json["dependency_updates"].as_array().unwrap().len(), 1);
    assert_eq!(json["dependency_update"]["previous_status"], " ");
    assert_eq!(json["dependency_update"]["dependent_status"], "?");
    let minted = json["minted_blocker_block_id"]["block_id"]
        .as_str()
        .expect("minted blocker id");
    assert!(!minted.is_empty(), "{json}");
    assert!(json.get("minted_dependent_block_id").is_none(), "{json}");
    assert_eq!(json["minted_blocker_block_id"]["path"], "mac_inbox.md");
    assert_eq!(json["minted_blocker_block_id"]["line"], 1);

    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    let task_line = json["task_line"].as_str().expect("task_line");
    assert_eq!(
        inbox,
        format!("{task_line}\n\t- 🧱 **BLOCKS:** [[job#^cleanup]]\n"),
        "N exact bytes:\n{inbox}"
    );
    assert!(
        task_line.starts_with("- [ ] #task Ask Kelly"),
        "Ready T does not raise new N past Ready:\n{task_line}"
    );
    assert_eq!(
        job,
        format!(
            "- [?] #task Cleanup [fresh:: 2026-10-14] [id:: job__cleanup] [dependsOn:: mac_inbox__{minted}] ^cleanup\n\t- ⛓️ **DEPENDS ON:** [[mac_inbox#^{minted}]]\n"
        ),
        "T exact bytes:\n{job}"
    );
    let day_after = fs::read_to_string(&day).expect("day");
    assert_eq!(
        day_after, "## Pomodoros\n- [ ] Unrelated placeholder\n",
        "unlinked T consumes nothing:\n{day_after}"
    );
}

// ST2: linked T with other children; session kept; N capped at Next.
#[test]
fn st2_consume_link_keep_session() {
    let (_temp, vault, day) = setup("bob-cli-star-st2");
    write_file(&vault.join("job.md"), "- [*] #task Cleanup ^cleanup\n");
    write_file(
        &day,
        &running_day("  - [[job#^cleanup]]\n  - other context\n"),
    );

    let json = capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    let minted = json["minted_blocker_block_id"]["block_id"]
        .as_str()
        .expect("minted");
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(
        inbox.contains("- [*] #task Ask Kelly"),
        "new N capped at Next:\n{inbox}"
    );
    assert!(
        !day_after.contains("[[job#^cleanup]]"),
        "T's link is consumed:\n{day_after}"
    );
    assert!(
        day_after.contains("other context")
            && day_after.contains("(1010-1035) — JOB"),
        "session kept:\n{day_after}"
    );
    assert_eq!(json["consumed_links"][0]["action"], "removed");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], false);
    assert_eq!(json["consumed_links"][0]["block_id"], "cleanup");
    let _ = minted;
}

// ST3: only child deletes the empty running entry; N capped at Next.
#[test]
fn st3_delete_empty_running_entry() {
    let (_temp, vault, day) = setup("bob-cli-star-st3");
    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&day, &running_day("  - [[job#^cleanup]]\n"));

    let json = capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(
        inbox.contains("- [*] #task Ask Kelly"),
        "In Progress T still caps new N at Next:\n{inbox}"
    );
    assert!(
        !inbox.contains("- [/] #task Ask Kelly"),
        "cap must not raise to In Progress:\n{inbox}"
    );
    assert_eq!(
        day_after, "## Pomodoros\n- [ ] Unrelated placeholder\n",
        "empty running entry deleted; placeholder kept:\n{day_after}"
    );
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], true);
    assert_eq!(json["consumed_links"][0]["pomodoro"]["status"], "running");
    assert_eq!(json["pomodoro_blocks"][0]["removed"], true);
}

// ST4: authored notes under the link become plain text (`notes_kept`).
#[test]
fn st4_notes_kept() {
    let (_temp, vault, day) = setup("bob-cli-star-st4");
    write_file(
        &vault.join("job.md"),
        "- [*] #task Cleanup with [[steps#^walk|the walk]] ^cleanup\n",
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (1010-1035) — JOB\n",
            "  - [[job#^cleanup]]\n",
            "    - authored work note\n",
            "- [ ] Unrelated placeholder\n",
        ),
    );

    let json = capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    let day_after = fs::read_to_string(&day).expect("day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (1010-1035) — JOB\n",
            "  - Cleanup with the walk\n",
            "    - authored work note\n",
            "- [ ] Unrelated placeholder\n",
        ),
        "{day_after}"
    );
    assert_eq!(json["consumed_links"][0]["action"], "notes_kept");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], false);
}

// ST5: nested T; basename ledger spelling consumed by identity.
#[test]
fn st5_nested_identity() {
    let (_temp, vault, day) = setup("bob-cli-star-st5");
    write_file(
        &vault.join("projects/house/kitchen.md"),
        "- [*] #task Cabinets ^cabinets\n",
    );
    write_file(&day, &running_day("  - [[kitchen#^cabinets]]\n"));

    let json = capture_json(
        &vault,
        &day,
        "Ask Kelly *projects/house/kitchen:cabinets",
    );
    let kitchen = fs::read_to_string(vault.join("projects/house/kitchen.md"))
        .expect("kitchen");
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(kitchen.contains("- [?] #task Cabinets"), "{kitchen}");
    assert!(
        !day_after.contains("[[kitchen#^cabinets]]"),
        "basename spelling consumed:\n{day_after}"
    );
    assert_eq!(json["consumed_links"][0]["block_id"], "cabinets");
}

// ST6: link N first, then consume T; session kept because N occupies the slot.
#[test]
fn st6_link_then_consume() {
    let (_temp, vault, day) = setup("bob-cli-star-st6");
    write_file(&vault.join("job.md"), "- [*] #task Cleanup ^cleanup\n");
    write_file(&day, &running_day("  - [[job#^cleanup]]\n"));

    let json = capture_json(
        &vault,
        &day,
        "Ask Kelly @mac_inbox:ask-kelly *job:cleanup",
    );
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(
        day_after.contains("[[mac_inbox#^ask-kelly]]"),
        "N linked before consume:\n{day_after}"
    );
    assert!(
        !day_after.contains("[[job#^cleanup]]"),
        "T consumed:\n{day_after}"
    );
    assert!(
        day_after.contains("(1010-1035) — JOB"),
        "session kept:\n{day_after}"
    );
    assert_eq!(json["block_id"], "ask-kelly");
    assert!(json.get("minted_blocker_block_id").is_none(), "{json}");
}

// ST7: linked T is consumed; unrelated childless placeholder stays.
#[test]
fn st7_unrelated_placeholder_untouched() {
    let (_temp, vault, day) = setup("bob-cli-star-st7");
    write_file(&vault.join("job.md"), "- [*] #task Cleanup ^cleanup\n");
    write_file(&day, &running_day("  - [[job#^cleanup]]\n"));
    let before = fs::read_to_string(&day).expect("day");
    assert!(before.contains("- [ ] Unrelated placeholder\n"), "{before}");
    capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(
        !day_after.contains("[[job#^cleanup]]"),
        "T's link is consumed:\n{day_after}"
    );
    assert!(
        day_after.contains("- [ ] Unrelated placeholder\n"),
        "placeholder untouched:\n{day_after}"
    );
    assert!(
        !day_after.contains("(1010-1035) — JOB"),
        "emptied running entry deleted:\n{day_after}"
    );
}

// ST8: `^prj` T blocks residents and reports `blocked_residents`.
#[test]
fn st8_prj_blocks_residents() {
    let (_temp, vault, day) = setup("bob-cli-star-st8");
    write_project(
        &vault,
        "proj.md",
        concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
            "- [/] #task Grind ^grind\n",
        ),
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (1010-1035) — JOB\n",
            "  - [[proj#^work]]\n",
            "    - authored work note\n",
            "  - [[proj#^grind]]\n",
            "  - other context\n",
        ),
    );

    let json = capture_json(&vault, &day, "Ask Kelly *proj:prj");
    assert_eq!(json["dependency_update"]["blocked_residents"], 2, "{json}");
    let proj = fs::read_to_string(vault.join("proj.md")).expect("proj");
    assert!(proj.contains("- [?] #task Ship"), "{proj}");
    assert!(
        task_line(&proj, "work").contains("- [?] ")
            && task_line(&proj, "work").contains("#prj_is_blocked"),
        "work resident is Blocked with #prj_is_blocked:\n{proj}"
    );
    assert!(
        task_line(&proj, "grind").contains("- [?] ")
            && task_line(&proj, "grind").contains("#prj_is_blocked"),
        "grind resident is Blocked with #prj_is_blocked:\n{proj}"
    );
    let day_after = fs::read_to_string(&day).expect("day");
    assert!(
        day_after.contains("  - Work\n    - authored work note\n"),
        "kept-notes strip tags from the resident line:\n{day_after}"
    );
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    assert!(
        inbox.contains("- [*] #task Ask Kelly"),
        "strongest resident [/] still caps new N at Next:\n{inbox}"
    );
    assert!(
        !day_after.contains("[[proj#^work]]")
            && !day_after.contains("[[proj#^grind]]"),
        "{day_after}"
    );
}

// ST9: `^prj` T refuses N in that project or a descendant; vault intact.
#[test]
fn st9_prj_internal_refuse() {
    let (_temp, vault, day) = setup("bob-cli-star-st9");
    write_project(
        &vault,
        "parent.md",
        "- [ ] #task Parent #hide ^prj\n- [ ] #task Staged resident ^staged\n",
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [ ] #task Child #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        ),
    );
    let parent_before =
        fs::read_to_string(vault.join("parent.md")).expect("parent");
    let child_before =
        fs::read_to_string(vault.join("child.md")).expect("child");

    let same =
        capture_fail(&vault, &day, "Internal *parent:prj @parent+staged");
    assert!(!same.status.success(), "{}", format_output(&same));
    assert!(
        stderr(&same).contains('*')
            && (stderr(&same).contains("same project")
                || stderr(&same).contains("cannot wait")),
        "{}",
        format_output(&same)
    );

    let descendant = capture_fail(&vault, &day, "Work *parent:prj @child+work");
    assert!(
        !descendant.status.success(),
        "{}",
        format_output(&descendant)
    );
    assert!(
        stderr(&descendant).contains("sub-project")
            || stderr(&descendant).contains("cannot wait"),
        "{}",
        format_output(&descendant)
    );
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).expect("parent"),
        parent_before
    );
    assert_eq!(
        fs::read_to_string(vault.join("child.md")).expect("child"),
        child_before
    );

    let new_in_project =
        capture_fail(&vault, &day, "Internal *parent:prj @parent");
    assert!(
        !new_in_project.status.success(),
        "{}",
        format_output(&new_in_project)
    );
    assert!(
        stderr(&new_in_project).contains('*'),
        "{}",
        format_output(&new_in_project)
    );

    let new_in_descendant =
        capture_fail(&vault, &day, "Work *parent:prj @child");
    assert!(
        !new_in_descendant.status.success(),
        "{}",
        format_output(&new_in_descendant)
    );
    assert!(
        stderr(&new_in_descendant).contains('*'),
        "{}",
        format_output(&new_in_descendant)
    );
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).expect("parent"),
        parent_before
    );
    assert_eq!(
        fs::read_to_string(vault.join("child.md")).expect("child"),
        child_before
    );
}

// ST10: direct, existing, transitive, and cross-item cycles refuse.
#[test]
fn st10_cycles_refuse() {
    let (_temp, vault, day) = setup("bob-cli-star-st10");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(&vault.join("cash.md"), "- [ ] #task Budget ^budget\n");
    let job_before = fs::read_to_string(vault.join("job.md")).expect("job");

    let direct =
        capture_fail(&vault, &day, "Ask Kelly *job:cleanup &job:cleanup");
    assert!(!direct.status.success(), "{}", format_output(&direct));
    assert!(
        stderr(&direct).contains("cycle"),
        "{}",
        format_output(&direct)
    );
    assert_eq!(
        fs::read_to_string(vault.join("job.md")).expect("job"),
        job_before
    );

    write_file(
        &vault.join("body.md"),
        concat!(
            "- [ ] #task Exercise [dependsOn:: job__cleanup] ^work\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^cleanup]]\n",
        ),
    );
    let body_before = fs::read_to_string(vault.join("body.md")).expect("body");
    let existing = capture_fail(&vault, &day, "*job:cleanup @body+work");
    assert!(!existing.status.success(), "{}", format_output(&existing));
    assert!(
        stderr(&existing).contains("cycle"),
        "{}",
        format_output(&existing)
    );
    assert_eq!(
        fs::read_to_string(vault.join("body.md")).expect("body"),
        body_before
    );

    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup [dependsOn:: cash__budget] ^cleanup\n",
            "\t- ⛓️ **DEPENDS ON:** [[cash#^budget]]\n",
        ),
    );
    let job_mid = fs::read_to_string(vault.join("job.md")).expect("job");
    let cash_before = fs::read_to_string(vault.join("cash.md")).expect("cash");
    let transitive =
        capture_fail(&vault, &day, "Ask Kelly *cash:budget &job:cleanup");
    assert!(
        !transitive.status.success(),
        "{}",
        format_output(&transitive)
    );
    assert!(
        stderr(&transitive).contains("cycle"),
        "{}",
        format_output(&transitive)
    );
    assert_eq!(
        fs::read_to_string(vault.join("job.md")).expect("job"),
        job_mid
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("cash"),
        cash_before
    );

    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    let cross = capture_fail(
        &vault,
        &day,
        "Ask Kelly ^ask-kelly *job:cleanup\n\nPrep *mac_inbox:ask-kelly &job:cleanup",
    );
    assert!(!cross.status.success(), "{}", format_output(&cross));
    assert!(
        stderr(&cross).contains("cycle"),
        "{}",
        format_output(&cross)
    );
}

// ST11: A8 teaching refusals.
#[test]
fn st11_a8_refusals() {
    let (_temp, vault, day) = setup("bob-cli-star-st11");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [x] #task Done ^done\n",
            "- [ ] #task Two lines ^two\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^cleanup]]\n",
            "\t- ⛓️ **DEPENDS ON:** [[job#^done]]\n",
            "- [ ] #task Broken ^broken\n",
            "\t- ⛓️ **DEPENDS ON:** needs review\n",
            "```md\n",
            "- [ ] #task Fenced example ^fenced\n",
            "```\n",
        ),
    );
    write_file(&vault.join("2026/20261014.md"), "- [ ] #task Daily ^gtd\n");
    write_file(&vault.join("done/old.md"), "- [ ] #task Archived ^old\n");
    write_file(
        &vault.join("_templates/t.md"),
        "- [ ] #task Template ^tmpl\n",
    );

    let cases: &[(&str, &str)] = &[
        ("Ask Kelly *job:done", "completed"),
        ("Ask Kelly *2026/20261014:gtd", "daily note"),
        ("Ask Kelly *done/old:old", "done/"),
        ("Ask Kelly *_templates/t:tmpl", "_templates"),
        ("Ask Kelly *job:fenced", "fenced"),
        ("Ask Kelly *job:two", "Depends-On"),
        ("Ask Kelly *job:broken", "malformed"),
        ("*job:cleanup @job+cleanup", "itself"),
    ];
    for (draft, needle) in cases {
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "{draft} should refuse:\n{}",
            format_output(&output)
        );
        let message = stderr(&output);
        assert!(message.contains('*'), "{draft}: must name *:\n{message}");
        assert!(
            message
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
                || message.contains(needle),
            "{draft} ({needle}): {message}"
        );
    }
}

// ST12: already_present no-op; second live run no-op.
#[test]
fn st12_already_present_noop() {
    let (_temp, vault, day) = setup("bob-cli-star-st12");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(
        &vault.join("mac_inbox.md"),
        "- [ ] #task Ask Kelly ^ask-kelly\n",
    );

    let first = capture_json(&vault, &day, "*job:cleanup @mac_inbox+ask-kelly");
    assert!(
        first["dependency_update"]["added"].as_u64().unwrap() >= 1,
        "{first}"
    );
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");

    let repeat =
        capture_json(&vault, &day, "*job:cleanup @mac_inbox+ask-kelly");
    assert_eq!(repeat["dependency_update"]["added"], 0, "{repeat}");
    assert!(
        repeat["dependency_update"]["already_present"]
            .as_u64()
            .unwrap()
            >= 1,
        "{repeat}"
    );
    assert_eq!(fs::read_to_string(vault.join("job.md")).expect("job"), job);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox"),
        inbox
    );
}

// ST13: multiple targets; duplicate is a no-op.
#[test]
fn st13_multiple_and_duplicate() {
    let (_temp, vault, day) = setup("bob-cli-star-st13");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [ ] #task Send batch ^send-batch\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        "Ask Kelly *job:cleanup *job:send-batch *job:cleanup",
    );
    assert_eq!(json["dependency_updates"].as_array().unwrap().len(), 2);
    assert_eq!(
        json["dependency_updates"][0]["dependent_block_id"],
        "cleanup"
    );
    assert_eq!(
        json["dependency_updates"][1]["dependent_block_id"],
        "send-batch"
    );
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    assert_eq!(
        job.matches("[[mac_inbox#^").count(),
        2,
        "duplicate did not add a third edge:\n{job}"
    );

    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [ ] #task Send batch ^send-batch\n",
        ),
    );
    let cased =
        capture_json(&vault, &day, "Ask Kelly *job:cleanup *Job:cleanup");
    assert_eq!(cased["dependency_updates"].as_array().unwrap().len(), 1);
}

// ST14: `N &P *T` inserts N between T and P.
#[test]
fn st14_insert_between() {
    let (_temp, vault, day) = setup("bob-cli-star-st14");
    write_file(&vault.join("cash.md"), "- [ ] #task Budget ^budget\n");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");

    let json =
        capture_json(&vault, &day, "Ask Kelly &cash:budget *job:cleanup");
    assert_eq!(json["dependency_updates"].as_array().unwrap().len(), 2);
    assert_eq!(json["dependency_updates"][0]["role"], "dependent");
    assert_eq!(json["dependency_updates"][1]["role"], "blocked_task");
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    assert!(
        inbox.contains("- [?] #task Ask Kelly")
            && inbox.contains("[[cash#^budget]]"),
        "N waits on P:\n{inbox}"
    );
    assert!(
        job.contains("- [?] #task Cleanup") && job.contains("[[mac_inbox#^"),
        "T waits on N:\n{job}"
    );
}

// ST15: kill switch still mints N's id and writes no BLOCKS line.
#[test]
fn st15_kill_switch_still_mints() {
    let (_temp, vault, day) = setup("bob-cli-star-st15");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    let config = vault.join("config.yml");
    write_file(&config, "dependencies:\n  blocks_lines: false\n");

    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .env("BOB_NOW", NOW)
            .env("BOB_DAY_FILE", &day)
            .env("BOB_CONFIG_FILE", &config)
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command.arg("--").arg("Ask Kelly *job:cleanup");
        let output = command.output().expect("star with kill switch");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("json")
    };
    let dry = run(true);
    let json = run(false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    if let Some(serde_json::Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = serde_json::Value::Bool(false);
        }
    }
    assert_eq!(masked, json, "ST15 dry-run JSON must match live");
    let minted = json["minted_blocker_block_id"]["block_id"]
        .as_str()
        .expect("minted");
    let inbox = fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox");
    assert!(inbox.contains(&format!("^{minted}")), "{inbox}");
    assert!(!inbox.contains("🧱 **BLOCKS:**"), "{inbox}");
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    assert!(job.contains("⛓️ **DEPENDS ON:**"), "{job}");
    let _ = json;
}

// ST16: later `@T:id` drops N (capture-link-drops).
#[test]
fn st16_later_link_drops_n() {
    let (_temp, vault, day) = setup("bob-cli-star-st16");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");

    let json =
        capture_json(&vault, &day, "Ask Kelly *job:cleanup\n\n@job:cleanup");
    assert_eq!(json["captures"].as_array().unwrap().len(), 2);
    assert_eq!(
        json["captures"][0]["dependency_update"]["role"],
        "blocked_task"
    );
    assert_eq!(
        json["captures"][1]["dropped_dependencies"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{json}"
    );
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    assert!(
        !job.contains("⛓️ **DEPENDS ON:**"),
        "N dropped from T:\n{job}"
    );
}

// ST17: `N *T` vs two-item `&` spelling; vault matches with a shared id.
#[test]
fn st17_two_item_equivalent() {
    let star_tmp = TempDir::new("bob-cli-star-st17-star");
    let amp_tmp = TempDir::new("bob-cli-star-st17-amp");
    for temp in [&star_tmp, &amp_tmp] {
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault");
        write_blocked_tasks_settings(&vault);
        write_file(&vault.join("job.md"), "- [*] #task Cleanup ^cleanup\n");
        write_file(&vault.join("day.md"), "## Pomodoros\n- [ ] () — later\n");
    }
    let star_vault = star_tmp.path().join("vault");
    let amp_vault = amp_tmp.path().join("vault");
    let star_day = star_vault.join("day.md");
    let amp_day = amp_vault.join("day.md");

    capture_json(
        &star_vault,
        &star_day,
        "Ask Kelly @mac_inbox:ask-kelly *job:cleanup",
    );
    capture_json(
        &amp_vault,
        &amp_day,
        "Ask Kelly @mac_inbox:ask-kelly\n\n&mac_inbox:ask-kelly @job+cleanup",
    );

    let star_job = fs::read_to_string(star_vault.join("job.md")).expect("job");
    let amp_job = fs::read_to_string(amp_vault.join("job.md")).expect("job");
    assert_eq!(star_job, amp_job, "T notes must match");
    let star_inbox =
        fs::read_to_string(star_vault.join("mac_inbox.md")).expect("inbox");
    let amp_inbox =
        fs::read_to_string(amp_vault.join("mac_inbox.md")).expect("inbox");
    assert_eq!(star_inbox, amp_inbox, "N notes must match");
    let star_day = fs::read_to_string(&star_day).expect("star day");
    let amp_day = fs::read_to_string(&amp_day).expect("amp day");
    assert_eq!(star_day, amp_day, "day files must match");
}

// ST19: existing owner + sub-bullet; forced `--task`/`--route`.
#[test]
fn st19_existing_owner_and_forced_task() {
    let (_temp, vault, day) = setup("bob-cli-star-st19");
    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");

    let json = capture_json(&vault, &day, "*job:cleanup @body+work");
    assert_eq!(json["kind"], "task_dependency");
    let body = fs::read_to_string(vault.join("body.md")).expect("body");
    assert!(
        task_line(&body, "work").contains("- [/] "),
        "existing owner takes In Progress:\n{body}"
    );

    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let sub = capture_json(&vault, &day, "Note text @body+work *job:cleanup");
    assert_eq!(sub["kind"], "sub_bullet");
    let body = fs::read_to_string(vault.join("body.md")).expect("body");
    assert!(body.contains("Note text"), "{body}");
    assert!(
        task_line(&body, "work").contains("- [/] "),
        "sub-bullet parent transferred:\n{body}"
    );

    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let forced = capture_json_flags(
        &vault,
        &day,
        &["--route", "body", "--task", "work"],
        "Forced note *job:cleanup",
    );
    assert_eq!(forced["kind"], "sub_bullet");
    let body = fs::read_to_string(vault.join("body.md")).expect("body");
    assert!(body.contains("Forced note"), "{body}");
    assert!(
        task_line(&body, "work").contains("- [/] "),
        "forced --task owner transferred:\n{body}"
    );

    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let colon = capture_json(&vault, &day, "@body:work *job:cleanup");
    assert_eq!(colon["kind"], "task_dependency");
    let body = fs::read_to_string(vault.join("body.md")).expect("body");
    assert!(
        task_line(&body, "work").contains("- [/] "),
        "@route:id *T existing owner transferred:\n{body}"
    );
}

#[test]
fn human_star_line_and_consumption() {
    let (_temp, vault, day) = setup("bob-cli-star-human");
    write_file(&vault.join("job.md"), "- [/] #task Cleanup ^cleanup\n");
    write_file(&day, &running_day("  - [[job#^cleanup]]\n"));

    let output = capture_human(&vault, &day, "Ask Kelly *job:cleanup");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("🧱 blocks \"Cleanup\""), "{out}");
    assert!(out.contains("[/] → [?]"), "{out}");
    assert!(
        out.contains("removed its Task Link from JOB 10:10–10:35 (running)"),
        "{out}"
    );
    assert!(out.contains("deleted the empty Pomodoro"), "{out}");

    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let existing = capture_human(&vault, &day, "*job:cleanup @body+work");
    assert_success(&existing);
    let out = stdout(&existing);
    assert!(out.contains("🧱 ^work now blocks \"Cleanup\""), "{out}");
    assert!(
        out.contains("Add blocker from \"Exercise\""),
        "*-only existing-owner header names the owner:\n{out}"
    );

    write_file(&vault.join("job.md"), "- [ ] #task Other ^other\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let sub = capture_human(&vault, &day, "Note text @body+work *job:other");
    assert_success(&sub);
    let out = stdout(&sub);
    assert!(out.contains("🧱 ^work now blocks \"Other\""), "{out}");

    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    let dry = capture_human_flags(
        &vault,
        &day,
        &["--dry-run"],
        "*job:cleanup @body+work",
    );
    assert_success(&dry);
    let out = stdout(&dry);
    assert!(
        out.contains("🧱 ^work would now block \"Cleanup\""),
        "{out}"
    );
    assert!(out.contains("would add blocker from \"Exercise\""), "{out}");

    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");
    assert_success(&capture_human(&vault, &day, "*job:cleanup @body+work"));
    let already = capture_human(&vault, &day, "*job:cleanup @body+work");
    assert_success(&already);
    let out = stdout(&already);
    assert!(out.contains("🧱 ^work already blocks \"Cleanup\""), "{out}");
    assert!(
        !out.contains('→'),
        "already-present has no status arrow:\n{out}"
    );
}

#[test]
fn human_dry_run_residents_and_kept_notes() {
    let (_temp, vault, day) = setup("bob-cli-star-human-residents");
    write_project(
        &vault,
        "proj.md",
        concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "- [ ] #task Work ^work\n",
            "- [/] #task Grind ^grind\n",
        ),
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (1010-1035) — JOB\n",
            "  - [[proj#^work]]\n",
            "    - authored work note\n",
            "  - [[proj#^grind]]\n",
        ),
    );
    let dry = capture_human_flags(
        &vault,
        &day,
        &["--dry-run"],
        "Ask Kelly *proj:prj",
    );
    assert_success(&dry);
    let out = stdout(&dry);
    assert!(out.contains("would block 2 project tasks"), "{out}");
    assert!(out.contains("would block \"Ship"), "{out}");

    let live = capture_human(&vault, &day, "Ask Kelly *proj:prj");
    assert_success(&live);
    let out = stdout(&live);
    assert!(out.contains("blocked 2 project tasks"), "{out}");
    assert!(
        out.contains("kept its notes") || stdout(&live).contains("Work"),
        "{out}"
    );
}

#[test]
fn same_note_task_blocks_relocate_by_block_id() {
    let (_temp, vault, day) = setup("bob-cli-star-relocate");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [ ] #task Other ^other\n",
        ),
    );
    let json = capture_json(&vault, &day, "Ask Kelly @job *job:cleanup");
    let ids = task_block_ids(&json);
    assert!(
        !ids.iter().any(|id| id == "other"),
        "untouched same-note task is not a task_block: {ids:?}\n{json}"
    );
    assert!(
        ids.iter().any(|id| id == "cleanup"),
        "T is reported: {ids:?}\n{json}"
    );
    let minted = json["minted_blocker_block_id"]["block_id"]
        .as_str()
        .expect("minted");
    assert_eq!(
        ids.iter().filter(|id| *id == minted).count(),
        1,
        "N appears once: {ids:?}\n{json}"
    );
    let job_before = fs::read_to_string(vault.join("job.md")).expect("job");

    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [ ] #task Send batch ^send-batch\n",
        ),
    );
    let two = capture_json(
        &vault,
        &day,
        "Ask Kelly @job *job:cleanup *job:send-batch",
    );
    let minted = two["minted_blocker_block_id"]["block_id"]
        .as_str()
        .expect("minted");
    let ids = task_block_ids(&two);
    assert_eq!(
        ids.iter().filter(|id| *id == minted).count(),
        1,
        "two * targets still report N once: {ids:?}\n{two}"
    );
    let _ = job_before;
}

#[test]
fn ampersand_same_note_prerequisite_below_dependent_relocates() {
    let (_temp, vault, day) = setup("bob-cli-amp-relocate");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "- [ ] #task Third ^third\n",
        ),
    );
    let json = capture_json(&vault, &day, "&job:third @job+cleanup");
    let ids = task_block_ids(&json);
    assert!(
        ids.iter().any(|id| id == "cleanup"),
        "dependent is reported: {ids:?}\n{json}"
    );
    assert!(
        ids.iter().any(|id| id == "third"),
        "prerequisite is reported: {ids:?}\n{json}"
    );
    assert_eq!(json["dependency_update"], json["dependency_updates"][0]);
    assert!(
        !json["dependency_updates"].as_array().unwrap().is_empty(),
        "{json}"
    );
}

#[test]
fn close_with_ampersand_emits_dependency_updates_array() {
    let (_temp, vault, day) = setup("bob-cli-amp-close-updates");
    write_file(&vault.join("cash.md"), "- [ ] #task Budget ^budget\n");
    write_file(&day, &running_day("  - other context\n"));
    let json = capture_json(
        &vault,
        &day,
        "Follow up @mac_inbox:follow-up=x &cash:budget",
    );
    assert!(json.get("dependency_update").is_some(), "{json}");
    assert_eq!(
        json["dependency_update"], json["dependency_updates"][0],
        "{json}"
    );
    assert_eq!(json["dependency_updates"][0]["role"], "dependent");
}

#[test]
fn forced_target_rejections_name_the_typed_sigil() {
    let (_temp, vault, day) = setup("bob-cli-star-forced");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^work\n");

    let task_ref = capture_pomodoro_ref("- [ ] #task Exercise ^work", 1);
    let star_ref = capture_fail_flags(
        &vault,
        &day,
        &["--route", "body", "--task-ref", &task_ref],
        "Forced note *job:cleanup",
    );
    assert!(!star_ref.status.success(), "{}", format_output(&star_ref));
    assert!(
        stderr(&star_ref).contains("'*'"),
        "{}",
        format_output(&star_ref)
    );

    let amp_ref = capture_fail_flags(
        &vault,
        &day,
        &["--route", "body", "--task-ref", &task_ref],
        "Forced note &job:cleanup",
    );
    assert!(!amp_ref.status.success(), "{}", format_output(&amp_ref));
    assert!(
        stderr(&amp_ref).contains("'&'"),
        "{}",
        format_output(&amp_ref)
    );
}

#[test]
fn fenced_example_does_not_hide_real_task() {
    let (_temp, vault, day) = setup("bob-cli-star-fenced-real");
    write_file(
        &vault.join("job.md"),
        concat!(
            "```md\n",
            "- [ ] #task Example ^cleanup\n",
            "```\n",
            "- [ ] #task Cleanup ^cleanup\n",
        ),
    );
    let json = capture_json(&vault, &day, "Ask Kelly *job:cleanup");
    assert_eq!(json["dependency_update"]["role"], "blocked_task");
    let job = fs::read_to_string(vault.join("job.md")).expect("job");
    assert!(
        job.contains("- [?] #task Cleanup"),
        "unfenced task is blocked:\n{job}"
    );
}
