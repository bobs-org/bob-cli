//! Dependent-recovery tests: sole prerequisites recover; open, future,
//! and non-Blocked dependents do not; unresolved ids never block.
use super::super::recovery::recover_blocked_dependents;
use super::*;
use chrono::NaiveDate;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 5).expect("date")
}

fn completed(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn snapshot(entries: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
    entries
        .iter()
        .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
        .collect()
}

#[test]
fn sole_prerequisite_recovers_to_ready() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [x] #task Done root [id:: done-root] ^done\n",
            "- [?] #task Waiting on root [dependsOn:: done-root] [id:: waiter] ^waiter\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert_eq!(recovery.recovered.len(), 1);
    assert_eq!(recovery.recovered[0].block_id.as_deref(), Some("waiter"));
    assert_eq!(recovery.recovered[0].line, 2);
    assert_eq!(recovery.recovered[0].status_symbol, ' ');
    let updated = recovery
        .changed_files
        .get(&PathBuf::from("sase.md"))
        .expect("sase.md changed");
    assert!(updated.contains(
        "- [ ] #task Waiting on root [dependsOn:: done-root] [id:: waiter] ^waiter"
    ));
}

#[test]
fn second_open_prerequisite_keeps_dependent_blocked() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [x] #task Done root [id:: done-root] ^done\n",
            "- [ ] #task Still open [id:: live-other] ^other\n",
            "- [?] #task Waiting on both [dependsOn:: done-root, live-other] [id:: waiter] ^waiter\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert!(recovery.recovered.is_empty());
    assert!(recovery.changed_files.is_empty());
}

#[test]
fn future_scheduled_dependent_stays_blocked() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [x] #task Done root [id:: done-root] ^done\n",
            "- [?] #task Waiting with schedule [dependsOn:: done-root] [scheduled:: 2099-01-01] [id:: waiter] ^waiter\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert!(recovery.recovered.is_empty());
    assert!(recovery.changed_files.is_empty());
}

#[test]
fn non_blocked_dependent_is_untouched() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [x] #task Done root [id:: done-root] ^done\n",
            "- [/] #task Working with dep [dependsOn:: done-root] [id:: wip] ^wip\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert!(recovery.recovered.is_empty());
    assert!(recovery.changed_files.is_empty());
}

#[test]
fn unresolved_dependency_id_does_not_block_recovery() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [x] #task Done root [id:: done-root] ^done\n",
            "- [?] #task Waiting with ghost [dependsOn:: done-root, ghost-id] [id:: waiter] ^waiter\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert_eq!(recovery.recovered.len(), 1);
    assert_eq!(recovery.recovered[0].block_id.as_deref(), Some("waiter"));
}

#[test]
fn dependent_without_completed_prerequisite_stays_blocked() {
    let recovery = recover_blocked_dependents(
        snapshot(&[("sase.md", concat!(
            "- [ ] #task Open root [id:: open-root] ^open\n",
            "- [?] #task Waiting on open [dependsOn:: open-root] [id:: waiter] ^waiter\n",
        ))]),
        &completed(&["done-root"]),
        today(),
        &test_settings(),
    );
    assert!(recovery.recovered.is_empty());
    assert!(recovery.changed_files.is_empty());
}

fn project_note(body: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}")
}

fn recovered_ids(
    recovery: &super::super::recovery::DependentRecovery,
) -> BTreeSet<&str> {
    recovery
        .recovered
        .iter()
        .filter_map(|row| row.block_id.as_deref())
        .collect()
}

// PDC1 (§13.16): a resident whose project is still blocked stays
// `[?]`, even though this completion removed its last own reason.
#[test]
fn pdc1_project_resident_stays_blocked() {
    let recovery = recover_blocked_dependents(
        snapshot(&[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "job.md",
                "- [ ] #task LinkedIn [id:: job__linkedin] ^linkedin\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [dependsOn:: job__linkedin] [id:: apply] ^prj\n",
                    "- [?] #task Resident [dependsOn:: done-p] [id:: res] ^res\n",
                )),
            ),
        ]),
        &completed(&["done-p"]),
        today(),
        &test_settings(),
    );
    assert!(!recovered_ids(&recovery).contains("res"), "{recovery:?}");
    assert!(recovery.changed_files.is_empty());
}

// PDC2 (§12.2 PROJECT RECOVERY): a recovered `^prj` recovers its
// residents with no own reason; Ready recoveries strip
// `#prj_is_blocked` and write `[unblocked::]` (UB1).
#[test]
fn pdc2_prj_recovery_recovers_residents() {
    let recovery = recover_blocked_dependents(
        snapshot(&[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [fresh:: 2026-10-01] [dependsOn:: done-p] [id:: apply] ^prj\n",
                    "- [?] #task Pitch one [fresh:: 2026-10-01] #prj_is_blocked [id:: pitch-1] ^pitch-1\n",
                    "- [?] #task Pitch two [id:: pitch-2] ^pitch-2\n",
                )),
            ),
        ]),
        &completed(&["done-p"]),
        today(),
        &test_settings(),
    );
    assert_eq!(
        recovered_ids(&recovery),
        BTreeSet::from(["prj", "pitch-1", "pitch-2"])
    );
    let updated = recovery
        .changed_files
        .get(&PathBuf::from("proj.md"))
        .expect("proj.md changed");
    assert!(!updated.contains("#prj_is_blocked"), "{updated}");
    assert!(updated.contains("- [ ] #task Apply"), "{updated}");
    assert!(updated.contains("- [ ] #task Pitch one"), "{updated}");
    assert!(updated.contains("[unblocked:: 2026-10-05]"), "{updated}");
    // UB2: the NEW resident (no `[fresh::]`) recovers without the field.
    let pitch_two = updated
        .lines()
        .find(|line| line.contains("^pitch-2"))
        .expect("pitch two line");
    assert!(!pitch_two.contains("[unblocked::"), "{pitch_two}");
}

// PDC3 (§13.16): a resident with its own open prerequisite stays
// Blocked when its `^prj` recovers.
#[test]
fn pdc3_resident_with_own_prerequisite_stays_blocked() {
    let recovery = recover_blocked_dependents(
        snapshot(&[
            (
                "sase.md",
                "- [x] #task Done P [id:: done-p] ^done-p\n- [ ] #task Extra [id:: extra] ^extra\n",
            ),
            (
                "proj.md",
                &project_note(concat!(
                    "- [?] #task Apply [dependsOn:: done-p] [id:: apply] ^prj\n",
                    "- [?] #task Pitch [id:: pitch] ^pitch\n",
                    "- [?] #task Sequenced [dependsOn:: extra] [id:: seq] ^seq\n",
                )),
            ),
        ]),
        &completed(&["done-p"]),
        today(),
        &test_settings(),
    );
    let ids = recovered_ids(&recovery);
    assert_eq!(ids, BTreeSet::from(["prj", "pitch"]));
    assert!(!ids.contains("seq"));
}
