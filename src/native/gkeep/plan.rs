//! Keep → vault classification and selection (pure).
//!
//! Every Home note becomes one [`PlannedNote`] with a state, an action,
//! and — for skips — a reason. The checks run in the contract order:
//! archived, empty, pinned, shared, then the ledger/journal lookups that
//! separate pending (archive only) from revised and new (write, then
//! archive). Explicit `-i` selection overrides the pinned and shared
//! skips; nothing overrides empty or archived.

use std::collections::{BTreeMap, BTreeSet};

use super::ledger::{DuplicateGroup, Journal, Ledger};
use super::model::{note_ref, KeepNote};
use super::GkeepError;

/// Planner inputs: the pull/list selection flags.
#[derive(Debug, Clone, Default)]
pub(super) struct PlanOptions {
    pub(super) include_pinned: bool,
    pub(super) include_shared: bool,
    /// Raw `-i` values: full Keep ids or REF prefixes.
    pub(super) ids: Vec<String>,
    /// Take only the first N actionable notes, oldest first.
    pub(super) limit: Option<usize>,
}

/// The classification of one Keep note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteState {
    New,
    Pending,
    Revised,
    Empty,
    Pinned,
    Shared,
    Archived,
}

impl NoteState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Pending => "pending",
            Self::Revised => "revised",
            Self::Empty => "empty",
            Self::Pinned => "pinned",
            Self::Shared => "shared",
            Self::Archived => "archived",
        }
    }

    /// `new`, `pending`, and `revised` notes drive writes or archives.
    pub(super) fn is_actionable(self) -> bool {
        matches!(self, Self::New | Self::Pending | Self::Revised)
    }
}

/// What `pull` should do with a planned note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanAction {
    Write,
    WriteRevision,
    ArchiveOnly,
    Skip,
}

impl PlanAction {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::WriteRevision => "write_revision",
            Self::ArchiveOnly => "archive_only",
            Self::Skip => "skip",
        }
    }
}

/// One classified note, in oldest-first plan order.
#[derive(Debug, Clone)]
pub(super) struct PlannedNote {
    pub(super) note: KeepNote,
    pub(super) ref_: String,
    pub(super) state: NoteState,
    pub(super) action: PlanAction,
    /// Why a skipped note stays in Keep (`pinned`, `shared`, …).
    pub(super) skip_reason: Option<String>,
}

/// The classified plan plus vault-integrity warnings and counts.
#[derive(Debug, Clone)]
pub(super) struct Plan {
    pub(super) notes: Vec<PlannedNote>,
    pub(super) duplicates: Vec<DuplicateGroup>,
    pub(super) summary: PlanSummary,
}

/// Per-state counts over the final (post-limit) plan notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PlanSummary {
    pub(super) new: usize,
    pub(super) pending: usize,
    pub(super) revised: usize,
    pub(super) skipped: usize,
    pub(super) archived: usize,
    pub(super) duplicates: usize,
}

/// Classify `notes` against the ledger and journal.
///
/// Notes are ordered oldest first by Keep `created`, ties broken by id.
/// When `opts.ids` is non-empty the plan narrows to the selected notes
/// (which also lifts the pinned/shared skips for them); unresolvable
/// `-i` values select nothing here — callers validate with [`resolve_ids`]
/// first for the exit-2 errors. `limit` keeps the first N actionable
/// notes; skips and archived notes are always kept.
pub(super) fn classify(
    notes: &[KeepNote],
    ledger: &Ledger,
    journal: &Journal,
    opts: &PlanOptions,
) -> Plan {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|left, right| {
        (created_key(&notes[*left]), notes[*left].id.as_str())
            .cmp(&(created_key(&notes[*right]), notes[*right].id.as_str()))
    });

    let selected = selected_indices(notes, &opts.ids);
    let mut planned: Vec<PlannedNote> = order
        .into_iter()
        .filter(|index| selected.as_ref().is_none_or(|set| set.contains(index)))
        .map(|index| {
            let explicit = selected.is_some();
            classify_one(&notes[index], explicit, ledger, journal, opts)
        })
        .collect();

    if let Some(limit) = opts.limit {
        let mut kept_actionable = 0;
        planned.retain(|planned| {
            if !planned.state.is_actionable() {
                return true;
            }
            if kept_actionable < limit {
                kept_actionable += 1;
                true
            } else {
                false
            }
        });
    }

    let duplicates = ledger.duplicates();
    let mut summary = PlanSummary {
        duplicates: duplicates.len(),
        ..PlanSummary::default()
    };
    for planned in &planned {
        match planned.state {
            NoteState::New => summary.new += 1,
            NoteState::Pending => summary.pending += 1,
            NoteState::Revised => summary.revised += 1,
            NoteState::Archived => summary.archived += 1,
            NoteState::Empty | NoteState::Pinned | NoteState::Shared => {
                summary.skipped += 1;
            }
        }
    }

    Plan {
        notes: planned,
        duplicates,
        summary,
    }
}

