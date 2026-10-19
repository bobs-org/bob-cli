use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    load_vault_snapshot, plan_task_move, MoveRequest, MoveSelector, PlanError,
    VaultSnapshot,
};

fn snap(notes: &[(&str, &str)]) -> VaultSnapshot {
    VaultSnapshot::from_notes(
        notes.iter().map(|(path, contents)| {
            (PathBuf::from(path), (*contents).to_string())
        }),
    )
}

fn area(body: &str) -> String {
    format!("---\ntype: [[area]]\n---\n{body}")
}

fn project(status: &str, body: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: {status}\n---\n{body}")
}

fn quoted_project(status: &str, body: &str) -> String {
    format!("---\ntype: \"[[project]]\"\nstatus: {status}\n---\n{body}")
}

fn request(
    source: &str,
    dest: &str,
    selectors: Vec<MoveSelector>,
) -> MoveRequest {
    MoveRequest {
        source: source.to_string(),
        dest: dest.to_string(),
        selectors,
    }
}

fn plan(
    notes: &[(&str, &str)],
    source: &str,
    dest: &str,
    selectors: Vec<MoveSelector>,
) -> super::MovePlan {
    plan_task_move(&snap(notes), &request(source, dest, selectors))
        .unwrap_or_else(|error| panic!("plan failed: {error}"))
}

fn plan_err(
    notes: &[(&str, &str)],
    source: &str,
    dest: &str,
    selectors: Vec<MoveSelector>,
) -> PlanError {
    plan_task_move(&snap(notes), &request(source, dest, selectors))
        .expect_err("plan should fail")
}

fn post<'a>(plan: &'a super::MovePlan, path: &str) -> &'a str {
    plan.postimages
        .get(Path::new(path))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing postimage {path}"))
}

fn block(id: &str) -> MoveSelector {
    MoveSelector::Block(id.to_string())
}

fn contains(text: &str) -> MoveSelector {
    MoveSelector::Match(text.to_string())
}

#[test]
fn moves_each_open_checkbox_via_block_and_match_selectors() {
    let source = area(
        "
# Inbox

## Tasks

- [ ] #task Ready item [fresh:: 2026-10-01] [keeps:: 2] ^ready
- [*] #task Next item [priority:: high] ^next
- [/] #task Pending item [unblocked:: 2026-10-18] ^pending
- [?] #task Blocked item [scheduled:: 2026-11-01] ^blocked
",
    );
    let dest = area(
        "
# Dest

## Tasks

- [ ] #task Stay ^stay
",
    );
    let result = plan(
        &[("inbox.md", &source), ("dest.md", &dest)],
        "inbox",
        "dest",
        vec![
            block("blocked"),
            contains("Pending item"),
            block("ready"),
            contains("Next item"),
            block("ready"),
        ],
    );
    assert_eq!(result.moves.len(), 4);
    assert_eq!(
        result
            .moves
            .iter()
            .map(|moved| moved.title.as_str())
            .collect::<Vec<_>>(),
        ["Ready item", "Next item", "Pending item", "Blocked item"]
    );
    let dest_post = post(&result, "dest.md");
    let ready_at = dest_post.find("- [ ] #task Ready item").unwrap();
    let next_at = dest_post.find("- [*] #task Next item").unwrap();
    let pending_at = dest_post.find("- [/] #task Pending item").unwrap();
    let blocked_at = dest_post.find("- [?] #task Blocked item").unwrap();
    assert!(
        ready_at < next_at && next_at < pending_at && pending_at < blocked_at
    );
    assert!(dest_post.contains("[fresh:: 2026-10-01]"));
    assert!(dest_post.contains("[keeps:: 2]"));
    assert!(dest_post.contains("[priority:: high]"));
    assert!(dest_post.contains("[unblocked:: 2026-10-18]"));
    assert!(dest_post.contains("[scheduled:: 2026-11-01]"));
    let source_post = post(&result, "inbox.md");
    assert!(!source_post.contains("Ready item"));
    assert!(!source_post.contains("Next item"));
}

