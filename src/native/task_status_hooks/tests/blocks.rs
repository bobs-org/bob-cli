//! Explicit-edge export for the BLOCKS pass (`docs/task-dependencies.md`
//! §14.3, §14.6).
use super::*;
use crate::native::project_dependencies::{
    ProjectBlockReasons, ProjectContext, ProjectTree,
};

fn scan(entries: &[(&str, &str)]) -> Vec<FileScan> {
    let settings = test_settings();
    entries
        .iter()
        .map(|(path, contents)| FileScan {
            path: PathBuf::from(path),
            relative_path: PathBuf::from(path),
            contents: contents.to_string(),
            tasks: parse_tasks(contents, &settings),
        })
        .collect()
}

fn project_tree_for(files: &[FileScan], index: &NoteIndex) -> ProjectTree {
    let contexts = files
        .iter()
        .map(ProjectContext::for_note)
        .collect::<Vec<_>>();
    let reasons = vec![ProjectBlockReasons::default(); files.len()];
    ProjectTree::build(files, &contexts, &reasons, index)
}

fn reconcile_edges(
    entries: &[(&str, &str)],
) -> (Vec<FileScan>, ReconcileOutcome) {
    let settings = test_settings();
    let mut files = scan(entries);
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let tree = project_tree_for(&files, &index);
    let mut inputs = Vec::new();
    let outcome = reconcile_dependencies(
        Path::new("/vault"),
        &mut files,
        &index,
        &tree,
        &settings,
        None,
        &mut inputs,
    );
    (files, outcome)
}

fn edge_targets<'a>(
    files: &'a [FileScan],
    outcome: &'a ReconcileOutcome,
    path: &str,
    block_id: Option<&str>,
) -> Option<&'a BTreeSet<(PathBuf, String)>> {
    files.iter().enumerate().find_map(|(file_index, file)| {
        if display_path(&file.relative_path) != path {
            return None;
        }
        file.tasks
            .iter()
            .enumerate()
            .find_map(|(task_index, task)| {
                let matches = match block_id {
                    Some(block_id) => {
                        task.block_id.as_deref() == Some(block_id)
                    }
                    None => task.block_id.is_none(),
                };
                matches
                    .then_some(
                        outcome.explicit_edges.get(&(file_index, task_index)),
                    )
                    .flatten()
            })
    })
}

#[test]
fn explicit_edges_include_idless_dependents() {
    // Id-less dependents still export their accepted targets so the
    // BLOCKS pass can mint and list them (BR4).
    let (files, outcome) = reconcile_edges(&[(
        "tasks.md",
        concat!(
            "- [ ] #task Dependent\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]]\n",
            "- [ ] #task Prerequisite ^pre\n",
        ),
    )]);
    let targets = edge_targets(&files, &outcome, "tasks.md", None)
        .expect("id-less dependent is in explicit_edges");
    assert!(targets.contains(&(PathBuf::from("tasks.md"), "pre".into())));
}

#[test]
fn explicit_edges_exclude_implicit_resident_and_br18_br20() {
    // Implicit resident → ^prj edges are added only to the R7 graph.
    // BR18 (unresolved, self, non-task) and BR20 (^prj internal) stay
    // off the BLOCKS edge set.
    let project = concat!(
        "---\n",
        "type: \"[[project]]\"\n",
        "status: wip\n",
        "---\n",
        "- [ ] #task #prj Owner ^prj\n",
        "  - ⛓️ **DEPENDS ON:** [[#^internal]] • [[#^missing]] • [[#^prj]]\n",
        "  ```\n",
        "  ^notatask\n",
        "  ```\n",
        "- [ ] #task Internal target ^internal\n",
        "- [ ] #task Resident without a line ^res\n",
        "- [ ] #task Heading target\n",
        "  ^notatask\n",
    );
    let (files, outcome) = reconcile_edges(&[
        ("proj.md", project),
        (
            "tasks.md",
            concat!(
                "- [ ] #task Outside ^out\n",
                "  - ⛓️ **DEPENDS ON:** [[proj#^prj]]\n",
            ),
        ),
    ]);
    assert!(
        edge_targets(&files, &outcome, "proj.md", Some("res")).is_none(),
        "implicit resident edges are not BLOCKS edges"
    );
    let prj_targets = edge_targets(&files, &outcome, "proj.md", Some("prj"));
    if let Some(targets) = prj_targets {
        assert!(
            !targets.contains(&(PathBuf::from("proj.md"), "internal".into())),
            "BR20: internal ^prj target is excluded"
        );
        assert!(
            !targets.contains(&(PathBuf::from("proj.md"), "prj".into())),
            "BR18/R6: self target is excluded"
        );
        assert!(
            !targets.iter().any(|(_, block)| block == "missing"),
            "BR18/R4: unresolved target is excluded"
        );
        assert!(
            !targets.iter().any(|(_, block)| block == "notatask"),
            "BR18/R5: non-task target is excluded"
        );
    }
    let outside = edge_targets(&files, &outcome, "tasks.md", Some("out"))
        .expect("external dependent lists the project task");
    assert!(outside.contains(&(PathBuf::from("proj.md"), "prj".into())));
}

#[test]
fn malformed_dependents_are_uncertain_not_edges() {
    // BR17/R10: a malformed Depends-On line records the dependent as
    // uncertain and exports no edges for it.
    let (files, outcome) = reconcile_edges(&[(
        "tasks.md",
        concat!(
            "- [ ] #task Messy ^messy\n",
            "  - ⛓️ **DEPENDS ON:** [[#^pre]] leftover\n",
            "- [ ] #task Prerequisite ^pre\n",
        ),
    )]);
    let messy = files[0]
        .tasks
        .iter()
        .position(|task| task.block_id.as_deref() == Some("messy"))
        .unwrap();
    assert!(outcome.uncertain_dependents.contains(&(0, messy)));
    assert!(edge_targets(&files, &outcome, "tasks.md", Some("messy")).is_none());
}
