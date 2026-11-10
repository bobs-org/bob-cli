//! Pure catalog and creation tests over temporary fixture vaults.

use std::{ffi::OsStr, fs, path::Path};

use chrono::NaiveDate;

use super::{
    catalog::FileEdit,
    create::{plan_create, CreateRefusal, CreateRequest},
    model::CatalogState,
    scan::{analyze, Analysis},
    snapshot::VaultSnapshot,
};
use crate::native::env::TestEnvGuard;

fn guard() -> TestEnvGuard {
    TestEnvGuard::set(&[(
        "BOB_CONFIG_FILE",
        Some(OsStr::new("/nonexistent-bob-cli-test-config.yml")),
    )])
}

fn write(root: &Path, name: &str, body: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn host(kind: &str, parent: &str, extra: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntype: \"[[{kind}]]\"\n{extra}---\n\n# Host\n\n## Tasks\n\n- [ ] #task Keep going ^keep\n"
    )
}

fn notebook(parent: &str, summary: &str, title: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: {summary}\ncreated: 2026-11-09T09:14:00-05:00\n---\n\n# {title}\n\n- A note [created::2026-11-09]\n"
    )
}

/// dev (area) > proj (outermost project) > sub (non-root) > deep (root: true),
/// plus home (an area beneath dev).
fn tree(root: &Path) {
    write(root, "dev.md", &host("area", "area", ""));
    write(root, "home.md", &host("area", "dev", ""));
    write(root, "proj.md", &host("project", "dev", ""));
    write(root, "sub.md", &host("project", "proj", ""));
    write(root, "deep.md", &host("project", "sub", "root: true\n"));
    write(
        root,
        "dev_tools.md",
        &notebook("dev", "Command-line tools.", "Developer tools"),
    );
    write(root, "git.md", &notebook("dev_tools", "Git tips.", "Git"));
    write(root, "zed.md", &notebook("dev", "Zed notes.", "zed"));
    write(root, "alpha.md", &notebook("dev", "Sorts first.", "Alpha"));
    write(
        root,
        "proj_log.md",
        &notebook("proj", "Project log.", "Project log"),
    );
    write(
        root,
        "sub_notes.md",
        &notebook("sub", "Sub notes.", "Sub notes"),
    );
    write(
        root,
        "deep_notes.md",
        &notebook("deep", "Deep notes.", "Deep notes"),
    );
    write(
        root,
        "home_recipes.md",
        &notebook("home", "Recipes.", "Recipes"),
    );
}

fn analysis(root: &Path) -> Analysis {
    let snapshot = VaultSnapshot::load(root);
    assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
    analyze(&snapshot)
}

fn edit<'a>(analysis: &'a Analysis, host: &str) -> &'a FileEdit {
    analysis
        .plan
        .host(Path::new(host))
        .unwrap_or_else(|| panic!("{host} planned"))
        .edit
        .as_ref()
        .unwrap_or_else(|| panic!("{host} has an edit"))
}

fn callout(after: &str) -> String {
    let start = after.find("> [!notebooks]").expect("callout");
    let rest = &after[start..];
    let mut end = 0;
    for line in rest.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let indent = content.chars().take_while(|ch| *ch == ' ').count();
        if indent <= 3 && content[indent..].starts_with('>') {
            end += line.len();
        } else {
            break;
        }
    }
    rest[..end].trim_end_matches(['\n', '\r']).to_string()
}