#[test]
fn match_selects_anchorless_task_without_minting_identity() {
    let source = area(
        "
## Tasks

- [ ] #task No anchor yet
",
    );
    let dest = area(
        "
## Tasks
",
    );
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![contains("No anchor yet")],
    );
    assert_eq!(result.moves[0].old_block_id, None);
    assert_eq!(result.moves[0].new_block_id, None);
    assert_eq!(result.moves[0].old_dependency_id, None);
    assert!(!post(&result, "dest.md").contains("[id::"));
    assert!(!post(&result, "dest.md").contains("^"));
}

#[test]
fn mixed_out_of_order_selectors_dedup_and_keep_source_order() {
    let source = area(
        "
## Tasks

- [ ] #task Alpha ^a
- [ ] #task Bravo ^b
- [ ] #task Charlie ^c
",
    );
    let dest = area("\n## Tasks\n");
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src.md",
        "dest.md",
        vec![block("c"), contains("Alpha"), block("a"), block("b")],
    );
    assert_eq!(
        result
            .moves
            .iter()
            .map(|moved| moved.old_block_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("a"), Some("b"), Some("c")]
    );
}

#[test]
fn zero_and_ambiguous_matches_list_candidates() {
    let source = area(
        "
## Tasks

- [ ] #task Alpha ^a
- [ ] #task Alpha copy ^b
",
    );
    let dest = area("\n## Tasks\n");
    let notes = [("src.md", source.as_str()), ("dest.md", dest.as_str())];
    match plan_err(&notes, "src", "dest", vec![contains("Nope")]) {
        PlanError::SelectorZero { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
            assert_eq!(candidates[0].line, 7);
            assert_eq!(candidates[0].title, "Alpha");
            assert_eq!(candidates[0].block_id.as_deref(), Some("a"));
        }
        other => panic!("expected zero match, got {other}"),
    }
    match plan_err(&notes, "src", "dest", vec![contains("Alpha")]) {
        PlanError::SelectorAmbiguous { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected ambiguous match, got {other}"),
    }
}

#[test]
fn refuses_closed_nested_prj_and_duplicate_source_anchors() {
    let source = area(
        "
## Tasks

- [x] #task Done ^done
- [ ] #task Parent ^parent
  - [ ] #task Nested ^nested
- [ ] #task Project ^prj
- [ ] #task Dup one ^dup
- [ ] #task Dup two ^dup
",
    );
    let dest = area("\n## Tasks\n");
    let notes = [("src.md", source.as_str()), ("dest.md", dest.as_str())];
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![block("done")]),
        PlanError::ClosedTask { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![block("nested")]),
        PlanError::NestedTask { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![block("prj")]),
        PlanError::ProjectTask { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![block("dup")]),
        PlanError::SelectorAmbiguous { .. }
    ));

    let duplicate_heading = area(
        "
## Tasks

- [ ] #task Unique title ^shared

Paragraph ^shared
",
    );
    let notes = [
        ("src.md", duplicate_heading.as_str()),
        ("dest.md", dest.as_str()),
    ];
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![contains("Unique title")]),
        PlanError::AmbiguousIdentity { .. }
    ));
}

#[test]
fn fenced_and_frontmatter_task_text_is_not_selectable() {
    let source = area(
        "---
type: [[area]]
taskish: \"- [ ] #task Hidden ^hid\"
---

## Tasks

```
- [ ] #task Fenced ^fenced
```

- [ ] #task Real ^real
",
    );
    let dest = area("\n## Tasks\n");
    let notes = [("src.md", source.as_str()), ("dest.md", dest.as_str())];
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![block("fenced")]),
        PlanError::NotATask { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "src", "dest", vec![contains("Hidden")]),
        PlanError::SelectorZero { .. }
    ));
    let result = plan(&notes, "src", "dest", vec![block("real")]);
    assert_eq!(result.moves[0].title, "Real");
}

