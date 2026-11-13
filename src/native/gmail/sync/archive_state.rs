//! Body-free, cross-host state for one-shot Gmail archive-on-close.
//!
//! A close is automatic only after the journal has a valid open observation
//! in the current enablement epoch. First-seen closed evidence is classified
//! as historical backlog and needs the preview phase's explicit confirmation.
//! Every terminal outcome names one close generation, so old host journals
//! cannot consume or recreate ownership for a later close.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{
    archive_scan::{ArchiveLinkTask, ArchiveScan, ArchiveThreadEvidence},
    ThreadKey,
};
use crate::native::{
    gmail::journal::NAMESPACE,
    pref_journal::{self, StoredEvent, MAX_MALFORMED_WARNINGS},
};

const ENABLED_KIND: &str = "gmail.archive_enabled";
const OBSERVED_KIND: &str = "gmail.archive_observed";
const CLOSED_KIND: &str = "gmail.archive_closed";
const ARCHIVED_KIND: &str = "gmail.archived";
const KEPT_KIND: &str = "gmail.kept";
const LET_GO_KIND: &str = "gmail.let_go";
const RESTORED_KIND: &str = "gmail.restored";

/// A Gmail receipt label's stable identity. The name is always recorded;
/// the remote ID is present after the label has been resolved or created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveLabelIdentity {
    pub(crate) name: String,
    pub(crate) id: Option<String>,
}

impl ArchiveLabelIdentity {
    pub(crate) fn new(
        name: impl Into<String>,
        id: Option<String>,
    ) -> Option<Self> {
        let identity = Self {
            name: name.into(),
            id,
        };
        valid_label(&identity).then_some(identity)
    }
}

/// Body-free locator for one task row linking the mail note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ArchiveTaskLocator {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) status: char,
}

impl From<&ArchiveLinkTask> for ArchiveTaskLocator {
    fn from(task: &ArchiveLinkTask) -> Self {
        Self {
            path: task.path.clone(),
            line: task.line,
            block_id: task.block_id.clone(),
            status: task.status,
        }
    }
}

/// Persisted close evidence. It contains paths, line numbers, saved IDs,
/// fingerprints, and label identity; task prose, subjects, and bodies never
/// enter this journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveCloseGeneration {
    pub(crate) key: ThreadKey,
    pub(crate) generation: String,
    pub(crate) enable_epoch: String,
    pub(crate) note: String,
    pub(crate) tasks: Vec<ArchiveTaskLocator>,
    pub(crate) saved_ids: Vec<String>,
    pub(crate) note_fingerprint: String,
    pub(crate) label: ArchiveLabelIdentity,
    pub(crate) historical: bool,
    pub(crate) confirmation_digest: Option<String>,
    /// The `gmail.archive_observed` event that proved the prior open state.
    pub(crate) opened_by: Option<String>,
    pub(crate) closed_at: Option<DateTime<FixedOffset>>,
}

/// The result of projecting one thread against the current scan and journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveDisposition {
    Disabled,
    Held,
    Watching,
    HistoricalBacklog,
    Ready {
        close: ArchiveCloseGeneration,
        needs_record: bool,
    },
    Consumed {
        close: ArchiveCloseGeneration,
        outcome: ArchiveTerminalKind,
    },
}

/// One terminal outcome that consumes a close generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveTerminalKind {
    Archived {
        message_ids: Vec<String>,
        label: ArchiveLabelIdentity,
        recovered: bool,
    },
    Kept {
        /// A bounded slug such as `unsaved`, `consent_removed`, or `manual`.
        reason: String,
    },
    LetGo {
        message_ids: Vec<String>,
        label: ArchiveLabelIdentity,
    },
    Restored {
        message_ids: Vec<String>,
        label: ArchiveLabelIdentity,
    },
}

/// One accepted terminal record. `event_id` lets release events name the
/// exact archived receipt they consume, even when host clocks disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveTerminalRecord {
    pub(crate) key: ThreadKey,
    pub(crate) generation: String,
    pub(crate) event_id: String,
    archive_event_id: Option<String>,
    pub(crate) kind: ArchiveTerminalKind,
    pub(crate) ts: DateTime<FixedOffset>,
}

/// An owned, still-live archive receipt, suitable for the later undo phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveReceipt {
    pub(crate) key: ThreadKey,
    pub(crate) generation: String,
    pub(crate) message_ids: Vec<String>,
    pub(crate) label: ArchiveLabelIdentity,
    pub(crate) recovered: bool,
    pub(crate) archived_at: DateTime<FixedOffset>,
    pub(crate) archive_event_id: String,
}

/// The accepted state of one archived generation, including generations
/// whose ownership has since ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveHistoryState {
    Archived,
    LetGo,
    Restored,
}

/// Body-free historical archive receipt paired with its frozen note locator.
/// This is deliberately distinct from `ArchiveReceipt`, which means current
/// ownership and is the only receipt suitable for restore actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveHistoryRecord {
    pub(crate) receipt: ArchiveReceipt,
    pub(crate) note: String,
    pub(crate) state: ArchiveHistoryState,
}

/// Distinct accepted terminal outcomes in the requested interval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ArchiveCounts {
    pub(crate) archived: usize,
    pub(crate) kept: usize,
    pub(crate) let_go: usize,
    pub(crate) restored: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Enablement {
    account: String,
    enabled: bool,
    epoch: String,
    change_id: String,
    previous_change_id: Option<String>,
    label: ArchiveLabelIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Observation {
    Open {
        enable_epoch: String,
        observation_id: String,
        after_generation: Option<String>,
    },
    Closed {
        enable_epoch: String,
        generation: String,
    },
}

#[derive(Debug, Clone, Default)]
struct GenerationState {
    close: Option<ArchiveCloseGeneration>,
    first_terminal: Option<ArchiveTerminalRecord>,
    receipt: Option<ArchiveReceipt>,
    release: Option<ArchiveTerminalRecord>,
    terminal_history: Vec<ArchiveTerminalRecord>,
    pending_releases: Vec<ArchiveTerminalRecord>,
}

impl GenerationState {
    fn consumed(&self) -> bool {
        self.first_terminal.is_some() || !self.pending_releases.is_empty()
    }

    fn outcome(&self) -> Option<ArchiveTerminalKind> {
        self.release
            .as_ref()
            .map(|record| record.kind.clone())
            .or_else(|| {
                self.first_terminal
                    .as_ref()
                    .map(|record| record.kind.clone())
            })
    }
}

#[derive(Debug, Clone, Default)]
struct ThreadState {
    observation: Option<Observation>,
    generations: BTreeMap<String, GenerationState>,
}

/// Folded `.bob/gmail/*.jsonl` archive state across all hosts.
#[derive(Debug, Clone, Default)]
pub(crate) struct ArchiveJournal {
    enablement: BTreeMap<String, Enablement>,
    threads: BTreeMap<ThreadKey, ThreadState>,
    unsafe_all: bool,
    unsafe_accounts: BTreeSet<String>,
    unsafe_threads: BTreeSet<ThreadKey>,
    warnings: Vec<String>,
}

/// A journal append prepared by a pure projection. Preview callers may
/// inspect the event and deliberately refrain from appending it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedArchiveEvent {
    kind: &'static str,
    value: Value,
}

impl PlannedArchiveEvent {
    fn new(kind: &'static str, value: Value) -> Self {
        Self { kind, value }
    }

    #[cfg(test)]
    fn stored(&self, host: &str, line: u32, second: i64) -> StoredEvent {
        use chrono::{TimeZone, Utc};
        StoredEvent {
            ts: Utc.timestamp_opt(second, 0).unwrap().fixed_offset(),
            host: host.to_string(),
            path: format!("{host}.jsonl"),
            line,
            kind: self.kind.to_string(),
            value: json!({"value": self.value}),
        }
    }
}

