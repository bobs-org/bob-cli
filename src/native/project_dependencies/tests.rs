//! PDP/PDW vector tests copied from `docs/task-dependencies.md`
//! §13, citing the vector id in a comment, plus
//! project-recognition and pipeline-free PDB rows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::projects::parse_frontmatter;
use super::super::task_status_hooks::{
    parse_tasks, FileScan, TaskLine, TaskStatusType, TasksSettings,
};
use super::super::vault_links::NoteIndex;
use super::*;

fn parse_property(contents: &str) -> DependsOnProperty {
    let frontmatter =
        parse_frontmatter(contents).expect("test has frontmatter");
    parse_depends_on(&frontmatter)
}

fn block_note(items: &str) -> String {
    format!(
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n{items}---\n- [ ] #task Done ^prj\n"
    )
}

fn item_links(property: &DependsOnProperty) -> Vec<String> {
    match property {
        DependsOnProperty::Items(items) => {
            items.iter().map(|item| item.link.clone()).collect()
        }
        other => panic!("expected items, got {other:?}"),
    }
}

fn test_settings() -> TasksSettings {
    TasksSettings {
        global_filter: "#task".to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::OnHold),
            ('?', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: Vec::new(),
        status_settings_error: None,
    }
}

fn scan_note(
    relative_path: &str,
    contents: &str,
    settings: &TasksSettings,
) -> FileScan {
    FileScan {
        path: PathBuf::from("/vault").join(relative_path),
        relative_path: PathBuf::from(relative_path),
        contents: contents.to_string(),
        tasks: parse_tasks(contents, settings),
    }
}

fn project_note(extra_frontmatter: &str, body: &str) -> String {
    format!(
        "---\ntype: \"[[project]]\"\nstatus: wip\n{extra_frontmatter}---\n{body}"
    )
}

const PRJ_LINE: &str = "- [ ] #task Ship the thing #hide ^prj\n";

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("test date")
}

// PDP1 (§13.11): block list of two double-quoted links.
#[test]
fn pdp_block_list_parses_two_links() {
    let property = parse_property(&block_note(
        "  - \"[[job#^update-linkedin]]\"\n  - \"[[job_update_cv#^prj]]\"\n",
    ));
    assert_eq!(
        item_links(&property),
        vec![
            "[[job#^update-linkedin]]".to_string(),
            "[[job_update_cv#^prj]]".to_string(),
        ]
    );
    match property {
        DependsOnProperty::Items(items) => {
            assert_eq!(items[0].line_number, 5);
            assert_eq!(items[1].line_number, 6);
            assert!(!items[0].internal);
        }
        other => panic!("expected items, got {other:?}"),
    }
}

// PDP2 (§13.11): flow list of the same two links.
#[test]
fn pdp_flow_list_parses_two_links() {
    let property = parse_property(
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: [\"[[job#^a]]\", \"[[job#^b]]\"]\n---\n",
    );
    assert_eq!(
        item_links(&property),
        vec!["[[job#^a]]".to_string(), "[[job#^b]]".to_string(),]
    );
}

// PDP3 (§13.11): one quoted scalar.
#[test]
fn pdp_single_quoted_scalar_parses() {
    let property = parse_property(
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: \"[[job#^update-linkedin]]\"\n---\n",
    );
    assert_eq!(
        item_links(&property),
        vec!["[[job#^update-linkedin]]".to_string()]
    );
}

// PDP4 (§13.11): `depends_on: null` is empty.
#[test]
fn pdp_null_is_empty() {
    assert_eq!(
        parse_property(
            "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: null\n---\n"
        ),
        DependsOnProperty::Empty
    );
}

// PDP5 (§13.11): `depends_on: []` is empty.
#[test]
fn pdp_empty_flow_is_empty() {
    assert_eq!(
        parse_property(
            "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: []\n---\n"
        ),
        DependsOnProperty::Empty
    );
}

// PDP6 (§13.11): key present, no items, is empty.
#[test]
fn pdp_key_without_items_is_empty() {
    assert_eq!(
        parse_property(
            "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n---\n"
        ),
        DependsOnProperty::Empty
    );
}

// PDP7 (§13.11): an aliased link canonicalises away the alias.
#[test]
fn pdp_aliased_link_canonicalises() {
    let property = parse_property(&block_note(
        "  - \"[[job#^update-linkedin|LinkedIn]]\"\n",
    ));
    assert_eq!(
        item_links(&property),
        vec!["[[job#^update-linkedin]]".to_string()]
    );
}

// PDP8 (§13.11): an `!` embed canonicalises away the embed.
#[test]
fn pdp_embed_link_canonicalises() {
    let property =
        parse_property(&block_note("  - \"![[job#^update-linkedin]]\"\n"));
    assert_eq!(
        item_links(&property),
        vec!["[[job#^update-linkedin]]".to_string()]
    );
}

// PDP9 (§13.11): a bare project link gains `#^prj`.
#[test]
fn pdp_bare_project_link_gains_prj() {
    let property = parse_property(&block_note("  - \"[[job_update_cv]]\"\n"));
    match &property {
        DependsOnProperty::Items(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].link, "[[job_update_cv#^prj]]");
            assert_eq!(items[0].target, "job_update_cv");
            assert_eq!(items[0].block_id, "prj");
            assert!(!items[0].internal);
        }
        other => panic!("expected items, got {other:?}"),
    }
}

