//! Coordinated PDF marker writes, holds, and the in-accessor re-check.
use super::pdf_write_policy::{
    PdfWritePolicy, ReadingActivity, ReadingHoldMode,
};
use super::*;
use crate::native::config as bob_config;
#[cfg(test)]
use crate::native::file_coordination::MoveNotifier;
use crate::native::file_coordination::{self, Coordination, MoveContext};
use std::time::Duration;

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Bytes, annotation id, and digest resolved inside a coordinated accessor
/// before the first write for that PDF.
pub(super) struct ResolvedMarkerWrite {
    pub(super) bytes: Vec<u8>,
    pub(super) annotation_id: ObjectId,
    pub(super) editor_saved_first: bool,
    pub(super) digest: String,
}

/// Real macOS coordination, a pass-through, or a test fake.
pub(super) enum MarkerWriteCoordinator {
    Platform,
    Direct,
    #[cfg(test)]
    Fake(FakeCoordinator),
}

impl MarkerWriteCoordinator {
    pub(super) fn is_coordinated(&self) -> bool {
        match self {
            Self::Platform => file_coordination::IS_COORDINATED,
            Self::Direct => false,
            #[cfg(test)]
            Self::Fake(_) => true,
        }
    }

    pub(super) fn run<T>(
        &self,
        path: &Path,
        budget: Duration,
        accessor: impl FnOnce() -> T,
    ) -> Coordination<T> {
        match self {
            Self::Platform | Self::Direct => {
                file_coordination::coordinate_merging_write(
                    path, budget, accessor,
                )
            }
            #[cfg(test)]
            Self::Fake(fake) => fake.run(path, budget, accessor),
        }
    }

    pub(super) fn run_move<T>(
        &self,
        from: &Path,
        to: &Path,
        budget: Duration,
        accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
    ) -> Coordination<T> {
        match self {
            Self::Platform => file_coordination::coordinate_moving_write(
                from, to, budget, accessor,
            ),
            Self::Direct => {
                file_coordination::move_directly(from, to, accessor)
            }
            #[cfg(test)]
            Self::Fake(fake) => fake.run_move(from, to, budget, accessor),
        }
    }
}

/// Per-command coordination state: policy, coordinator, and spent budget.
pub(super) struct MarkerWriteSession {
    pub(super) policy: PdfWritePolicy,
    coordinator: MarkerWriteCoordinator,
    spent: Duration,
}

impl MarkerWriteSession {
    pub(super) fn new(
        policy: PdfWritePolicy,
        coordinator: MarkerWriteCoordinator,
    ) -> Self {
        Self {
            policy,
            coordinator,
            spent: Duration::ZERO,
        }
    }

    pub(super) fn from_config(reading_hold: ReadingHoldMode) -> Result<Self> {
        let config =
            bob_config::load_highlights_config(&bob_config::config_path())
                .map_err(config_error)?;
        Ok(Self::new(
            PdfWritePolicy::from_config(&config)
                .with_reading_hold(reading_hold),
            MarkerWriteCoordinator::Platform,
        ))
    }

    pub(super) fn for_scan(attended: bool) -> Result<Self> {
        let mode = if attended {
            ReadingHoldMode::Exempt
        } else {
            ReadingHoldMode::Evaluate
        };
        Self::from_config(mode)
    }

    pub(super) fn for_explicit_write() -> Result<Self> {
        Self::from_config(ReadingHoldMode::Exempt)
    }

    /// Explicit overwrite owns one outer file-coordination scope for the
    /// staged replacement and its marker sync. The inner sync must not ask
    /// NSFileCoordinator for the same PDF a second time.
    pub(super) fn for_external_coordination() -> Self {
        Self::new(
            PdfWritePolicy::from_config(
                &bob_config::HighlightsConfig::default(),
            )
            .with_reading_hold(ReadingHoldMode::Exempt),
            MarkerWriteCoordinator::Direct,
        )
    }