impl ArchiveJournal {
    /// Read and fold all Gmail journal files. Unreadable files fail the
    /// load; malformed envelopes or archive events mark state unsafe so a
    /// caller can hold affected work instead of treating it as unconsumed.
    pub(crate) fn load(vault: &Path) -> io::Result<Self> {
        ensure_gmail_journals_readable(vault)?;
        let (events, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
        Ok(Self::from_events(events, warnings))
    }

    fn from_events(
        mut events: Vec<StoredEvent>,
        warnings: Vec<String>,
    ) -> Self {
        pref_journal::sort_by_writer_order(&mut events, |event| {
            (
                event.ts,
                event.host.as_str(),
                event.path.as_str(),
                event.line,
            )
        });
        let mut state = Self {
            unsafe_all: !warnings.is_empty(),
            warnings: warnings
                .into_iter()
                .take(MAX_MALFORMED_WARNINGS)
                .collect(),
            ..Self::default()
        };
        let mut terminal_events = Vec::new();

        for event in events {
            match event.kind.as_str() {
                ENABLED_KIND => match parse_enablement(&event) {
                    Some(parsed) => state.apply_enablement(parsed),
                    None => state.mark_malformed(&event),
                },
                OBSERVED_KIND => match parse_open_observation(&event) {
                    Some(parsed) => state.apply_open_observation(parsed),
                    None => state.mark_malformed(&event),
                },
                CLOSED_KIND => match parse_close_generation(&event) {
                    Some(parsed) => state.apply_close(parsed, &event),
                    None => state.mark_malformed(&event),
                },
                ARCHIVED_KIND | KEPT_KIND | LET_GO_KIND | RESTORED_KIND => {
                    match parse_terminal(&event) {
                        Some(parsed) => terminal_events.push(parsed),
                        None => state.mark_malformed(&event),
                    }
                }
                kind if kind.starts_with("gmail.archive_") => {
                    state.mark_malformed(&event)
                }
                _ => {}
            }
        }

        // Terminal transitions are folded after their referenced close
        // generations, so clock skew cannot make a terminal appear to
        // precede its close. Release records also name the exact archive
        // event they consume.
        for terminal in terminal_events {
            state.apply_terminal(terminal);
        }
        state.finish_pending_releases();
        state
    }

    /// Bounded, body-free warnings from unreadable journal state.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether an account's archive state is safe to use.
    pub(crate) fn account_is_safe(&self, account: &str) -> bool {
        !self.unsafe_all
            && !self.unsafe_accounts.contains(&normalize_account(account))
    }

    /// Whether a thread's archive state is safe to use.
    pub(crate) fn thread_is_safe(&self, key: &ThreadKey) -> bool {
        self.account_is_safe(key.account())
            && !self.unsafe_threads.contains(key)
    }

    /// Whether a complete receipt snapshot is safe for the selected account
    /// scope. A single unsafe thread makes that account's snapshot
    /// unavailable rather than looking like an empty receipt list.
    pub(crate) fn scope_is_safe(&self, account: Option<&str>) -> bool {
        match account {
            Some(account) => {
                let account = normalize_account(account);
                self.account_is_safe(&account)
                    && !self
                        .unsafe_threads
                        .iter()
                        .any(|key| key.account() == account)
            }
            None => {
                !self.unsafe_all
                    && self.unsafe_accounts.is_empty()
                    && self.unsafe_threads.is_empty()
            }
        }
    }

    /// Whether archive-on-close is enabled for an account in the latest
    /// observed epoch.
    pub(crate) fn is_enabled(&self, account: &str) -> bool {
        self.enablement
            .get(&normalize_account(account))
            .is_some_and(|current| current.enabled)
    }

    /// Latest enablement epoch, including its frozen receipt label.
    pub(crate) fn enablement(
        &self,
        account: &str,
    ) -> Option<(&str, &ArchiveLabelIdentity)> {
        let current = self.enablement.get(&normalize_account(account))?;
        current
            .enabled
            .then_some((current.epoch.as_str(), &current.label))
    }

    /// Current close generation for a thread, if it belongs to the active
    /// enablement epoch and has not been consumed.
    pub(crate) fn unconsumed_close(
        &self,
        key: &ThreadKey,
    ) -> Option<&ArchiveCloseGeneration> {
        let epoch = self.enablement.get(key.account())?;
        if !epoch.enabled || !self.thread_is_safe(key) {
            return None;
        }
        let thread = self.threads.get(key)?;
        let Observation::Closed {
            enable_epoch,
            generation,
        } = thread.observation.as_ref()?
        else {
            return None;
        };
        if enable_epoch != &epoch.epoch {
            return None;
        }
        let state = thread.generations.get(generation)?;
        (!state.consumed())
            .then_some(state.close.as_ref())
            .flatten()
    }

    /// The recorded close for one generation, regardless of consumption.
    /// Used to fill live report rows from the frozen evidence.
    pub(crate) fn close_for(
        &self,
        key: &ThreadKey,
        generation: &str,
    ) -> Option<&ArchiveCloseGeneration> {
        self.threads
            .get(key)?
            .generations
            .get(generation)?
            .close
            .as_ref()
    }

    /// Return all still-owned archived receipts, including receipts from
    /// older label/config epochs. Release events remove ownership without
    /// rewriting the receipt's original label identity.
    pub(crate) fn owned_receipts(
        &self,
    ) -> impl Iterator<Item = &ArchiveReceipt> {
        self.threads.iter().flat_map(|(key, thread)| {
            thread.generations.values().filter_map(|generation| {
                if self.thread_is_safe(key)
                    && generation.release.is_none()
                    && generation.pending_releases.is_empty()
                {
                    generation.receipt.as_ref()
                } else {
                    None
                }
            })
        })
    }

    /// Accepted archive history, including released receipts, in stable
    /// thread/generation order. Unsafe thread histories are omitted; callers
    /// must check `scope_is_safe` before treating the result as complete.
    pub(crate) fn archive_history(&self) -> Vec<ArchiveHistoryRecord> {
        let mut records = Vec::new();
        for (key, thread) in &self.threads {
            if !self.thread_is_safe(key) {
                continue;
            }
            for generation in thread.generations.values() {
                let (Some(receipt), Some(close)) =
                    (generation.receipt.as_ref(), generation.close.as_ref())
                else {
                    continue;
                };
                let state = match generation
                    .release
                    .as_ref()
                    .map(|record| &record.kind)
                {
                    Some(ArchiveTerminalKind::LetGo { .. }) => {
                        ArchiveHistoryState::LetGo
                    }
                    Some(ArchiveTerminalKind::Restored { .. }) => {
                        ArchiveHistoryState::Restored
                    }
                    _ => ArchiveHistoryState::Archived,
                };
                records.push(ArchiveHistoryRecord {
                    receipt: receipt.clone(),
                    note: close.note.clone(),
                    state,
                });
            }
        }
        records
    }

    /// Every accepted terminal transition, in deterministic thread and
    /// generation order. Used by reports and doctor counts.
    pub(crate) fn terminal_records(
        &self,
    ) -> impl Iterator<Item = &ArchiveTerminalRecord> {
        self.threads.values().flat_map(|thread| {
            thread
                .generations
                .values()
                .flat_map(|generation| generation.terminal_history.iter())
        })
    }

    /// Count distinct accepted terminal outcomes on or after `since`.
    /// `since` should already be expressed in the caller's configured
    /// timezone so the journal remains independent of display settings.
    pub(crate) fn counts_since(
        &self,
        account: Option<&str>,
        since: DateTime<FixedOffset>,
    ) -> ArchiveCounts {
        let account = account.map(normalize_account);
        let mut counts = ArchiveCounts::default();
        for record in self.terminal_records() {
            if record.ts < since
                || account
                    .as_deref()
                    .is_some_and(|account| account != record.key.account())
            {
                continue;
            }
            match record.kind {
                ArchiveTerminalKind::Archived { .. } => counts.archived += 1,
                ArchiveTerminalKind::Kept { .. } => counts.kept += 1,
                ArchiveTerminalKind::LetGo { .. } => counts.let_go += 1,
                ArchiveTerminalKind::Restored { .. } => counts.restored += 1,
            }
        }
        counts
    }

    /// Project the baseline that a live enabled run would append, entirely
    /// in memory. Dry-run uses this to show first-enable backlog rows and to
    /// derive the same open-observation witnesses as the following live run.
    pub(crate) fn project_baseline(
        &self,
        account: &str,
        label: ArchiveLabelIdentity,
        scan: &ArchiveScan,
    ) -> Self {
        let mut projected = self.clone();
        if let Some(event) =
            projected.event_for_enablement(account, true, label)
        {
            projected.apply_projected_observation(&event);
        }

        let mut keys: Vec<&ThreadKey> = scan.threads().collect();
        keys.sort();
        for key in keys {
            let Some(evidence) = scan.evidence(key) else {
                continue;
            };
            if let Some(event) = projected.event_for_open(scan, evidence) {
                projected.apply_projected_observation(&event);
            }
        }
        projected
    }

    fn apply_projected_observation(&mut self, event: &PlannedArchiveEvent) {
        let stored = StoredEvent {
            ts: pref_journal::event_timestamp(),
            host: "dry-run".to_string(),
            path: "dry-run.jsonl".to_string(),
            line: 0,
            kind: event.kind.to_string(),
            value: json!({"value": event.value.clone()}),
        };
        match event.kind {
            ENABLED_KIND => {
                if let Some(parsed) = parse_enablement(&stored) {
                    self.apply_enablement(parsed);
                }
            }
            OBSERVED_KIND => {
                if let Some(parsed) = parse_open_observation(&stored) {
                    self.apply_open_observation(parsed);
                }
            }
            _ => {}
        }
    }

    /// Plan a body-free enable/disable transition. Fresh default-off state
    /// stays write-free; a disable record is emitted only when it turns off
    /// a previously enabled account.
    pub(crate) fn event_for_enablement(
        &self,
        account: &str,
        enabled: bool,
        label: ArchiveLabelIdentity,
    ) -> Option<PlannedArchiveEvent> {
        let account = normalize_account(account);
        if account.is_empty()
            || !valid_label(&label)
            || !self.account_is_safe(&account)
        {
            return None;
        }
        let previous = self.enablement.get(&account);
        match (enabled, previous) {
            (false, None)
            | (false, Some(Enablement { enabled: false, .. })) => {
                return None;
            }
            (
                true,
                Some(Enablement {
                    enabled: true,
                    label: current_label,
                    ..
                }),
            ) if current_label == &label => return None,
            _ => {}
        }

        let previous_change_id =
            previous.map(|record| record.change_id.clone());
        let label = if enabled {
            label
        } else {
            previous?.label.clone()
        };
        let epoch = if enabled {
            hash_value(&json!({
                "purpose": "gmail.archive.epoch",
                "account": account,
                "previous_change_id": previous_change_id,
                "label": label_value(&label),
            }))
        } else {
            previous?.epoch.clone()
        };
        let change_id = hash_value(&json!({
            "purpose": "gmail.archive.enablement",
            "account": account,
            "enabled": enabled,
            "epoch": epoch,
            "previous_change_id": previous_change_id,
            "label": label_value(&label),
        }));
        Some(PlannedArchiveEvent::new(
            ENABLED_KIND,
            json!({
                "account": account,
                "enabled": enabled,
                "epoch": epoch,
                "change_id": change_id,
                "previous_change_id": previous_change_id,
                "label": label_value(&label),
            }),
        ))
    }

    /// Plan a positive open observation. A scan must be complete and the
    /// thread's mail identity and saved-message evidence must be valid.
    pub(crate) fn event_for_open(
        &self,
        scan: &ArchiveScan,
        evidence: &ArchiveThreadEvidence,
    ) -> Option<PlannedArchiveEvent> {
        if !scan.scan_complete()
            || !scan_contains_evidence(scan, evidence)
            || !valid_observation_evidence(evidence)
            || !evidence.has_open()
            || evidence.has_unknown()
            || !self.thread_is_safe(&evidence.key)
        {
            return None;
        }
        let current = self.enablement.get(evidence.key.account())?;
        if !current.enabled {
            return None;
        }
        let thread = self.threads.get(&evidence.key);
        let observed = thread
            .and_then(|thread| thread.observation.as_ref())
            .filter(|observation| {
                observation_epoch(observation) == current.epoch.as_str()
            });
        if matches!(observed, Some(Observation::Open { .. })) {
            return None;
        }
        let after_generation = match observed {
            Some(Observation::Closed { generation, .. }) => {
                Some(generation.clone())
            }
            _ => None,
        };
        let tasks = task_locators(&evidence.tasks);
        let observation_id = hash_value(&json!({
            "purpose": "gmail.archive.open",
            "account": evidence.key.account(),
            "thread_id": evidence.key.thread_id(),
            "enable_epoch": current.epoch,
            "after_generation": after_generation,
            "tasks": task_values(&tasks),
        }));
        Some(PlannedArchiveEvent::new(
            OBSERVED_KIND,
            json!({
                "account": evidence.key.account(),
                "thread_id": evidence.key.thread_id(),
                "enable_epoch": current.epoch,
                "state": "open",
                "observation_id": observation_id,
                "after_generation": after_generation,
                "tasks": task_values(&tasks),
            }),
        ))
    }

    /// Plan a new close generation after a valid observed-open event. The
    /// scan is strict: incomplete reads, unknown statuses, merge holds,
    /// ambiguity, or invalid mail cards cannot create a transition.
    pub(crate) fn event_for_close(
        &self,
        scan: &ArchiveScan,
        evidence: &ArchiveThreadEvidence,
        label: ArchiveLabelIdentity,
    ) -> Option<PlannedArchiveEvent> {
        if !scan_contains_evidence(scan, evidence)
            || !scan.archivable(evidence)
            || !self.thread_is_safe(&evidence.key)
            || !valid_label(&label)
        {
            return None;
        }
        let current = self.enablement.get(evidence.key.account())?;
        if !current.enabled || !compatible_label(&current.label, &label) {
            return None;
        }
        let thread = self.threads.get(&evidence.key)?;
        let Some(Observation::Open {
            enable_epoch,
            observation_id,
            after_generation,
        }) = thread.observation.as_ref()
        else {
            return None;
        };
        if enable_epoch != &current.epoch {
            return None;
        }
        let close = close_from_evidence(
            evidence,
            current.epoch.clone(),
            Some(observation_id.clone()),
            after_generation.clone(),
            label,
            false,
            None,
        )?;
        Some(close_event(&close, after_generation.as_deref()))
    }

    /// Plan an explicitly confirmed historical-backlog generation. The
    /// confirmation digest is recorded with the frozen evidence so later
    /// terminal events consume this exact one-shot decision.
    pub(crate) fn event_for_confirmed_backlog(
        &self,
        scan: &ArchiveScan,
        evidence: &ArchiveThreadEvidence,
        label: ArchiveLabelIdentity,
        confirmation_digest: &str,
    ) -> Option<PlannedArchiveEvent> {
        if !scan_contains_evidence(scan, evidence)
            || !scan.archivable(evidence)
            || !self.thread_is_safe(&evidence.key)
            || !valid_label(&label)
            || !valid_digest(confirmation_digest)
            || !matches!(
                self.project(scan, evidence),
                ArchiveDisposition::HistoricalBacklog
            )
        {
            return None;
        }
        let current = self.enablement.get(evidence.key.account())?;
        if !current.enabled || !compatible_label(&current.label, &label) {
            return None;
        }
        let close = close_from_evidence(
            evidence,
            current.epoch.clone(),
            None,
            None,
            label,
            true,
            Some(confirmation_digest.to_string()),
        )?;
        Some(close_event(&close, None))
    }

    /// Pure planning projection for one evidence row. It never writes or
    /// advances observations; live apply persists the returned event only
    /// after its own boundary revalidation.
    pub(crate) fn project(
        &self,
        scan: &ArchiveScan,
        evidence: &ArchiveThreadEvidence,
    ) -> ArchiveDisposition {
        if !self.is_enabled(evidence.key.account()) {
            return ArchiveDisposition::Disabled;
        }
        if !self.thread_is_safe(&evidence.key)
            || !scan.scan_complete()
            || !scan_contains_evidence(scan, evidence)
            || !valid_observation_evidence(evidence)
        {
            return ArchiveDisposition::Held;
        }
        if evidence.has_open() {
            return if evidence.has_unknown() {
                ArchiveDisposition::Held
            } else {
                ArchiveDisposition::Watching
            };
        }
        if !scan.archivable(evidence) {
            return ArchiveDisposition::Held;
        }
        let Some(current) = self.enablement.get(evidence.key.account()) else {
            return ArchiveDisposition::Disabled;
        };
        let observed = self
            .threads
            .get(&evidence.key)
            .and_then(|thread| thread.observation.as_ref())
            .filter(|observation| {
                observation_epoch(observation) == current.epoch
            });
        match observed {
            Some(Observation::Closed { generation, .. }) => {
                let Some(generation_state) = self
                    .threads
                    .get(&evidence.key)
                    .and_then(|thread| thread.generations.get(generation))
                else {
                    return ArchiveDisposition::Held;
                };
                let Some(close) = generation_state.close.clone() else {
                    return ArchiveDisposition::Held;
                };
                if let Some(outcome) = generation_state.outcome() {
                    ArchiveDisposition::Consumed { close, outcome }
                } else {
                    ArchiveDisposition::Ready {
                        close,
                        needs_record: false,
                    }
                }
            }
            Some(Observation::Open { .. }) => {
                let Some(event) =
                    self.event_for_close(scan, evidence, current.label.clone())
                else {
                    return ArchiveDisposition::Held;
                };
                match parse_close_value(&event.value, None) {
                    Some(parsed) => ArchiveDisposition::Ready {
                        close: parsed.close,
                        needs_record: true,
                    },
                    None => ArchiveDisposition::Held,
                }
            }
            None => ArchiveDisposition::HistoricalBacklog,
        }
    }

    /// Plan a terminal event for an existing close generation. Archived IDs
    /// must be a non-empty subset of the frozen saved set; release IDs must
    /// be a subset of the exact owned receipt.
    pub(crate) fn event_for_terminal(
        &self,
        key: &ThreadKey,
        generation: &str,
        terminal: ArchiveTerminalKind,
    ) -> Option<PlannedArchiveEvent> {
        if !self.thread_is_safe(key) {
            return None;
        }
        let generation_state =
            self.threads.get(key)?.generations.get(generation)?;
        let close = generation_state.close.as_ref()?;
        let kind = match &terminal {
            ArchiveTerminalKind::Archived {
                message_ids, label, ..
            } => {
                if generation_state.consumed()
                    || !valid_ids(message_ids)
                    || !ids_are_subset(message_ids, &close.saved_ids)
                    || !compatible_label(&close.label, label)
                {
                    return None;
                }
                ARCHIVED_KIND
            }
            ArchiveTerminalKind::Kept { reason } => {
                if generation_state.consumed() || !valid_reason(reason) {
                    return None;
                }
                KEPT_KIND
            }
            ArchiveTerminalKind::LetGo { message_ids, label }
            | ArchiveTerminalKind::Restored { message_ids, label } => {
                let receipt = generation_state.receipt.as_ref()?;
                if generation_state.release.is_some()
                    || !generation_state.pending_releases.is_empty()
                    || !valid_ids(message_ids)
                    || !ids_are_subset(message_ids, &receipt.message_ids)
                    || !compatible_label(&receipt.label, label)
                {
                    return None;
                }
                match &terminal {
                    ArchiveTerminalKind::LetGo { .. } => LET_GO_KIND,
                    ArchiveTerminalKind::Restored { .. } => RESTORED_KIND,
                    _ => unreachable!(),
                }
            }
        };
        let mut value = terminal_value(key, generation, &terminal);
        if matches!(kind, LET_GO_KIND | RESTORED_KIND) {
            let archive_event_id =
                generation_state.receipt.as_ref()?.archive_event_id.clone();
            value.as_object_mut()?.insert(
                "archive_event_id".to_string(),
                json!(archive_event_id),
            );
        }
        let event_id = terminal_event_id(kind, &value);
        value
            .as_object_mut()?
            .insert("event_id".to_string(), json!(event_id));
        Some(PlannedArchiveEvent::new(kind, value))
    }

    /// Append a previously planned event. Callers serialize state changes
    /// under the existing Gmail sync guard and revalidate before appending.
    pub(crate) fn append(
        vault: &Path,
        host: &str,
        event: &PlannedArchiveEvent,
    ) -> io::Result<()> {
        let ts = pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let mut line = serde_json::to_string(&json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": event.kind,
            "value": event.value,
        }))
        .map_err(io::Error::other)?;
        line.push('\n');
        pref_journal::append_line(vault, NAMESPACE, host, &line)
    }

    fn apply_enablement(&mut self, parsed: Enablement) {
        let current = self.enablement.get(&parsed.account);
        let expected_previous = current.map(|item| item.change_id.as_str());
        if expected_previous != parsed.previous_change_id.as_deref() {
            return;
        }
        if let Some(current) = current
            && parsed.enabled
            && current.enabled
            && current.epoch == parsed.epoch
            && current.label == parsed.label
        {
            return;
        }
        self.enablement.insert(parsed.account.clone(), parsed);
    }

    fn apply_open_observation(&mut self, parsed: OpenObservation) {
        let Some(enabled) = self.enablement.get(parsed.key.account()) else {
            self.mark_invalid_scope(Some(&parsed.key));
            return;
        };
        if !enabled.enabled || enabled.epoch != parsed.enable_epoch {
            return;
        }
        let thread = self.threads.entry(parsed.key.clone()).or_default();
        if thread.observation.as_ref().is_some_and(|observation| {
            observation_epoch(observation) != parsed.enable_epoch.as_str()
        }) {
            thread.observation = None;
        }
        match thread.observation.as_ref() {
            Some(Observation::Open { enable_epoch, .. })
                if enable_epoch == &parsed.enable_epoch =>
            {
                // Concurrent copies of the same transition converge; a
                // second different open cannot replace the winning witness.
            }
            Some(Observation::Closed {
                enable_epoch,
                generation,
            }) if enable_epoch == &parsed.enable_epoch => {
                if parsed.after_generation.as_deref()
                    == Some(generation.as_str())
                {
                    thread.observation = Some(Observation::Open {
                        enable_epoch: parsed.enable_epoch,
                        observation_id: parsed.observation_id,
                        after_generation: parsed.after_generation,
                    });
                }
            }
            Some(_) => {}
            None => {
                if parsed.after_generation.is_none() {
                    thread.observation = Some(Observation::Open {
                        enable_epoch: parsed.enable_epoch,
                        observation_id: parsed.observation_id,
                        after_generation: parsed.after_generation,
                    });
                }
            }
        }
    }

    fn apply_close(&mut self, parsed: CloseEvent, _event: &StoredEvent) {
        let Some(enabled) = self.enablement.get(parsed.close.key.account())
        else {
            self.mark_invalid_scope(Some(&parsed.close.key));
            return;
        };
        if !enabled.enabled || enabled.epoch != parsed.close.enable_epoch {
            return;
        }
        if let Some(existing) = self
            .threads
            .get(&parsed.close.key)
            .and_then(|thread| thread.generations.get(&parsed.close.generation))
        {
            if existing.close.as_ref() == Some(&parsed.close) {
                return;
            }
            self.mark_invalid_scope(Some(&parsed.close.key));
            return;
        }
        let thread = self.threads.entry(parsed.close.key.clone()).or_default();
        if parsed.close.historical {
            if matches!(
                thread.observation.as_ref(),
                Some(observation) if observation_epoch(observation) == parsed.close.enable_epoch
            ) {
                return;
            }
        } else {
            let Some(Observation::Open {
                enable_epoch,
                observation_id,
                after_generation,
            }) = thread.observation.as_ref()
            else {
                self.mark_invalid_scope(Some(&parsed.close.key));
                return;
            };
            if enable_epoch != &parsed.close.enable_epoch
                || observation_id
                    != parsed.opened_by.as_deref().unwrap_or_default()
                || after_generation != &parsed.after_generation
            {
                self.mark_invalid_scope(Some(&parsed.close.key));
                return;
            }
        }
        thread.generations.insert(
            parsed.close.generation.clone(),
            GenerationState {
                close: Some(parsed.close.clone()),
                ..GenerationState::default()
            },
        );
        thread.observation = Some(Observation::Closed {
            enable_epoch: parsed.close.enable_epoch,
            generation: parsed.close.generation,
        });
    }

    fn apply_terminal(&mut self, parsed: ArchiveTerminalRecord) {
        let Some(generation) = self
            .threads
            .get_mut(&parsed.key)
            .and_then(|thread| thread.generations.get_mut(&parsed.generation))
        else {
            self.mark_invalid_scope(Some(&parsed.key));
            return;
        };
        match &parsed.kind {
            ArchiveTerminalKind::Archived {
                message_ids,
                label,
                recovered,
            } => {
                if generation
                    .first_terminal
                    .as_ref()
                    .is_some_and(|first| first.event_id == parsed.event_id)
                {
                    return;
                }
                let Some(close) = generation.close.as_ref() else {
                    self.mark_invalid_scope(Some(&parsed.key));
                    return;
                };
                if generation.first_terminal.is_some()
                    || !valid_ids(message_ids)
                    || !ids_are_subset(message_ids, &close.saved_ids)
                    || !compatible_label(&close.label, label)
                {
                    self.mark_invalid_scope(Some(&parsed.key));
                    return;
                }
                let receipt = ArchiveReceipt {
                    key: parsed.key.clone(),
                    generation: parsed.generation.clone(),
                    message_ids: message_ids.clone(),
                    label: label.clone(),
                    recovered: *recovered,
                    archived_at: parsed.ts,
                    archive_event_id: parsed.event_id.clone(),
                };
                generation.receipt = Some(receipt);
                generation.first_terminal = Some(parsed.clone());
                generation.terminal_history.push(parsed);
            }
            ArchiveTerminalKind::Kept { reason } => {
                if generation.first_terminal.is_some() || !valid_reason(reason)
                {
                    return;
                }
                generation.first_terminal = Some(parsed.clone());
                generation.terminal_history.push(parsed);
            }
            ArchiveTerminalKind::LetGo { .. }
            | ArchiveTerminalKind::Restored { .. } => {
                generation.pending_releases.push(parsed.clone());
            }
        }
    }

    fn finish_pending_releases(&mut self) {
        for (key, thread) in &mut self.threads {
            for (generation_id, generation) in &mut thread.generations {
                if generation.pending_releases.is_empty() {
                    continue;
                }
                generation.pending_releases.sort_by(|left, right| {
                    terminal_record_order(left)
                        .cmp(&terminal_record_order(right))
                });
                for release in std::mem::take(&mut generation.pending_releases)
                {
                    let Some(receipt) = generation.receipt.as_ref() else {
                        self.unsafe_threads.insert(key.clone());
                        push_warning(
                            &mut self.warnings,
                            "Gmail archive release has no matching archived receipt; work is held",
                        );
                        generation.pending_releases.push(release);
                        continue;
                    };
                    if generation.release.is_some() {
                        continue;
                    }
                    let (ids, label) = match &release.kind {
                        ArchiveTerminalKind::LetGo { message_ids, label }
                        | ArchiveTerminalKind::Restored {
                            message_ids,
                            label,
                        } => (message_ids, label),
                        _ => continue,
                    };
                    if release.archive_event_id.as_deref()
                        != Some(receipt.archive_event_id.as_str())
                        || !valid_ids(ids)
                        || !ids_are_subset(ids, &receipt.message_ids)
                        || !compatible_label(&receipt.label, label)
                    {
                        self.unsafe_threads.insert(key.clone());
                        push_warning(
                            &mut self.warnings,
                            "Gmail archive release does not match its receipt; work is held",
                        );
                        generation.pending_releases.push(release);
                        continue;
                    }
                    generation.release = Some(release.clone());
                    generation.terminal_history.push(release);
                }
                let _ = generation_id;
            }
        }
    }

    fn mark_malformed(&mut self, event: &StoredEvent) {
        let account = event
            .value
            .get("value")
            .and_then(Value::as_object)
            .and_then(|value| value.get("account"))
            .and_then(Value::as_str)
            .map(normalize_account)
            .filter(|account| !account.is_empty());
        let key = event
            .value
            .get("value")
            .and_then(Value::as_object)
            .and_then(|value| {
                Some(ThreadKey::new(
                    value.get("account")?.as_str()?,
                    value.get("thread_id")?.as_str()?,
                )?)
            });
        if let Some(key) = key.as_ref() {
            self.unsafe_threads.insert(key.clone());
        } else if let Some(account) = account {
            self.unsafe_accounts.insert(account);
        } else {
            self.unsafe_all = true;
        }
        push_warning(
            &mut self.warnings,
            "skipping malformed Gmail archive journal event; affected work is held",
        );
    }

    fn mark_invalid_scope(&mut self, key: Option<&ThreadKey>) {
        if let Some(key) = key {
            self.unsafe_threads.insert(key.clone());
        } else {
            self.unsafe_all = true;
        }
        push_warning(
            &mut self.warnings,
            "Gmail archive journal transition does not match its generation; work is held",
        );
    }
}

