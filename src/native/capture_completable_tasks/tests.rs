//! Catalog unit tests for the `!` picker contract: today roles and
//! precedence, today-first ordering and ranking, guard sinking, the
//! ignored-link kinds, quoted locators, and draft selection.

use std::{collections::HashMap, fs, path::Path};

use super::{
    draft_selected, order_for_picker, CompletableGroup, TodayRole,
    ALREADY_SELECTED_DISABLED_REASON, RECURRING_DISABLED_REASON,
};
use crate::native::{
    capture_completable_tasks as completable, capture_dependency_tasks,
};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn write_settings(root: &Path) {
    write_file(
        &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
}

/// Fixture vault: every open status, an ID-less task, hidden and
/// recurring rows, closed history, a quoted note name, and a day file
/// with running, worked (two entries), queued, and noted links plus
/// one struck, one fenced, and one Depends-On link that must not
/// count.
fn fixture(root: &Path) -> std::path::PathBuf {
    write_settings(root);
    write_file(
        &root.join("sase.md"),
        concat!(
            "- [ ] #task Ship blog post [scheduled:: 2026-10-03] ^blog\n",
            "- [*] #task Fix deep bug ^deep-fix\n",
            "- [/] #task Draft outline ^outline\n",
            "- [ ] #task No id yet\n",
            "- [ ] #task Hidden chore #hide ^hidden-chore\n",
            "- [ ] #task Water plants \u{1f501} ^water\n",
            "- [?] #task Blocked bill ^bill\n",
            "- [x] #task Old fix ^old-fix\n",
            "- [-] #task Dropped idea ^dropped\n",
            "- [ ] #task Ignored everywhere ^ignored\n",
        ),
    );
    write_file(
        &root.join("cash.md"),
        concat!(
            "- [ ] #task Call the bank ^call-bank\n",
            "- [ ] #task Buy milk ^milk\n",
            "- [/] #task Cook dinner ^dinner\n",
            "- [*] #task Plan meals ^meals\n",
        ),
    );
    write_file(
        &root.join("Shopping List.md"),
        "- [ ] #task Buy eggs ^eggs\n",
    );
    let day_file = root.join("2026/20260930.md");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "- [[Shopping List#^eggs]]\n",
            "\n",
            "## Pomodoros\n",
            // Column-zero fence: the shared fenced-lines helper only
            // recognizes up to three leading spaces, so the fenced
            // link below must stay column-zero to exercise the fence
            // skip. Tab-indented fences are a shared limitation with
            // the ledger, not this catalog.
            "```\n",
            "- [[sase#^ignored]]\n",
            "```\n",
            "- [x] (0700-0730) \u{2014} MORNING\n",
            "\t- [[sase#^blog]]\n",
            "- [ ] (**09:20 - 09:50** [t:: 30m]) \u{2014} CAPTURE\n",
            "\t- [[sase#^deep-fix|deep]]\n",
            "\t- ![[sase#^outline]]\n",
            "\t- ~~[[sase#^ignored]]~~\n",
            "- [x] (0800-0830) \u{2014} PLAN\n",
            "\t- [[sase#^outline]]\n",
            "\t- [[cash#^call-bank]]\n",
            "\t- \u{26d3}\u{fe0f} **DEPENDS ON:** [[sase#^ignored]]\n",
            "- [ ] () \u{2014} QUEUED\n",
            "\t- ![[cash#^milk]]\n",
            "\t- ~~[[cash#^call-bank]]~~\n",
        ),
    );
    day_file
}

fn catalog(root: &Path, day_file: &Path) -> super::CompletableResult {
    let dep = capture_dependency_tasks::discover(root);
    completable::discover(root, &dep, day_file)
}