#[test]
fn inserts_into_ungrouped_and_grouped_intake_and_creates_area_tasks() {
    let source = area(
        "
## Tasks

- [ ] #task Move me ^m
  child stays
",
    );
    let grouped = area(
        "
## Tasks

- [ ] #task Intake ^i

### Ready

- [ ] #task Grouped ^g
",
    );
    let ungrouped = area(
        "
## Tasks

- [ ] #task Existing ^e

## Notes

notes
",
    );
    let bare_area = area("# Bare\n");
    let notes = [
        ("src.md", source.as_str()),
        ("grouped.md", grouped.as_str()),
        ("ungrouped.md", ungrouped.as_str()),
        ("bare.md", bare_area.as_str()),
    ];
    let grouped_plan = plan(&notes, "src", "grouped", vec![block("m")]);
    let grouped_post = post(&grouped_plan, "grouped.md");
    let moved_at = grouped_post.find("- [ ] #task Move me ^m").unwrap();
    let ready_at = grouped_post.find("### Ready").unwrap();
    let grouped_at = grouped_post.find("- [ ] #task Grouped ^g").unwrap();
    assert!(moved_at < ready_at && ready_at < grouped_at);
    assert!(grouped_post.contains("  child stays"));

    let source = area(
        "
## Tasks

- [ ] #task Move me ^m
",
    );
    let notes = [
        ("src.md", source.as_str()),
        ("ungrouped.md", ungrouped.as_str()),
    ];
    let ungrouped_plan = plan(&notes, "src", "ungrouped", vec![block("m")]);
    let ungrouped_post = post(&ungrouped_plan, "ungrouped.md");
    let moved_at = ungrouped_post.find("- [ ] #task Move me ^m").unwrap();
    let notes_at = ungrouped_post.find("## Notes").unwrap();
    assert!(moved_at < notes_at);

    let source = area(
        "
## Tasks

- [ ] #task Move me ^m
",
    );
    let notes = [("src.md", source.as_str()), ("bare.md", bare_area.as_str())];
    let bare_plan = plan(&notes, "src", "bare", vec![block("m")]);
    let bare_post = post(&bare_plan, "bare.md");
    assert!(bare_post.contains("## Tasks"));
    assert!(bare_post.contains("- [ ] #task Move me ^m"));
}

#[test]
fn project_without_tasks_is_refused() {
    let source = area(
        "
## Tasks

- [ ] #task Move me ^m
",
    );
    let dest = project(
        "wip",
        "
- [ ] #task Criteria ^prj
",
    );
    let error = plan_err(
        &[("src.md", source.as_str()), ("prj.md", dest.as_str())],
        "src",
        "prj",
        vec![block("m")],
    );
    assert!(matches!(error, PlanError::ProjectWithoutTasks { .. }));
}

#[test]
fn collapses_source_removal_seams_and_preserves_crlf_and_eof() {
    let source = area(
        "## Tasks\r\n\r\n- [ ] #task Keep ^k\r\n\r\n- [ ] #task Move ^m\r\n\r\n- [ ] #task Tail ^t",
    );
    let dest = area("## Tasks\r\n");
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![block("m")],
    );
    let source_post = post(&result, "src.md");
    assert!(source_post.contains("\r\n"));
    assert!(!source_post.contains("\r\n\r\n\r\n"));
    assert!(source_post.contains("- [ ] #task Keep ^k"));
    assert!(source_post.contains("- [ ] #task Tail ^t"));
    assert!(!source_post.contains("Move ^m"));
    assert!(!post(&result, "dest.md").ends_with("\r\n\r\n\r\n"));
}