#[derive(Debug, Clone)]
struct OpenObservation {
    key: ThreadKey,
    enable_epoch: String,
    observation_id: String,
    after_generation: Option<String>,
}

#[derive(Debug, Clone)]
struct CloseEvent {
    close: ArchiveCloseGeneration,
    opened_by: Option<String>,
    after_generation: Option<String>,
}

fn observation_epoch(observation: &Observation) -> &str {
    match observation {
        Observation::Open { enable_epoch, .. }
        | Observation::Closed { enable_epoch, .. } => enable_epoch,
    }
}

fn close_event(
    close: &ArchiveCloseGeneration,
    after_generation: Option<&str>,
) -> PlannedArchiveEvent {
    PlannedArchiveEvent::new(
        CLOSED_KIND,
        json!({
            "account": close.key.account(),
            "thread_id": close.key.thread_id(),
            "enable_epoch": close.enable_epoch,
            "generation": close.generation,
            "opened_by": close.opened_by,
            "after_generation": after_generation,
            "note": close.note,
            "tasks": task_values(&close.tasks),
            "saved_ids": close.saved_ids,
            "note_fingerprint": close.note_fingerprint,
            "label": label_value(&close.label),
            "historical": close.historical,
            "confirmation_digest": close.confirmation_digest,
        }),
    )
}