// PDP10 (§13.11): an unquoted flow item is malformed.
#[test]
fn pdp_unquoted_flow_item_is_malformed() {
    match parse_property(
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: [[job#^x]]\n---\n",
    ) {
        DependsOnProperty::Malformed(reason) => {
            assert!(reason.contains("line 4"), "{reason}");
        }
        other => panic!("expected malformed, got {other:?}"),
    }
}

// PDP11 (§13.11): duplicate keys are malformed.
#[test]
fn pdp_duplicate_keys_are_malformed() {
    match parse_property(
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n  - \"[[job#^a]]\"\ndepends_on:\n  - \"[[job#^b]]\"\n---\n",
    ) {
        DependsOnProperty::Malformed(reason) => {
            assert!(reason.contains("duplicate"), "{reason}");
        }
        other => panic!("expected malformed, got {other:?}"),
    }
}

// PDP12 (§13.11): a same-note link is internal.
#[test]
fn pdp_same_note_link_is_internal() {
    let property = parse_property(&block_note("  - \"[[#^x]]\"\n"));
    match &property {
        DependsOnProperty::Items(items) => {
            assert_eq!(items.len(), 1);
            assert!(items[0].internal);
            assert_eq!(items[0].link, "[[#^x]]");
        }
        other => panic!("expected items, got {other:?}"),
    }
}

// PDP13 (§13.11): an archive link keeps its explicit `done/` path.
#[test]
fn pdp_archive_link_keeps_done_path() {
    let property = parse_property(&block_note("  - \"[[done/note#^x]]\"\n"));
    assert_eq!(item_links(&property), vec!["[[done/note#^x]]".to_string()]);
}

// PDP14 (§13.11): an absent key means empty (D3).
#[test]
fn pdp_absent_key_means_empty() {
    assert_eq!(
        parse_property("---\ntype: \"[[project]]\"\nstatus: wip\n---\n"),
        DependsOnProperty::Absent
    );
}

fn splice(contents: &str, links: &[&str]) -> String {
    splice_depends_on(
        contents,
        &links.iter().map(ToString::to_string).collect::<Vec<_>>(),
    )
}

// PDW1 (§13.12): create the key before the closing `---`.
#[test]
fn pdw_creates_key_before_closing_fence() {
    assert_eq!(
        splice(
            "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Done ^prj\n",
            &["[[job#^a]]"],
        ),
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n  - \"[[job#^a]]\"\n---\n- [ ] #task Done ^prj\n"
    );
}

// PDW2 (§13.12): append keeps order.
#[test]
fn pdw_append_keeps_order() {
    assert_eq!(
        splice(
            &block_note("  - \"[[job#^a]]\"\n"),
            &["[[job#^a]]", "[[job#^b]]"],
        ),
        block_note("  - \"[[job#^a]]\"\n  - \"[[job#^b]]\"\n")
    );
}

// PDW3 (§13.12): remove the middle link.
#[test]
fn pdw_removes_middle_link() {
    assert_eq!(
        splice(
            &block_note(
                "  - \"[[job#^a]]\"\n  - \"[[job#^b]]\"\n  - \"[[job#^c]]\"\n"
            ),
            &["[[job#^a]]", "[[job#^c]]"],
        ),
        block_note("  - \"[[job#^a]]\"\n  - \"[[job#^c]]\"\n")
    );
}

// PDW4 (§13.12): remove the last link deletes the key.
#[test]
fn pdw_last_link_deletes_key() {
    let out = splice(&block_note("  - \"[[job#^a]]\"\n"), &[]);
    assert!(!out.contains("depends_on"), "{out}");
    assert_eq!(
        out,
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Done ^prj\n"
    );
}

// PDW5 (§13.12): the splice keeps the caller's order (a re-add moves
// to the end in the caller, which passes `[b, a]`).
#[test]
fn pdw_keeps_caller_order_on_readd() {
    assert_eq!(
        splice(
            &block_note("  - \"[[job#^a]]\"\n  - \"[[job#^b]]\"\n"),
            &["[[job#^b]]", "[[job#^a]]"],
        ),
        block_note("  - \"[[job#^b]]\"\n  - \"[[job#^a]]\"\n")
    );
}

// PDW6 (§13.12): a flow list is rewritten as a block list.
#[test]
fn pdw_flow_list_rewritten_as_block() {
    assert_eq!(
        splice(
            "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on: [\"[[job#^a]]\"]\n---\n",
            &["[[job#^a]]"],
        ),
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n  - \"[[job#^a]]\"\n---\n"
    );
}

// PDW7 (§13.12): CRLF notes splice with CRLF.
#[test]
fn pdw_crlf_preserved() {
    let out = splice(
        "---\r\ntype: \"[[project]]\"\r\nstatus: wip\r\n---\r\n",
        &["[[job#^a]]"],
    );
    assert_eq!(
        out,
        "---\r\ntype: \"[[project]]\"\r\nstatus: wip\r\ndepends_on:\r\n  - \"[[job#^a]]\"\r\n---\r\n"
    );
}

// PDW8 (§13.12): a note without a final newline gains none.
#[test]
fn pdw_no_final_newline_preserved() {
    let out = splice(
        "---\ntype: \"[[project]]\"\nstatus: wip\n---",
        &["[[job#^a]]"],
    );
    assert_eq!(
        out,
        "---\ntype: \"[[project]]\"\nstatus: wip\ndepends_on:\n  - \"[[job#^a]]\"\n---"
    );
}

// PDW9 (§13.12): unrelated YAML stays byte-identical.
#[test]
fn pdw_unrelated_yaml_untouched() {
    let before = "---\n# leading comment\ntype: \"[[project]]\"  # trailing\nstatus: wip\ncustom:  [1, 2]\ndepends_on: [\"[[job#^a]]\"]\n---\nbody\n";
    let out = splice(before, &["[[job#^a]]", "[[job#^b]]"]);
    assert_eq!(
        out,
        "---\n# leading comment\ntype: \"[[project]]\"  # trailing\nstatus: wip\ncustom:  [1, 2]\ndepends_on:\n  - \"[[job#^a]]\"\n  - \"[[job#^b]]\"\n---\nbody\n"
    );
}

// PDW10 (§13.12): a same-note target is written `[[note#^x]]`.
#[test]
fn pdw_same_note_target_uses_note_form() {
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("job.md"),
    ]);
    // Same-note targets never use the `[[#^x]]` form.
    assert_eq!(
        format_project_link(
            Path::new("proj.md"),
            "other",
            Path::new("proj.md"),
            &index,
        ),
        "[[proj#^other]]"
    );
    // Archive targets keep the explicit `done/` path even when the
    // basename is unique.
    assert_eq!(
        format_project_link(
            Path::new("done/old.md"),
            "x",
            Path::new("proj.md"),
            &index,
        ),
        "[[done/old#^x]]"
    );
    // Otherwise the shortest unambiguous form wins.
    assert_eq!(
        format_project_link(
            Path::new("job.md"),
            "a",
            Path::new("proj.md"),
            &index,
        ),
        "[[job#^a]]"
    );
}

