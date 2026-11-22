//! Carried-task pull-in placement and pinned-requirement jump landings
//! (RV `placement` and `jump` families).
//!
//! Placement links a `carry_pulled` task once at its owner's ordinary
//! successor destination, raises it to Next, and never stamps freshness.
//! Jump landings resolve a pinned row after every write and route by
//! exact identity, refusing rather than landing on a same-text neighbor.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::{
    req_close::{ReqCloseGroup, ReqCloseOwner},
    req_landing::{
        resolve_req_landing, ReqLanding, ReqLandingInput, ReqRouteMove,
    },
    req_placement::{
        closing_after_successors, plan_req_links, pulled_tasks, ReqLinkInput,
        ReqLinkPlan, ReqPlacementError,
    },
    req_verdicts::{
        resolve_verdicts, ReqVerdict, ReqVerdictRequest, ReqVerdictSelection,
    },
    req_writes::{plan_req_writes, ReqWritePlan},
    successors::{
        plan_successors, ClosingContext, CompletedTask, SuccessorInput,
        SuccessorSelection,
    },
};
use super::req_close_tests::{collect_case, vectors};
use crate::native::{
    note_tasks, task_move::VaultSnapshot, task_split::plan::SplitSettings,
    vault_links::NoteIndex,
};

const TODAY: &str = "2026-11-22";

fn settings() -> note_tasks::NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn split_settings() -> SplitSettings {
    SplitSettings {
        linked_from_lines: true,
        linked_from_mentions: true,
        inbox_route: "inbox.md".to_string(),
    }
}

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 22).expect("valid date")
}

fn joined(lines: &[&str]) -> String {
    lines.join("\n") + "\n"
}

fn source(value: &serde_json::Value) -> String {
    let lines: Vec<&str> = value
        .as_array()
        .expect("source_lines")
        .iter()
        .map(|line| line.as_str().expect("line"))
        .collect();
    joined(&lines)
}

/// A close scenario: owners, their notes, and the card resolution.
struct Scenario {
    owners: Vec<ReqCloseOwner>,
    groups: Vec<ReqCloseGroup>,
    snapshot: VaultSnapshot,
}

