//! Close paths respect project blocking (PDC vectors,
//! `docs/task-dependencies.md` §13.16): a resident whose project is
//! still blocked stays `[?]` with `still_blocked{reason: project}`,
//! and a `^prj` recovery cascades to its residents with
//! `#prj_is_blocked` stripped and `[unblocked::]` written.

use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-05 09:30:00";

fn vault_with_settings(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    (temp, vault)
}

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> serde_json::Value {
    capture_json_flags(vault, day_file, &[], args)
}

fn capture_json_flags(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    flags: &[&str],
    args: &[&str],
) -> serde_json::Value {
    let mut full = vec!["-f", "json"];
    full.extend(flags.iter().copied());
    full.push("--");
    full.extend(args.iter().copied());
    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .args(full)
            .env("BOB_DAY_FILE", day_file)
            .env("BOB_NOW", NOW);
        command.output().expect("run bob capture json")
    };
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

// PDC1 (§13.16): completing a resident's last own prerequisite while
// its project stays blocked leaves the resident `[?]` with a
// `still_blocked{reason: project}` row carrying the project path.
#[test]
fn close_leaves_resident_blocked_by_project() {
    let (_temp, vault) =
        vault_with_settings("bob-cli-project-close-still-blocked");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Done P [id:: done-p] ^done-p\n",
    );
    write_file(
        &vault.join("job.md"),
        "- [ ] #task LinkedIn [id:: job__linkedin] ^linkedin\n",
    );
    write_file(
        &vault.join("proj.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [?] #task Apply [dependsOn:: job__linkedin] [id:: apply] ^prj\n- [?] #task Resident [dependsOn:: done-p] [id:: res] ^res\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () \u{2014} FIX\n");

    let json = capture_json(&vault, &day_file, &["!sase:done-p"]);
    assert_eq!(json["ok"], true);
    let complete = &json["task_complete"];
    assert_eq!(
        complete["unblocked"].as_array().expect("unblocked").len(),
        0
    );
    let still = complete["still_blocked"].as_array().expect("still_blocked");
    assert_eq!(still.len(), 1);
    assert_eq!(still[0]["block_id"], "res");
    assert_eq!(still[0]["reason"], "project");
    assert_eq!(still[0]["project"], "proj.md");

    let proj = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(proj.contains("- [?] #task Resident"), "{proj}");
}

// PDC2 (§13.16): completing the `^prj`'s last prerequisite recovers
// the `^prj` and its residents; residents lose `#prj_is_blocked`
// and Ready recoveries with `[fresh::]` gain `[unblocked::]`.
#[test]
fn close_recovers_prj_and_residents_with_unblocked() {
    let (_temp, vault) = vault_with_settings("bob-cli-project-close-recover");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Done P [id:: done-p] ^done-p\n",
    );
    write_file(
        &vault.join("proj.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n- [?] #task Pitch [fresh:: 2026-10-01] #prj_is_blocked [id:: pitch] ^pitch\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () \u{2014} FIX\n");

    let json = capture_json_flags(
        &vault,
        &day_file,
        &["-p", "all"],
        &["!sase:done-p"],
    );
    assert_eq!(json["ok"], true);
    let complete = &json["task_complete"];
    assert_eq!(
        complete["still_blocked"]
            .as_array()
            .expect("still_blocked")
            .len(),
        0
    );
    let unblocked = complete["unblocked"].as_array().expect("unblocked");
    let prj = unblocked
        .iter()
        .find(|row| row["block_id"] == "prj")
        .expect("^prj");
    assert_eq!(prj["not_linked"], "project_task");
    assert!(prj.get("offer").is_none(), "{prj}");
    let pitch = unblocked
        .iter()
        .find(|row| row["block_id"] == "pitch")
        .expect("pitch");
    assert!(pitch.get("not_linked").is_none(), "{pitch}");
    assert_eq!(pitch["status_symbol"], "*");
    assert_eq!(pitch["offer"]["selected"], true);
    assert!(pitch["link"].is_object(), "{pitch}");

    let proj = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(!proj.contains("#prj_is_blocked"), "{proj}");
    let pitch_line = proj
        .lines()
        .find(|line| line.contains("^pitch"))
        .expect("pitch line");
    assert!(pitch_line.contains("- [*] #task Pitch"), "{pitch_line}");
    assert!(pitch_line.contains("[fresh:: 2026-10-05]"), "{pitch_line}");
    assert!(!pitch_line.contains("[unblocked::"), "{pitch_line}");
}

