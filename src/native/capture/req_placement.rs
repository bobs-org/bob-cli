//! Capture adapter for carried-task pull-in placement and requirement
//! jump landings.
//!
//! [`plan_req_link_pass`] runs inside a managed Done item, after the
//! requirement write pass and the item's ordinary successor insertion and
//! before ledger retirement (§12.3: Rust inserts successors before
//! retirement strikes the predecessor). It records every carried task as
//! born in this capture, so later items and freshness logic treat it as
//! new, links each pulled carry once, and stages the day and note
//! postimages through the batch planner. It never writes to disk.
//!
//! [`build_req_capture_jump`] resolves a pinned requirement row against
//! the batch's final staged texts and inbox-route landings. A lost or
//! ambiguous target refuses before commit.

use std::path::Path;

use chrono::NaiveDate;

use super::{
    batch::CaptureBatchPlanner,
    inbox_route::RoutedTaskMove,
    open_notes::{landed_note, OpenNoteJson},
    output::{CaptureError, CaptureJumpJson},
    plan::PlannedCaptureBatch,
};
use crate::native::{
    note_tasks,
    task_complete::{
        req_close::ReqCloseOwner,
        req_landing::{
            resolve_req_landing, ReqLanding, ReqLandingInput, ReqRouteMove,
        },
        req_placement::{
            plan_req_links, pulled_tasks, ReqLinkInput, ReqLinkPlan,
            ReqPlacementError,
        },
        req_verdicts::ReqCardTarget,
        req_writes::ReqWritePlan,
        ClosingContext, CompletedTask,
    },
    vault_links::NoteIndex,
};

/// The close item's successor context, shared with its ordinary
/// successor planning.
pub(super) struct ReqLinkPassContext<'a> {
    pub(super) day_file: &'a Path,
    pub(super) day_relative: &'a Path,
    pub(super) today: NaiveDate,
    pub(super) index: &'a NoteIndex,
    pub(super) completed: &'a [CompletedTask],
    /// The Pomodoro close's closing context, adjusted with
    /// `req_placement::closing_after_successors`; `None` for `!`.
    pub(super) closing: Option<&'a ClosingContext>,
}

/// Record carried tasks as created by this capture, then link every
/// pulled carry at its owner's ordinary successor destination.
pub(super) fn plan_req_link_pass(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    context: &ReqLinkPassContext<'_>,
    owners: &[ReqCloseOwner],
    plan: &ReqWritePlan,
) -> Result<ReqLinkPlan, CaptureError> {
    // Birth identity first: every carried task (pulled or not) was created
    // by this capture, whatever later items do with it.
    for outcome in &plan.outcomes {
        if let Some(carried) = outcome.carried.as_ref() {
            planner.record_created_task(
                &bob_dir.join(&carried.path),
                &carried.block_id,
            );
        }
    }
    let pulled = pulled_tasks(owners, plan);
    if pulled.is_empty() {
        return Ok(ReqLinkPlan::default());
    }
    let day_text = planner.current_contents(context.day_file)?;
    let mut notes = std::collections::BTreeMap::new();
    for task in &pulled {
        if notes.contains_key(&task.path) {
            continue;
        }
        if let Some(text) =
            planner.current_contents(&bob_dir.join(&task.path))?
        {
            notes.insert(task.path.clone(), text);
        }
    }
    let settings = note_tasks::read_settings(bob_dir);
    let links = plan_req_links(
        &ReqLinkInput {
            bob_dir: Some(bob_dir),
            today: context.today,
            day_text: day_text.as_deref(),
            day_relative: context.day_relative,
            index: context.index,
            completed: context.completed,
            closing: context.closing,
            notes: &notes,
            settings: &settings,
        },
        &pulled,
    )
    .map_err(placement_error)?;
    for (relative, text) in &links.notes {
        planner.stage(&bob_dir.join(relative), text.clone())?;
    }
    if let Some(day) = links.day_text.as_ref() {
        planner.stage(context.day_file, day.clone())?;
    }
    Ok(links)
}

/// Resolve a pinned requirement row to its final landing. Task-offer pins
/// return `Ok(None)`: they keep the existing offer jump path.
pub(super) fn build_req_capture_jump(
    bob_dir: &Path,
    batch: &PlannedCaptureBatch,
    target: &ReqCardTarget,
    owners: &[ReqCloseOwner],
    plan: &ReqWritePlan,
) -> Result<Option<(CaptureJumpJson, ReqLanding)>, CaptureError> {
    let final_text = |relative: &Path| {
        super::jump::staged_text(bob_dir, batch, &bob_dir.join(relative))
    };
    req_jump_from_texts(
        bob_dir,
        &batch.routed_landings,
        &final_text,
        target,
        owners,
        plan,
    )
}

