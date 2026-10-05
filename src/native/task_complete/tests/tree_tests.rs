//! Tree-close tests: nested embeds, left-open reasons, root policy,
//! and completion-field spacing.
use super::super::tree::add_or_replace_completion_field;
use super::*;

const DATE: &str = "2026-10-05";

fn tree_note() -> String {
    concat!(
        "- [*] #task Fix flaky test ^fix-flaky\n",
        "  - ![[sase#^write-test]]\n",
        "  - ![[sase#^ask-infra]]\n",
        "  - ![[sase#^water]]\n",
        "- [/] #task Write the regression test ^write-test\n",
        "- [?] #task Ask infra ^ask-infra\n",
        "- [/] #task Water plants [repeat:: every week] ^water\n",
    )
    .to_string()
}

#[test]
fn nested_embeds_close_with_blocked_and_recurring_left_open() {
    let mut vault = MemoryVault::new();
    vault.insert("sase.md", &tree_note());
    let root = vault.absolute("sase.md");
    let outcome = complete_task_tree(
        &vault,
        &root,
        "fix-flaky",
        DATE,
        RootPolicy::Explicit,
    )
    .expect("tree closes");
    assert!(outcome.warnings.is_empty());
    let root_transition = outcome.root.expect("root closed");
    assert_eq!(root_transition.block_id, "fix-flaky");
    assert_eq!(root_transition.previous_status_symbol, '*');
    assert_eq!(root_transition.status_symbol, 'x');
    assert_eq!(outcome.closed_subtasks.len(), 1);
    assert_eq!(outcome.closed_subtasks[0].block_id, "write-test");
    assert_eq!(outcome.closed_subtasks[0].previous_status_symbol, '/');
    let left: BTreeMap<&str, &LeftOpenTask> = outcome
        .left_open
        .iter()
        .map(|task| (task.block_id.as_str(), task))
        .collect();
    assert_eq!(left.len(), 2);
    assert_eq!(left["ask-infra"].reason, LeftOpenReason::Blocked);
    assert_eq!(left["ask-infra"].status_symbol, '?');
    assert_eq!(left["water"].reason, LeftOpenReason::Recurring);
    let updated = outcome.changed_files.get(&root).expect("sase.md changed");
    assert!(updated.contains(
        "- [x] #task Fix flaky test  [completion:: 2026-10-05] ^fix-flaky"
    ));
    assert!(updated.contains(
        "- [x] #task Write the regression test  [completion:: 2026-10-05] ^write-test"
    ));
    assert!(updated.contains("- [?] #task Ask infra ^ask-infra"));
    assert!(updated
        .contains("- [/] #task Water plants [repeat:: every week] ^water"));
}

#[test]
fn explicit_completes_blocked_root_while_close_link_refuses_it() {
    let mut vault = MemoryVault::new();
    vault.insert("sase.md", "- [?] #task Blocked root ^blocked\n");
    let root = vault.absolute("sase.md");
    let explicit = complete_task_tree(
        &vault,
        &root,
        "blocked",
        DATE,
        RootPolicy::Explicit,
    )
    .expect("explicit closes a Blocked root");
    assert!(explicit.root.is_some());
    assert!(explicit.left_open.is_empty());
    assert!(explicit.changed_files[&root].contains(
        "- [x] #task Blocked root  [completion:: 2026-10-05] ^blocked"
    ));

    let link = complete_task_tree(
        &vault,
        &root,
        "blocked",
        DATE,
        RootPolicy::CloseLink,
    )
    .expect("close-link refuses a Blocked root");
    assert!(link.root.is_none());
    assert!(link.changed_files.is_empty());
    assert_eq!(link.left_open.len(), 1);
    assert_eq!(link.left_open[0].block_id, "blocked");
    assert_eq!(link.left_open[0].reason, LeftOpenReason::Blocked);
}

#[test]
fn completion_field_spacing_with_and_without_block_id() {
    assert_eq!(
        add_or_replace_completion_field("- [x] #task Done ^done", DATE),
        "- [x] #task Done  [completion:: 2026-10-05] ^done"
    );
    assert_eq!(
        add_or_replace_completion_field("- [x] #task Done", DATE),
        "- [x] #task Done  [completion:: 2026-10-05]"
    );
    assert_eq!(
        add_or_replace_completion_field(
            "- [x] #task Done [completion:: 2026-10-01] ^done",
            DATE
        ),
        "- [x] #task Done  [completion:: 2026-10-05] ^done"
    );
}

#[test]
fn done_root_is_a_no_op_that_still_closes_open_children() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "sase.md",
        concat!(
            "- [x] #task Already done ^done\n",
            "  - ![[sase#^child]]\n",
            "- [ ] #task Open child ^child\n",
        ),
    );
    let root = vault.absolute("sase.md");
    let outcome =
        complete_task_tree(&vault, &root, "done", DATE, RootPolicy::Explicit)
            .expect("done root is a no-op");
    assert!(outcome.root.is_none());
    assert_eq!(outcome.closed_subtasks.len(), 1);
    assert_eq!(outcome.closed_subtasks[0].block_id, "child");
}

#[test]
fn unknown_status_descendant_is_left_open() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "sase.md",
        concat!(
            "- [ ] #task Root ^root\n",
            "  - ![[sase#^weird]]\n",
            "- [-] #task Canceled child ^weird\n",
        ),
    );
    let root = vault.absolute("sase.md");
    let outcome =
        complete_task_tree(&vault, &root, "root", DATE, RootPolicy::Explicit)
            .expect("tree closes");
    assert!(outcome.root.is_some());
    assert!(outcome.closed_subtasks.is_empty());
    assert_eq!(outcome.left_open.len(), 1);
    assert_eq!(outcome.left_open[0].block_id, "weird");
    assert_eq!(outcome.left_open[0].reason, LeftOpenReason::UnknownStatus);
}
