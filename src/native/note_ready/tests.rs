//! R1–R14 conformance vectors, shared verbatim with the
//! `bob-ledger-tools` JavaScript mirror.

use super::*;

fn note(
    path: &str,
    kind: &str,
    status: &str,
    cap_raw: Option<&str>,
) -> NoteInput {
    NoteInput {
        path: path.to_string(),
        name: path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .strip_suffix(".md")
            .unwrap_or(path)
            .to_string(),
        kind: kind.to_string(),
        status: status.to_string(),
        is_area: kind == "area",
        is_terminal: matches!(status, "done" | "canceled" | "cancelled"),
        parent: None,
        ready_cap_raw: cap_raw.map(str::to_string),
    }
}

fn lane_rows(
    path: &str,
    count: u32,
    bucket: Option<&str>,
    recurring: bool,
    block_id: Option<&str>,
) -> Vec<LaneRow> {
    (0..count)
        .map(|_| LaneRow {
            path: path.to_string(),
            is_recurring: recurring,
            block_id: block_id.map(str::to_string),
            bucket: bucket.map(str::to_string),
        })
        .collect()
}

fn crowded_note(count: u32) -> (Vec<NoteInput>, Vec<LaneRow>) {
    (
        vec![note("sase.md", "project", "wip", None)],
        lane_rows("sase.md", count, None, false, None),
    )
}

#[test]
fn r1_six_tasks_is_crowded_over_by_one() {
    let (notes, rows) = crowded_note(6);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    let entry = &report.notes[0];
    assert_eq!(entry.count, 6);
    assert_eq!(entry.state, NoteState::Crowded);
    assert_eq!(entry.over_by, 1);
    assert_eq!(report.totals.excess, 1);
    assert_eq!(report.totals.crowded, 1);
}

#[test]
fn r2_five_tasks_is_full_without_lint() {
    let (notes, rows) = crowded_note(5);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(report.notes[0].state, NoteState::Full);
    assert!(report.lints.is_empty());
}

#[test]
fn r3_lane_exclusions_never_count() {
    // Lane exclusions live in READY_QUERY; the pure evaluator only
    // sees lane rows. A note with no lane rows counts zero.
    let notes = vec![note("sase.md", "project", "wip", None)];
    let report = evaluate(&notes, &[], 5, CapSource::Default, true);
    assert_eq!(report.notes[0].count, 0);
    assert_eq!(report.notes[0].state, NoteState::Empty);
}

#[test]
fn r4_make_up_splits_new_rotten_ready() {
    let notes = vec![note("sase.md", "project", "wip", None)];
    let mut rows = Vec::new();
    rows.extend(lane_rows("sase.md", 1, None, false, None));
    // Unstamped NEW: bucket "new".
    rows.push(LaneRow {
        path: "sase.md".to_string(),
        is_recurring: false,
        block_id: None,
        bucket: Some("new".to_string()),
    });
    // Stamped 8 days ago: bucket "rotten".
    rows.push(LaneRow {
        path: "sase.md".to_string(),
        is_recurring: false,
        block_id: None,
        bucket: Some("rotten".to_string()),
    });
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    let entry = &report.notes[0];
    assert_eq!(entry.count, 3);
    assert_eq!(
        entry.make_up,
        Some(MakeUp {
            ready: 1,
            new: 1,
            rotten: 1
        })
    );
}

#[test]
fn r5_recurring_excluded_and_counted() {
    let notes = vec![note("sase.md", "project", "wip", None)];
    let rows = lane_rows("sase.md", 2, None, true, None);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    let entry = &report.notes[0];
    assert_eq!(entry.count, 0);
    assert_eq!(entry.recurring, 2);
    assert_eq!(entry.state, NoteState::Empty);
}

#[test]
fn r6_prj_rows_never_count() {
    let notes = vec![note("sase.md", "project", "wip", None)];
    let rows = lane_rows("sase.md", 1, None, false, Some("prj"));
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(report.notes[0].count, 0);
}