/// [`build_req_capture_jump`] over explicit final texts.
pub(super) fn req_jump_from_texts(
    bob_dir: &Path,
    routed: &[RoutedTaskMove],
    final_text: &dyn Fn(&Path) -> Option<String>,
    target: &ReqCardTarget,
    owners: &[ReqCloseOwner],
    plan: &ReqWritePlan,
) -> Result<Option<(CaptureJumpJson, ReqLanding)>, CaptureError> {
    let Some(requirement) = target.requirement.as_ref() else {
        return Ok(None);
    };
    let routes: Vec<ReqRouteMove> = routed
        .iter()
        .map(|movement| ReqRouteMove {
            source: movement.source_relative.clone(),
            dest: movement.dest_relative.clone(),
            old_block_id: movement.old_block_id.clone(),
            new_block_id: movement.new_block_id.clone(),
        })
        .collect();
    let settings = note_tasks::read_settings(bob_dir);
    let by_relative = |relative: &str| final_text(Path::new(relative));
    let landing = resolve_req_landing(
        &ReqLandingInput {
            target: requirement,
            owners,
            plan,
            routes: &routes,
            settings: &settings,
        },
        &by_relative,
    )
    .map_err(placement_error)?;
    let text = final_text(Path::new(&landing.path));
    let label = Path::new(&landing.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&landing.path)
        .to_string();
    let note = landed_note(
        OpenNoteJson {
            path: bob_dir.join(&landing.path).display().to_string(),
            relative_path: landing.path.clone(),
            role: "note",
            label,
            line: None,
            r#ref: None,
            block_id: None,
        },
        text.as_deref(),
        Some(landing.line),
    );
    if note.line.is_none() {
        return Err(placement_error(ReqPlacementError::JumpLost {
            key: requirement.key.clone(),
        }));
    }
    Ok(Some((
        CaptureJumpJson {
            key: requirement.key.clone(),
            n: target.card_row.n,
            label: requirement.text.clone(),
            outcome: landing.outcome.as_str(),
            landing: Some(note),
        },
        landing,
    )))
}

