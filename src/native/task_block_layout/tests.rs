use std::path::Path;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::*;
use crate::native::note_tasks;

fn settings() -> NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn source_of(case: &Value) -> (String, &'static str) {
    let lines: Vec<&str> = case["source_lines"]
        .as_array()
        .expect("source_lines")
        .iter()
        .map(|line| line.as_str().expect("line string"))
        .collect();
    let ending = if case["eol"].as_str() == Some("crlf") {
        "\r\n"
    } else {
        "\n"
    };
    let mut source = lines.join(ending);
    source.push_str(ending);
    (source, ending)
}

fn lines_of(contents: &str) -> Vec<String> {
    contents
        .strip_suffix('\n')
        .unwrap_or(contents)
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

fn task_at(contents: &str, case: &Value) -> NoteTask {
    let line = case["task"].as_u64().expect("task line") as usize - 1;
    find_task(contents, line, &settings()).expect("task at line")
}

fn slot_json(slot: &Slot) -> Value {
    json!({"line": slot.line_index + 1, "indent": slot.indent})
}

fn run_slot(case: &Value) -> Value {
    let (contents, _) = source_of(case);
    let task = task_at(&contents, case);
    let kind = ChildKind::parse(case["kind"].as_str().expect("kind"))
        .expect("known child kind");
    slot_json(&insertion_offset(&contents, &task, kind, &settings()))
}

fn run_violations(case: &Value) -> Value {
    let (contents, _) = source_of(case);
    let task = task_at(&contents, case);
    Value::Array(
        layout_violations(&contents, &task, &settings())
            .iter()
            .map(|violation| {
                json!({
                    "line": violation.line_index + 1,
                    "kind": violation.kind.name(),
                })
            })
            .collect(),
    )
}

fn run_tidy(case: &Value) -> Value {
    let (contents, _) = source_of(case);
    let task = task_at(&contents, case);
    let settings = settings();
    match plan_tidy(
        &contents,
        &task,
        &TidyPolicy {
            settings: &settings,
        },
    ) {
        None => Value::Null,
        Some(plan) => json!({
            "lines": lines_of(&plan.updated),
            "moves": plan.moves,
            "notes_filed": plan.notes_filed,
            "notes_created": plan.notes_created,
        }),
    }
}

fn run_note_home(case: &Value) -> Value {
    let (contents, _) = source_of(case);
    let task = task_at(&contents, case);
    match note_home(&contents, &task, &settings()) {
        NoteHome::Loose(slot) => {
            json!({"home": "loose", "line": slot.line_index + 1, "indent": slot.indent})
        }
        NoteHome::Notes(drawer) => {
            json!({"home": "notes", "line": drawer.line})
        }
        NoteHome::NewNotes => json!({"home": "new-notes"}),
    }
}

fn refused(refusal: &WriteRefusal) -> Value {
    json!({"refused": refusal.code})
}

fn run_write(case: &Value, date: NaiveDate) -> Value {
    let (contents, ending) = source_of(case);
    let settings = settings();
    let physical = lines_of(&contents);
    let line = |key: &str| case[key].as_u64().expect(key) as usize - 1;
    let target = |key: &str| {
        let index = line(key);
        ReqTarget {
            line_index: index,
            preimage: case["preimage"].as_str().unwrap_or_else(|| {
                physical.get(index).map_or("", String::as_str)
            }),
        }
    };
    let rejoined = |updated: &str| {
        // Every byte of the answer keeps the note's own line endings.
        assert!(
            ending == "\n"
                || updated.matches('\n').count()
                    == updated.matches("\r\n").count(),
            "{}: CRLF note stays CRLF",
            case["id"]
        );
        lines_of(updated)
    };
    match case["op"].as_str().expect("op") {
        "drop" => match drop_req(
            &contents,
            target("line"),
            date,
            case["reason"].as_str().unwrap_or(""),
            &settings,
        ) {
            Ok(write) => json!({
                "lines": rejoined(&write.updated),
                "log": match write.log {
                    writers::LogOutcome::None => "none",
                    writers::LogOutcome::Created => "created",
                    writers::LogOutcome::Prepended => "prepended",
                },
                "descendants": write.descendants,
            }),
            Err(refusal) => refused(&refusal),
        },
        "restore" => match restore_req(&contents, target("line"), &settings) {
            Ok(write) => json!({
                "lines": rejoined(&write.updated),
                "descendants": write.descendants,
            }),
            Err(refusal) => refused(&refusal),
        },
        "toggle" => match toggle_note_req(&contents, target("line"), &settings)
        {
            Ok(write) => json!({
                "lines": rejoined(&write.updated),
                "direction": match write.direction {
                    ToggleDirection::NoteToReq => "note-to-req",
                    ToggleDirection::ReqToNote => "req-to-note",
                },
                "line": write.line_index + 1,
                "depth": write.depth,
                "scooped": write.scooped,
                "notes_created": write.notes_created,
            }),
            Err(refusal) => refused(&refusal),
        },
        "insert" => {
            let blocks: Vec<String> = case["blocks"]
                .as_array()
                .expect("blocks")
                .iter()
                .map(|block| block.as_str().expect("block").to_string())
                .collect();
            match insert_reqs(&contents, target("task"), &blocks, &settings) {
                Ok(write) => json!({
                    "lines": rejoined(&write.updated),
                    "first_line": write.first_line + 1,
                    "line_count": write.line_count,
                    "scooped": write.scooped,
                    "notes_created": write.notes_created,
                }),
                Err(refusal) => refused(&refusal),
            }
        }
        other => panic!("unknown op {other}"),
    }
}

fn load(raw: &str) -> Value {
    serde_json::from_str(raw).expect("valid vectors")
}

fn layout_vectors() -> Value {
    load(include_str!(
        "../../../tests/fixtures/task_reqs/layout.json"
    ))
}

fn writes_vectors() -> Value {
    load(include_str!(
        "../../../tests/fixtures/task_reqs/writes.json"
    ))
}

fn check_all(vectors: &Value, section: &str, run: impl Fn(&Value) -> Value) {
    let cases = vectors[section].as_array().expect(section);
    assert!(!cases.is_empty(), "{section} has cases");
    for case in cases {
        assert_eq!(
            run(case),
            case["expect"],
            "{} {}",
            case["id"].as_str().unwrap_or("?"),
            case["title"].as_str().unwrap_or("")
        );
    }
}

#[test]
fn layout_slots_match_vectors() {
    check_all(&layout_vectors(), "slots", run_slot);
}

#[test]
fn layout_violations_match_vectors() {
    check_all(&layout_vectors(), "violations", run_violations);
}

#[test]
fn layout_tidy_matches_vectors() {
    check_all(&layout_vectors(), "tidy", run_tidy);
}

#[test]
fn layout_note_homes_match_vectors() {
    check_all(&layout_vectors(), "note_homes", run_note_home);
}

#[test]
fn req_writers_match_vectors() {
    let vectors = writes_vectors();
    let date = NaiveDate::parse_from_str(
        vectors["date"].as_str().expect("date"),
        "%Y-%m-%d",
    )
    .expect("vector date");
    check_all(&vectors, "cases", |case| run_write(case, date));
}

#[test]
fn vector_ids_are_unique_and_sequenced() {
    let layout = layout_vectors();
    let mut ids: Vec<String> = ["slots", "violations", "tidy", "note_homes"]
        .iter()
        .flat_map(|section| {
            layout[*section]
                .as_array()
                .expect("section")
                .iter()
                .map(|case| case["id"].as_str().expect("id").to_string())
        })
        .collect();
    let count = ids.len();
    ids.sort_by_key(|id| id[2..].parse::<usize>().expect("RL number"));
    ids.dedup();
    assert_eq!(ids.len(), count, "RL ids are unique");
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(id, &format!("RL{}", index + 1), "RL ids are sequential");
    }
    let writes = writes_vectors();
    let write_ids: Vec<&str> = writes["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| case["id"].as_str().expect("id"))
        .collect();
    for (index, id) in write_ids.iter().enumerate() {
        assert_eq!(*id, format!("RW{}", index + 1), "RW ids are sequential");
    }
}

#[test]
fn a_tidied_block_has_no_violations_and_tidies_to_none() {
    let settings = settings();
    let vectors = layout_vectors();
    for case in vectors["tidy"].as_array().expect("tidy") {
        let (contents, _) = source_of(case);
        let task = task_at(&contents, case);
        let Some(plan) = plan_tidy(
            &contents,
            &task,
            &TidyPolicy {
                settings: &settings,
            },
        ) else {
            continue;
        };
        let task = find_task(&plan.updated, task.line_index, &settings)
            .expect("task survives tidy");
        assert!(
            layout_violations(&plan.updated, &task, &settings).is_empty(),
            "{} leaves no violations",
            case["id"]
        );
        assert!(
            plan_tidy(
                &plan.updated,
                &task,
                &TidyPolicy {
                    settings: &settings
                }
            )
            .is_none(),
            "{} is idempotent",
            case["id"]
        );
    }
}

#[test]
fn zones_name_every_task_child_kind() {
    use crate::native::task_children::classify_task_child_with_filter;
    for (line, zone) in [
        ("\t- ❌ **CANCEL LOG**", Zone::CancelLog),
        ("\t- ⛓️ **DEPENDS ON:** [[#^a]]", Zone::ManagedLinks),
        ("\t- [ ] a requirement", Zone::Reqs),
        ("\t- [-] dropped requirement", Zone::Reqs),
        ("\t- a plain note", Zone::Authored),
        ("\t- [ ] #task nested task", Zone::Authored),
        ("\t- NOTES", Zone::Drawers),
        ("\t- FUTURE WORK", Zone::Drawers),
        ("\t- 🛠️ **WORK LOG**", Zone::Logs),
        ("\t- 🗓️ **SCHEDULE LOG**", Zone::Logs),
        ("\t- 🥀 **ROTTEN LOG**", Zone::Logs),
    ] {
        let kind = classify_task_child_with_filter(line, Some(true), "#task");
        assert_eq!(zone_of(&kind), zone, "{line}");
    }
    assert!(Zone::CancelLog < Zone::ManagedLinks);
    assert!(Zone::Drawers < Zone::Logs);
}

// --- agreement with the writers that already place their own children -----

const FULL_BLOCK: &str = "\
- [ ] #task Ship it ^ship
\t- ❌ **CANCEL LOG**
\t\t- *2026-11-01* — earlier
\t- ⛓️ **DEPENDS ON:** [[#^parser]]
\t- [ ] First requirement
\t\t- a note on the req
\t- loose note
\t- NOTES
\t\t- an entry
\t- 🛠️ **WORK LOG**
\t\t- *2026-11-02* — worked
";

#[test]
fn managed_link_writers_land_inside_the_managed_zone() {
    use crate::native::{
        task_dependencies::blocks::blocks_insert_slot,
        task_relations::format::{relation_insert_slot, RelationSlotKind},
        task_status_hooks::{fenced_lines, logical_lines},
    };
    let settings = settings();
    let task = find_task(FULL_BLOCK, 0, &settings).expect("task");
    let layout =
        insertion_offset(FULL_BLOCK, &task, ChildKind::ManagedLink, &settings);
    // After the Cancel Log and the Depends-On line, before the first req.
    assert_eq!(layout.line_index, 4);

    let lines = logical_lines(FULL_BLOCK);
    let fenced = fenced_lines(&lines, 0..lines.len());
    let blocks = blocks_insert_slot(&lines, &fenced, 0);
    assert_eq!(
        blocks.line_index, 4,
        "BLOCKS follows the Depends-On subtree"
    );
    for kind in [RelationSlotKind::Links, RelationSlotKind::LinkedFrom] {
        let slot = relation_insert_slot(&lines, &fenced, 0, kind);
        assert!(
            slot.line_index <= layout.line_index,
            "{kind:?} lands no later than the end of the managed zone"
        );
        assert_eq!(slot.indent, layout.indent);
    }

    // Without a Cancel Log or Depends-On line, links take the first slot,
    // ahead of reqs, the way the layout module answers.
    let bare = "- [ ] #task A ^a\n\t- [ ] req\n\t- note\n";
    let task = find_task(bare, 0, &settings).expect("task");
    let layout =
        insertion_offset(bare, &task, ChildKind::ManagedLink, &settings);
    let lines = logical_lines(bare);
    let fenced = fenced_lines(&lines, 0..lines.len());
    assert_eq!(layout.line_index, 1);
    assert_eq!(
        relation_insert_slot(&lines, &fenced, 0, RelationSlotKind::Links)
            .line_index,
        1
    );
    assert_eq!(blocks_insert_slot(&lines, &fenced, 0).line_index, 1);
}

#[test]
fn log_and_cancel_log_writers_land_in_their_zones() {
    use crate::native::{capture_schedule_log, task_cancel};
    let settings = settings();
    let task = find_task(FULL_BLOCK, 0, &settings).expect("task");

    // A new managed log appends as the task's last direct child.
    let log_slot =
        insertion_offset(FULL_BLOCK, &task, ChildKind::Log, &settings);
    let (with_log, _) = capture_schedule_log::plan_entry_insertion(
        FULL_BLOCK,
        0,
        "*2026-11-03* — scheduled",
    );
    let lines = lines_of(&with_log);
    assert_eq!(lines[log_slot.line_index], "\t- 🗓️ **SCHEDULE LOG**");

    // A created Cancel Log is the task's first child.
    let cancel_slot =
        insertion_offset(FULL_BLOCK, &task, ChildKind::CancelLog, &settings);
    let bare = "- [ ] #task A ^a\n\t- [ ] req\n";
    let edited = task_cancel::line::edit_source_create_log(
        bare,
        0,
        NaiveDate::from_ymd_opt(2026, 11, 21).expect("date"),
        &["why".to_string()],
    );
    let bare_task = find_task(bare, 0, &settings).expect("task");
    let bare_slot =
        insertion_offset(bare, &bare_task, ChildKind::CancelLog, &settings);
    assert_eq!(
        lines_of(&edited.content)[bare_slot.line_index],
        "\t- ❌ **CANCEL LOG**"
    );
    assert_eq!(cancel_slot.line_index, 1);
}

#[test]
fn a_created_notes_drawer_lands_first_among_the_drawers() {
    let settings = settings();
    let contents = "\
- [ ] #task A ^a
\t- [ ] req
\t- FUTURE WORK
\t\t- later
\t- 🛠️ **WORK LOG**
";
    let task = find_task(contents, 0, &settings).expect("task");
    let (updated, drawer, created) =
        ensure_notes_drawer(contents, &task).expect("created");
    assert!(created);
    assert_eq!(drawer.title, "Notes");
    assert_eq!(
        lines_of(&updated),
        [
            "- [ ] #task A ^a",
            "\t- [ ] req",
            "\t- **Notes**",
            "\t- FUTURE WORK",
            "\t\t- later",
            "\t- 🛠️ **WORK LOG**",
        ]
    );
    // Any other created drawer keeps today's slot before the managed logs.
    assert_eq!(
        drawer_header_offset(contents, &task, false),
        contents.find("\t- 🛠️").expect("log offset")
    );
    assert_eq!(
        drawer_header_offset(contents, &task, true),
        contents.find("\t- FUTURE").expect("drawer offset")
    );
}

#[test]
fn notes_home_matches_bold_notes_by_key_and_tidy_reuses_its_header() {
    let settings = settings();
    let contents = concat!(
        "- [ ] #task A ^a\n",
        "\t- [ ] req\n",
        "\t- **Notes**\n",
        "\t\t- existing note\n",
        "\t- loose note\n",
    );
    let task = find_task(contents, 0, &settings).expect("task");
    let Some(plan) = plan_tidy(
        contents,
        &task,
        &TidyPolicy {
            settings: &settings,
        },
    ) else {
        panic!("loose note should file into the existing notes drawer");
    };
    assert_eq!(plan.notes_filed, 1);
    assert!(!plan.notes_created);
    assert_eq!(plan.updated.matches("**Notes**").count(), 1);
    assert!(plan
        .updated
        .contains("\t\t- existing note\n\t\t- loose note\n"));
    assert!(!plan.updated.contains("\t- NOTES\n"));
    let refreshed =
        find_task(&plan.updated, 0, &settings).expect("task remains");
    let drawer = notes_drawer(&plan.updated, &refreshed).expect("notes home");
    assert_eq!(drawer.title, "Notes");
    assert_eq!(drawer.marker, capture_drawers::DrawerMarker::Bold);
}

#[test]
fn a_note_without_a_final_newline_keeps_that_state() {
    let settings = settings();
    let contents = "- [ ] #task A ^a\n\t- loose\n\t- [ ] first req";
    let task = find_task(contents, 0, &settings).expect("task");
    let plan = plan_tidy(
        contents,
        &task,
        &TidyPolicy {
            settings: &settings,
        },
    )
    .expect("tidy moves the req and files the note");
    assert_eq!(
        plan.updated,
        "- [ ] #task A ^a\n\t- [ ] first req\n\t- **Notes**\n\t\t- loose"
    );
    assert_eq!(
        (plan.moves, plan.notes_filed, plan.notes_created),
        (0, 1, true)
    );
}

#[test]
fn canonicalize_new_req_block_files_notes_and_keeps_req_subtrees() {
    let settings = settings();
    let block = concat!(
        "- [ ] #task Ship the exporter [created::2026-06-15]\n",
        "\t- see the ticket\n",
        "\t- [ ] Handles CRLF\n",
        "\t\t- [ ] Windows\n",
        "\t\t\t- seen in March\n",
        "\t- FUTURE WORK\n",
        "\t\t- later\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-06-16* — tomorrow",
    );
    let canonical =
        canonicalize_new_req_block(block, &settings).expect("owns reqs");
    assert_eq!(canonical.owner, "Ship the exporter");
    assert_eq!(canonical.added, 2);
    assert_eq!(canonical.notes_filed, 1);
    assert!(canonical.notes_created);
    assert_eq!(
        canonical.updated,
        concat!(
            "- [ ] #task Ship the exporter [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t\t- [ ] Windows\n",
            "\t\t\t- seen in March\n",
            "\t- **Notes**\n",
            "\t\t- see the ticket\n",
            "\t- FUTURE WORK\n",
            "\t\t- later\n",
            "\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t- *2026-06-16* — tomorrow",
        )
    );
    let task = find_task(&canonical.updated, 0, &settings).expect("task");
    assert!(layout_violations(&canonical.updated, &task, &settings).is_empty());
    assert!(plan_tidy(
        &canonical.updated,
        &task,
        &TidyPolicy {
            settings: &settings
        }
    )
    .is_none());
}

#[test]
fn canonicalize_new_req_block_leaves_zero_req_and_closed_blocks() {
    let settings = settings();
    let zero_req = concat!(
        "- [ ] #task No reqs [created::2026-06-15]\n",
        "\t- just a note",
    );
    assert_eq!(canonicalize_new_req_block(zero_req, &settings), None);
    let closed = concat!(
        "- [x] #task Done already [created::2026-06-15]\n",
        "\t- leftover note\n",
        "\t- [ ] still a req",
    );
    let canonical = canonicalize_new_req_block(closed, &settings)
        .expect("closed owns reqs");
    assert_eq!(canonical.updated, closed);
    assert_eq!(canonical.added, 1);
    assert_eq!(canonical.notes_filed, 0);
    assert!(!canonical.notes_created);
}