fn close_from_evidence(
    evidence: &ArchiveThreadEvidence,
    enable_epoch: String,
    opened_by: Option<String>,
    after_generation: Option<String>,
    label: ArchiveLabelIdentity,
    historical: bool,
    confirmation_digest: Option<String>,
) -> Option<ArchiveCloseGeneration> {
    if !valid_observation_evidence(evidence)
        || evidence.tasks.is_empty()
        || evidence.has_open()
        || evidence.has_unknown()
        || evidence.positive_closed().is_empty()
        || evidence.merge_hold
        || evidence.identity_uncertain
        || !valid_label(&label)
    {
        return None;
    }
    let note_fingerprint = evidence.note_fingerprint.as_ref()?;
    let tasks = task_locators(&evidence.tasks);
    let saved_ids = evidence.saved_ids.clone();
    if !valid_ids(&saved_ids) {
        return None;
    }
    if historical != confirmation_digest.is_some()
        || confirmation_digest
            .as_deref()
            .is_some_and(|digest| !valid_digest(digest))
    {
        return None;
    }
    let generation = hash_value(&json!({
        "purpose": "gmail.archive.close",
        "account": evidence.key.account(),
        "thread_id": evidence.key.thread_id(),
        "enable_epoch": enable_epoch,
        "opened_by": opened_by,
        "after_generation": after_generation,
        "note": evidence.note,
        "tasks": task_values(&tasks),
        "saved_ids": saved_ids,
        "note_fingerprint": note_fingerprint,
        "label": label_value(&label),
        "historical": historical,
        "confirmation_digest": confirmation_digest,
    }));
    Some(ArchiveCloseGeneration {
        key: evidence.key.clone(),
        generation,
        enable_epoch,
        note: evidence.note.clone(),
        tasks,
        saved_ids,
        note_fingerprint: note_fingerprint.clone(),
        label,
        historical,
        confirmation_digest,
        opened_by,
        closed_at: None,
    })
}

