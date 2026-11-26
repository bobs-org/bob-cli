use std::{fs, path::Path};

use super::{
    catalog::{plan_area_catalogs, CatalogState},
    graph::build_area_graph,
};
use crate::native::notebooks::snapshot::VaultSnapshot;

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn area(parent: &str, extra: &str) -> String {
    format!(
        "---\ntype: \"[[area]]\"\nparent: \"[[{parent}]]\"\n{extra}---\n\n# Area\n"
    )
}

fn build(
    root: &Path,
) -> (VaultSnapshot, super::AreaGraph, super::catalog::CatalogPlan) {
    let snapshot = VaultSnapshot::load(root);
    assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
    let graph = build_area_graph(&snapshot);
    let plan = plan_area_catalogs(&snapshot, &graph);
    (snapshot, graph, plan)
}

fn callout(contents: &str) -> String {
    let start = contents.find("> [!areas]").expect("areas callout");
    let rest = &contents[start..];
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

fn real_shape(root: &Path) {
    write(root, "area.md", "# Area root\n\nThe untyped root note.\n");
    for (name, parent) in [
        ("body", "area"),
        ("cash", "area"),
        ("dev", "area"),
        ("gtd", "area"),
        ("job", "area"),
        ("love", "area"),
        ("meet", "area"),
        ("garden", "cash"),
        ("recur", "cash"),
        ("gtd_daily", "gtd"),
        ("inbox", "gtd"),
        ("gkeep_inbox", "inbox"),
        ("gmail_inbox", "inbox"),
        ("mac_inbox", "inbox"),
        ("abby", "love"),
        ("family", "love"),
        ("fun", "love"),
        ("kelly", "love"),
        ("vacation", "love"),
    ] {
        write(root, &format!("{name}.md"), &area(parent, ""));
    }
}

#[test]
fn real_vault_shape_plans_the_five_nested_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    real_shape(temp.path());
    let (_, graph, plan) = build(temp.path());

    assert_eq!(graph.areas.len(), 19);
    assert_eq!(graph.root_children.len(), 7);
    assert!(graph.findings.is_empty());
    assert_eq!(plan.edits().len(), 5);
    assert_eq!(
        plan.hosts
            .iter()
            .map(|host| host.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec![
            "area.md".to_string(),
            "cash.md".to_string(),
            "gtd.md".to_string(),
            "inbox.md".to_string(),
            "love.md".to_string(),
        ]
    );

    let expected = [
        (
            "area.md",
            "> [!areas]\n>\n> - **[[body]]**\n> - **[[cash]]**\n>   - **[[garden]]**\n>   - **[[recur]]**\n> - **[[dev]]**\n> - **[[gtd]]**\n>   - **[[gtd_daily]]**\n>   - **[[inbox]]**\n>     - **[[gkeep_inbox]]**\n>     - **[[gmail_inbox]]**\n>     - **[[mac_inbox]]**\n> - **[[job]]**\n> - **[[love]]**\n>   - **[[abby]]**\n>   - **[[family]]**\n>   - **[[fun]]**\n>   - **[[kelly]]**\n>   - **[[vacation]]**\n> - **[[meet]]**",
        ),
        (
            "cash.md",
            "> [!areas]\n>\n> - **[[garden]]**\n> - **[[recur]]**",
        ),
        (
            "gtd.md",
            "> [!areas]\n>\n> - **[[gtd_daily]]**\n> - **[[inbox]]**\n>   - **[[gkeep_inbox]]**\n>   - **[[gmail_inbox]]**\n>   - **[[mac_inbox]]**",
        ),
        (
            "inbox.md",
            "> [!areas]\n>\n> - **[[gkeep_inbox]]**\n> - **[[gmail_inbox]]**\n> - **[[mac_inbox]]**",
        ),
        (
            "love.md",
            "> [!areas]\n>\n> - **[[abby]]**\n> - **[[family]]**\n> - **[[fun]]**\n> - **[[kelly]]**\n> - **[[vacation]]**",
        ),
    ];
    for (host, expected) in expected {
        let edit = plan
            .hosts
            .iter()
            .find(|item| item.path.as_path() == Path::new(host))
            .unwrap();
        assert_eq!(
            callout(&edit.edit.as_ref().unwrap().after),
            expected,
            "{host}"
        );
    }
    let leaves = [
        "body",
        "dev",
        "job",
        "meet",
        "garden",
        "recur",
        "gtd_daily",
        "gkeep_inbox",
        "gmail_inbox",
        "mac_inbox",
        "abby",
        "family",
        "fun",
        "kelly",
        "vacation",
    ];
    for leaf in leaves {
        assert!(
            plan.host(Path::new(&format!("{leaf}.md"))).is_none(),
            "{leaf}"
        );
    }

    for edit in plan.edits() {
        fs::write(temp.path().join(&edit.path), &edit.after).unwrap();
    }
    let (_, _, second) = build(temp.path());
    assert!(second.edits().is_empty(), "second plan is idempotent");
    assert!(second
        .hosts
        .iter()
        .all(|host| host.state == CatalogState::Current));
}

#[test]
fn root_resolution_prefers_exact_area_and_retains_untyped_root_bytes() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "# Untyped root\n\nBody\n");
    write(temp.path(), "sub/area.md", &area("area", ""));
    write(temp.path(), "top.md", &area("area", ""));
    write(
        temp.path(),
        "type.md",
        "---\ntype: \"[[type]]\"\n---\n\n# Type\n",
    );

    let (snapshot, graph, plan) = build(temp.path());
    assert_eq!(
        graph.root.as_ref().unwrap().path.as_deref(),
        Some(Path::new("area.md"))
    );
    assert_eq!(
        snapshot.contents(Path::new("area.md")),
        Some("# Untyped root\n\nBody\n")
    );
    assert!(!graph.areas.contains_key(Path::new("area.md")));
    assert_eq!(plan.hosts.len(), 1);
    assert_eq!(plan.hosts[0].path, Path::new("area.md"));
    assert!(plan.host(Path::new("type.md")).is_none());

    let typed = tempfile::tempdir().unwrap();
    write(
        typed.path(),
        "area.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\n---\n# Sentinel\n",
    );
    write(typed.path(), "top.md", &area("area", ""));
    let (_, graph, plan) = build(typed.path());
    assert!(!graph.areas.contains_key(Path::new("area.md")));
    assert!(!graph.findings.iter().any(|f| f.code == "area_parent_self"));
    assert_eq!(plan.hosts.len(), 1);
    assert_eq!(plan.hosts[0].kind.as_str(), "root");
}