#[test]
fn r8_ready_cap_forms() {
    // cap 8 (source note) / exempt / exempt / exempt / lint then
    // default / lint then default / lint then default.
    let notes = vec![
        note("a.md", "project", "wip", Some("8")),
        note("b.md", "project", "wip", Some("off")),
        note("c.md", "project", "wip", Some("OFF")),
        note("d.md", "project", "wip", Some("false")),
        note("e.md", "project", "wip", Some("0")),
        note("f.md", "project", "wip", Some("lots")),
        note("g.md", "project", "wip", Some("1000")),
    ];
    let report = evaluate(&notes, &[], 5, CapSource::Default, true);
    let by_path = |path: &str| {
        report
            .notes
            .iter()
            .find(|n| n.path == path)
            .unwrap()
            .clone()
    };
    assert_eq!(by_path("a.md").cap, Some(8));
    assert_eq!(by_path("a.md").cap_source, CapSource::Note);
    for path in ["b.md", "c.md", "d.md"] {
        assert_eq!(by_path(path).state, NoteState::Exempt);
        assert_eq!(by_path(path).cap, None);
    }
    for path in ["e.md", "f.md", "g.md"] {
        assert_eq!(by_path(path).cap, Some(5));
    }
    assert_eq!(report.lints.len(), 3);
    assert!(report
        .lints
        .iter()
        .all(|lint| lint.code == LINT_NOTE_READY_CAP_INVALID));
}

#[test]
fn r9_nested_child_tasks_each_count() {
    let notes = vec![note("sase.md", "project", "wip", None)];
    let rows = lane_rows("sase.md", 3, None, false, None);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(report.notes[0].count, 3);
}

#[test]
fn r10_terminal_projects_are_not_capped() {
    for status in ["done", "cancelled"] {
        let notes = vec![note("old.md", "project", status, None)];
        let rows = lane_rows("old.md", 2, None, false, None);
        let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
        assert!(report.notes.is_empty(), "terminal has no entry");
        assert_eq!(report.lints.len(), 1);
        assert_eq!(report.lints[0].code, LINT_NOTE_READY_IN_TERMINAL_PROJECT);
    }
}

#[test]
fn r11_all_type_forms_are_eligible() {
    // Eligibility itself is the classifier; here every kind parses
    // to an entry regardless of form.
    let notes = vec![
        note("a.md", "project", "wip", None),
        note("b.md", "area", "wip", None),
    ];
    let rows = lane_rows("a.md", 1, None, false, None);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(report.notes.len(), 2);
    assert_eq!(report.totals.projects, 1);
    assert_eq!(report.totals.areas, 1);
}

#[test]
fn r12_stamping_moves_new_to_ready_without_changing_count() {
    let notes = vec![note("sase.md", "project", "wip", None)];
    let before = vec![
        LaneRow {
            path: "sase.md".to_string(),
            is_recurring: false,
            block_id: None,
            bucket: Some("new".to_string()),
        },
        LaneRow {
            path: "sase.md".to_string(),
            is_recurring: false,
            block_id: None,
            bucket: None,
        },
        LaneRow {
            path: "sase.md".to_string(),
            is_recurring: false,
            block_id: None,
            bucket: None,
        },
        LaneRow {
            path: "sase.md".to_string(),
            is_recurring: false,
            block_id: None,
            bucket: None,
        },
        LaneRow {
            path: "sase.md".to_string(),
            is_recurring: false,
            block_id: None,
            bucket: None,
        },
    ];
    let report = evaluate(&notes, &before, 5, CapSource::Default, true);
    assert_eq!(report.notes[0].count, 5);
    assert_eq!(report.notes[0].state, NoteState::Full);
    let mut after = before.clone();
    after[0].bucket = None;
    let moved = evaluate(&notes, &after, 5, CapSource::Default, true);
    assert_eq!(moved.notes[0].count, 5);
    assert_eq!(moved.notes[0].state, NoteState::Full);
    assert_eq!(moved.notes[0].make_up.unwrap().new, 0);
    assert_eq!(moved.notes[0].make_up.unwrap().ready, 5);
}