#[test]
fn golden_tree_scopes_roots_and_groups() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    let analysis = analysis(temp.path());

    assert_eq!(
        callout(&edit(&analysis, "dev.md").after),
        "\
> [!notebooks]
>
> - **[[alpha|Alpha]]** — Sorts first.
> - **[[dev_tools|Developer tools]]** — Command-line tools.
>   - **[[git|Git]]** — Git tips.
> - **[[zed|zed]]** — Zed notes."
    );
    // proj is an outermost project: it stops dev's rollup, lists its own
    // notebooks first, then the non-root sub-project's under a group.
    assert_eq!(
        callout(&edit(&analysis, "proj.md").after),
        "\
> [!notebooks]
>
> - **[[proj_log|Project log]]** — Project log.
> - 🧩 [[sub]]
>   - **[[sub_notes|Sub notes]]** — Sub notes."
    );
    // A non-root sub-project also gets its own slice.
    assert_eq!(
        callout(&edit(&analysis, "sub.md").after),
        "> [!notebooks]\n>\n> - **[[sub_notes|Sub notes]]** — Sub notes."
    );
    // A `root: true` project and an area beneath an area stop the rollup.
    assert_eq!(
        callout(&edit(&analysis, "deep.md").after),
        "> [!notebooks]\n>\n> - **[[deep_notes|Deep notes]]** — Deep notes."
    );
    assert_eq!(
        callout(&edit(&analysis, "home.md").after),
        "> [!notebooks]\n>\n> - **[[home_recipes|Recipes]]** — Recipes."
    );

    // Each notebook appears in exactly one root catalog.
    let report = &analysis.report;
    for node in analysis.graph.notebooks.values() {
        let roots = node
            .hosts
            .iter()
            .filter(|path| {
                let host = &analysis.graph.hosts[*path];
                host.root.as_deref()
                    == host.path.file_stem().and_then(|s| s.to_str())
            })
            .count();
        assert_eq!(roots, 1, "{}", node.path.display());
    }
    assert_eq!(report.catalogs.len(), 5);
    assert!(report
        .catalogs
        .iter()
        .all(|record| record.state == CatalogState::Missing));
    assert_eq!(report.counts.warnings, 5, "one stale warning per host");
    assert_eq!(report.counts.errors, 0);
}

#[test]
fn block_lands_before_tasks_and_second_plan_is_current() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    let first = analysis(temp.path());
    for planned in first.plan.edits() {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    let dev = fs::read_to_string(temp.path().join("dev.md")).unwrap();
    let notebooks = dev.find("## Notebooks").unwrap();
    let tasks = dev.find("## Tasks").unwrap();
    assert!(notebooks < tasks, "inserted before ## Tasks:\n{dev}");

    let second = analysis(temp.path());
    assert!(second.plan.edits().is_empty(), "idempotent");
    assert!(second
        .report
        .catalogs
        .iter()
        .all(|record| record.state == CatalogState::Current));
    assert_eq!(second.report.counts.warnings, 0);
}

#[test]
fn reparent_and_edits_update_old_and_new_hosts() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    for planned in analysis(temp.path()).plan.edits() {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    // Reparent zed from dev to home, retitle git, and change a summary.
    write(
        temp.path(),
        "zed.md",
        &notebook("home", "Zed notes.", "zed"),
    );
    write(
        temp.path(),
        "git.md",
        &notebook("dev_tools", "New tip.", "Git!"),
    );
    let after = analysis(temp.path());
    let dev = &edit(&after, "dev.md").after;
    assert!(!dev.contains("zed"), "old owner drops the row:\n{dev}");
    assert!(dev.contains("**[[git|Git!]]** — New tip."));
    let home = &edit(&after, "home.md").after;
    assert!(
        home.contains("**[[zed|zed]]**"),
        "new owner gains it:\n{home}"
    );
    assert_eq!(
        after
            .plan
            .edits()
            .iter()
            .map(|edit| edit.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["dev.md".to_string(), "home.md".to_string()]
    );
}

#[test]
fn hosts_that_lose_every_notebook_are_planned_for_removal() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    for planned in analysis(temp.path()).plan.edits() {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    fs::remove_file(temp.path().join("home_recipes.md")).unwrap();
    let after = analysis(temp.path());
    let home = after.plan.host(Path::new("home.md")).expect("home planned");
    assert_eq!(home.record.state, CatalogState::Stale);
    assert_eq!(home.record.notebooks, 0);
    assert_eq!(
        home.edit.as_ref().unwrap().after,
        host("area", "dev", ""),
        "removal restores the original bytes"
    );
}

#[test]
fn damaged_host_is_reported_and_others_stay_plannable() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    let broken = format!(
        "{}\n<!-- bob:notebooks:begin v1 -->\n",
        host("area", "area", "")
    );
    write(temp.path(), "dev.md", &broken);
    let analysis = analysis(temp.path());
    let dev = analysis.plan.host(Path::new("dev.md")).unwrap();
    assert_eq!(dev.record.state, CatalogState::Damaged);
    assert!(dev.edit.is_none());
    assert_eq!(analysis.report.counts.errors, 1);
    assert!(analysis
        .plan
        .host(Path::new("home.md"))
        .unwrap()
        .edit
        .is_some());
}