fn context_for(
    relative_path: &str,
    contents: &str,
    settings: &TasksSettings,
) -> (FileScan, ProjectContext) {
    let file = scan_note(relative_path, contents, settings);
    let context = ProjectContext::for_note(&file);
    (file, context)
}

#[test]
fn recognition_accepts_a_plain_project() {
    let settings = test_settings();
    let (_, context) =
        context_for("proj.md", &project_note("", PRJ_LINE), &settings);
    assert!(context.recognised);
    assert_eq!(context.scheduled, None);
    assert_eq!(context.depends_on, DependsOnProperty::Absent);
}

#[test]
fn recognition_rejects_terminal_status() {
    // PDB4 (§13.14): a terminal project is not `project(T)`.
    let settings = test_settings();
    let contents = "---\ntype: \"[[project]]\"\nstatus: done\ndepends_on:\n  - \"[[job#^a]]\"\n---\n- [x] #task Ship #hide ^prj\n- [ ] #task Leftover ^a\n";
    let (_, context) = context_for("proj.md", contents, &settings);
    assert!(!context.recognised);
}

#[test]
fn recognition_rejects_multiple_open_prj() {
    // PDB6 (§13.14): two open `^prj` lines are not a project.
    let settings = test_settings();
    let (_, context) = context_for(
        "proj.md",
        &project_note("", "- [ ] #task One ^prj\n- [ ] #task Two ^prj\n"),
        &settings,
    );
    assert!(!context.recognised);
}