#[test]
fn r13_today_tasks_count_as_ready() {
    // Today is not a filter: a Today row has bucket None and counts.
    let notes = vec![note("sase.md", "project", "wip", None)];
    let rows = lane_rows("sase.md", 1, None, false, None);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(report.notes[0].count, 1);
}

#[test]
fn r14_config_cap_with_note_override() {
    let notes = vec![
        note("a.md", "project", "wip", None),
        note("b.md", "project", "wip", Some("8")),
    ];
    let report = evaluate(&notes, &[], 3, CapSource::Config, true);
    let by_path = |path: &str| {
        report
            .notes
            .iter()
            .find(|n| n.path == path)
            .unwrap()
            .clone()
    };
    assert_eq!(by_path("a.md").cap, Some(3));
    assert_eq!(by_path("a.md").cap_source, CapSource::Config);
    assert_eq!(by_path("b.md").cap, Some(8));
    assert_eq!(by_path("b.md").cap_source, CapSource::Note);
}

#[test]
fn ordering_is_crowded_full_room_empty_exempt() {
    let notes = vec![
        note("exempt.md", "project", "wip", Some("off")),
        note("empty.md", "project", "wip", None),
        note("room.md", "project", "wip", None),
        note("full.md", "project", "wip", None),
        note("crowded.md", "project", "wip", None),
    ];
    let mut rows = Vec::new();
    rows.extend(lane_rows("room.md", 2, None, false, None));
    rows.extend(lane_rows("full.md", 5, None, false, None));
    rows.extend(lane_rows("crowded.md", 7, None, false, None));
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    let order: Vec<&str> =
        report.notes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(order, vec!["crowded", "full", "room", "empty", "exempt"]);
}

#[test]
fn lints_emit_once_per_note() {
    let notes = vec![note("sase.md", "project", "wip", Some("lots"))];
    let rows = lane_rows("sase.md", 6, None, false, None);
    let report = evaluate(&notes, &rows, 5, CapSource::Default, true);
    assert_eq!(
        report
            .lints
            .iter()
            .filter(|l| l.code == LINT_NOTE_READY_CAP_INVALID)
            .count(),
        1
    );
}

// --- Temp-vault scan tests ---