#[test]
fn open_tasks_list_with_locators_scheduled_and_suggestions() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());

    let result = catalog(root.path(), &day_file);
    // Closed and canceled history never lists.
    assert!(
        result
            .tasks
            .iter()
            .all(|task| task.block_id.as_deref() != Some("old-fix")),
        "done tasks are excluded"
    );
    assert!(
        result
            .tasks
            .iter()
            .all(|task| task.block_id.as_deref() != Some("dropped")),
        "canceled tasks are excluded"
    );
    let by_id: HashMap<String, _> = result
        .tasks
        .iter()
        .map(|task| (task.block_id.clone().unwrap_or_default(), task))
        .collect();
    // Quoted notes keep their display locator.
    assert_eq!(by_id["eggs"].locator, "Shopping List");
    assert_eq!(by_id["eggs"].note_path, "Shopping List.md");
    // The first strict scheduled date is exposed, never pulled.
    assert_eq!(
        by_id["blog"].scheduled.as_deref(),
        Some("2026-10-03"),
        "scheduled date"
    );
    assert_eq!(by_id["deep-fix"].scheduled, None);
    // ID-less rows carry suggestions for the Add block ID flow.
    let no_id = by_id[""];
    assert_eq!(no_id.block_id, None);
    assert!(
        !no_id.block_id_suggestions.is_empty(),
        "ID-less rows carry suggestions"
    );
    // Hidden and recurring facts ride along for sinking and guards.
    assert!(by_id["hidden-chore"].hidden);
    assert!(by_id["water"].recurring);
    assert!(!by_id["blog"].recurring);
}

#[test]
fn today_roles_precedence_sessions_and_pomodoro() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());

    let result = catalog(root.path(), &day_file);
    let by_id: HashMap<String, _> = result
        .tasks
        .iter()
        .map(|task| (task.block_id.clone().unwrap_or_default(), task))
        .collect();
    // Alias and embed links resolve like plain links.
    assert_eq!(
        by_id["deep-fix"].today.as_ref().map(|today| today.role),
        Some(TodayRole::Running)
    );
    // Running beats worked; both entries count as sessions.
    let outline = by_id["outline"].today.as_ref().expect("today row");
    assert_eq!(outline.role, TodayRole::Running);
    assert_eq!(outline.sessions, 2);
    let running = outline.pomodoro.as_ref().expect("running entry");
    assert_eq!(running.name.as_deref(), Some("CAPTURE"));
    assert_eq!(
        running.time_range.as_deref(),
        Some("0920-0950"),
        "entry time range"
    );
    assert_eq!(
        format!("{:?}", running.status),
        "Running",
        "running entry status"
    );
    // A struck copy in another entry adds no session.
    let bank = by_id["call-bank"].today.as_ref().expect("today row");
    assert_eq!(bank.role, TodayRole::Worked);
    assert_eq!(bank.sessions, 1, "struck copies add no session");
    assert_eq!(
        bank.pomodoro
            .as_ref()
            .and_then(|entry| entry.name.as_deref()),
        Some("PLAN"),
        "worked keeps its entry"
    );
    assert_eq!(
        by_id["milk"].today.as_ref().map(|today| today.role),
        Some(TodayRole::Queued)
    );
    let noted = by_id["eggs"].today.as_ref().expect("today row");
    assert_eq!(noted.role, TodayRole::Noted);
    assert!(noted.pomodoro.is_none(), "noted rows carry no entry");
    assert_eq!(noted.sessions, 0);
}

#[test]
fn struck_fenced_and_depends_on_links_never_place() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());

    let result = catalog(root.path(), &day_file);
    let ignored = result
        .tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some("ignored"))
        .expect("ignored task lists");
    assert_eq!(
        ignored.today, None,
        "struck, fenced, and Depends-On links never place"
    );
    assert_eq!(ignored.group, CompletableGroup::Open);
}

#[test]
fn empty_query_orders_today_first_then_status() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());
    let result = catalog(root.path(), &day_file);

    let ordered = order_for_picker(&result.tasks, "");
    let ids: Vec<&str> = ordered
        .iter()
        .map(|task| task.block_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        vec![
            // Today: running in ledger order, worked most-recent
            // first, queued, noted.
            "deep-fix",
            "outline",
            "call-bank",
            "blog",
            "milk",
            "eggs",
            // Then In Progress and Next by note path and line.
            "dinner",
            "meals",
            // Then open by note path and line, hidden and recurring
            // sunk within the section.
            "",
            "bill",
            "ignored",
            "hidden-chore",
            "water",
        ],
        "today-first canonical order"
    );
}