#[test]
fn body_errors_do_not_drop_catalog_rows_but_structural_ones_do() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    // A checkbox is an error but the notebook still has a clear owner.
    write(
        temp.path(),
        "zed.md",
        &format!(
            "{}\n- [ ] not allowed\n",
            notebook("dev", "Zed notes.", "zed")
        ),
    );
    // A missing parent leaves the notebook unowned: never in a catalog.
    write(
        temp.path(),
        "orphan.md",
        &notebook("nowhere", "Lost.", "Orphan"),
    );
    let analysis = analysis(temp.path());
    let dev = &edit(&analysis, "dev.md").after;
    assert!(dev.contains("[[zed|zed]]"));
    assert!(!dev.contains("orphan"));
    assert!(analysis
        .graph
        .notebooks
        .get(Path::new("orphan.md"))
        .is_none());
}

#[test]
fn titles_and_summaries_render_inert() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "dev.md", &host("area", "area", ""));
    write(
        temp.path(),
        "odd.md",
        &notebook(
            "dev",
            "\"See [[Other]] and `code` <b>bold</b> *x* #tag ^id\"",
            "Odd [title](x)",
        ),
    );
    let analysis = analysis(temp.path());
    let dev = &edit(&analysis, "dev.md").after;
    assert!(dev.contains(
        "> - **[[odd|Odd \\[title\\](x)]]** — See \\[\\[Other\\]\\] and \\`code\\` \\<b\\>bold\\</b\\> \\*x\\* \\#tag \\^id"
    ), "{dev}");
}

fn request(parent: &str, id: &str) -> CreateRequest {
    CreateRequest {
        parent: parent.to_string(),
        id: id.to_string(),
        summary: "Neovim config notes.".to_string(),
        title: None,
        body: None,
        now: NaiveDate::from_ymd_opt(2026, 11, 9)
            .unwrap()
            .and_hms_opt(9, 14, 0)
            .unwrap(),
    }
}

fn refusal(snapshot: &VaultSnapshot, request: &CreateRequest) -> CreateRefusal {
    plan_create(snapshot, request).expect_err("refused")
}

#[test]
fn create_under_area_plans_file_and_catalog_together() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    for planned in analysis(temp.path()).plan.edits() {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    let snapshot = VaultSnapshot::load(temp.path());
    let plan = plan_create(&snapshot, &request("dev", "nvim")).unwrap();
    assert_eq!(plan.path.to_string_lossy(), "dev_nvim.md");
    assert_eq!(plan.title, "Nvim");
    assert!(plan.contents.starts_with(
        "---\nparent: \"[[dev]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: Neovim config notes.\ncreated: "
    ));
    assert!(plan.contents.ends_with("---\n\n# Nvim\n"));
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.edits[0].path.to_string_lossy(), "dev.md");
    assert!(plan.edits[0]
        .after
        .contains("> - **[[dev_nvim|Nvim]]** — Neovim config notes."));
    // The creation view is consistent: applying it leaves every host current.
    fs::write(temp.path().join(&plan.path), &plan.contents).unwrap();
    for planned in &plan.edits {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    let after = analysis(temp.path());
    assert!(after.plan.edits().is_empty(), "no stale host after create");
    assert_eq!(after.report.counts.errors, 0);
}