fn placement_error(error: ReqPlacementError) -> CaptureError {
    CaptureError::usage_code(error.to_string(), error.code())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::native::{
        env::TestEnvGuard,
        task_complete::{
            req_close::{collect_owner, groups_from_owners},
            req_verdicts::{
                resolve_verdicts, ReqCardRow, ReqCardRowKind, ReqVerdict,
                ReqVerdictRequest, ReqVerdictSelection,
            },
        },
    };

    const TASKS: &str = concat!(
        "- [x] #task Ship the exporter ✅ 2026-11-22 ^export\n",
        "\t- [ ] Handles supported inputs\n",
        "\t- [ ] Handles empty input\n",
    );
    const PRE_CLOSE: &str = concat!(
        "- [ ] #task Ship the exporter ^export\n",
        "\t- [ ] Handles supported inputs\n",
        "\t- [ ] Handles empty input\n",
    );

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 22).expect("date")
    }

    /// Owners frozen from the pre-close text, as tree close does.
    fn owners(bob_dir: &Path) -> Vec<ReqCloseOwner> {
        let settings = note_tasks::read_settings(bob_dir);
        let scan = note_tasks::scan(PRE_CLOSE, &settings);
        let note_tasks::BlockIdLookup::Found(task) = scan.by_block_id("export")
        else {
            panic!("owner");
        };
        vec![
            collect_owner(PRE_CLOSE, Path::new("Tasks.md"), task, &settings)
                .expect("owner snapshot"),
        ]
    }

    fn select(
        owners: &[ReqCloseOwner],
        picks: &[(&str, ReqVerdict)],
        jump: Option<&str>,
    ) -> ReqVerdictSelection {
        let groups = groups_from_owners(owners);
        let key = |text: &str| {
            groups[0]
                .rows
                .iter()
                .find(|row| row.text == text)
                .expect("row")
                .offer
                .key
                .clone()
        };
        let requests: Vec<ReqVerdictRequest> = picks
            .iter()
            .map(|(text, verdict)| ReqVerdictRequest {
                selector: key(text),
                verdict: *verdict,
                reason: None,
            })
            .collect();
        let jump = jump.map(key);
        resolve_verdicts(owners, &[], &requests, jump.as_deref())
            .expect("selection")
    }

    struct Vault {
        _dir: tempfile::TempDir,
        root: PathBuf,
        _guard: TestEnvGuard,
    }

    fn vault(day: &str) -> Vault {
        let dir = tempfile::tempdir().expect("vault");
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("Tasks.md"), TASKS).expect("tasks");
        std::fs::write(root.join("20261122.md"), day).expect("day");
        // Hermetic D7 routing: an empty config, never the host's.
        let config = root.join("config.yml");
        std::fs::write(&config, "").expect("config");
        let guard =
            TestEnvGuard::set(&[("BOB_CONFIG_FILE", Some(config.as_os_str()))]);
        Vault {
            _dir: dir,
            root,
            _guard: guard,
        }
    }

    fn completed() -> Vec<CompletedTask> {
        vec![CompletedTask {
            relative_path: PathBuf::from("Tasks.md"),
            block_id: "export".to_string(),
            task_id: String::new(),
            text: "Ship the exporter".to_string(),
            is_root: true,
        }]
    }

    fn run(
        vault: &Vault,
        planner: &mut CaptureBatchPlanner,
        selection: &ReqVerdictSelection,
    ) -> (ReqWritePlan, Result<ReqLinkPlan, CaptureError>) {
        let owners = owners(&vault.root);
        let plan = super::super::req_close::plan_req_write_pass(
            planner,
            &vault.root,
            &owners,
            selection,
            "2026-11-22",
            date(),
        )
        .expect("write pass");
        let index = NoteIndex::from_paths(
            ["Tasks.md", "20261122.md"].into_iter().map(PathBuf::from),
        );
        let completed = completed();
        let day_file = vault.root.join("20261122.md");
        let links = plan_req_link_pass(
            planner,
            &vault.root,
            &ReqLinkPassContext {
                day_file: &day_file,
                day_relative: Path::new("20261122.md"),
                today: date(),
                index: &index,
                completed: &completed,
                closing: None,
            },
            &owners,
            &plan,
        );
        (plan, links)
    }

    #[test]
    fn link_pass_stages_one_next_link_and_records_the_created_task() {
        let day = concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
            "\t- [[Tasks#^export]]\n",
        );
        let vault = vault(day);
        let mut planner = CaptureBatchPlanner::default();
        let owners = owners(&vault.root);
        let selection = select(
            &owners,
            &[
                ("Handles empty input", ReqVerdict::CarryPulled),
                ("Handles supported inputs", ReqVerdict::Carry),
            ],
            Some("Handles empty input"),
        );
        let (plan, links) = run(&vault, &mut planner, &selection);
        let links = links.expect("link pass");
        assert_eq!(links.links.len(), 1, "only the pulled carry links");
        let pulled = plan
            .outcomes
            .iter()
            .find(|outcome| outcome.pulled)
            .and_then(|outcome| outcome.carried.clone())
            .expect("pulled carry");
        let kept = plan
            .outcomes
            .iter()
            .find(|outcome| !outcome.pulled)
            .and_then(|outcome| outcome.carried.clone())
            .expect("unpulled carry");
        let day_text = planner
            .peek_text(&vault.root.join("20261122.md"))
            .expect("day staged");
        assert_eq!(
            day_text,
            format!("{day}\t- [[Tasks#^{}]]\n", pulled.block_id)
        );
        let tasks = planner
            .peek_text(&vault.root.join("Tasks.md"))
            .expect("tasks staged");
        assert!(tasks.contains(&format!(
            "- [*] #task Handles empty input [created:: 2026-11-22] ^{}",
            pulled.block_id
        )));
        assert!(tasks.contains(&format!(
            "- [ ] #task Handles supported inputs [created:: 2026-11-22] ^{}",
            kept.block_id
        )));
        assert!(!tasks.contains("[fresh::"));
        // Both carried tasks were born in this capture, pulled or not.
        assert!(planner
            .task_was_created(&vault.root.join("Tasks.md"), &pulled.block_id));
        assert!(planner
            .task_was_created(&vault.root.join("Tasks.md"), &kept.block_id));
        // Nothing reached disk: the single commit still owns the write.
        assert_eq!(
            std::fs::read_to_string(vault.root.join("Tasks.md")).unwrap(),
            TASKS
        );

        // The pinned pulled row lands on the task it became in the final
        // staged text, never on its `[>]` stub.
        let batch = staged_batch(planner);
        let (json, landing) = build_req_capture_jump(
            &vault.root,
            &batch,
            selection.jump.as_ref().expect("pin"),
            &owners,
            &plan,
        )
        .expect("jump resolves")
        .expect("requirement pin");
        assert_eq!(landing.kind.as_str(), "carried_task");
        assert_eq!(json.outcome, "carry_pulled");
        let note = json.landing.expect("landing");
        assert_eq!(note.relative_path, "Tasks.md");
        assert_eq!(note.block_id.as_deref(), Some(pulled.block_id.as_str()));
        let line = note.line.expect("line");
        assert!(tasks
            .lines()
            .nth(line - 1)
            .expect("landing line")
            .starts_with("- [*] #task Handles empty input"));
    }

    fn staged_batch(mut planner: CaptureBatchPlanner) -> PlannedCaptureBatch {
        let read_guards = std::mem::take(&mut planner.read_guards);
        PlannedCaptureBatch {
            items: Vec::new(),
            text_files: planner.into_staged_files().expect("staged files"),
            read_guards,
            global_destination: None,
            offer_closes: Vec::new(),
            warnings: Vec::new(),
            plan_budget: None,
            pomodoro_blocks: Vec::new(),
            task_blocks: Vec::new(),
            section_blocks: Vec::new(),
            move_choice: super::super::move_choice::MoveChoiceReport::empty(),
            inbox_route: super::super::inbox_route::InboxRouteReport::empty(),
            routed_landings: Vec::new(),
            template: None,
            template_rows: Vec::new(),
            type_agreement: Vec::new(),
            staged_jobs: Vec::new(),
            catalog_walks: 0,
            snapshot_read_counts: std::collections::BTreeMap::new(),
            record_folders: Vec::new(),
            record_files_opened: 0,
            album_files: Vec::new(),
            walk_files: Vec::new(),
        }
    }

    #[test]
    fn link_pass_without_a_destination_refuses_and_links_nothing() {
        let day =
            concat!("## Pomodoros\n", "- [x] (**0850-0915**) \u{2014} MAIL\n",);
        let vault = vault(day);
        let mut planner = CaptureBatchPlanner::default();
        let owners = owners(&vault.root);
        let selection = select(
            &owners,
            &[("Handles empty input", ReqVerdict::CarryPulled)],
            None,
        );
        let (_, links) = run(&vault, &mut planner, &selection);
        let error = links.expect_err("no destination");
        assert_eq!(error.code.as_deref(), Some("req_pull_in_no_destination"));
        assert_eq!(
            planner
                .peek_text(&vault.root.join("20261122.md"))
                .as_deref(),
            Some(day),
            "the day file gained no link"
        );
    }

    #[test]
    fn requirement_jump_follows_routes_and_task_offer_pins_defer() {
        let vault = vault("## Pomodoros\n");
        let owners = owners(&vault.root);
        let selection = select(&owners, &[], Some("Handles empty input"));
        let target = selection.jump.clone().expect("pin");
        let plan = ReqWritePlan::default();
        let finals = std::collections::BTreeMap::from([
            (PathBuf::from("Tasks.md"), "## Tasks\n".to_string()),
            (
                PathBuf::from("Projects/Alpha.md"),
                format!("## Tasks\n{TASKS}"),
            ),
        ]);
        let final_text =
            |relative: &Path| finals.get(&relative.to_path_buf()).cloned();
        let routed = vec![RoutedTaskMove {
            candidate: 0,
            source: vault.root.join("Tasks.md"),
            source_relative: "Tasks.md".to_string(),
            old_block_id: Some("export".to_string()),
            old_line: 1,
            dest: vault.root.join("Projects/Alpha.md"),
            dest_relative: "Projects/Alpha.md".to_string(),
            dest_route: "Projects/Alpha".to_string(),
            new_block_id: Some("export".to_string()),
            new_line: 2,
            repaired_links: 0,
            repaired_dep_tokens: 0,
        }];
        let (json, landing) = req_jump_from_texts(
            &vault.root,
            &routed,
            &final_text,
            &target,
            &owners,
            &plan,
        )
        .expect("jump resolves")
        .expect("requirement pin");
        assert_eq!(landing.kind.as_str(), "requirement");
        assert_eq!(json.key, target.card_row.key);
        assert_eq!(json.n, 2);
        assert_eq!(json.label, "Handles empty input");
        assert_eq!(json.outcome, "met");
        let note = json.landing.expect("landing");
        assert_eq!(note.relative_path, "Projects/Alpha.md");
        assert_eq!(note.line, Some(4));
        assert!(note.r#ref.is_some());

        let offer_pin = ReqCardTarget {
            card_row: ReqCardRow {
                key: "Tasks.md#^other".to_string(),
                n: 3,
                kind: ReqCardRowKind::TaskOffer,
            },
            requirement: None,
        };
        assert!(req_jump_from_texts(
            &vault.root,
            &routed,
            &final_text,
            &offer_pin,
            &owners,
            &plan,
        )
        .expect("defers")
        .is_none());
    }
}