#[test]
fn nonempty_query_keeps_today_matches_on_top() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());
    let result = catalog(root.path(), &day_file);

    let ordered = order_for_picker(&result.tasks, "cash");
    let ids: Vec<&str> = ordered
        .iter()
        .map(|task| task.block_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        vec!["call-bank", "milk", "dinner", "meals"],
        "today matches first by score, then the rest"
    );
    // Every term must match: nothing in the vault mentions this.
    assert!(
        order_for_picker(&result.tasks, "cash zebra").is_empty(),
        "all terms must match"
    );
}

#[test]
fn hidden_and_recurring_sink_within_their_section() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());
    let result = catalog(root.path(), &day_file);

    let ordered = order_for_picker(&result.tasks, "");
    let open: Vec<&str> = ordered
        .iter()
        .filter(|task| task.group == CompletableGroup::Open)
        .map(|task| task.block_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        open.last(),
        Some(&"water"),
        "recurring rows go last: {open:?}"
    );
    let hidden_position = open
        .iter()
        .position(|id| *id == "hidden-chore")
        .expect("hidden");
    let water_position = open
        .iter()
        .position(|id| *id == "water")
        .expect("recurring");
    let bill_position =
        open.iter().position(|id| *id == "bill").expect("visible");
    assert!(
        bill_position < hidden_position && hidden_position < water_position,
        "visible before hidden before recurring: {open:?}"
    );
}

#[test]
fn draft_selected_marks_only_other_items() {
    let root = tempfile::tempdir().expect("temp vault");
    let _day_file = fixture(root.path());
    let dep = capture_dependency_tasks::discover(root.path());

    let draft = "!sase:deep-fix\n\n!cash:milk";
    let first = draft_selected(&dep, root.path(), draft, 0);
    assert_eq!(
        first,
        [("cash.md".to_string(), "milk".to_string())]
            .into_iter()
            .collect(),
        "cursor in the first item selects the second"
    );
    let second = draft_selected(&dep, root.path(), draft, draft.len());
    assert_eq!(
        second,
        [("sase.md".to_string(), "deep-fix".to_string())]
            .into_iter()
            .collect(),
        "cursor in the second item selects the first"
    );
    // Queries and prose never select.
    let query = draft_selected(&dep, root.path(), "!fix", 4);
    assert!(query.is_empty(), "partial tokens select nothing");
}

#[test]
fn missing_day_file_means_no_today_rows() {
    let root = tempfile::tempdir().expect("temp vault");
    let _day_file = fixture(root.path());

    let missing = root.path().join("2026/20990101.md");
    let result = catalog(root.path(), &missing);
    assert!(
        result.tasks.iter().all(|task| task.today.is_none()),
        "no day file means no today rows"
    );
    assert!(
        result.warnings.is_empty(),
        "no day file means no warning either"
    );
}

#[test]
fn bang_replacement_quotes_reserved_locators() {
    let root = tempfile::tempdir().expect("temp vault");
    let day_file = fixture(root.path());
    let dep = capture_dependency_tasks::discover(root.path());
    let _ = catalog(root.path(), &day_file);

    let quoted = capture_dependency_tasks::replacement_for_sigil(
        &dep.index,
        Path::new("Shopping List.md"),
        "eggs",
        b'!',
    );
    assert_eq!(quoted, "!\"Shopping List\":eggs");
    let plain = capture_dependency_tasks::replacement_for_sigil(
        &dep.index,
        Path::new("sase.md"),
        "deep-fix",
        b'!',
    );
    assert_eq!(plain, "!sase:deep-fix");
}

#[test]
fn guard_reasons_match_the_contract() {
    assert_eq!(
        RECURRING_DISABLED_REASON,
        "Recurring — complete it in Obsidian so Tasks writes the next occurrence"
    );
    assert_eq!(ALREADY_SELECTED_DISABLED_REASON, "Already in this draft");
}