impl Scenario {
    /// `notes`: `(path, text, closing block IDs in close order)`.
    fn new(notes: &[(&str, &str, &[&str])]) -> Self {
        let notes_json: Vec<serde_json::Value> = notes
            .iter()
            .map(|(path, text, closing)| {
                serde_json::json!({
                    "path": path,
                    "source_lines": text.strip_suffix('\n').unwrap_or(text)
                        .split('\n').collect::<Vec<_>>(),
                    "closing": closing.iter()
                        .map(|id| serde_json::json!({"block_id": id}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        Self::from_case(&serde_json::json!({"id": "test", "notes": notes_json}))
    }

    fn from_case(case: &serde_json::Value) -> Self {
        let (owners, groups) = collect_case(case, &settings());
        let mut files = BTreeMap::new();
        for note in case["notes"].as_array().expect("notes") {
            files.insert(
                PathBuf::from(note["path"].as_str().expect("path")),
                source(&note["source_lines"]),
            );
        }
        Self {
            owners,
            groups,
            snapshot: VaultSnapshot { notes: files },
        }
    }

    fn key(&self, owner: usize, path: &str) -> String {
        let owner_key = &self.owners[owner].key;
        self.groups
            .iter()
            .filter(|group| &group.owner.key == owner_key)
            .flat_map(|group| &group.rows)
            .find(|row| row.path == path)
            .unwrap_or_else(|| panic!("no requirement {path}"))
            .offer
            .key
            .clone()
    }

    fn select(
        &self,
        picks: &[(usize, &str, ReqVerdict, Option<&str>)],
        jump: Option<(usize, &str)>,
    ) -> ReqVerdictSelection {
        let requests: Vec<ReqVerdictRequest> = picks
            .iter()
            .map(|(owner, path, verdict, reason)| ReqVerdictRequest {
                selector: self.key(*owner, path),
                verdict: *verdict,
                reason: reason.map(str::to_string),
            })
            .collect();
        let jump = jump.map(|(owner, path)| self.key(owner, path));
        resolve_verdicts(&self.owners, &[], &requests, jump.as_deref())
            .expect("card resolves")
    }

    fn write(&self, selection: &ReqVerdictSelection) -> ReqWritePlan {
        plan_req_writes(
            &self.snapshot,
            &self.owners,
            selection,
            TODAY,
            date(),
            &split_settings(),
            &settings(),
        )
        .unwrap_or_else(|error| panic!("writes plan: {error}"))
    }

    /// Note texts after the write plan.
    fn after(&self, plan: &ReqWritePlan) -> BTreeMap<PathBuf, String> {
        let mut notes = self.snapshot.notes.clone();
        notes.extend(plan.postimages.clone());
        notes
    }
}

fn index(paths: &[&str]) -> NoteIndex {
    NoteIndex::from_paths(paths.iter().map(PathBuf::from))
}

fn root(path: &str, block_id: &str, text: &str) -> CompletedTask {
    CompletedTask {
        relative_path: PathBuf::from(path),
        block_id: block_id.to_string(),
        task_id: String::new(),
        text: text.to_string(),
        is_root: true,
    }
}

fn link(
    scenario: &Scenario,
    plan: &ReqWritePlan,
    day: Option<&str>,
    day_relative: &str,
    vault: &NoteIndex,
    completed: &[CompletedTask],
    closing: Option<&ClosingContext>,
) -> Result<ReqLinkPlan, ReqPlacementError> {
    let notes = scenario.after(plan);
    let task_settings = settings();
    plan_req_links(
        &ReqLinkInput {
            bob_dir: None,
            today: date(),
            day_text: day,
            day_relative: Path::new(day_relative),
            index: vault,
            completed,
            closing,
            notes: &notes,
            settings: &task_settings,
        },
        &pulled_tasks(&scenario.owners, plan),
    )
}

fn land(
    scenario: &Scenario,
    selection: &ReqVerdictSelection,
    plan: &ReqWritePlan,
    finals: &BTreeMap<PathBuf, String>,
    routes: &[ReqRouteMove],
) -> Result<ReqLanding, ReqPlacementError> {
    let target = selection
        .jump
        .as_ref()
        .and_then(|target| target.requirement.as_ref())
        .expect("requirement pin");
    let task_settings = settings();
    let final_text = |path: &str| finals.get(&PathBuf::from(path)).cloned();
    resolve_req_landing(
        &ReqLandingInput {
            target,
            owners: &scenario.owners,
            plan,
            routes,
            settings: &task_settings,
        },
        &final_text,
    )
}

/// Block ID (else description) of the innermost task whose block holds
/// 1-based `line`: the identity a landing proves it hit.
fn enclosing_task(text: &str, line: usize) -> Option<String> {
    let scan = note_tasks::scan(text, &settings());
    scan.tasks()
        .iter()
        .filter(|task| {
            let end_line = text[..task.block_end.min(text.len())]
                .trim_end_matches('\n')
                .matches('\n')
                .count();
            task.line_index < line && line - 1 <= end_line
        })
        .max_by_key(|task| task.line_index)
        .map(|task| {
            task.block_id
                .clone()
                .unwrap_or_else(|| task.description.clone())
        })
}

fn line_of(text: &str, line: usize) -> &str {
    text.lines().nth(line - 1).expect("line in range")
}

const EXPORTER: &str = concat!(
    "- [ ] #task Ship the exporter ^export\n",
    "\t- [ ] Handles supported inputs\n",
    "\t\t- [ ] Handles Windows files\n",
    "\t\t\t- [ ] Preserves CRLF\n",
    "\t\t- [ ] Handles empty input\n",
);

const DAY_WITH_EXPORT: &str = concat!(
    "## Pomodoros\n",
    "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    "  - [[Tasks#^export]]\n",
    "- [ ] () \u{2014} FIX\n",
    "  - [[sase#^other]]\n",
);

#[test]
fn pulled_carry_links_after_owner_slot_as_next_without_freshness() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let vault = index(&["Tasks.md", "20261122.md", "sase.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(DAY_WITH_EXPORT),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("slot placement");
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    let day = links.day_text.as_deref().expect("day changed");
    assert_eq!(
        day,
        &format!(
            concat!(
                "## Pomodoros\n",
                "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
                "  - [[Tasks#^export]]\n",
                "  - [[Tasks#^{}]]\n",
                "- [ ] () \u{2014} FIX\n",
                "  - [[sase#^other]]\n",
            ),
            carried.block_id
        )
    );
    let note = links
        .notes
        .get(&PathBuf::from("Tasks.md"))
        .expect("status raised");
    let task_line = note
        .lines()
        .find(|line| line.ends_with(&format!("^{}", carried.block_id)))
        .expect("created task line");
    assert_eq!(
        task_line,
        format!(
            "- [*] #task Handles empty input [created:: {TODAY}] ^{}",
            carried.block_id
        )
    );
    assert!(!note.contains("[fresh::"));
    assert!(!note.contains("[unblocked::"));
    assert!(!note.contains("[keeps::"));
    assert_eq!(links.links.len(), 1);
    let report = &links.links[0];
    assert_eq!(report.previous_status_symbol, ' ');
    assert_eq!(report.status_symbol, '*');
    assert_eq!(report.destination.entry_name, "CAPTURE");
    assert_eq!(report.destination.entry_role.as_str(), "slot");
    assert!(!report.destination.fallback);
    assert_eq!(report.link.line, 4);
    assert_eq!(report.link.entry_line, 2);
    assert!(!report.link.entry_created);
}

#[test]
fn carry_without_pull_in_stays_unlinked_and_ready() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::Carry, None)], None);
    let plan = scenario.write(&selection);
    let vault = index(&["Tasks.md", "20261122.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(DAY_WITH_EXPORT),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("nothing to place");
    assert_eq!(links, ReqLinkPlan::default());
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    let note = &plan.postimages[&PathBuf::from("Tasks.md")];
    assert!(note.contains(&format!(
        "- [ ] #task Handles empty input [created:: {TODAY}] ^{}",
        carried.block_id
    )));
}

#[test]
fn nested_owner_inherits_the_root_slot() {
    let notes = concat!(
        "- [ ] #task Release ^release\n",
        "\t- [ ] #task Ship the exporter ^export\n",
        "\t\t- [ ] Handles empty input\n",
    );
    let scenario =
        Scenario::new(&[("Tasks.md", notes, &["export", "release"])]);
    let selection =
        scenario.select(&[(0, "1", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} SHIP\n",
        "  - [[Tasks#^release]]\n",
    );
    let vault = index(&["Tasks.md", "20261122.md"]);
    let completed = [
        root("Tasks.md", "release", "Release"),
        CompletedTask {
            is_root: false,
            ..root("Tasks.md", "export", "Ship the exporter")
        },
    ];
    let links = link(
        &scenario,
        &plan,
        Some(day),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("inherit placement");
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    assert_eq!(
        links.day_text.as_deref(),
        Some(format!("{day}  - [[Tasks#^{}]]\n", carried.block_id).as_str())
    );
    assert_eq!(links.links[0].destination.entry_name, "SHIP");
    assert_eq!(links.links[0].destination.entry_role.as_str(), "slot");
}

#[test]
fn unlinked_owner_falls_back_to_the_implicit_destination() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let day = concat!("## Pomodoros\n", "- [ ] () \u{2014} FIX\n");
    let vault = index(&["Tasks.md", "20261122.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(day),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("implicit placement");
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    assert_eq!(
        links.day_text.as_deref(),
        Some(format!("{day}\t- [[Tasks#^{}]]\n", carried.block_id).as_str())
    );
    let report = &links.links[0];
    assert_eq!(report.destination.entry_role.as_str(), "implicit");
    assert!(report.destination.fallback);
    assert_eq!(report.destination.entry_name, "FIX");
}

#[test]
fn pulled_carry_without_destination_refuses_the_batch() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let vault = index(&["Tasks.md", "20261122.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    for day in [None, Some("## Pomodoros\n- [x] 09:00-09:25 \u{2014} FIX\n")] {
        let error = link(
            &scenario,
            &plan,
            day,
            "20261122.md",
            &vault,
            &completed,
            None,
        )
        .expect_err("no destination");
        assert_eq!(error.code(), "req_pull_in_no_destination");
    }
}

#[test]
fn closing_target_reuses_the_placeholder_ordinary_successors_created() {
    // An ordinary successor of the closed FIX entry lands in a created
    // `- [ ] ()` placeholder; the carried link must join it, not create a
    // second placeholder. The ordinary successor keeps its freshness
    // stamp; the capture-created task gets none.
    let tasks = concat!(
        "- [x] #task Fix apollo [id:: fix] ✅ 2026-11-22 ^fix\n",
        "\t- [ ] Handles empty input\n",
        "- [?] #task Relaunch [dependsOn:: fix] [id:: relaunch] ^relaunch\n",
    );
    let pre_close = tasks.replace(
        "- [x] #task Fix apollo [id:: fix] ✅ 2026-11-22",
        "- [ ] #task Fix apollo [id:: fix]",
    );
    let scenario = Scenario::new(&[("Tasks.md", &pre_close, &["fix"])]);
    let selection =
        scenario.select(&[(0, "1", ReqVerdict::CarryPulled, None)], None);
    let vault = index(&["Tasks.md", "20261122.md"]);
    let post_day = concat!(
        "## Pomodoros\n",
        "- [x] (**0920-0950**) \u{2014} FIX\n",
        "  - ~~[[Tasks#^fix]]~~\n",
    );
    let closing = ClosingContext {
        continuation_name: String::new(),
        closed_line: 1,
        continuation: None,
    };
    let task_settings = super::test_settings();
    let successors = plan_successors(SuccessorInput {
        bob_dir: None,
        completed: vec![CompletedTask {
            task_id: "fix".to_string(),
            ..root("Tasks.md", "fix", "Fix apollo")
        }],
        snapshot: vec![(PathBuf::from("Tasks.md"), tasks.to_string())],
        note_texts: Vec::new(),
        pre_day: Some(post_day.to_string()),
        post_day: Some(post_day.to_string()),
        closing: Some(closing.clone()),
        day_relative: PathBuf::from("20261122.md"),
        today: date(),
        tasks_settings: &task_settings,
        note_settings: &task_settings,
        link_unblocked: true,
        keep_moving: false,
        snapshot_complete: true,
        selection: SuccessorSelection::All,
        index: &vault,
        relation: None,
    });
    let ordinary_day = successors.new_day_text.clone().expect("successor");
    assert_eq!(
        ordinary_day,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0920-0950**) \u{2014} FIX\n",
            "  - ~~[[Tasks#^fix]]~~\n",
            "- [ ] ()\n",
            "\t- [[Tasks#^relaunch]]\n",
        )
    );
    let continuation = successors
        .closing_continuation
        .clone()
        .expect("closing target materialized");
    assert_eq!(continuation.line, 4);
    assert!(continuation.created);
    let relaunch = &successors.changed_files[&PathBuf::from("Tasks.md")];
    assert!(relaunch.contains("[fresh:: 2026-11-22]"));

    // Requirement writes run on the post-successor note text.
    let scenario = Scenario {
        snapshot: VaultSnapshot {
            notes: BTreeMap::from([(
                PathBuf::from("Tasks.md"),
                relaunch.clone(),
            )]),
        },
        ..scenario
    };
    let plan = scenario.write(&selection);
    let adjusted = closing_after_successors(&closing, &successors);
    let completed = [root("Tasks.md", "fix", "Fix apollo")];
    let links = link(
        &scenario,
        &plan,
        Some(&ordinary_day),
        "20261122.md",
        &vault,
        &completed,
        Some(&adjusted),
    )
    .expect("closing placement");
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    assert_eq!(
        links.day_text.as_deref(),
        Some(
            format!("{ordinary_day}\t- [[Tasks#^{}]]\n", carried.block_id)
                .as_str()
        )
    );
    let report = &links.links[0];
    assert_eq!(report.destination.entry_role.as_str(), "created");
    assert!(report.link.entry_created);
    assert_eq!(report.link.entry_line, 4);
    let note = &links.notes[&PathBuf::from("Tasks.md")];
    let created = note
        .lines()
        .find(|line| line.ends_with(&format!("^{}", carried.block_id)))
        .expect("created task");
    assert!(created.starts_with("- [*] #task Handles empty input"));
    assert!(!created.contains("[fresh::"));
    // The ordinary successor kept its own freshness stamp.
    assert!(note.contains("[fresh:: 2026-11-22]"));

    // Without the adjustment the same text would gain a second
    // placeholder: the reuse is what the adjustment buys.
    let unadjusted = link(
        &scenario,
        &plan,
        Some(&ordinary_day),
        "20261122.md",
        &vault,
        &completed,
        Some(&closing),
    )
    .expect("unadjusted placement");
    assert_eq!(
        unadjusted
            .day_text
            .as_deref()
            .expect("day")
            .matches("- [ ] ()")
            .count(),
        2
    );
}

#[test]
fn already_live_created_task_is_linked_once() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    let day = format!(
        "{DAY_WITH_EXPORT}- [ ] () \u{2014} LATER\n  - [[Tasks#^{}]]\n",
        carried.block_id
    );
    let vault = index(&["Tasks.md", "20261122.md", "sase.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(&day),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("placement");
    assert_eq!(links.day_text, None, "no second bullet");
    assert!(links.links.is_empty());
    assert!(links.notes[&PathBuf::from("Tasks.md")]
        .contains("- [*] #task Handles empty input"));
}

#[test]
fn jump_alone_changes_no_verdict_and_no_bytes() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection = scenario.select(&[], Some((0, "1.1.1")));
    assert!(selection.verdicts.is_empty());
    let plan = scenario.write(&selection);
    assert!(plan.postimages.is_empty());
    assert!(plan.outcomes.is_empty());
    let vault = index(&["Tasks.md", "20261122.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(DAY_WITH_EXPORT),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("nothing to place");
    assert_eq!(links, ReqLinkPlan::default());
    let landing =
        land(&scenario, &selection, &plan, &scenario.after(&plan), &[])
            .expect("lands");
    assert_eq!(landing.kind.as_str(), "requirement");
    assert_eq!(landing.outcome, ReqVerdict::Met);
    assert_eq!((landing.path.as_str(), landing.line), ("Tasks.md", 4));
}

#[test]
fn tampered_move_map_refuses_instead_of_guessing() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection = scenario
        .select(&[(0, "1", ReqVerdict::Carry, None)], Some((0, "1.1.1")));
    let mut plan = scenario.write(&selection);
    let finals = scenario.after(&plan);
    let landing =
        land(&scenario, &selection, &plan, &finals, &[]).expect("lands");
    assert_eq!(landing.kind.as_str(), "moved_with_ancestor");
    // Shift one move target onto a neighbouring line: structure and the
    // move map now disagree, so the pin refuses before commit.
    for line_move in &mut plan.outcomes[0].moves {
        line_move.to += 1;
    }
    let error = land(&scenario, &selection, &plan, &finals, &[])
        .expect_err("disagreement refuses");
    assert_eq!(error.code(), "req_jump_ambiguous");
}

#[test]
fn lost_pinned_row_refuses() {
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection = scenario.select(&[], Some((0, "1.2")));
    let plan = scenario.write(&selection);
    let mut finals = scenario.after(&plan);
    finals.insert(
        PathBuf::from("Tasks.md"),
        EXPORTER.replace("\t\t- [ ] Handles empty input\n", ""),
    );
    let error =
        land(&scenario, &selection, &plan, &finals, &[]).expect_err("lost");
    assert_eq!(error.code(), "req_jump_lost");
}

#[test]
fn idless_owner_lands_when_unique_and_refuses_when_ambiguous() {
    let one = concat!(
        "- [ ] #task Idless owner\n",
        "\t- [ ] Check it\n",
        "- [ ] #task Other ^other\n",
    );
    let case = serde_json::json!({"id": "idless", "notes": [{
        "path": "Tasks.md",
        "source_lines": one.strip_suffix('\n').unwrap().split('\n').collect::<Vec<_>>(),
        "closing": [{"line": 1}],
    }]});
    let scenario = Scenario::from_case(&case);
    let selection = scenario.select(&[], Some((0, "1")));
    let plan = scenario.write(&selection);
    let mut finals = scenario.after(&plan);
    let landing =
        land(&scenario, &selection, &plan, &finals, &[]).expect("unique");
    assert_eq!((landing.path.as_str(), landing.line), ("Tasks.md", 2));
    // A later edit duplicates the idless owner block: refuse, never pick.
    finals.insert(
        PathBuf::from("Tasks.md"),
        format!("{one}- [ ] #task Idless owner\n\t- [ ] Check it\n"),
    );
    let error = land(&scenario, &selection, &plan, &finals, &[])
        .expect_err("ambiguous");
    assert_eq!(error.code(), "req_jump_ambiguous");
}

#[test]
fn retirement_keeps_the_carried_link_in_the_vacated_slot() {
    // §12.3: Rust inserts before retirement. Retirement then moves the
    // closed owner's bullet out of the placeholder; the carried link stays
    // in FIX exactly once, and FIX is not removed as emptied.
    use super::super::super::task_status_hooks::RawReference;
    use super::super::retirement::{retire_completed_links, LinkStatus};
    let scenario = Scenario::new(&[("Tasks.md", EXPORTER, &["export"])]);
    let selection =
        scenario.select(&[(0, "1.2", ReqVerdict::CarryPulled, None)], None);
    let plan = scenario.write(&selection);
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
        "  - [[sase#^other]]\n",
        "- [ ] () \u{2014} FIX\n",
        "  - [[Tasks#^export]]\n",
    );
    let vault = index(&["Tasks.md", "20261122.md", "sase.md"]);
    let completed = [root("Tasks.md", "export", "Ship the exporter")];
    let links = link(
        &scenario,
        &plan,
        Some(day),
        "20261122.md",
        &vault,
        &completed,
        None,
    )
    .expect("slot placement");
    let carried = &plan.outcomes[0].carried.as_ref().expect("carried").block_id;
    let placed = links.day_text.expect("day");
    let reference = |target: &str, block_id: &str| RawReference {
        target: target.to_string(),
        block_id: block_id.to_string(),
    };
    let statuses = BTreeMap::from([
        (
            reference("Tasks", "export"),
            LinkStatus::Done {
                path: PathBuf::from("Tasks.md"),
            },
        ),
        (
            reference("Tasks", carried),
            LinkStatus::Live {
                path: PathBuf::from("Tasks.md"),
                status: '*',
            },
        ),
        (
            reference("sase", "other"),
            LinkStatus::Live {
                path: PathBuf::from("sase.md"),
                status: ' ',
            },
        ),
    ]);
    let retirement = retire_completed_links(
        &placed,
        &std::collections::BTreeSet::from([(
            PathBuf::from("Tasks.md"),
            "export".to_string(),
        )]),
        &statuses,
        &super::test_settings(),
    );
    assert!(retirement.removed_placeholders.is_empty());
    let text = retirement.text;
    let link = format!("[[Tasks#^{carried}]]");
    assert_eq!(text.matches(&link).count(), 1, "linked exactly once");
    let fix = text.find("\u{2014} FIX").expect("FIX entry kept");
    assert!(text.find(&link).expect("carried link") > fix);
    assert!(!text[fix..].contains("[[Tasks#^export]]"));
}

#[test]
fn ancestry_walks_through_a_hand_ticked_intermediate() {
    // A hand-ticked `[x]` row is Met for itself only, so it is no card
    // row; its Pending descendant still moves with a carried ancestor.
    let notes = concat!(
        "- [ ] #task Ship ^ship\n",
        "\t- [ ] Branch\n",
        "\t\t- [x] Ticked by hand\n",
        "\t\t\t- [ ] Deep pending\n",
    );
    let scenario = Scenario::new(&[("Tasks.md", notes, &["ship"])]);
    assert!(
        scenario.groups[0].rows.iter().all(|row| row.path != "1.1"),
        "the hand-ticked row is not a card row"
    );
    let pin_only = scenario.select(&[], Some((0, "1.1.1")));
    let plan = scenario.write(&pin_only);
    let landing =
        land(&scenario, &pin_only, &plan, &scenario.after(&plan), &[])
            .expect("met row lands");
    assert_eq!(landing.kind.as_str(), "requirement");
    assert_eq!(landing.line, 4);

    let carried = scenario
        .select(&[(0, "1", ReqVerdict::Carry, None)], Some((0, "1.1.1")));
    let plan = scenario.write(&carried);
    let finals = scenario.after(&plan);
    let landing =
        land(&scenario, &carried, &plan, &finals, &[]).expect("moved row");
    assert_eq!(landing.kind.as_str(), "moved_with_ancestor");
    assert_eq!(landing.outcome, ReqVerdict::Carry);
    assert_eq!(
        line_of(&finals[&PathBuf::from("Tasks.md")], landing.line),
        "\t\t- [ ] Deep pending"
    );
}

// ---------------------------------------------------------------------
// RV `placement` and `jump` families.
// ---------------------------------------------------------------------

fn verdict_of(name: &str) -> ReqVerdict {
    match name {
        "met" => ReqVerdict::Met,
        "drop" => ReqVerdict::Drop,
        "carry" => ReqVerdict::Carry,
        "carry_pulled" => ReqVerdict::CarryPulled,
        other => panic!("unknown verdict {other}"),
    }
}

fn case_by_id(id: &str) -> serde_json::Value {
    vectors()["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing {id}"))
        .clone()
}

fn scenario_for(case: &serde_json::Value) -> Scenario {
    let input = &case["input"];
    match input["base_case"].as_str() {
        Some(base) => Scenario::from_case(&case_by_id(base)),
        None => Scenario::from_case(&serde_json::json!({
            "id": case["id"],
            "notes": input["notes"],
        })),
    }
}

fn selection_for(
    scenario: &Scenario,
    case: &serde_json::Value,
) -> ReqVerdictSelection {
    let input = &case["input"];
    let requests: Vec<ReqVerdictRequest> = input["requests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|request| ReqVerdictRequest {
            selector: request["selector"]
                .as_str()
                .expect("selector")
                .to_string(),
            verdict: verdict_of(request["verdict"].as_str().expect("verdict")),
            reason: request["reason"].as_str().map(str::to_string),
        })
        .collect();
    let jump = input["jump"].as_str();
    resolve_verdicts(&scenario.owners, &[], &requests, jump)
        .unwrap_or_else(|error| panic!("{} resolves: {error:?}", case["id"]))
}

fn placement_actual(case: &serde_json::Value) -> serde_json::Value {
    let input = &case["input"];
    let scenario = scenario_for(case);
    let selection = selection_for(&scenario, case);
    let plan = scenario.write(&selection);
    let after = scenario.after(&plan);
    let day = &input["day"];
    let day_relative = day["path"].as_str().expect("day path");
    let day_text = if day["source_lines"].is_null() {
        after.get(&PathBuf::from(day_relative)).cloned()
    } else {
        Some(source(&day["source_lines"]))
    };
    let paths: Vec<&str> = input["index"]
        .as_array()
        .expect("index")
        .iter()
        .map(|path| path.as_str().expect("index path"))
        .collect();
    let vault = index(&paths);
    let completed: Vec<CompletedTask> = input["completed"]
        .as_array()
        .expect("completed")
        .iter()
        .map(|task| CompletedTask {
            relative_path: PathBuf::from(task["path"].as_str().expect("path")),
            block_id: task["block_id"].as_str().unwrap_or_default().to_string(),
            task_id: String::new(),
            text: String::new(),
            is_root: task["is_root"].as_bool().unwrap_or(false),
        })
        .collect();
    let closing = input["closing"].as_object().map(|closing| ClosingContext {
        continuation_name: closing["continuation_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        closed_line: closing["closed_line"].as_u64().unwrap_or(0) as usize,
        continuation: closing.get("continuation").and_then(|value| {
            value.as_object().map(|continuation| {
                super::super::successors::ClosingContinuation {
                    line: continuation["line"].as_u64().unwrap_or(0) as usize,
                    created: continuation["created"].as_bool().unwrap_or(false),
                }
            })
        }),
    });
    let task_settings = settings();
    let result = plan_req_links(
        &ReqLinkInput {
            bob_dir: None,
            today: date(),
            day_text: day_text.as_deref(),
            day_relative: Path::new(day_relative),
            index: &vault,
            completed: &completed,
            closing: closing.as_ref(),
            notes: &after,
            settings: &task_settings,
        },
        &pulled_tasks(&scenario.owners, &plan),
    );
    let links = match result {
        Ok(links) => links,
        Err(error) => {
            return serde_json::json!({"refusal": error.code()});
        }
    };
    let mut notes_after = serde_json::Map::new();
    for (path, original) in &scenario.snapshot.notes {
        let _ = original;
        let final_text = links
            .notes
            .get(path)
            .or_else(|| after.get(path))
            .expect("note");
        if path.to_str() == Some(day_relative) {
            continue;
        }
        if Some(final_text) != scenario.snapshot.notes.get(path) {
            notes_after.insert(
                path.display().to_string(),
                serde_json::Value::String(final_text.clone()),
            );
        }
    }
    for (path, text) in after.iter().chain(links.notes.iter()) {
        if scenario.snapshot.notes.contains_key(path)
            || path.to_str() == Some(day_relative)
        {
            continue;
        }
        let final_text = links.notes.get(path).unwrap_or(text);
        notes_after.insert(
            path.display().to_string(),
            serde_json::Value::String(final_text.clone()),
        );
    }
    let link_rows: Vec<serde_json::Value> = links
        .links
        .iter()
        .map(|report| {
            serde_json::json!({
                "row_key": report.row_key,
                "path": report.path.display().to_string(),
                "block_id": report.block_id,
                "previous_status": report.previous_status_symbol.to_string(),
                "status": report.status_symbol.to_string(),
                "entry_name": report.destination.entry_name,
                "entry_role": report.destination.entry_role.as_str(),
                "fallback": report.destination.fallback,
                "entry_line": report.link.entry_line,
                "line": report.link.line,
                "block_link": report.link.block_link,
                "entry_created": report.link.entry_created,
            })
        })
        .collect();
    serde_json::json!({
        "day_after": links.day_text,
        "notes_after": notes_after,
        "links": link_rows,
    })
}

fn jump_actual(case: &serde_json::Value) -> serde_json::Value {
    let input = &case["input"];
    let scenario = scenario_for(case);
    let selection = selection_for(&scenario, case);
    let plan = scenario.write(&selection);
    let mut finals = scenario.after(&plan);
    if let Some(overrides) = input["final_notes"].as_object() {
        for (path, lines) in overrides {
            if lines.is_null() {
                finals.remove(&PathBuf::from(path));
            } else {
                finals.insert(PathBuf::from(path), source(lines));
            }
        }
    }
    let routes: Vec<ReqRouteMove> = input["routes"]
        .as_array()
        .map(|routes| {
            routes
                .iter()
                .map(|route| ReqRouteMove {
                    source: route["source"]
                        .as_str()
                        .expect("source")
                        .to_string(),
                    dest: route["dest"].as_str().expect("dest").to_string(),
                    old_block_id: route["old_block_id"]
                        .as_str()
                        .map(str::to_string),
                    new_block_id: route["new_block_id"]
                        .as_str()
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    match land(&scenario, &selection, &plan, &finals, &routes) {
        Ok(landing) => {
            let text = finals
                .get(&PathBuf::from(&landing.path))
                .expect("landing note");
            serde_json::json!({
                "landing": {
                    "kind": landing.kind.as_str(),
                    "outcome": landing.outcome.as_str(),
                    "path": landing.path,
                    "line": landing.line,
                    "block_id": landing.block_id,
                    "text": line_of(text, landing.line),
                    "in_task": enclosing_task(text, landing.line),
                },
                "postimage_count": plan.postimages.len(),
                "carried": plan
                    .outcomes
                    .iter()
                    .filter_map(|outcome| {
                        let carried = outcome.carried.as_ref()?;
                        Some((
                            outcome.path.clone(),
                            serde_json::Value::String(carried.block_id.clone()),
                        ))
                    })
                    .collect::<serde_json::Map<_, _>>(),
            })
        }
        Err(error) => serde_json::json!({"refusal": error.code()}),
    }
}

fn family(name: &str) -> Vec<serde_json::Value> {
    vectors()["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["family"] == name)
        .cloned()
        .collect()
}

#[test]
fn placement_vectors_match_pinned_links_and_bytes() {
    let cases = family("placement");
    assert_eq!(cases.len(), 7, "placement freezes RV29–RV35");
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("RV{}", index + 29));
        assert_eq!(
            placement_actual(case),
            case["expect"],
            "{} placement",
            case["id"]
        );
    }
}

#[test]
fn jump_vectors_match_pinned_landings() {
    let cases = family("jump");
    assert_eq!(cases.len(), 8, "jump freezes RV36–RV43");
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("RV{}", index + 36));
        assert_eq!(jump_actual(case), case["expect"], "{} jump", case["id"]);
    }
}

/// Regenerates the expectations of the `placement` and `jump` families
/// into `$TMPDIR/rv_placement_expect.json` for review before a reviewed
/// copy into the fixture. Never run in CI.
#[test]
#[ignore = "fixture generator"]
fn generate_placement_and_jump_expectations() {
    let mut out = serde_json::Map::new();
    for case in family("placement") {
        out.insert(
            case["id"].as_str().unwrap().to_string(),
            placement_actual(&case),
        );
    }
    for case in family("jump") {
        out.insert(
            case["id"].as_str().unwrap().to_string(),
            jump_actual(&case),
        );
    }
    let path = std::env::temp_dir().join("rv_placement_expect.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap(),
    )
    .unwrap();
    println!("wrote {}", path.display());
}