#[test]
fn create_under_closed_project_and_nested_notebook() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    write(
        temp.path(),
        "old.md",
        &host("project", "dev", "status: done\n"),
    );
    let snapshot = VaultSnapshot::load(temp.path());
    let closed = plan_create(&snapshot, &request("old", "log")).unwrap();
    assert_eq!(closed.path.to_string_lossy(), "old_log.md");
    assert_eq!(closed.edits[0].path.to_string_lossy(), "old.md");

    let nested =
        plan_create(&snapshot, &request("dev_tools", "my-tool")).unwrap();
    assert_eq!(nested.path.to_string_lossy(), "dev_tools_my_tool.md");
    assert!(nested.contents.contains("parent: \"[[dev_tools]]\""));
    assert_eq!(nested.edits[0].path.to_string_lossy(), "dev.md");
    assert!(nested.edits[0].after.contains(
        ">   - **[[dev_tools_my_tool|My tool]]** — Neovim config notes."
    ));
    // Ancestry that is read but not rewritten is guarded.
    let deps = format!("{:?}", nested.deps);
    assert!(deps.contains("dev_tools.md"), "parent guarded in {deps}");
}

#[test]
fn create_refusals_carry_stable_codes() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    write(temp.path(), "plain.md", "# Plain\n");
    write(temp.path(), "areas/twin.md", &host("area", "area", ""));
    write(temp.path(), "other/twin.md", &host("area", "area", ""));
    // Two notebooks that name each other: a cyclic chain.
    write(temp.path(), "loop_a.md", &notebook("loop_b", "A.", "A"));
    write(temp.path(), "loop_b.md", &notebook("loop_a", "B.", "B"));
    let snapshot = VaultSnapshot::load(temp.path());

    let code =
        |parent: &str, id: &str| refusal(&snapshot, &request(parent, id)).code;
    assert_eq!(code("missing", "x"), "notebook_parent_missing");
    assert_eq!(code("plain", "x"), "notebook_parent_unsupported");
    assert_eq!(code("loop_a", "x"), "notebook_parent_invalid");
    assert_eq!(code("dev", "bad id"), "notebook_id_invalid");
    assert_eq!(code("twin", "x"), "notebook_parent_ambiguous");
    // Mixed-case collision against an existing root file.
    write(
        temp.path(),
        "Dev_Nvim.md",
        &notebook("dev", "Mixed.", "Mixed"),
    );
    let snapshot = VaultSnapshot::load(temp.path());
    assert_eq!(
        refusal(&snapshot, &request("dev", "nvim")).code,
        "notebook_exists"
    );
}

#[test]
fn damaged_affected_host_refuses_but_unrelated_one_is_tolerated() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    for planned in analysis(temp.path()).plan.edits() {
        fs::write(temp.path().join(&planned.path), &planned.after).unwrap();
    }
    let healthy = fs::read_to_string(temp.path().join("home.md")).unwrap();
    write(
        temp.path(),
        "home.md",
        &format!("{healthy}\n<!-- bob:notebooks:begin v1 -->\n"),
    );
    let snapshot = VaultSnapshot::load(temp.path());
    // dev is healthy: creating under dev ignores the damaged home catalog.
    let plan = plan_create(&snapshot, &request("dev", "nvim")).unwrap();
    assert!(plan
        .edits
        .iter()
        .all(|edit| edit.path.to_string_lossy() != "home.md"));
    // Creating under the damaged host refuses the whole create.
    let error = refusal(&snapshot, &request("home", "tea"));
    assert_eq!(error.code, "notebook_catalog_damaged");
    assert!(error.message.contains("home.md"), "{}", error.message);
}

#[test]
fn overlay_snapshot_plans_like_the_disk() {
    let _guard = guard();
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path());
    let disk = VaultSnapshot::load(temp.path());
    let overlaid = disk.with_overlay([(
        "extra.md".into(),
        notebook("dev", "Staged only.", "Extra"),
    )]);
    let analysis = analyze(&overlaid);
    assert!(edit(&analysis, "dev.md")
        .after
        .contains("[[extra|Extra]]** — Staged only."));
    assert!(
        !temp.path().join("extra.md").exists(),
        "an overlay never touches the filesystem"
    );
    assert!(overlaid.contains_ignore_case(Path::new("EXTRA.md")));
}