    pub(super) fn uncoordinated() -> Self {
        Self::new(
            PdfWritePolicy::from_config(
                &bob_config::HighlightsConfig::default(),
            ),
            MarkerWriteCoordinator::Direct,
        )
    }

    pub(super) fn coordinates(&self) -> bool {
        self.policy.coordinate_pdf_writes && self.coordinator.is_coordinated()
    }

    fn next_budget(&self) -> Option<Duration> {
        let remaining = self.policy.scan_budget.saturating_sub(self.spent);
        if remaining.is_zero() {
            None
        } else {
            Some(remaining.min(self.policy.per_pdf_budget))
        }
    }

    /// Use this session's policy and coordinator for one bounded explicit
    /// write. The instance form also lets transaction tests inject a fake
    /// save/hold without introducing a production environment override.
    pub(super) fn coordinate_explicit_pdf_write_with<T>(
        &self,
        pdf: &Path,
        accessor: impl FnOnce() -> T,
    ) -> Result<Coordination<T>> {
        if !self.policy.coordinate_pdf_writes || !self.coordinates() {
            return Ok(Coordination::Granted {
                value: accessor(),
                waited: Duration::ZERO,
            });
        }
        let Some(budget) = self.next_budget() else {
            return Ok(Coordination::Held(file_coordination::TimedOut {
                waited: Duration::ZERO,
            }));
        };
        Ok(self.coordinator.run(pdf, budget, accessor))
    }

    /// Use this session's policy and coordinator for one planned, reversible
    /// move. The context is valid only inside the accessor, so both forward
    /// and rollback moves share one bounded coordination request.
    #[allow(dead_code)] // Used by the dependent rename transaction phase.
    pub(super) fn coordinate_explicit_pdf_move_with<T>(
        &self,
        from: &Path,
        to: &Path,
        accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
    ) -> Result<Coordination<T>> {
        if !self.policy.coordinate_pdf_writes || !self.coordinates() {
            return Ok(file_coordination::move_directly(from, to, accessor));
        }
        let Some(budget) = self.next_budget() else {
            return Ok(Coordination::Held(file_coordination::TimedOut {
                waited: Duration::ZERO,
            }));
        };
        Ok(self.coordinator.run_move(from, to, budget, accessor))
    }

    fn record_wait(&mut self, waited: Duration) {
        self.spent = self.spent.saturating_add(waited);
    }

    pub(super) fn reading_hold_report(
        &self,
        plans: &[&PdfSyncPlan],
        dry_run: bool,
    ) -> String {
        if dry_run {
            return "n/a".to_string();
        }
        match self.policy.reading_hold {
            ReadingHoldMode::Exempt => "attended".to_string(),
            ReadingHoldMode::Evaluate => {
                if !plans.iter().any(|plan| plan.marker_write_needed) {
                    return "n/a".to_string();
                }
                match self.policy.reading_activity() {
                    ReadingActivity::Reading { idle }
                        if *idle < self.policy.reading_window =>
                    {
                        format!(
                            "holding (Highlights frontmost, last input {} s ago)",
                            idle.as_secs()
                        )
                    }
                    ReadingActivity::Unknown(why) => {
                        format!("unknown ({why})")
                    }
                    ReadingActivity::Reading { .. }
                    | ReadingActivity::Away { .. } => "clear".to_string(),
                }
            }
        }
    }
}

pub(super) fn hold_message(
    reason: HoldReason,
    policy: &PdfWritePolicy,
    cap_spent: bool,
) -> String {
    match reason {
        HoldReason::EditorActive => {
            "waiting while you read in Highlights · retrying next scan"
                .to_string()
        }
        HoldReason::CoordinationTimeout if cap_spent => format!(
            "Highlights didn't answer this scan ({} s spent) · retrying next scan",
            policy.scan_budget.as_secs()
        ),
        HoldReason::CoordinationTimeout => format!(
            "Highlights didn't answer within {} s (a dialog may be open) · retrying next scan",
            policy.per_pdf_budget.as_secs()
        ),
        HoldReason::MarkerChanged => {
            "marker changed in Highlights during the scan · retrying next scan"
                .to_string()
        }
        HoldReason::ReplacedCapture => {
            "Highlights wrote the replaced capture back".to_string()
        }
    }
}