fn classify_one(
    note: &KeepNote,
    selected: bool,
    ledger: &Ledger,
    journal: &Journal,
    opts: &PlanOptions,
) -> PlannedNote {
    let mut planned = PlannedNote {
        note: note.clone(),
        ref_: note_ref(&note.id),
        state: NoteState::New,
        action: PlanAction::Write,
        skip_reason: None,
    };
    let skip = |state: NoteState| {
        let mut skipped = planned.clone();
        skipped.state = state;
        skipped.action = PlanAction::Skip;
        skipped.skip_reason = Some(state.as_str().to_string());
        skipped
    };
    if note.archived {
        return skip(NoteState::Archived);
    }
    if is_empty(note) {
        return skip(NoteState::Empty);
    }
    if note.pinned && !opts.include_pinned && !selected {
        return skip(NoteState::Pinned);
    }
    if note.shared && !opts.include_shared && !selected {
        return skip(NoteState::Shared);
    }
    let fp = note.content.fingerprint();
    if ledger.has(&note.id, &fp) || journal.has(&note.id, &fp) {
        planned.state = NoteState::Pending;
        planned.action = PlanAction::ArchiveOnly;
        return planned;
    }
    if ledger.has_id(&note.id) || journal.has_id(&note.id) {
        planned.state = NoteState::Revised;
        planned.action = PlanAction::WriteRevision;
        return planned;
    }
    planned
}

/// No title, text, items, or attachments. Attachments count even when
/// their OCR text is missing: an image note is never empty.
fn is_empty(note: &KeepNote) -> bool {
    let blank = |text: &str| text.trim().is_empty();
    blank(&note.content.title)
        && note.content.text.lines().all(|line| blank(line))
        && blank(&note.content.text)
        && note.content.items.iter().all(|item| blank(&item.text))
        && note.attachments.is_empty()
}

/// Lenient selection: exact ids plus REF-prefix hits. Unknown and
/// ambiguous values select nothing; [`resolve_ids`] reports those.
fn selected_indices(
    notes: &[KeepNote],
    ids: &[String],
) -> Option<BTreeSet<usize>> {
    if ids.is_empty() {
        return None;
    }
    let mut selected = BTreeSet::new();
    for raw in ids {
        for hit in match_id(notes, raw) {
            selected.insert(hit);
        }
    }
    Some(selected)
}

/// Indices matching one `-i` value: an exact Keep id, else REF prefixes.
fn match_id(notes: &[KeepNote], raw: &str) -> Vec<usize> {
    if let Some(exact) = notes.iter().position(|note| note.id == raw) {
        return vec![exact];
    }
    let lowered = raw.to_lowercase();
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| note_ref(&note.id).starts_with(&lowered))
        .map(|(index, _)| index)
        .collect()
}