fn valid_observation_evidence(evidence: &ArchiveThreadEvidence) -> bool {
    evidence.issues.is_empty()
        && safe_relative_path(&evidence.note)
        && evidence
            .note_fingerprint
            .as_deref()
            .is_some_and(valid_digest)
        && valid_ids(&evidence.saved_ids)
        && !evidence.identity_uncertain
        && !evidence.merge_hold
        && evidence
            .tasks
            .iter()
            .all(|task| task.line > 0 && safe_relative_path(&task.path))
}

fn scan_contains_evidence(
    scan: &ArchiveScan,
    evidence: &ArchiveThreadEvidence,
) -> bool {
    scan.evidence(&evidence.key) == Some(evidence)
}

fn task_locators(tasks: &[ArchiveLinkTask]) -> Vec<ArchiveTaskLocator> {
    let mut locators: Vec<_> =
        tasks.iter().map(ArchiveTaskLocator::from).collect();
    locators.sort();
    locators.dedup();
    locators
}

fn task_values(tasks: &[ArchiveTaskLocator]) -> Vec<Value> {
    tasks
        .iter()
        .map(|task| {
            json!({
                "path": task.path,
                "line": task.line,
                "block_id": task.block_id,
                "status": task.status.to_string(),
            })
        })
        .collect()
}

fn label_value(label: &ArchiveLabelIdentity) -> Value {
    json!({"name": label.name, "id": label.id})
}

fn normalize_account(account: &str) -> String {
    account.trim().to_ascii_lowercase()
}

fn hash_value(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("JSON value serializes");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn valid_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_ids(ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.windows(2).all(|pair| pair[0] < pair[1])
        && ids.iter().all(|id| valid_id(id))
}

fn ids_are_subset(ids: &[String], allowed: &[String]) -> bool {
    ids.iter().all(|id| allowed.binary_search(id).is_ok())
}

fn valid_label(label: &ArchiveLabelIdentity) -> bool {
    !label.name.trim().is_empty()
        && label.name.len() <= 512
        && !label.name.chars().any(char::is_control)
        && label.id.as_ref().is_none_or(|id| {
            !id.trim().is_empty()
                && id.len() <= 512
                && !id.chars().any(char::is_control)
        })
}

fn compatible_label(
    original: &ArchiveLabelIdentity,
    candidate: &ArchiveLabelIdentity,
) -> bool {
    original.name == candidate.name
        && original
            .id
            .as_ref()
            .is_none_or(|id| candidate.id.as_ref() == Some(id))
}

fn valid_reason(reason: &str) -> bool {
    !reason.is_empty()
        && reason.len() <= 64
        && reason.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b"_-".contains(&byte)
        })
}

fn safe_relative_path(value: &str) -> bool {
    if value.is_empty()
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn parse_key(value: &Map<String, Value>) -> Option<ThreadKey> {
    ThreadKey::new(
        value.get("account")?.as_str()?,
        value.get("thread_id")?.as_str()?,
    )
}

fn parse_enablement(event: &StoredEvent) -> Option<Enablement> {
    let value = event.value.get("value")?.as_object()?;
    let account = normalize_account(value.get("account")?.as_str()?);
    if account.is_empty() {
        return None;
    }
    let enabled = value.get("enabled")?.as_bool()?;
    let epoch = value.get("epoch")?.as_str()?.to_string();
    let change_id = value.get("change_id")?.as_str()?.to_string();
    let previous_change_id = match value.get("previous_change_id")? {
        Value::Null => None,
        Value::String(previous) => Some(previous.clone()),
        _ => return None,
    };
    let label = parse_label(value.get("label")?)?;
    if !valid_digest(&epoch)
        || !valid_digest(&change_id)
        || previous_change_id
            .as_deref()
            .is_some_and(|previous| !valid_digest(previous))
    {
        return None;
    }
    let expected_epoch = if enabled {
        hash_value(&json!({
            "purpose": "gmail.archive.epoch",
            "account": account,
            "previous_change_id": previous_change_id,
            "label": label_value(&label),
        }))
    } else {
        epoch.clone()
    };
    if expected_epoch != epoch {
        return None;
    }
    let expected_change = hash_value(&json!({
        "purpose": "gmail.archive.enablement",
        "account": account,
        "enabled": enabled,
        "epoch": epoch,
        "previous_change_id": previous_change_id,
        "label": label_value(&label),
    }));
    if change_id != expected_change {
        return None;
    }
    Some(Enablement {
        account,
        enabled,
        epoch,
        change_id,
        previous_change_id,
        label,
    })
}

fn parse_open_observation(event: &StoredEvent) -> Option<OpenObservation> {
    let value = event.value.get("value")?.as_object()?;
    if value.get("state")?.as_str()? != "open" {
        return None;
    }
    let key = parse_key(value)?;
    let enable_epoch = value.get("enable_epoch")?.as_str()?.to_string();
    let observation_id = value.get("observation_id")?.as_str()?.to_string();
    let after_generation = match value.get("after_generation")? {
        Value::Null => None,
        Value::String(generation) => Some(generation.clone()),
        _ => return None,
    };
    let tasks = parse_task_locators(value.get("tasks")?)?;
    if !valid_digest(&enable_epoch)
        || !valid_digest(&observation_id)
        || after_generation
            .as_deref()
            .is_some_and(|generation| !valid_digest(generation))
        || tasks.is_empty()
        || !tasks
            .iter()
            .any(|task| matches!(task.status, ' ' | '*' | '/' | '?'))
        || tasks.iter().any(|task| {
            !matches!(task.status, ' ' | '*' | '/' | '?' | 'x' | 'X' | '-')
        })
    {
        return None;
    }
    let expected_id = hash_value(&json!({
        "purpose": "gmail.archive.open",
        "account": key.account(),
        "thread_id": key.thread_id(),
        "enable_epoch": enable_epoch,
        "after_generation": after_generation,
        "tasks": task_values(&tasks),
    }));
    if observation_id != expected_id {
        return None;
    }
    Some(OpenObservation {
        key,
        enable_epoch,
        observation_id,
        after_generation,
    })
}

fn parse_close_generation(event: &StoredEvent) -> Option<CloseEvent> {
    parse_close_value(event.value.get("value")?, Some(event.ts))
}

fn parse_close_value(
    value: &Value,
    closed_at: Option<DateTime<FixedOffset>>,
) -> Option<CloseEvent> {
    let object = value.as_object()?;
    let key = parse_key(object)?;
    let enable_epoch = object.get("enable_epoch")?.as_str()?.to_string();
    let generation = object.get("generation")?.as_str()?.to_string();
    let opened_by = match object.get("opened_by")? {
        Value::Null => None,
        Value::String(id) => Some(id.clone()),
        _ => return None,
    };
    let after_generation = match object.get("after_generation")? {
        Value::Null => None,
        Value::String(generation) => Some(generation.clone()),
        _ => return None,
    };
    let note = object.get("note")?.as_str()?.to_string();
    let tasks = parse_task_locators(object.get("tasks")?)?;
    let saved_ids = parse_ids(object.get("saved_ids")?)?;
    let note_fingerprint =
        object.get("note_fingerprint")?.as_str()?.to_string();
    let label = parse_label(object.get("label")?)?;
    let historical = object.get("historical")?.as_bool()?;
    let confirmation_digest = match object.get("confirmation_digest")? {
        Value::Null => None,
        Value::String(digest) => Some(digest.clone()),
        _ => return None,
    };
    if !safe_relative_path(&note)
        || !valid_digest(&enable_epoch)
        || !valid_digest(&generation)
        || !valid_digest(&note_fingerprint)
        || after_generation
            .as_deref()
            .is_some_and(|generation| !valid_digest(generation))
        || !valid_label(&label)
        || tasks.is_empty()
        || tasks
            .iter()
            .any(|task| !matches!(task.status, 'x' | 'X' | '-'))
        || !valid_ids(&saved_ids)
        || historical != confirmation_digest.is_some()
        || confirmation_digest
            .as_deref()
            .is_some_and(|digest| !valid_digest(digest))
        || historical == opened_by.is_some()
    {
        return None;
    }
    if !historical && opened_by.as_deref().is_none_or(|id| !valid_digest(id)) {
        return None;
    }
    let close = ArchiveCloseGeneration {
        key: key.clone(),
        generation: generation.clone(),
        enable_epoch,
        note,
        tasks: tasks.clone(),
        saved_ids: saved_ids.clone(),
        note_fingerprint: note_fingerprint.clone(),
        label: label.clone(),
        historical,
        confirmation_digest: confirmation_digest.clone(),
        opened_by: opened_by.clone(),
        closed_at,
    };
    let expected_generation = hash_value(&json!({
        "purpose": "gmail.archive.close",
        "account": key.account(),
        "thread_id": key.thread_id(),
        "enable_epoch": close.enable_epoch,
        "opened_by": close.opened_by,
        "after_generation": after_generation,
        "note": close.note,
        "tasks": task_values(&close.tasks),
        "saved_ids": close.saved_ids,
        "note_fingerprint": close.note_fingerprint,
        "label": label_value(&close.label),
        "historical": close.historical,
        "confirmation_digest": close.confirmation_digest,
    }));
    if expected_generation != close.generation {
        return None;
    }
    Some(CloseEvent {
        close,
        opened_by,
        after_generation,
    })
}

fn terminal_value(
    key: &ThreadKey,
    generation: &str,
    terminal: &ArchiveTerminalKind,
) -> Value {
    let mut value = Map::new();
    value.insert("account".to_string(), json!(key.account()));
    value.insert("thread_id".to_string(), json!(key.thread_id()));
    value.insert("generation".to_string(), json!(generation));
    match terminal {
        ArchiveTerminalKind::Archived {
            message_ids,
            label,
            recovered,
        } => {
            value.insert("message_ids".to_string(), json!(message_ids));
            value.insert("label".to_string(), label_value(label));
            value.insert("recovered".to_string(), json!(recovered));
        }
        ArchiveTerminalKind::Kept { reason } => {
            value.insert("reason".to_string(), json!(reason));
        }
        ArchiveTerminalKind::LetGo { message_ids, label }
        | ArchiveTerminalKind::Restored { message_ids, label } => {
            value.insert("message_ids".to_string(), json!(message_ids));
            value.insert("label".to_string(), label_value(label));
        }
    }
    Value::Object(value)
}

fn terminal_event_id(kind: &str, value: &Value) -> String {
    hash_value(
        &json!({"purpose": "gmail.archive.terminal", "kind": kind, "value": value}),
    )
}

fn parse_terminal(event: &StoredEvent) -> Option<ArchiveTerminalRecord> {
    let value = event.value.get("value")?.as_object()?;
    let key = parse_key(value)?;
    let generation = value.get("generation")?.as_str()?.to_string();
    let event_id = value.get("event_id")?.as_str()?.to_string();
    let archive_event_id = value
        .get("archive_event_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !valid_digest(&generation) || !valid_digest(&event_id) {
        return None;
    }
    let kind = match event.kind.as_str() {
        ARCHIVED_KIND => ArchiveTerminalKind::Archived {
            message_ids: parse_ids(value.get("message_ids")?)?,
            label: parse_label(value.get("label")?)?,
            recovered: value.get("recovered")?.as_bool()?,
        },
        KEPT_KIND => {
            let reason = value.get("reason")?.as_str()?.to_string();
            if !valid_reason(&reason) {
                return None;
            }
            ArchiveTerminalKind::Kept { reason }
        }
        LET_GO_KIND => ArchiveTerminalKind::LetGo {
            message_ids: parse_ids(value.get("message_ids")?)?,
            label: parse_label(value.get("label")?)?,
        },
        RESTORED_KIND => ArchiveTerminalKind::Restored {
            message_ids: parse_ids(value.get("message_ids")?)?,
            label: parse_label(value.get("label")?)?,
        },
        _ => return None,
    };
    let mut without_ids = value.clone();
    without_ids.remove("event_id");
    if matches!(event.kind.as_str(), LET_GO_KIND | RESTORED_KIND) {
        if archive_event_id
            .as_deref()
            .is_none_or(|id| !valid_digest(id))
        {
            return None;
        }
    } else if archive_event_id.is_some() {
        return None;
    }
    let expected_id =
        terminal_event_id(&event.kind, &Value::Object(without_ids));
    if event_id != expected_id {
        return None;
    }
    Some(ArchiveTerminalRecord {
        key,
        generation,
        event_id,
        archive_event_id,
        kind,
        ts: event.ts,
    })
}

fn parse_label(value: &Value) -> Option<ArchiveLabelIdentity> {
    let object = value.as_object()?;
    let id = match object.get("id")? {
        Value::Null => None,
        Value::String(id) => Some(id.clone()),
        _ => return None,
    };
    let identity = ArchiveLabelIdentity {
        name: object.get("name")?.as_str()?.to_string(),
        id,
    };
    valid_label(&identity).then_some(identity)
}

fn parse_ids(value: &Value) -> Option<Vec<String>> {
    let values = value.as_array()?;
    let ids = values
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    valid_ids(&ids).then_some(ids)
}

fn parse_task_locators(value: &Value) -> Option<Vec<ArchiveTaskLocator>> {
    let values = value.as_array()?;
    let mut tasks = Vec::with_capacity(values.len());
    for value in values {
        let object = value.as_object()?;
        let path = object.get("path")?.as_str()?.to_string();
        let line = usize::try_from(object.get("line")?.as_u64()?).ok()?;
        let block_id = match object.get("block_id")? {
            Value::Null => None,
            Value::String(block_id) => Some(block_id.clone()),
            _ => return None,
        };
        let status = object.get("status")?.as_str()?;
        let mut chars = status.chars();
        let status = chars.next()?;
        if chars.next().is_some()
            || line == 0
            || !safe_relative_path(&path)
            || block_id.as_deref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 256
                    || id.chars().any(char::is_control)
            })
        {
            return None;
        }
        tasks.push(ArchiveTaskLocator {
            path,
            line,
            block_id,
            status,
        });
    }
    let original = tasks.clone();
    tasks.sort();
    tasks.dedup();
    (tasks == original).then_some(tasks)
}

