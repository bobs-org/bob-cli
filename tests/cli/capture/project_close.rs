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
    let mut full = vec!["-f", "json", "--"];
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

    let json = capture_json(&vault, &day_file, &["!sase:done-p"]);
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
    let reasons: Vec<(&str, &str)> = unblocked
        .iter()
        .map(|row| {
            (
                row["block_id"].as_str().unwrap_or(""),
                row["not_linked"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert!(reasons.contains(&("prj", "project_task")), "{reasons:?}");
    assert!(
        reasons.contains(&("pitch", "project_member")),
        "{reasons:?}"
    );

    let proj = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(!proj.contains("#prj_is_blocked"), "{proj}");
    assert!(proj.contains("- [ ] #task Pitch"), "{proj}");
    assert!(proj.contains("[unblocked:: 2026-10-05]"), "{proj}");
}