// PDC4 (§13.16): D is a resident of K whose `^prj` carries
// `#prj_is_blocked`; closing D's last own prerequisite leaves D `[?]`
// with `still_blocked{reason: project, project: parent(K)}` and no link.
#[test]
fn close_leaves_inherited_resident_blocked_naming_parent() {
    let (_temp, vault) =
        vault_with_settings("bob-cli-project-close-inherited-still-blocked");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Done P [id:: done-p] ^done-p\n",
    );
    write_file(
        &vault.join("par.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Parent chore [id:: chore] ^chore\n",
    );
    write_file(
        &vault.join("k.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[par]]\"\n---\n- [?] #task Kid #prj_is_blocked ^prj\n- [?] #task Dee [dependsOn:: done-p] #prj_is_blocked [id:: dee] ^dee\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () \u{2014} FIX\n");

    let json = capture_json(&vault, &day_file, &["!sase:done-p"]);
    assert_eq!(json["ok"], true);
    let complete = &json["task_complete"];
    assert_eq!(
        complete["unblocked"].as_array().expect("unblocked").len(),
        0
    );
    let still = complete["still_blocked"].as_array().expect("still_blocked");
    assert_eq!(still.len(), 1);
    assert_eq!(still[0]["block_id"], "dee");
    assert_eq!(still[0]["reason"], "project");
    assert_eq!(still[0]["project"], "par.md");

    let k = fs::read_to_string(vault.join("k.md")).expect("read k");
    assert!(
        k.contains("- [?] #task Dee [dependsOn:: done-p] #prj_is_blocked [id:: dee] ^dee"),
        "D stays byte-identical:\n{k}"
    );
}

// PDC5 (§13.16): K's tagged `^prj` loses its last own prerequisite →
// stays `[?]` with `still_blocked{reason: project}`; no
// `project_task` recover and K's residents keep their tags.
#[test]
fn close_leaves_tagged_prj_blocked_without_recovery() {
    let (_temp, vault) =
        vault_with_settings("bob-cli-project-close-tagged-prj");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Done P [id:: done-p] ^done-p\n",
    );
    write_file(
        &vault.join("par.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Parent chore [id:: chore] ^chore\n",
    );
    write_file(
        &vault.join("k.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[par]]\"\n---\n- [?] #task Kid [dependsOn:: done-p] #prj_is_blocked [id:: kprj] ^prj\n- [?] #task Rita #prj_is_blocked [id:: rita] ^rita\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () \u{2014} FIX\n");

    let json = capture_json(&vault, &day_file, &["!sase:done-p"]);
    assert_eq!(json["ok"], true);
    let complete = &json["task_complete"];
    assert_eq!(
        complete["unblocked"].as_array().expect("unblocked").len(),
        0,
        "no project_task or project_member recovery"
    );
    let still = complete["still_blocked"].as_array().expect("still_blocked");
    assert_eq!(still.len(), 1);
    assert_eq!(still[0]["block_id"], "prj");
    assert_eq!(still[0]["reason"], "project");
    assert_eq!(still[0]["project"], "par.md");

    let k = fs::read_to_string(vault.join("k.md")).expect("read k");
    assert!(
        k.contains("- [?] #task Kid [dependsOn:: done-p] #prj_is_blocked [id:: kprj] ^prj"),
        "K stays byte-identical:\n{k}"
    );
    assert!(
        k.contains("- [?] #task Rita #prj_is_blocked [id:: rita] ^rita"),
        "residents keep their tags until reconcile:\n{k}"
    );
}