/// Strict `-i` resolution: an exact Keep id always wins, otherwise the
/// value is a REF prefix. Zero or several matches are exit-2 errors that
/// list the candidates.
pub(super) fn resolve_ids(
    notes: &[KeepNote],
    ids: &[String],
) -> Result<Vec<usize>, GkeepError> {
    let mut resolved = Vec::new();
    for raw in ids {
        if let Some(exact) = notes.iter().position(|note| note.id == *raw) {
            if !resolved.contains(&exact) {
                resolved.push(exact);
            }
            continue;
        }
        let hits = match_id(notes, raw);
        match hits.len() {
            0 => {
                return Err(GkeepError::setup(
                    "unknown_id",
                    format!("unknown note id: {raw}"),
                )
                .with_hint("run `bob gkeep list` to see REF ids"));
            }
            1 => {
                if !resolved.contains(&hits[0]) {
                    resolved.push(hits[0]);
                }
            }
            _ => {
                let mut candidates: BTreeMap<String, &str> = BTreeMap::new();
                for hit in hits {
                    candidates.insert(
                        note_ref(&notes[hit].id),
                        notes[hit].id.as_str(),
                    );
                }
                let listed = candidates
                    .iter()
                    .map(|(ref_, id)| format!("{ref_} ({id})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(GkeepError::setup(
                    "ambiguous_id",
                    format!(
                        "ambiguous note id `{raw}` matches {} notes: {listed}",
                        candidates.len()
                    ),
                )
                .with_hint("use a full Keep id or a longer REF prefix"));
            }
        }
    }
    Ok(resolved)
}

/// Oldest-first sort key: the Keep `created` timestamp, with unparseable
/// values sorting before everything (they still break ties by id).
fn created_key(note: &KeepNote) -> i64 {
    note.created_local()
        .map(|time| time.timestamp())
        .unwrap_or(i64::MIN)
}

#[cfg(test)]
mod tests {
    use super::super::ledger::{JournalEvent, JournalRecord, LedgerEntry};
    use super::super::model::{KeepContent, KeepNoteKind};
    use super::*;

    fn note(id: &str, created: &str) -> KeepNote {
        KeepNote {
            id: id.to_string(),
            server_id: None,
            kind: KeepNoteKind::Note,
            content: KeepContent {
                title: format!("title {id}"),
                text: String::new(),
                items: Vec::new(),
            },
            pinned: false,
            archived: false,
            shared: false,
            labels: Vec::new(),
            attachments: Vec::new(),
            created: created.to_string(),
            edited: created.to_string(),
            url: None,
        }
    }

    fn entry(id: &str, fp: &str) -> LedgerEntry {
        LedgerEntry {
            id: id.to_string(),
            fp: fp.to_string(),
            path: "gkeep_inbox.md".to_string(),
            line: 1,
        }
    }

    fn written(id: &str, fp: &str) -> JournalRecord {
        JournalRecord {
            ts: "2026-09-28T00:00:00Z".to_string(),
            event: JournalEvent::Written,
            id: id.to_string(),
            ref_: note_ref(id),
            fp: fp.to_string(),
            path: "gkeep_inbox.md".to_string(),
            commit: None,
            status: None,
        }
    }

    fn empty_plan() -> (Ledger, Journal, PlanOptions) {
        (
            Ledger {
                entries: Vec::new(),
            },
            Journal {
                records: Vec::new(),
                skipped: 0,
            },
            PlanOptions::default(),
        )
    }

    fn states(plan: &Plan) -> Vec<(&str, &str, &str)> {
        plan.notes
            .iter()
            .map(|planned| {
                (
                    planned.note.id.as_str(),
                    planned.state.as_str(),
                    planned.action.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn fresh_note_is_new() {
        let (ledger, journal, opts) = empty_plan();
        let notes = vec![note("n1", "2026-09-27T21:14:03Z")];
        let plan = classify(&notes, &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "new", "write")]);
        assert_eq!(
            plan.summary,
            PlanSummary {
                new: 1,
                ..PlanSummary::default()
            },
        );
    }

    #[test]
    fn ledger_hit_with_same_fingerprint_is_pending() {
        let note = note("n1", "2026-09-27T21:14:03Z");
        let fp = note.content.fingerprint();
        let (mut ledger, journal, opts) = empty_plan();
        ledger.entries.push(entry("n1", &fp));
        let plan =
            classify(std::slice::from_ref(&note), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "pending", "archive_only")]);
        assert_eq!(plan.notes[0].skip_reason, None);
        assert_eq!(plan.summary.pending, 1);
    }

