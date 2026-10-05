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