pub(super) fn hold_verbose_message(message: &str) -> String {
    message.replace(" · ", "; ")
}

pub(super) fn scan_hold_for(
    plan: &PdfSyncPlan,
    reason: HoldReason,
    message: String,
) -> ScanHold {
    ScanHold {
        pdf: plan.pdf.clone(),
        note_path: plan.note_path.clone(),
        title: note_title(&plan.pdf, &plan.synced_projection),
        reason,
        message,
    }
}

pub(super) fn marker_status_change(
    plan: &PdfSyncPlan,
) -> Option<(String, String)> {
    let from = marker_status_value(&plan.marker.contents)?;
    let to = marker_status_value(&plan.rendered_marker)?;
    (from != to).then_some((from, to))
}

fn marker_status_value(contents: &str) -> Option<String> {
    match parse_marker(contents).ok()?.get(FIELD_STATUS)? {
        MarkerValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(super) fn pdf_marker_action_display(report: &SyncWriteReport) -> String {
    match report.marker_write.as_ref() {
        Some(info) if info.mode == MarkerWriteMode::Coordinated => {
            let mut label = format!(
                "updated (coordinated in {:.2} s",
                info.waited.as_secs_f64()
            );
            if info.editor_saved_first {
                label.push_str("; unsaved annotations saved first");
            }
            label.push(')');
            label
        }
        _ => report.marker_action.to_string(),
    }
}

pub(super) fn write_planned_pdf_marker(
    plan: &PdfSyncPlan,
    marker_prep: Option<&ResolvedMarkerWrite>,
) -> Result<()> {
    match marker_prep {
        Some(prep) => {
            let current = sha256_file(&plan.pdf)?;
            if current != prep.digest {
                return Err(CommandError::new(format!(
                    "PDF changed during sync; rerun: {}",
                    plan.pdf.display()
                )));
            }
            write_pdf_marker_from_bytes(
                &plan.pdf,
                &prep.bytes,
                prep.annotation_id,
                &plan.rendered_marker,
            )
        }
        None => {
            ensure_pdf_unchanged_for_write(plan)?;
            write_pdf_marker(
                &plan.pdf,
                plan.marker.annotation_id,
                &plan.rendered_marker,
            )
        }
    }
}

enum Recheck {
    Write(ResolvedMarkerWrite),
    Hold(ScanHold),
}

fn recheck_marker(
    plan: &PdfSyncPlan,
    policy: &PdfWritePolicy,
) -> Result<Recheck> {
    let bytes = fs::read(&plan.pdf).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", plan.pdf.display()))
    })?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest == plan.marker.source_pdf_sha256 {
        return Ok(Recheck::Write(ResolvedMarkerWrite {
            annotation_id: plan.marker.annotation_id,
            editor_saved_first: false,
            digest,
            bytes,
        }));
    }
    let fresh = read_pdf_marker_from_bytes(&plan.pdf, &bytes)?;
    if normalize_line_endings(&fresh.contents)
        == normalize_line_endings(&plan.marker.contents)
    {
        return Ok(Recheck::Write(ResolvedMarkerWrite {
            annotation_id: fresh.annotation_id,
            editor_saved_first: true,
            digest,
            bytes,
        }));
    }
    Ok(Recheck::Hold(scan_hold_for(
        plan,
        HoldReason::MarkerChanged,
        hold_message(HoldReason::MarkerChanged, policy, false),
    )))
}

fn attach_marker_write(
    mut report: SyncWriteReport,
    plan: &PdfSyncPlan,
    mode: MarkerWriteMode,
    editor_saved_first: bool,
    waited: Duration,
) -> SyncWriteReport {
    if plan.marker_write_needed && report.marker_action != "none" {
        report.marker_write = Some(MarkerWriteInfo {
            mode,
            editor_saved_first,
            status_change: marker_status_change(plan),
            waited,
            restored: plan.stale_restore,
        });
    }
    report
}