#[test]
fn missing_or_ambiguous_root_reports_once_without_top_level_noise() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "top.md", &area("area", ""));
    let (_, graph, _) = build(temp.path());
    assert_eq!(graph.root.as_ref().unwrap().state.as_str(), "missing");
    assert_eq!(
        graph
            .findings
            .iter()
            .filter(|f| f.code == "area_root_missing")
            .count(),
        1
    );
    assert!(!graph
        .findings
        .iter()
        .any(|f| f.code == "area_parent_unresolved"));

    let ambiguous = tempfile::tempdir().unwrap();
    write(ambiguous.path(), "a/area.md", &area("absent", ""));
    write(ambiguous.path(), "b/area.md", &area("absent", ""));
    write(ambiguous.path(), "top.md", &area("area", ""));
    let (_, graph, _) = build(ambiguous.path());
    assert_eq!(graph.root.as_ref().unwrap().state.as_str(), "ambiguous");
    assert_eq!(
        graph
            .findings
            .iter()
            .filter(|f| f.code == "area_root_ambiguous")
            .count(),
        1
    );
    assert!(!graph
        .findings
        .iter()
        .any(|f| f.code == "area_parent_unresolved"));
}

#[test]
fn parent_findings_cover_lists_missing_ambiguous_self_kind_and_unresolved() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "untyped\n");
    write(
        temp.path(),
        "listed.md",
        "---\ntype: \"[[area]]\"\nparent:\n  - \"[[area]]\"\n---\n",
    );
    write(temp.path(), "duplicate.md", "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\nparent: \"[[other]]\"\n---\n");
    write(temp.path(), "missing.md", "---\ntype: \"[[area]]\"\n---\n");
    write(temp.path(), "unresolved.md", &area("no_such_note", ""));
    write(temp.path(), "self.md", &area("self", ""));
    write(
        temp.path(),
        "project.md",
        "---\ntype: \"[[project]]\"\nparent: \"[[area]]\"\n---\n",
    );
    write(temp.path(), "wrong_kind.md", &area("project", ""));
    write(temp.path(), "one/duplicate_name.md", &area("area", ""));
    write(temp.path(), "two/duplicate_name.md", &area("area", ""));
    write(temp.path(), "ambiguous.md", &area("duplicate_name", ""));

    let (_, graph, _) = build(temp.path());
    for (path, code) in [
        ("listed.md", "area_parent_list"),
        ("duplicate.md", "area_parent_list"),
        ("missing.md", "area_parent_missing"),
        ("unresolved.md", "area_parent_unresolved"),
        ("self.md", "area_parent_self"),
        ("wrong_kind.md", "area_parent_kind"),
        ("ambiguous.md", "area_parent_ambiguous"),
    ] {
        assert!(
            graph.findings.iter().any(
                |issue| issue.path == Path::new(path) && issue.code == code
            ),
            "{path}: {code}"
        );
    }
}