#[test]
fn recognition_ignores_daily_notes() {
    // PDB8 (§13.14): a canonical daily note typed `project` is ignored.
    let settings = test_settings();
    let (_, context) = context_for(
        "2026/20261012.md",
        &project_note("depends_on:\n  - \"[[job#^a]]\"\n", PRJ_LINE),
        &settings,
    );
    assert!(!context.recognised);
}

#[test]
fn recognition_rejects_non_project_type() {
    let settings = test_settings();
    let contents = "---\ntype: \"[[area]]\"\nstatus: wip\n---\n- [ ] #task Ship #hide ^prj\n";
    let (_, context) = context_for("area.md", contents, &settings);
    assert!(!context.recognised);
}

#[test]
fn future_project_schedule_is_a_reason() {
    // PDB7 (§13.14): a future project `scheduled` blocks with
    // `project_schedule`.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("scheduled: 2999-01-01\n", PRJ_LINE),
        &settings,
    );
    assert!(context.recognised);
    let files = vec![file.clone()];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![PathBuf::from("proj.md")]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert_eq!(reasons.future_scheduled, Some(date(2999, 1, 1)));
    assert!(reasons.open_dependencies.is_empty());
    assert!(!reasons.is_empty());
    let prj = file
        .tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some("prj"));
    let reasons = effective_block_reasons(
        prj.expect("prj parses"),
        &[],
        &context,
        &reasons,
        today,
    );
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].kind(), "project_schedule");
}

#[test]
fn open_dependency_blocks_prj_and_residents() {
    // PDB1/PDB2 (§13.14): `#ref` and nested residents are covered,
    // fenced lines are not, and `^prj` carries `project_dependency`.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let body = "- [?] #task Ship #hide ^prj\n- [ ] #task Ref work #ref ^a\n\t- [ ] #task Nested ^b\n```\n- [ ] #task Fenced example ^c\n```\n";
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[job#^x]]\"\n", body),
        &settings,
    );
    assert!(context.recognised);
    let job =
        scan_note("job.md", "---\n---\n- [ ] #task LinkedIn ^x\n", &settings);
    let files = vec![file.clone(), job];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("job.md"),
    ]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert_eq!(reasons.open_dependencies.len(), 1);
    assert_eq!(reasons.open_dependencies[0].link, "[[job#^x]]".to_string());
    let by_block: BTreeMap<&str, &TaskLine> = file
        .tasks
        .iter()
        .filter_map(|task| task.block_id.as_deref().map(|block| (block, task)))
        .collect();
    for block in ["prj", "a", "b"] {
        let task = by_block[block];
        assert!(context.covers(task), "{block} is covered");
        if block != "prj" {
            assert!(context.is_resident(task), "{block} is a resident");
        }
        let task_reasons =
            effective_block_reasons(task, &[], &context, &reasons, today);
        assert_eq!(task_reasons.len(), 1, "{block}");
        assert_eq!(task_reasons[0].kind(), "project_dependency");
    }
    let fenced = by_block["c"];
    assert!(!context.is_resident(fenced));
    assert!(!context.covers(fenced));
    assert!(
        effective_block_reasons(fenced, &[], &context, &reasons, today)
            .is_empty()
    );
}

