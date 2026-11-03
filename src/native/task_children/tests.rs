use std::path::Path;

use super::*;

#[test]
fn classifies_managed_links_logs_drawers_and_nested_tasks() {
    for (line, expected) in [
        (
            "\t- ⛓️ **DEPENDS ON:** [[#^prep]]",
            TaskChildKind::ManagedLink(ManagedLinkKind::DependsOn),
        ),
        (
            "\t- 🧱 **BLOCKS:** [[#^dependent]]",
            TaskChildKind::ManagedLink(ManagedLinkKind::Blocks),
        ),
        (
            "\t- 🪢 **LINKS:** [[#^related]]",
            TaskChildKind::ManagedLink(ManagedLinkKind::Links),
        ),
        (
            "\t- 🪢 **LINKED FROM:** [[other#^related]]",
            TaskChildKind::ManagedLink(ManagedLinkKind::LinkedFrom),
        ),
        (
            "\t- 🧩 **Sub-projects:** [[Child]]",
            TaskChildKind::ManagedLink(ManagedLinkKind::SubProjects),
        ),
        (
            "\t- 🗓️ **SCHEDULE LOG**",
            TaskChildKind::ManagedLog(ManagedLogKind::Schedule),
        ),
        (
            "\t- 🛠️ **WORK LOG**",
            TaskChildKind::ManagedLog(ManagedLogKind::Work),
        ),
        (
            "\t- 🥀 **ROTTEN LOG**",
            TaskChildKind::ManagedLog(ManagedLogKind::Rotten),
        ),
        (
            "\t- ❌ **CANCEL LOG**",
            TaskChildKind::ManagedLog(ManagedLogKind::Cancel),
        ),
        (
            "\t- REQUIREMENTS",
            TaskChildKind::Drawer {
                title: "REQUIREMENTS".to_string(),
            },
        ),
        (
            "\t- [ ] #task Nested",
            TaskChildKind::NestedTask { open: true },
        ),
        (
            "\t- [x] #task Closed",
            TaskChildKind::NestedTask { open: false },
        ),
        ("\t- an authored note", TaskChildKind::Authored),
    ] {
        let open = match &expected {
            TaskChildKind::NestedTask { open } => Some(*open),
            _ => None,
        };
        assert_eq!(classify_task_child(line, open), expected, "{line}");
    }
}

#[test]
fn direct_children_exclude_grandchildren_and_fenced_examples() {
    let contents = concat!(
        "- [ ] #task Parent ^parent\n",
        "  - direct note\n",
        "    - grandchild note\n",
        "  - [x] #task Closed child ^closed\n",
        "  - ```markdown\n",
        "    - not a child\n",
        "  - ```\n",
        "- [ ] #task Sibling ^sibling\n",
    );
    let settings = note_tasks::read_settings(Path::new("/no-bob-vault"));
    let scan = note_tasks::scan(contents, &settings);
    let parent = scan.tasks().first().expect("parent task");
    let children = classify_direct_children(contents, parent, &settings);

    assert_eq!(children.len(), 2);
    assert_eq!(children[0].line_index, 1);
    assert_eq!(children[0].kind, TaskChildKind::Authored);
    assert_eq!(children[1].line_index, 3);
    assert_eq!(children[1].kind, TaskChildKind::NestedTask { open: false });
}
