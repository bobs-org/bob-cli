//! Scoped ledger-retirement tests: strike, moves, placeholders,
//! mixed bullets, untouched elsewhere-done links, dedupe, and CRLF.
use super::super::super::task_status_hooks::RawReference;
use super::super::retirement::{retire_completed_links, LinkStatus};
use super::*;

fn done(path: &str) -> LinkStatus {
    LinkStatus::Done {
        path: PathBuf::from(path),
    }
}

fn live(path: &str, status: char) -> LinkStatus {
    LinkStatus::Live {
        path: PathBuf::from(path),
        status,
    }
}

fn reference(target: &str, block_id: &str) -> RawReference {
    RawReference {
        target: target.to_string(),
        block_id: block_id.to_string(),
    }
}

fn completed(pairs: &[(&str, &str)]) -> BTreeSet<(PathBuf, String)> {
    pairs
        .iter()
        .map(|(path, block_id)| (PathBuf::from(path), block_id.to_string()))
        .collect()
}

fn statuses(
    pairs: &[(&str, &str, LinkStatus)],
) -> BTreeMap<RawReference, LinkStatus> {
    pairs
        .iter()
        .map(|(target, block_id, status)| {
            (reference(target, block_id), status.clone())
        })
        .collect()
}

#[test]
fn strike_in_place_under_running_entry() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] CAPTURE (0900-0930)\n",
        "  - [[sase#^done]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[("sase", "done", done("sase.md"))]),
        &test_settings(),
    );
    assert!(retirement.changed);
    assert_eq!(retirement.struck, 1);
    assert!(retirement.moved.is_empty());
    assert!(retirement.deduplicated.is_empty());
    assert!(retirement.removed_placeholders.is_empty());
    assert!(retirement.text.contains("  - ~~[[sase#^done]]~~\n"));
}

#[test]
fn move_from_placeholder_to_running_removes_emptied_placeholder() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] CAPTURE (0900-0930)\n",
        "  - [[sase#^other]]\n",
        "- [ ] SASE\n",
        "  - [[sase#^done]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[
            ("sase", "other", live("sase.md", ' ')),
            ("sase", "done", done("sase.md")),
        ]),
        &test_settings(),
    );
    assert!(retirement.changed);
    assert_eq!(retirement.struck, 1);
    assert_eq!(retirement.moved.len(), 1);
    assert_eq!(retirement.moved[0].block_id, "done");
    assert!(retirement.moved[0].source_context.contains("SASE"));
    assert!(retirement.moved[0].destination_context.contains("CAPTURE"));
    assert_eq!(retirement.removed_placeholders.len(), 1);
    assert!(retirement.removed_placeholders[0].line.contains("SASE"));
    assert!(!retirement.text.contains("- [ ] SASE"));
    assert!(retirement.text.contains("  - ~~[[sase#^done]]~~\n"));
}

#[test]
fn move_to_last_completed_entry_when_nothing_runs() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [x] Morning (0800-0830)\n",
        "  - ~~[[sase#^old]]~~\n",
        "- [ ] SASE\n",
        "  - [[sase#^done]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[("sase", "done", done("sase.md"))]),
        &test_settings(),
    );
    assert!(retirement.changed);
    assert_eq!(retirement.moved.len(), 1);
    assert!(retirement.moved[0].destination_context.contains("Morning"));
    assert!(!retirement.moved[0].destination_open);
    assert!(retirement.text.contains("🍅 ~~[[sase#^done]]~~"));
}

#[test]
fn mixed_bullet_with_live_second_link_does_not_move() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [x] Morning (0800-0830)\n",
        "  - ~~[[sase#^old]]~~\n",
        "- [ ] SASE\n",
        "  - [[sase#^done]] and [[sase#^live]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[
            ("sase", "done", done("sase.md")),
            ("sase", "live", live("sase.md", ' ')),
        ]),
        &test_settings(),
    );
    assert_eq!(retirement.struck, 1);
    assert!(retirement.moved.is_empty());
    assert!(retirement
        .text
        .contains("  - ~~[[sase#^done]]~~ and [[sase#^live]]\n"));
}

#[test]
fn unrelated_already_done_link_is_left_untouched() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] CAPTURE (0900-0930)\n",
        "  - [[sase#^done]]\n",
        "  - [[sase#^stale]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[("sase", "done", done("sase.md"))]),
        &test_settings(),
    );
    assert_eq!(retirement.struck, 1);
    assert!(retirement.text.contains("  - [[sase#^stale]]\n"));
}

#[test]
fn dedupe_drops_carried_copy_when_destination_already_links_task() {
    let day = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] CAPTURE (0900-0930)\n",
        "  - [[sase#^done]]\n",
        "- [ ] SASE\n",
        "  - [[sase#^done]]\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[("sase", "done", done("sase.md"))]),
        &test_settings(),
    );
    assert_eq!(retirement.struck, 1);
    assert!(retirement.moved.is_empty());
    assert_eq!(retirement.deduplicated.len(), 1);
    assert_eq!(retirement.deduplicated[0].block_id, "done");
    assert_eq!(retirement.removed_placeholders.len(), 1);
    assert_eq!(
        retirement.text,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] CAPTURE (0900-0930)\n",
            "  - ~~[[sase#^done]]~~\n",
        )
    );
}

#[test]
fn reconcile_without_flag_moves_instead_of_deduping() {
    use super::super::super::pomodoro as native_pomodoro;
    use super::super::super::task_status_hooks::{
        logical_lines, plan_structural_changes, scan_pomodoros,
        ResolvedReference,
    };
    let contents = concat!(
        "## Pomodoros\n",
        "\n",
        "- [ ] CAPTURE (0900-0930)\n",
        "  - [[sase#^done]]\n",
        "- [ ] SASE\n",
        "  - [[sase#^done]]\n",
    );
    let lines = logical_lines(contents);
    let section =
        native_pomodoro::pomodoros_section_range(&lines).expect("section");
    let model = scan_pomodoros(&lines, section);
    let resolved = BTreeMap::from([(
        reference("sase", "done"),
        ResolvedReference {
            path: PathBuf::from("sase.md"),
            statuses: vec!['x'],
        },
    )]);
    let settings = test_settings();
    let without = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        false,
    );
    assert_eq!(without.moved.len(), 1);
    assert!(without.deduplicated.is_empty());
    let with = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        true,
    );
    assert!(with.moved.is_empty());
    assert_eq!(with.deduplicated.len(), 1);
}

#[test]
fn retirement_preserves_crlf() {
    let day = concat!(
        "## Pomodoros\r\n",
        "\r\n",
        "- [ ] CAPTURE (0900-0930)\r\n",
        "  - [[sase#^done]]\r\n",
    );
    let retirement = retire_completed_links(
        day,
        &completed(&[("sase.md", "done")]),
        &statuses(&[("sase", "done", done("sase.md"))]),
        &test_settings(),
    );
    assert!(retirement.changed);
    assert!(retirement.text.contains("  - ~~[[sase#^done]]~~\r\n"));
}