fn write_blocked_settings(vault: &std::path::Path) {
    let data = r##"{
  "globalFilter": "#task",
  "taskFormat": "dataview",
  "statusSettings": {
    "coreStatuses": [
      {"symbol":" ","name":"Todo","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
      {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
    ],
    "customStatuses": [
      {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
      {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
      {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
      {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"},
      {"symbol":"~","name":"Reference","nextStatusSymbol":" ","availableAsCommand":false,"type":"NON_TASK"}
    ]
  }
}
"##;
    let dir = vault.join(".obsidian/plugins/obsidian-tasks-plugin");
    std::fs::create_dir_all(&dir).expect("settings dir");
    std::fs::write(dir.join("data.json"), data).expect("settings");
}

fn temp_vault(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("{prefix}-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp vault");
    dir
}

fn write_note(vault: &std::path::Path, rel: &str, contents: &str) {
    let path = vault.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("note parent");
    }
    std::fs::write(&path, contents).expect("note");
}

fn scan_fixture(vault: &std::path::Path) -> super::scan::ScanReport {
    // No global env mutation: the fixtures use dates relative to
    // today, and the default per-note cap (5) holds whether the
    // config file is absent or lacks the key.
    super::scan::scan_note_ready(vault).expect("scan fixture vault")
}

fn tomorrow_string() -> String {
    let today = crate::native::env::current_datetime().date();
    let tomorrow = today
        .checked_add_days(chrono::Days::new(1))
        .unwrap_or(today);
    tomorrow.format("%Y-%m-%d").to_string()
}

#[test]
fn scan_excludes_r3_and_r7_paths() {
    let vault = temp_vault("bob-cli-note-ready-r3r7");
    write_blocked_settings(&vault);
    // Eligible note with one counted task plus every R3 exclusion.
    let future = tomorrow_string();
    let body = format!(
        "---\ntype: \"[[project]]\"\n---\n\
- [ ] #task Count me\n\
- [ ] #task Hidden #hide\n\
- [ ] #task Hidden upper #Hide\n\
- [ ] #task Hidden sub #hide/x\n\
- [ ] #task Future [scheduled:: {future}]\n\
- [?] #task Waiting\n\
- [*] #task Next\n\
- [/] #task Progress\n\
- [x] #task Done\n\
- [-] #task Canceled\n"
    );
    write_note(&vault, "sase.md", &body);
    // R7: daily, untyped, template, done, and dash rows are absent.
    write_note(&vault, "2026/20261001.md", "- [ ] #task Daily thing\n");
    write_note(&vault, "plain.md", "- [ ] #task Plain\n");
    write_note(
        &vault,
        "_templates/tpl.md",
        "---\ntype: \"[[project]]\"\n---\n- [ ] #task Tpl\n",
    );
    write_note(
        &vault,
        "done/old.md",
        "---\ntype: \"[[project]]\"\n---\n- [ ] #task Old\n",
    );
    write_note(&vault, "dash.md", "- [ ] #task Dash\n");
    let scanned = scan_fixture(&vault);
    let entry = scanned
        .report
        .notes
        .iter()
        .find(|n| n.path == "sase.md")
        .expect("sase entry");
    assert_eq!(entry.count, 1, "only the plain task counts");
    assert!(
        scanned.report.notes.iter().all(|n| n.path != "dash.md"),
        "dash never lists"
    );
    let _ = std::fs::remove_dir_all(&vault);
}

#[test]
fn scan_covers_r11_r8_terminal_and_recurring() {
    let vault = temp_vault("bob-cli-note-ready-scan");
    write_blocked_settings(&vault);
    write_note(
        &vault,
        "flow.md",
        "---\ntype: [\"[[project]]\"]\nready_cap: 8\n---\n\
- [ ] #task One\n\
- [ ] #task Two\n",
    );
    write_note(
        &vault,
        "block.md",
        "---\ntype:\n  - \"[[area]]\"\nready_cap: off\n---\n\
- [ ] #task Many one\n\
- [ ] #task Many two\n",
    );
    write_note(
        &vault,
        "term.md",
        "---\ntype: \"[[project]]\"\nstatus: done\n---\n\
- [ ] #task Lingering one\n\
- [ ] #task Lingering two\n",
    );
    write_note(
        &vault,
        "recur.md",
        "---\ntype: \"[[project]]\"\n---\n\
- [ ] #task Weekly [repeat:: every week]\n\
- [ ] #task Plain\n",
    );
    let scanned = scan_fixture(&vault);
    let by_path = |path: &str| {
        scanned
            .report
            .notes
            .iter()
            .find(|n| n.path == path)
            .unwrap()
            .clone()
    };
    assert_eq!(by_path("flow.md").cap, Some(8));
    assert_eq!(by_path("block.md").state, NoteState::Exempt);
    assert!(
        scanned.report.notes.iter().all(|n| n.path != "term.md"),
        "terminal not capped"
    );
    assert!(
        scanned
            .report
            .lints
            .iter()
            .any(|l| l.code == LINT_NOTE_READY_IN_TERMINAL_PROJECT
                && l.path == "term.md"),
        "terminal lint"
    );
    assert_eq!(by_path("recur.md").count, 1);
    assert_eq!(by_path("recur.md").recurring, 1);
    let _ = std::fs::remove_dir_all(&vault);
}