#[test]
fn repairs_canonical_ids_and_preserves_absent_ids() {
    let source = area(
        "
## Tasks

- [ ] #task Has id [id:: src__a] ^a
  - ⛓️ **DEPENDS ON:** [[#^b]]
- [ ] #task No id ^b
- [ ] #task Other [dependsOn:: src__a, src__b] ^c
",
    );
    let dest = area(
        "
## Tasks
",
    );
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![block("a"), block("b")],
    );
    let dest_post = post(&result, "dest.md");
    assert!(dest_post.contains("[id:: dest__a]"));
    assert!(!dest_post
        .lines()
        .any(|line| line.contains("No id") && line.contains("[id::")));
    let source_post = post(&result, "src.md");
    assert!(source_post.contains("[dependsOn:: dest__a, dest__b]"));
}

#[test]
fn refuses_non_canonical_id_fields() {
    let source = area(
        "
## Tasks

- [ ] #task Stale [id:: wrong] ^a
",
    );
    let dest = area("\n## Tasks\n");
    let error = plan_err(
        &[("src.md", source.as_str()), ("dest.md", dest.as_str())],
        "src",
        "dest",
        vec![block("a")],
    );
    match error {
        PlanError::NonCanonicalId {
            found, expected, ..
        } => {
            assert_eq!(found, "wrong");
            assert_eq!(expected, "src__a");
        }
        other => panic!("expected non-canonical id, got {other}"),
    }
}

#[test]
fn repairs_every_required_link_form_across_note_kinds() {
    let source = area(
        "
## Tasks

- [ ] #task Stay ^stay
- [ ] #task Move ^x
  - ⛓️ **DEPENDS ON:** [[#^stay]]
  - see [[#^x]] and [[src#^x|alias]] and ![[src#^x]]
  - md [label](src.md#^x)
",
    );
    let dest = area(
        "
## Tasks

- [ ] #task Dest stay ^x
  - [[#^x]] stays dest-local
",
    );
    let daily = "- [[src#^x]]\n";
    let reference = "---\ntype: [[ref]]\n---\nBack [[src#^x]]\n";
    let archive =
        "---\nparent: \"[[src]]\"\ntype: \"[[done]]\"\n---\n- [[src#^x]]\n";
    let result = plan(
        &[
            ("src.md", source.as_str()),
            ("dest.md", dest.as_str()),
            ("dailies/2026-10-19.md", daily),
            ("ref/paper.md", reference),
            ("done/src_done.md", archive),
        ],
        "src",
        "dest",
        vec![block("x")],
    );
    let dest_post = post(&result, "dest.md");
    assert!(dest_post.contains("[[src#^stay]]"));
    assert!(dest_post.contains("[[#^x-1]]"));
    assert!(dest_post.contains("[[dest#^x-1|alias]]"));
    assert!(dest_post.contains("![[dest#^x-1]]"));
    assert!(dest_post.contains("[label](dest.md#^x-1)"));
    assert!(dest_post.contains("[[#^x]] stays dest-local"));
    assert!(dest_post.contains("- [ ] #task Dest stay ^x"));
    assert!(dest_post.contains("- [ ] #task Move ^x-1"));
    let source_post = post(&result, "src.md");
    assert!(source_post.contains("- [ ] #task Stay ^stay"));
    assert!(!source_post.contains("- [ ] #task Move"));
    assert!(post(&result, "dailies/2026-10-19.md").contains("[[dest#^x-1]]"));
    assert!(post(&result, "ref/paper.md").contains("[[dest#^x-1]]"));
    assert!(post(&result, "done/src_done.md").contains("[[dest#^x-1]]"));
}

#[test]
fn pathless_stayed_and_co_moved_links_and_unresolved_links() {
    let source = area(
        "
## Tasks

- [ ] #task Stay ^stay
- [ ] #task Move ^m
  - ⛓️ **DEPENDS ON:** [[#^stay]] • [[#^m]] • [[#^ghost]]
",
    );
    let dest = area("\n## Tasks\n");
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![block("m")],
    );
    let dest_post = post(&result, "dest.md");
    assert!(dest_post.contains("[[src#^stay]]"));
    assert!(dest_post.contains("[[#^m]]"));
    assert!(dest_post.contains("[[#^ghost]]"));
}

#[test]
fn collision_chain_renames_incoming_x_without_stealing_x_minus_one() {
    let source = area(
        "
## Tasks

- [ ] #task Incoming x [id:: src__x] ^x
  - child ^kid
- [ ] #task Incoming x-1 [id:: src__x-1] ^x-1
",
    );
    let dest = area(
        "
## Tasks

- [ ] #task Existing x ^x
",
    );
    let elsewhere = "See [[src#^x]] and [[src#^kid]] and [[src#^x-1]]\n";
    let result = plan(
        &[
            ("src.md", source.as_str()),
            ("dest.md", dest.as_str()),
            ("links.md", elsewhere),
        ],
        "src",
        "dest",
        vec![block("x"), block("x-1")],
    );
    assert_eq!(result.renamed_block_ids.len(), 1);
    assert_eq!(result.renamed_block_ids[0].old_block_id, "x");
    assert_eq!(result.renamed_block_ids[0].new_block_id, "x-2");
    let dest_post = post(&result, "dest.md");
    assert!(dest_post.contains("- [ ] #task Existing x ^x"));
    assert!(dest_post.contains("- [ ] #task Incoming x [id:: dest__x-2] ^x-2"));
    assert!(
        dest_post.contains("- [ ] #task Incoming x-1 [id:: dest__x-1] ^x-1")
    );
    assert!(dest_post.contains("  - child ^kid"));
    let links = post(&result, "links.md");
    assert!(links.contains("[[dest#^x-2]]"));
    assert!(links.contains("[[dest#^kid]]"));
    assert!(links.contains("[[dest#^x-1]]"));
}

#[test]
fn nested_paths_encode_dependency_ids_and_leave_unresolved_links() {
    let source = area(
        "
## Tasks

- [ ] #task Nested [id:: projects__alpha__n] ^n
",
    );
    let dest = area("\n## Tasks\n");
    let dangling = "[[missing#^n]] and [[projects/alpha#^n]]\n";
    let result = plan(
        &[
            ("projects/alpha.md", source.as_str()),
            ("projects/beta.md", dest.as_str()),
            ("dangle.md", dangling),
        ],
        "projects/alpha",
        "projects/beta",
        vec![block("n")],
    );
    assert_eq!(
        result.moves[0].new_dependency_id.as_deref(),
        Some("projects__beta__n")
    );
    assert!(
        post(&result, "projects/beta.md").contains("[id:: projects__beta__n]")
    );
    assert!(post(&result, "dangle.md").contains("[[missing#^n]]"));
    assert!(post(&result, "dangle.md").contains("[[projects/beta#^n]]"));
}

#[test]
fn dest_prj_dependency_is_refused_and_blocked_dest_accepts_only_blocked_tasks()
{
    let source = area(
        "
## Tasks

- [ ] #task Ready ^r
  - ⛓️ **DEPENDS ON:** [[prj#^prj]]
- [?] #task Already blocked ^b
- [*] #task Next ^n
",
    );
    let blocked_prj = quoted_project(
        "wip",
        "
- [?] #task Criteria ^prj

## Tasks
",
    );
    let tagged_prj = project(
        "waiting",
        "
- [ ] #task Criteria #prj_is_blocked ^prj

## Tasks
",
    );
    let waiting_unblocked = project(
        "waiting",
        "
- [ ] #task Criteria ^prj

## Tasks
",
    );
    let notes_blocked = [
        ("src.md", source.as_str()),
        ("prj.md", blocked_prj.as_str()),
    ];
    assert!(matches!(
        plan_err(&notes_blocked, "src", "prj", vec![block("r")]),
        PlanError::DependsOnDestPrj { .. }
    ));
    assert!(matches!(
        plan_err(&notes_blocked, "src", "prj", vec![block("n")]),
        PlanError::DestBlocked { .. }
    ));
    let accepted = plan(&notes_blocked, "src", "prj", vec![block("b")]);
    assert!(accepted.needs_reconcile);
    assert!(
        post(&accepted, "prj.md").contains("- [?] #task Already blocked ^b")
    );

    let notes_tagged =
        [("src.md", source.as_str()), ("prj.md", tagged_prj.as_str())];
    assert!(matches!(
        plan_err(&notes_tagged, "src", "prj", vec![block("n")]),
        PlanError::DestBlocked { .. }
    ));

    let notes_waiting = [
        ("src.md", source.as_str()),
        ("prj.md", waiting_unblocked.as_str()),
    ];
    let waiting_plan = plan(&notes_waiting, "src", "prj", vec![block("n")]);
    assert!(!waiting_plan.needs_reconcile);
}

#[test]
fn carried_prj_is_blocked_tag_sets_reconcile_hint() {
    let source = area(
        "
## Tasks

- [?] #task Carried #prj_is_blocked ^c
",
    );
    let dest = area("\n## Tasks\n");
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![block("c")],
    );
    assert!(result.needs_reconcile);
    assert!(post(&result, "dest.md").contains("#prj_is_blocked"));
}

#[test]
fn refuses_done_paths_same_note_and_missing_or_ambiguous_stems() {
    let source = area("\n## Tasks\n\n- [ ] #task A ^a\n");
    let dest = area("\n## Tasks\n");
    let notes = [
        ("src.md", source.as_str()),
        ("dest.md", dest.as_str()),
        ("done/src_done.md", "archive\n"),
        ("folder/dup.md", dest.as_str()),
        ("other/dup.md", dest.as_str()),
    ];
    assert!(matches!(
        plan_err(&notes, "done/src_done", "dest", vec![block("a")]),
        PlanError::ArchivePath { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "src", "src", vec![block("a")]),
        PlanError::SameSourceAndDest { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "missing", "dest", vec![block("a")]),
        PlanError::MissingNote { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "dup", "src", vec![block("a")]),
        PlanError::AmbiguousNote { .. }
    ));
    assert!(matches!(
        plan_err(&notes, "../escape", "dest", vec![block("a")]),
        PlanError::InvalidPath { .. }
    ));
}

#[test]
fn dest_must_be_area_or_wip_waiting_project() {
    let source = area("\n## Tasks\n\n- [ ] #task A ^a\n");
    let done_prj = project(
        "done",
        "
- [x] #task Criteria ^prj

## Tasks
",
    );
    let untyped = "# Note\n\n## Tasks\n";
    assert!(matches!(
        plan_err(
            &[("src.md", source.as_str()), ("prj.md", done_prj.as_str())],
            "src",
            "prj",
            vec![block("a")],
        ),
        PlanError::DestKind { .. }
    ));
    assert!(matches!(
        plan_err(
            &[("src.md", source.as_str()), ("note.md", untyped)],
            "src",
            "note",
            vec![block("a")],
        ),
        PlanError::DestKind { .. }
    ));
}

#[test]
fn display_errors_include_candidates_and_ctrl_shift_m_guidance() {
    let source = area(
        "
## Tasks

- [*] #task Next ^n
",
    );
    let dest = project(
        "wip",
        "
- [?] #task Criteria ^prj

## Tasks
",
    );
    let error = plan_err(
        &[("src.md", source.as_str()), ("prj.md", dest.as_str())],
        "src",
        "prj",
        vec![block("n")],
    );
    let text = error.to_string();
    assert!(text.contains("Ctrl+Shift+M"));
    assert!(text.contains("Next"));
}

#[test]
fn load_snapshot_includes_done_skips_excluded_dirs_and_fails_on_non_utf8() {
    let vault = TempDir::new("task-move-snapshot");
    write_file(&vault.path().join("ok.md"), "# hi\n");
    write_file(&vault.path().join("done/old.md"), "archive\n");
    write_file(&vault.path().join(".obsidian/skip.md"), "nope\n");
    write_file(&vault.path().join("_templates/skip.md"), "nope\n");
    let snapshot = load_vault_snapshot(vault.path()).expect("load");
    assert!(snapshot.notes.contains_key(Path::new("ok.md")));
    assert!(snapshot.notes.contains_key(Path::new("done/old.md")));
    assert!(!snapshot.notes.contains_key(Path::new(".obsidian/skip.md")));

    let bad = TempDir::new("task-move-non-utf8");
    fs::write(bad.path().join("bad.md"), [0xff, 0xfe]).unwrap();
    let error = load_vault_snapshot(bad.path()).expect_err("non-utf8");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn no_selectors_is_an_error() {
    let source = area("\n## Tasks\n\n- [ ] #task A ^a\n");
    let dest = area("\n## Tasks\n");
    assert!(matches!(
        plan_err(
            &[("src.md", source.as_str()), ("dest.md", dest.as_str())],
            "src",
            "dest",
            vec![],
        ),
        PlanError::NoSelectors
    ));
}

#[test]
fn source_pathless_backlink_to_moved_task_gains_destination_path() {
    let source = area(
        "
## Tasks

- [ ] #task Stay
  - see [[#^m]]
- [ ] #task Move ^m
",
    );
    let dest = area("\n## Tasks\n");
    let result = plan(
        &[("src.md", &source), ("dest.md", &dest)],
        "src",
        "dest",
        vec![block("m")],
    );
    assert!(post(&result, "src.md").contains("[[dest#^m]]"));
}