#[test]
fn parent_alias_path_heading_and_unquoted_forms_resolve() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "root\n");
    write(temp.path(), "nested/parent.md", &area("area", ""));
    write(
        temp.path(),
        "alias.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[parent|Parent]]\"\n---\n",
    );
    write(
        temp.path(),
        "heading.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[nested/parent#Section]]\"\n---\n",
    );
    write(
        temp.path(),
        "path.md",
        "---\ntype: \"[[area]]\"\nparent: \"[[nested/parent.md]]\"\n---\n",
    );
    write(
        temp.path(),
        "unquoted.md",
        "---\ntype: \"[[area]]\"\nparent: [[area]]\n---\n",
    );

    let (_, graph, _) = build(temp.path());
    for child in ["alias.md", "heading.md", "path.md"] {
        assert_eq!(
            graph.areas[Path::new(child)].parent.as_deref(),
            Some(Path::new("nested/parent.md")),
            "{child}"
        );
    }
    assert_eq!(
        graph.areas[Path::new("unquoted.md")].parent.as_deref(),
        Some(Path::new("area.md"))
    );
    assert!(graph
        .findings
        .iter()
        .any(|f| f.path == Path::new("unquoted.md")
            && f.code == "area_parent_unquoted"));
}

#[test]
fn cycle_edges_drop_but_valid_children_below_broken_areas_remain() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "root\n");
    write(temp.path(), "a.md", &area("b", ""));
    write(temp.path(), "b.md", &area("a", ""));
    write(temp.path(), "child.md", &area("a", ""));
    write(temp.path(), "broken.md", &area("missing", ""));
    write(temp.path(), "valid_child.md", &area("broken", ""));

    let (_, graph, plan) = build(temp.path());
    assert_eq!(
        graph
            .findings
            .iter()
            .filter(|f| f.code == "area_cycle")
            .count(),
        2
    );
    assert!(graph.areas[Path::new("a.md")].parent.is_none());
    assert_eq!(
        graph.areas[Path::new("child.md")].parent.as_deref(),
        Some(Path::new("a.md"))
    );
    assert_eq!(graph.areas[Path::new("child.md")].depth, None);
    let host = plan.host(Path::new("a.md")).unwrap();
    assert_eq!(
        host.rows
            .iter()
            .map(|row| row.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("child.md")]
    );
    let broken_host = plan.host(Path::new("broken.md")).unwrap();
    assert_eq!(broken_host.rows[0].path, Path::new("valid_child.md"));
}

#[test]
fn hop_limit_breaks_the_overlong_chain_and_links_keep_path_aliases() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "root\n");
    let mut parent = "area".to_string();
    for index in 0..17 {
        let name = format!("a{index}");
        write(temp.path(), &format!("{name}.md"), &area(&parent, ""));
        parent = name;
    }
    write(temp.path(), "outer/ambiguous.md", &area("area", ""));
    write(temp.path(), "other/ambiguous.md", &area("area", ""));
    write(temp.path(), "child.md", &area("outer/ambiguous", ""));

    let (_, graph, plan) = build(temp.path());
    assert!(graph
        .findings
        .iter()
        .any(|f| f.path == Path::new("a16.md") && f.code == "area_cycle"));
    assert_eq!(graph.areas[Path::new("child.md")].link, "child");
    assert_eq!(
        graph.areas[Path::new("child.md")].parent.as_deref(),
        Some(Path::new("outer/ambiguous.md"))
    );
    let linked_row = plan
        .host(Path::new("outer/ambiguous.md"))
        .unwrap()
        .edit
        .as_ref()
        .unwrap()
        .after
        .clone();
    assert!(linked_row.contains("[[child]]"));
    let root_after = &plan
        .host(Path::new("area.md"))
        .unwrap()
        .edit
        .as_ref()
        .unwrap()
        .after;
    assert!(root_after.contains("[[other/ambiguous|ambiguous]]"));
    assert!(root_after.contains("[[outer/ambiguous|ambiguous]]"));
}

#[test]
fn summaries_are_inert_and_non_hosts_clean_up_only_owned_regions() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "area.md", "root\n");
    write(temp.path(), "parent.md", &area("area", ""));
    write(
        temp.path(),
        "child.md",
        &area("parent", "summary: \"[[x]] #tag **bold**\\nnext\"\n"),
    );
    write(temp.path(), "project.md", "---\ntype: \"[[project]]\"\n---\n\n## Areas\n\n> [!areas]\n>\n> - old\n");
    write(temp.path(), "type.md", "---\ntype: \"[[type]]\"\n---\n\n## Areas\n\n> [!areas]\n>\n> authored\n");

    let (_, graph, plan) = build(temp.path());
    let rendered = plan
        .host(Path::new("parent.md"))
        .unwrap()
        .edit
        .as_ref()
        .unwrap()
        .after
        .clone();
    assert!(callout(&rendered)
        .contains("\\[\\[x\\]\\] \\#tag \\*\\*bold\\*\\* next"));
    let project_cleanup = plan.host(Path::new("project.md")).unwrap();
    assert_eq!(project_cleanup.state, CatalogState::Stale);
    assert!(!project_cleanup
        .edit
        .as_ref()
        .unwrap()
        .after
        .contains("> [!areas]"));
    assert!(!project_cleanup
        .edit
        .as_ref()
        .unwrap()
        .after
        .contains("## Areas"));
    assert!(plan.host(Path::new("type.md")).is_none());
    assert!(graph
        .findings
        .iter()
        .any(|f| f.code == "area_summary_multiline"));
}