    #[test]
    fn ledger_hit_with_other_fingerprint_is_revised() {
        let note = note("n1", "2026-09-27T21:14:03Z");
        let (mut ledger, journal, opts) = empty_plan();
        ledger.entries.push(entry("n1", "000000000000"));
        let plan =
            classify(std::slice::from_ref(&note), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "revised", "write_revision")]);
        assert_eq!(plan.summary.revised, 1);
    }

    #[test]
    fn journal_written_record_is_a_backstop() {
        let note = note("n1", "2026-09-27T21:14:03Z");
        let fp = note.content.fingerprint();
        let (ledger, mut journal, opts) = empty_plan();
        journal.records.push(written("n1", &fp));
        let plan =
            classify(std::slice::from_ref(&note), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "pending", "archive_only")]);

        let (ledger, mut journal, opts) = empty_plan();
        journal.records.push(written("n1", "ffffffffffff"));
        let plan =
            classify(std::slice::from_ref(&note), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "revised", "write_revision")]);
    }

    #[test]
    fn empty_note_is_skipped() {
        let (ledger, journal, opts) = empty_plan();
        let mut blank = note("n1", "2026-09-27T21:14:03Z");
        blank.content.title = "   ".to_string();
        let plan =
            classify(std::slice::from_ref(&blank), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "empty", "skip")]);
        assert_eq!(plan.notes[0].skip_reason.as_deref(), Some("empty"));
        assert_eq!(plan.summary.skipped, 1);
    }

    #[test]
    fn pinned_and_shared_notes_stay_unless_included_or_selected() {
        let (ledger, journal, _) = empty_plan();
        let mut pinned = note("pinned", "2026-09-27T21:14:03Z");
        pinned.pinned = true;
        let mut shared = note("shared", "2026-09-27T21:14:03Z");
        shared.shared = true;
        let notes = vec![pinned, shared];

        let plan = classify(&notes, &ledger, &journal, &PlanOptions::default());
        assert_eq!(
            states(&plan),
            vec![("pinned", "pinned", "skip"), ("shared", "shared", "skip")],
        );

        let opts = PlanOptions {
            include_pinned: true,
            include_shared: true,
            ..PlanOptions::default()
        };
        let plan = classify(&notes, &ledger, &journal, &opts);
        assert_eq!(
            states(&plan),
            vec![("pinned", "new", "write"), ("shared", "new", "write")],
        );

        // Explicit selection narrows to the note and lifts its skip.
        let opts = PlanOptions {
            ids: vec!["pinned".to_string()],
            ..PlanOptions::default()
        };
        let plan = classify(&notes, &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("pinned", "new", "write")]);

        // Unknown selection values select nothing (strict callers
        // reject them through `resolve_ids` before planning).
        let opts = PlanOptions {
            ids: vec!["nope".to_string()],
            ..PlanOptions::default()
        };
        let plan = classify(&notes, &ledger, &journal, &opts);
        assert!(plan.notes.is_empty());
    }

    #[test]
    fn pinned_beats_pending() {
        let mut pinned = note("n1", "2026-09-27T21:14:03Z");
        pinned.pinned = true;
        let fp = pinned.content.fingerprint();
        let (mut ledger, journal, opts) = empty_plan();
        ledger.entries.push(entry("n1", &fp));
        let plan =
            classify(std::slice::from_ref(&pinned), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "pinned", "skip")]);
    }

    #[test]
    fn archived_notes_get_the_archived_state() {
        let (ledger, journal, opts) = empty_plan();
        let mut archived = note("n1", "2026-09-27T21:14:03Z");
        archived.archived = true;
        let plan =
            classify(std::slice::from_ref(&archived), &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("n1", "archived", "skip")]);
        assert_eq!(plan.notes[0].skip_reason.as_deref(), Some("archived"));
        assert_eq!(plan.summary.archived, 1);
    }

    #[test]
    fn notes_order_oldest_first_with_id_tiebreak() {
        let (ledger, journal, opts) = empty_plan();
        let notes = vec![
            note("b-new", "2026-09-27T21:14:03Z"),
            note("a-new", "2026-09-27T21:14:03Z"),
            note("old", "2026-09-20T08:00:00Z"),
        ];
        let plan = classify(&notes, &ledger, &journal, &opts);
        let ids: Vec<&str> =
            plan.notes.iter().map(|p| p.note.id.as_str()).collect();
        assert_eq!(ids, vec!["old", "a-new", "b-new"]);
    }

    #[test]
    fn limit_counts_only_actionable_notes() {
        let (ledger, journal, _) = empty_plan();
        let mut pinned = note("pinned", "2026-09-20T08:00:00Z");
        pinned.pinned = true;
        let notes = vec![
            pinned,
            note("first", "2026-09-21T08:00:00Z"),
            note("second", "2026-09-22T08:00:00Z"),
        ];
        let opts = PlanOptions {
            limit: Some(1),
            ..PlanOptions::default()
        };
        let plan = classify(&notes, &ledger, &journal, &opts);
        // The pinned skip survives; only the first actionable note is kept.
        assert_eq!(
            states(&plan),
            vec![("pinned", "pinned", "skip"), ("first", "new", "write")],
        );
        assert_eq!(plan.summary.new, 1);
        assert_eq!(plan.summary.skipped, 1);
    }

    #[test]
    fn id_filter_narrows_to_selected_notes() {
        let (ledger, journal, _) = empty_plan();
        let notes = vec![
            note("keep-a", "2026-09-21T08:00:00Z"),
            note("keep-b", "2026-09-22T08:00:00Z"),
        ];
        let opts = PlanOptions {
            ids: vec!["keep-b".to_string()],
            ..PlanOptions::default()
        };
        let plan = classify(&notes, &ledger, &journal, &opts);
        assert_eq!(states(&plan), vec![("keep-b", "new", "write")]);
    }

    #[test]
    fn duplicates_flow_into_the_plan() {
        let (mut ledger, journal, opts) = empty_plan();
        ledger.entries.push(entry("n1", "0123456789ab"));
        ledger.entries.push(LedgerEntry {
            path: "done/old.md".to_string(),
            line: 9,
            ..entry("n1", "0123456789ab")
        });
        let plan = classify(&[], &ledger, &journal, &opts);
        assert_eq!(plan.duplicates.len(), 1);
        assert_eq!(
            plan.duplicates[0].locations,
            vec!["gkeep_inbox.md:1".to_string(), "done/old.md:9".to_string()],
        );
        assert_eq!(plan.summary.duplicates, 1);
    }

    #[test]
    fn resolve_ids_prefers_exact_ids() {
        let notes = vec![
            note("keep-a", "2026-09-21T08:00:00Z"),
            note("keep-b", "2026-09-22T08:00:00Z"),
        ];
        assert_eq!(
            resolve_ids(&notes, &["keep-b".to_string()]).expect("exact"),
            vec![1],
        );
        let full_ref = note_ref("keep-a");
        assert_eq!(resolve_ids(&notes, &[full_ref]).expect("ref"), vec![0],);
    }

    #[test]
    fn resolve_ids_rejects_unknown_and_ambiguous() {
        let notes = vec![
            note("keep-a", "2026-09-21T08:00:00Z"),
            note("keep-b", "2026-09-22T08:00:00Z"),
        ];
        let error =
            resolve_ids(&notes, &["missing".to_string()]).expect_err("unknown");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("unknown note id"));

        // Find a REF prefix shared by two generated notes.
        let candidates: Vec<KeepNote> = (0..60)
            .map(|n| note(&format!("amb-{n}"), "2026-09-21T08:00:00Z"))
            .collect();
        let mut shared_prefix = None;
        for width in [3, 2, 1] {
            let mut groups: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for candidate in &candidates {
                *groups
                    .entry(note_ref(&candidate.id)[..width].to_string())
                    .or_default() += 1;
            }
            if let Some(prefix) = groups
                .into_iter()
                .find(|(_, count)| *count > 1)
                .map(|(p, _)| p)
            {
                shared_prefix = Some(prefix);
                break;
            }
        }
        let prefix = shared_prefix.expect("a shared prefix exists");
        let error =
            resolve_ids(&candidates, &[prefix.clone()]).expect_err("ambiguous");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("ambiguous"));
        assert!(error.message().contains(&prefix));
    }
}