fn terminal_record_order(
    record: &ArchiveTerminalRecord,
) -> (DateTime<FixedOffset>, &str) {
    (record.ts, record.event_id.as_str())
}

fn push_warning(warnings: &mut Vec<String>, warning: &str) {
    if warnings.len() < MAX_MALFORMED_WARNINGS {
        warnings.push(warning.to_string());
    }
}

/// Catch per-file permission/read errors that `pref_journal::read_raw`
/// intentionally skips for other namespaces. Archive state cannot treat a
/// skipped host as an empty, unconsumed journal.
fn ensure_gmail_journals_readable(vault: &Path) -> io::Result<()> {
    let directory = pref_journal::journal_dir(vault, NAMESPACE);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    for path in paths {
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
            && path.is_file()
        {
            fs::read(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::note::index::{MailIndex, MailIndexEntry};
    use crate::native::gmail::sync::{
        archive_scan::{classify_archive_status, ArchiveIssue},
        scan::LinkPlacement,
    };
    use chrono::{TimeZone, Utc};

    const ACCOUNT: &str = "me@example.com";
    const THREAD: &str = "aa11";

    fn key() -> ThreadKey {
        ThreadKey::new(ACCOUNT, THREAD).unwrap()
    }

    fn label(name: &str, id: Option<&str>) -> ArchiveLabelIdentity {
        ArchiveLabelIdentity::new(name, id.map(str::to_string)).unwrap()
    }

    fn evidence(key: ThreadKey, status: char) -> ArchiveThreadEvidence {
        ArchiveThreadEvidence {
            key,
            note: "mail/thread.md".to_string(),
            tasks: vec![ArchiveLinkTask {
                path: "Projects/Task.md".to_string(),
                line: 7,
                text: format!(
                    "- [{}] #task PRIVATE TASK PHRASE [[mail/thread]]",
                    status
                ),
                block_id: Some("task-7".to_string()),
                status,
                class: classify_archive_status(status),
                placement: LinkPlacement::TaskLine,
                in_done: false,
                merge_retired: false,
                from_duplicate_note: false,
            }],
            saved_ids: vec!["aa01".to_string(), "bb02".to_string()],
            note_fingerprint: Some(format!("sha256:{}", "a".repeat(64))),
            merge_hold: false,
            identity_uncertain: false,
            issues: Vec::<ArchiveIssue>::new(),
        }
    }

    fn scan(evidence: ArchiveThreadEvidence) -> ArchiveScan {
        ArchiveScan::test_scan(evidence, true)
    }

    fn push(
        events: &mut Vec<StoredEvent>,
        event: &PlannedArchiveEvent,
        host: &str,
        second: i64,
    ) {
        let line =
            events.iter().filter(|prior| prior.host == host).count() as u32 + 1;
        events.push(event.stored(host, line, second));
    }

    fn enabled() -> (ArchiveJournal, Vec<StoredEvent>, ArchiveLabelIdentity) {
        let label = label("bob/closed", None);
        let state = ArchiveJournal::default();
        assert!(state
            .event_for_enablement(ACCOUNT, false, label.clone())
            .is_none());
        let event = state
            .event_for_enablement(ACCOUNT, true, label.clone())
            .unwrap();
        let mut events = Vec::new();
        push(&mut events, &event, "athena", 1);
        let state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert_eq!(state.enablement(ACCOUNT).unwrap().1.name, "bob/closed");
        (state, events, label)
    }

    fn closed_after_open() -> (
        ArchiveJournal,
        Vec<StoredEvent>,
        ArchiveThreadEvidence,
        ArchiveCloseGeneration,
    ) {
        let (mut state, mut events, label) = enabled();
        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "athena", 2);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let closed = evidence(key(), 'x');
        let closed_scan = scan(closed.clone());
        let close_event =
            state.event_for_close(&closed_scan, &closed, label).unwrap();
        push(&mut events, &close_event, "athena", 3);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        let ArchiveDisposition::Ready { close, .. } =
            state.project(&closed_scan, &closed)
        else {
            panic!("observed close should be ready")
        };
        (state, events, closed, close)
    }

    fn terminal_event(
        state: &ArchiveJournal,
        close: &ArchiveCloseGeneration,
        terminal: ArchiveTerminalKind,
    ) -> PlannedArchiveEvent {
        state
            .event_for_terminal(&close.key, &close.generation, terminal)
            .unwrap()
    }

    #[test]
    fn first_closed_observation_is_backlog_until_digest_confirmation() {
        let (mut state, mut events, receipt_label) = enabled();
        let closed = evidence(key(), 'x');
        let closed_scan = scan(closed.clone());
        assert!(state
            .event_for_close(&closed_scan, &closed, receipt_label.clone())
            .is_none());
        assert_eq!(
            state.project(&closed_scan, &closed),
            ArchiveDisposition::HistoricalBacklog
        );

        let digest = format!("sha256:{}", "b".repeat(64));
        let confirmed = state
            .event_for_confirmed_backlog(
                &closed_scan,
                &closed,
                receipt_label,
                &digest,
            )
            .unwrap();
        push(&mut events, &confirmed, "athena", 2);
        state = ArchiveJournal::from_events(events, Vec::new());
        let ArchiveDisposition::Ready {
            close,
            needs_record,
        } = state.project(&closed_scan, &closed)
        else {
            panic!("confirmed backlog should become ready")
        };
        assert!(close.historical);
        assert_eq!(close.confirmation_digest.as_deref(), Some(digest.as_str()));
        assert!(!needs_record);
    }

    #[test]
    fn new_thread_open_is_observed_but_missing_linkers_do_not_change_state() {
        let (mut state, mut events, _) = enabled();
        let new_key = ThreadKey::new(ACCOUNT, "cc33").unwrap();
        let open = evidence(new_key.clone(), ' ');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "mac", 2);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let closed = evidence(new_key, 'x');
        let closed_scan = scan(closed.clone());
        let close = state
            .event_for_close(&closed_scan, &closed, label("bob/closed", None))
            .unwrap();
        push(&mut events, &close, "mac", 3);
        state = ArchiveJournal::from_events(events, Vec::new());
        let before_missing = state
            .unconsumed_close(&closed.key)
            .unwrap()
            .generation
            .clone();

        let missing_scan = ArchiveScan::test_empty_scan(true);
        assert!(missing_scan.evidence(&closed.key).is_none());
        assert_eq!(
            state.unconsumed_close(&closed.key).unwrap().generation,
            before_missing
        );
    }

    #[test]
    fn incomplete_or_unknown_evidence_cannot_create_open_or_close_observations()
    {
        let (state, _, receipt_label) = enabled();
        let unknown = evidence(key(), 'z');
        let complete_scan = scan(unknown.clone());
        assert!(state.event_for_open(&complete_scan, &unknown).is_none());
        assert!(state
            .event_for_close(&complete_scan, &unknown, receipt_label.clone())
            .is_none());

        let open = evidence(key(), ' ');
        let incomplete_scan = ArchiveScan::test_scan(open.clone(), false);
        assert!(state.event_for_open(&incomplete_scan, &open).is_none());
        assert!(matches!(
            state.project(&incomplete_scan, &open),
            ArchiveDisposition::Held
        ));
    }

    #[test]
    fn observed_open_then_closed_creates_one_stable_generation() {
        let (mut state, mut events, receipt_label) = enabled();
        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        assert_eq!(
            state.project(&open_scan, &open),
            ArchiveDisposition::Watching
        );
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "athena", 2);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert!(state.event_for_open(&open_scan, &open).is_none());

        let closed = evidence(key(), 'x');
        let closed_scan = scan(closed.clone());
        let close_event = state
            .event_for_close(&closed_scan, &closed, receipt_label.clone())
            .unwrap();
        let close_value =
            parse_close_value(&close_event.value, None).unwrap().close;
        assert!(matches!(
            state.project(&closed_scan, &closed),
            ArchiveDisposition::Ready {
                needs_record: true,
                ..
            }
        ));
        push(&mut events, &close_event, "athena", 3);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert_eq!(
            state.unconsumed_close(&key()).unwrap().generation,
            close_value.generation
        );
        assert!(state
            .event_for_close(&closed_scan, &closed, receipt_label)
            .is_none());
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn question_mark_open_observation_round_trips_and_can_close_later() {
        let (mut state, mut events, receipt_label) = enabled();
        let open = evidence(key(), '?');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "athena", 2);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert!(state.thread_is_safe(&key()));

        let closed = evidence(key(), 'x');
        let closed_scan = scan(closed.clone());
        let close_event = state
            .event_for_close(&closed_scan, &closed, receipt_label)
            .unwrap();
        push(&mut events, &close_event, "athena", 3);
        state = ArchiveJournal::from_events(events, Vec::new());
        assert!(state.unconsumed_close(&key()).is_some());
    }

    #[test]
    fn divergent_host_close_is_warned_and_holds_the_thread() {
        let (mut state, mut events, receipt_label) = enabled();
        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "athena", 2);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let closed_a = evidence(key(), 'x');
        let close_a = state
            .event_for_close(
                &scan(closed_a.clone()),
                &closed_a,
                receipt_label.clone(),
            )
            .unwrap();
        let mut closed_b = closed_a;
        closed_b.saved_ids = vec!["cc03".to_string()];
        let close_b = state
            .event_for_close(&scan(closed_b.clone()), &closed_b, receipt_label)
            .unwrap();
        push(&mut events, &close_a, "apollo", 3);
        push(&mut events, &close_a, "athena", 3);
        push(&mut events, &close_b, "mac", 3);

        let folded = ArchiveJournal::from_events(events, Vec::new());
        assert!(!folded.thread_is_safe(&key()));
        assert!(folded
            .warnings()
            .iter()
            .any(|warning| warning.contains("transition")));
    }

    #[test]
    fn rejected_and_unknown_archived_events_warn_and_hold_the_thread() {
        let (state, mut events, closed, close) = closed_after_open();
        let valid = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: close.label.clone(),
                recovered: false,
            },
        );

        let mut rejected = valid.clone();
        {
            let rejected_value = rejected.value.as_object_mut().unwrap();
            rejected_value.insert("message_ids".to_string(), json!(["cc03"]));
            rejected_value.remove("event_id");
        }
        let rejected_id = terminal_event_id(ARCHIVED_KIND, &rejected.value);
        rejected
            .value
            .as_object_mut()
            .unwrap()
            .insert("event_id".to_string(), json!(rejected_id));
        let mut rejected_events = events.clone();
        push(&mut rejected_events, &rejected, "mac", 4);
        let rejected_fold =
            ArchiveJournal::from_events(rejected_events, Vec::new());
        assert!(!rejected_fold.thread_is_safe(&key()));
        assert!(rejected_fold
            .warnings()
            .iter()
            .any(|warning| warning.contains("transition")));

        let mut unknown = valid;
        {
            let unknown_value = unknown.value.as_object_mut().unwrap();
            unknown_value.insert(
                "generation".to_string(),
                json!(format!("sha256:{}", "f".repeat(64))),
            );
            unknown_value.remove("event_id");
        }
        let unknown_id = terminal_event_id(ARCHIVED_KIND, &unknown.value);
        unknown
            .value
            .as_object_mut()
            .unwrap()
            .insert("event_id".to_string(), json!(unknown_id));
        events.retain(|event| event.kind != ARCHIVED_KIND);
        push(&mut events, &unknown, "mac", 4);
        let unknown_fold = ArchiveJournal::from_events(events, Vec::new());
        assert!(!unknown_fold.thread_is_safe(&key()));
        assert!(unknown_fold
            .warnings()
            .iter()
            .any(|warning| warning.contains("transition")));

        assert!(closed.note.contains("mail/"));
    }

    #[test]
    fn terminal_outcomes_consume_once_and_release_only_the_matching_receipt() {
        let (state, mut events, closed, close) = closed_after_open();
        let archived = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("label-1")),
                recovered: false,
            },
        );
        let archived_record = archived.stored("athena", 4, 10);
        let archived_state = {
            events.push(archived_record.clone());
            ArchiveJournal::from_events(events.clone(), Vec::new())
        };
        let receipt = archived_state.owned_receipts().next().unwrap();
        assert_eq!(receipt.label.id.as_deref(), Some("label-1"));
        let history = archived_state.archive_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].note, "mail/thread.md");
        assert_eq!(history[0].state, ArchiveHistoryState::Archived);

        // Simulate clock skew: the release sorts before the archive event.
        // Its explicit archive_event_id still releases exactly that receipt.
        let release = terminal_event(
            &archived_state,
            &close,
            ArchiveTerminalKind::Restored {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("label-1")),
            },
        );
        let mut reordered = events.clone();
        reordered.push(release.stored("mac", 1, 5));
        reordered.push(archived.stored("apollo", 1, 20));
        let released = ArchiveJournal::from_events(reordered, Vec::new());
        assert_eq!(released.owned_receipts().count(), 0);
        assert_eq!(
            released.archive_history()[0].state,
            ArchiveHistoryState::Restored
        );
        let counts = released.counts_since(
            Some(ACCOUNT),
            Utc.timestamp_opt(0, 0).unwrap().fixed_offset(),
        );
        assert_eq!(counts.archived, 1);
        assert_eq!(counts.restored, 1);
        assert!(matches!(
            released.project(&scan(closed.clone()), &closed),
            ArchiveDisposition::Consumed {
                outcome: ArchiveTerminalKind::Restored { .. },
                ..
            }
        ));
    }

    #[test]
    fn let_go_release_stays_terminal_after_a_late_duplicate_archive_event() {
        let (state, mut events, _closed, close) = closed_after_open();
        let archived = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("label-1")),
                recovered: false,
            },
        );
        events.push(archived.stored("athena", 4, 10));
        let archived_state =
            ArchiveJournal::from_events(events.clone(), Vec::new());
        let let_go = terminal_event(
            &archived_state,
            &close,
            ArchiveTerminalKind::LetGo {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("label-1")),
            },
        );
        // The release is ordered before its referenced archive event by
        // timestamp, and the late duplicate arrives after both.
        events.push(let_go.stored("mac", 1, 5));
        events.push(archived.stored("apollo", 1, 20));
        let folded = ArchiveJournal::from_events(events, Vec::new());
        assert_eq!(folded.owned_receipts().count(), 0);
        assert_eq!(folded.archive_history().len(), 1);
        assert_eq!(
            folded.archive_history()[0].state,
            ArchiveHistoryState::LetGo
        );
        let counts = folded.counts_since(
            Some(ACCOUNT),
            Utc.timestamp_opt(0, 0).unwrap().fixed_offset(),
        );
        assert_eq!(counts.archived, 1);
        assert_eq!(counts.let_go, 1);
    }

    #[test]
    fn conflicting_late_archive_holds_one_shot_keep_without_receipt() {
        let (state, mut events, closed, close) = closed_after_open();
        let kept = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Kept {
                reason: "unsaved".to_string(),
            },
        );
        let archive = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", None),
                recovered: true,
            },
        );
        push(&mut events, &kept, "mac", 10);
        push(&mut events, &archive, "athena", 20);
        let folded = ArchiveJournal::from_events(events, Vec::new());
        assert_eq!(folded.owned_receipts().count(), 0);
        assert!(!folded.thread_is_safe(&key()));
        assert!(matches!(
            folded.project(&scan(closed.clone()), &closed),
            ArchiveDisposition::Held
        ));
    }

    #[test]
    fn reopen_rearms_a_new_generation_and_stale_open_cannot_rearm_it_again() {
        let (mut state, mut events, _closed, first_close) = closed_after_open();
        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        let second_open = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &second_open, "mac", 4);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        let closed_again = evidence(key(), 'X');
        let closed_scan = scan(closed_again.clone());
        let second_close = state
            .event_for_close(
                &closed_scan,
                &closed_again,
                label("bob/closed", None),
            )
            .unwrap();
        push(&mut events, &second_close, "mac", 5);
        // The stale first open is sorted after generation two. It references
        // no prior generation and must not replace the current close.
        let first_open = events
            .iter()
            .find(|event| event.kind == OBSERVED_KIND)
            .unwrap()
            .clone();
        let stale = StoredEvent {
            ts: Utc.timestamp_opt(99, 0).unwrap().fixed_offset(),
            host: "apollo".to_string(),
            path: "apollo.jsonl".to_string(),
            line: 1,
            ..first_open
        };
        events.push(stale);
        let folded = ArchiveJournal::from_events(events, Vec::new());
        let current = folded.unconsumed_close(&key()).unwrap();
        assert_ne!(current.generation, first_close.generation);
        assert_eq!(
            current.generation,
            parse_close_value(&second_close.value, None)
                .unwrap()
                .close
                .generation
        );
    }

    #[test]
    fn recent_projection_keeps_each_generation_but_counts_one_thread_once() {
        let (mut state, mut events, _closed, first_close) = closed_after_open();
        let first_archive = terminal_event(
            &state,
            &first_close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("old-label")),
                recovered: true,
            },
        );
        push(&mut events, &first_archive, "athena", 100_000);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "athena", 100_001);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let closed = evidence(key(), 'x');
        let closed_scan = scan(closed.clone());
        let second_close_event = state
            .event_for_close(&closed_scan, &closed, label("bob/closed", None))
            .unwrap();
        let second_close = parse_close_value(&second_close_event.value, None)
            .unwrap()
            .close;
        push(&mut events, &second_close_event, "athena", 100_002);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let second_archive = terminal_event(
            &state,
            &second_close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["bb02".to_string()],
                label: label("bob/closed", Some("new-label")),
                recovered: false,
            },
        );
        push(&mut events, &second_archive, "athena", 100_003);
        let folded = ArchiveJournal::from_events(events, Vec::new());
        let owned: Vec<_> = folded.owned_receipts().collect();
        assert_eq!(owned.len(), 2);
        assert!(owned.iter().any(|receipt| {
            receipt.label.id.as_deref() == Some("old-label")
        }));
        assert!(owned.iter().any(|receipt| {
            receipt.label.id.as_deref() == Some("new-label")
        }));

        let mut index = MailIndex::default();
        for account in [ACCOUNT, "other@example.com"] {
            index.by_identity.insert(
                (account.to_ascii_lowercase(), THREAD.to_string()),
                MailIndexEntry {
                    account: account.to_string(),
                    thread_id: THREAD.to_string(),
                    path: format!("mail/{account}.md"),
                    title: String::new(),
                    who: String::new(),
                    label: String::new(),
                    started: String::new(),
                    updated: String::new(),
                    messages: 0,
                    turn: None,
                    turn_since: None,
                },
            );
        }
        let listed = super::super::archive_projection::list_receipts(
            &folded, &index, None,
        )
        .unwrap();
        let primary = listed
            .get(&(ACCOUNT.to_string(), THREAD.to_string()))
            .unwrap();
        assert_eq!(primary.len(), 2);
        assert_eq!(primary[0].label.id.as_deref(), Some("new-label"));
        assert!(
            listed
                .get(&("other@example.com".to_string(), THREAD.to_string()))
                .is_none(),
            "thread IDs from another account never inherit this receipt"
        );

        let recovered_only = super::super::archive_projection::recent_archive(
            &folded,
            ACCOUNT,
            chrono_tz::UTC,
            Utc.timestamp_opt(100_001, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(recovered_only.count, 1);
        assert_eq!(
            recovered_only.items[0].archive_event_id,
            first_archive.value["event_id"].as_str().unwrap()
        );
        assert_eq!(recovered_only.items[0].state, "archived");
        assert_eq!(
            recovered_only.items[0].label.id.as_deref(),
            Some("old-label")
        );

        let recent = super::super::archive_projection::recent_archive(
            &folded,
            ACCOUNT,
            chrono_tz::UTC,
            Utc.timestamp_opt(200_000, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(recent.count, 1);
        assert_eq!(recent.items.len(), 1);
        assert_eq!(
            recent.items[0].archive_event_id,
            second_archive.value["event_id"].as_str().unwrap()
        );
        assert_eq!(recent.items[0].state, "archived");
        assert_eq!(recent.items[0].message_count, 1);
        assert_eq!(recent.items[0].label.id.as_deref(), Some("new-label"));
        let other_account = super::super::archive_projection::recent_archive(
            &folded,
            "other@example.com",
            chrono_tz::UTC,
            Utc.timestamp_opt(200_000, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(other_account.count, 0);
    }

    #[test]
    fn disable_reenable_and_label_rename_rebaseline_without_rewriting_receipts()
    {
        let (mut state, mut events, closed, close) = closed_after_open();
        let archived = terminal_event(
            &state,
            &close,
            ArchiveTerminalKind::Archived {
                message_ids: vec!["aa01".to_string()],
                label: label("bob/closed", Some("old-label-id")),
                recovered: false,
            },
        );
        push(&mut events, &archived, "athena", 4);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());

        let disabled = state
            .event_for_enablement(ACCOUNT, false, label("ignored", None))
            .unwrap();
        push(&mut events, &disabled, "athena", 5);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert!(!state.is_enabled(ACCOUNT));

        let reenabled = state
            .event_for_enablement(ACCOUNT, true, label("new/closed", None))
            .unwrap();
        push(&mut events, &reenabled, "mac", 6);
        state = ArchiveJournal::from_events(events.clone(), Vec::new());
        assert!(state.is_enabled(ACCOUNT));
        assert_eq!(
            state.project(&scan(closed.clone()), &closed),
            ArchiveDisposition::HistoricalBacklog
        );
        let receipt = state.owned_receipts().next().unwrap().clone();
        assert_eq!(receipt.label.name, "bob/closed");
        assert_eq!(receipt.label.id.as_deref(), Some("old-label-id"));

        let open = evidence(closed.key.clone(), ' ');
        let open_scan = scan(open.clone());
        let open_event = state.event_for_open(&open_scan, &open).unwrap();
        push(&mut events, &open_event, "apollo", 7);
        state = ArchiveJournal::from_events(events, Vec::new());
        assert!(state.event_for_open(&open_scan, &open).is_none());
    }

    #[test]
    fn malformed_archive_state_holds_its_thread_and_warnings_stay_body_free() {
        let (state, mut events, closed, _) = closed_after_open();
        let malformed = StoredEvent {
            ts: Utc.timestamp_opt(50, 0).unwrap().fixed_offset(),
            host: "badhost".to_string(),
            path: "badhost.jsonl".to_string(),
            line: 1,
            kind: CLOSED_KIND.to_string(),
            value: json!({
                "value": {
                    "account": ACCOUNT,
                    "thread_id": THREAD,
                    "generation": "not-a-digest",
                    "task_text": "PRIVATE TASK PHRASE",
                }
            }),
        };
        events.push(malformed);
        let folded = ArchiveJournal::from_events(events, Vec::new());
        assert!(!folded.thread_is_safe(&key()));
        assert!(matches!(
            folded.project(&scan(closed.clone()), &closed),
            ArchiveDisposition::Held
        ));
        assert!(folded
            .warnings()
            .iter()
            .all(|warning| !warning.contains("PRIVATE TASK PHRASE")));
        assert!(state.thread_is_safe(&key()));
    }

    #[test]
    fn repeated_observations_do_not_append_and_persisted_events_are_body_free()
    {
        let (state, enable_events, _) = enabled();
        let open = evidence(key(), ' ');
        let open_scan = scan(open.clone());
        let planned = state.event_for_open(&open_scan, &open).unwrap();
        let vault = tempfile::tempdir().unwrap();
        ArchiveJournal::append(
            vault.path(),
            "athena",
            &PlannedArchiveEvent {
                kind: ENABLED_KIND,
                value: enable_events[0].value.get("value").unwrap().clone(),
            },
        )
        .unwrap();
        ArchiveJournal::append(vault.path(), "athena", &planned).unwrap();
        let loaded = ArchiveJournal::load(vault.path()).unwrap();
        assert!(loaded.event_for_open(&open_scan, &open).is_none());
        let text = fs::read_to_string(pref_journal::host_journal_path(
            vault.path(),
            NAMESPACE,
            "athena",
        ))
        .unwrap();
        assert!(!text.contains("PRIVATE TASK PHRASE"));
        assert!(!text.contains("subject"));
        assert_eq!(text.lines().count(), 2);
    }
}