#[test]
fn recovery_needs_every_reason_gone() {
    // PDB3 (§13.14): a resident with its own future `scheduled`
    // stays `[?]` after the project reason clears.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let body = "- [ ] #task Ship #hide ^prj\n- [ ] #task Slow [scheduled:: 2999-01-01] ^s\n";
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[job#^x]]\"\n", body),
        &settings,
    );
    let job =
        scan_note("job.md", "---\n---\n- [ ] #task LinkedIn ^x\n", &settings);
    let files = vec![file.clone(), job];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("job.md"),
    ]);
    let blocked = project_block_reasons(&context, &index, today, &openness);
    let resident = file
        .tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some("s"))
        .expect("resident parses");
    let kinds: Vec<&str> =
        effective_block_reasons(resident, &[], &context, &blocked, today)
            .iter()
            .map(BlockReason::kind)
            .collect();
    assert_eq!(kinds, vec!["local_schedule", "project_dependency"]);
    let cleared = ProjectBlockReasons::default();
    let kinds: Vec<&str> =
        effective_block_reasons(resident, &[], &context, &cleared, today)
            .iter()
            .map(BlockReason::kind)
            .collect();
    assert_eq!(kinds, vec!["local_schedule"]);
}

#[test]
fn unresolved_targets_never_block() {
    // R4 parity: a link that resolves nowhere contributes no reason.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[missing#^x]]\"\n", PRJ_LINE),
        &settings,
    );
    let files = vec![file];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![PathBuf::from("proj.md")]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert!(reasons.is_empty());
}

#[test]
fn archive_targets_are_closed() {
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[done/old#^x]]\"\n", PRJ_LINE),
        &settings,
    );
    let old =
        scan_note("done/old.md", "---\n---\n- [ ] #task Old ^x\n", &settings);
    let files = vec![file, old];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("done/old.md"),
    ]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert!(reasons.is_empty());
    match &context.depends_on {
        DependsOnProperty::Items(items) => {
            let resolved = resolve_dependencies(
                items,
                Path::new("proj.md"),
                &index,
                &openness,
            );
            assert!(resolved[0].archive);
            assert!(!resolved[0].open);
        }
        other => panic!("expected items, got {other:?}"),
    }
}

#[test]
fn explicit_same_note_target_is_internal() {
    // §13.6: a hand-authored internal target stays in YAML, is not
    // projected, and never blocks.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[proj#^other]]\"\n", PRJ_LINE),
        &settings,
    );
    assert!(context.recognised);
    let files = vec![file];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![PathBuf::from("proj.md")]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert!(reasons.is_empty());
    match &context.depends_on {
        DependsOnProperty::Items(items) => {
            let resolved = resolve_dependencies(
                items,
                Path::new("proj.md"),
                &index,
                &openness,
            );
            assert!(resolved[0].internal);
            assert!(!resolved[0].open);
        }
        other => panic!("expected items, got {other:?}"),
    }
}

#[test]
fn id_fallback_opens_unlocated_targets() {
    // A target whose block is missing at its resolved note still
    // blocks when its `[id::]` is open elsewhere.
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[other#^a]]\"\n", PRJ_LINE),
        &settings,
    );
    let other = scan_note(
        "other.md",
        "---\n---\n- [ ] #task Elsewhere ^z\n",
        &settings,
    );
    let third = scan_note(
        "third.md",
        "---\n---\n- [ ] #task Healed [id:: other__a] ^q\n",
        &settings,
    );
    let files = vec![file, other, third];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("other.md"),
        PathBuf::from("third.md"),
    ]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert_eq!(reasons.open_dependencies.len(), 1);
}

#[test]
fn malformed_property_contributes_no_targets() {
    let settings = test_settings();
    let today = date(2026, 10, 12);
    let (file, context) = context_for(
        "proj.md",
        &project_note("depends_on:\n  - \"[[job#^a]]\"\n  - oops\n", PRJ_LINE),
        &settings,
    );
    assert!(context.recognised);
    assert!(matches!(
        context.depends_on,
        DependsOnProperty::Malformed(_)
    ));
    let job =
        scan_note("job.md", "---\n---\n- [ ] #task LinkedIn ^a\n", &settings);
    let files = vec![file, job];
    let openness = Openness::for_files(&files);
    let index = NoteIndex::from_paths(vec![
        PathBuf::from("proj.md"),
        PathBuf::from("job.md"),
    ]);
    let reasons = project_block_reasons(&context, &index, today, &openness);
    assert!(reasons.is_empty());
}