/// Run one PDF's write sequence, wrapping marker writes when the policy is on.
pub(super) fn execute_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
    session: &mut MarkerWriteSession,
) -> Result<PdfSyncOutcome> {
    if let Some(hold) = super::sync::replaced_capture_hold(config, &plan.pdf)? {
        return Ok(PdfSyncOutcome::Held(hold));
    }
    if !plan.marker_write_needed || !session.policy.coordinate_pdf_writes {
        return execute_pdf_sync_inner(config, plan, run_state, None).map(
            |report| {
                PdfSyncOutcome::Written(attach_marker_write(
                    report,
                    plan,
                    MarkerWriteMode::Direct,
                    false,
                    Duration::ZERO,
                ))
            },
        );
    }

    if session.policy.should_hold_for_reading() {
        return Ok(PdfSyncOutcome::Held(scan_hold_for(
            plan,
            HoldReason::EditorActive,
            hold_message(HoldReason::EditorActive, &session.policy, false),
        )));
    }

    if !session.coordinates() {
        return execute_pdf_sync_inner(config, plan, run_state, None).map(
            |report| {
                PdfSyncOutcome::Written(attach_marker_write(
                    report,
                    plan,
                    MarkerWriteMode::Direct,
                    false,
                    Duration::ZERO,
                ))
            },
        );
    }

    let Some(budget) = session.next_budget() else {
        return Ok(PdfSyncOutcome::Held(scan_hold_for(
            plan,
            HoldReason::CoordinationTimeout,
            hold_message(
                HoldReason::CoordinationTimeout,
                &session.policy,
                true,
            ),
        )));
    };

    let result = {
        let policy = &session.policy;
        let coordinator = &session.coordinator;
        coordinator.run(&plan.pdf, budget, || {
            // The coordinator may have asked Highlights to save just before
            // this callback. Check the archived digest again before marker
            // reconciliation or any per-PDF note/task/asset work.
            if let Some(hold) =
                super::sync::replaced_capture_hold(config, &plan.pdf)?
            {
                return Ok(PdfSyncOutcome::Held(hold));
            }
            match recheck_marker(plan, policy)? {
                Recheck::Hold(hold) => Ok(PdfSyncOutcome::Held(hold)),
                Recheck::Write(prep) => {
                    execute_pdf_sync_inner(config, plan, run_state, Some(&prep))
                        .map(|report| {
                            PdfSyncOutcome::Written(attach_marker_write(
                                report,
                                plan,
                                MarkerWriteMode::Coordinated,
                                prep.editor_saved_first,
                                Duration::ZERO,
                            ))
                        })
                }
            }
        })
    };
    match result {
        Coordination::Granted { value, waited } => {
            session.record_wait(waited);
            match value {
                Ok(PdfSyncOutcome::Written(mut report)) => {
                    if let Some(info) = report.marker_write.as_mut() {
                        info.waited = waited;
                    }
                    Ok(PdfSyncOutcome::Written(report))
                }
                other => other,
            }
        }
        Coordination::Held(timeout) => {
            session.record_wait(timeout.waited);
            Ok(PdfSyncOutcome::Held(scan_hold_for(
                plan,
                HoldReason::CoordinationTimeout,
                hold_message(
                    HoldReason::CoordinationTimeout,
                    &session.policy,
                    false,
                ),
            )))
        }
        Coordination::Failed(message) => Err(CommandError::new(format!(
            "file coordination failed: {message}; nothing was written"
        ))),
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct FakeCoordinator {
    action: FakeAction,
    calls: Arc<Mutex<usize>>,
    move_events: Arc<Mutex<Vec<FakeMoveEvent>>>,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FakeMoveEvent {
    Will(PathBuf, PathBuf),
    Did(PathBuf, PathBuf),
}

#[cfg(test)]
struct FakeMoveNotifier(Arc<Mutex<Vec<FakeMoveEvent>>>);

#[cfg(test)]
impl MoveNotifier for FakeMoveNotifier {
    fn will_move(&self, from: &Path, to: &Path) {
        self.0
            .lock()
            .expect("fake move events")
            .push(FakeMoveEvent::Will(from.to_path_buf(), to.to_path_buf()));
    }

    fn did_move(&self, from: &Path, to: &Path) {
        self.0
            .lock()
            .expect("fake move events")
            .push(FakeMoveEvent::Did(from.to_path_buf(), to.to_path_buf()));
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(super) enum FakeAction {
    Grant { waited: Duration },
    GrantAfterSave { waited: Duration, bytes: Vec<u8> },
    Hold { waited: Duration },
    Fail { message: String },
}

#[cfg(test)]
impl FakeCoordinator {
    pub(super) fn grant(waited: Duration) -> Self {
        Self {
            action: FakeAction::Grant { waited },
            calls: Arc::new(Mutex::new(0)),
            move_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn grant_after_save(waited: Duration, bytes: Vec<u8>) -> Self {
        Self {
            action: FakeAction::GrantAfterSave { waited, bytes },
            calls: Arc::new(Mutex::new(0)),
            move_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn hold(waited: Duration) -> Self {
        Self {
            action: FakeAction::Hold { waited },
            calls: Arc::new(Mutex::new(0)),
            move_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn fail(message: impl Into<String>) -> Self {
        Self {
            action: FakeAction::Fail {
                message: message.into(),
            },
            calls: Arc::new(Mutex::new(0)),
            move_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn call_count(&self) -> usize {
        *self.calls.lock().expect("fake coordinator calls")
    }

    pub(super) fn move_events(&self) -> Vec<FakeMoveEvent> {
        self.move_events.lock().expect("fake move events").clone()
    }

    fn run<T>(
        &self,
        path: &Path,
        budget: Duration,
        accessor: impl FnOnce() -> T,
    ) -> Coordination<T> {
        let _ = (path, budget);
        *self.calls.lock().expect("fake coordinator calls") += 1;
        match &self.action {
            FakeAction::Grant { waited } => Coordination::Granted {
                value: accessor(),
                waited: *waited,
            },
            FakeAction::GrantAfterSave { waited, bytes } => {
                std::fs::write(path, bytes)
                    .expect("simulate Highlights saving before access");
                Coordination::Granted {
                    value: accessor(),
                    waited: *waited,
                }
            }
            FakeAction::Hold { waited } => {
                Coordination::Held(file_coordination::TimedOut {
                    waited: *waited,
                })
            }
            FakeAction::Fail { message } => {
                Coordination::Failed(message.clone())
            }
        }
    }

    fn run_move<T>(
        &self,
        from: &Path,
        to: &Path,
        budget: Duration,
        accessor: impl FnOnce(&mut MoveContext<'_>) -> T,
    ) -> Coordination<T> {
        let _ = (to, budget);
        *self.calls.lock().expect("fake coordinator calls") += 1;
        match &self.action {
            FakeAction::Grant { waited } => {
                let notifier = FakeMoveNotifier(Arc::clone(&self.move_events));
                Coordination::Granted {
                    value: file_coordination::move_with_notifier_for_test(
                        from, to, &notifier, accessor,
                    ),
                    waited: *waited,
                }
            }
            FakeAction::GrantAfterSave { waited, bytes } => {
                std::fs::write(from, bytes)
                    .expect("simulate presenter saving before move access");
                let notifier = FakeMoveNotifier(Arc::clone(&self.move_events));
                Coordination::Granted {
                    value: file_coordination::move_with_notifier_for_test(
                        from, to, &notifier, accessor,
                    ),
                    waited: *waited,
                }
            }
            FakeAction::Hold { waited } => {
                Coordination::Held(file_coordination::TimedOut {
                    waited: *waited,
                })
            }
            FakeAction::Fail { message } => {
                Coordination::Failed(message.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn default_policy() -> PdfWritePolicy {
        PdfWritePolicy::from_config(
            &crate::native::config::HighlightsConfig::default(),
        )
    }

    fn hold_verbose_phrase(
        reason: HoldReason,
        policy: &PdfWritePolicy,
        cap_spent: bool,
    ) -> String {
        hold_verbose_message(&hold_message(reason, policy, cap_spent))
    }

    #[test]
    fn hold_copy_and_reason_ids_match_the_contract() {
        let policy = default_policy();
        assert_eq!(HoldReason::EditorActive.as_str(), "editor_active");
        assert_eq!(
            HoldReason::CoordinationTimeout.as_str(),
            "coordination_timeout"
        );
        assert_eq!(HoldReason::MarkerChanged.as_str(), "marker_changed");

        assert_eq!(
            hold_message(HoldReason::EditorActive, &policy, false),
            "waiting while you read in Highlights · retrying next scan"
        );
        assert_eq!(
            hold_verbose_phrase(HoldReason::EditorActive, &policy, false),
            "waiting while you read in Highlights; retrying next scan"
        );
        assert_eq!(
            hold_message(HoldReason::CoordinationTimeout, &policy, false),
            "Highlights didn't answer within 10 s (a dialog may be open) · retrying next scan"
        );
        assert_eq!(
            hold_verbose_phrase(HoldReason::CoordinationTimeout, &policy, false),
            "Highlights didn't answer within 10 s (a dialog may be open); retrying next scan"
        );
        assert_eq!(
            hold_message(HoldReason::CoordinationTimeout, &policy, true),
            "Highlights didn't answer this scan (30 s spent) · retrying next scan"
        );
        assert_eq!(
            hold_verbose_phrase(HoldReason::CoordinationTimeout, &policy, true),
            "Highlights didn't answer this scan (30 s spent); retrying next scan"
        );
        assert_eq!(
            hold_message(HoldReason::MarkerChanged, &policy, false),
            "marker changed in Highlights during the scan · retrying next scan"
        );
        assert_eq!(
            hold_verbose_phrase(HoldReason::MarkerChanged, &policy, false),
            "marker changed in Highlights during the scan; retrying next scan"
        );
    }

    fn report_with(info: Option<MarkerWriteInfo>) -> SyncWriteReport {
        SyncWriteReport {
            note_action: "update",
            marker_action: "updated",
            image_count: 0,
            image_note_count: 0,
            image_notes_unresolved: Vec::new(),
            continuation_count: 0,
            dangling_continuations: Vec::new(),
            underlines_merged: 0,
            standalone_underlines: Vec::new(),
            section_diagnostics: Vec::new(),
            section_count: 0,
            image_assets_written: 0,
            image_assets_skipped: 0,
            routed_note_actions: 0,
            annotation_tasks_created: 0,
            annotation_tasks_skipped: 0,
            reading_created: None,
            reading_updated: None,
            marker_write: info,
        }
    }

    #[test]
    fn coordinated_marker_action_includes_wait_and_saved_first() {
        let direct = report_with(Some(MarkerWriteInfo {
            mode: MarkerWriteMode::Direct,
            editor_saved_first: false,
            status_change: None,
            waited: Duration::from_millis(420),
            restored: false,
        }));
        assert_eq!(pdf_marker_action_display(&direct), "updated");

        let coordinated = report_with(Some(MarkerWriteInfo {
            mode: MarkerWriteMode::Coordinated,
            editor_saved_first: false,
            status_change: None,
            waited: Duration::from_millis(30),
            restored: false,
        }));
        assert_eq!(
            pdf_marker_action_display(&coordinated),
            "updated (coordinated in 0.03 s)"
        );

        let saved = report_with(Some(MarkerWriteInfo {
            mode: MarkerWriteMode::Coordinated,
            editor_saved_first: true,
            status_change: Some(("wip".into(), "read".into())),
            waited: Duration::from_millis(420),
            restored: false,
        }));
        assert_eq!(
            pdf_marker_action_display(&saved),
            "updated (coordinated in 0.42 s; unsaved annotations saved first)"
        );
    }
}
